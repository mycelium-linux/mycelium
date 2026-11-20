# Mycelium menu bar (macOS)

A Tailscale-shaped surface for the machine Mycelium is installed on: one menu
bar item with live status, the two toggles worth having in a menu, a Spores
window, and a door to the browser for everything else.

It is a client, not a second daemon. Every value comes from the same
`mycelium … --json` call a person would run, and every write is the same
`--write` command. It holds no credentials and opens no privileged socket.

## What is in the panel

- **Identity**: hostname, site, short node id, and a colour that means
  *daemon down*, *tunnel configured but stopped*, or *tunnel running*.
- **Overlay**: the addresses from `mycelium iroh-tun status --json`.
- **Peers**: the count and the first five rows from
  `mycelium fleet status --json`.
- **Frontdoor**: installed/listening from `mycelium frontdoor status --json`.
- **Toggles**: daemon start/stop, public relays (only when the binary was
  built with `iroh-sync`), and tunnel restart when a tunnel is configured.
- **Actions**: Open Home (`https://home.myc/`), Open Store
  (`https://store.myc/`), Spores…, Quit.

Anything that needs a form, a table, or an explanation opens in the browser at
`home.myc`; the panel deliberately stops where a native menu stops being the
right shape.

## Spores window

The window scans the two install roots the installer writes
(`~/Library/Application Support/fungOS/spores` and
`~/.local/share/fungos/spores`), reads each `spore.toml`, and lists the Spore's
title, id, and declared `[http] hosts`. **Open** goes to the Spore's own host
when it declares one, otherwise to `home.myc`; **Reveal** shows the bundle in
Finder. Installing, permissions, and updates stay in the Store.

## Build and install

```sh
apps/macos/MyceliumMenuBar/build.sh
apps/macos/MyceliumMenuBar/build.sh --install --login-item
```

The first form assembles `dist/macos/Mycelium.app`. The second copies it to
`~/Applications` and writes `dev.fpl.mycelium-menubar` as a LaunchAgent
(`RunAtLoad`, no `KeepAlive`, so Quit stays quit). `install.sh` performs the
same install when a bundle is available at `dist/macos/Mycelium.app` or passed
with `--menubar-app PATH`; `uninstall.sh` removes both the bundle and the
agent.

The app needs macOS 14 (for `MenuBarExtra` with a custom window) and the
`mycelium` CLI on `PATH` or at one of the installer's locations. Set
`MYCELIUM_BIN` to point it at a development build.

## Why not a Soil/Canvas surface

The menu bar item is a window-server object and a login item; that is native
authority in the same list as `canvas-winit` and `iroh-tun`. Rendering status
with a wasm Spore would still need a native host process to own the status
item, so the smaller program is this bundle, with the Spore-browsing surface
already living at `store.myc`/`home.myc`.
