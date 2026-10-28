// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import Foundation

public struct SolstoneRuntimeLayout: Sendable {
    public let rootURL: URL

    public init(rootURL: URL) {
        self.rootURL = rootURL
    }

    public var binDir: URL { rootURL.appendingPathComponent("bin", isDirectory: true) }
    public var solstoneBinary: URL { binDir.appendingPathComponent("solstone") }

    public func ensureCreated() throws {
        let fileManager = FileManager.default
        for dir in [rootURL, binDir] {
            try fileManager.createDirectory(at: dir, withIntermediateDirectories: true)
        }
    }
}
