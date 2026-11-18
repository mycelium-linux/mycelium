# Managed installs

A Spore is sandboxed and renders. Some of what a member wants cannot render: a
Cloudflare connector needs root and a token, a Prometheus pair needs disk and a
service, a build fleet needs ssh keys and a scheduler. Those are *managed
installs*: a signed manifest, four small scripts, and the runbook that explains
them.

The store distributes the catalogue. The trust is the same as for releases: a
host runs only manifests signed by a key it already trusts, unless the operator
names another key for a local catalogue.

## What one looks like

A catalogue entry is a directory:

```
cloudflared-tunnel/
  manifest.json     id, version, requirements, steps, docs, signer, signature
  check.sh          is this host ready? no writes
  install.sh        do the work
  verify.sh         prove it works after install
  uninstall.sh      remove it
  runbook.md        what it does, what can go wrong, how to undo it
```

```json
{
  "id": "cloudflared-tunnel",
  "title": "Cloudflare tunnel ingress",
  "version": "1.0.0",
  "channel": "stable",
  "summary": "public ingress for this node through a Cloudflare named tunnel",
  "platforms": ["aarch64-apple-darwin", "x86_64-unknown-linux-musl"],
  "requires": {
    "root": true,
    "network": true,
    "credentials": ["cloudflare.tunnel-token"],
    "commands": ["cloudflared"]
  },
  "provides": ["ingress.public"],
  "steps": {
    "check": "check.sh",
    "install": "install.sh",
    "verify": "verify.sh",
    "uninstall": "uninstall.sh"
  },
  "docs": { "runbook": "runbook.md" },
  "signer": "<hex ed25519 public key>",
  "signature": "<hex ed25519 signature>"
}
```

The signature covers every field an installer acts on - id, version, channel,
requirements, steps and docs - and not itself or the signer, so an entry can be
re-signed without editing it.

## Trust and consent are separate

- **Trust**: `apply` refuses a manifest whose signature does not verify, or whose
  signer is not in `--trusted-key` / `MYCELIUM_RELEASE_KEYS`. An unsigned entry
  never runs, even from the store that delivered it.
- **Consent**: a trusted manifest still has to be named, with `--write`. A
  manifest that declares `requires.root` additionally needs `--allow-root`: a
  manifest cannot grant itself privilege by being trusted.

The scripts are shell, not compiled into the binary, because the person
approving the install should be able to read exactly what will run. They receive
`MYCELIUM_MANAGED_ID`, `MYCELIUM_MANAGED_VERSION`, `MYCELIUM_MANAGED_DIR` and
`MYCELIUM_MANAGED_STATE`, so one entry works from a store, a checkout or a
mirror.

## Commands

```sh
mycelium managed list [--catalog DIR] [--json]
mycelium managed plan cloudflared-tunnel [--catalog DIR]
mycelium managed apply cloudflared-tunnel --allow-root --write
mycelium managed uninstall cloudflared-tunnel --allow-root --write
```

The default catalogue is `~/.mycelium/managed`. A store that serves a signed
catalogue is the distribution path; a checkout of this repository is a valid
catalogue too (`--catalog packaging/managed`), which is how an entry is developed
and reviewed before publishing.

## Runbook convention

A runbook answers, in this order: what it installs and where; what it needs
before it can run (privilege, credentials, ports, disk); how to verify it by
hand; what the common failures look like and what they mean; and how to undo it.
It names the exact commands it describes, because the first reader is someone
whose install just failed at 2am.

## The first catalogue

| id | what it provisions | notes |
|---|---|---|
| `cloudflared-tunnel` | Cloudflare named tunnel to the loopback ingress | needs a tunnel token; wired, mains, always-on hosts only |
| `fpl-bob` | build fleet client: bob + boat.dev | needs scheduler URL and a client token |
| `prometheus-ha` | paired Prometheus + `soil:metrics` scraping + Unibus alerts | disk and a service per node |
| `edgerunner` | edge runtime: Bifrost / Yggdrasil / UMIE | design tracked in `edge-ingress.md` |

## What this is not

A managed install is not a Spore and does not replace one: Spores stay
sandboxed, render, and are distributed as signed CIDs on the store channel.
Managed installs exist for the host-level pieces a Spore cannot own, and they
carry the documentation that makes them safe to run.
