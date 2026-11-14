#![cfg(unix)]

//! The broker wire protocol, exercised against a socket that answers the way
//! `geoded` does. A real `geoded` is the same one JSON line in, one JSON object
//! out, so these tests pin the contract both sides depend on.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use geode::{BrokerClient, GeodeError, GeodeRef, Purpose};

const LEASE_TTL: Duration = Duration::from_secs(5);

/// Answer exactly one lease request on `socket`, then stop.
///
/// Returns the request line the client sent, so a test can assert the wire
/// shape rather than only the decoded secret.
fn serve_once(socket: &Path, reply: &'static str) -> JoinHandle<String> {
    let listener = UnixListener::bind(socket).expect("bind test socket");
    thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accept broker client");
        let mut reader = BufReader::new(stream.try_clone().expect("clone socket"));
        let mut request = String::new();
        reader.read_line(&mut request).expect("read request");
        let mut stream = stream;
        stream.write_all(reply.as_bytes()).expect("write reply");
        stream.write_all(b"\n").expect("terminate reply");
        stream.flush().expect("flush reply");
        request
    })
}

fn reference() -> GeodeRef {
    GeodeRef::parse("geode://mail/work/imap").unwrap()
}

#[test]
fn leases_a_secret_over_the_socket() {
    let directory = tempfile::tempdir().unwrap();
    let socket = directory.path().join("geode.sock");
    let server = serve_once(&socket, r#"{"ok":true,"secret_b64":"aHVudGVyMg=="}"#);

    let client = BrokerClient::new(&socket);
    let purpose = Purpose::mail_connect();
    let secret = client
        .lease(&reference(), &purpose, LEASE_TTL)
        .expect("lease succeeds");
    assert_eq!(secret.expose(), b"hunter2");

    let request = server.join().unwrap();
    let request: serde_json::Value = serde_json::from_str(&request).expect("request is JSON");
    assert_eq!(request["op"], "lease");
    assert_eq!(request["reference"], "geode://mail/work/imap");
    assert_eq!(request["purpose"], "mail.connect");
    assert_eq!(request["ttl_ms"], 5_000);
}

#[test]
fn a_denial_keeps_the_broker_wording() {
    let directory = tempfile::tempdir().unwrap();
    let socket = directory.path().join("geode.sock");
    let denial =
        r#"{"ok":false,"error":"purpose `mail.submit` is not allowed for geode://mail/work/imap"}"#;
    let server = serve_once(&socket, denial);

    let client = BrokerClient::new(&socket);
    let error = client
        .lease(&reference(), &Purpose::mail_submit(), LEASE_TTL)
        .expect_err("denial is an error");
    assert!(
        matches!(&error, GeodeError::Broker(message) if message.contains("mail.submit")),
        "unexpected error: {error}"
    );
    server.join().unwrap();
}

#[test]
fn a_missing_broker_fails_closed() {
    let directory = tempfile::tempdir().unwrap();
    let socket = directory.path().join("absent.sock");

    let client = BrokerClient::new(&socket);
    let error = client
        .lease(&reference(), &Purpose::mail_connect(), LEASE_TTL)
        .expect_err("a missing socket is an error");
    assert!(
        matches!(&error, GeodeError::Broker(message) if message.starts_with("connect ")),
        "unexpected error: {error}"
    );
}

#[test]
fn a_reply_without_a_secret_is_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let socket = directory.path().join("geode.sock");
    let server = serve_once(&socket, r#"{"ok":true}"#);

    let client = BrokerClient::new(&socket);
    let error = client
        .lease(&reference(), &Purpose::mail_connect(), LEASE_TTL)
        .expect_err("a reply without a secret is an error");
    assert!(
        matches!(&error, GeodeError::Broker(message) if message.contains("no secret")),
        "unexpected error: {error}"
    );
    server.join().unwrap();
}
