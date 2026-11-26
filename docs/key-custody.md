# Release key custody

Status: **cutover, 2026-11-24.** One signing key, one verify set, one rollout.

Every trust decision in the fleet now rests on a release signing key: managed
catalogue entries, release manifests, and anything else that is signed rather
than downloaded. If the key is wrong or missing, the failure is silent in one
direction (a host accepts nothing) and catastrophic in the other (a host accepts
anything). So custody is written down rather than remembered.

## What is pinned today

Checked on 2026-11-24:

| Host | `MYCELIUM_RELEASE_KEYS` | Private half present |
|---|---|---|
| studio | `962c…,14f5…` (after this change) | `14f5…` at `~/.mycelium/release-signing.key` (0600) |
| pris | `962c…` | no |
| agora-one | `962c…` | no |
| batty | empty | no |
| home-pi | unreachable on ssh | unknown |
| fulfillment-pi | not enrolled in the mesh | — |

`962c…` is **verify-only**: it is pinned on two hosts and its private half is
not present on any host reachable today. Nothing in the fleet could sign a
release with it, which means release signing was effectively broken before this
change, and no host could have verified a new managed install anyway.

`14f5…` is the current signer: `packaging/managed/trusted-keys.txt` records it,
and the catalogue is signed with it.

## Bootstrap manifest signer

The installer verifies `release-manifests.json` with `ssh-keygen` before running
anything, against a public key pinned in `install.sh`. Its private half is
`~/.mycelium/release-ssh-signing` (0600, studio); the public key is:

```
ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAID9wzQ7aEGWgxHnwuL90yrHHgMJDPScqR57hjmBFSQtw release@fpl.dev
```

`scripts/release.sh --publish` signs the manifest with it (namespace
`mycelium-release`) and uploads `release-manifests.json.sig` and
`allowed_signers`. Rotating it is a reviewed change to the pinned line in
`install.sh` **and** the release key: publish the new public key first, then sign
with it. Do not fetch `allowed_signers` from the release and trust it; that
would let the release vouch for itself.

## Custody

- **Private key**: `~/.mycelium/release-signing.key` on `studio`, mode `0600`,
  never in a repository, a chat, or a backup that is not encrypted.
- **Public key**: `packaging/managed/trusted-keys.txt` for reviewers, and
  `MYCELIUM_RELEASE_KEYS` on every host for enforcement.
- **Target state**: the private half moves into `geode`
  (`purpose: "mycelium.release-signing"`, bounded lease, audited reads). That
  needs a `geode` store verb: the broker is deliberately read-only over its
  socket, so storing is an operator action and there is no CLI for it yet. Until
  then a `0600` file on a single build host is the honest interim.

## Pinning a host

The canonical store is `~/.mycelium/node.env`: systemd reads it as an
`EnvironmentFile`, and `mycelium setup --repair` regenerates the launchd plist
from it. Pin with the CLI rather than editing the file, because it is idempotent
and preserves every other line:

```sh
mycelium releases trust --signing-key 14f549457cc7e979037abc8cc7812d4cae92f9b3d104fde4f6e17b239ebeb0f8 --write
mycelium setup --repair     # rewrites the unit/plist environment
# then restart the daemon (repair does this; verify it came back)
mycelium doctor
```

`releases trust` with no key prints the current set, so a rollout can read each
host before and after. The value is a comma-separated list: **append, never
replace.** A host that is verifying a release it already has must keep trusting
the key that signed it.

Hosts still on the old set: `pris`, `agora-one`, `batty` (empty), `home-pi`,
plus anything enrolled but not reachable when this was written. `batty` is the
first to fix because it currently trusts **no** release key at all.

## Rotation

1. Generate on the build host: `mycelium releases keygen --path NEW --write`
   (prints the public key; refuses to overwrite).
2. Append the new public key to `packaging/managed/trusted-keys.txt` and to every
   host's `MYCELIUM_RELEASE_KEYS` **before** signing anything with it.
3. Sign the catalogue and the next release with the new key.
4. Verify on a host: `mycelium managed verify --trusted-key NEW`.
5. Remove the old key only after every host has been seen running (not merely
   accepting) a release signed by the new one.

Never rotate by replacing the list: that invalidates the artifact a host may be
about to roll back to.

## Failure modes to recognize

| Symptom | Meaning |
|---|---|
| `no trusted release keys` | `MYCELIUM_RELEASE_KEYS` empty; host cannot verify anything |
| `signed by … which this host does not trust` | key not pinned, or pinned on a host that did not restart |
| `mycelium managed verify` non-zero, `verified: false` | manifest edited after signing |
| `verified: true`, `trusted: false` | key is real, but not in this host's set |
| `releases keygen` says the key exists | do not overwrite; rotate deliberately |

## What is deliberately not here

- **Geode storage** for the private half (needs a store verb; the broker is
  read-only by design).
- **Key material in CI.** Signing runs where the key is, and today that is one
  host, by hand. A CI signer needs either a hardware/attested key or a short-lived
  signing grant, not a repository secret.
- **Automatic rotation.** Rotation touches every host's trust set; it is an
  operator action with a verification step, not a timer.
