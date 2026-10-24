// swift-tools-version:5.9
//
// The Safari side of the browser link, as a package: the relay the Safari
// extension's native handler is made of (`ConnectRelay`), its tests, and a
// test host that runs the same relay outside Safari
// (`connect-relay-harness`, driven by `connectd selftest safari`).
//
// Tools version 5.9 on purpose: the sources are Swift 5 (no strict
// concurrency checking), which is also what the extension is built as.
//
// The extension itself (the .appex Safari loads) is built by
// `scripts/build-safari-appex.sh` from `Extension/` and the same
// `Sources/ConnectRelay` files.

import PackageDescription

let package = Package(
    name: "ConnectSafari",
    platforms: [.macOS(.v12)],
    products: [
        .library(name: "ConnectRelay", targets: ["ConnectRelay"]),
        .executable(name: "connect-relay-harness", targets: ["connect-relay-harness"]),
    ],
    targets: [
        .target(name: "ConnectRelay"),
        .executableTarget(
            name: "connect-relay-harness",
            dependencies: ["ConnectRelay"]
        ),
        .testTarget(
            name: "ConnectRelayTests",
            dependencies: ["ConnectRelay"]
        ),
    ]
)
