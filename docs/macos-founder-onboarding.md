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

The bootstrap installer is currently blocked by a dead release host, so this is
the verified-today path. It checks the published SHA-256; it does **not** verify
a signature, because the GitHub mirror carries none. That is a known gap, not a
recommendation.

```sh
set -eu
VERSION=v0.1.3
T=$(mktemp -d) && cd "$T"
gh release download "$VERSION" --repo mycelium-linux/mycelium \
  --pattern 'mycelium-*-aarch64-apple-darwin.tar.gz' --pattern SHA256SUMS
shasum -a 256 -c SHA256SUMS 2>&1 | grep 'aarch64-apple-darwin'
tar -xzf mycelium-*-aarch64-apple-darwin.tar.gz
mkdir -p "$HOME/.local/bin"
install -m 755 mycelium "$HOME/.local/bin/mycelium"
install -m 755 iroh-tun "$HOME/.local/bin/iroh-tun"
"$HOME/.local/bin/mycelium" doctor || true   # expected: no daemon yet
```

Verified 2026-11-25: archive extracts, SHA-256 matches, binary is
`Mach-O 64-bit executable arm64`, `doctor` runs.

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

**Untested from a clean machine**: steps 2's launchd/iroh-tun installation.
Executed only on an already-installed host, where `iroh-tun` reports a healthy
external root service.

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
