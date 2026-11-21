# External subcommands

`mycelium` has a built-in command set and a git/cargo-style extension point.
An unknown first argument is looked up in `PATH` as `mycelium-<command>`:

```sh
mycelium store list          # runs mycelium-store list
mycelium spore install ...   # runs mycelium-spore install ...
```

Built-ins always win. The external program is only considered when the first
argument is not a built-in, so a package cannot shadow `status`, `peers`,
`setup`, `access`, or `daemon`.

## Contract

- The binary name is exactly `mycelium-<command>`.
- `<command>` is one path-free segment: lowercase ASCII letters, digits and
  internal hyphens, at most 64 bytes. Slashes, `..`, leading `-` and
  uppercase are refused.
- Arguments after the command are passed through unchanged.
- stdin, stdout, stderr and the exit status are the child's. `--json` and
  interactive confirmation belong to the child, not to the dispatcher.
- The child inherits the caller's environment, including `MYCELIUM_HOME` and
  `MYCELIUM_SOCKET`.
- If no matching binary is in `PATH`, `mycelium` reports an unknown command and
  names the missing binary.

## Why this shape

The store, a Spore host, and a vendor integration should be installable and
releasable without changing the core `mycelium` binary. Cargo and git both use
this contract; it needs no plugin manifest, no dynamic loading and no version
negotiation. The ABI is argv, environment, and exit status.

The same rule is appropriate for `soil`: `soil-<command>` for an unknown
`soil` subcommand. That keeps the store a Spore and keeps the host binary from
growing every product.

## Discovery

`mycelium --help` lists only built-ins. External commands are discovered from
`PATH`; a future completion generator can scan for `mycelium-*` without making
the core binary know their names.
