//! Managed installs: host-level provisioning that ships with its documentation.
//!
//! A Spore is sandboxed and renders. Much of what a member actually wants is not
//! a renderer: a Cloudflare connector needs root and a token, a Prometheus pair
//! needs disk and a service, a build fleet needs ssh keys and a scheduler. Those
//! are *managed installs*: a signed manifest naming a check, an install, a
//! verify and an uninstall script, the runbook that explains them, and what the
//! host must already have (root, network, named credentials).
//!
//! The store is the distribution channel for the catalogue; the trust is the
//! same as for releases. A host runs only manifests signed by a key it already
//! trusts for releases, unless the operator names a key explicitly for a local
//! catalogue. Nothing here needs a daemon: an install is a host fact, and the
//! person running it should see the script, the runbook and the requirement list
//! before anything executes.

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use std::path::{Path, PathBuf};

/// One managed install: the manifest plus where its scripts and docs live.
#[derive(Clone, Debug)]
pub struct Entry {
    pub manifest: Manifest,
    pub dir: PathBuf,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Manifest {
    pub id: String,
    pub title: String,
    pub version: String,
    pub channel: String,
    #[serde(default)]
    pub summary: String,
    /// Target triples this install supports; empty means every supported host.
    #[serde(default)]
    pub platforms: Vec<String>,
    #[serde(default)]
    pub requires: Requirements,
    /// What the install makes available, in the same words `services --json` uses.
    #[serde(default)]
    pub provides: Vec<String>,
    #[serde(default)]
    pub steps: Steps,
    #[serde(default)]
    pub docs: Docs,
    #[serde(default)]
    pub signer: String,
    #[serde(default)]
    pub signature: String,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Requirements {
    /// Steps need root. The operator must pass --allow-root: a manifest cannot
    /// grant itself privilege just by being trusted.
    #[serde(default)]
    pub root: bool,
    #[serde(default)]
    pub network: bool,
    /// Credential names the runbook expects to find in geode.
    #[serde(default)]
    pub credentials: Vec<String>,
    /// Commands that must exist before install (checked, not installed).
    #[serde(default)]
    pub commands: Vec<String>,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Steps {
    #[serde(default)]
    pub check: Option<String>,
    #[serde(default)]
    pub install: Option<String>,
    #[serde(default)]
    pub verify: Option<String>,
    #[serde(default)]
    pub uninstall: Option<String>,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Docs {
    #[serde(default)]
    pub runbook: Option<String>,
    #[serde(default)]
    pub readme: Option<String>,
}

#[derive(serde::Serialize)]
struct Unsigned<'a> {
    id: &'a str,
    title: &'a str,
    version: &'a str,
    channel: &'a str,
    summary: &'a str,
    platforms: &'a [String],
    requires: &'a Requirements,
    provides: &'a [String],
    steps: &'a Steps,
    docs: &'a Docs,
}

impl Manifest {
    fn unsigned_bytes(&self) -> Result<Vec<u8>, String> {
        serde_json::to_vec(&Unsigned {
            id: &self.id,
            title: &self.title,
            version: &self.version,
            channel: &self.channel,
            summary: &self.summary,
            platforms: &self.platforms,
            requires: &self.requires,
            provides: &self.provides,
            steps: &self.steps,
            docs: &self.docs,
        })
        .map_err(|error| error.to_string())
    }

    /// Sign every field an installer will act on. The signature deliberately
    /// does not cover itself or the signer, so a manifest can be re-signed.
    pub fn sign(mut self, key: &SigningKey) -> Result<Self, String> {
        let bytes = self.unsigned_bytes()?;
        self.signer = mycelium_peer_protocol::encode_hex(key.verifying_key().as_bytes());
        self.signature = mycelium_peer_protocol::encode_hex(&key.sign(&bytes).to_bytes());
        Ok(self)
    }

    pub fn verify(&self) -> Result<(), String> {
        let signer = decode::<32>(&self.signer)?;
        let key = VerifyingKey::from_bytes(&signer).map_err(|error| error.to_string())?;
        let signature = Signature::from_bytes(&decode::<64>(&self.signature)?);
        key.verify(&self.unsigned_bytes()?, &signature)
            .map_err(|error| error.to_string())
    }

    pub fn trusted_by(&self, trusted: &[String]) -> bool {
        trusted
            .iter()
            .any(|key| key.eq_ignore_ascii_case(&self.signer))
    }
}

fn decode<const N: usize>(hex_value: &str) -> Result<[u8; N], String> {
    let bytes = hex::decode(hex_value).map_err(|error| error.to_string())?;
    bytes
        .as_slice()
        .try_into()
        .map_err(|_| format!("expected {N} bytes, got {}", bytes.len()))
}

/// Read every `manifest.json` under `catalogue`. A broken manifest is reported
/// and skipped: one bad entry must not hide the rest of the store from `list`.
pub fn load_catalogue(catalogue: &Path) -> Result<(Vec<Entry>, Vec<String>), String> {
    let mut entries = Vec::new();
    let mut problems = Vec::new();
    let read = std::fs::read_dir(catalogue)
        .map_err(|error| format!("{}: {error}", catalogue.display()))?;
    for item in read {
        let Ok(item) = item else { continue };
        let dir = item.path();
        if !dir.is_dir() {
            continue;
        }
        let path = dir.join("manifest.json");
        if !path.is_file() {
            continue;
        }
        match std::fs::read(&path)
            .map_err(|error| error.to_string())
            .and_then(|bytes| {
                serde_json::from_slice::<Manifest>(&bytes)
                    .map_err(|error| format!("{}: {error}", path.display()))
            }) {
            Ok(manifest) => entries.push(Entry { manifest, dir }),
            Err(error) => problems.push(error),
        }
    }
    entries.sort_by(|left, right| left.manifest.id.cmp(&right.manifest.id));
    Ok((entries, problems))
}

/// The manifest's platform list, checked against this host's target triples.
pub fn supports_host(manifest: &Manifest) -> bool {
    manifest.platforms.is_empty()
        || mycelium_peer_protocol::local_compatible_targets()
            .iter()
            .any(|target| manifest.platforms.iter().any(|platform| platform == target))
}

/// Everything a person should see before they say yes.
pub fn plan(entry: &Entry) -> Vec<String> {
    let manifest = &entry.manifest;
    let mut lines = vec![
        format!("{} {} ({})", manifest.title, manifest.version, manifest.id),
        format!("  summary: {}", manifest.summary),
        format!("  directory: {}", entry.dir.display()),
    ];
    if !supports_host(manifest) {
        lines.push(format!(
            "  unsupported here: it targets {:?}",
            manifest.platforms
        ));
    }
    if manifest.requires.root {
        lines.push("  needs root: apply requires --allow-root".into());
    }
    if manifest.requires.network {
        lines.push("  needs network".into());
    }
    if !manifest.requires.credentials.is_empty() {
        lines.push(format!(
            "  credentials: {} (look them up in geode)",
            manifest.requires.credentials.join(", ")
        ));
    }
    if !manifest.requires.commands.is_empty() {
        lines.push(format!(
            "  required commands: {}",
            manifest.requires.commands.join(", ")
        ));
    }
    if !manifest.provides.is_empty() {
        lines.push(format!("  provides: {}", manifest.provides.join(", ")));
    }
    for (name, step) in [
        ("check", &manifest.steps.check),
        ("install", &manifest.steps.install),
        ("verify", &manifest.steps.verify),
        ("uninstall", &manifest.steps.uninstall),
    ] {
        if let Some(step) = step {
            lines.push(format!("  {name}: {step}"));
        }
    }
    if let Some(runbook) = &manifest.docs.runbook {
        lines.push(format!("  runbook: {}", entry.dir.join(runbook).display()));
    }
    lines
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Check,
    Install,
    Verify,
    Uninstall,
}

/// Run one step. The script is `sh` on purpose: it ships as data and must be
/// readable by the person approving it, not compiled into the binary. The
/// environment tells the script where it is and where its state belongs, so the
/// same catalogue works from a store, a checkout or a mirror.
pub fn run_step(
    entry: &Entry,
    action: Action,
    allow_root: bool,
    write: bool,
) -> Result<Vec<String>, String> {
    let manifest = &entry.manifest;
    if manifest.signature.is_empty() {
        return Err(format!("{} is unsigned", manifest.id));
    }
    if !supports_host(manifest) {
        return Err(format!(
            "{} does not support this host ({:?})",
            manifest.id, manifest.platforms
        ));
    }
    if manifest.requires.root && !allow_root {
        return Err(format!(
            "{} needs root; read the runbook, then pass --allow-root",
            manifest.id
        ));
    }
    let (name, script) = match action {
        Action::Check => ("check", &manifest.steps.check),
        Action::Install => ("install", &manifest.steps.install),
        Action::Verify => ("verify", &manifest.steps.verify),
        Action::Uninstall => ("uninstall", &manifest.steps.uninstall),
    };
    let Some(script) = script else {
        return Ok(vec![format!("{} has no {name} step", manifest.id)]);
    };
    let path = entry.dir.join(script);
    if !path.is_file() {
        return Err(format!(
            "{}: missing {name} script {}",
            manifest.id,
            path.display()
        ));
    }
    let mut lines = vec![format!("{} {name}: {}", manifest.id, path.display())];
    if !write {
        lines.push("plan only; pass --write to run it".into());
        return Ok(lines);
    }
    let status = std::process::Command::new("sh")
        .arg("-e")
        .arg(&path)
        .current_dir(&entry.dir)
        .env("MYCELIUM_MANAGED_ID", &manifest.id)
        .env("MYCELIUM_MANAGED_VERSION", &manifest.version)
        .env("MYCELIUM_MANAGED_DIR", &entry.dir)
        .env(
            "MYCELIUM_MANAGED_STATE",
            myceliumd::home_dir().join("managed").join(&manifest.id),
        )
        .status()
        .map_err(|error| format!("{}: {error}", path.display()))?;
    if !status.success() {
        return Err(format!("{name} exited with {status}"));
    }
    lines.push(format!("{name} finished"));
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;

    fn manifest() -> Manifest {
        Manifest {
            id: "cloudflared-tunnel".into(),
            title: "Cloudflare tunnel ingress".into(),
            version: "1.0.0".into(),
            channel: "stable".into(),
            summary: "public ingress for this node".into(),
            platforms: Vec::new(),
            requires: Requirements {
                root: true,
                network: true,
                credentials: vec!["cloudflare.tunnel-token".into()],
                commands: vec!["cloudflared".into()],
            },
            provides: vec!["ingress.public".into()],
            steps: Steps {
                check: Some("check.sh".into()),
                install: Some("install.sh".into()),
                verify: Some("verify.sh".into()),
                uninstall: Some("uninstall.sh".into()),
            },
            docs: Docs {
                runbook: Some("runbook.md".into()),
                readme: None,
            },
            signer: String::new(),
            signature: String::new(),
        }
    }

    fn signed() -> (Manifest, String) {
        let key = SigningKey::generate(&mut rand_core::OsRng);
        let signer = mycelium_peer_protocol::encode_hex(key.verifying_key().as_bytes());
        (manifest().sign(&key).unwrap(), signer)
    }

    #[test]
    fn a_signed_manifest_verifies_and_reports_its_signer() {
        let (manifest, signer) = signed();
        manifest.verify().unwrap();
        assert!(manifest.trusted_by(&[signer]));
        assert!(!manifest.trusted_by(&["00".repeat(32)]));
    }

    #[test]
    fn editing_an_approved_manifest_breaks_the_signature() {
        let (mut manifest, _) = signed();
        manifest.steps.install = Some("other.sh".into());
        assert!(manifest.verify().is_err());
    }

    #[test]
    fn a_plan_names_the_runbook_the_root_requirement_and_the_credentials() {
        let (manifest, _) = signed();
        let entry = Entry {
            manifest,
            dir: PathBuf::from("/store/cloudflared-tunnel"),
        };
        let plan = plan(&entry).join("\n");
        assert!(
            plan.contains("needs root: apply requires --allow-root"),
            "{plan}"
        );
        assert!(plan.contains("cloudflare.tunnel-token"), "{plan}");
        assert!(plan.contains("runbook.md"), "{plan}");
    }

    #[test]
    fn root_steps_need_explicit_consent() {
        let (manifest, _) = signed();
        let entry = Entry {
            manifest,
            dir: std::env::temp_dir(),
        };
        let error = run_step(&entry, Action::Install, false, false).unwrap_err();
        assert!(error.contains("--allow-root"), "{error}");
    }

    #[test]
    fn a_signed_entry_runs_its_scripts_in_order() {
        let work = std::env::temp_dir().join(format!("mycelium-managed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&work);
        std::fs::create_dir_all(&work).unwrap();
        std::fs::write(work.join("check.sh"), "#!/bin/sh\necho checked\n").unwrap();
        std::fs::write(
            work.join("install.sh"),
            "#!/bin/sh\nmkdir -p \"$MYCELIUM_MANAGED_STATE\"\nprintf installed >\"$MYCELIUM_MANAGED_STATE/marker\"\n",
        )
        .unwrap();
        std::fs::write(
            work.join("verify.sh"),
            "#!/bin/sh\ntest -f \"$MYCELIUM_MANAGED_STATE/marker\"\n",
        )
        .unwrap();
        let (mut manifest, _) = signed();
        manifest.requires.root = false;
        let (manifest, signer) = {
            let key = SigningKey::generate(&mut rand_core::OsRng);
            let signer = mycelium_peer_protocol::encode_hex(key.verifying_key().as_bytes());
            (manifest.sign(&key).unwrap(), signer)
        };
        assert!(manifest.trusted_by(&[signer]));
        let entry = Entry {
            manifest,
            dir: work.clone(),
        };
        run_step(&entry, Action::Check, false, true).unwrap();
        run_step(&entry, Action::Install, false, true).unwrap();
        run_step(&entry, Action::Verify, false, true).unwrap();
        let marker = myceliumd::home_dir()
            .join("managed")
            .join("cloudflared-tunnel")
            .join("marker");
        assert_eq!(std::fs::read_to_string(&marker).unwrap(), "installed");
        let _ = std::fs::remove_file(&marker);
        let _ = std::fs::remove_dir_all(&work);
    }

    #[test]
    fn an_unsigned_manifest_never_runs() {
        let entry = Entry {
            manifest: manifest(),
            dir: std::env::temp_dir(),
        };
        let error = run_step(&entry, Action::Check, true, true).unwrap_err();
        assert!(error.contains("unsigned"), "{error}");
    }
}
