//! Runs the `mycelium` CLI and decodes its JSON.
//!
//! The daemon owns the truth; the menu bar is a client. Every read is a
//! `--json` call on the same binary the terminal uses, so the panel and the
//! shell can never disagree about the shape of the state.

import AppKit
import Foundation

struct CommandResult {
    let status: Int32
    let stdout: String
    let stderr: String

    var ok: Bool { status == 0 }

    var json: Any? {
        guard let data = stdout.data(using: .utf8), !data.isEmpty else { return nil }
        return try? JSONSerialization.jsonObject(with: data)
    }
}

enum MyceliumCLI {
    /// The binary the app should call. A packaged app cannot assume PATH, so
    /// the installer's location wins over a developer checkout.
    static func binary() -> String? {
        if let override = ProcessInfo.processInfo.environment["MYCELIUM_BIN"],
           FileManager.default.isExecutableFile(atPath: override) {
            return override
        }
        let home = FileManager.default.homeDirectoryForCurrentUser.path
        let candidates = [
            "/usr/local/bin/mycelium",
            "/opt/homebrew/bin/mycelium",
            home + "/.local/bin/mycelium",
            home + "/bin/mycelium",
        ]
        if let found = candidates.first(where: { FileManager.default.isExecutableFile(atPath: $0) }) {
            return found
        }
        return nil
    }

    static func run(_ arguments: [String], timeout: TimeInterval = 8) async -> CommandResult {
        guard let binary = binary() else {
            return CommandResult(status: 127, stdout: "", stderr: "mycelium is not installed")
        }
        return await run(binary: binary, arguments, timeout: timeout)
    }

    static func run(binary: String, _ arguments: [String], timeout: TimeInterval = 8) async -> CommandResult {
        await withCheckedContinuation { continuation in
            let process = Process()
            process.executableURL = URL(fileURLWithPath: binary)
            process.arguments = arguments

            let out = Pipe()
            let err = Pipe()
            process.standardOutput = out
            process.standardError = err

            // A daemon that is not answering must not leave the panel spinning
            // forever; terminate the child and report the timeout as an error.
            let deadline = DispatchWorkItem {
                if process.isRunning { process.terminate() }
            }
            DispatchQueue.global().asyncAfter(deadline: .now() + timeout, execute: deadline)

            do {
                try process.run()
            } catch {
                deadline.cancel()
                continuation.resume(
                    returning: CommandResult(status: 127, stdout: "", stderr: error.localizedDescription)
                )
                return
            }
            process.terminationHandler = { finished in
                deadline.cancel()
                let stdout = String(data: out.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) ?? ""
                let stderr = String(data: err.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) ?? ""
                continuation.resume(
                    returning: CommandResult(status: finished.terminationStatus, stdout: stdout, stderr: stderr)
                )
            }
        }
    }

    static func open(_ url: String) {
        guard let target = URL(string: url) else { return }
        NSWorkspace.shared.open(target)
    }
}
