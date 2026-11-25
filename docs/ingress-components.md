# Ingress components

Status: **design, 2026-11-25.** Supersedes "the frontdoor is a binary you install
by hand".

Public traffic needs four things, and they are four different programs with four
different lifetimes. Shipping them together is a packaging decision, not an
architecture one: each is installed, verified, rolled back and reasoned about on
its own.

| Component | Owns | Lifetime |
|---|---|---|
| **mycelium** | fleet identity, transport (iroh), leases, gossip, path selection | always-on daemon |
| **loam** | the wasm host: Spores, `soil:*` capabilities, `soil:streams` | always-on daemon |
| **thyra** | the node's local ingress adapter: plaintext→mTLS, LAN/browser TLS | per node that accepts non-iroh traffic |
| **palisade** | routing policy, caller identity, public names, zone records | the ingress host(s) |

## Anycast is three layers, not one

It is worth being explicit, because "anycast via iroh" conflates three hops:

1. **Cloudflare's edge is the only true anycast.** A client resolves
   `*.fpl.dev` to Cloudflare, and Cloudflare hands the connection to the nearest
   *connector*. We do not implement this and should not pretend to.
2. **Connector → origin is ours.** A connector knows only which tunnel it
   belongs to. Which node should actually serve `elmers.fpl.dev` is a fleet
   question, answered from the service catalogue:
   `mycelium ingress route elmers.fpl.dev` ranks advertised services by
   **same site first**, then **iroh reachability**, then freshness, and returns a
   node, an endpoint and the ALPNs that node serves.
3. **Origin → service is local.** Once the call reaches the chosen node it is
   ordinary local dispatch (palisade vhost, loam route, spore).

So iroh is not anycast; it is the thing that makes the *origin* choice cheap and
verifiable. The connector no longer needs a hand-maintained table of upstream
addresses, and a service that moves between sites is routable as soon as its
lease moves.

```sh
# what a connector asks, instead of a static upstream
mycelium ingress route elmers.fpl.dev --json
{"host":"elmers.fpl.dev","site":"fpl",
 "chosen":{"name":"elmers.myc","node_id":"…","site":"fpl",
           "endpoints":["iroh://…"],"alpns":["soil/http/1"],"score":115},
 "candidates":[…]}
```

The ranking lives in `crates/mycelium-cli/src/ingress_route.rs` and is covered by
tests for the two rules that matter: same-site beats a fresher remote node, and
a deterministic tie-break so two connectors cannot disagree.

## Shipping: one train, four artifacts

A release publishes, per target:

```text
mycelium      binary          managed units, daemons
loam          binary          runtime + spore host
palisade      binary|image    ingress
thyra         binary|image    edge adapter
```

All four are signed by the same release key and verified by the same
`MYCELIUM_RELEASE_KEYS`, and each has a managed package
(`palisade-ingress`, `thyra-edge`) with check/install/verify/uninstall and a
runbook. Managed installs record a receipt and keep the previous version, so a
bad ingress release is a `mycelium managed rollback` away — the mechanism is
already in place; what is missing is the artifacts and the two manifests.

The unit contract is deliberately small:

- **palisade**: one listener, one config file, `GET /config/revision` for
  convergence, `SIGHUP`/`POST /reload` for rollout.
- **thyra**: one listener, one mode (`edge` = loopback for a connector, `lan` =
  browser TLS), one upstream (palisade over the mesh).

Neither may own a Cloudflare token, a tunnel, or a public DNS record. Those are
mycelium's (tunnel + leases) and palisade's (zone rendering), respectively.

## What exists today, and what does not

```text
done     mycelium ingress route (nearest-origin ranking, tested)
done     palisade reload + revision, tunnel-provenance identity
done     thyra edge/lan modes
done     cloudflared connector as a managed package
missing  palisade/thyra release artifacts in the signed train
missing  palisade-ingress and thyra-edge manifests
missing  connector consuming `ingress route` instead of a static service
missing  second ingress origin for real HA (connectors are redundant; batty is not)
```

The last item is the sharp edge: today twelve connections across three sites all
forward to one `thyra → palisade` on batty. Connector loss is survived; host loss
is not. A second palisade on pris, with the tunnel ingress split by hostname or
a second tunnel, is the next step, and it is exactly the case `ingress route`
already knows how to rank.
