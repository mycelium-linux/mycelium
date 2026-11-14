//! Client for a running `geoded` broker.
//!
//! `geoded` owns the encrypted store and its key. A process that needs a secret
//! leases one over a `0600` Unix socket instead of linking a backend or reading
//! the store file. This module is the single implementation of that wire
//! protocol: one newline-terminated JSON request per connection, one JSON reply.
//!
//! ```text
//! -> {"op":"lease","reference":"geode://mail/work/imap","purpose":"mail.connect","ttl_ms":5000}
//! <- {"ok":true,"secret_b64":"...","expires_at":1793833200}
//! <- {"ok":false,"error":"purpose `mail.submit` is not allowed for geode://mail/work/imap"}
//! ```
//!
//! The client adds no policy of its own. Purpose allowlists, credential expiry,
//! audit, and redaction stay in the store behind the socket, so a caller cannot
//! widen them by talking to the broker directly.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{Deserialize, Serialize};

use crate::{GeodeError, GeodeRef, Purpose, SecretBytes};

/// Socket path used when `GEODE_SOCKET` is unset, relative to `$HOME`.
pub const DEFAULT_SOCKET: &str = ".geode/geode.sock";

/// A secret is leased for one operation, not for the life of a process.
pub const DEFAULT_LEASE: Duration = Duration::from_secs(5);

/// A readiness check must not hang a driver that is about to fail anyway.
const IO_TIMEOUT: Duration = Duration::from_secs(5);

/// One reply is a small JSON object; anything larger did not come from `geoded`.
const MAX_RESPONSE_BYTES: u64 = 256 * 1024;

#[derive(Serialize)]
struct LeaseRequest<'a> {
    op: &'static str,
    reference: &'a str,
    purpose: &'a str,
    ttl_ms: u64,
}

#[derive(Deserialize)]
struct LeaseResponse {
    ok: bool,
    #[serde(default)]
    secret_b64: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

/// A client for the owner-local `geoded` socket.
///
/// Construct it explicitly: there is no implicit backend selection, matching the
/// rest of geode. [`BrokerClient::from_env`] is the convenience form that reads
/// `GEODE_SOCKET` and falls back to `$HOME/.geode/geode.sock`.
#[derive(Clone, Debug)]
pub struct BrokerClient {
    socket: PathBuf,
}

impl BrokerClient {
    pub fn new(socket: impl Into<PathBuf>) -> Self {
        Self {
            socket: socket.into(),
        }
    }

    /// `GEODE_SOCKET` when it is set and non-empty, otherwise
    /// `$HOME/.geode/geode.sock`.
    pub fn from_env() -> Result<Self, GeodeError> {
        if let Some(path) = std::env::var_os("GEODE_SOCKET") {
            if !path.is_empty() {
                return Ok(Self::new(path));
            }
        }
        let home = std::env::var_os("HOME").ok_or_else(|| {
            GeodeError::Broker("GEODE_SOCKET or HOME is required to locate the geode broker".into())
        })?;
        Ok(Self::new(PathBuf::from(home).join(DEFAULT_SOCKET)))
    }

    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// Lease `reference` for `purpose`.
    ///
    /// The broker enforces the entry's purpose allowlist, credential expiry, and
    /// the `1s ..= 1h` lease bounds, so a denial is reported exactly as the store
    /// worded it rather than retried or downgraded.
    pub fn lease(
        &self,
        reference: &GeodeRef,
        purpose: &Purpose,
        ttl: Duration,
    ) -> Result<SecretBytes, GeodeError> {
        let reference = reference.as_reference();
        let request = serde_json::to_vec(&LeaseRequest {
            op: "lease",
            reference: &reference,
            purpose: purpose.as_str(),
            ttl_ms: u64::try_from(ttl.as_millis()).unwrap_or(u64::MAX),
        })
        .map_err(|error| GeodeError::Broker(format!("encode lease request: {error}")))?;

        let body = exchange(&self.socket, &request)?;
        let response: LeaseResponse = serde_json::from_slice(&body)
            .map_err(|error| GeodeError::Broker(format!("decode broker reply: {error}")))?;
        if !response.ok {
            return Err(GeodeError::Broker(
                response
                    .error
                    .unwrap_or_else(|| "the broker denied the lease".into()),
            ));
        }
        let encoded = response
            .secret_b64
            .ok_or_else(|| GeodeError::Broker("the broker returned no secret".into()))?;
        let bytes = STANDARD
            .decode(encoded)
            .map_err(|error| GeodeError::Broker(format!("invalid secret encoding: {error}")))?;
        Ok(SecretBytes::new(bytes))
    }
}

#[cfg(unix)]
fn exchange(socket: &Path, request: &[u8]) -> Result<Vec<u8>, GeodeError> {
    use std::os::unix::net::UnixStream;

    let stream = UnixStream::connect(socket)
        .map_err(|error| GeodeError::Broker(format!("connect {}: {error}", socket.display())))?;
    stream
        .set_write_timeout(Some(IO_TIMEOUT))
        .map_err(|error| GeodeError::Broker(format!("set write timeout: {error}")))?;
    stream
        .set_read_timeout(Some(IO_TIMEOUT))
        .map_err(|error| GeodeError::Broker(format!("set read timeout: {error}")))?;

    let mut line = request.to_vec();
    line.push(b'\n');
    (&stream)
        .write_all(&line)
        .map_err(|error| GeodeError::Broker(format!("write request: {error}")))?;

    let mut body = Vec::new();
    stream
        .take(MAX_RESPONSE_BYTES)
        .read_to_end(&mut body)
        .map_err(|error| GeodeError::Broker(format!("read reply: {error}")))?;
    Ok(body)
}

#[cfg(not(unix))]
fn exchange(_socket: &Path, _request: &[u8]) -> Result<Vec<u8>, GeodeError> {
    Err(GeodeError::Broker(
        "the geode broker needs unix domain sockets".into(),
    ))
}
