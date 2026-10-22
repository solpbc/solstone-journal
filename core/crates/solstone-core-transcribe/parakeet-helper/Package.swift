// swift-tools-version: 6.2
import PackageDescription

let package = Package(
    name: "parakeet-helper",
    platforms: [.macOS(.v14), .iOS("26.0")],
    products: [
        .executable(name: "parakeet-helper", targets: ["parakeet-helper"])
    ],
    dependencies: [
        .package(
            url: "https://github.com/quartzjer/FluidAudio.git",
            revision: "03e48b07ab136062987f157b6ed84ebc8f5eb204",
            traits: []
        )
    ],
    targets: [
        .target(
            name: "InstalledModels",
            dependencies: [.product(name: "FluidAudio", package: "FluidAudio")]
        ),
        .executableTarget(
            name: "parakeet-helper",
            dependencies: [
                .product(name: "FluidAudio", package: "FluidAudio"),
                "InstalledModels"
            ]
        ),
        .testTarget(
            name: "parakeet-helperTests",
            dependencies: ["InstalledModels"]
        )
    ],
    swiftLanguageModes: [.v5]
)
