//! Where a host gets a release from, and why it should believe it.
//!
//! Until now a release reached a host two ways: the installer downloaded it once
//! (verifying a checksum that travelled beside it), or an operator ran
//! `releases seed` on that host. Neither is an update channel, so
//! `update auto-run` could only report `still downloading` forever.
//!
//! This module is the missing half: one asset per release
//! (`release-manifests.json`) carrying a [`ReleaseManifest`] per target, each
//! signed by the fleet's release key over the *binary* it describes. A host
//! trusts a small set of release keys (pinned at install time in
//! `release-source.json`, or supplied by `MYCELIUM_RELEASE_KEYS`), verifies the
//! signature before it looks at any bytes, downloads the archive that metadata
//! points at, and accepts the binary only when its SHA-256 equals the digest the
//! verified manifest names.
//!
//! The archive's own checksum is deliberately not the trust anchor: it is
//! published by the same channel as the archive. What is trusted is the
//! signature over the digest of the bytes that will run.

use mycelium_peer_protocol::ReleaseManifest;
use std::path::{Path, PathBuf};

/// The signed manifests, one per target, published beside each release.
pub const MANIFESTS_ASSET: &str = "release-manifests.json";
/// The installer's map from target to archive name; not a trust anchor.
pub const METADATA_ASSET: &str = "release-metadata.json";
/// Where the installer records this host's source and pinned keys.
pub const SOURCE_FILE: &str = "release-source.json";

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Source {
    /// Release download base: an `https://` URL or a local directory (mirrors,
    /// tests). Empty means "no configured source".
    #[serde(default)]
    pub base: String,
    /// Channel to follow. Empty means the update policy's channel.
    #[serde(default)]
    pub channel: String,
    /// Release keys this host will accept, hex-encoded Ed25519 public keys.
    #[serde(default)]
    pub trusted_keys: Vec<String>,
}

impl Source {
    /// Read the host's source file, then let the environment override it: an
    /// operator debugging a channel should not have to edit a file the
    /// installer manages.
    pub fn load(home: &Path) -> Self {
        let path = home.join(SOURCE_FILE);
        let mut source: Source = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        if let Ok(base) = std::env::var("MYCELIUM_RELEASE_SOURCE") {
            if !base.is_empty() {
                source.base = base;
            }
        }
        if let Ok(channel) = std::env::var("MYCELIUM_UPDATE_CHANNEL") {
            if !channel.is_empty() {
                source.channel = channel;
            }
        }
        for key in env_keys() {
            if !source
                .trusted_keys
                .iter()
                .any(|known| known.eq_ignore_ascii_case(&key))
            {
                source.trusted_keys.push(key);
            }
        }
        source
    }

    pub fn path(home: &Path) -> PathBuf {
        home.join(SOURCE_FILE)
    }

    pub fn is_configured(&self) -> bool {
        !self.base.is_empty() && !self.trusted_keys.is_empty()
    }
}

fn env_keys() -> Vec<String> {
    std::env::var("MYCELIUM_RELEASE_KEYS")
        .unwrap_or_default()
        .split([',', ' '])
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .map(str::to_owned)
        .collect()
}

/// A release metadata document: enough to find the archive for a target.
#[derive(Debug, serde::Deserialize)]
struct ReleaseMetadata {
    #[serde(default)]
    targets: Vec<ReleaseTarget>,
}

#[derive(Debug, serde::Deserialize)]
struct ReleaseTarget {
    target: String,
    archive: String,
}

pub fn parse_manifests(bytes: &[u8]) -> Result<Vec<ReleaseManifest>, String> {
    serde_json::from_slice(bytes).map_err(|error| format!("{MANIFESTS_ASSET}: {error}"))
}

pub fn archive_for(metadata: &[u8], target: &str) -> Result<String, String> {
    let parsed: ReleaseMetadata =
        serde_json::from_slice(metadata).map_err(|error| format!("{METADATA_ASSET}: {error}"))?;
    parsed
        .targets
        .into_iter()
        .find(|entry| entry.target == target)
        .map(|entry| entry.archive)
        .ok_or_else(|| format!("no {target} archive in {METADATA_ASSET}"))
}

/// Choose the newest manifest for `channel` among this host's targets, after
/// verifying it: an unverified or untrusted manifest is not a candidate at all,
/// so a hostile channel cannot win by publishing a higher version.
pub fn select_verified(
    manifests: &[ReleaseManifest],
    channel: &str,
    targets: &[String],
    trusted: &[String],
) -> Result<ReleaseManifest, String> {
    if trusted.is_empty() {
        return Err(
            "no trusted release keys; pass --trusted-key or set MYCELIUM_RELEASE_KEYS".into(),
        );
    }
    let mut best: Option<(ReleaseManifest, semver::Version, usize)> = None;
    let mut rejected = 0usize;
    for manifest in manifests
        .iter()
        .filter(|manifest| manifest.channel == channel)
    {
        let Some(rank) = targets.iter().position(|target| target == &manifest.target) else {
            continue;
        };
        let Ok(version) = semver::Version::parse(&manifest.version) else {
            continue;
        };
        let trusted_signer = trusted
            .iter()
            .any(|key| key.eq_ignore_ascii_case(&manifest.signer));
        if !trusted_signer || manifest.verify().is_err() {
            rejected += 1;
            continue;
        }
        let better = match &best {
            None => true,
            Some((_, current, current_rank)) => {
                version > *current || (version == *current && rank < *current_rank)
            }
        };
        if better {
            best = Some((manifest.clone(), version, rank));
        }
    }
    match best {
        Some((manifest, _, _)) => Ok(manifest),
        None if rejected > 0 => Err(format!(
            "{rejected} release manifest(s) on channel {channel} failed signature or trust"
        )),
        None => Err(format!(
            "no verified release on channel {channel} for {targets:?}"
        )),
    }
}

/// Read an asset from the configured base. `https://` goes over the network;
/// anything else is a local directory, which is what a mirror and the tests are.
async fn read_asset(base: &str, name: &str) -> Result<Vec<u8>, String> {
    let base = base.trim_end_matches('/');
    if base.starts_with("https://") || base.starts_with("http://") {
        let url = format!("{base}/{name}");
        let response = reqwest::get(&url)
            .await
            .map_err(|error| format!("{url}: {error}"))?;
        if !response.status().is_success() {
            return Err(format!("{url}: HTTP {}", response.status()));
        }
        return response
            .bytes()
            .await
            .map(|bytes| bytes.to_vec())
            .map_err(|error| format!("{url}: {error}"));
    }
    std::fs::read(Path::new(base).join(name)).map_err(|error| format!("{base}/{name}: {error}"))
}

/// Write `bytes` to a content-addressed store under the digest the verified
/// manifest names. `update apply` reads that path; it never fetches.
fn seed(manifest: &ReleaseManifest, bytes: &[u8], directory: &Path) -> Result<PathBuf, String> {
    let digest = mycelium_peer_protocol::sha256_hex(bytes);
    if digest != manifest.artifact_digest {
        return Err(format!(
            "binary digest {digest} does not match the signed manifest {} ({} bytes)",
            manifest.artifact_digest, manifest.artifact_size
        ));
    }
    std::fs::create_dir_all(directory)
        .map_err(|error| format!("{}: {error}", directory.display()))?;
    let path = directory.join(&digest);
    let temporary = directory.join(format!("{digest}.{}.tmp", std::process::id()));
    std::fs::write(&temporary, bytes)
        .map_err(|error| format!("{}: {error}", temporary.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o755));
    }
    std::fs::rename(&temporary, &path).map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(path)
}

/// Fetch, verify, and (with `write`) seed one release for this host.
pub async fn fetch(source: &Source, channel: &str, write: bool) -> Result<Vec<String>, String> {
    fetch_into(source, channel, write, &myceliumd::artifacts_dir()).await
}

/// `fetch` with an explicit artifact store, so a test never needs the process
/// environment to decide where verified bytes land.
pub async fn fetch_into(
    source: &Source,
    channel: &str,
    write: bool,
    store: &Path,
) -> Result<Vec<String>, String> {
    if source.base.is_empty() {
        return Err(format!(
            "no release source configured; write {} or pass --source",
            SOURCE_FILE
        ));
    }
    let targets = mycelium_peer_protocol::local_compatible_targets();
    let manifests = parse_manifests(&read_asset(&source.base, MANIFESTS_ASSET).await?)?;
    let manifest = select_verified(&manifests, channel, &targets, &source.trusted_keys)?;
    let metadata = read_asset(&source.base, METADATA_ASSET).await?;
    let archive = archive_for(&metadata, &manifest.target)?;
    let mut lines = vec![format!(
        "verified {} {} for {} (signer {}…)",
        manifest.version,
        manifest.channel,
        manifest.target,
        &manifest.signer[..12.min(manifest.signer.len())]
    )];
    let path = store.join(&manifest.artifact_digest);
    if path.is_file() {
        lines.push(format!("already present: {}", path.display()));
        return Ok(lines);
    }
    if !write {
        lines.push(format!(
            "would download {archive} and verify it against {}",
            manifest.artifact_digest
        ));
        return Ok(lines);
    }
    let bytes = read_asset(&source.base, &archive).await?;
    let binary = extract_binary(&archive, &bytes)?;
    let seeded = seed(&manifest, &binary, store)?;
    lines.push(format!("seeded {}", seeded.display()));
    Ok(lines)
}

/// Pull `mycelium` out of a release archive. The extraction shells out to `tar`
/// because the archive is a `.tar.gz` and every supported host ships one; the
/// trust decision is made on the extracted bytes, not on the container.
fn extract_binary(archive: &str, bytes: &[u8]) -> Result<Vec<u8>, String> {
    let directory = std::env::temp_dir().join(format!(
        "mycelium-release-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("{}: {error}", directory.display()))?;
    let path = directory.join(archive);
    std::fs::write(&path, bytes).map_err(|error| format!("{}: {error}", path.display()))?;
    let status = std::process::Command::new("tar")
        .args(["-xzf"])
        .arg(&path)
        .arg("-C")
        .arg(&directory)
        .status()
        .map_err(|error| format!("tar: {error}"))?;
    if !status.success() {
        let _ = std::fs::remove_dir_all(&directory);
        return Err(format!("tar could not unpack {archive}"));
    }
    let binary = std::fs::read(directory.join("mycelium"))
        .map_err(|error| format!("{archive} has no mycelium binary: {error}"))?;
    let _ = std::fs::remove_dir_all(&directory);
    Ok(binary)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signed(
        version: &str,
        channel: &str,
        target: &str,
        binary: &[u8],
    ) -> (ReleaseManifest, String) {
        let key = ed25519_dalek::SigningKey::generate(&mut rand_core::OsRng);
        let manifest = ReleaseManifest::sign(
            &key,
            version.to_string(),
            channel.to_string(),
            target.to_string(),
            binary,
        )
        .unwrap();
        (
            manifest,
            mycelium_peer_protocol::encode_hex(key.verifying_key().as_bytes()),
        )
    }

    #[test]
    fn a_verified_manifest_is_selected_by_channel_and_target() {
        let (canary, key) = signed("0.3.0", "canary", "aarch64-apple-darwin", b"canary");
        let (stable, _) = signed("0.2.9", "stable", "aarch64-apple-darwin", b"stable");
        let targets = vec!["aarch64-apple-darwin".to_string()];
        let picked = select_verified(
            &[stable.clone(), canary.clone()],
            "canary",
            &targets,
            &[key],
        )
        .unwrap();
        assert_eq!(picked.version, "0.3.0");
    }

    #[test]
    fn an_untrusted_signer_never_wins_even_with_a_higher_version() {
        let (attacker, _) = signed("9.9.9", "stable", "aarch64-apple-darwin", b"evil");
        let (genuine, key) = signed("0.2.9", "stable", "aarch64-apple-darwin", b"good");
        let targets = vec!["aarch64-apple-darwin".to_string()];
        let picked = select_verified(&[attacker, genuine], "stable", &targets, &[key]).unwrap();
        assert_eq!(picked.version, "0.2.9");
    }

    #[test]
    fn a_tampered_manifest_is_refused() {
        let (mut manifest, key) = signed("0.2.9", "stable", "aarch64-apple-darwin", b"good");
        manifest.artifact_digest = mycelium_peer_protocol::sha256_hex(b"evil");
        let targets = vec!["aarch64-apple-darwin".to_string()];
        let error = select_verified(&[manifest], "stable", &targets, &[key]).unwrap_err();
        assert!(error.contains("signature or trust"), "{error}");
    }

    #[test]
    fn no_trusted_keys_is_an_error_not_a_default() {
        let (manifest, _) = signed("0.2.9", "stable", "aarch64-apple-darwin", b"good");
        let targets = vec!["aarch64-apple-darwin".to_string()];
        let error = select_verified(&[manifest], "stable", &targets, &[]).unwrap_err();
        assert!(error.contains("no trusted release keys"), "{error}");
    }

    #[test]
    fn metadata_names_the_archive_for_a_target() {
        let metadata =
            br#"{"targets":[{"target":"aarch64-apple-darwin","archive":"m-darwin.tar.gz"}]}"#;
        assert_eq!(
            archive_for(metadata, "aarch64-apple-darwin").unwrap(),
            "m-darwin.tar.gz"
        );
        assert!(archive_for(metadata, "x86_64-unknown-linux-musl").is_err());
    }

    /// The whole path, offline: a signed manifest, an archive, a local "GitHub
    /// release" directory, and a byte-for-byte check of what lands in the store.
    #[tokio::test]
    async fn fetch_verifies_then_seeds_the_store() {
        let target = mycelium_peer_protocol::local_compatible_targets()
            .into_iter()
            .next()
            .expect("a target");
        let work = std::env::temp_dir().join(format!("mycelium-fetch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&work);
        let release = work.join("release");
        let staging = work.join("staging");
        let store = work.join("store");
        std::fs::create_dir_all(&release).unwrap();
        std::fs::create_dir_all(&staging).unwrap();

        let binary = b"#!/bin/sh\necho mycelium-test\n".to_vec();
        std::fs::write(staging.join("mycelium"), &binary).unwrap();
        let archive = format!("mycelium-v0.0.1-{target}.tar.gz");
        let status = std::process::Command::new("tar")
            .args(["-czf"])
            .arg(release.join(&archive))
            .args(["-C"])
            .arg(&staging)
            .arg("mycelium")
            .status()
            .unwrap();
        assert!(status.success());

        let key = ed25519_dalek::SigningKey::generate(&mut rand_core::OsRng);
        let manifest = ReleaseManifest::sign(
            &key,
            "0.0.1".into(),
            "stable".into(),
            target.clone(),
            &binary,
        )
        .unwrap();
        let signer = mycelium_peer_protocol::encode_hex(key.verifying_key().as_bytes());
        std::fs::write(
            release.join(MANIFESTS_ASSET),
            serde_json::to_vec(&vec![manifest.clone()]).unwrap(),
        )
        .unwrap();
        std::fs::write(
            release.join(METADATA_ASSET),
            format!(
                "{{\"version\":\"0.0.1\",\"channel\":\"stable\",\"targets\":[{{\"target\":\"{target}\",\"archive\":\"{archive}\"}}]}}"
            ),
        )
        .unwrap();

        let source = Source {
            base: release.to_string_lossy().to_string(),
            channel: "stable".into(),
            trusted_keys: vec![signer],
        };
        let lines = fetch_into(&source, "stable", true, &store).await.unwrap();
        assert!(lines.iter().any(|line| line.contains("seed")), "{lines:?}");
        let seeded = store.join(&manifest.artifact_digest);
        assert_eq!(std::fs::read(&seeded).unwrap(), binary);

        // A key this host does not trust is refused before any bytes are read.
        let untrusted = Source {
            trusted_keys: vec![mycelium_peer_protocol::encode_hex(&[0u8; 32])],
            ..source.clone()
        };
        let error = fetch_into(&untrusted, "stable", true, &work.join("other-store"))
            .await
            .unwrap_err();
        assert!(error.contains("signature or trust"), "{error}");
        assert!(!work
            .join("other-store")
            .join(&manifest.artifact_digest)
            .exists());

        let _ = std::fs::remove_dir_all(&work);
    }
}
