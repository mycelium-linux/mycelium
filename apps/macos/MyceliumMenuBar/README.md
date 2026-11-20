# MyceliumMenuBar

The macOS menu bar app. See [`docs/macos-menubar.md`](../../../docs/macos-menubar.md)
for the design and the installer contract.

```sh
./build.sh                      # dist/macos/Mycelium.app
./build.sh --install --login-item
```

Source layout:

| File | Role |
|---|---|
| `MyceliumMenuBarApp.swift` | `MenuBarExtra` + Spores `Window` scenes |
| `StatusPanel.swift` | the Tailscale-shaped dropdown |
| `SporesWindow.swift` | installed Spores list |
| `MyceliumModel.swift` | observable state, JSON reads, write commands |
| `CommandRunner.swift` | runs the `mycelium` CLI with a timeout |
