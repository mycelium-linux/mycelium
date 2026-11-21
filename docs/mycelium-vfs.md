# `mycelium-vfs`: fleet-aware VFS front door

`mycelium-vfs` adds two fleet facts to the existing `fungos-vfs` CLI:

- which VFS roots exist on this machine (`roots`);
- how a Mycelium peer name maps to a reachable SSH destination (`peers`).

Everything else is passed through to `fungos-vfs`. There is no second sync
engine and no second content store.

```sh
mycelium vfs roots
mycelium vfs peers
mycelium vfs pull --root ~/.local/share/fungos/desktops/fpl --from neo
mycelium vfs serve --root ~/.local/share/fungos/desktops/fpl
mycelium vfs --root ~/.local/share/fungos/vfs pending
```

`mycelium vfs` is the external subcommand form of `mycelium-vfs`.

## Commands

| Command | Meaning |
|---|---|
| `peers [--json]` | Fleet peers with hostname, site, node id, address, and the SSH target chosen for sync. |
| `roots [--json]` | Desktop roots from `~/.config/fungos/desktops/*.toml` plus the default VFS root. |
| `pull --root DIR --from PEER [--remote-root DIR]` | Run `fungos-vfs sync pull` against that peer. The local root is used as the remote root unless `--remote-root` says otherwise. |
| `serve --root DIR` | Run `fungos-vfs sync serve`, usually as the remote command behind SSH. |
| anything else | Passed to `fungos-vfs` with the caller's arguments. |

## Transport and authority

`pull` uses `fungos-vfs`'s SSH carrier by default. The SSH destination is the
first observed peer address, so normal Mycelium SSH access grants apply. Set
`FUNGOS_VFS_SSH` to use another carrier, or use the generic passthrough for
QUIC:

```sh
mycelium vfs --root ~/vfs sync pull \
  --quic neo.example:7701 --fingerprint <hex>
```

The default peer address is a path hint, not an access grant. If SSH is denied,
sync is denied.

## What this is not

- Not a live-sync daemon. `fungos-vfs` sync is pull-based; this binary does not
  watch files or push.
- Not a merge policy. Divergence is still settled by `fungos-vfs sync resolve`.
- Not a second VFS. Content, ops, leases and replicas remain `fungos-vfs`.

The next step, once the CLI is in use, is a pull-on-schedule service or a
Mycelium-managed VFS replica observation. Neither changes the content model.
