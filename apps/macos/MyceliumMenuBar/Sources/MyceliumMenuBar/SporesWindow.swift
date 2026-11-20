//! The Spore window: what is installed on this Mac, and one click to open it.
//! Installing, permissions and updates belong to the browser store.

import SwiftUI

struct SporesWindow: View {
    @EnvironmentObject private var model: MyceliumModel
    @State private var query = ""

    private var filtered: [Spore] {
        guard !query.isEmpty else { return model.spores }
        return model.spores.filter {
            $0.displayTitle.localizedCaseInsensitiveContains(query)
                || $0.id.localizedCaseInsensitiveContains(query)
        }
    }

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider()
            if filtered.isEmpty {
                VStack(spacing: 8) {
                    Image(systemName: "square.grid.2x2")
                        .font(.system(size: 28))
                        .foregroundStyle(.tertiary)
                    Text(model.spores.isEmpty ? "No Spores installed yet." : "No Spore matches “\(query)”.")
                        .foregroundStyle(.secondary)
                    Button("Open the Store") { MyceliumCLI.open("https://store.myc/") }
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .padding(30)
            } else {
                List(filtered) { spore in
                    SporeRow(spore: spore)
                        .environmentObject(model)
                }
                .listStyle(.inset)
            }
        }
        .frame(minWidth: 480, minHeight: 380)
        .onAppear { model.refresh() }
    }

    private var header: some View {
        HStack(spacing: 10) {
            VStack(alignment: .leading, spacing: 1) {
                Text("Spores")
                    .font(.system(size: 15, weight: .semibold, design: .rounded))
                Text("\(model.spores.count) installed on \(model.hostname)")
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
            }
            Spacer()
            TextField("Filter", text: $query)
                .textFieldStyle(.roundedBorder)
                .frame(width: 170)
            Button {
                MyceliumCLI.open("https://store.myc/")
            } label: {
                Label("Store", systemImage: "bag")
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
    }
}

private struct SporeRow: View {
    let spore: Spore
    @EnvironmentObject private var model: MyceliumModel

    var body: some View {
        HStack(spacing: 10) {
            RoundedRectangle(cornerRadius: 7, style: .continuous)
                .fill(Color.accentColor.opacity(0.12))
                .frame(width: 30, height: 30)
                .overlay(
                    Image(systemName: "square.stack.3d.up")
                        .font(.system(size: 13))
                        .foregroundStyle(Color.accentColor)
                )
            VStack(alignment: .leading, spacing: 1) {
                Text(spore.displayTitle)
                    .font(.system(size: 12, weight: .medium))
                Text(spore.id)
                    .font(.system(size: 10, design: .monospaced))
                    .foregroundStyle(.secondary)
            }
            Spacer()
            if let host = spore.hosts.first {
                Text(host)
                    .font(.system(size: 10, design: .monospaced))
                    .foregroundStyle(.tertiary)
            }
            Button("Reveal") { model.reveal(spore) }
                .controlSize(.small)
            Button(spore.openURL == nil ? "Open Home" : "Open") { model.open(spore) }
                .controlSize(.small)
                .buttonStyle(.borderedProminent)
        }
        .padding(.vertical, 3)
    }
}
