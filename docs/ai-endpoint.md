# ai.myc: one AI endpoint per machine

Every tool that talks to a model — pi, opencode, Delta, a Spore — points at
`https://ai.myc/v1`. One name, one credential, one place that counts tokens.
What answers behind that name depends on the machine and on what the mesh can
reach, not on how the client is configured.

```text
client (pi / opencode / Delta)
  -> https://ai.myc/v1
  -> local frontdoor (host-owned :443, TLS on loopback)
  -> 1. this machine      Bifrost :4000, or UMIE on a UMIE host
     2. this mesh         an `ai` lease owned by another node (UMIE)
     3. the public edge   https://ai.fpl.dev
```

The route is one entry in `$MYCELIUM_HOME/frontdoor/routes.json`:

```json
{
  "name": "ai",
  "hosts": ["ai.myc", "api.myc"],
  "upstream": ["http://127.0.0.1:4000", "https://ai.fpl.dev"],
  "service_kind": "ai",
  "remote_token_file": "$HOME/.fpl/bifrost-api-key",
  "priority": 10
}
```

Candidate order is derived, not written: loopback upstreams first, then fresh
`ai` service leases owned by this node, then leases owned by other nodes, then
non-loopback upstreams (the public edge). Only a connection failure advances to
the next candidate — a timeout or an HTTP error does not, because replaying a
completion could duplicate billed work.

`remote_token_file` is read per request, so the edge key rotates without a
frontdoor restart and never lands in a plist or unit file. See
[the frontdoor README](../crates/mycelium-frontdoor/README.md) for the full routing
and credential rules.

## Direct to UMIE on fleet nodes

UMIE (`umie-serve`) exposes the OpenAI surface the route needs:
`/v1/chat/completions`, `/v1/completions`, `/v1/models`, `/health`, `/metrics`,
and it reports `usage` (`prompt_tokens`, `completion_tokens`,
`prompt_tokens_details.cached_tokens`) in both buffered and streamed responses
when `stream_options.include_usage` is set.

On a UMIE host (agora-one, Titan, DGX Spark) the route lists UMIE as the local
backend:

```json
{
  "name": "ai",
  "hosts": ["ai.myc"],
  "upstream": ["http://127.0.0.1:8122", "https://ai.fpl.dev"],
  "service_kind": "ai"
}
```

That is enough for `ai.myc` on that host. For **other** fleet nodes, no extra
publishing step is needed — the normal scan produces the lease:

1. `ss -H -lnt` / `lsof` report the listener and its bound address;
2. the recognizer maps AI software by process or product name (`umie`,
   `llama-server`, `llama.cpp`, `ollama`, `vLLM`, `SGLang`, `TGI`, `LM Studio`,
   `bifrost`), or by Ollama's default port (derived confidence);
3. projection publishes an `ai` observation with an **HTTP** origin — the scheme
   is stated because the kind's contract is an OpenAI-compatible HTTP surface, not
   because a port proves anything;
4. every frontdoor's `service_kind: "ai"` discovery turns that into a candidate,
   ordered after this machine and before `ai.fpl.dev`.

One deployment rule follows from the second step: **bind the server beyond
loopback** (`0.0.0.0:8122` or the mesh address). A loopback-only listener is
published as `http://127.0.0.1:<port>` with `endpoint_scope=host-local`: the
owning host's frontdoor accepts it, every peer drops it as a remote loopback hint,
and nothing is advertised that a peer cannot reach. Scanning used to discard the
bind address, which is why an AI listener could never be discovered before.

For a service the scan cannot identify, the application-owner lease in
`$MYCELIUM_HOME/application-services.json` remains the general escape hatch
([application-owned service leases](application-services.md)): it expires within
300 seconds, so whatever publishes it must refresh it.

Mesh `ai` candidates are only tried after this machine's backends fail, so a peer
cannot take over traffic that a local backend can serve. When it is used, the
request goes to that peer, so the usual gossip trust applies: only advertise and
accept leases from nodes you would route prompts to.

## Models and capacity: claims in the mesh, counters in the adapter

A node that serves a model advertises it — and the limit it enforces — on its
`ai` service observation, in the open `attributes` bag the observation already
carries:

| attribute | meaning |
| --- | --- |
| `models` | model ids this node serves, comma-separated (`ornith-9b`) |
| `max_in_flight` | requests it admits at once |
| `tokens_per_sec` | optional: the rate it meters to |

The mesh does not queue, proxy or admit, and it does not choose. It publishes
what each node says about itself. **Bifrost owns inference**
([authority](authority.md), [scope audit](scope-audit.md)), so Bifrost is what
filters by model, admits or refuses, and keeps a conversation on a node.

### Who reads the claims

Bifrost should learn its uplinks the way the [iroh contract](iroh-discovery.md)
already tells applications to: run `mycelium services --json` (schema 1), select
fresh `kind: "ai"` observations, and fence the result by generation so a failed
read holds the last set instead of downgrading to an empty catalog. Whether
Bifrost reads that directly or a small adapter feeds it, the claims are input and
the routing stays Bifrost's.

That is why `ai.myc` is a *configured* route to the local Bifrost and not a
catalog-derived one. A configured route owns its hostname, and the catalog only
fills names nothing in `routes.json` claims
(`RouteTable::set_discovered`). Traffic that reached a box without passing
Bifrost would have no model filter and no admission, so the default name should
not resolve that way.

### Choosing a node (Bifrost)

1. Filter to uplinks whose `models` contains the requested model: an
   `ornith-122b` request never lands on a 9B box, and a 9B request never
   displaces the 122B.
2. A conversation with a live sticky uplink below `max_in_flight` keeps it.
3. Otherwise the least loaded filtered uplink; ties follow candidate order
   (this machine, this node's leases, peer leases, the edge). Bifrost's own
   in-flight tally undercounts direct callers, which is why the uplink's
   `overloaded` answer, not the tally, decides admission.
4. `overloaded` is terminal for the request, not a signal to advance mid
   conversation: replaying a completion can duplicate billed work and throws
   away the cache the affinity exists for.

### The cookie is Bifrost's

Bifrost terminates the request, so Bifrost sets the affinity cookie. The
gateway must not, and does not: it is a name resolver that also proxies, and
teaching it conversation semantics for one of its names would put inference
policy in the component that also serves `mcp.myc`, `files.myc` and `store.myc`.

The hop is transparent today — the gateway forwards `Cookie` upstream and copies
`Set-Cookie` back unchanged, dropping only hop-by-hop headers — so affinity
crosses it intact. Keep it that way: a header allowlist added for some other
reason would break affinity silently, which is why the contract is written down
here rather than left implied by a `matches!`.

`prompt_cache_key` (scoped by Bifrost to the authenticated identity) or a
caller-supplied `x-session-id` is the sticky key. Whatever Bifrost puts in the
cookie carries the choice between requests; the key itself is never a metric
label.

### When Bifrost is not in the path

A node that advertises `ai` earns a derived route only when `routes.json` claims
nothing for its name. That path resolves the name to the uplinks the catalog
names and does no balancing, no affinity and no admission. It is a way to reach
one server, not a router: two model servers behind a derived name is a
configuration mistake, not something the gateway should paper over.

## Token accounting

The frontdoor is the only hop every client shares, so it is where usage is counted.
For completion requests it reads the upstream `usage` object — from a buffered
JSON body, or from the final SSE event. A streaming request that did not ask for
usage gets `stream_options.include_usage` added; a client that set the field
explicitly keeps its choice, and `metering.stream_usage: false` turns the
injection off.

Two rates are reported, and they answer different questions:

- `tokens_per_second` — completion tokens over wall clock in a rolling 60-second
  window: what the fleet is producing right now, zero when idle;
- `generation_tokens_per_second` — completion tokens over request lifetime: how
  fast the backend generates while it generates.

Requests whose upstream reported nothing are counted as **unmetered**, never
estimated. A visible gap is usable; a plausible guess is not.

```sh
mycelium frontdoor usage            # this machine
mycelium frontdoor usage --json
mycelium frontdoor usage --fleet    # gossiped totals from every peer
curl -sk https://ai.myc/.mycelium/usage
curl -sk https://ai.myc/.mycelium/metrics
```

With `--usage` in the installed unit (the installer sets it), the frontdoor writes
`$MYCELIUM_HOME/frontdoor/usage.json` and `myceliumd` republishes the scalars as
`mycelium_ai.*` metrics inside the existing gossiped `platform_metrics`. A
snapshot older than ten minutes is not gossiped: a frontdoor that stopped must not
read as a fleet that went idle. Counters survive a restart; the throughput window
starts empty. One request crosses one frontdoor — do not chain `.myc` frontdoors, or
the same tokens are counted twice.

## Client configuration

All three clients use the OpenAI-compatible surface with the **local** Bifrost
key. The frontdoor replaces that key with `remote_token_file` when it falls
through to the public edge, so the local key never has to double as the cloud
key. Keep the local key in a file so it can be rotated without editing three
configs:

```sh
# the [user] key from ~/.config/bifrost-local/bifrost.toml
install -m 600 /dev/null ~/.fpl/bifrost-local-key
# paste the key, then verify:
curl -sk -H "Authorization: Bearer $(cat ~/.fpl/bifrost-local-key)" \
  https://ai.myc/v1/models
```

pi (`~/.pi/agent/models.json`):

```json
"bifrost": {
  "baseUrl": "https://ai.myc/v1",
  "api": "openai-completions",
  "apiKey": "!cat ~/.fpl/bifrost-local-key"
}
```

opencode (`~/.config/opencode/opencode.jsonc`):

```json
"Bifrost": {
  "npm": "@ai-sdk/openai-compatible",
  "options": {
    "apiKey": "<contents of ~/.fpl/bifrost-local-key>",
    "baseURL": "https://ai.myc/v1"
  }
}
```

Delta: a custom provider with `base_url: https://ai.myc/v1` and
`api_mode: open_ai_chat_completions`.

On a machine with no local backend, the caller's key is forwarded to the
configured public edge, so a cloud key in the client config also works there.
A discovered peer never receives the caller's key.

## Verification

```sh
mycelium doctor
curl -sk https://ai.myc/health
curl -sk -H "Authorization: Bearer $KEY" https://ai.myc/v1/models
mycelium frontdoor usage
mycelium frontdoor usage --fleet
```

`mycelium doctor` reports frontdoor state, certificate trust, route health and the
TLS handshake. Usage reporting is per machine: a host appears in
`frontdoor usage --fleet` only once its daemon has gossiped a fresh snapshot.
