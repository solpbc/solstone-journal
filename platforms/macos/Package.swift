// swift-tools-version: 6.1
// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import PackageDescription

// The Journal app for macOS: a Swift lifecycle and window adapter around the native journal
// runtime this repository builds. The shared Mac libraries (core types, the journal mark,
// setup chrome, updates and the watchdog) come from solstone-macos at one exact commit; bump the revision
// and Package.resolved together, never to a branch or tag.
let supportRevision = "07a9bc0644f973e6e1b616d7e77b8e821366919d"

let package = Package(
    name: "SolstoneJournalMac",
    platforms: [
        .macOS(.v15)
    ],
    products: [
        .executable(name: "journal", targets: ["journal"]),
        // Installed as Contents/MacOS/solstone-watchdog; the target name differs only so it
        // cannot collide with the capture app's watchdog target in the dependency.
        .executable(name: "solstone-watchdog", targets: ["JournalWatchdogMain"]),
        .executable(name: "journal-icon-gen", targets: ["journal-icon-gen"]),
        // Test-only: a real runtime under the real supervisor for the capture app's door test.
        .executable(name: "journal-integration-fixture", targets: ["journal-integration-fixture"]),
    ],
    dependencies: [
        .package(url: "https://github.com/solpbc/solstone-macos.git", revision: supportRevision),
    ],
    targets: [
        .target(
            name: "JournalRuntime",
            dependencies: [
                .product(name: "SolstoneCore", package: "solstone-macos")
            ],
            path: "Sources/JournalRuntime",
            swiftSettings: [
                .swiftLanguageMode(.v6)
            ]
        ),
        .executableTarget(
            name: "journal-icon-gen",
            dependencies: [
                .product(name: "JournalMarkKit", package: "solstone-macos")
            ],
            path: "Sources/journal-icon-gen",
            swiftSettings: [
                .swiftLanguageMode(.v6)
            ]
        ),
        .executableTarget(
            name: "journal",
            dependencies: [
                .product(name: "SolstoneCore", package: "solstone-macos"),
                .target(name: "JournalRuntime"),
                .product(name: "JournalMarkKit", package: "solstone-macos"),
                .product(name: "SetupChrome", package: "solstone-macos"),
                .product(name: "UpdateKit", package: "solstone-macos"),
                .product(name: "Sparkle", package: "solstone-macos")
            ],
            path: "Sources/journal",
            exclude: [
                "Info.plist",
                "entitlements.plist",
                "app.solstone.journal.watchdog.plist"
            ],
            resources: [
                .copy("Resources")
            ],
            swiftSettings: [
                .swiftLanguageMode(.v6)
            ],
            linkerSettings: [
                .linkedFramework("AppKit"),
                .linkedFramework("ServiceManagement")
            ]
        ),
        .executableTarget(
            name: "JournalWatchdogMain",
            dependencies: [
                .product(name: "WatchdogKit", package: "solstone-macos")
            ],
            path: "Sources/JournalWatchdogMain",
            swiftSettings: [
                .swiftLanguageMode(.v6)
            ]
        ),
        .target(
            name: "JournalRuntimeTestSupport",
            dependencies: [
                .target(name: "JournalRuntime"),
                .product(name: "JournalMarkKit", package: "solstone-macos"),
                .product(name: "SolstoneCore", package: "solstone-macos")
            ],
            path: "Tests/JournalRuntimeTestSupport",
            swiftSettings: [
                .swiftLanguageMode(.v6)
            ]
        ),
        .executableTarget(
            name: "journal-integration-fixture",
            dependencies: [
                .target(name: "JournalRuntime"),
                .target(name: "JournalRuntimeTestSupport")
            ],
            path: "Tests/NativeIntegrationFixture",
            swiftSettings: [
                .swiftLanguageMode(.v6)
            ]
        ),
        .testTarget(
            name: "JournalRuntimeTests",
            dependencies: [
                .target(name: "JournalRuntime"),
                .target(name: "JournalRuntimeTestSupport"),
                .product(name: "MacTestSupport", package: "solstone-macos"),
                .product(name: "SolstoneCore", package: "solstone-macos")
            ],
            path: "Tests/JournalRuntimeTests",
            resources: [
                .copy("Fixtures")
            ],
            swiftSettings: [
                .swiftLanguageMode(.v6)
            ]
        ),
        .testTarget(
            name: "journalTests",
            dependencies: [
                .target(name: "journal"),
                .product(name: "JournalMarkKit", package: "solstone-macos"),
                .target(name: "JournalRuntime"),
                .target(name: "JournalRuntimeTestSupport"),
                .product(name: "MacTestSupport", package: "solstone-macos"),
                .product(name: "SetupChrome", package: "solstone-macos"),
                .product(name: "SolstoneCore", package: "solstone-macos"),
                .product(name: "UpdateKit", package: "solstone-macos")
            ],
            path: "Tests/journalTests",
            swiftSettings: [
                .swiftLanguageMode(.v6)
            ]
        ),
    ]
)
