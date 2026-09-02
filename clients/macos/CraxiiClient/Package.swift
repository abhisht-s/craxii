// swift-tools-version: 6.0

import PackageDescription

let package = Package(
    name: "CraxiiClient",
    platforms: [.macOS(.v14)],
    products: [
        .library(name: "CraxiiProtocol", targets: ["CraxiiProtocol"]),
        .library(name: "CraxiiClientCore", targets: ["CraxiiClientCore"]),
        .library(name: "CraxiiAppleAdapters", targets: ["CraxiiAppleAdapters"]),
        .executable(name: "CraxiiIntegrationProbe", targets: ["CraxiiIntegrationProbe"]),
    ],
    dependencies: [],
    targets: [
        .target(name: "CraxiiProtocol"),
        .target(name: "CraxiiClientCore", dependencies: ["CraxiiProtocol"]),
        .target(
            name: "CraxiiAppleAdapters",
            dependencies: ["CraxiiProtocol", "CraxiiClientCore"],
            linkerSettings: [.linkedFramework("Security"), .linkedFramework("Network")]
        ),
        .executableTarget(
            name: "CraxiiIntegrationProbe",
            dependencies: ["CraxiiProtocol", "CraxiiClientCore", "CraxiiAppleAdapters"]
        ),
        .testTarget(name: "CraxiiProtocolTests", dependencies: ["CraxiiProtocol"]),
        .testTarget(
            name: "CraxiiClientCoreTests",
            dependencies: ["CraxiiProtocol", "CraxiiClientCore"]
        ),
        .testTarget(
            name: "CraxiiAppleAdaptersTests",
            dependencies: ["CraxiiProtocol", "CraxiiClientCore", "CraxiiAppleAdapters"]
        ),
    ],
    swiftLanguageModes: [.v6]
)
