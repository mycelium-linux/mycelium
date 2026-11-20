//! The menu bar surface. Status and toggles live here; everything deeper is a
//! browser page (`home.myc`) or the Spores window.

import SwiftUI

@main
struct MyceliumMenuBarApp: App {
    @StateObject private var model = MyceliumModel()

    var body: some Scene {
        MenuBarExtra {
            StatusPanel()
                .environmentObject(model)
                .onAppear { model.refresh() }
        } label: {
            Image(systemName: model.menuBarSymbol)
                .accessibilityLabel("Mycelium")
        }
        .menuBarExtraStyle(.window)

        Window("Spores", id: "spores") {
            SporesWindow()
                .environmentObject(model)
        }
        .defaultSize(width: 560, height: 460)
        .windowResizability(.contentMinSize)
    }
}
