//! The built-in dashboard served at `home.myc`.
//!
//! This replaces the old soil-home launcher. The frontdoor already owns TLS,
//! routing and the user's session, so it is the right process to answer the
//! settings surface: the dashboard is a client of the same `mycelium … --json`
//! reads and `--write` commands a person would run in a terminal, and it keeps
//! no state the daemon does not already own (the one exception is the small
//! `dashboard.json` preference file).
//!
//! The four sections are deliberately read-mostly where the host has no safe
//! write verb: geode leases and VFS mounts are operator authority, not form
//! fields. Pulse, relays, the daemon and the frontdoor route table do have real
//! verbs, and those are what the Settings and Pulse pages call.

use crate::server::FrontdoorState;
use axum::body::{Body, Bytes};
use axum::http::{header, Method, Request, Response, StatusCode};
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::process::Command;

const INDEX: &str = include_str!("../dashboard/index.html");
const MAX_BODY_BYTES: usize = 256 * 1024;
const MAX_OUTPUT_BYTES: usize = 4 * 1024 * 1024;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);

/// Handle a request addressed to a dashboard host. The caller has already
/// checked the frontdoor bearer token.
pub async fn handle(state: &FrontdoorState, request: Request<Body>) -> Response<Body> {
    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    let query = request.uri().query().unwrap_or_default().to_owned();

    // Mutations are cross-site-forgeable if the browser is holding a frontdoor
    // credential, so every non-GET must carry a header only this page's JS
    // sets. The check is cheap and makes a normal HTML form insufficient.
    if method != Method::GET {
        let marked = request
            .headers()
            .get("x-mycelium-dashboard")
            .and_then(|value| value.to_str().ok())
            == Some("1");
        if !marked {
            return api_error(StatusCode::FORBIDDEN, "dashboard header required");
        }
    }

    let body = match axum::body::to_bytes(request.into_body(), MAX_BODY_BYTES).await {
        Ok(body) => body,
        Err(_) => return api_error(StatusCode::PAYLOAD_TOO_LARGE, "request body too large"),
    };

    match (method, path.as_str()) {
        (Method::GET, "/") | (Method::GET, "/index.html") => html(INDEX),
        (Method::GET, "/api/summary") => json_response(summary(state).await),
        (Method::GET, "/api/fleet") => {
            let bin = binary(state);
            match call_json(&bin, &["fleet", "status", "--json"]).await {
                Ok(value) => json_response(json!({"ok": true, "peers": value})),
                Err(err) => json_response(json!({"ok": false, "error": err, "peers": []})),
            }
        }
        (Method::GET, "/api/geode") => json_response(json!({"ok": true, "geode": geode()})),
        (Method::GET, "/api/vfs") => json_response(json!({"ok": true, "vfs": vfs()})),
        (Method::GET, "/api/pulse") => {
            let bin = binary(state);
            let list = call_json(&bin, &["pulse", "list", "--json"]).await;
            let runs = call_json(&bin, &["pulse", "runs", "--limit", "20", "--json"]).await;
            json_response(json!({
                "ok": list.is_ok(),
                "pulses": list.unwrap_or(json!([])),
                "runs": runs.unwrap_or(json!([])),
            }))
        }
        (Method::GET, "/api/pulse/runs") => {
            let id = query_value(&query, "id").unwrap_or_default();
            let bin = binary(state);
            let mut args = vec!["pulse".to_owned(), "runs".to_owned()];
            if !id.is_empty() {
                args.push(id);
            }
            args.extend(["--limit".into(), "100".into(), "--json".into()]);
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            match call_json(&bin, &args).await {
                Ok(value) => json_response(json!({"ok": true, "runs": value})),
                Err(err) => json_response(json!({"ok": false, "error": err, "runs": []})),
            }
        }
        (Method::GET, "/api/settings") => {
            json_response(json!({"ok": true, "settings": read_settings()}))
        }
        (Method::GET, "/api/routes") => json_response(routes(state)),
        (Method::PUT, "/api/routes") => put_routes(state, &body).await,
        (Method::PUT, "/api/settings") => put_settings(state, &body).await,
        (Method::PUT, "/api/overlay") => put_overlay(state, &body).await,
        (Method::POST, "/api/pulse") => pulse_add(state, &body).await,
        (Method::POST, "/api/pulse/toggle") => pulse_toggle(state, &body).await,
        (Method::POST, "/api/pulse/remove") => pulse_remove(state, &body).await,
        (Method::POST, "/api/actions") => action(state, &body).await,
        (Method::GET, _) => api_error(StatusCode::NOT_FOUND, "not found"),
        _ => api_error(StatusCode::METHOD_NOT_ALLOWED, "method not allowed"),
    }
}

// ---------------------------------------------------------------- responses

fn html(body: &str) -> Response<Body> {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(body.to_owned()))
        .unwrap_or_else(|_| api_error(StatusCode::INTERNAL_SERVER_ERROR, "invalid response"))
}

fn json_response(value: Value) -> Response<Body> {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(value.to_string()))
        .unwrap_or_else(|_| api_error(StatusCode::INTERNAL_SERVER_ERROR, "invalid response"))
}

fn api_error(status: StatusCode, message: &str) -> Response<Body> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(
            json!({"ok": false, "error": message}).to_string(),
        ))
        .unwrap_or_default()
}

fn query_value(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        (name == key).then(|| percent_decode(value))
    })
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok();
                if let Some(byte) = hex.and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                    out.push(byte);
                    index += 3;
                    continue;
                }
                out.push(bytes[index]);
                index += 1;
            }
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

fn parse_body(body: &Bytes) -> Result<Value, String> {
    if body.is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_slice(body).map_err(|error| format!("invalid JSON: {error}"))
}

// ---------------------------------------------------------------- commands

fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

fn binary(state: &FrontdoorState) -> Option<PathBuf> {
    if let Some(path) = &state.mycelium_bin {
        if path.is_absolute() {
            if path.is_file() {
                return Some(path.clone());
            }
        } else if let Some(found) = find_on_path(&path.display().to_string()) {
            return Some(found);
        }
    }
    find_on_path("mycelium")
}

async fn call_json(bin: &Option<PathBuf>, args: &[&str]) -> Result<Value, String> {
    let Some(bin) = bin else {
        return Err("mycelium is not installed".into());
    };
    let mut command = Command::new(bin);
    command.args(args).kill_on_drop(true);
    let output = tokio::time::timeout(COMMAND_TIMEOUT, command.output())
        .await
        .map_err(|_| format!("{} timed out", args.join(" ")))?
        .map_err(|error| format!("run {}: {error}", bin.display()))?;
    if output.stdout.len() > MAX_OUTPUT_BYTES {
        return Err("mycelium output exceeded the dashboard bound".into());
    }
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let message = stderr.trim();
        return Err(if message.is_empty() {
            format!("mycelium {} exited {}", args.join(" "), output.status)
        } else {
            message.to_owned()
        });
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("parse mycelium {} output: {error}", args.join(" ")))
}

async fn call_write(bin: &Option<PathBuf>, args: &[String]) -> Result<Value, String> {
    let Some(bin) = bin else {
        return Err("mycelium is not installed".into());
    };
    let mut command = Command::new(bin);
    command.args(args).kill_on_drop(true);
    let output = tokio::time::timeout(COMMAND_TIMEOUT, command.output())
        .await
        .map_err(|_| format!("{} timed out", args.join(" ")))?
        .map_err(|error| format!("run {}: {error}", bin.display()))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if !output.status.success() {
        return Err(if stderr.is_empty() { stdout } else { stderr });
    }
    Ok(json!({"ok": true, "output": stdout, "stderr": stderr}))
}

// ---------------------------------------------------------------- summary

async fn summary(state: &FrontdoorState) -> Value {
    let bin = binary(state);
    let (node, fleet, tunnel, pulse, peers) = tokio::join!(
        call_json(&bin, &["node", "status", "--json"]),
        call_json(&bin, &["fleet", "status", "--json"]),
        call_json(&bin, &["iroh-tun", "status", "--json"]),
        call_json(&bin, &["pulse", "list", "--json"]),
        call_json(&bin, &["peers", "--json"]),
    );
    let (geode, vfs, unibus) = tokio::join!(
        tokio::task::spawn_blocking(geode),
        tokio::task::spawn_blocking(vfs),
        tokio::task::spawn_blocking(unibus)
    );
    let table = state.table.read().unwrap();
    json!({
        "ok": true,
        "node": result_or_error(node),
        "fleet": result_or_error(decorate_fleet(fleet, peers)),
        "tunnel": result_or_error(tunnel),
        "overlay_config": overlay_config(),
        "pulse": result_or_error(pulse),
        "geode": geode.unwrap_or_else(|_| json!({"error": "geode probe failed"})),
        "vfs": vfs.unwrap_or_else(|_| json!({"error": "vfs probe failed"})),
        "unibus": unibus.unwrap_or_else(|_| json!({"error": "unibus probe failed"})),
        "frontdoor": {
            "routes": table.total_routes(),
            "derived_routes": table.derived_routes(),
            "auth_required": state.auth_token.is_some(),
            "tls": state.forwarded_proto == "https",
        },
        "settings": read_settings(),
        "dashboard_hosts": state.dashboard_hosts.read().unwrap().iter().cloned().collect::<Vec<_>>(),
    })
}

fn result_or_error(result: Result<Value, String>) -> Value {
    match result {
        Ok(value) => json!({"ok": true, "value": value}),
        Err(error) => json!({"ok": false, "error": error}),
    }
}

/// Join `fleet status` with the richer `peers` projection. The fleet table is
/// what the daemon publishes to every console; `peers` adds the addresses and
/// capabilities that make a row worth opening. Hardware inventories are
/// dropped here: a dashboard does not need a PCI tree to name a peer.
fn decorate_fleet(
    fleet: Result<Value, String>,
    peers: Result<Value, String>,
) -> Result<Value, String> {
    let mut fleet = fleet?;
    let Ok(peers) = peers else {
        return Ok(fleet);
    };
    let mut details = std::collections::BTreeMap::new();
    for peer in peers.as_array().into_iter().flatten() {
        let hello = &peer["hello"];
        let Some(id) = hello["node_id"].as_str().filter(|id| !id.is_empty()) else {
            continue;
        };
        let addresses: Vec<Value> = hello["interfaces"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|interface| {
                interface["addresses"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
            })
            .collect();
        details.insert(
            id.to_owned(),
            json!({
                "node_id": id,
                "addresses": addresses,
                "capabilities": hello["capabilities"].clone(),
                "platform": hello["platform"].clone(),
                "architecture": hello["architecture"].clone(),
                "ssh_listening": peer["health"]["ssh_listening"].clone(),
                "load_average": peer["health"]["load_average"].clone(),
                "transports": peer["transports"].as_array().map(Vec::len).unwrap_or(0),
            }),
        );
    }
    if let Some(rows) = fleet.as_array_mut() {
        // A peer that has a heartbeat but no hello is a row nobody can act on;
        // keep the table to peers that can actually be addressed.
        rows.retain(|row| row["hostname"].is_string() || row["node_id"].is_string());
        for row in rows {
            let Some(id) = row["node_id"].as_str() else {
                continue;
            };
            if let Some(detail) = details.get(id).and_then(Value::as_object) {
                if let Some(fields) = row.as_object_mut() {
                    for (key, value) in detail {
                        fields.entry(key.clone()).or_insert_with(|| value.clone());
                    }
                }
            }
        }
    }
    Ok(fleet)
}

fn config_home() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".config"))
}

/// The Unibus hub is a TCP bus, not an HTTP service, so the dashboard reports
/// what it can verify without inventing an admin protocol: the configured
/// producers and routes, whether the listener answers, and the recent hub log.
fn unibus() -> Value {
    let config_path = config_home().join("unibus/hub.json");
    let text = std::fs::read_to_string(&config_path).unwrap_or_default();
    let config: Value = serde_json::from_str(&text).unwrap_or_else(|_| json!({}));
    let listen = config["listen"].as_str().unwrap_or("127.0.0.1:18890");
    let reachable = probe_tcp(listen, Duration::from_millis(500));

    let producers: Vec<Value> = config["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|node| {
            let token = node["token_file"].as_str().unwrap_or_default();
            json!({
                "id": node["producer_id"],
                "zones": node["zones"].clone(),
                "capabilities": node["capabilities"].clone(),
                "publishes": node["publishes"].clone(),
                "token_present": !token.is_empty() && Path::new(token).is_file(),
            })
        })
        .collect();
    let routes: Vec<Value> = config["routes"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|route| {
            json!({
                "kind": route["kind"],
                "mode": route["target"]["mode"],
                "holder": route["target"]["name"],
                "peer": route["target"]["peer_id"],
            })
        })
        .collect();
    let log_path = config_home().join("unibus/hub.log");
    json!({
        "config": config_path.display().to_string(),
        "config_present": config_path.is_file(),
        "log": log_path.display().to_string(),
        "listen": listen,
        "reachable": reachable,
        "installation": config["installation_id"],
        "allow_insecure_lan": config["allow_insecure_lan"].clone(),
        "producers": producers,
        "routes": routes,
        "recent": tail_lines(&log_path, 14),
    })
}

fn overlay_config() -> Value {
    let path = mycelium_home().join("iroh-tun.json");
    let config = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok());
    json!({"path": path.display().to_string(), "config": config})
}

async fn put_overlay(state: &FrontdoorState, body: &Bytes) -> Response<Body> {
    let input = match parse_body(body) {
        Ok(value) => value,
        Err(error) => return api_error(StatusCode::BAD_REQUEST, &error),
    };
    let mut args = vec!["iroh-tun".to_owned(), "config".to_owned()];
    if let Some(enabled) = input.get("enabled").and_then(Value::as_bool) {
        args.push("--enabled".into());
        args.push(enabled.to_string());
    }
    for (key, flag) in [
        ("tun", "--tun"),
        ("listen", "--listen"),
        ("status", "--status"),
        ("binary", "--binary"),
    ] {
        if let Some(value) = input.get(key).and_then(Value::as_str) {
            if value.len() > 200 || value.contains('\n') {
                return api_error(StatusCode::BAD_REQUEST, &format!("{key} is invalid"));
            }
            args.push(flag.into());
            args.push(value.to_owned());
        }
    }
    if let Some(mtu) = input.get("mtu").and_then(Value::as_u64) {
        args.push("--mtu".into());
        args.push(mtu.to_string());
    }
    if let Some(refresh) = input.get("refresh_secs").and_then(Value::as_u64) {
        args.push("--refresh-secs".into());
        args.push(refresh.to_string());
    }
    if let Some(external) = input.get("external").and_then(Value::as_bool) {
        args.push("--external".into());
        args.push(external.to_string());
    }
    if let Some(overlay) = input.get("overlay").and_then(Value::as_array) {
        for cidr in overlay {
            let Some(cidr) = cidr.as_str().filter(|cidr| !cidr.trim().is_empty()) else {
                continue;
            };
            if cidr.len() > 64 || cidr.contains('\n') {
                return api_error(StatusCode::BAD_REQUEST, "overlay CIDR is invalid");
            }
            args.push("--overlay".into());
            args.push(cidr.to_owned());
        }
    }
    if args.len() <= 2 {
        return api_error(StatusCode::BAD_REQUEST, "no overlay settings supplied");
    }
    args.push("--write".into());
    let bin = binary(state);
    match call_write(&bin, &args).await {
        Ok(value) => {
            let status = call_json(&bin, &["iroh-tun", "status", "--json"])
                .await
                .ok();
            json_response(json!({
                "ok": true,
                "output": value["output"],
                "status": status,
                "config": overlay_config()["config"],
            }))
        }
        Err(error) => json_response(json!({"ok": false, "error": error})),
    }
}

fn probe_tcp(listen: &str, timeout: Duration) -> bool {
    let Some((host, port)) = listen.rsplit_once(':') else {
        return false;
    };
    let host = match host.trim_matches(['[', ']']) {
        "0.0.0.0" | "" | "::" => "127.0.0.1",
        host => host,
    };
    let Ok(port) = port.parse::<u16>() else {
        return false;
    };
    let Ok(addr) = format!("{host}:{port}").parse::<std::net::SocketAddr>() else {
        return false;
    };
    std::net::TcpStream::connect_timeout(&addr, timeout).is_ok()
}

fn strip_ansi(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut escape = false;
    for ch in line.chars() {
        if escape {
            if ch.is_ascii_alphabetic() {
                escape = false;
            }
            continue;
        }
        if ch == '\u{1b}' {
            escape = true;
            continue;
        }
        out.push(ch);
    }
    out
}

fn tail_lines(path: &Path, count: usize) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let lines: Vec<&str> = text.lines().rev().take(count * 4).collect();
    lines
        .into_iter()
        .rev()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let clean = strip_ansi(line);
            clean.chars().take(360).collect::<String>()
        })
        .rev()
        .take(count)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

// ---------------------------------------------------------------- probes

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

fn mycelium_home() -> PathBuf {
    std::env::var_os("MYCELIUM_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".mycelium"))
}

fn geode() -> Value {
    let dir = home_dir().join(".geode");
    let socket = dir.join("geode.sock");
    let store = dir.join("store.db");
    let key = dir.join("store.key");

    #[cfg(unix)]
    let running = std::os::unix::net::UnixStream::connect(&socket).is_ok();
    #[cfg(not(unix))]
    let running = false;

    let store_bytes = std::fs::metadata(&store)
        .map(|meta| meta.len())
        .unwrap_or(0);
    let key_mode = std::fs::metadata(&key)
        .ok()
        .filter(|meta| {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                meta.permissions().mode() & 0o777 == 0o600
            }
            #[cfg(not(unix))]
            {
                let _ = meta;
                true
            }
        })
        .is_some();

    json!({
        "running": running,
        "socket": socket.display().to_string(),
        "socket_present": socket.is_file(),
        "store": store.display().to_string(),
        "store_bytes": store_bytes,
        "key_present": key.is_file(),
        "key_private": key_mode,
        // Read-only by design: leasing a secret is an authenticated caller
        // action, never a dashboard read.
        "read_only": true,
    })
}

fn vfs() -> Value {
    let home = home_dir();
    let roots = [
        home.join(".local/share/fungos/vfs"),
        home.join("fungOS"),
        home.join("fungos-node"),
    ];
    let mounts = mount_points();
    let root_rows: Vec<Value> = roots
        .iter()
        .filter(|path| path.exists())
        .map(|path| {
            let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
            let mounted = mounts.iter().any(|mount| {
                std::fs::canonicalize(mount)
                    .map(|mounted| mounted == canonical)
                    .unwrap_or(false)
            });
            let entries = std::fs::read_dir(path)
                .map(|reader| reader.filter_map(Result::ok).take(500).count())
                .unwrap_or(0);
            json!({
                "path": path.display().to_string(),
                "mounted": mounted,
                "entries": entries,
            })
        })
        .collect();

    json!({
        "roots": root_rows,
        "desktops": desktops(),
        "files": {
            "url": "https://files.myc/",
            "running": probe_tcp("127.0.0.1:8803", Duration::from_millis(350)),
        },
    })
}

fn mount_points() -> BTreeSet<PathBuf> {
    let mut points = BTreeSet::new();
    if let Ok(output) = std::process::Command::new("mount").output() {
        if output.stdout.len() < 512 * 1024 {
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                if let Some(rest) = line.split_once(" on ").map(|(_, rest)| rest) {
                    if let Some((path, _)) = rest.split_once(" (") {
                        points.insert(PathBuf::from(path));
                    }
                }
            }
        }
    }
    points
}

/// Canvas/desktop Spores: the ones that declare a surface, not every Spore.
fn desktops() -> Vec<Value> {
    let home = home_dir();
    let roots = [
        home.join("Library/Application Support/fungOS/spores"),
        home.join(".local/share/fungos/spores"),
    ];
    let mut found: std::collections::BTreeMap<String, Value> = std::collections::BTreeMap::new();
    for root in roots {
        let Ok(children) = std::fs::read_dir(&root) else {
            continue;
        };
        for child in children.flatten() {
            let manifest = child.path().join("spore.toml");
            let Ok(text) = std::fs::read_to_string(&manifest) else {
                continue;
            };
            let parsed = parse_spore(&text);
            let Some(id) = parsed.get("id").and_then(Value::as_str).map(str::to_owned) else {
                continue;
            };
            let declares_canvas = text.contains("soil:canvas") || text.contains("soil:present");
            if !declares_canvas {
                continue;
            }
            let mut entry = parsed;
            entry.insert(
                "path".into(),
                Value::String(child.path().display().to_string()),
            );
            found.insert(id, Value::Object(entry));
        }
    }
    found.into_values().collect()
}

/// The four keys the dashboard needs. A Spore manifest is not arbitrary TOML
/// here; keeping the reader small avoids a second TOML dialect in the frontdoor.
fn parse_spore(text: &str) -> Map<String, Value> {
    let mut section = String::new();
    let mut out = Map::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = line.trim_matches(['[', ']']).to_owned();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim().trim_matches('"');
        if section == "spore" && matches!(key, "id" | "name" | "title" | "summary") {
            out.insert(key.to_owned(), Value::String(value.to_owned()));
        }
    }
    out
}

// ---------------------------------------------------------------- settings

fn dashboard_settings_path() -> PathBuf {
    mycelium_home().join("dashboard.json")
}

fn read_settings() -> Value {
    let defaults = json!({"refresh_secs": 10, "theme": "auto"});
    let stored = std::fs::read_to_string(dashboard_settings_path())
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .unwrap_or_else(|| json!({}));
    let mut value = defaults;
    if let (Some(target), Some(source)) = (value.as_object_mut(), stored.as_object()) {
        for (key, item) in source {
            if matches!(key.as_str(), "refresh_secs" | "theme") {
                target.insert(key.clone(), item.clone());
            }
        }
    }
    let relays = std::fs::read_to_string(mycelium_home().join("iroh-sync.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| value.get("public_relays").and_then(Value::as_bool));
    if let Some(relays) = relays {
        value["public_relays"] = json!(relays);
    }
    value
}

async fn put_settings(state: &FrontdoorState, body: &Bytes) -> Response<Body> {
    let input = match parse_body(body) {
        Ok(value) => value,
        Err(error) => return api_error(StatusCode::BAD_REQUEST, &error),
    };
    let refresh = input
        .get("refresh_secs")
        .and_then(Value::as_u64)
        .unwrap_or(10)
        .clamp(3, 300);
    let theme = input
        .get("theme")
        .and_then(Value::as_str)
        .filter(|theme| matches!(*theme, "auto" | "light" | "dark"))
        .unwrap_or("auto");

    let mut saved = json!({"refresh_secs": refresh, "theme": theme});
    if let Some(relays) = input.get("public_relays").and_then(Value::as_bool) {
        let bin = binary(state);
        let args = vec![
            "sync".to_owned(),
            "relays".to_owned(),
            if relays { "enable" } else { "disable" }.to_owned(),
            "--write".to_owned(),
        ];
        if let Err(error) = call_write(&bin, &args).await {
            return json_response(json!({"ok": false, "error": error}));
        }
        saved["public_relays"] = json!(relays);
    }

    let path = dashboard_settings_path();
    if let Some(parent) = path.parent() {
        if let Err(error) = std::fs::create_dir_all(parent) {
            return api_error(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string());
        }
    }
    let bytes = serde_json::to_vec_pretty(&saved).unwrap_or_default();
    if let Err(error) = write_private(&path, &bytes) {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, &error);
    }
    json_response(json!({"ok": true, "settings": read_settings()}))
}

// ---------------------------------------------------------------- routes

fn routes_path(state: &FrontdoorState) -> PathBuf {
    state
        .config_path
        .clone()
        .unwrap_or_else(|| mycelium_home().join("frontdoor/routes.json"))
}

fn routes(state: &FrontdoorState) -> Value {
    let path = routes_path(state);
    match std::fs::read_to_string(&path) {
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Ok(value) => json!({"ok": true, "path": path.display().to_string(), "routes": value}),
            Err(error) => {
                json!({"ok": false, "error": error.to_string(), "path": path.display().to_string()})
            }
        },
        Err(error) => {
            json!({"ok": false, "error": error.to_string(), "path": path.display().to_string()})
        }
    }
}

async fn put_routes(state: &FrontdoorState, body: &Bytes) -> Response<Body> {
    let value: Value = match parse_body(body) {
        Ok(value) => value,
        Err(error) => return api_error(StatusCode::BAD_REQUEST, &error),
    };
    // Validation is the same deserializer the daemon uses on reload, so a save
    // that succeeds here is one the next reload will accept.
    let mut config: crate::config::FrontdoorConfig = match serde_json::from_value(value.clone()) {
        Ok(config) => config,
        Err(error) => {
            return api_error(StatusCode::BAD_REQUEST, &format!("invalid routes: {error}"))
        }
    };
    if config.dashboard.hosts.is_empty() {
        config.dashboard.hosts = vec!["home.myc".into()];
    }
    if let Err(error) = config.validate() {
        return api_error(StatusCode::BAD_REQUEST, &error.to_string());
    }
    let path = routes_path(state);
    let bytes = serde_json::to_vec_pretty(&value).unwrap_or_default();
    if let Err(error) = write_private(&path, &bytes) {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, &error);
    }
    // Take the new host list immediately rather than waiting for the reload.
    if let Ok(mut hosts) = state.dashboard_hosts.write() {
        *hosts = normalized_hosts(&config.dashboard.hosts);
    }
    json_response(json!({"ok": true, "path": path.display().to_string()}))
}

pub(crate) fn normalized_hosts(hosts: &[String]) -> BTreeSet<String> {
    hosts
        .iter()
        .map(|host| crate::route::normalize_host(host))
        .filter(|host| !host.is_empty())
        .collect()
}

// ---------------------------------------------------------------- pulse

fn string_field(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

async fn pulse_add(state: &FrontdoorState, body: &Bytes) -> Response<Body> {
    let input = match parse_body(body) {
        Ok(value) => value,
        Err(error) => return api_error(StatusCode::BAD_REQUEST, &error),
    };
    let Some(schedule) = string_field(&input, "schedule") else {
        return api_error(StatusCode::BAD_REQUEST, "schedule is required");
    };
    let Some(command) = string_field(&input, "command") else {
        return api_error(StatusCode::BAD_REQUEST, "command is required");
    };
    let mut args = vec![
        "pulse".to_owned(),
        "add".to_owned(),
        "--schedule".to_owned(),
        schedule,
        "--command".to_owned(),
        command,
    ];
    if let Some(id) = string_field(&input, "id") {
        args.push("--id".into());
        args.push(id);
    }
    let disabled = input
        .get("disabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if disabled {
        args.push("--disabled".into());
    }
    if let Some(timeout) = input.get("timeout_secs").and_then(Value::as_u64) {
        args.push("--timeout".into());
        args.push(timeout.to_string());
    }
    args.push("--write".into());
    match call_write(&binary(state), &args).await {
        Ok(value) => json_response(value),
        Err(error) => json_response(json!({"ok": false, "error": error})),
    }
}

async fn pulse_toggle(state: &FrontdoorState, body: &Bytes) -> Response<Body> {
    let input = match parse_body(body) {
        Ok(value) => value,
        Err(error) => return api_error(StatusCode::BAD_REQUEST, &error),
    };
    let Some(id) = string_field(&input, "id") else {
        return api_error(StatusCode::BAD_REQUEST, "id is required");
    };
    let args = vec![
        "pulse".to_owned(),
        "toggle".to_owned(),
        id,
        "--write".to_owned(),
    ];
    match call_write(&binary(state), &args).await {
        Ok(value) => json_response(value),
        Err(error) => json_response(json!({"ok": false, "error": error})),
    }
}

async fn pulse_remove(state: &FrontdoorState, body: &Bytes) -> Response<Body> {
    let input = match parse_body(body) {
        Ok(value) => value,
        Err(error) => return api_error(StatusCode::BAD_REQUEST, &error),
    };
    let Some(id) = string_field(&input, "id") else {
        return api_error(StatusCode::BAD_REQUEST, "id is required");
    };
    let args = vec![
        "pulse".to_owned(),
        "remove".to_owned(),
        id,
        "--write".to_owned(),
    ];
    match call_write(&binary(state), &args).await {
        Ok(value) => json_response(value),
        Err(error) => json_response(json!({"ok": false, "error": error})),
    }
}

// ---------------------------------------------------------------- actions

async fn action(state: &FrontdoorState, body: &Bytes) -> Response<Body> {
    let input = match parse_body(body) {
        Ok(value) => value,
        Err(error) => return api_error(StatusCode::BAD_REQUEST, &error),
    };
    let Some(action) = string_field(&input, "action") else {
        return api_error(StatusCode::BAD_REQUEST, "action is required");
    };
    let args: Vec<String> = match action.as_str() {
        "daemon-start" => vec!["daemon".into(), "start".into()],
        "daemon-stop" => vec!["daemon".into(), "stop".into()],
        "tunnel-restart" => vec!["iroh-tun".into(), "restart".into(), "--write".into()],
        "relays-enable" => vec![
            "sync".into(),
            "relays".into(),
            "enable".into(),
            "--write".into(),
        ],
        "relays-disable" => vec![
            "sync".into(),
            "relays".into(),
            "disable".into(),
            "--write".into(),
        ],
        other => {
            return api_error(
                StatusCode::BAD_REQUEST,
                &format!("unknown action `{other}`"),
            )
        }
    };
    match call_write(&binary(state), &args).await {
        Ok(value) => json_response(value),
        Err(error) => json_response(json!({"ok": false, "error": error})),
    }
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, bytes).map_err(|error| error.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&temporary, path).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spore_manifest_is_read_without_a_toml_dependency() {
        let manifest = r#"
[spore]
id = "dev.fungos.gallery"
name = "gallery"
title = "Gallery"
summary = "A grid of images"

[grants]
"soil:canvas" = "draw"
"#;
        let parsed = parse_spore(manifest);
        assert_eq!(parsed["id"], json!("dev.fungos.gallery"));
        assert_eq!(parsed["title"], json!("Gallery"));
    }

    #[test]
    fn the_spore_reader_ignores_unknown_sections() {
        let parsed = parse_spore("[http]\nhosts = [\"gallery.myc\"]\n[spore]\nid = \"x\"\n");
        assert_eq!(parsed["id"], json!("x"));
        assert!(parsed.get("hosts").is_none());
    }
}
