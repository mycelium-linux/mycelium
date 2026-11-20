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
use std::net::ToSocketAddrs;
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
        (Method::GET, "/api/profile") => {
            json_response(json!({"ok": true, "profile": read_profile()}))
        }
        (Method::POST, "/api/profile") => put_profile(&body).await,
        (Method::GET, "/api/profile/avatar") => serve_avatar(),
        (Method::GET, "/api/files") => {
            let root = query_value(&query, "root").unwrap_or_default();
            let path = query_value(&query, "path").unwrap_or_default();
            match files_list(&root, &path) {
                Ok(value) => json_response(json!({"ok": true, "listing": value})),
                Err(error) => json_response(json!({"ok": false, "error": error})),
            }
        }
        (Method::GET, "/api/files/raw") => {
            let root = query_value(&query, "root").unwrap_or_default();
            let path = query_value(&query, "path").unwrap_or_default();
            match files_raw(&root, &path) {
                Ok((bytes, kind)) => Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, kind)
                    .header(header::CACHE_CONTROL, "no-store")
                    .body(Body::from(bytes))
                    .unwrap_or_else(|_| {
                        api_error(StatusCode::INTERNAL_SERVER_ERROR, "invalid response")
                    }),
                Err(error) => api_error(StatusCode::NOT_FOUND, &error),
            }
        }
        (Method::POST, "/api/connectors/ai") => put_ai_key(&body).await,
        (Method::POST, "/api/keys/cloudflare") => put_cloudflare_token(&body).await,
        (Method::GET, "/api/spore/icon") => {
            let id = query_value(&query, "id").unwrap_or_default();
            spore_icon(&id)
        }
        (Method::POST, "/api/reveal") => reveal(&body).await,
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
        "profile": read_profile(),
        "keymap": keymap(),
        "connectors": connectors(state),
        "spores": scan_spores(),
        "frontdoor": {
            "routes": table.total_routes(),
            "derived_routes": table.derived_routes(),
            "auth_required": state.auth_token.is_some(),
            "tls": state.forwarded_proto == "https",
            // Route name -> its first public host. The Unibus page uses this to
            // link a producer to the console that owns it, without guessing.
            "links": table
                .routes()
                .iter()
                .filter_map(|route| {
                    route
                        .hosts
                        .first()
                        .map(|host| (route.name.clone(), format!("https://{host}/")))
                })
                .collect::<std::collections::BTreeMap<_, _>>(),
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
                "uptime_seconds": peer["health"]["uptime_seconds"].clone(),
                "logical_cpus": peer["health"]["logical_cpus"].clone(),
                "memory_total_bytes": peer["health"]["memory_total_bytes"].clone(),
                "memory_available_bytes": peer["health"]["memory_available_bytes"].clone(),
                "swap_total_bytes": peer["health"]["swap_total_bytes"].clone(),
                "swap_free_bytes": peer["health"]["swap_free_bytes"].clone(),
                "filesystems": peer["health"]["filesystems"].clone(),
                "established_ssh_sessions": peer["health"]["established_ssh_sessions"].clone(),
                "power": peer["health"]["power"].clone(),
                "link": peer["health"]["link"].clone(),
                "pressure": json!({
                    "cpu": peer["health"]["platform_metrics"]["pressure_cpu"],
                    "memory": peer["health"]["platform_metrics"]["pressure_memory"],
                    "io": peer["health"]["platform_metrics"]["pressure_io"],
                }),
                "network_rx_bytes": peer["health"]["platform_metrics"]["network_rx_bytes"],
                "network_tx_bytes": peer["health"]["platform_metrics"]["network_tx_bytes"],
                "process_leaders": Value::Array(
                    peer["health"]["process_leaders"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .take(5)
                        .cloned()
                        .collect()
                ),
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
    let Ok(addresses) = (host, port).to_socket_addrs() else {
        return false;
    };
    addresses
        .into_iter()
        .any(|address| std::net::TcpStream::connect_timeout(&address, timeout).is_ok())
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
    // Only timestamped lines are bus activity. A startup failure prints a
    // multi-line `Error:`/`Caused by:` block with no timestamp; showing that in
    // the feed as three separate items is noise, not history.
    let mut lines: Vec<String> = text
        .lines()
        .map(strip_ansi)
        .filter(|line| line.chars().next().is_some_and(|ch| ch.is_ascii_digit()))
        .map(|line| line.chars().take(360).collect())
        .collect();
    let drop = lines.len().saturating_sub(count);
    lines.drain(..drop);
    lines
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
fn spore_roots() -> [PathBuf; 2] {
    let home = home_dir();
    [
        home.join("Library/Application Support/fungOS/spores"),
        home.join(".local/share/fungos/spores"),
    ]
}

/// Every installed Spore, not only the desktop surfaces. The dashboard shows
/// these in their own section; Files is about content, not packages.
fn scan_spores() -> Vec<Value> {
    let mut found: std::collections::BTreeMap<String, Value> = std::collections::BTreeMap::new();
    for root in spore_roots() {
        let Ok(children) = std::fs::read_dir(&root) else {
            continue;
        };
        for child in children.flatten() {
            let manifest = child.path().join("spore.toml");
            let Ok(text) = std::fs::read_to_string(&manifest) else {
                continue;
            };
            let mut entry = parse_spore(&text);
            let Some(id) = entry.get("id").and_then(Value::as_str).map(str::to_owned) else {
                continue;
            };
            let hosts = parse_spore_hosts(&text);
            entry.insert(
                "path".into(),
                Value::String(child.path().display().to_string()),
            );
            entry.insert(
                "open_url".into(),
                match hosts.first() {
                    Some(host) => Value::String(format!("https://{host}/")),
                    None => Value::Null,
                },
            );
            entry.insert("hosts".into(), json!(hosts));
            entry.insert(
                "desktop".into(),
                json!(text.contains("soil:canvas") || text.contains("soil:present")),
            );
            entry.insert("grants".into(), json!(count_grants(&text)));
            found.insert(id, Value::Object(entry));
        }
    }
    let mut rows: Vec<Value> = found.into_values().collect();
    rows.sort_by(|left, right| {
        let key = |value: &Value| {
            value["title"]
                .as_str()
                .or_else(|| value["name"].as_str())
                .unwrap_or_default()
                .to_lowercase()
        };
        key(left).cmp(&key(right))
    });
    rows
}

fn desktops() -> Vec<Value> {
    scan_spores()
        .into_iter()
        .filter(|spore| spore["desktop"].as_bool() == Some(true))
        .collect()
}

fn count_grants(text: &str) -> usize {
    let mut section = String::new();
    let mut count = 0;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') && line.ends_with(']') {
            section = line.trim_matches(['[', ']']).to_owned();
            continue;
        }
        if section == "grants" && line.contains('=') && !line.starts_with('#') {
            count += 1;
        }
    }
    count
}

/// The dashboard only needs the first host a Spore declares. A full TOML
/// dependency is a larger program than this rule.
fn parse_spore_hosts(text: &str) -> Vec<String> {
    let mut section = String::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') && line.ends_with(']') {
            section = line.trim_matches(['[', ']']).to_owned();
            continue;
        }
        if section != "http" {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() != "hosts" {
            continue;
        }
        return value
            .trim()
            .trim_start_matches('[')
            .trim_end_matches(']')
            .split(',')
            .map(|host| host.trim().trim_matches('"').to_owned())
            .filter(|host| !host.is_empty())
            .collect();
    }
    Vec::new()
}

/// The keys the dashboard needs. A Spore manifest is not arbitrary TOML here;
/// keeping the reader small avoids a second TOML dialect in the frontdoor.
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
        if section == "spore" && matches!(key, "id" | "name" | "title" | "summary" | "version") {
            out.insert(key.to_owned(), Value::String(value.to_owned()));
        }
        if section == "presentation" && matches!(key, "badge" | "color" | "mark") {
            out.insert(key.to_owned(), Value::String(value.to_owned()));
        }
    }
    out
}

fn spore_icon(id: &str) -> Response<Body> {
    let Some(spore) = scan_spores()
        .into_iter()
        .find(|spore| spore["id"].as_str() == Some(id))
    else {
        return api_error(StatusCode::NOT_FOUND, "no such spore");
    };
    let Some(mark) = spore["mark"].as_str().filter(|mark| !mark.is_empty()) else {
        return api_error(StatusCode::NOT_FOUND, "spore has no mark");
    };
    let Some(directory) = spore["path"].as_str() else {
        return api_error(StatusCode::NOT_FOUND, "spore has no path");
    };
    let Ok(base) = std::fs::canonicalize(directory) else {
        return api_error(StatusCode::NOT_FOUND, "spore path is not readable");
    };
    let Ok(target) = std::fs::canonicalize(base.join(mark)) else {
        return api_error(StatusCode::NOT_FOUND, "spore mark is missing");
    };
    if !target.starts_with(&base) {
        return api_error(StatusCode::FORBIDDEN, "spore mark escapes the spore");
    }
    let kind = content_type(&target);
    if !matches!(
        kind.split(';').next().unwrap_or(""),
        "image/svg+xml" | "image/png" | "image/jpeg" | "image/webp"
    ) {
        return api_error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "spore mark is not an image",
        );
    }
    let Ok(meta) = std::fs::metadata(&target) else {
        return api_error(StatusCode::NOT_FOUND, "spore mark is missing");
    };
    if meta.len() > 1024 * 1024 {
        return api_error(StatusCode::PAYLOAD_TOO_LARGE, "spore mark is too large");
    }
    let Ok(bytes) = std::fs::read(&target) else {
        return api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "could not read spore mark",
        );
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, kind)
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(bytes))
        .unwrap_or_else(|_| api_error(StatusCode::INTERNAL_SERVER_ERROR, "invalid response"))
}

fn revealable(path: &Path) -> bool {
    let Ok(canonical) = std::fs::canonicalize(path) else {
        return false;
    };
    let home = home_dir();
    let mut roots: Vec<PathBuf> = allowed_roots().to_vec();
    roots.extend(spore_roots());
    roots.push(mycelium_home());
    roots.push(home.clone());
    roots.iter().any(|root| {
        std::fs::canonicalize(root)
            .map(|root| canonical.starts_with(root))
            .unwrap_or(false)
    })
}

async fn reveal(body: &Bytes) -> Response<Body> {
    let input = match parse_body(body) {
        Ok(value) => value,
        Err(error) => return api_error(StatusCode::BAD_REQUEST, &error),
    };
    let Some(path) = input.get("path").and_then(Value::as_str) else {
        return api_error(StatusCode::BAD_REQUEST, "path is required");
    };
    let path = PathBuf::from(path);
    if !revealable(&path) {
        return api_error(
            StatusCode::FORBIDDEN,
            "path is not one this page may reveal",
        );
    }
    #[cfg(target_os = "macos")]
    {
        let status = if path.is_dir() {
            std::process::Command::new("open").arg(&path).status()
        } else {
            std::process::Command::new("open")
                .arg("-R")
                .arg(&path)
                .status()
        };
        match status {
            Ok(status) if status.success() => json_response(json!({"ok": true})),
            _ => api_error(StatusCode::INTERNAL_SERVER_ERROR, "Finder refused the path"),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let target = if path.is_dir() {
            path.clone()
        } else {
            path.parent().map(Path::to_path_buf).unwrap_or(path.clone())
        };
        match std::process::Command::new("xdg-open").arg(&target).status() {
            Ok(status) if status.success() => json_response(json!({"ok": true})),
            _ => api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "the desktop refused the path",
            ),
        }
    }
}

fn keymap() -> Vec<Value> {
    let home = home_dir();
    let config = config_home();
    let bifrost = std::fs::read_to_string(config.join("bifrost-local/bifrost.toml"))
        .map(|text| parse_bifrost(&text))
        .unwrap_or_else(|_| json!({"users": [], "backends": []}));
    let mut keys: Vec<Value> = Vec::new();

    for user in list_values(&bifrost["users"]) {
        let name = user["name"].as_str().unwrap_or("account");
        keys.push(json!({
            "id": format!("bifrost-{name}"),
            "name": format!("Bifrost · {name}"),
            "provider": "Bifrost",
            "badge": "BF",
            "color": "#8b9dff",
            "present": user["has_key"].as_bool().unwrap_or(false),
            "source": "local Bifrost account",
            "editable": true,
            "action": "ai",
        }));
    }

    let cloudflare = mycelium_home().join("managed/cloudflared-tunnel/token");
    keys.push(json!({
        "id": "cloudflare-tunnel",
        "name": "Cloudflare tunnel",
        "provider": "Cloudflare",
        "badge": "CF",
        "color": "#f0a35e",
        "present": cloudflare.is_file(),
        "source": "managed tunnel token",
        "path": cloudflare.display().to_string(),
        "editable": true,
        "action": "cloudflare",
    }));

    let loki = config.join("loki/loki.toml");
    keys.push(json!({
        "id": "loki-upstream",
        "name": "Loki upstream",
        "provider": "Loki",
        "badge": "LK",
        "color": "#6fbf9a",
        "present": std::env::var_os("LOKI_UPSTREAM_TOKEN").is_some() && loki.is_file(),
        "source": "LOKI_UPSTREAM_TOKEN + loki.toml",
        "path": loki.display().to_string(),
        "editable": false,
        "action": "",
    }));

    for (file, label) in [
        ("studio-mcp-provider.token", "MCP provider"),
        ("confluence.token", "Confluence webhook"),
    ] {
        let path = config.join("unibus").join(file);
        keys.push(json!({
            "id": file,
            "name": label,
            "provider": "Unibus",
            "badge": "UB",
            "color": "#c9a6ff",
            "present": path.is_file(),
            "source": "Unibus token file",
            "path": path.display().to_string(),
            "editable": false,
            "action": "",
        }));
    }
    keys
}

fn list_values(value: &Value) -> Vec<Value> {
    value.as_array().cloned().unwrap_or_default()
}

async fn put_cloudflare_token(body: &Bytes) -> Response<Body> {
    let input = match parse_body(body) {
        Ok(value) => value,
        Err(error) => return api_error(StatusCode::BAD_REQUEST, &error),
    };
    let Some(token) = input.get("token").and_then(Value::as_str).map(str::trim) else {
        return api_error(StatusCode::BAD_REQUEST, "token is required");
    };
    if token.len() < 20 || token.len() > 4096 || token.contains('\n') {
        return api_error(StatusCode::BAD_REQUEST, "token is invalid");
    }
    let path = mycelium_home().join("managed/cloudflared-tunnel/token");
    if let Err(error) = write_private(&path, format!("{token}\n").as_bytes()) {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, &error);
    }
    json_response(json!({"ok": true, "key": file_note(&path)}))
}

// ---------------------------------------------------------------- profile

fn posix_user() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "unknown".into())
}

fn profile_path() -> PathBuf {
    mycelium_home().join("dashboard/profile.json")
}

fn avatar_path() -> PathBuf {
    mycelium_home().join("dashboard/avatar")
}

fn read_profile() -> Value {
    let user = posix_user();
    let stored = std::fs::read_to_string(profile_path())
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .unwrap_or_else(|| json!({}));
    let name = stored
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.trim().is_empty())
        .unwrap_or(&user)
        .to_owned();
    let avatar_type = std::fs::read_to_string(avatar_path().with_extension("type"))
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    json!({
        "name": name,
        "posix_user": user,
        "has_avatar": avatar_path().is_file(),
        "avatar_url": if avatar_path().is_file() { "/api/profile/avatar" } else { "" },
        "avatar_type": avatar_type,
    })
}

async fn put_profile(body: &Bytes) -> Response<Body> {
    let input = match parse_body(body) {
        Ok(value) => value,
        Err(error) => return api_error(StatusCode::BAD_REQUEST, &error),
    };
    let mut stored = json!({});
    if let Some(name) = input.get("name").and_then(Value::as_str) {
        let name = name.trim();
        if name.len() > 80 || name.contains('\n') {
            return api_error(StatusCode::BAD_REQUEST, "name is invalid");
        }
        stored["name"] = json!(name);
    }
    let path = profile_path();
    if let Err(error) = write_private(
        &path,
        serde_json::to_vec_pretty(&stored)
            .unwrap_or_default()
            .as_slice(),
    ) {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, &error);
    }
    if let (Some(encoded), Some(kind)) = (
        input.get("avatar_base64").and_then(Value::as_str),
        input.get("avatar_type").and_then(Value::as_str),
    ) {
        if !matches!(
            kind,
            "image/png" | "image/jpeg" | "image/webp" | "image/gif"
        ) {
            return api_error(
                StatusCode::BAD_REQUEST,
                "avatar must be a png, jpeg, webp or gif",
            );
        }
        let Some(bytes) = base64_decode(encoded) else {
            return api_error(StatusCode::BAD_REQUEST, "avatar is not valid base64");
        };
        if bytes.len() > 20 * 1024 * 1024 {
            return api_error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "avatar is larger than 20 MiB",
            );
        }
        if let Err(error) = write_private(&avatar_path(), &bytes) {
            return api_error(StatusCode::INTERNAL_SERVER_ERROR, &error);
        }
        let _ = write_private(&avatar_path().with_extension("type"), kind.as_bytes());
    }
    json_response(json!({"ok": true, "profile": read_profile()}))
}

fn base64_decode(value: &str) -> Option<Vec<u8>> {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut lookup = [255u8; 256];
    for (index, byte) in TABLE.iter().enumerate() {
        lookup[*byte as usize] = index as u8;
    }
    let mut out = Vec::with_capacity(value.len() * 3 / 4);
    let mut buffer = 0u32;
    let mut bits = 0u32;
    for byte in value.bytes() {
        match byte {
            b'=' => break,
            b'\n' | b'\r' | b' ' | b'\t' => continue,
            _ => {}
        }
        let value = lookup[byte as usize];
        if value == 255 {
            return None;
        }
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    Some(out)
}

fn serve_avatar() -> Response<Body> {
    let Ok(bytes) = std::fs::read(avatar_path()) else {
        return api_error(StatusCode::NOT_FOUND, "no avatar");
    };
    let kind = std::fs::read_to_string(avatar_path().with_extension("type"))
        .unwrap_or_else(|_| "image/png".into())
        .trim()
        .to_owned();
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, kind)
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(bytes))
        .unwrap_or_else(|_| api_error(StatusCode::INTERNAL_SERVER_ERROR, "invalid response"))
}

// ---------------------------------------------------------------- connectors

fn file_note(path: &Path) -> Value {
    let meta = std::fs::metadata(path).ok();
    json!({
        "path": path.display().to_string(),
        "present": meta.is_some(),
        "bytes": meta.as_ref().map(|meta| meta.len()).unwrap_or(0),
        "updated_ms": meta
            .and_then(|meta| meta.modified().ok())
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or(0),
    })
}

fn running(needle: &str) -> bool {
    let Ok(output) = std::process::Command::new("ps")
        .args(["-Ao", "command"])
        .output()
    else {
        return false;
    };
    output.stdout.len() < 2 * 1024 * 1024
        && String::from_utf8_lossy(&output.stdout)
            .lines()
            .any(|line| line.contains(needle) && !line.contains("ps -Ao"))
}

fn split_http(url: &str) -> Option<(&str, u16)> {
    let (scheme, rest) = url.split_once("://").unwrap_or(("http", url));
    let rest = rest.split('/').next().unwrap_or(rest);
    if let Some((host, port)) = rest.rsplit_once(':') {
        return Some((host.trim_matches(['[', ']']), port.parse().ok()?));
    }
    let port = if scheme == "https" { 443 } else { 80 };
    Some((rest.trim_matches(['[', ']']), port))
}

fn push_bifrost_section(
    kind: &str,
    map: Map<String, Value>,
    users: &mut Vec<Value>,
    backends: &mut Vec<Value>,
) {
    if map.is_empty() {
        return;
    }
    match kind {
        "user" => users.push(Value::Object(map)),
        "backend" => backends.push(Value::Object(map)),
        _ => {}
    }
}

/// Read the same Bifrost config the local model router uses. The dashboard
/// shows accounts and named backends, not a loopback URL: the point of the page
/// is which provider an operator can use, not which port it happens to bind.
fn parse_bifrost(text: &str) -> Value {
    let mut users: Vec<Value> = Vec::new();
    let mut backends: Vec<Value> = Vec::new();
    let mut kind = String::new();
    let mut current = Map::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with("[[") {
            push_bifrost_section(
                &kind,
                std::mem::take(&mut current),
                &mut users,
                &mut backends,
            );
            kind = line.trim_matches(['[', ']']).to_owned();
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        let quoted = value.trim_matches('"');
        match (kind.as_str(), key) {
            ("user", "name") => {
                current.insert("name".into(), json!(quoted));
            }
            ("user", "key") => {
                current.insert("has_key".into(), json!(!quoted.is_empty()));
            }
            ("backend", "name") => {
                current.insert("name".into(), json!(quoted));
            }
            ("backend", "base_url") => {
                current.insert("base_url".into(), json!(quoted));
            }
            ("backend", "api_key_env") => {
                current.insert("api_key_env".into(), json!(quoted));
            }
            ("backend", "probe") => {
                current.insert("probe".into(), json!(value == "true"));
            }
            ("backend", "aliases") => {
                let pairs = value
                    .trim_matches(['{', '}'])
                    .split(',')
                    .filter(|pair| !pair.trim().is_empty())
                    .count();
                current.insert("aliases".into(), json!(pairs));
            }
            _ => {}
        }
    }
    push_bifrost_section(&kind, current, &mut users, &mut backends);

    for backend in backends.iter_mut() {
        let name = backend["name"].as_str().unwrap_or("").to_owned();
        let label = match name.as_str() {
            "local" => "This Mac",
            "agora" => "Agora",
            "loki" => "Loki",
            "fpl-cloud" => "FPL Cloud",
            other => other,
        };
        let base_url = backend["base_url"].as_str().map(str::to_owned);
        let env = backend["api_key_env"].as_str().map(str::to_owned);
        if let Some(fields) = backend.as_object_mut() {
            fields.insert("label".into(), json!(label));
            let reachable = base_url
                .and_then(|url| {
                    let (host, port) = split_http(&url)?;
                    Some(probe_tcp(
                        &format!("{host}:{port}"),
                        Duration::from_millis(350),
                    ))
                })
                .unwrap_or(false);
            fields.insert("reachable".into(), json!(reachable));
            if let Some(env) = env {
                fields.insert("env_present".into(), json!(std::env::var_os(env).is_some()));
            }
        }
    }
    json!({"users": users, "backends": backends})
}

fn connectors(state: &FrontdoorState) -> Value {
    let home = home_dir();
    let config = config_home();
    let table = state.table.read().unwrap();
    let link = |name: &str| {
        table
            .routes()
            .iter()
            .find(|route| route.name == name)
            .and_then(|route| route.hosts.first())
            .map(|host| format!("https://{host}/"))
    };
    let upstreams = |name: &str| {
        table
            .routes()
            .iter()
            .find(|route| route.name == name)
            .map(|route| route.upstreams.clone())
            .unwrap_or_default()
    };

    let ai_keys = [
        home.join(".fpl/bifrost-api-key"),
        config.join("fpl/bifrost-api-key"),
        home.join(".fpl/bifrost-local-key"),
    ]
    .map(|path| file_note(&path));

    let mcp_configs: Vec<Value> = std::fs::read_dir(config.join("unibus"))
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            name.starts_with("mcp").then(|| {
                json!({
                    "name": name,
                    "path": entry.path().display().to_string(),
                    "updated_ms": std::fs::metadata(entry.path())
                        .and_then(|meta| meta.modified())
                        .ok()
                        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|duration| duration.as_millis() as u64)
                        .unwrap_or(0),
                })
            })
        })
        .collect();

    let loki_config = config.join("loki/loki.toml");
    let cloudflared = running("cloudflared");

    json!({
        "ai": {
            "link": link("ai"),
            "upstreams": upstreams("ai"),
            "reachable": probe_tcp("127.0.0.1:4000", Duration::from_millis(350)),
            "keys": ai_keys,
            "config": file_note(&config.join("bifrost-local/bifrost.toml")),
            "bifrost": std::fs::read_to_string(config.join("bifrost-local/bifrost.toml"))
                .map(|text| parse_bifrost(&text))
                .unwrap_or_else(|_| json!({"users": [], "backends": []})),
        },
        "mcp": {
            "link": link("mcp"),
            "upstreams": upstreams("mcp"),
            "reachable": probe_tcp("127.0.0.1:8000", Duration::from_millis(350)),
            "configs": mcp_configs,
        },
        "logs": {
            "reachable": probe_tcp("127.0.0.1:4010", Duration::from_millis(350)),
            "listen": "127.0.0.1:4010",
            "config": file_note(&loki_config),
        },
        "cloudflare": {
            "running": cloudflared,
            "installed": std::path::Path::new("/Library/LaunchDaemons/dev.fpl.cloudflared.plist").is_file(),
            "token": file_note(&mycelium_home().join("managed/cloudflared-tunnel/token")),
            "command": "mycelium managed apply cloudflared-tunnel --allow-root --write",
        },
        "domain": {
            "hosts": table
                .routes()
                .iter()
                .flat_map(|route| route.hosts.iter().cloned())
                .filter(|host| host.ends_with(".myc"))
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>(),
        },
    })
}

async fn put_ai_key(body: &Bytes) -> Response<Body> {
    let input = match parse_body(body) {
        Ok(value) => value,
        Err(error) => return api_error(StatusCode::BAD_REQUEST, &error),
    };
    let Some(key) = input.get("api_key").and_then(Value::as_str).map(str::trim) else {
        return api_error(StatusCode::BAD_REQUEST, "api_key is required");
    };
    if key.is_empty() || key.len() > 512 || key.contains('\n') {
        return api_error(StatusCode::BAD_REQUEST, "api_key is invalid");
    }
    let path = home_dir().join(".fpl/bifrost-api-key");
    if let Err(error) = write_private(&path, format!("{key}\n").as_bytes()) {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, &error);
    }
    json_response(json!({"ok": true, "key": file_note(&path)}))
}

// ---------------------------------------------------------------- files

fn content_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_lowercase()
        .as_str()
    {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "md" | "txt" | "log" | "toml" | "json" | "yaml" | "yml" | "rs" | "py" | "js" | "ts"
        | "lua" | "css" | "html" | "sh" | "csv" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn is_previewable_text(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("")
            .to_lowercase()
            .as_str(),
        "md" | "txt"
            | "log"
            | "toml"
            | "json"
            | "yaml"
            | "yml"
            | "rs"
            | "py"
            | "js"
            | "ts"
            | "lua"
            | "css"
            | "html"
            | "sh"
            | "csv"
    )
}

fn resolve_under_root(root: &str, rel: &str) -> Result<(PathBuf, PathBuf), String> {
    let canonical_root = std::fs::canonicalize(root).map_err(|error| format!("root: {error}"))?;
    let allowed = allowed_roots();
    if !allowed.iter().any(|path| {
        std::fs::canonicalize(path)
            .map(|candidate| candidate == canonical_root)
            .unwrap_or(false)
    }) {
        return Err("root is not a known content root".into());
    }
    let rel = rel.trim_start_matches('/');
    if rel.split('/').any(|part| part == "..") {
        return Err("path escapes the root".into());
    }
    let target = canonical_root.join(rel);
    let canonical = std::fs::canonicalize(&target).map_err(|error| format!("path: {error}"))?;
    if !canonical.starts_with(&canonical_root) {
        return Err("path escapes the root".into());
    }
    Ok((canonical_root, canonical))
}

fn allowed_roots() -> [PathBuf; 3] {
    let home = home_dir();
    [
        home.join(".local/share/fungos/vfs"),
        home.join("fungOS"),
        home.join("fungos-node"),
    ]
}

fn files_list(root: &str, rel: &str) -> Result<Value, String> {
    let (base, target) = resolve_under_root(root, rel)?;
    if target.is_file() {
        let meta = std::fs::metadata(&target).map_err(|error| error.to_string())?;
        let mut preview = None;
        if is_previewable_text(&target) && meta.len() <= 512 * 1024 {
            let bytes = std::fs::read(&target).map_err(|error| error.to_string())?;
            preview =
                Some(String::from_utf8_lossy(&bytes[..bytes.len().min(16 * 1024)]).to_string());
        }
        return Ok(json!({
            "root": base.display().to_string(),
            "path": rel,
            "file": {
                "name": target.file_name().and_then(|name| name.to_str()).unwrap_or_default(),
                "size": meta.len(),
                "modified_ms": meta.modified().ok()
                    .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|duration| duration.as_millis() as u64)
                    .unwrap_or(0),
                "mime": content_type(&target),
                "preview": preview,
            },
        }));
    }
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(&target)
        .map_err(|error| error.to_string())?
        .flatten()
    {
        let metadata = entry.metadata().ok();
        entries.push(json!({
            "name": entry.file_name().to_string_lossy(),
            "dir": entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false),
            "size": metadata.as_ref().map(|meta| meta.len()).unwrap_or(0),
            "modified_ms": metadata
                .and_then(|meta| meta.modified().ok())
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|duration| duration.as_millis() as u64)
                .unwrap_or(0),
            "mime": if entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
                "inode/directory".to_string()
            } else {
                content_type(&entry.path()).to_string()
            },
        }));
    }
    entries.sort_by(|left, right| {
        right["dir"]
            .as_bool()
            .cmp(&left["dir"].as_bool())
            .then_with(|| {
                left["name"]
                    .as_str()
                    .unwrap_or("")
                    .to_lowercase()
                    .cmp(&right["name"].as_str().unwrap_or("").to_lowercase())
            })
    });
    Ok(json!({
        "root": base.display().to_string(),
        "path": rel,
        "entries": entries,
    }))
}

fn files_raw(root: &str, rel: &str) -> Result<(Vec<u8>, &'static str), String> {
    let (_, target) = resolve_under_root(root, rel)?;
    let meta = std::fs::metadata(&target).map_err(|error| error.to_string())?;
    if !target.is_file() {
        return Err("not a file".into());
    }
    if meta.len() > 8 * 1024 * 1024 {
        return Err("file is larger than 8 MiB".into());
    }
    Ok((
        std::fs::read(&target).map_err(|error| error.to_string())?,
        content_type(&target),
    ))
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
