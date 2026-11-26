# macOS founder onboarding

Status: **draft, 2026-11-25.** The Apple Silicon path is partially verified; the
blockers below mean a cofounder cannot complete this end-to-end today without an
administrator step. Do not hand this to a founder as "just run it" until the
blockers in `macos-onboarding-audit.md` are cleared.

## Who this is for

Two FPL cofounders on macOS. **Apple Silicon only** for now: there is no signed
`x86_64-apple-darwin` Mycelium release (see the audit), so an Intel Mac is
explicitly **unsupported** — not "untested", unsupported.

## What you get

| Piece | What it is |
|---|---|
| `mycelium` | fleet identity, mesh transport (iroh), service catalogue, DNS |
| `loam` | the Soil runtime that runs Spores |
| `mycelium-frontdoor` | optional local HTTPS frontdoor for `.myc` names |
| `iroh-tun` | optional L3 overlay; needs a root service |

You do not need a Cloudflare token, a tunnel, a certificate, or an SSH key.
Access to fleet services comes from your enrolled node identity and from
`sso.fpl.dev`, not from secrets on your laptop.

## Before you start (administrator, not founder)

1. **An `@fpl.dev` mailbox for each founder.** Sign-in is a magic link to that
   address; the fleet does not create mailboxes.
2. **An enrollment claim per machine**, minted on an already-enrolled device by
   someone with fleet authority:
   ```sh
   mycelium invite create --kind peer --name FOUNDER-MAC --site SITE --write
   ```
   Treat the claim like a temporary password: one use, short TTL, delivered out
   of band. Do not put it in a ticket, a repo, or a chat log.
3. **Confirm the macOS release host is serving artifacts.** Today
   `releases.fungos.dev` and `https://fungos.dev/install.sh` are down; the
   install path below uses the GitHub release until that is fixed.

## 1. Install Mycelium (Apple Silicon)

The bootstrap is the repo installer pointed at the signed release. It verifies
the archive checksum **and** that the installed binary is the one the signed
`release-manifests.json` names, and refuses a signer that is not the pinned key.
It does not run a signature verifier — the release manifest signature is
verified out of band (or by every subsequent update):

```sh
curl --proto '=https' --tlsv1.2 -fsSL \
  https://raw.githubusercontent.com/mycelium-linux/mycelium/master/install.sh \
  | sh -s -- --release v0.1.4 \
      --release-key 14f549457cc7e979037abc8cc7812d4cae92f9b3d104fde4f6e17b239ebeb0f8
```

Pin the full signer, not a prefix. To verify the release by hand:

```sh
gh release download v0.1.4 --repo mycelium-linux/mycelium \
  --pattern 'mycelium-v0.1.4-aarch64-apple-darwin.tar.gz' \
  --pattern release-manifests.json
tar -xzf mycelium-v0.1.4-aarch64-apple-darwin.tar.gz
shasum -a 256 mycelium          # must equal artifact_digest in release-manifests.json
mycelium releases verify --manifest release-manifests.json \
  --trusted-key 14f549457cc7e979037abc8cc7812d4cae92f9b3d104fde4f6e17b239ebeb0f8
```

Verified 2026-11-25 on Apple Silicon: installer exit 0 from a clean `HOME`,
binaries installed, `release-source.json` written with channel `stable` and the
pinned key, `mycelium doctor` runs. Apple Gatekeeper will still warn because the
binaries are not notarized; the installer's `xattr` handling is a separate
step (see "Notarization" below).

### Recovery

If the install fails, the previous binaries remain in
`~/.local/bin` (`mycelium` is replaced atomically). To fall back to the manually
verified tarball:

```sh
gh release download v0.1.4 --repo mycelium-linux/mycelium \
  --pattern 'mycelium-v0.1.4-aarch64-apple-darwin.tar.gz' --pattern SHA256SUMS
shasum -a 256 -c SHA256SUMS 2>&1 | grep aarch64-apple-darwin
tar -xzf mycelium-v0.1.4-aarch64-apple-darwin.tar.gz
install -m 755 mycelium iroh-tun "$HOME/.local/bin/"
```

## 2. Join the fleet

Use the claim your administrator minted:

```sh
mycelium setup --claim 'MYC1-…' --system-service
```

`--system-service` installs the daemon as a launchd service and (per
`docs/install.md`) the root-owned `iroh-tun` child. Without it you get a
user-level daemon and no overlay.

Verify:

```sh
mycelium doctor
mycelium peers          # other nodes should appear within a minute
mycelium iroh-tun status
```

`iroh-tun status` showing `owner external privileged service (root)` means the
root child is running. `0 peer prefixes learned` is normal on the first node and
a warning, not a failure, on a later one.

**Untested from a clean machine**: step 2's launchd/iroh-tun installation (sudo
is interactive in the test environment). Executed only on an already-installed
host, where `iroh-tun` reports a healthy external root service. The root service
can now be repaired without re-enrolling:

```sh
sudo mycelium iroh-tun install --overlay "$(mycelium iroh-tun address)" --tun utun220
sudo mycelium iroh-tun repair
```

## 3. Install Loam

Loam ships separately and supports both Apple architectures.

```sh
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/mycelium-linux/loam/releases/latest/download/loam-installer.sh | sh
loam --help
```

The installer verifies the published SHA-256 (again: checksum, not signature).
Loam also needs a running `myceliumd` for identity and, depending on the Spore,
the local frontdoor.

## 4. Sign in to FPL tools

Open <https://sso.fpl.dev>. Sign-in is a magic link to your `@fpl.dev` address;
after the first sign-in you can enrol a passkey in the account settings.

**Blocked today**: the SSO application is running, but its SMTP transport is
built without credentials, so magic-link mail will not be delivered. An
administrator must configure an authenticated relay before founders can sign
in. `https://sso.fpl.dev` returning 200 and `/vouch` returning 401 proves the
service is reachable, not that mail works.

## What not to do

- Do not copy `~/.mycelium` between machines; the node identity is the machine.
- Do not share a claim, a tunnel token, or an API token in chat.
- Do not install a binary whose digest you did not check, and do not "just use
  the latest from the internet" while the signed host is down.
- Do not add DNS records or tunnel ingress for a founder machine. Ingress is
  the fleet's, not the laptop's.

## Troubleshooting

| Symptom | Meaning |
|---|---|
| `doctor: daemon not running` after setup | launchd unit failed; `mycelium setup --repair` |
| `.myc` names resolve but nothing answers | frontdoor not installed: `mycelium frontdoor install --write --trust-cert` |
| `iroh-tun: permission denied` | the root child is not installed; re-run `setup --system-service` |
| `no peers yet` | expected on the first node; check `mycelium peers` and the LAN/overlay route |
| `sso.fpl.dev` login never arrives | SMTP credentials missing (administrator), not your account |
