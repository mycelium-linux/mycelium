# Spore, Tweak, and Plugin v0

Status: draft product contract. Soil, Loam, and Mycelium each own a different
part of the implementation.

## Definitions

A **Spore** is a live Soil image.

A **Tweak** is an ordered Soil delta applied to a Spore image.

A **Plugin** is a non-editable WIT provider, implemented by a Wasm module or a
native sidecar.

A Spore may import and export WIT. Providing WIT does not make a Spore a Plugin.
The implementation model decides the artifact kind:

```text
Spore    mutable live image
Plugin   immutable WIT capability boundary
Tweak    image delta
```

`Guest`, `component`, and `provider` are roles or implementation details, not
top-level package kinds. A command or service is a Plugin role:

```text
plugin.role = provider | command | service
plugin.mode = wasm | native-sidecar
```

## Ownership

```text
Soil        language, image runtime, eval/load, deltas, snapshot/restore
Loam        host process, sandbox, WIT linking, capability composition
Mycelium    package specs, store, desktop placement, ACLs, fleet sync
```

### Soil

Soil owns the image semantics:

```text
core/image.soil
tools/image
core/repl.soil
core/repl-session.soil
core/safe-eval.soil
compiler/
```

It owns:

```text
live Runtime
eval/load
try-load rollback
image protocol
tweak delta application
image snapshot/restore
compiled-function cache invalidation
```

Soil does not own installation, store policy, desktop placement, or ACLs.

### Loam

Loam owns the execution host:

```text
process lifecycle and limits
sandbox
WIT provider linking
image host process
capability composition
start/stop/status plumbing
```

Loam calls the Soil image runtime. It must not reimplement image mutation.

### Mycelium

Mycelium owns the product and fleet:

```text
spore / tweak / plugin package specs
store and discovery
desktop placement
ACLs and trust policy
fleet sync and replication
install/update/rollback policy
```

## Package layouts

### Spore

```text
my-spore/
  spore.soil          image manifest
  image.soil          live image source or snapshot
  source/             optional source spine for recovery/diffing
  wit/                imported and exported contracts
  assets/
  signatures/
```

A Spore package may contain an optional compiled cache, but the image is the
artifact. The cache must be invalidated when a definition changes.

### Plugin

```text
my-plugin/
  plugin.soil
  module.wasm         Wasm provider/command/service
  bin/host            optional native sidecar
  wit/
  signatures/
```

A native sidecar uses a Wasm adapter:

```soil
(plugin
  (id "ajmwagar:surf")
  (mode "native-sidecar")
  (adapter "module.wasm")
  (sidecar "bin/surf-host")
  (exports "ajmwagar:surf/browser@0.1.0"))
```

### Tweak

```text
dark-marbles/
  tweak.soil
  delta/
    001-theme.soil
    002-commands.soil
  assets/
```

```soil
(tweak
  (id "personal/dark-marbles")
  (title "Dark Marbles")
  (description "A dark, high-contrast Marbles UI.")
  (tags "theme" "dark" "ui" "marbles")
  (target
    (spore "dev.fungos.marbles")
    (version "0.1.0")
    (digest "sha256:..."))
  (kind ui)
  (delta "delta/*.soil")
  (reload immediate))
```

A Tweak carries no grants. It cannot widen a Spore's WIT boundary.

## Runtime flow

```text
Mycelium decides what to install and which ACL applies
  -> Loam starts/attaches a Soil image host
  -> Soil applies ordered Tweak deltas to the live image
  -> Mycelium records the desktop, tweak, and trust state
```

The image is mutable. The package and its grants are immutable starting points.

## Store kinds

```text
spore
plugin
tweak
```

Discovery metadata for a Tweak:

```text
title
description
tags
target spore/version/digest
author
license
delta digest
preview CID
```

## Security

Install consent is defined in
[`spore-consent-v0.md`](spore-consent-v0.md): manifests request, operators approve,
Loam enforces, and Tweaks cannot widen.

```text
Tweak edits behavior inside existing grants
Tweak cannot add WIT imports
Tweak cannot widen capabilities
Tweak can be rejected by target/digest/ACL policy
Production can set image eval=false
```

## Landed

Loam PR #195 (merge `62e5a59`) makes function-only and native-sidecar plugin
installation real:

```text
loam plugin verify PATH
loam plugin list --json
loam plugin inspect ID-OR-INTERFACE
lock/signature/sidecar/executable verification
unsigned dev installs recorded in install state
duplicate-provider rejection
function-provider composition at install time
sidecar-backed adapters linked at runtime
bounded loam:plugin/ipc with payload canonical, params/result compatible
malformed/oversized/EOF/missing-sidecar failures
rollback and drift reporting
consumer -> adapter -> loam:plugin/ipc -> sidecar e2e proof
```

Still out of scope in Loam:

```text
resource exports
async exports
```

## Gap analysis

| Area | Current state | Gap |
| --- | --- | --- |
| Soil image protocol | `core/image.soil` and `tools/image/main.soil` exist and run one live image over JSON lines | no package kind, no snapshot/restore, no delta apply/revert transaction, no compiler loaded into image |
| Soil compiler | `compiler/core.soil` emits WAT/Wasm bytes; limited numeric/core and partial canonical ABI | cannot compile arbitrary Spore or full WIT surface; intended as checker/lowerer, not image runtime |
| Soil image persistence | `tools/image` tracks loaded sources and can flush sources | no image snapshot, no image delta log, no restore generation |
| Soil WIT imports | generated bindings and `host-call` exist | an image host world with `ajmwagar:surf`/`fpl:bob` imports and profile composition is missing |
| Loam image mode | no image mode; current Spore packages use `module.wasm` | add image package kind, runtime lifecycle, image profile composition |
| Loam plugin composition | function-only and native-sidecar plugins install, verify, inspect, compose, roll back, and hot-link; PR #195 merged | resource exports and async exports remain out of scope |
| Loam static imports | plugin provider links only into consumers importing the interface | image REPL/image host needs a profile-driven WIT import set |
| Mycelium spore spec | this document | needs executable package schema and migration from component-only Spore packages |
| Mycelium tweak spec | this document | needs tweak package reader, target validation, apply/revert call into Soil |
| Mycelium store | existing store paths for current Spores | needs `kind=spore|plugin|tweak`, search over description/tags, image/tweak digests |
| Mycelium desktops/ACLs | existing desktop/ACL direction | needs per-Spore image policy, tweak trust, eval lock policy |
| Mycelium fleet sync | VFS and records sync paths exist | needs image snapshots/tweak deltas and convergence policy |
| VFS | content-addressed storage, sync, R2 replica exist | image snapshots and tweak deltas are not first-class VFS objects yet |
| Surf plugin | WIT, adapter, sidecar, package, install tested | provider works; no image consumer/profile yet |
| Bob plugin | WIT and Soil bindings | adapter/package and Soil port not built |
| Theseus/LSP | separate agent/editor surfaces | no image profile/plugin integration |
| Deadwax Spore | Soil source controller and SSR routes exist in deadwax PR #1 | host lacks `soil:streams` HTTP accept and `soil:http` allowlist/credential wiring |
| Radioman Pi | no image Spore yet | needs host grants and provider boundary without process/socket/path exposure |

## First real image Spore: Deadwax

`ajmwagar/deadwax` PR #1 is the first practical image Spore controller.

Required host links:

```text
soil:streams with protocol = "http"
soil:http allowlisting http://127.0.0.1:6601 and https://api.spotify.com
soil:creds handle "spotify"
```

Boundary:

```text
deadwax serve owns local library/MPD
Spotify Web API is host-owned HTTP
the Spore never sees a token, socket, path, PCM, or process
librespot remains a separate plugin capability
```

This exposes the immediate host gap: `loam doctor` currently lists `soil:creds`
but not `soil:http` or `soil:streams`.

## Immediate next steps

1. Add `image` mode and image package/schema to Soil.
2. Add snapshot/restore and ordered delta apply/revert to `core/image.soil`.
3. Add `loamd image start/eval/snapshot/restore` host lifecycle.
4. Add profile-driven WIT imports to the image host.
5. Add Tweak package reader and target validation in Mycelium/Loam.
6. Add `kind=spore|plugin|tweak` to store/catalog and ACL policy.
7. Run one real Surf-backed image Spore.
