# Fleet top

`mycelium fleet top` is the agent- and terminal-readable version of the
dashboard's fleet view: one row per enrolled peer, grouped by site, with the
health the peer already gossips.

```sh
mycelium fleet top
mycelium fleet top --site wagar-house
mycelium fleet top --platform darwin --json
```

Columns: site, host, 1-minute load, used/total memory, used/total disk, uptime,
SSH listener.

`--json` returns one object per peer:

```json
{
  "node_id": "…",
  "hostname": "agora-one",
  "site": "wagar-house",
  "platform": "linux",
  "daemon_version": "0.1.3",
  "last_seen": 1795159447,
  "load_average": [0.36, 0.16, 0.05],
  "logical_cpus": 4,
  "memory_total_bytes": 3980124160,
  "memory_used_bytes": 579272704,
  "disk_total_bytes": 62262038528,
  "disk_used_bytes": 11432628224,
  "uptime_seconds": 4181036,
  "ssh_listening": true,
  "power": "unknown",
  "pressure_memory": "some avg10=0.00 avg60=0.00 avg300=0.00 total=0",
  "network_rx_bytes": "1301186187707",
  "network_tx_bytes": "1301186187707",
  "top_process": "mycelium"
}
```

## What each number is

- **Memory / disk / uptime** come from the peer's `HostHealth`, the same
  document that already carries load, filesystems and process leaders. Missing
  fields render as `—`, not zero: an older peer that does not report a metric
  is unknown, not empty.
- **Load** is the peer's own 1-minute load average. It is not normalized by CPU
  count; compare within a host or read `logical_cpus` from `--json`.
- **Pressure** is the Linux PSI `avg10` value when present. It is the earliest
  warning that a host is short on CPU, memory or IO.
- **Bandwidth** uses cumulative non-loopback interface counters
  (`network_rx_bytes`/`network_tx_bytes`) from the peer health. The dashboard
  computes a rate from two observations, so the first refresh after a reload
  shows `—` and the next one shows bytes/second. Older peers do not report the
  counters until they run a daemon that includes them.

## Sites

A site is a LAN/network label from the peer `hello.site`. `fleet top` groups by
it; the dashboard draws one card per site with online count, memory and disk
utilisation, versions and SSH coverage.

The fleet keeps reporting a peer that is known but has not gossiped health yet.
Those rows show `—` for memory/disk/uptime.

## Danger zones

`fleet exec` runs an arbitrary command on every selected peer with that peer's
authority. It now requires one of:

- an interactive `type 'yes'` confirmation, or
- `--yes` when running non-interactively (an agent's shell).

Without either it refuses:

```text
mycelium: fleet exec is destructive; pass --yes in non-interactive use
```

The rest of the read path (`status`, `top`, `peers`) is safe and needs no
confirmation.
