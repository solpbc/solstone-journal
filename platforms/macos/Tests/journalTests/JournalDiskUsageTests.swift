// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import Foundation
import ServiceManagement
import Testing
@testable import journal

@MainActor
@Suite("JournalDiskUsage")
struct JournalDiskUsageTests {
    @Test func calculateBytesSumsRegularFilesAndSkipsHiddenFiles() async throws {
        let root = try makeTemporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        try Data(repeating: 0x01, count: 7).write(to: root.appendingPathComponent("one.txt"))
        let nested = root.appendingPathComponent("nested", isDirectory: true)
        try FileManager.default.createDirectory(at: nested, withIntermediateDirectories: true)
        try Data(repeating: 0x02, count: 11).write(to: nested.appendingPathComponent("two.txt"))
        try Data(repeating: 0x03, count: 13).write(to: root.appendingPathComponent(".hidden"))

        let bytes = await JournalDiskUsage.calculateBytes(under: root)

        #expect(bytes == 18)
    }

    @Test func windowModelCachesDiskUsageUntilInvalidatedOrExpired() async throws {
        let root = try makeTemporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        let defaults = makeDefaults()
        defer { defaults.clear() }
        let config = JournalAppConfig(defaults: defaults.defaults, loginItemManager: DiskFakeLoginItemManager())
        config.journalRoot = root
        let clock = DateBox(Date(timeIntervalSince1970: 0))
        let counter = DiskCounter()
        let model = JournalWindowModel(
            config: config,
            supervisor: JournalSupervisor(),
            fetchDiskUsage: { _ in await counter.next() },
            now: { clock.value },
            diskCacheDuration: 30
        )

        await model.loadDiskUsageIfNeeded()
        await model.loadDiskUsageIfNeeded()
        #expect(model.diskUsageBytes == 10)
        #expect(await counter.count == 1)

        clock.value = Date(timeIntervalSince1970: 31)
        await model.loadDiskUsageIfNeeded()
        #expect(model.diskUsageBytes == 20)
        #expect(await counter.count == 2)

        model.invalidateDiskUsage()
        await model.loadDiskUsageIfNeeded()
        #expect(model.diskUsageBytes == 30)
        #expect(await counter.count == 3)
    }

    private func makeTemporaryDirectory() throws -> URL {
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("journal-disk-tests-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        return url
    }
}

private actor DiskCounter {
    private var calls = 0

    var count: Int { calls }

    func next() -> Int64 {
        calls += 1
        return Int64(calls * 10)
    }
}

private final class DateBox: @unchecked Sendable {
    private let lock = NSLock()
    private var stored: Date

    init(_ stored: Date) {
        self.stored = stored
    }

    var value: Date {
        get { lock.withLock { stored } }
        set { lock.withLock { stored = newValue } }
    }
}

private struct DefaultsFixture {
    let suiteName: String
    let defaults: UserDefaults

    func clear() {
        defaults.removePersistentDomain(forName: suiteName)
    }
}

private func makeDefaults() -> DefaultsFixture {
    let suiteName = "app.solstone.journal.disk-tests.\(UUID().uuidString)"
    let defaults = UserDefaults(suiteName: suiteName)!
    defaults.removePersistentDomain(forName: suiteName)
    return DefaultsFixture(suiteName: suiteName, defaults: defaults)
}

@MainActor
private final class DiskFakeLoginItemManager: LoginItemManaging {
    var status: SMAppService.Status = .notRegistered

    func register() throws {}
    func unregister() throws {}
    func unregisterAwaitingCompletion() async throws {}
}
