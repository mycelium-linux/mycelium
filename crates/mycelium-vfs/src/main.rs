//! Fleet-aware `fungos-vfs` front door.
//!
//! The VFS owns content, sync and merge. This binary adds the two facts a
//! single-host CLI cannot know: which VFS roots exist on this machine, and how
//! a fleet peer name maps to a reachable SSH destination. Everything else is
//! passed through to `fungos-vfs`; there is no second sync implementation.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

const USAGE: &str = "\
mycelium-vfs — fleet-aware content-addressed VFS

usage:
  mycelium-vfs peers [--json]
  mycelium-vfs roots [--json]
  mycelium-vfs pull --root DIR --from PEER [--remote-root DIR] [--content all|none] [VFS OPTIONS...]
  mycelium-vfs serve --root DIR [VFS OPTIONS...]
  mycelium-vfs [--root DIR] <fungos-vfs command> [ARGS...]

environment:
  MYCELIUM_BIN       `mycelium` to ask for peers (default: mycelium)
  FUNGOS_VFS_BIN     `fungos-vfs` to run (default: fungos-vfs)
  FUNGOS_VFS_SSH     carrier used by `fungos-vfs sync pull` (default: ssh)
";

fn main() {
    if let Err(error) = run() {
        eprintln!("mycelium-vfs: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str).unwrap_or("help") {
        "help" | "--help" | "-h" => {
            print!("{USAGE}");
            Ok(())
        }
        "peers" => peers(&args[1..]),
        "roots" => roots(&args[1..]),
        "pull" | "sync-pull" => pull(&args[1..]),
        "serve" => serve(&args[1..]),
        _ => passthrough(&args),
    }
}

fn peers(args: &[String]) -> Result<(), String> {
    let json = only_flag(args, "--json")?;
    let value = mycelium_peers()?;
    let mut rows = Vec::new();
    let mut json_rows = Vec::new();
    for peer in value.as_array().into_iter().flatten() {
        let Some(hello) = peer.get("hello") else {
            continue;
        };
        let hostname = hello["hostname"].as_str().unwrap_or("?");
        let site = hello["site"].as_str().unwrap_or("");
        let node_id = hello["node_id"].as_str().unwrap_or("");
        let addresses: Vec<String> = peer["observed_endpoints"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|endpoint| endpoint["address"].as_str())
            .filter(|address| !address.is_empty())
            .map(str::to_owned)
            .collect();
        let target = addresses
            .first()
            .cloned()
            .unwrap_or_else(|| hostname.to_owned());
        if json {
            json_rows.push(serde_json::json!({
                "hostname": hostname,
                "site": site,
                "node_id": node_id,
                "target": target,
                "addresses": addresses,
            }));
        } else {
            rows.push(format!(
                "{hostname:<24} {site:<18} {:<12} {target}",
                &node_id[..node_id.len().min(12)]
            ));
        }
    }
    if json {
        println!("{}", Value::Array(json_rows));
        return Ok(());
    }
    print_rows(rows)
}

fn roots(args: &[String]) -> Result<(), String> {
    let json = only_flag(args, "--json")?;
    let mut rows = Vec::new();
    let home = home()?;
    let registry = home.join(".config/fungos/desktops");
    if let Ok(entries) = std::fs::read_dir(&registry) {
        let mut files: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("toml"))
            .collect();
        files.sort();
        for path in files {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let id = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("?")
                .to_owned();
            let root = key(&text, "root").unwrap_or_else(|| {
                home.join(".local/share/fungos/desktops")
                    .join(&id)
                    .display()
                    .to_string()
            });
            let name = key(&text, "name").unwrap_or_else(|| id.clone());
            rows.push((id, name, expand_home(&root, &home)));
        }
    }
    let default = home.join(".local/share/fungos/vfs");
    if default.is_dir() && !rows.iter().any(|(_, _, root)| root == &default) {
        rows.push(("vfs".into(), "VFS".into(), default));
    }
    if json {
        let values: Vec<Value> = rows
            .into_iter()
            .map(|(id, name, root)| {
                serde_json::json!({"id": id, "name": name, "root": root.display().to_string()})
            })
            .collect();
        println!("{}", Value::Array(values));
        return Ok(());
    }
    let lines: Vec<String> = rows
        .into_iter()
        .map(|(id, name, root)| format!("{id:<24} {name:<24} {}", root.display()))
        .collect();
    print_rows(lines)
}

fn pull(args: &[String]) -> Result<(), String> {
    let mut root = None;
    let mut from = None;
    let mut remote_root = None;
    let mut passthrough = Vec::new();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--root" => {
                index += 1;
                root = args.get(index).cloned();
            }
            "--from" | "--peer" => {
                index += 1;
                from = args.get(index).cloned();
            }
            "--remote-root" => {
                index += 1;
                remote_root = args.get(index).cloned();
            }
            other => passthrough.push(other.to_owned()),
        }
        index += 1;
    }
    let root = root.ok_or("pull needs --root DIR")?;
    let from = from.ok_or("pull needs --from PEER")?;
    let target = resolve_peer(&from)?;
    let remote = remote_root.unwrap_or_else(|| root.clone());
    let vfs = vfs_bin();
    let mut command = Command::new(&vfs);
    command
        .arg("--root")
        .arg(&root)
        .args([
            "sync",
            "pull",
            "--remote",
            &target,
            "--remote-root",
            &remote,
        ])
        .args(&passthrough);
    run_command(&mut command, &vfs)
}

fn serve(args: &[String]) -> Result<(), String> {
    let mut root = None;
    let mut passthrough = Vec::new();
    let mut index = 0;
    while index < args.len() {
        if args[index] == "--root" {
            index += 1;
            root = args.get(index).cloned();
        } else {
            passthrough.push(args[index].clone());
        }
        index += 1;
    }
    let root = root.ok_or("serve needs --root DIR")?;
    let vfs = vfs_bin();
    let mut command = Command::new(&vfs);
    command
        .arg("--root")
        .arg(&root)
        .args(["sync", "serve"])
        .args(&passthrough);
    run_command(&mut command, &vfs)
}

fn passthrough(args: &[String]) -> Result<(), String> {
    let vfs = vfs_bin();
    let mut command = Command::new(&vfs);
    command.args(args);
    run_command(&mut command, &vfs)
}

fn run_command(command: &mut Command, program: &str) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let error = command.exec();
        Err(format!("{program}: {error}"))
    }
    #[cfg(not(unix))]
    {
        let status = command
            .status()
            .map_err(|error| format!("{program}: {error}"))?;
        if status.success() {
            Ok(())
        } else {
            std::process::exit(status.code().unwrap_or(1));
        }
    }
}

fn resolve_peer(name: &str) -> Result<String, String> {
    let value = mycelium_peers()?;
    let mut matches = Vec::new();
    for peer in value.as_array().into_iter().flatten() {
        let Some(hello) = peer.get("hello") else {
            continue;
        };
        let hostname = hello["hostname"].as_str().unwrap_or("");
        let site = hello["site"].as_str().unwrap_or("");
        let node_id = hello["node_id"].as_str().unwrap_or("");
        let qualifies = name == hostname
            || name == format!("{hostname}.{site}")
            || (!node_id.is_empty() && node_id.starts_with(name));
        if !qualifies {
            continue;
        }
        let target = peer["observed_endpoints"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|endpoint| endpoint["address"].as_str())
            .find(|address| !address.is_empty())
            .unwrap_or(hostname);
        matches.push(target.to_owned());
    }
    match matches.len() {
        1 => Ok(matches.remove(0)),
        0 => Err(format!("no peer matches `{name}`")),
        _ => Err(format!(
            "`{name}` matches more than one peer; use the full name"
        )),
    }
}

fn mycelium_peers() -> Result<Value, String> {
    let binary = std::env::var("MYCELIUM_BIN").unwrap_or_else(|_| "mycelium".into());
    let output = Command::new(&binary)
        .args(["peers", "--json"])
        .output()
        .map_err(|error| format!("{binary} peers --json: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "{binary} peers --json exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    serde_json::from_slice(&output.stdout).map_err(|error| format!("parse peers: {error}"))
}

fn only_flag(args: &[String], flag: &str) -> Result<bool, String> {
    if let Some(other) = args.iter().find(|argument| argument.as_str() != flag) {
        return Err(format!("unknown argument `{other}`"));
    }
    Ok(args.iter().any(|argument| argument == flag))
}

fn vfs_bin() -> String {
    std::env::var("FUNGOS_VFS_BIN").unwrap_or_else(|_| "fungos-vfs".into())
}

fn home() -> Result<PathBuf, String> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| "HOME is not set".into())
}

fn key(text: &str, key: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        if let Some((name, value)) = line.split_once('=') {
            if name.trim() == key {
                return Some(value.trim().trim_matches('"').trim_matches('\'').to_owned());
            }
        }
    }
    None
}

fn expand_home(value: &str, home: &Path) -> PathBuf {
    match value.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None => PathBuf::from(value),
    }
}

fn print_rows(rows: Vec<String>) -> Result<(), String> {
    for row in rows {
        println!("{row}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sibling_names_match_only_one_way() {
        assert_eq!(
            key("root = \"~/vfs\"\nname = \"FPL\"\n", "root").as_deref(),
            Some("~/vfs")
        );
        assert_eq!(key("name = \"FPL\"\n", "name").as_deref(), Some("FPL"));
        assert_eq!(key("name = 'FPL'\n", "name").as_deref(), Some("FPL"));
        assert_eq!(key("name = \"FPL\"\n", "root"), None);
    }

    #[test]
    fn a_root_expands_home_only_at_the_front() {
        let home = Path::new("/Users/me");
        assert_eq!(expand_home("~/vfs", home), PathBuf::from("/Users/me/vfs"));
        assert_eq!(expand_home("/srv/vfs", home), PathBuf::from("/srv/vfs"));
        assert_eq!(expand_home("vfs", home), PathBuf::from("vfs"));
    }
}
