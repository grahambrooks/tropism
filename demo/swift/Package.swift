// swift-tools-version:5.9
//
// A manifest that is a program. tropism parses it with the Swift grammar and never
// runs it — see crates/tropism-lang/src/swift.rs.

import PackageDescription

let package = Package(
    name: "Shop",
    products: [
        .library(name: "ShopCore", targets: ["ShopCore"]),
        .executable(name: "shop", targets: ["ShopCLI"]),
    ],
    dependencies: [
        .package(url: "https://github.com/apple/swift-log.git", from: "1.5.4"),
        // PLANTED: declared, and no target takes a product from it.
        .package(url: "https://github.com/apple/swift-collections.git", from: "1.1.0"),
    ],
    targets: [
        .target(
            name: "ShopCore",
            dependencies: [
                // The mapping no other ecosystem supplies: `import Logging` comes
                // from the swift-log package, and the manifest says so outright.
                .product(name: "Logging", package: "swift-log"),
            ]
        ),
        .target(name: "ShopStore", dependencies: ["ShopCore"]),
        .executableTarget(name: "ShopCLI", dependencies: ["ShopCore", "ShopStore"]),
        .testTarget(name: "ShopCoreTests", dependencies: ["ShopCore"]),
    ]
)
