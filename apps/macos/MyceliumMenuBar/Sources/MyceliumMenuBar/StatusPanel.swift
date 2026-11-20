//! The Tailscale-shaped dropdown: identity, live state, the two toggles worth
//! having in a menu, and a door to the browser for everything else.

import SwiftUI

struct StatusPanel: View {
    @EnvironmentObject private var model: MyceliumModel
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            Divider()
            if let error = model.lastError, !error.isEmpty {
                Text(error)
                    .font(.caption)
                    .foregroundStyle(.red)
                    .padding(.horizontal, 14)
                    .padding(.top, 8)
            }
            stateRows
            if !model.peers.isEmpty {
                Divider().padding(.top, 6)
                peersSection
            }
            Divider()
            actions
        }
        .frame(width: 320)
        .background(.regularMaterial)
        .onAppear { model.refresh() }
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in
            model.refresh()
        }
    }

    private var header: some View {
        HStack(alignment: .center, spacing: 10) {
            Circle()
                .fill(model.statusColor)
                .frame(width: 9, height: 9)
                .padding(.top, 2)
            VStack(alignment: .leading, spacing: 1) {
                Text(model.hostname)
                    .font(.system(size: 13, weight: .semibold, design: .rounded))
                Text(subtitle)
                    .font(.system(size: 10))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
            Spacer(minLength: 4)
            Button {
                model.refresh()
            } label: {
                Image(systemName: "arrow.clockwise")
                    .font(.system(size: 11, weight: .medium))
            }
            .buttonStyle(.plain)
            .help("Refresh")
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 11)
    }

    private var subtitle: String {
        if !model.daemonRunning { return model.daemonDetail.isEmpty ? "daemon not running" : model.daemonDetail }
        var parts: [String] = []
        if !model.site.isEmpty { parts.append(model.site) }
        parts.append(shortNodeID)
        return parts.joined(separator: " · ")
    }

    private var shortNodeID: String {
        guard model.nodeID.count > 10 else { return model.nodeID }
        return String(model.nodeID.prefix(10)) + "…"
    }

    private var stateRows: some View {
        VStack(spacing: 0) {
            stateRow(
                icon: "point.3.connected.trianglepath.dotted",
                label: "Overlay",
                value: model.tunnelOverlay,
                valueColor: model.tunnelRunning ? .primary : .secondary
            )
            stateRow(
                icon: "person.2",
                label: "Peers",
                value: "\(model.peers.count) enrolled"
            )
            stateRow(
                icon: "arrow.triangle.branch",
                label: "Frontdoor",
                value: model.frontdoorRunning
                    ? (model.frontdoorDetail.isEmpty ? "running" : model.frontdoorDetail)
                    : "not installed"
            )
            if model.syncEnabled {
                toggleRow(
                    icon: "globe",
                    label: "Public relays",
                    isOn: Binding(
                        get: { model.publicRelays },
                        set: { _ in model.togglePublicRelays() }
                    )
                )
            }
            toggleRow(
                icon: "power",
                label: model.daemonRunning ? "Daemon running" : "Daemon stopped",
                isOn: Binding(
                    get: { model.daemonRunning },
                    set: { _ in model.toggleDaemon() }
                )
            )
            if model.tunnelEnabled {
                HStack(spacing: 8) {
                    Image(systemName: "bolt.horizontal")
                        .frame(width: 14)
                        .foregroundStyle(.secondary)
                    Text(model.tunnelRunning ? "Tunnel running" : "Tunnel stopped")
                    Spacer()
                    Button("Restart") { model.restartTunnel() }
                        .buttonStyle(.borderless)
                        .font(.system(size: 11))
                        .disabled(model.busy)
                }
                .font(.system(size: 12))
                .padding(.horizontal, 14)
                .padding(.vertical, 6)
            }
        }
        .padding(.vertical, 2)
    }

    private func stateRow(icon: String, label: String, value: String, valueColor: Color = .secondary) -> some View {
        HStack(spacing: 8) {
            Image(systemName: icon)
                .frame(width: 14)
                .foregroundStyle(.secondary)
            Text(label)
            Spacer(minLength: 8)
            Text(value)
                .foregroundStyle(valueColor)
                .lineLimit(1)
                .truncationMode(.middle)
        }
        .font(.system(size: 12))
        .padding(.horizontal, 14)
        .padding(.vertical, 4)
    }

    private func toggleRow(icon: String, label: String, isOn: Binding<Bool>) -> some View {
        HStack(spacing: 8) {
            Image(systemName: icon)
                .frame(width: 14)
                .foregroundStyle(.secondary)
            Toggle(label, isOn: isOn)
                .toggleStyle(.switch)
                .controlSize(.mini)
                .font(.system(size: 12))
                .disabled(model.busy)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 3)
    }

    private var peersSection: some View {
        VStack(alignment: .leading, spacing: 3) {
            Text("FLEET")
                .font(.system(size: 9, weight: .semibold))
                .foregroundStyle(.tertiary)
                .padding(.horizontal, 14)
                .padding(.top, 8)
            ForEach(model.peers.prefix(5)) { peer in
                HStack(spacing: 7) {
                    Circle()
                        .fill(peer.lastSeenText == "never" ? Color.secondary.opacity(0.4) : Color.green)
                        .frame(width: 5, height: 5)
                    Text(peer.hostname)
                        .lineLimit(1)
                    Spacer()
                    Text(peer.site)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                    Text(peer.lastSeenText)
                        .foregroundStyle(.tertiary)
                        .frame(width: 46, alignment: .trailing)
                }
                .font(.system(size: 11))
                .padding(.horizontal, 14)
            }
            if model.peers.count > 5 {
                Text("+\(model.peers.count - 5) more in the browser")
                    .font(.system(size: 10))
                    .foregroundStyle(.tertiary)
                    .padding(.horizontal, 14)
                    .padding(.bottom, 4)
            }
        }
        .padding(.bottom, 4)
    }

    private var actions: some View {
        VStack(spacing: 2) {
            menuAction("Open Home", icon: "house") { MyceliumCLI.open("https://home.myc/") }
            menuAction("Open Store", icon: "bag") { MyceliumCLI.open("https://store.myc/") }
            menuAction("Spores…", icon: "square.grid.2x2") {
                NSApp.activate(ignoringOtherApps: true)
                openWindow(id: "spores")
            }
            Divider().padding(.vertical, 3)
            menuAction("Quit Mycelium", icon: "power") { NSApp.terminate(nil) }
        }
        .padding(.vertical, 5)
    }

    private func menuAction(_ title: String, icon: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack(spacing: 8) {
                Image(systemName: icon)
                    .frame(width: 14)
                Text(title)
                Spacer()
            }
            .font(.system(size: 12))
            .contentShape(Rectangle())
            .padding(.horizontal, 14)
            .padding(.vertical, 4)
        }
        .buttonStyle(.plain)
    }
}
