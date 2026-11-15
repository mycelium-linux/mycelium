//! Local allocation ledger for authority-signed Unix identities.
//!
//! The ledger is a convenience for the human running the access authority: it
//! keeps the next free UID/GID from colliding when several grants are composed.
//! It is not authoritative. The signed `unix_ids` map in the grant is what
//! hosts enforce.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const DEFAULT_MIN: u32 = 3000;
const DEFAULT_MAX: u32 = 5999;
const LEDGER_VERSION: u32 = 1;

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Ledger {
    version: u32,
    #[serde(default)]
    allocations: BTreeMap<String, Identity>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
struct Identity {
    uid: u32,
    gid: u32,
}

pub(crate) fn run(args: &[String]) -> Result<Vec<String>, String> {
    match args.first().map(String::as_str) {
        Some("allocate") => allocate(args),
        Some("list") => list(args),
        _ => Err("access unix-id needs allocate or list".into()),
    }
}

fn allocate(args: &[String]) -> Result<Vec<String>, String> {
    if !args.iter().any(|arg| arg == "--write") {
        return Err("unix-id allocation requires --write".into());
    }
    let users: Vec<String> = args
        .get(1..)
        .unwrap_or_default()
        .iter()
        .filter(|arg| !arg.starts_with("--"))
        .filter(|arg| {
            !matches!(
                arg.as_str(),
                "allocate" | "list" | "--write" | "--json" | "--path" | "--min" | "--max"
            )
        })
        .cloned()
        .collect();
    let users = users_without_flag_values(args, users);
    if users.is_empty() {
        return Err("unix-id allocate needs at least one Unix user".into());
    }
    let path = path(args);
    let min = number(args, "--min").unwrap_or(DEFAULT_MIN);
    let max = number(args, "--max").unwrap_or(DEFAULT_MAX);
    if min < 1000 || max < min || max == u32::MAX {
        return Err("unix-id range must start at 1000 or above and be non-empty".into());
    }
    let mut ledger = load(&path)?;
    let mut used: BTreeSet<u32> = ledger
        .allocations
        .values()
        .flat_map(|id| [id.uid, id.gid])
        .collect();
    let mut next = min;
    let mut changed = false;
    for user in users {
        validate_user(&user)?;
        if ledger.allocations.contains_key(&user) {
            continue;
        }
        while next <= max && used.contains(&next) {
            next += 1;
        }
        if next > max {
            return Err(format!(
                "no free UID/GID remains in {min}..={max} for `{user}`"
            ));
        }
        let id = Identity {
            uid: next,
            gid: next,
        };
        used.insert(next);
        ledger.allocations.insert(user, id);
        next += 1;
        changed = true;
    }
    if changed {
        write(&path, &ledger)?;
    }
    let value = serde_json::json!({
        "path": path.display().to_string(),
        "unix_ids": ledger.allocations,
    });
    Ok(vec![
        serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?
    ])
}

fn list(args: &[String]) -> Result<Vec<String>, String> {
    let path = path(args);
    let ledger = load(&path)?;
    let value = serde_json::json!({
        "path": path.display().to_string(),
        "unix_ids": ledger.allocations,
    });
    Ok(vec![
        serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?
    ])
}

fn users_without_flag_values(args: &[String], users: Vec<String>) -> Vec<String> {
    let mut values = BTreeSet::new();
    let mut index = 0;
    while index < args.len() {
        if matches!(args[index].as_str(), "--path" | "--min" | "--max") {
            index += 1;
            if let Some(value) = args.get(index) {
                values.insert(value.clone());
            }
        }
        index += 1;
    }
    users
        .into_iter()
        .filter(|user| !values.contains(user))
        .collect()
}

fn path(args: &[String]) -> PathBuf {
    find(args, "--path")
        .map(PathBuf::from)
        .unwrap_or_else(|| myceliumd::home_dir().join("access/unix-ids.json"))
}

fn number(args: &[String], flag: &str) -> Option<u32> {
    find(args, flag).and_then(|value| value.parse().ok())
}

fn find<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.windows(2)
        .find(|window| window[0] == flag)
        .map(|window| window[1].as_str())
}

fn validate_user(user: &str) -> Result<(), String> {
    if user.is_empty()
        || user.len() > 32
        || !user.as_bytes()[0].is_ascii_lowercase()
        || !user
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"_-".contains(&byte))
        || user == "root"
    {
        return Err(format!("unsafe Unix account name `{user}`"));
    }
    Ok(())
}

fn load(path: &Path) -> Result<Ledger, String> {
    if !path.exists() {
        return Ok(Ledger {
            version: LEDGER_VERSION,
            allocations: BTreeMap::new(),
        });
    }
    let metadata =
        fs::symlink_metadata(path).map_err(|e| format!("inspect {}: {e}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(format!("ledger {} must be a regular file", path.display()));
    }
    let ledger: Ledger = serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?)
        .map_err(|e| format!("parse {}: {e}", path.display()))?;
    if ledger.version != LEDGER_VERSION {
        return Err(format!(
            "ledger {} has unsupported version {}",
            path.display(),
            ledger.version
        ));
    }
    let mut seen = BTreeMap::new();
    for (user, id) in &ledger.allocations {
        validate_user(user)?;
        if id.uid < 1000 || id.gid < 1000 || id.uid == 65_534 || id.gid == 65_534 {
            return Err(format!(
                "ledger entry for `{user}` is not a managed UID/GID"
            ));
        }
        if let Some(other) = seen.insert(id.uid, user) {
            if other != user {
                return Err(format!(
                    "ledger assigns duplicate uid {} to both `{other}` and `{user}`",
                    id.uid
                ));
            }
        }
        if id.gid != id.uid {
            if let Some(other) = seen.insert(id.gid, user) {
                if other != user {
                    return Err(format!(
                        "ledger assigns duplicate gid {} to both `{other}` and `{user}`",
                        id.gid
                    ));
                }
            }
        }
    }
    Ok(ledger)
}

fn write(path: &Path, ledger: &Ledger) -> Result<(), String> {
    let parent = path.parent().ok_or("ledger path has no parent")?;
    fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
        .map_err(|e| format!("protect {}: {e}", parent.display()))?;
    // The ledger is authority-local state, not gossip. Keep the replacement
    // atomic and private; a partial write must not look like a valid ledger.
    let temp = path.with_extension(format!("tmp-{}", std::process::id()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temp)
        .map_err(|e| format!("create {}: {e}", temp.display()))?;
    let result = (|| {
        file.write_all(&serde_json::to_vec_pretty(ledger).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        fs::rename(&temp, path).map_err(|e| e.to_string())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("mycelium-unix-ids-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn run_ok(args: &[&str]) -> serde_json::Value {
        let args = args.iter().map(|arg| arg.to_string()).collect::<Vec<_>>();
        let out = run(&args).unwrap();
        serde_json::from_str(&out[0]).unwrap()
    }

    #[test]
    fn allocation_is_stable_and_reuses_gaps() {
        let path = dir("stable").join("ledger.json");
        let path = path.to_str().unwrap();
        let first = run_ok(&[
            "allocate", "alice", "bob", "--path", path, "--min", "4000", "--max", "4002", "--write",
        ]);
        assert_eq!(first["unix_ids"]["alice"]["uid"], 4000);
        assert_eq!(first["unix_ids"]["bob"]["uid"], 4001);
        let again = run_ok(&[
            "allocate", "alice", "--path", path, "--min", "4000", "--max", "4002", "--write",
        ]);
        assert_eq!(again["unix_ids"]["alice"]["uid"], 4000);
        let third = run_ok(&[
            "allocate", "carol", "--path", path, "--min", "4000", "--max", "4002", "--write",
        ]);
        assert_eq!(third["unix_ids"]["carol"]["uid"], 4002);
    }

    #[test]
    fn a_hand_edited_duplicate_ledger_is_rejected() {
        let path = dir("duplicate").join("ledger.json");
        std::fs::write(
            &path,
            r#"{"version":1,"allocations":{"alice":{"uid":4000,"gid":4000},"bob":{"uid":4000,"gid":4001}}}"#,
        )
        .unwrap();
        let args = vec![
            "list".to_string(),
            "--path".to_string(),
            path.to_str().unwrap().to_string(),
        ];
        assert!(run(&args).unwrap_err().contains("duplicate"));
    }

    #[test]
    fn allocation_refuses_exhaustion_and_bad_names() {
        let path = dir("exhaust").join("ledger.json");
        let path = path.to_str().unwrap();
        run_ok(&[
            "allocate", "alice", "--path", path, "--min", "4000", "--max", "4000", "--write",
        ]);
        let args = vec![
            "allocate".to_string(),
            "bob".to_string(),
            "--path".to_string(),
            path.to_string(),
            "--min".to_string(),
            "4000".to_string(),
            "--max".to_string(),
            "4000".to_string(),
            "--write".to_string(),
        ];
        assert!(run(&args).unwrap_err().contains("no free UID/GID"));
        let args = vec![
            "allocate".to_string(),
            "Root".to_string(),
            "--path".to_string(),
            path.to_string(),
            "--write".to_string(),
        ];
        assert!(run(&args).unwrap_err().contains("unsafe"));
    }
}
