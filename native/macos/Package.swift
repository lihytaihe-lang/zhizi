// swift-tools-version: 6.2
import Foundation
import PackageDescription

let packageDirectory = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
let repositoryRoot = packageDirectory.deletingLastPathComponent().deletingLastPathComponent()
let coreLibraryDirectory = ProcessInfo.processInfo.environment["AIM_CORE_LIBRARY_DIR"]
    ?? repositoryRoot.appendingPathComponent("native/core/target/release").path
let rimeRoot = repositoryRoot.appendingPathComponent(".build-deps/librime/1.16.1")

let package = Package(
    name: "AgentInputMethodNative",
    platforms: [.macOS(.v14)],
    products: [
        .executable(name: "AgentIMEContextModelHarness", targets: ["AgentIMEContextModelHarness"]),
        .executable(name: "AgentInputMethod", targets: ["AgentInputMethodHost"]),
        .executable(name: "AgentIMESecureHarness", targets: ["AgentIMESecureHarness"]),
        .executable(name: "AgentIMECoreHarness", targets: ["AgentIMECoreHarness"]),
        .executable(name: "AgentIMEBehaviorHarness", targets: ["AgentIMEBehaviorHarness"]),
        .executable(name: "AgentIMEReplayHarness", targets: ["AgentIMEReplayHarness"]),
        .executable(name: "AgentIMEPersonalReplayHarness", targets: ["AgentIMEPersonalReplayHarness"]),
        .executable(name: "AgentIMEFuzzyHarness", targets: ["AgentIMEFuzzyHarness"]),
        .executable(name: "AgentIMEUserDBHarness", targets: ["AgentIMEUserDBHarness"]),
        .executable(name: "AgentIMEUserDBCtl", targets: ["AgentIMEUserDBCtl"]),
        .executable(name: "AgentIMEPerformanceHarness", targets: ["AgentIMEPerformanceHarness"]),
        .executable(name: "AgentIMESnapshotHarness", targets: ["AgentIMESnapshotHarness"]),
        .executable(name: "AgentIMECandidateHarness", targets: ["AgentIMECandidateHarness"]),
    ],
    targets: [
        .executableTarget(name: "AgentIMEContextModelHarness", dependencies: ["AgentInputMethodHostKit"]),
        .target(
            name: "AgentIMECoreBridge",
            path: "Sources/AgentIMECoreBridge",
            publicHeadersPath: "include",
            linkerSettings: [
                .unsafeFlags(["-L", coreLibraryDirectory]),
                .linkedLibrary("agent_ime_core"),
            ]
        ),
        .target(
            name: "RimeShim",
            path: "Sources/RimeShim",
            publicHeadersPath: "include",
            cSettings: [.unsafeFlags(["-I", rimeRoot.appendingPathComponent("include").path])],
            linkerSettings: [
                .unsafeFlags(["-L", rimeRoot.appendingPathComponent("lib").path]),
                .linkedLibrary("rime"),
            ]
        ),
        .target(
            name: "AgentIMEUserDBTestSupport",
            path: "Sources/AgentIMEUserDBTestSupport",
            publicHeadersPath: "include",
            cSettings: [.unsafeFlags(["-I", rimeRoot.appendingPathComponent("include").path])],
            linkerSettings: [
                .unsafeFlags(["-L", rimeRoot.appendingPathComponent("lib").path]),
                .linkedLibrary("rime"),
            ]
        ),
        .target(
            name: "AgentInputMethodHostKit",
            dependencies: ["AgentIMECoreBridge", "RimeShim"],
            path: "Sources/AgentInputMethodHostKit",
            linkerSettings: [
                .linkedFramework("AppKit"),
                .linkedFramework("InputMethodKit"),
                .linkedFramework("Carbon"),
                .linkedFramework("Security"),
                .linkedFramework("CoreML"),
                .unsafeFlags(["-Xlinker", "-rpath", "-Xlinker", "@executable_path/../Frameworks"]),
            ]
        ),
        .executableTarget(
            name: "AgentInputMethodHost",
            dependencies: ["AgentInputMethodHostKit"],
            path: "Sources/AgentInputMethodHost"
        ),
        .executableTarget(
            name: "AgentIMESecureHarness",
            path: "Sources/AgentIMESecureHarness",
            linkerSettings: [.linkedFramework("AppKit")]
        ),
        .executableTarget(
            name: "AgentIMECoreHarness",
            dependencies: ["AgentInputMethodHostKit"],
            path: "Sources/AgentIMECoreHarness"
        ),
        .executableTarget(
            name: "AgentIMEBehaviorHarness",
            dependencies: ["AgentInputMethodHostKit", "AgentIMECoreBridge"],
            path: "Sources/AgentIMEBehaviorHarness"
        ),
        .executableTarget(
            name: "AgentIMEReplayHarness",
            dependencies: ["AgentInputMethodHostKit"],
            path: "Sources/AgentIMEReplayHarness"
        ),
        .executableTarget(
            name: "AgentIMEPersonalReplayHarness",
            dependencies: ["AgentInputMethodHostKit"],
            path: "Sources/AgentIMEPersonalReplayHarness"
        ),
        .executableTarget(
            name: "AgentIMEFuzzyHarness",
            dependencies: ["AgentInputMethodHostKit"],
            path: "Sources/AgentIMEFuzzyHarness"
        ),
        .executableTarget(
            name: "AgentIMEUserDBHarness",
            dependencies: ["AgentInputMethodHostKit", "AgentIMEUserDBTestSupport"],
            path: "Sources/AgentIMEUserDBHarness"
        ),
        .executableTarget(
            name: "AgentIMEUserDBCtl",
            dependencies: ["AgentInputMethodHostKit", "RimeShim"],
            path: "Sources/AgentIMEUserDBCtl"
        ),
        .executableTarget(
            name: "AgentIMEPerformanceHarness",
            dependencies: ["AgentInputMethodHostKit"],
            path: "Sources/AgentIMEPerformanceHarness"
        ),
        .executableTarget(
            name: "AgentIMESnapshotHarness",
            dependencies: ["AgentInputMethodHostKit"],
            path: "Sources/AgentIMESnapshotHarness"
        ),
        .executableTarget(
            name: "AgentIMECandidateHarness",
            dependencies: ["AgentInputMethodHostKit"],
            path: "Sources/AgentIMECandidateHarness"
        ),
        .testTarget(
            name: "AgentInputMethodHostKitTests",
            dependencies: ["AgentInputMethodHostKit"],
            path: "Tests/AgentInputMethodHostKitTests"
        ),
    ]
)
