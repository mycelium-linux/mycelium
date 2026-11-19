//! Publish one application service lease into the catalog `myceliumd` scans.
//!
//! The file belongs to the application, not to Mycelium: an application writes
//! what it knows about its own listener and the daemon observes it. So a
//! publisher has one job — write our own entry, atomically, and never touch
//! anyone else's. A service that cannot do that reliably is worse than one that
//! never published, because a stale entry keeps routing to a dead port.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Error, ErrorKind, Write as _};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use fpl_resource_observation::{
    Confidence, Provenance, Service, ServiceCatalog, ServiceId, ServiceObservation, SCHEMA_VERSION,
};

/// The provider string the daemon trusts for an application-owned lease. A
/// lease that names another provider is ignored rather than guessed at.
pub const PROVIDER: &str = "mycelium-application-owner-lease";

/// The daemon drops a longer lease, so publishing one is a renewer that never
/// runs: refuse it here rather than write something that cannot route.
pub const MAX_TTL_SECS: u64 = 300;

/// Long enough to survive one missed beat, short enough that a dead process
/// leaves the pool inside a few minutes.
pub const DEFAULT_TTL_SECS: u64 = 240;

/// The attribute a lease lists what it can be asked for in. Space-separated
/// capabilities, so `soil:metrics` and `soil:unibus` can be held at once and a
/// reader can filter on one without a second schema.
pub const CAPABILITIES: &str = "capabilities";

/// One fresh lease, as a reader sees it: the service's name is composed from
/// the kind and the node that owns it, which is what turns a lease into a name
/// an operator or a scraper can dial.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observed {
    pub service_id: ServiceId,
    pub kind: String,
    /// `<kind>.<node_name>` when the lease names its node, else `<kind>.<observer>`.
    pub name: String,
    pub endpoints: BTreeSet<String>,
    pub attributes: BTreeMap<String, String>,
}

impl Observed {
    /// Whether this lease holds `capability`.
    pub fn holds(&self, capability: &str) -> bool {
        self.attributes
            .get(CAPABILITIES)
            .is_some_and(|held| held.split_whitespace().any(|one| one == capability))
    }

    /// The health path this service answers, if it published one.
    pub fn healthz(&self) -> Option<&str> {
        self.attributes.get("healthz").map(String::as_str)
    }

    pub fn readyz(&self) -> Option<&str> {
        self.attributes.get("readyz").map(String::as_str)
    }
}

/// Everything fresh in a catalog, sorted by name. Filtering is the caller's:
/// a lease that does not declare what it holds is still observed, just not
/// scrapeable by capability.
pub fn observe(path: &Path) -> Result<Vec<Observed>, Error> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(Vec::new());
    }
    let catalog: ServiceCatalog = serde_json::from_slice(&bytes)
        .map_err(|error| invalid(format!("{}: {error}", path.display())))?;
    let now = unix_now();
    let mut observed = catalog
        .observations
        .into_iter()
        .filter(|one| one.is_fresh_at(now))
        .map(|one| {
            let node = one
                .value
                .attributes
                .get("node_name")
                .cloned()
                .unwrap_or_else(|| one.provenance.observer.clone());
            Observed {
                service_id: one.service_id.clone(),
                name: format!("{}.{node}", one.value.kind),
                kind: one.value.kind,
                endpoints: one.value.endpoints,
                attributes: one.value.attributes,
            }
        })
        .collect::<Vec<_>>();
    observed.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(observed)
}

/// One service this process is currently offering.
#[derive(Debug, Clone)]
pub struct Lease {
    kind: String,
    labels: BTreeSet<String>,
    endpoints: BTreeSet<String>,
    attributes: BTreeMap<String, String>,
    ttl_secs: u64,
}

impl Lease {
    pub fn new(kind: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            labels: BTreeSet::new(),
            endpoints: BTreeSet::new(),
            attributes: BTreeMap::new(),
            ttl_secs: DEFAULT_TTL_SECS,
        }
    }

    /// Where a client can dial. Prefer the mesh address: a LAN or loopback
    /// address only works from the host that reads it, so a lease that names
    /// one is a record that looks right and routes nowhere.
    pub fn endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoints.insert(endpoint.into());
        self
    }

    pub fn attribute(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.attributes.insert(key.into(), value.into());
        self
    }

    /// The label this service asks the zone for. A lease never spells a full
    /// name: the zone owns the site and the node, and a service that composed
    /// its own name could claim someone else's. Validation is the zone's own
    /// rule, so there is exactly one definition of a legal label.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.labels.insert(label.into());
        self
    }

    pub fn labels(&self) -> &BTreeSet<String> {
        &self.labels
    }

    pub fn ttl(mut self, secs: u64) -> Self {
        self.ttl_secs = secs;
        self
    }

    /// What this service can be asked for. A holder of `soil:metrics` answers
    /// Prometheus on its endpoint, so the lease says so and an observer can find
    /// every scrapeable name without asking each service.
    pub fn capability(mut self, capability: impl Into<String>) -> Self {
        let mut held = self
            .attributes
            .remove(CAPABILITIES)
            .unwrap_or_default()
            .split_whitespace()
            .map(str::to_owned)
            .collect::<BTreeSet<_>>();
        held.insert(capability.into());
        self.attributes.insert(
            CAPABILITIES.into(),
            held.into_iter().collect::<Vec<_>>().join(" "),
        );
        self
    }

    /// The scrape dialect, when the service publishes one (`prometheus`
    /// today). Set it with the metric endpoint's own capability, not by hand.
    pub fn metrics(mut self, dialect: impl Into<String>) -> Self {
        self.attributes.insert("metrics".into(), dialect.into());
        self
    }

    /// Where liveness is answered. A probe that has to guess a path is a probe
    /// that reports a 404 as an outage.
    pub fn healthz(mut self, path: impl Into<String>) -> Self {
        self.attributes.insert("healthz".into(), path.into());
        self
    }

    /// Where readiness is answered, when it differs from liveness.
    pub fn readyz(mut self, path: impl Into<String>) -> Self {
        self.attributes.insert("readyz".into(), path.into());
        self
    }

    pub fn attributes(&self) -> &BTreeMap<String, String> {
        &self.attributes
    }

    pub fn kind(&self) -> &str {
        &self.kind
    }

    pub fn ttl_secs(&self) -> u64 {
        self.ttl_secs
    }

    /// The managed identity: one entry per kind, per node, so a renewal
    /// replaces its own record and two kinds never collide.
    pub fn service_id(&self, node_id: &str) -> ServiceId {
        ServiceId(format!("service/{node_id}/application/{}", self.kind))
    }

    fn validate(&self) -> Result<(), Error> {
        if self.kind.is_empty() || self.kind.len() > 64 {
            return Err(invalid("a lease needs a kind of 1..=64 bytes"));
        }
        if self.endpoints.is_empty() {
            return Err(invalid("a lease with no endpoint names nothing to dial"));
        }
        for label in &self.labels {
            if !mycelium_dns_policy::service_names::valid_label(label) {
                return Err(invalid(format!(
                    "label {label:?} is not one the zone can compose; a label is lowercase, \
                     1..=63 characters, no dot, no leading or trailing hyphen"
                )));
            }
        }
        if self.endpoints.len() > 32 {
            return Err(invalid("a lease carries at most 32 endpoints"));
        }
        for endpoint in &self.endpoints {
            if !endpoint_ok(endpoint) {
                return Err(invalid(format!(
                    "endpoint {endpoint:?} is not scheme://host[:port]; an endpoint is a hint, not a capability, so it carries no userinfo, path, query or fragment"
                )));
            }
        }
        if self.ttl_secs == 0 || self.ttl_secs > MAX_TTL_SECS {
            return Err(invalid(format!(
                "ttl_secs {} must be 1..={MAX_TTL_SECS}; the daemon expires anything longer",
                self.ttl_secs
            )));
        }
        Ok(())
    }

    fn observation(&self, node_id: &str, now: u64) -> ServiceObservation {
        let mut attributes = self.attributes.clone();
        // The daemon checks this: an observation whose `node_id` disagrees with
        // its observer is a claim about somebody else's node.
        attributes.insert("node_id".into(), node_id.to_owned());
        ServiceObservation {
            schema_version: SCHEMA_VERSION,
            service_id: self.service_id(node_id),
            observed_at: now,
            expires_at: now + self.ttl_secs,
            provenance: Provenance {
                provider: PROVIDER.into(),
                observer: node_id.to_owned(),
                source_id: None,
            },
            confidence: Confidence::Strong,
            value: Service {
                kind: self.kind.clone(),
                // Labels, not names: the zone turns `unibus` into
                // `unibus.pris.fpl.myc`, and a lease that spelled a full name
                // would be taking the zone's job.
                names: self.labels.iter().cloned().collect(),
                endpoints: self.endpoints.clone(),
                protocols: BTreeSet::new(),
                formats: BTreeSet::new(),
                attributes,
            },
        }
    }
}

/// A Spore's `[anycast]` claim, read from its manifest.
///
/// The manifest is the interface: mycelium reads a file, not a crate, so the
/// package format and the mesh stay uncoupled. A Spore that wants a name writes
/// the same table it already writes; nothing in soil has to call anything here.
#[derive(Debug, Clone, serde::Deserialize)]
struct SporeManifest {
    #[serde(default)]
    spore: SporeHeader,
    anycast: Option<AnycastClaim>,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
struct SporeHeader {
    id: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct AnycastClaim {
    name: String,
    kind: String,
    #[serde(default)]
    alpns: Vec<String>,
    #[serde(default = "default_ttl")]
    ttl_secs: u64,
}

fn default_ttl() -> u64 {
    DEFAULT_TTL_SECS
}

/// The lease a Spore's manifest asks for, if it asks for one.
///
/// `None` means the package declares no `[anycast]`, which is the default and is
/// not an error: most Spores are local-only. Endpoints are the caller's, because
/// which address a client can dial is one machine's fact and never the
/// package's.
pub fn lease_from_manifest(
    root: impl AsRef<Path>,
    endpoints: &[String],
) -> Result<Option<Lease>, Error> {
    let root = root.as_ref();
    let manifest = if root.is_dir() {
        root.join("spore.toml")
    } else {
        root.to_path_buf()
    };
    let source = std::fs::read_to_string(&manifest)
        .map_err(|error| invalid(format!("read {}: {error}", manifest.display())))?;
    let parsed: SporeManifest = toml::from_str(&source)
        .map_err(|error| invalid(format!("parse {}: {error}", manifest.display())))?;
    let Some(claim) = parsed.anycast else {
        return Ok(None);
    };
    let mut lease = Lease::new(claim.kind).label(claim.name).ttl(claim.ttl_secs);
    // A Spore's gateway answers both probes before the grant table, so they are
    // a property of being served, not something each package opts into. Filling
    // them here is what stops a publisher advertising a service that a probe
    // then reads as a 404 outage.
    lease = lease.healthz("/healthz").readyz("/readyz");
    for endpoint in endpoints {
        lease = lease.endpoint(endpoint.clone());
    }
    if let Some(id) = parsed.spore.id {
        lease = lease.attribute("spore_id", id);
    }
    if !claim.alpns.is_empty() {
        lease = lease.attribute("alpns", claim.alpns.join(" "));
    }
    Ok(Some(lease))
}

/// Every Spore in an installed store that claims a name.
///
/// The store is the standard install location (`~/.local/share/fungos/spores`),
/// one directory per Spore with its manifest beside its module. Scanning it is
/// what makes this configuration-and-install: dropping a package in the store is
/// the whole of the act, and a package that claims nothing is simply skipped.
pub fn leases_from_store(
    root: impl AsRef<Path>,
    endpoints: &[String],
) -> Result<Vec<Lease>, Error> {
    let root = root.as_ref();
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut leases = Vec::new();
    for entry in entries {
        let path = entry?.path();
        if !path.is_dir() || !path.join("spore.toml").is_file() {
            continue;
        }
        if let Some(lease) = lease_from_manifest(&path, endpoints)? {
            leases.push(lease);
        }
    }
    leases.sort_by(|left, right| left.kind().cmp(right.kind()));
    Ok(leases)
}

/// How often to renew: a third of the lease, so two beats can be lost before a
/// client stops seeing this node.
pub fn renew_every(ttl_secs: u64) -> u64 {
    (ttl_secs / 3).max(1)
}

/// The renewer's interval, refused when it cannot keep the lease alive.
pub fn renewal_interval(requested: Option<u64>, ttl_secs: u64) -> Result<u64, Error> {
    let every = requested.unwrap_or_else(|| renew_every(ttl_secs));
    if every == 0 || every >= ttl_secs {
        return Err(invalid(format!(
            "renewing every {every}s is not shorter than the {ttl_secs}s lease; this node would expire between beats"
        )));
    }
    Ok(every)
}

/// One node's view of the catalog file.
#[derive(Debug, Clone)]
pub struct Publisher {
    path: PathBuf,
    node_id: String,
}

impl Publisher {
    /// The file Mycelium already scans, named once so callers cannot disagree
    /// about it.
    pub fn new(home: impl AsRef<Path>, node_id: impl Into<String>) -> Self {
        Self {
            path: home.as_ref().join("application-services.json"),
            node_id: node_id.into(),
        }
    }

    pub fn at(path: impl Into<PathBuf>, node_id: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            node_id: node_id.into(),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn node_id(&self) -> &str {
        &self.node_id
    }

    /// Replace this node's entry for this kind, keeping every other entry.
    pub fn publish(&self, lease: &Lease) -> Result<ServiceId, Error> {
        lease.validate()?;
        let now = unix_now();
        let ours = lease.observation(&self.node_id, now);
        let service_id = ours.service_id.clone();
        let mut catalog = self.read()?;
        catalog.observations.retain(|existing| {
            existing.provenance.observer != self.node_id || existing.service_id != service_id
        });
        catalog.observations.push(ours);
        catalog.normalize();
        self.write(&catalog)?;
        Ok(service_id)
    }

    /// Take this node's entry for `kind` out of the pool. `false` means there
    /// was nothing to withdraw, which is not a failure: a node that never
    /// published, or already drained, is in the state the caller wanted.
    pub fn withdraw(&self, kind: &str) -> Result<bool, Error> {
        if !self.path.exists() {
            return Ok(false);
        }
        let service_id = Lease::new(kind).service_id(&self.node_id);
        let mut catalog = self.read()?;
        let before = catalog.observations.len();
        catalog.observations.retain(|existing| {
            existing.provenance.observer != self.node_id || existing.service_id != service_id
        });
        if catalog.observations.len() == before {
            return Ok(false);
        }
        self.write(&catalog)?;
        Ok(true)
    }

    /// A file we cannot parse is not ours to overwrite: another application's
    /// entries may be in it, and clobbering them would take a healthy service
    /// out of the catalog to publish our own.
    fn read(&self) -> Result<ServiceCatalog, Error> {
        let bytes = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                return Ok(ServiceCatalog {
                    schema_version: SCHEMA_VERSION,
                    observations: Vec::new(),
                })
            }
            Err(error) => return Err(error),
        };
        if bytes.iter().all(u8::is_ascii_whitespace) {
            return Ok(ServiceCatalog {
                schema_version: SCHEMA_VERSION,
                observations: Vec::new(),
            });
        }
        serde_json::from_slice(&bytes).map_err(|error| {
            invalid(format!(
                "{} is not a service catalog ({error}); refusing to overwrite it",
                self.path.display()
            ))
        })
    }

    /// Write through a sibling temp file: a reader either sees the old catalog
    /// or the new one, never half of each.
    fn write(&self, catalog: &ServiceCatalog) -> Result<(), Error> {
        let body = serde_json::to_vec_pretty(catalog)
            .map_err(|error| invalid(format!("serialise the catalog: {error}")))?;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // A privileged writer (a root tunnel publishing its lease) must not
        // change who owns the catalog: the readers are the user who owns the
        // daemon, and a rename that flips ownership to root locks them out on
        // every renewal.
        #[cfg(unix)]
        let owner = std::fs::metadata(&self.path).ok().map(|metadata| {
            use std::os::unix::fs::MetadataExt as _;
            (metadata.uid(), metadata.gid(), metadata.mode() & 0o777)
        });
        let temporary = self.path.with_extension("json.tmp");
        let mut file = std::fs::File::create(&temporary)?;
        file.write_all(&body)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            match owner {
                // Keep what the catalog already had.
                Some((uid, gid, mode)) => {
                    file.set_permissions(std::fs::Permissions::from_mode(mode))?;
                    std::os::unix::fs::chown(&temporary, Some(uid), Some(gid))?;
                }
                // A fresh catalog is private until someone widens it.
                None => file.set_permissions(std::fs::Permissions::from_mode(0o600))?,
            }
        }
        drop(file);
        std::fs::rename(&temporary, &self.path)?;
        Ok(())
    }
}

fn invalid(message: impl Into<String>) -> Error {
    Error::new(ErrorKind::InvalidInput, message.into())
}

/// `scheme://host[:port]`, with nothing that only makes sense in a URL a human
/// clicks.
fn endpoint_ok(endpoint: &str) -> bool {
    let Some((scheme, rest)) = endpoint.split_once("://") else {
        return false;
    };
    if !matches!(scheme, "http" | "https" | "tcp" | "quic" | "iroh") {
        return false;
    }
    !rest.is_empty()
        && !rest.contains('@')
        && !rest.contains('/')
        && !rest.contains('?')
        && !rest.contains('#')
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lease() -> Lease {
        Lease::new("unibus").endpoint("tcp://100.70.187.127:18790")
    }

    fn read(path: &Path) -> ServiceCatalog {
        serde_json::from_slice(&std::fs::read(path).expect("catalog bytes")).expect("catalog")
    }

    #[test]
    fn publish_writes_one_observation_this_node_owns() {
        let home = tempfile::tempdir().unwrap();
        let publisher = Publisher::new(home.path(), "fa259d68a0b40a65");
        let id = publisher.publish(&lease()).unwrap();
        assert_eq!(id.0, "service/fa259d68a0b40a65/application/unibus");
        let catalog = read(publisher.path());
        assert_eq!(catalog.observations.len(), 1);
        let only = &catalog.observations[0];
        assert_eq!(only.provenance.provider, PROVIDER);
        assert_eq!(only.provenance.observer, "fa259d68a0b40a65");
        assert_eq!(
            only.value.attributes.get("node_id").map(String::as_str),
            Some("fa259d68a0b40a65")
        );
        assert_eq!(only.value.kind, "unibus");
        assert!(only.is_fresh_at(only.observed_at));
    }

    #[test]
    fn renewing_replaces_our_entry_instead_of_growing_the_file() {
        let home = tempfile::tempdir().unwrap();
        let publisher = Publisher::new(home.path(), "node-a");
        publisher.publish(&lease()).unwrap();
        publisher.publish(&lease()).unwrap();
        publisher.publish(&lease()).unwrap();
        assert_eq!(read(publisher.path()).observations.len(), 1);
    }

    #[test]
    fn another_application_entry_survives_our_writes() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("application-services.json");
        let foreign = Lease::new("geode")
            .endpoint("https://100.0.0.9:8443")
            .ttl(120);
        Publisher::at(&path, "node-b").publish(&foreign).unwrap();

        let ours = Publisher::at(&path, "node-a");
        ours.publish(&lease()).unwrap();
        assert_eq!(read(&path).observations.len(), 2);
        assert!(ours.withdraw("unibus").unwrap());
        let remaining = read(&path);
        assert_eq!(remaining.observations.len(), 1);
        assert_eq!(remaining.observations[0].provenance.observer, "node-b");
    }

    #[test]
    fn withdrawing_something_we_never_published_is_not_an_error() {
        let home = tempfile::tempdir().unwrap();
        let publisher = Publisher::new(home.path(), "node-a");
        assert!(!publisher.withdraw("unibus").unwrap());
    }

    #[test]
    fn a_lease_longer_than_the_daemon_accepts_is_refused() {
        let home = tempfile::tempdir().unwrap();
        let publisher = Publisher::new(home.path(), "node-a");
        let error = publisher.publish(&lease().ttl(301)).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::InvalidInput);
        assert!(!publisher.path().exists(), "a refused lease must not write");
    }

    #[test]
    fn a_lease_with_no_endpoint_names_nothing_to_dial() {
        let home = tempfile::tempdir().unwrap();
        let publisher = Publisher::new(home.path(), "node-a");
        assert!(publisher.publish(&Lease::new("unibus")).is_err());
    }

    #[test]
    fn an_endpoint_is_not_a_url_a_human_clicks() {
        assert!(endpoint_ok("tcp://100.70.187.127:18790"));
        assert!(endpoint_ok("https://[fd7a:115c:a1e0::4e01:bbaf]:8443"));
        assert!(!endpoint_ok("https://user:pass@example.test:443"));
        assert!(!endpoint_ok("https://example.test/path"));
        assert!(!endpoint_ok("ftp://example.test"));
        assert!(!endpoint_ok("example.test:443"));
    }

    #[test]
    fn a_catalog_we_cannot_read_is_not_clobbered() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("application-services.json");
        std::fs::write(&path, b"{not json").unwrap();
        let publisher = Publisher::at(&path, "node-a");
        assert!(publisher.publish(&lease()).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"{not json");
    }

    #[test]
    fn a_spore_manifest_supplies_the_lease() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("spore.toml"),
            "[spore]\nid = \"dev.fungos.confluence\"\n\n[anycast]\nname = \"confluence\"\nkind = \"confluence\"\nalpns = [\"soil/http/1\"]\nttl_secs = 240\n",
        )
        .unwrap();
        let lease = lease_from_manifest(dir.path(), &["https://100.74.130.83:8790".into()])
            .unwrap()
            .expect("the manifest claims a name");
        assert_eq!(lease.kind(), "confluence");
        assert_eq!(lease.ttl_secs(), 240);
        assert_eq!(
            lease.attributes().get("spore_id").map(String::as_str),
            Some("dev.fungos.confluence")
        );
        assert_eq!(
            lease.attributes().get("healthz").map(String::as_str),
            Some("/healthz")
        );
        assert_eq!(
            lease.attributes().get("alpns").map(String::as_str),
            Some("soil/http/1")
        );
    }

    #[test]
    fn a_spore_without_a_claim_asks_for_no_name() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("spore.toml"),
            "[spore]\nid = \"dev.fungos.local\"\n",
        )
        .unwrap();
        assert!(
            lease_from_manifest(dir.path(), &["https://127.0.0.1:8790".into()])
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn a_label_is_validated_by_the_zones_own_rule() {
        let home = tempfile::tempdir().unwrap();
        let publisher = Publisher::new(home.path(), "node-a");
        publisher.publish(&lease().label("unibus")).unwrap();
        let catalog = read(publisher.path());
        assert_eq!(catalog.observations[0].value.names, ["unibus"]);

        // A label is not a name: a dot would escape the zone's composition.
        assert!(Publisher::new(home.path(), "node-b")
            .publish(&lease().label("unibus.pris.fpl"))
            .is_err());
        assert!(Publisher::new(home.path(), "node-b")
            .publish(&lease().label("Unibus"))
            .is_err());
    }

    #[test]
    fn a_renewal_that_cannot_keep_the_lease_alive_is_refused() {
        assert_eq!(renew_every(240), 80);
        assert!(renewal_interval(Some(240), 240).is_err());
        assert!(renewal_interval(Some(0), 240).is_err());
        assert_eq!(renewal_interval(None, 240).unwrap(), 80);
    }

    #[test]
    fn a_lease_says_what_it_can_be_asked_for() {
        let lease = lease()
            .capability("soil:unibus")
            .capability("soil:metrics")
            .metrics("prometheus")
            .healthz("/healthz");
        assert_eq!(
            lease.attributes().get(CAPABILITIES).map(String::as_str),
            Some("soil:metrics soil:unibus")
        );
        assert_eq!(
            lease.attributes().get("metrics").map(String::as_str),
            Some("prometheus")
        );
        assert_eq!(
            lease.attributes().get("healthz").map(String::as_str),
            Some("/healthz")
        );
    }

    #[test]
    fn an_observer_finds_scrapeable_names_and_their_paths() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("application-services.json");
        Publisher::at(&path, "node-a")
            .publish(
                &lease()
                    .capability("soil:metrics")
                    .metrics("prometheus")
                    .healthz("/healthz")
                    .attribute("node_name", "pris.fpl.myc"),
            )
            .unwrap();
        Publisher::at(&path, "node-b")
            .publish(
                &Lease::new("geode")
                    .endpoint("https://100.0.0.9:8443")
                    .attribute("node_name", "batty.fpl.myc"),
            )
            .unwrap();

        let seen = observe(&path).unwrap();
        let names = seen.iter().map(|one| one.name.clone()).collect::<Vec<_>>();
        assert_eq!(names, ["geode.batty.fpl.myc", "unibus.pris.fpl.myc"]);
        let scrapeable = seen
            .iter()
            .filter(|one| one.holds("soil:metrics"))
            .collect::<Vec<_>>();
        assert_eq!(scrapeable.len(), 1);
        assert_eq!(scrapeable[0].healthz(), Some("/healthz"));
        assert_eq!(scrapeable[0].readyz(), None);
    }

    #[test]
    fn an_observer_does_not_see_a_withdrawn_lease() {
        let home = tempfile::tempdir().unwrap();
        let publisher = Publisher::new(home.path(), "node-a");
        publisher.publish(&lease()).unwrap();
        assert_eq!(observe(publisher.path()).unwrap().len(), 1);
        publisher.withdraw("unibus").unwrap();
        assert!(observe(publisher.path()).unwrap().is_empty());
    }
}

#[cfg(test)]
mod ownership_tests {
    use super::*;
    #[cfg(unix)]
    #[test]
    fn rewriting_keeps_the_files_mode() {
        use std::os::unix::fs::PermissionsExt as _;
        let home = tempfile::tempdir().unwrap();
        let publisher = Publisher::new(home.path(), "node-a");
        let lease = Lease::new("unibus").endpoint("tcp://100.70.187.127:18790");
        publisher.publish(&lease).unwrap();
        std::fs::set_permissions(publisher.path(), std::fs::Permissions::from_mode(0o644)).unwrap();
        // A renewal (and a root writer) must not quietly re-privatise the file:
        // the readers are a different uid than the writer.
        publisher.publish(&lease).unwrap();
        let mode = std::fs::metadata(publisher.path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o644);
    }
}
