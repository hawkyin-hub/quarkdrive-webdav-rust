// swift-tools-version:5.9
// Stage 13b.1: Privileged helper for com.localquark.webdav.
//
// Build:    helper/build.sh
// Install:  bin/install-helper.sh
// Usage (server mode, invoked by launchd):
//     LocalQuarkHelper serve
// Usage (client mode, invoked by bin/helper-client.sh):
//     LocalQuarkHelper client mount <url> <mountPoint>
//     LocalQuarkHelper client unmount <mountPoint>
//     LocalQuarkHelper client mkdir <path>
//     LocalQuarkHelper client version

import PackageDescription

let package = Package(
    name: "LocalQuarkHelper",
    platforms: [
        .macOS(.v13)
    ],
    products: [
        .executable(name: "LocalQuarkHelper", targets: ["LocalQuarkHelper"])
    ],
    targets: [
        .executableTarget(
            name: "LocalQuarkHelper",
            path: "Sources/LocalQuarkHelper"
        )
    ]
)
