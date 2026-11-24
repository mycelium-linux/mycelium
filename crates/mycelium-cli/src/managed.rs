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

/// Sign one entry and, when asked, write the signed manifest back.
///
/// The catalogue a host runs must be signed by a key it already trusts, so
/// publishing a catalogue is a signing step rather than an editing step. The
/// manifest carries its own signer public key, so a host only needs the set of
/// keys it trusts; it never needs the store's account.
pub fn sign_entry(
    entry: &Entry,
    key: &SigningKey,
    write: bool,
) -> Result<(String, String, PathBuf), String> {
    let signed = entry.manifest.clone().sign(key)?;
    let path = entry.dir.join("manifest.json");
    if write {
        let body = serde_json::to_vec_pretty(&signed).map_err(|error| error.to_string())?;
        std::fs::write(&path, body).map_err(|error| format!("{}: {error}", path.display()))?;
    }
    Ok((signed.signer, signed.signature, path))
}

/// Every entry with its signature state, for a gate before publishing a store.
pub fn verify_catalogue(entries: &[Entry], trusted: &[String]) -> Vec<serde_json::Value> {
    entries
        .iter()
        .map(|entry| {
            let signature = entry.manifest.verify();
            serde_json::json!({
                "id": entry.manifest.id,
                "version": entry.manifest.version,
                "signer": entry.manifest.signer,
                "signed": !entry.manifest.signature.is_empty(),
                "verified": signature.is_ok(),
                "error": signature.err(),
                "trusted": entry.manifest.trusted_by(trusted),
            })
        })
        .collect()
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
/// What an install left behind, so a later version can be backed out.
///
/// The receipt snapshots the scripts that actually ran. A rollback must not
/// depend on the catalogue still carrying the version that was installed: the
/// store moves on, and the host has to be able to return to what it was.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Receipt {
    pub id: String,
    pub version: String,
    pub signer: String,
    pub installed_at: u64,
    /// Set when `uninstall` succeeded. A rollback after that reinstalls.
    #[serde(default)]
    pub uninstalled_at: Option<u64>,
    /// Snapshot directory holding the scripts and manifest that ran.
    pub directory: PathBuf,
    #[serde(default)]
    pub previous: Option<Box<Receipt>>,
}

/// How many earlier versions a receipt keeps. Enough to walk back through a
/// bad release, bounded so a host that updates often cannot grow forever.
pub const RECEIPT_HISTORY: usize = 5;

pub fn receipt_path(state_root: &Path, id: &str) -> PathBuf {
    state_root.join(id).join("receipt.json")
}

pub fn read_receipt(state_root: &Path, id: &str) -> Option<Receipt> {
    let bytes = std::fs::read(receipt_path(state_root, id)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub fn write_receipt(state_root: &Path, receipt: &Receipt) -> Result<(), String> {
    let path = receipt_path(state_root, &receipt.id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("{}: {error}", parent.display()))?;
    }
    let body = serde_json::to_vec_pretty(receipt).map_err(|error| error.to_string())?;
    std::fs::write(&path, body).map_err(|error| format!("{}: {error}", path.display()))
}

/// Copy the entry's own files into the install state, returning that directory.
fn snapshot(entry: &Entry, state_root: &Path) -> Result<PathBuf, String> {
    let version_dir = state_root
        .join(&entry.manifest.id)
        .join("versions")
        .join(&entry.manifest.version);
    std::fs::create_dir_all(&version_dir)
        .map_err(|error| format!("{}: {error}", version_dir.display()))?;
    let mut copied = 0usize;
    for item in std::fs::read_dir(&entry.dir).map_err(|error| error.to_string())? {
        let item = item.map_err(|error| error.to_string())?;
        let path = item.path();
        let meta = item.metadata().map_err(|error| error.to_string())?;
        if !meta.is_file() || meta.len() > 256 * 1024 {
            continue;
        }
        let Some(name) = path.file_name() else {
            continue;
        };
        std::fs::copy(&path, version_dir.join(name))
            .map_err(|error| format!("{}: {error}", path.display()))?;
        copied += 1;
        if copied >= 32 {
            break;
        }
    }
    // The snapshot is only useful if it carries the manifest it belongs to.
    let manifest = version_dir.join("manifest.json");
    if !manifest.is_file() {
        let body = serde_json::to_vec_pretty(&entry.manifest).map_err(|e| e.to_string())?;
        std::fs::write(&manifest, body).map_err(|error| error.to_string())?;
    }
    Ok(version_dir)
}

fn record_install(entry: &Entry, state_root: &Path) -> Result<Receipt, String> {
    let previous = read_receipt(state_root, &entry.manifest.id).map(Box::new);
    let directory = snapshot(entry, state_root)?;
    let receipt = Receipt {
        id: entry.manifest.id.clone(),
        version: entry.manifest.version.clone(),
        signer: entry.manifest.signer.clone(),
        installed_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or_default(),
        uninstalled_at: None,
        directory,
        previous,
    };
    let receipt = truncate(receipt, RECEIPT_HISTORY);
    write_receipt(state_root, &receipt)?;
    Ok(receipt)
}

fn truncate(mut receipt: Receipt, keep: usize) -> Receipt {
    let mut depth = 1;
    let mut cursor: &mut Receipt = &mut receipt;
    loop {
        depth += 1;
        if depth > keep {
            cursor.previous = None;
            break;
        }
        match cursor.previous.as_mut() {
            Some(previous) => cursor = previous,
            None => break,
        }
    }
    receipt
}

/// Reinstall the version before the one currently recorded.
pub fn rollback(
    state_root: &Path,
    id: &str,
    allow_root: bool,
    write: bool,
) -> Result<Vec<String>, String> {
    let current = read_receipt(state_root, id)
        .ok_or_else(|| format!("{id} has no install receipt on this host; nothing to roll back"))?;
    let Some(previous) = current.previous.clone() else {
        return Err(format!(
            "{id} {} is the only recorded version; there is nothing to roll back to",
            current.version
        ));
    };
    let manifest_path = previous.directory.join("manifest.json");
    let bytes = std::fs::read(&manifest_path)
        .map_err(|error| format!("{}: {error}", manifest_path.display()))?;
    let manifest: Manifest =
        serde_json::from_slice(&bytes).map_err(|error| format!("{manifest_path:?}: {error}"))?;
    // The snapshot was trusted when it ran, but trust is checked again: a
    // receipt is a file, and a file can be edited.
    manifest
        .verify()
        .map_err(|error| format!("{id}: {error}"))?;
    if manifest.requires.root && !allow_root {
        return Err(format!(
            "{id} {} needs root to reinstall; pass --allow-root",
            manifest.version
        ));
    }
    let entry = Entry {
        manifest,
        dir: previous.directory.clone(),
    };
    let mut lines = vec![format!(
        "rollback {id}: {} -> {}",
        current.version, entry.manifest.version
    )];
    if !write {
        lines.push("plan only; pass --write to run it".into());
        return Ok(lines);
    }
    let mut restore = run_step_in(&entry, Action::Install, allow_root, true, state_root, true)?;
    lines.append(&mut restore);
    lines.extend(run_step_in(
        &entry,
        Action::Verify,
        allow_root,
        true,
        state_root,
        true,
    )?);
    // The install just recorded the previous version as current and kept the
    // newer one as `previous`, which would make the next rollback a re-upgrade.
    if let Some(mut restored) = read_receipt(state_root, id) {
        restored.previous = previous.previous;
        write_receipt(state_root, &restored)?;
    }
    lines.push(format!("rolled back to {}", entry.manifest.version));
    Ok(lines)
}

pub fn run_step(
    entry: &Entry,
    action: Action,
    allow_root: bool,
    write: bool,
) -> Result<Vec<String>, String> {
    run_step_in(
        entry,
        action,
        allow_root,
        write,
        &myceliumd::home_dir().join("managed"),
        false,
    )
}

pub fn run_step_in(
    entry: &Entry,
    action: Action,
    allow_root: bool,
    write: bool,
    state_root: &Path,
    rolling_back: bool,
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
        .env("MYCELIUM_MANAGED_STATE", state_root.join(&manifest.id))
        .env(
            "MYCELIUM_MANAGED_ROLLBACK",
            if rolling_back { "1" } else { "0" },
        )
        .status()
        .map_err(|error| format!("{}: {error}", path.display()))?;
    if !status.success() {
        return Err(format!("{name} exited with {status}"));
    }
    lines.push(format!("{name} finished"));
    match action {
        Action::Install => {
            let receipt = record_install(entry, state_root)?;
            lines.push(format!(
                "recorded {} {} (receipt {})",
                receipt.id,
                receipt.version,
                receipt_path(state_root, &receipt.id).display()
            ));
        }
        Action::Uninstall => {
            if let Some(mut receipt) = read_receipt(state_root, &manifest.id) {
                receipt.uninstalled_at = Some(
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|elapsed| elapsed.as_secs())
                        .unwrap_or_default(),
                );
                write_receipt(state_root, &receipt)?;
            }
        }
        Action::Check | Action::Verify => {}
    }
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

    fn write_script(path: &Path, body: &str) {
        std::fs::write(path, body).unwrap();
    }

    fn entry_at(dir: &Path, version: &str) -> Entry {
        std::fs::create_dir_all(dir).unwrap();
        let (mut manifest, _) = signed();
        manifest.version = version.into();
        manifest.requires.root = false;
        write_script(
            &dir.join("install.sh"),
            "#!/bin/sh\nmkdir -p \"$MYCELIUM_MANAGED_STATE\"\nprintf '%s' \"$MYCELIUM_MANAGED_VERSION\" >\"$MYCELIUM_MANAGED_STATE/version\"\nprintf '%s' \"$MYCELIUM_MANAGED_ROLLBACK\" >\"$MYCELIUM_MANAGED_STATE/rollback\"\n",
        );
        write_script(
            &dir.join("verify.sh"),
            "#!/bin/sh\ntest -f \"$MYCELIUM_MANAGED_STATE/version\"\n",
        );
        let manifest = manifest
            .sign(&SigningKey::generate(&mut rand_core::OsRng))
            .unwrap();
        Entry {
            manifest,
            dir: dir.to_path_buf(),
        }
    }

    fn state() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "mycelium-managed-receipt-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn install_records_a_receipt_and_a_snapshot() {
        let root = state();
        let dir = root.join("catalogue");
        let entry = entry_at(&dir, "1.0.0");
        run_step_in(&entry, Action::Install, false, true, &root, false).unwrap();
        let receipt = read_receipt(&root, entry.manifest.id.as_str()).expect("receipt");
        assert_eq!(receipt.version, "1.0.0");
        assert!(receipt.directory.join("install.sh").is_file());
        assert_eq!(
            std::fs::read_to_string(root.join(&entry.manifest.id).join("version")).unwrap(),
            "1.0.0"
        );
        assert_eq!(
            std::fs::read_to_string(root.join(&entry.manifest.id).join("rollback")).unwrap(),
            "0"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn rollback_reinstalls_the_previous_version_and_pops_it() {
        let root = state();
        let dir = root.join("catalogue");
        let first = entry_at(&dir, "1.0.0");
        run_step_in(&first, Action::Install, false, true, &root, false).unwrap();

        // The catalogue moves on; the snapshot is what makes rollback possible.
        let second = entry_at(&dir, "2.0.0");
        run_step_in(&second, Action::Install, false, true, &root, false).unwrap();
        assert_eq!(
            read_receipt(&root, "cloudflared-tunnel").unwrap().version,
            "2.0.0"
        );
        std::fs::remove_dir_all(&dir).ok();

        rollback(&root, "cloudflared-tunnel", false, true).unwrap();
        let receipt = read_receipt(&root, "cloudflared-tunnel").unwrap();
        assert_eq!(receipt.version, "1.0.0");
        assert!(
            receipt.previous.is_none(),
            "one rollback consumed the history"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("cloudflared-tunnel").join("version")).unwrap(),
            "1.0.0"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("cloudflared-tunnel").join("rollback")).unwrap(),
            "1"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_single_version_has_nothing_to_roll_back_to() {
        let root = state();
        let dir = root.join("catalogue");
        let entry = entry_at(&dir, "1.0.0");
        run_step_in(&entry, Action::Install, false, true, &root, false).unwrap();
        let error = rollback(&root, "cloudflared-tunnel", false, true).unwrap_err();
        assert!(error.contains("nothing to roll back to"), "{error}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn rollback_without_a_receipt_is_refused() {
        let root = state();
        let error = rollback(&root, "cloudflared-tunnel", false, true).unwrap_err();
        assert!(error.contains("no install receipt"), "{error}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn signing_an_entry_makes_it_installable() {
        // The catalogue shipped unsigned, and `run_step` refuses an unsigned
        // manifest. Signing is the step that turns a reviewed entry into one a
        // host will run, so it has to round-trip through disk.
        let root = state();
        let dir = root.join("catalogue").join("palisade-edge");
        let mut entry = entry_at(&dir, "1.0.0");
        // What the catalogue actually ships today: reviewed, but unsigned.
        entry.manifest.signer.clear();
        entry.manifest.signature.clear();
        assert!(run_step_in(&entry, Action::Install, false, true, &root, false).is_err());

        let key = SigningKey::generate(&mut rand_core::OsRng);
        let (signer, signature, path) = sign_entry(&entry, &key, true).unwrap();
        assert!(signature.len() == 128, "{signature}");
        let written: Manifest = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        written.verify().unwrap();
        assert_eq!(written.signer, signer);
        assert!(written.trusted_by(&[signer.clone()]));

        let reloaded = Entry {
            manifest: written,
            dir: dir.clone(),
        };
        run_step_in(&reloaded, Action::Install, false, true, &root, false).unwrap();
        assert_eq!(
            read_receipt(&root, "cloudflared-tunnel").unwrap().version,
            "1.0.0"
        );

        let rows = verify_catalogue(&[reloaded], &[signer]);
        assert_eq!(rows[0]["verified"], serde_json::json!(true));
        assert_eq!(rows[0]["trusted"], serde_json::json!(true));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn an_untrusted_signer_verifies_but_is_not_trusted() {
        let root = state();
        let dir = root.join("catalogue").join("palisade-edge");
        let entry = entry_at(&dir, "1.0.0");
        let key = SigningKey::generate(&mut rand_core::OsRng);
        sign_entry(&entry, &key, true).unwrap();
        let signed: Manifest =
            serde_json::from_slice(&std::fs::read(dir.join("manifest.json")).unwrap()).unwrap();
        let entry = Entry {
            manifest: signed,
            dir,
        };
        let rows = verify_catalogue(&[entry], &["00".repeat(32)]);
        assert_eq!(rows[0]["verified"], serde_json::json!(true));
        assert_eq!(rows[0]["trusted"], serde_json::json!(false));
        std::fs::remove_dir_all(&root).ok();
    }
}
