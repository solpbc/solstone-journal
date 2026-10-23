// swift-tools-version: 6.2
// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
import PackageDescription

let package = Package(
    name: "mlx-helper",
    platforms: [.macOS(.v15)],
    products: [.executable(name: "mlx-helper", targets: ["mlx-helper"])],
    dependencies: [
        .package(url: "https://github.com/ml-explore/mlx-swift-lm.git",
                 revision: "48114eedf3c3d4a682a6b1d644cc5af5c2945d10", traits: []),
        .package(url: "https://github.com/ml-explore/mlx-swift.git", .upToNextMinor(from: "0.32.3")),
        .package(url: "https://github.com/huggingface/swift-transformers.git", from: "1.3.3"),
        .package(url: "https://github.com/hummingbird-project/hummingbird.git", from: "2.20.0"),
    ],
    targets: [
        .target(name: "HelperServer", dependencies: [
            .product(name: "Hummingbird", package: "hummingbird"),
        ]),
        .target(name: "GuidedGeneration", dependencies: [
            .product(name: "MLXGuidedGeneration", package: "mlx-swift-lm"),
            .product(name: "MLXLMCommon", package: "mlx-swift-lm"),
            .product(name: "MLX", package: "mlx-swift"),
        ], exclude: ["LICENSE"]),
        .testTarget(name: "GuidedGenerationTests", dependencies: [
            "GuidedGeneration", "HelperServer",
            .product(name: "HummingbirdTesting", package: "hummingbird"),
            .product(name: "MLXLMCommon", package: "mlx-swift-lm"),
            .product(name: "MLX", package: "mlx-swift"),
            .product(name: "MLXNN", package: "mlx-swift"),
        ]),
        .executableTarget(name: "mlx-helper", dependencies: [
            "GuidedGeneration",
            "HelperServer",
            .product(name: "Hummingbird", package: "hummingbird"),
            .product(name: "MLX", package: "mlx-swift"),
            .product(name: "MLXVLM", package: "mlx-swift-lm"),
            .product(name: "MLXLMCommon", package: "mlx-swift-lm"),
            .product(name: "MLXHuggingFace", package: "mlx-swift-lm"),
            .product(name: "Tokenizers", package: "swift-transformers"),
        ]),
        .testTarget(name: "HelperServerTests", dependencies: [
            "HelperServer",
            .product(name: "HummingbirdTesting", package: "hummingbird"),
        ]),
    ]
)
