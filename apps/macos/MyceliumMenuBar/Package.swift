// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "MyceliumMenuBar",
    platforms: [.macOS(.v14)],
    targets: [
        .executableTarget(
            name: "MyceliumMenuBar",
            path: "Sources/MyceliumMenuBar"
        )
    ]
)
