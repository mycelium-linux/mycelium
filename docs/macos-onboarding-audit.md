# macOS onboarding audit

Status: **audit, 2026-11-25.** Read with `macos-founder-onboarding.md`. No
production routing was changed and no identities were granted while producing
this.

## Support matrix (verified against published artifacts)

| Piece | arm64 macOS | x86_64 macOS | Evidence |
|---|---|---|---|
| mycelium (`v0.1.3`, GitHub) | ✅ | ❌ absent | release assets: only `aarch64-apple-darwin` |
| mycelium (signed mesh manifests) | ✅ `0.2.26` | ❌ absent | `mycelium releases list --json`; targets are `aarch64-apple-darwin`, `aarch64/x86_64-unknown-linux-*` |
| loam (`v0.1.0`) | ✅ | ✅ | release assets include both `*-apple-darwin` |
| thyra / palisade | — | — | no published artifacts at all (built locally) |

**Intel Macs are unsupported for Mycelium**, not merely untested. If a cofounder
has one, either add `x86_64-apple-darwin` to the release train or say no
clearly.

## Download and verification

**Updated 2026-11-25:** a signed release now exists on the GitHub train.

```text
v0.1.4 assets             archive + SHA256SUMS + release-manifests.json
                          + release-metadata.json + release-set.json
signer                    14f549457cc7e979037abc8cc7812d4cae92f9b3d104fde4f6e17b239ebeb0f8
manifest verification     `mycelium releases verify … --trusted-key 14f5…` → verified
binary binding            archive's `mycelium` sha256 == manifest artifact_digest
installer                 checks checksum, then requires the binary to equal the
                          signed manifest's digest and the signer to be pinned
install.sh default base   https://releases.fungos.dev/mycelium   → still down (000)
bootstrap URL             https://fungos.dev/install.sh          → still down (000)
```

What is fixed: the founder path uses the repo installer from `raw.githubusercontent.com`
with `--release v0.1.4 --release-key 14f5…`, so the download is bound to the
signed manifest and the pinned signer. What is not: no shell Ed25519 verifier
runs during bootstrap (stock macOS LibreSSL cannot verify Ed25519), so the
manifest **signature** is checked out of band or by later updates. Relocating
the `fungos.dev` bootstrap URL and `releases.fungos.dev` base remains open.

Verified end to end in a clean `HOME`: installer exit 0, digest matched the
manifest, binaries installed, `release-source.json` pinned to channel `stable`
and key `14f5…`; `mycelium doctor` runs.

## Signing and rollback

- Signed mesh releases are signed by `962c5dc8…`; that key's private half is on
  no reachable host (see `docs/key-custody.md`). The fleet now also trusts
  `14f54945…`, which is the key future releases should use.
- `mycelium update apply` verifies signature and digest before activation, but
  **keeps no last-known-good binary**, so `mycelium update rollback` does not
  exist (recorded in `docs/releases.md`).
- Managed installs *do* have receipts, snapshots and rollback (`mycelium managed
  rollback`), but neither thyra nor palisade has a managed package yet.

## Identity and SSO

- `https://sso.fpl.dev` → 200, `/vouch` → 401 without a session, and the palisade
  `[server.sso]` vhost points at `192.168.1.40:4000` (fpl-auth container, up 4
  weeks, state on a host mount).
- Magic-link mail is **not configured**: `SmtpMailer::new` builds
  `AsyncSmtpTransport::builder_dangerous` with no credentials while the compose
  file sets `smtp-relay.gmail.com:465`. An authenticated relay (or an
  IP-authorised relay) is an administrator prerequisite.
- Accounts are `@fpl.dev` addresses; creating mailboxes and minting enrollment
  claims are identity-granting actions and were **not** performed here.

## Device enrollment

- Claims come from `mycelium invite create --kind peer --name … --write` on an
  already-enrolled device, redeemed with `mycelium setup --claim …`.
- That command needs fleet authority; it is an owner action. A founder cannot
  self-enrol, by design.
- Enrollment itself was not executed (would have created identities).

## iroh-tun

Working on an installed host: `mycelium iroh-tun status` reports `utun220`,
`owner external privileged service (root)`, a self route and `0 peer prefixes
learned`. The root child is a launchd service installed by the installer.

Gaps for a founder:

1. **The tarball path installs no root child.** Copying `iroh-tun` next to
   `mycelium` gives a binary that cannot create a `utun`. Only
   `mycelium setup --system-service` installs the LaunchDaemon, and that path is
   blocked by the dead release host.
2. **No independent install/verify command.** `iroh-tun status` observes; there
   is no `mycelium iroh-tun install --write` to install or repair just the root
   child, so a partial setup is hard to fix without re-running full `setup`.
3. **No peer-prefix test on a two-node overlay.** The single-node status is not
   evidence that two founders' Macs learn each other's prefixes; that needs a
   second clean machine (or a loopback pair) and a check that `kernel_routes`
   grows and survives a restart.

Smallest useful implementation, separate from commercial work:

- `mycelium iroh-tun install --write` / `uninstall --write`: write the same
  root-owned unit the installer writes, verify it with the existing `status`
  contract, and be idempotent. One file, one test that the unit is written with
  the right paths and that `status` reports `owner external`.
- A two-node integration test: start two temp daemons with `iroh-tun` enabled,
  assert each learns the other's prefix into `kernel_routes` (skip on
  non-privileged CI, run it on a Linux runner with `CAP_NET_ADMIN`).

## Coordination (owners and asks)

| Area | Owner | Ask |
|---|---|---|
| identity / SSO | `fpl-auth` owners | configure authenticated SMTP; confirm which `@fpl.dev` addresses exist; decide who may mint peer claims |
| ingress / DNS / tunnels | palisade + mycelium ingress owners | do not add per-founder DNS or tunnel entries; confirm principals are issued by enrollment, not by ingress |
| Spores | `spores` repo owners | confirm which Spores a founder should have on day one and that each has a signed artifact |
| release train | mycelium/loam release owners | restore a signed download host, publish signed manifests beside GitHub assets, add `x86_64-apple-darwin` only if Intel is wanted |

## Exact blockers

1. `releases.fungos.dev` and `fungos.dev/install.sh` are down; the founder path
   therefore uses the raw GitHub installer URL. **(partially fixed)**
2. Signed manifests are now published beside the v0.1.4 artifacts and the
   installer binds the binary to them; a shell signature verifier during
   bootstrap is still missing. **(partially fixed)**
3. No `x86_64-apple-darwin` Mycelium artifact → Intel unsupported.
4. Signed releases are signed by a key whose private half is missing; new
   releases must use `14f5…` and the fleet must stay dual-trust during rotation.
5. No `mycelium update rollback` (no last-known-good binary kept).
6. fpl-auth SMTP has no credentials → magic-link sign-in cannot deliver mail.
7. `iroh-tun` root child only arrived via `setup --system-service`. Now
   `mycelium iroh-tun install|repair|uninstall` manages it directly; the live
   privileged path was **not executed** because sudo is interactive in this
   environment (dry-run and unit tests pass). **(command implemented, live test
   blocked)**
8. Enrollment claims require an authorised human; none were minted here.

## What was not changed

No DNS records, tunnel ingress, palisade config, identity grants, or MCP
ingress were modified for this task.
