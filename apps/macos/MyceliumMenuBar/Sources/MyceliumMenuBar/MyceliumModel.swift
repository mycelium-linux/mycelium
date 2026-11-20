//! Observable state for the panel. One refresh reads four small JSON documents
//! and never blocks the main thread.

import AppKit
import Foundation
import SwiftUI

struct PeerRow: Identifiable, Hashable {
    let id: String
    let hostname: String
    let site: String
    let lastSeen: TimeInterval
    let daemonVersion: String
    let ssh: Bool?

    var lastSeenText: String {
        guard lastSeen > 0 else { return "never" }
        let seconds = Date().timeIntervalSince1970 - lastSeen
        switch seconds {
        case ..<5: return "now"
        case ..<90: return "\(Int(seconds))s ago"
        case ..<5400: return "\(Int(seconds / 60))m ago"
        case ..<172800: return "\(Int(seconds / 3600))h ago"
        default: return "\(Int(seconds / 86400))d ago"
        }
    }
}

struct Spore: Identifiable, Hashable {
    let id: String
    let name: String
    let title: String
    let path: URL
    let hosts: [String]

    var openURL: String? {
        guard let host = hosts.first, !host.isEmpty else { return nil }
        return "https://\(host)"
    }

    var displayTitle: String { title.isEmpty ? name : title }
}

@MainActor
final class MyceliumModel: ObservableObject {
    @Published var hostname = "Mycelium"
    @Published var site = ""
    @Published var nodeID = ""
    @Published var daemonRunning = false
    @Published var daemonDetail = ""
    @Published var peers: [PeerRow] = []
    @Published var tunnelEnabled = false
    @Published var tunnelRunning = false
    @Published var tunnelPublishing = false
    @Published var tunnelOverlay = "—"
    @Published var frontdoorRunning = false
    @Published var frontdoorDetail = ""
    @Published var syncEnabled = false
    @Published var publicRelays = false
    @Published var spores: [Spore] = []
    @Published var busy = false
    @Published var lastError: String?
    @Published var lastRefresh = Date.distantPast

    private var refreshing = false
    private var poll: Task<Void, Never>?

    init() {
        // The menu bar label is visible before the panel is ever opened, so the
        // first read cannot wait for `.onAppear`. Poll slowly: the daemon is the
        // source of truth, and a menu item is not a status stream.
        refresh()
        poll = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 20 * 1_000_000_000)
                await self?.refresh()
            }
        }
    }

    deinit { poll?.cancel() }

    var menuBarSymbol: String {
        if !daemonRunning { return "network.slash" }
        if tunnelRunning { return "network" }
        return "network"
    }

    var statusColor: Color {
        if !daemonRunning { return .secondary }
        if !tunnelEnabled { return .orange }
        return tunnelRunning ? .green : .yellow
    }

    func refresh() {
        if refreshing { return }
        refreshing = true
        Task {
            defer { refreshing = false }
            async let node = MyceliumCLI.run(["node", "status", "--json"])
            async let fleet = MyceliumCLI.run(["fleet", "status", "--json"])
            async let tunnel = MyceliumCLI.run(["iroh-tun", "status", "--json"])
            async let front = MyceliumCLI.run(["frontdoor", "status", "--json"])
            async let selfCheck = MyceliumCLI.run(["_self-check"])

            let (nodeResult, fleetResult, tunnelResult, frontResult, checkResult) =
                await (node, fleet, tunnel, front, selfCheck)

            applyNode(nodeResult)
            applyFleet(fleetResult)
            applyTunnel(tunnelResult)
            applyFrontdoor(frontResult)
            applySelfCheck(checkResult)
            loadSpores()
            lastRefresh = Date()
        }
    }

    // MARK: - Reads

    private func applyNode(_ result: CommandResult) {
        guard result.ok, let root = result.json as? [String: Any],
              let node = root["node"] as? [String: Any],
              let hello = node["hello"] as? [String: Any] else {
            daemonRunning = false
            daemonDetail = result.stderr.isEmpty ? "daemon is not running" : result.stderr
            return
        }
        daemonRunning = true
        daemonDetail = ""
        hostname = hello["hostname"] as? String ?? "Mycelium"
        site = hello["site"] as? String ?? ""
        nodeID = hello["node_id"] as? String ?? ""
    }

    private func applyFleet(_ result: CommandResult) {
        guard result.ok, let rows = result.json as? [[String: Any]] else {
            peers = []
            return
        }
        // `fleet status --json` is already the flat projection this panel
        // needs, so the same code also accepts a raw `peers --json` row when
        // an older binary answers.
        peers = rows.map { row in
            let hello = row["hello"] as? [String: Any] ?? row
            let health = row["health"] as? [String: Any] ?? [:]
            let id = hello["node_id"] as? String ?? UUID().uuidString
            return PeerRow(
                id: id,
                hostname: hello["hostname"] as? String ?? "unknown",
                site: hello["site"] as? String ?? "",
                lastSeen: (hello["last_seen"] as? NSNumber)?.doubleValue
                    ?? (row["last_seen"] as? NSNumber)?.doubleValue ?? 0,
                daemonVersion: hello["daemon_version"] as? String ?? "?",
                ssh: (row["ssh_listening"] as? Bool) ?? (health["ssh_listening"] as? Bool)
            )
        }
        .sorted { $0.hostname.localizedCaseInsensitiveCompare($1.hostname) == .orderedAscending }
    }

    private func applyTunnel(_ result: CommandResult) {
        guard result.ok, let value = result.json as? [String: Any] else {
            tunnelEnabled = false
            tunnelRunning = false
            tunnelPublishing = false
            tunnelOverlay = "not configured"
            return
        }
        tunnelEnabled = value["enabled"] as? Bool ?? false
        tunnelRunning = value["running"] as? Bool ?? false
        tunnelPublishing = value["publishing"] as? Bool ?? false
        if let overlay = value["overlay"] as? [String], !overlay.isEmpty {
            tunnelOverlay = overlay.joined(separator: ", ")
        } else {
            tunnelOverlay = tunnelEnabled ? "no overlay" : "not configured"
        }
        if let error = value["config_error"] as? String, !error.isEmpty {
            tunnelOverlay = error
        }
    }

    private func applyFrontdoor(_ result: CommandResult) {
        guard result.ok, let value = result.json as? [String: Any] else {
            // Older installs call this `gateway`.
            frontdoorRunning = false
            frontdoorDetail = ""
            return
        }
        let installed = value["installed"] as? Bool
            ?? value["running"] as? Bool
            ?? value["listen"] != nil
        frontdoorRunning = installed
        frontdoorDetail = value["listen"] as? String ?? ""
    }

    private func applySelfCheck(_ result: CommandResult) {
        let text = result.stdout
        syncEnabled = text.contains("iroh-sync=true")
        if !syncEnabled { publicRelays = false }
    }

    // MARK: - Writes

    func toggleDaemon() {
        perform(busyLabel: daemonRunning ? "Stopping daemon…" : "Starting daemon…") {
            if self.daemonRunning {
                _ = await MyceliumCLI.run(["daemon", "stop"])
            } else {
                _ = await MyceliumCLI.run(["daemon", "start"])
            }
        }
    }

    func restartTunnel() {
        perform(busyLabel: "Restarting tunnel…") {
            _ = await MyceliumCLI.run(["iroh-tun", "restart", "--write"])
        }
    }

    func togglePublicRelays() {
        let enable = !publicRelays
        perform(busyLabel: enable ? "Enabling relays…" : "Disabling relays…") {
            _ = await MyceliumCLI.run(["sync", "relays", enable ? "enable" : "disable", "--write"])
        }
    }

    private func perform(busyLabel: String, _ work: @escaping () async -> Void) {
        busy = true
        lastError = nil
        Task {
            await work()
            busy = false
            refresh()
        }
    }

    // MARK: - Spores

    private func loadSpores() {
        let home = FileManager.default.homeDirectoryForCurrentUser
        let roots = [
            home.appendingPathComponent("Library/Application Support/fungOS/spores"),
            home.appendingPathComponent(".local/share/fungos/spores"),
        ]
        var found: [String: Spore] = [:]
        for root in roots {
            guard let children = try? FileManager.default.contentsOfDirectory(
                at: root, includingPropertiesForKeys: [.isDirectoryKey]
            ) else { continue }
            for child in children {
                let manifest = child.appendingPathComponent("spore.toml")
                guard FileManager.default.fileExists(atPath: manifest.path),
                      let spore = Self.parseSpore(at: child, manifest: manifest) else { continue }
                found[spore.id] = spore
            }
        }
        spores = found.values.sorted { $0.displayTitle.localizedCaseInsensitiveCompare($1.displayTitle) == .orderedAscending }
    }

    /// A deliberately small TOML reader: the Spore manifest is a flat table plus
    /// a `[http] hosts = [...]` list, and adding a TOML dependency for five keys
    /// would be the larger program.
    static func parseSpore(at directory: URL, manifest: URL) -> Spore? {
        guard let text = try? String(contentsOf: manifest, encoding: .utf8) else { return nil }
        var section = ""
        var values: [String: String] = [:]
        var hosts: [String] = []
        for rawLine in text.split(separator: "\n", omittingEmptySubsequences: false) {
            let line = rawLine.trimmingCharacters(in: .whitespaces)
            if line.isEmpty || line.hasPrefix("#") { continue }
            if line.hasPrefix("[") && line.hasSuffix("]") {
                section = String(line.dropFirst().dropLast())
                continue
            }
            guard let equals = line.firstIndex(of: "=") else { continue }
            let key = line[..<equals].trimmingCharacters(in: .whitespaces)
            var value = line[line.index(after: equals)...].trimmingCharacters(in: .whitespaces)
            if value.hasPrefix("\"") && value.hasSuffix("\"") && value.count >= 2 {
                value = String(value.dropFirst().dropLast())
            }
            switch (section, key) {
            case ("spore", "id"): values["id"] = value
            case ("spore", "name"): values["name"] = value
            case ("spore", "title"): values["title"] = value
            case ("http", "hosts"):
                hosts = value
                    .trimmingCharacters(in: CharacterSet(charactersIn: "[]"))
                    .split(separator: ",")
                    .map { $0.trimmingCharacters(in: .whitespaces).trimmingCharacters(in: CharacterSet(charactersIn: "\"")) }
                    .filter { !$0.isEmpty }
            default:
                break
            }
        }
        guard let id = values["id"], !id.isEmpty else { return nil }
        return Spore(
            id: id,
            name: values["name"] ?? id,
            title: values["title"] ?? "",
            path: directory,
            hosts: hosts
        )
    }

    func reveal(_ spore: Spore) {
        NSWorkspace.shared.activateFileViewerSelecting([spore.path])
    }

    func open(_ spore: Spore) {
        if let url = spore.openURL {
            MyceliumCLI.open(url)
        } else {
            MyceliumCLI.open("https://home.myc/")
        }
    }
}
