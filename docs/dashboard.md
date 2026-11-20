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
| **Fleet members** | Who is enrolled, where, how fresh, what version, is SSH listening? | `fleet status --json` |
| **Geode** | Is the owner-local broker live, is its store private, how big is it? | socket probe + `~/.geode` metadata |
| **Desktops & VFS** | Which content roots exist and are mounted; which Canvas surfaces are installed? | `mount` + the Spore install roots |
| **Pulse** | Which schedules the daemon owns, and what they last did. | `pulse list`, `pulse runs` |
| **Settings** | Dashboard refresh/theme, public relays, the frontdoor route table. | `dashboard.json`, `iroh-sync.json`, `routes.json` + `sync relays` |

Geode and VFS are deliberately read-only here. Leasing a secret and mounting a
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
