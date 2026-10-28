// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import Foundation

public enum NativeJournalRuntimeMaterializerError: LocalizedError, Sendable, Equatable {
    case missingRuntimeRoot(String)
    case missingSolstoneExecutable(String)
    case invalidBundleVersion

    public var errorDescription: String? {
        switch self {
        case .missingRuntimeRoot(let path):
            return "bundled native journal runtime is missing at \(path)"
        case .missingSolstoneExecutable(let path):
            return "bundled native solstone executable is missing at \(path)"
        case .invalidBundleVersion:
            return "journal.app is missing a valid release identity"
        }
    }
}

/// Resolves the app-carried Rust tree without materializing, mutating, or selecting
/// an external runtime. The app owns only this in-bundle path.
public final class NativeJournalRuntimeMaterializer: RuntimeMaterializing, @unchecked Sendable {
    private let bundleURL: URL
    private let baseEnvironment: [String: String]
    private let fileManager: FileManager

    public init(
        bundleURL: URL = Bundle.main.bundleURL,
        environment: [String: String] = ProcessInfo.processInfo.environment,
        fileManager: FileManager = .default
    ) {
        self.bundleURL = bundleURL.standardizedFileURL
        self.baseEnvironment = environment
        self.fileManager = fileManager
    }

    public func materialize(excludingLiveKey _: String?) async throws -> MaterializedRuntime {
        let runtimeRoot = bundleURL
            .appendingPathComponent("Contents/Resources/solstone-runtime", isDirectory: true)
            .standardizedFileURL
        guard isDirectory(runtimeRoot) else {
            throw NativeJournalRuntimeMaterializerError.missingRuntimeRoot(runtimeRoot.path)
        }

        let layout = SolstoneRuntimeLayout(rootURL: runtimeRoot)
        guard fileManager.isExecutableFile(atPath: layout.solstoneBinary.path) else {
            throw NativeJournalRuntimeMaterializerError.missingSolstoneExecutable(layout.solstoneBinary.path)
        }

        guard let version = bundleReleaseComponent("CFBundleShortVersionString"),
              let build = bundleReleaseComponent("CFBundleVersion") else {
            throw NativeJournalRuntimeMaterializerError.invalidBundleVersion
        }

        return MaterializedRuntime(
            key: "journal-\(version)-\(build)",
            layout: layout,
            environment: nativeEnvironment()
        )
    }

    private func isDirectory(_ url: URL) -> Bool {
        var isDirectory = ObjCBool(false)
        return fileManager.fileExists(atPath: url.path, isDirectory: &isDirectory) && isDirectory.boolValue
    }

    private func bundleReleaseComponent(_ key: String) -> String? {
        guard let value = Bundle(url: bundleURL)?.object(forInfoDictionaryKey: key) as? String else {
            return nil
        }
        let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }

    private func nativeEnvironment() -> [String: String] {
        baseEnvironment.filter { key, _ in
            !key.hasPrefix("UV_") && key != "PYTHONHOME" && key != "PYTHONPATH"
        }
    }
}
