// swift-tools-version: 6.0
import PackageDescription

// Swift 5 language mode is retained while actor-isolation migration is reviewed.
let package = Package(
    name: "FastZip",
    defaultLocalization: "en",
    platforms: [.macOS(.v15)],
    products: [
        .library(name: "FastZipKit", targets: ["FastZipKit"]),
        .executable(name: "FastZipApp", targets: ["FastZipApp"]),
        .library(name: "FinderSyncExtension", targets: ["FinderSyncExtension"]),
    ],
    dependencies: [.package(url: "https://github.com/sparkle-project/Sparkle", exact: "2.10.0")],
    targets: [
        .target(name: "FastZipKit", resources: [.process("Resources")]),
        .executableTarget(name: "FastZipApp", dependencies: ["FastZipKit", .product(name: "Sparkle", package: "Sparkle")], resources: [.process("Resources")]),
        // Source for the Finder Sync app extension. A real .appex must be
        // produced by an Xcode "Finder Sync Extension" target that includes
        // these sources; see README.md.
        .target(name: "FinderSyncExtension", dependencies: ["FastZipKit"]),
        .testTarget(name: "FastZipKitTests", dependencies: ["FastZipKit"]),
        .testTarget(name: "FastZipAppTests", dependencies: ["FastZipApp", "FastZipKit"]),
    ],
    swiftLanguageModes: [.v5]
)
