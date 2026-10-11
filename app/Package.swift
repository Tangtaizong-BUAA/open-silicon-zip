// swift-tools-version: 6.0
import PackageDescription

// Swift 5 language mode is retained while actor-isolation migration is reviewed.
let package = Package(
    name: "FastZip",
    defaultLocalization: "en",
    platforms: [.macOS(.v13)],
    products: [
        .library(name: "FastZipKit", targets: ["FastZipKit"]),
        .executable(name: "FastZipApp", targets: ["FastZipApp"]),
        .library(name: "FinderSyncExtension", targets: ["FinderSyncExtension"]),
    ],
    targets: [
        .target(name: "FastZipKit", resources: [.process("Resources")]),
        .executableTarget(name: "FastZipApp", dependencies: ["FastZipKit"], resources: [.process("Resources")]),
        // Source for the Finder Sync app extension. A real .appex must be
        // produced by an Xcode "Finder Sync Extension" target that includes
        // these sources; see README.md.
        .target(name: "FinderSyncExtension", dependencies: ["FastZipKit"]),
        .testTarget(name: "FastZipKitTests", dependencies: ["FastZipKit"]),
    ],
    swiftLanguageModes: [.v5]
)
