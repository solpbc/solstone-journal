// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import Foundation
import Testing
@testable import JournalRuntime

@Suite("SolstoneRuntimeLayout")
struct SolstoneRuntimeLayoutTests {
    @Test func ensureCreatedIsIdempotent() throws {
        let root = try makeTemporaryDirectory().appendingPathComponent("runtime", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: root.deletingLastPathComponent()) }
        let layout = SolstoneRuntimeLayout(rootURL: root)

        try layout.ensureCreated()
        try layout.ensureCreated()

        for dir in [layout.rootURL, layout.binDir] {
            var isDirectory: ObjCBool = false
            #expect(FileManager.default.fileExists(atPath: dir.path, isDirectory: &isDirectory))
            #expect(isDirectory.boolValue)
        }
        #expect(!FileManager.default.fileExists(atPath: root.appendingPathComponent("python").path))
        #expect(!FileManager.default.fileExists(atPath: root.appendingPathComponent("cache").path))
        #expect(!FileManager.default.fileExists(atPath: root.appendingPathComponent("tools").path))
    }

    @Test func pathConstantsHaveStableSuffixes() {
        let layout = makeLayout()

        #expect(layout.binDir.path.hasSuffix("/bin"))
        #expect(layout.solstoneBinary.path.hasSuffix("/bin/solstone"))
    }

    private func makeLayout() -> SolstoneRuntimeLayout {
        SolstoneRuntimeLayout(rootURL: URL(fileURLWithPath: "/tmp/solstone-runtime-layout-tests/runtime", isDirectory: true))
    }

    private func makeTemporaryDirectory() throws -> URL {
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("solstone-runtime-layout-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        return url
    }
}
