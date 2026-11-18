//! The one TLS and caller-identity policy every mesh ingress shares.
//!
//! Two processes terminate mesh TLS today - `mycelium-frontdoor` for `.myc`
//! names on a node, and `palisade` for public ones - and a third copy of "who is
//! calling" is how a fleet ends up with a bearer token, a per-node token and a
//! shared secret for the same question. So the policy lives here, once, and both
//! ingress processes depend on it.
//!
//! Three decisions this crate fixes for every caller:
//!
//! - **Client certificates are optional, not required.** A single HTTP listener
//!   serves phones (no certificate) and fleet nodes (certificate) on the same
//!   port, and a TLS handshake has one policy per port. Whether an identity is
//!   *sufficient* is the application's per-route decision; this crate only makes
//!   it available.
//! - **The trust anchor is the mesh CA**, never an ambient store. A node is a
//!   node because the CA that enrolled it signed its certificate.
//! - **The forwarded identity names the certificate.** It is the leaf's SHA-256,
//!   not the daemon's node id, because the two differ (a node's `pki/node.pem`
//!   digest is not what `mycelium gateway status` reports as `local_node`). A
//!   header that quietly meant "node id" would be wrong in a way nobody could
//!   see.

use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::WebPkiClientVerifier;
use rustls::{RootCertStore, ServerConfig};
use sha2::{Digest, Sha256};

/// The header a verified caller is forwarded under.
///
/// The name says *certificate* on purpose. It is not the daemon's node id, and
/// nothing downstream should treat it as one.
pub const CLIENT_CERT_HEADER: &str = "x-mycelium-cert-sha256";

/// What an ingress needs to terminate TLS.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerTls {
    pub certificate: PathBuf,
    pub key: PathBuf,
    /// CA a client certificate must chain to. `None` keeps today's behaviour:
    /// the listener proves its own identity and learns nothing back.
    pub client_ca: Option<PathBuf>,
}

impl ServerTls {
    pub fn new(certificate: impl Into<PathBuf>, key: impl Into<PathBuf>) -> Self {
        Self {
            certificate: certificate.into(),
            key: key.into(),
            client_ca: None,
        }
    }

    pub fn with_client_ca(mut self, ca: impl Into<PathBuf>) -> Self {
        self.client_ca = Some(ca.into());
        self
    }
}

#[derive(Debug)]
pub enum IngressError {
    Io(String),
    Config(String),
}

impl std::fmt::Display for IngressError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(message) | Self::Config(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for IngressError {}

/// The one crypto provider this crate builds TLS objects with. A process that
/// links both rustls providers has no process-level default, so letting rustls
/// infer one panics; naming it keeps the listener and the client verifier on the
/// same provider even when a binary in the same graph enables the other one.
fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::aws_lc_rs::default_provider())
}

/// Build the listener's TLS policy.
pub fn server_config(tls: &ServerTls) -> Result<Arc<ServerConfig>, IngressError> {
    let certificates = load_certificates(&tls.certificate)?;
    let key = load_key(&tls.key)?;
    // The provider is named rather than inferred: a process that happens to have
    // two crypto providers compiled in makes `ServerConfig::builder()` panic,
    // and a panic inside a spawned listener is a server that never serves.
    let provider = provider();
    let base = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|error| IngressError::Config(format!("TLS versions: {error}")))?;
    let builder = match &tls.client_ca {
        Some(ca) => {
            let mut roots = RootCertStore::empty();
            for certificate in load_certificates(ca)? {
                roots
                    .add(certificate)
                    .map_err(|error| IngressError::Config(format!("{}: {error}", ca.display())))?;
            }
            if roots.is_empty() {
                return Err(IngressError::Config(format!(
                    "{} contains no CA certificate",
                    ca.display()
                )));
            }
            base.with_client_cert_verifier(client_verifier(roots)?)
        }
        None => base.with_no_client_auth(),
    };
    let mut config = builder
        .with_single_cert(certificates, key)
        .map_err(|error| IngressError::Config(format!("{}: {error}", tls.certificate.display())))?;
    // Both ingresses speak HTTP, and both want the browser to pick h2 when it
    // can: the forwarder is a stream relay either way.
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}

fn client_verifier(
    roots: RootCertStore,
) -> Result<Arc<dyn rustls::server::danger::ClientCertVerifier>, IngressError> {
    // `allow_unauthenticated` is the whole point: a client without a
    // certificate is today's anonymous caller, and one with a certificate the
    // mesh CA signed is a node. Requiring one would take every browser off the
    // public listener.
    WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider())
        .allow_unauthenticated()
        .build()
        .map_err(|error| IngressError::Config(format!("client verifier: {error}")))
}

/// Where a node's own enrolment lives, for callers that want the default.
pub fn default_client_ca() -> Option<PathBuf> {
    let home = std::env::var_os("MYCELIUM_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".mycelium")))?;
    let ca = home.join("pki/ca.pem");
    ca.is_file().then_some(ca)
}

/// The identity a verified caller is forwarded as: the leaf certificate's
/// SHA-256, lowercase hex.
pub fn client_identity(leaf: &CertificateDer<'_>) -> String {
    let digest = Sha256::digest(leaf.as_ref());
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn load_certificates(path: &Path) -> Result<Vec<CertificateDer<'static>>, IngressError> {
    let file = std::fs::File::open(path)
        .map_err(|error| IngressError::Io(format!("{}: {error}", path.display())))?;
    let certificates = rustls_pemfile::certs(&mut BufReader::new(file))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| IngressError::Config(format!("{}: {error}", path.display())))?;
    if certificates.is_empty() {
        return Err(IngressError::Config(format!(
            "{} contains no certificates",
            path.display()
        )));
    }
    Ok(certificates)
}

fn load_key(path: &Path) -> Result<PrivateKeyDer<'static>, IngressError> {
    let file = std::fs::File::open(path)
        .map_err(|error| IngressError::Io(format!("{}: {error}", path.display())))?;
    rustls_pemfile::private_key(&mut BufReader::new(file))
        .map_err(|error| IngressError::Config(format!("{}: {error}", path.display())))?
        .ok_or_else(|| IngressError::Config(format!("{} contains no private key", path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A CA and one leaf signed by it, written to a scratch directory. rcgen is
    /// already a dev-dependency of the frontdoor for exactly this.
    fn scratch(name: &str) -> (PathBuf, PathBuf, PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("mycelium-ingress-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let mut ca_params = rcgen::CertificateParams::new(Vec::new()).unwrap();
        ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        let ca_key = rcgen::KeyPair::generate().unwrap();
        let ca = ca_params.self_signed(&ca_key).unwrap();

        let leaf_key = rcgen::KeyPair::generate().unwrap();
        let leaf_params = rcgen::CertificateParams::new(vec!["node.test.myc".to_string()]).unwrap();
        let leaf = leaf_params.signed_by(&leaf_key, &ca, &ca_key).unwrap();

        let ca_path = dir.join("ca.pem");
        let leaf_path = dir.join("node.pem");
        let key_path = dir.join("node-key.pem");
        std::fs::write(&ca_path, ca.pem()).unwrap();
        std::fs::write(&leaf_path, leaf.pem()).unwrap();
        std::fs::write(&key_path, leaf_key.serialize_pem()).unwrap();
        (ca_path, leaf_path, key_path)
    }

    #[test]
    fn a_certificate_has_one_identity_and_it_says_so() {
        let (_, leaf, _) = scratch("identity");
        let certificates = load_certificates(&leaf).unwrap();
        let identity = client_identity(&certificates[0]);
        assert_eq!(identity.len(), 64, "sha256 hex is 64 characters");
        assert!(identity
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert_eq!(identity, client_identity(&certificates[0]));
        assert_eq!(CLIENT_CERT_HEADER, "x-mycelium-cert-sha256");
    }

    #[test]
    fn a_listener_without_a_client_ca_learns_nothing() {
        let (_, leaf, key) = scratch("no-ca");
        let config = server_config(&ServerTls::new(leaf, key)).unwrap();
        assert!(config.alpn_protocols.contains(&b"h2".to_vec()));
    }

    #[test]
    fn a_listener_with_the_mesh_ca_asks_for_a_certificate() {
        let (ca, leaf, key) = scratch("with-ca");
        let config = server_config(&ServerTls::new(leaf, key).with_client_ca(ca)).unwrap();
        assert!(config.alpn_protocols.contains(&b"http/1.1".to_vec()));
    }

    #[test]
    fn a_missing_ca_is_an_error_not_a_silent_downgrade() {
        let (_, leaf, key) = scratch("missing-ca");
        let error = server_config(&ServerTls::new(leaf, key).with_client_ca("/nonexistent/ca.pem"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("/nonexistent/ca.pem"), "{error}");
    }

    #[test]
    fn the_default_ca_is_the_enrolment_ca_when_it_exists() {
        let (ca, leaf, key) = scratch("default-ca");
        let home = ca.parent().unwrap();
        // pki/ca.pem beside a node.pem is the shape mycelium installs.
        let pki = home.join("pki");
        std::fs::create_dir_all(&pki).unwrap();
        std::fs::copy(&ca, pki.join("ca.pem")).unwrap();
        unsafe { std::env::set_var("MYCELIUM_HOME", home) };
        let found = default_client_ca();
        unsafe { std::env::remove_var("MYCELIUM_HOME") };
        assert_eq!(found, Some(pki.join("ca.pem")));
        let _ = (leaf, key);
    }
}
