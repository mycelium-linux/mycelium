# Cloudflare tunnel ingress

Public ingress for this node through a Cloudflare named tunnel. Traffic lands at
Cloudflare, the connector dials out from this host, and the tunnel forwards to
the loopback ingress that Mycelium already runs (`http://127.0.0.1:80`, which
socket-forwards to the frontdoor on `:443`). No inbound port is opened on the
host or the router.

## What it installs

| Path | What |
|---|---|
| `cloudflared` (Homebrew on macOS, package on Linux) | the connector |
| `~/Library/LaunchAgents`-equivalent *system* service `dev.fpl.cloudflared` / `mycelium-cloudflared.service` | runs the connector as root, restarts it |
| `~/.mycelium/managed/cloudflared-tunnel/token` (`0600`) | the tunnel token |
| `/var/log/mycelium-cloudflared.log` | connector log |

The tunnel's public hostname lives in Cloudflare, not here: the connector does not
write DNS records.

## Before you run it

- **A host that stays up.** Wired, mains, always on. `check.sh` refuses to run on
  battery, which is the same rule the edge design uses to pick ingress candidates.
- **A tunnel token** from the Cloudflare dashboard (Zero Trust → Networks →
  Tunnels → create → the `cloudflared tunnel run --token` value). Stored in
  geode as `cloudflare.tunnel-token`; export it as `CLOUDFLARE_TUNNEL_TOKEN` when
  running the install.
- **A domain you control**, for the wildcard. Point `*.myc.<your-domain>` at the
  tunnel as a public hostname with origin `http://127.0.0.1:80`.
- **The loopback ingress already answering.** If `install.sh` runs before the
  frontdoor is installed, the connector is up but has nothing to forward to.

## Verify by hand

```sh
mycelium managed plan cloudflared-tunnel
mycelium managed apply cloudflared-tunnel --allow-root --write
```

Then, from anywhere with the domain in DNS:

```sh
curl -sS -o /dev/null -w '%{http_code}\n' https://<node>.myc.<your-domain>/
```

A `200` or a Mycelium page means the path is live end to end.

## Failures and what they mean

| Symptom | Meaning |
|---|---|
| `no saved tunnel token` | the install ran without `CLOUDFLARE_TUNNEL_TOKEN` |
| Connector up, requests 502 | the loopback ingress is not answering on `:80` |
| 1033 from Cloudflare | the tunnel exists but no connector is attached |
| Works on one network, not another | the host lost its outbound connection; the connector reconnects with backoff |

## Undo

```sh
mycelium managed uninstall cloudflared-tunnel --allow-root --write
```

That removes the service and the token from this host. **It does not delete the
tunnel or its DNS record** — those are a console decision with a public side
effect, and the runbook is the wrong place to make it silently.
