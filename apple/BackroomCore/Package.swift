// swift-tools-version: 5.9
// Swift wrapper around the Rust core. Run scripts/build-apple.sh first: it
// generates BackroomFFI.xcframework and Sources/BackroomCore/backroom_ffi.swift.
import PackageDescription

let package = Package(
    name: "BackroomCore",
    platforms: [.macOS(.v14)],
    products: [
        .library(name: "BackroomCore", targets: ["BackroomCore"]),
    ],
    targets: [
        .binaryTarget(name: "BackroomFFI", path: "BackroomFFI.xcframework"),
        .target(
            name: "BackroomCore",
            dependencies: ["BackroomFFI"],
            linkerSettings: [.linkedFramework("CoreFoundation"), .linkedFramework("Security")]
        ),
        .testTarget(name: "BackroomCoreTests", dependencies: ["BackroomCore"]),
    ]
)
