# Logic placement: what stays in Rust, what moves up

Status: **analysis, not a mandate**
Companion to: `docs/lua-plugin-audit.md` (the per-area decisions), `docs/dns-lua-drivers.md`
(the existing host adapters), `docs/native-components.md`.

The question is not "how much can we rewrite in Lua". It is: **which logic is
Rust because it must be, and which logic is Rust only because that is where it
was written first?** This document separates those two and puts a number on each.

## 1. Measured shape

Line counts are from `crates/*`, `.rs` files, tests included. They are a
rough size signal, not a quality score.

| Region | Lines | Share | Character |
|---|---:|---:|---|
| `mycelium-cli` | 23,746 | 27% | command dispatch, rendering, orchestration of one-shot operations |
| `myceliumd` | 21,558 | 25% | long-lived state: peers, leases, supervisors, RPC |
| core/contract/policy (`mycelium-core`, `mycelium-peer-protocol`, `mycelium-dns-policy`, `geode-*`, `mycelium-frontdoor`) | 19,667 | 23% | typed contracts, wire formats, policy evaluation |
| drivers and observers (`mycelium-driver-*`, `dnssd`, `ssdp`, `tailscale`, `doh`, `iroh-discovery`) | 12,506 | 14% | vendor dialects, platform observation, transports |
| Lua host and plugins (`mycelium-plugins-lua`) | 1,697 + 455 | 2% | the current bounded scripting boundary |
| **Total Rust** | **86,830** | | 671 `#[test]` functions |

Two facts matter more than the totals:

- **Half the tree is CLI + daemon** (52%). That is where "logic" accumulates
  fastest, and it is almost entirely *sequential I/O and human-facing text*.
  It is also the least protocol-sensitive code in the repository.
- **The scripting boundary is already 2%, and it works.** UniFi AP, NextDNS and
  Pi-hole dialects are Lua today (`docs/lua-plugin-audit.md` marks the AP
  migration complete). The question is not whether the boundary can carry
  logic; it is what else belongs on the far side of it.

## 2. The rule, stated as a test

Move a piece of logic to a higher-level language when **all** hold:

1. It is product- or policy-specific, not protocol-specific.
2. It is a pure function of already-validated inputs (recognize, parse, plan,
   render).
3. Rust can validate its inputs *and its outputs* at a narrow typed boundary.
4. It needs no sockets, files, secrets, clocks, retries, or free-form processes.
5. A malicious or malformed version cannot bypass dry-run, mutation risk,
   verification, authorization, or persistence.

If (4) or (5) fails, it stays in Rust regardless of how "declarative" it looks.
Security-sensitive escaping, network type validation, transport lifecycle and
merge semantics get exactly one implementation, in Rust.

## 3. Four buckets

### A. Must stay Rust — non-negotiable

| Area | Why |
|---|---|
| Wire formats (peer protocol, service catalog, DNS record sets, topology) | Bounds, canonicalization, and signature checks are the security boundary. A script that can emit a name or an address is a script that can forge one. |
| Identity and key material (ed25519, passkeys, CA, frontdoor TLS) | Zeroization, constant-time comparison, key-file permissions. Not expressible safely in a sandbox without re-adding all of it as host calls. |
| Transports (SSH session lifecycle, TLS, HTTP, SNMP BER, QUIC/iroh, dnsmasq apply) | Timeouts, retries, credential references, rate limits, rollback windows. |
| Mutation gates (dry-run → plan → apply → verify → record) | The gate must be unbypassable from plugin code. It is a property of the host, not of each dialect. |
| Topology/path semantics (which address is an identity, which is a path; LAN/overlay/mesh precedence) | Deciding what a record *means* is policy that must be identical in every adapter. |
| Hardware/firmware paths (`mycelium-firmware-hal`, `imgpatch`, Redfish actions) | Binary bounds, integrity, and irreversibility. |

### B. Should move up — dialect and rendering

These are small, numerous, and change for vendor reasons, not protocol reasons:

1. **Vendor text dialects** — the remaining command spelling and output parsing
   in EdgeOS/UniFi-AP-shaped drivers. Rust keeps SSH, quoting, output limits and
   the risk gate; Lua turns `{program, args}` into a plan and stdout into facts.
2. **SNMP enterprise/vendor classification** — feed normalized varbinds to a
   pure classifier. OIDs, GET/WALK, row limits and SET authorization stay Rust.
3. **Service-alias and route tables** — `service_aliases` and the frontdoor
   `routes.json` are already data. They are one step from being loaded rather
   than compiled, and doing so removes a rebuild from an operator's loop.
4. **Recognition profiles** — ONVIF/HomeKit/Bambu-style advertisements already
   go through Lua (`recognizers/*.lua`). Continue; do not grow a Rust recognizer
   for the next product.

### C. Should move up — orchestration and presentation (the big prize)

This is the 52% and it is not driver work at all. Two distinct sub-buckets:

**C1. Command *shape*, not command *behavior*.** `mycelium-cli/src/main.rs` is
6,482 lines with 194 dispatch arms. Most arms are the same three moves: parse
flags into a struct, call one daemon RPC, render the result. That is a table,
not a program. A declarative command manifest — name, flags, RPC, renderer —
would delete most of the 194 arms while keeping every behavior in Rust. The
Rust side becomes a small set of RPC verbs; the CLI becomes data plus a generic
dispatcher. This is the highest line-per-risk change available.

**C2. Human-facing rendering.** Status lines, doctor prose, table columns, and
dashboard cards are text policy. They need no privileges and are the most
frequently edited surface. Moving renderers behind a stable JSON contract lets
them change without touching the daemon, and lets a test assert on the JSON
instead of on prose.

The existing `--json` contract is already the right seam: the daemon emits typed
JSON and the CLI formats it. Finish the split rather than inventing a new one.

### D. Do not move, even though it is repetitive

| Tempting | Why not |
|---|---|
| Shell quoting/escaping (26 sites today) | One Rust primitive; plugins emit argv, never text. |
| Slug generation, CIDR parsing, address classification | Cross-cutting invariants. One implementation or the fleet disagrees with itself. |
| Tailscale/DNS-SD/SSDP/STUN/WireGuard parsing | Wire formats and shared observation. Consumers get normalized facts. |
| dnsmasq config rendering of *safety-relevant* lines | Injection surface. Rendering *policy tables* (C1) is fine; emitting resolver directives is not. |

## 4. What the overlay/DNS work showed

Two live defects found while validating this analysis are good tests of the rule:

- A tunnel child that learns the prefix **it answers for** from the catalog
  installs a route to its own endpoint. Every count said "1 peer learned". The
  bug was not in the parsing (bucket B) — it was that a *fact* (peer vs self)
  was never typed. Rust had to classify it, because the classification is the
  meaning of the number.
- `.myc` flat names published any IPv6 address the local policy admitted,
  including another mesh's ULA. The same fleet therefore answered `<node>.myc`
  with different address sets depending on which node built the zone. The fix
  was a rule, not a dialect: **a flat name is identity, so it carries only the
  overlay prefix.** Where one host's configuration could change another host's
  answer, the logic was in the wrong place.

Both are bucket-A items: meaning and identity stay in Rust. A Lua rewrite would
have made both worse by adding a second place to be wrong.

## 5. Sequencing

Ordered by payoff per unit of risk, not by size:

1. **Type the facts people count.** Anything the UI totals (peers, routes,
   leases, records) gets a named variant before it gets a renderer. Cheap, and
   it is what turned the overlay bug from invisible to obvious.
2. **Command manifest for the CLI (C1).** Start with the read-only verbs.
   Measurable target: `main.rs` under 2,000 lines with no lost tests.
3. **Renderer split (C2).** Move doctor/status text behind the existing `--json`
   contract. Assert on JSON.
4. **Finish the dialect migrations already designed** in
   `docs/lua-plugin-audit.md` (SNMP classifier, EdgeOS intent-to-command).
5. **Only then** consider a generic bounded HTTP host — and only when two real
   drivers need it.

## 6. Smells that mean you moved the wrong thing

- A plugin needs a new host call to work at all.
- A rule now exists in both Rust and a script, and can drift.
- A malformed script can produce a plan the host would not have produced.
- Two hosts answer the same name differently because their *local* config
  differs.
- A test can only assert on prose.

If any of these appear, the logic was not dialect. Put it back.
