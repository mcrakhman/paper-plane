// swift-tools-version: 5.10
// The swift-tools-version declares the minimum version of Swift required to build this package.

import PackageDescription

let binaryTarget: Target = .binaryTarget(
    name: "ChatLibCore",
    path: "./chat-platform/target/ios/libchat-rs.xcframework"
)

let package = Package(
    name: "ChatLib",
    platforms: [
        .iOS(.v16),
    ],
    products: [
        .library(
            name: "ChatLib",
            targets: ["ChatLib"]
        ),
    ],
    targets: [
        binaryTarget,
        .target(
            name: "ChatLib",
            dependencies: [.target(name: "UniFFI")],
            path: "apple/Sources/ChatLib"
        ),
        .target(
            name: "UniFFI",
            dependencies: [.target(name: "ChatLibCore")],
            path: "apple/Sources/UniFFI"
        ),
        .testTarget(
            name: "ChatLibTests",
            dependencies: ["ChatLib"],
            path: "apple/Tests/ChatLibTests"
        ),
    ]
)
