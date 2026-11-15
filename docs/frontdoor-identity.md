# Frontdoor identity: terminate mTLS once, forward who called

Status: proposal. Applies to `mycelium-frontdoor`, and unblocks node
enrollment in Bifrost without teaching every service TLS.

## Today

`tls_config` in `crates/mycelium-frontdoor/src/server.rs` builds its listener
with `.with_no_client_auth()`. The caller is never authenticated: the front door
proves *its* identity to the client and learns nothing back. Every service behind
it therefore invents its own caller identity — Bifrost does it with a bearer
token, a per-node token, and a shared secret.

## What to change

Three things, none of them large:

1. **Optional client auth against the mesh CA.** `WebPkiClientVerifier` rooted at
   `pki/ca.pem`, built with `.allow_unauthenticated()`. A client that presents a
   certificate signed by the mesh CA is authenticated; one that presents nothing
   is the same anonymous caller it is today.
2. **Forward the verified leaf.** After the handshake, hash the leaf and attach
   it to the request as `x-mycelium-cert-sha256`.
3. **Strip it on the way in.** `skip_header` already exists; an inbound copy of
   the header is a client claiming to be a node, and must be dropped before the
   forwarder adds the real one.

Optional rather than required is the only workable shape on one port: phones and
agents call `ai.myc` with no certificate, and a TLS listener has one handshake
policy for every route on it. Whether an identity is *sufficient* is a per-route
decision the application makes — which is exactly what forwarding it enables.

## The contract

| Header | Value | Present when |
|---|---|---|
| `x-mycelium-cert-sha256` | hex SHA-256 of the client leaf certificate | the caller presented a cert the mesh CA signed |

One header, and it says what it is. A service behind the door (Bifrost for
`/yggdrasil/*`) treats its presence as "an enrolled node is calling", and its
absence as "an anonymous caller", and enforces per route.

The trust boundary is loopback: the front door is the only process that may
connect to the service's bind address, so a header cannot be forged by anyone
who is not already able to open that socket.

## The open question

The certificate digest is **not** the daemon's node id. They differ on the
Studio today: the digest of `pki/node.pem` with whitespace removed is
`79aeea5b0828cab6…`, while `mycelium gateway status` reports `local_node` as
`2fd5bf85bb21f71b…`.

So forwarding the certificate digest is honest — it identifies the certificate —
but it is a *second* identifier in a fleet that already has one. Two ways out:

- **Forward what the daemon already calls the node id.** Requires knowing how it
  is derived; if it is not a function of the certificate, the front door cannot
  compute it without asking the daemon, which is a socket call in a request path.
- **Forward the certificate digest and let the service bind to it,** with the
  daemon's id correlated later through `gateway status` / `topology`. Cheaper,
  and correct as long as nothing assumes the two are equal.

This proposal takes the second and names the header for the certificate, so
nobody can mistake it for the daemon id. Reconciling them is worth its own
change: one fleet, one node id.

## What it unblocks

Bifrost's `/yggdrasil/enroll` and `/yggdrasil/heartbeat` can authenticate a node
by its certificate instead of a bearer token, with no second TLS listener and no
shared secret between the gateway and the application. The enrollment token then
disappears for the reason it should: it was a second identity for a node that
already had one.

## Bounds

- The verifier's roots are the mesh CA, nothing ambient: no system trust store,
  no client-supplied CA.
- A revoked certificate stops authenticating when the CA stops signing it; the
  front door does not keep a second revocation list.
- No identity is forwarded on the plain-HTTP listener. If the front door is
  serving `http://`, there was no handshake and there is nothing to say.
