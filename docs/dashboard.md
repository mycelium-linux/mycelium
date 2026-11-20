# The Mycelium dashboard (`home.myc`)

`home.myc` used to be the soil-home launcher: a grid of links. It is now the
Mycelium dashboard, served by the frontdoor itself. The old launcher service is
retired (the installer stops and removes `dev.fungos.soil-home`); nothing else
has to run for the page to exist.

The dashboard is a client, not a second control plane. Every read is the same
`mycelium … --json` call a person would run, and every write is the same
`--write` command. It holds no credentials, leases no Geode secret and mounts
nothing.

## Sections

| Section | What it answers | How it reads |
|---|---|---|
| **Overview** | Is the daemon up, is the overlay up, are the frontdoor and peers healthy? | `node status`, `fleet status`, `iroh-tun status`, internal frontdoor state |
| **Home** | A quiet launcher: greeting, one-line state, and doors into Pulse, Activity, Files, Spores, Fleet, Vault, Connect and Settings. | the probes below |
| **Pulse** | The next few things coming up, all pulses in human language, and the checks Mycelium runs on its own. No cron, no run log. | `pulse list --json` (`next_at_ms`) |
| **Activity** | A human view of the local bus: connected sources, what they publish, where kinds route, and a plain-language event feed. | `~/.config/unibus/hub.json` + a TCP probe + `hub.log` tail |
| **Files** | A Finder-shaped browser over the content roots: sidebar, list, breadcrumb, inline preview for text and images, and **Reveal in Finder**. `Open Files` goes to `files.myc` | `/api/files` over the allowed roots + `mount` |
| **Spores** | Every installed Spore, separate from content: its own mark (or badge/colour), one-line description, Open and Reveal, plus the Store. | the two Spore install roots + `[presentation]` |
| **Fleet** | One health view: site cards with online/memory/disk/versions, plus a fleettop table (load, memory, disk, network rate, uptime, SSH) and a row that opens into addresses, capabilities, top process and pressure. | `fleet status --json` joined with `peers --json`; the same numbers are available as `mycelium fleet top [--json]` ([fleet-top.md](fleet-top.md)) |
| **Vault (Geode)** | Is the local broker live, is its store private, how big is it? Plus a provider key map (Bifrost, Cloudflare, Loki, Unibus) with logos, presence, Reveal, and rotation for keys that have a safe write path. | socket probe + `~/.geode` metadata + the host's key files |
| **Connect** | AI (`ai.myc`/Bifrost accounts and named backends, not loopback URLs), MCP (`mcp.myc` and its configs), Loki, the Cloudflare connector, `.myc` hosts, and Spore installs. | the Bifrost TOML + route table + config files + TCP probes |
| **Settings** | Profile (name, POSIX user, picture), dashboard refresh/theme, public relays, the **overlay tunnel**, and (under Advanced) the frontdoor routes. Controls are bounded where the host has a finite set of choices. | `profile.json`/`avatar`, `dashboard.json`, `iroh-sync.json`, `iroh-tun.json`, `routes.json` |

Unibus is a TCP bus with no HTTP admin surface, so the page reports the hub
it can verify (listener, producers, routes, recent log) rather than inventing
one. A richer, message-level firehose belongs in a Spore that consumes the bus
with its own grants; the per-kind templates for that belong in the Spore
manifest.

Geode and the Files browser are deliberately read-only: there is no vault item
API yet, and the Files browser only reads inside the installed content roots.
Connect writes the Bifrost API key to `~/.fpl/bifrost-api-key`; Vault writes the
Cloudflare tunnel token to the managed token file. Both are `0600`. Everything
else in the key map is detected and revealed, not copied into the page. It reads the local Bifrost config for the account and
named backend list, so the page says "This Mac / Agora / Loki / FPL Cloud"
instead of a loopback URL. `files.myc` still owns CID-addressed browsing; the
dashboard links to it rather than reimplementing that contract. Leasing a secret and mounting a
filesystem are operator authority with their own confirmation path; a browser
form is not that path. The page says so rather than hiding the surface.

Pulse is not a Spore. It needs the daemon's authority (it executes commands on a
schedule) and keeps a durable run history in the daemon's store, so it lives
with the daemon and the dashboard is only its console.

## Writes

All mutations require the `x-mycelium-dashboard: 1` header that only this page's
JS sets. That makes a cross-site HTML form insufficient even when a browser is
holding a frontdoor token. The frontdoor's own bearer-token check runs first.

| Action | Command |
|---|---|
| Daemon start/stop | `mycelium daemon start|stop` |
| Tunnel restart | `mycelium iroh-tun restart --write` |
| Public relays | `mycelium sync relays enable|disable --write` |
| Pulse add/toggle/remove | `mycelium pulse add|toggle|remove … --write` |
| Profile | writes `~/.mycelium/dashboard/profile.json` (0600) and `avatar` (0600). Pictures up to 20 MiB are resized in the browser to 512px before upload. |
| AI key | writes `~/.fpl/bifrost-api-key` (0600) |
| Cloudflare token | writes `~/.mycelium/managed/cloudflared-tunnel/token` (0600) |
| Reveal | opens a path in Finder (macOS) or the desktop file manager; restricted to the content roots, Spore roots and Mycelium home |
| Overlay settings | `mycelium iroh-tun config … --write` |
| Save routes | validates with `FrontdoorConfig` and writes `routes.json` |

Dashboard preferences are the only state the page owns:
`~/.mycelium/dashboard.json` (`refresh_secs`, `theme`), written atomically at
mode `0600`.

## Configuration

The frontdoor config gains a `dashboard` table:

```json
{
  "mycelium_bin": "mycelium",
  "dashboard": { "hosts": ["home.myc"] },
  "routes": [ ... ]
}
```

Older configs without the table still answer `home.myc` as the dashboard: the
default host list is `["home.myc"]`. A host listed here is answered before route
selection, so a stale `home` route from an earlier install cannot win.

## Why the frontdoor serves it

TLS, the `.myc` names, the bearer token and the user's session already live in
the frontdoor. A separate dashboard process would need all four again. The page
is one embedded HTML file in the binary (`crates/mycelium-frontdoor/dashboard/`),
so there is no asset directory to install and no second service to keep running.
