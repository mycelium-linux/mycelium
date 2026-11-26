# Ingress failover receipt — 2026-11-26

Status: **connector-loss canary passed; origin failover is not deployed.** No
public route, DNS record, or Palisade upstream was changed.

## Actual topology

| Host | Cloudflare connector | HA connections | Public origin used |
|---|---|---|---|
| Batty | `cloudflared-fpl-ingress` | 4 | `https://100.70.187.127:446` (Palisade on Batty) |
| Pris | `cloudflared-fpl-ingress` | 4 | the same `100.70.187.127:446` |
| Agora One | `cloudflared-fpl-ingress` | 4 | the same `100.70.187.127:446` |

Tunnel: `878e9542-bce7-4008-a9cd-8238c56e0636` (remotely managed, config
version 5):

```json
{"ingress":[
  {"hostname":"sso.fpl.dev","service":"http://100.70.187.127:4000"},
  {"hostname":"*.fpl.dev","originRequest":{"noTLSVerify":true,"originServerName":"elmers.fpl.dev"},"service":"https://100.70.187.127:446"},
  {"service":"http_status:404"}
]}
```

There is a second tunnel, `f3cc7436-1837-427f-bbe1-5dc3d6cdf4e8`
(`fpl-edge`), for `edge-canary.fpl.dev`, `ai-lab.fpl.dev`, and (on Pris)
`twenty.fpl.dev`. It does not carry the wildcard.

## Canary

Stopped `cloudflared-fpl-ingress` on Pris, sampled the public paths every second
for 20 seconds, then restarted it. The other eight connections (Batty + Agora)
continued to serve.

```text
31 samples
sso.fpl.dev     200 x31
m.fpl.dev       303 x31
mcp.fpl.dev     401 x31 (no bearer)
elmers.fpl.dev  302 x31 (SSO redirect)
```

No client-visible error was observed. The stopped connector returned with four
connections (`cloudflared_tunnel_ha_connections 4`).

This proves **connector** loss is survived. It does not prove **origin** loss:
if Batty's Palisade or Batty itself fails, all three connectors still dial the
same unreachable origin.

## The Studio dependency

Palisade's `mcp_agora` vhost currently forwards `mcp.fpl.dev` to
`100.108.9.93:8805`, which is the Mac Studio MCP process. The documented Agora
upstream is `100.80.85.86:18792`. The public MCP path therefore depends on a
laptop being awake, contrary to the durable-ingress requirement. It was not
changed: the Agora listener currently answers `tools/list` without a bearer,
whereas Studio rejects it with `401`; switching without auth parity would weaken
the public path.

## Blockers to true origin failover

1. **No second Palisade.** `fab-9i3` records that Priss ingress does not exist;
   Pris has `palisade-avery` for Fab Sites only, not the wildcard vhost table.
2. **Tunnel config is remotely managed.** Updating the wildcard origin, or
   splitting it across per-host tunnels, needs Cloudflare tunnel/DNS admin. The
   available local credential cannot edit tunnel configuration.
3. **Stateful auth is single-host.** `fpl-auth` runs with SQLite at
   `/data/fpl-auth.db` on Batty. A second active origin would duplicate the
   writer; the safe path is shared Postgres/HA, not a copied database.
4. **Agora MCP auth parity is missing.** It exposes `tools/list` without a
   bearer, so it is not a drop-in replacement for Studio's authenticated route.
5. **Beachhead has no reachable control path.** The declared host
   `167.172.159.68` is not SSH-reachable from here and needs its DigitalOcean
   token; it cannot be used as an independent origin today.

## Reversible next step

Use the already-implemented `mycelium ingress route HOST` ranking (`86db03a`)
behind the existing connector, with a local health gate, rather than adding a
parallel platform. It needs one origin that serves the same vhosts and one
Cloudflare config change; both are blocked items above.

