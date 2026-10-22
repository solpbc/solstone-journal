// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import AppKit
import Foundation
import JournalMarkKit
import ServiceManagement
import Testing
@testable import journal

@MainActor
@Suite("JournalIconManager")
struct JournalIconManagerTests {
    @Test func launchReassertAppliesCachedMarkWhenBundleIconIsMissing() async throws {
        let fixture = try makeFixture()
        defer { fixture.clear() }
        fixture.config.setCachedIconMark(.uiTestSample)
        let applier = FakeIconApplier()
        let inspector = FakeBundleIconInspector(hasCustomIcon: false)
        let manager = JournalIconManager(
            config: fixture.config,
            applier: applier,
            inspector: inspector,
            logger: FakeIconLogger(),
            bundlePath: fixture.bundleURL.path
        )

        manager.reassertOnLaunch()
        await drainIconWork()

        #expect(applier.applyCalls == 1)
        #expect(applier.appliedBundlePaths == [fixture.bundleURL.path])
        #expect(applier.dockSetCalls == 1)
        #expect(!applier.lastDockImageWasNil)
        #expect(fixture.config.iconMarkAppliedAtLeastOnce)
    }

    @Test func launchReassertDoesNothingWhenCustomIconIsPresent() async throws {
        let fixture = try makeFixture()
        defer { fixture.clear() }
        fixture.config.setCachedIconMark(.uiTestSample)
        let applier = FakeIconApplier()
        let manager = JournalIconManager(
            config: fixture.config,
            applier: applier,
            inspector: FakeBundleIconInspector(hasCustomIcon: true),
            logger: FakeIconLogger(),
            bundlePath: fixture.bundleURL.path
        )

        manager.reassertOnLaunch()
        await drainIconWork()

        #expect(applier.applyCalls == 0)
    }

    @Test func launchReassertDoesNothingWithoutCachedMark() async throws {
        let fixture = try makeFixture()
        defer { fixture.clear() }
        let applier = FakeIconApplier()
        let manager = JournalIconManager(
            config: fixture.config,
            applier: applier,
            inspector: FakeBundleIconInspector(hasCustomIcon: false),
            logger: FakeIconLogger(),
            bundlePath: fixture.bundleURL.path
        )

        manager.reassertOnLaunch()
        await drainIconWork()

        #expect(applier.applyCalls == 0)
    }

    @Test func triggerArrivingMidComposeIsNoOp() async throws {
        let fixture = try makeFixture()
        defer { fixture.clear() }
        let applier = FakeIconApplier()
        let manager = JournalIconManager(
            config: fixture.config,
            applier: applier,
            inspector: FakeBundleIconInspector(hasCustomIcon: false),
            logger: FakeIconLogger(),
            bundlePath: fixture.bundleURL.path
        )
        applier.onApply = {
            manager.reassertOnLaunch()
        }

        manager.handleIdentityMark(.uiTestSample)
        await drainIconWork()

        #expect(applier.applyCalls == 1)
    }

    @Test func setIconFalseFallsBackToGenericAndDoesNotMarkApplied() async throws {
        let fixture = try makeFixture()
        defer { fixture.clear() }
        let applier = FakeIconApplier()
        applier.applyResult = false
        let logger = FakeIconLogger()
        let manager = JournalIconManager(
            config: fixture.config,
            applier: applier,
            inspector: FakeBundleIconInspector(hasCustomIcon: false),
            logger: logger,
            bundlePath: fixture.bundleURL.path
        )

        manager.handleIdentityMark(.uiTestSample)
        await drainIconWork()

        #expect(applier.applyCalls == 1)
        #expect(applier.clearCalls == 1)
        #expect(applier.dockSetCalls == 1)
        #expect(applier.lastDockImageWasNil)
        #expect(!fixture.config.iconMarkAppliedAtLeastOnce)
        #expect(logger.notices.contains { $0.contains("setIcon returned false") })
    }

    @Test func invalidMarkFallsBackWithoutApplyingPartialIcon() async throws {
        let fixture = try makeFixture()
        defer { fixture.clear() }
        let applier = FakeIconApplier()
        let logger = FakeIconLogger()
        let manager = JournalIconManager(
            config: fixture.config,
            applier: applier,
            inspector: FakeBundleIconInspector(hasCustomIcon: false),
            logger: logger,
            bundlePath: fixture.bundleURL.path
        )
        let invalid = JournalMark(
            icon1: JournalMark.Icon(
                name: "bug",
                color: JournalMark.MarkColor(hex: "#f59e0b"),
                rot: 90,
                svg: JournalMark.uiTestSample.icon1.svg
            ),
            icon2: JournalMark.uiTestSample.icon2,
            words: JournalMark.uiTestSample.words
        )

        manager.handleIdentityMark(invalid)
        await drainIconWork()

        #expect(applier.applyCalls == 0)
        #expect(applier.clearCalls == 1)
        #expect(applier.lastDockImageWasNil)
        #expect(logger.notices.contains { $0.contains("failed validation") })
    }

    @Test func realBundleInspectionTracksSetAndClearCustomIcon() throws {
        let appURL = try makeScratchAppBundle()
        defer { try? FileManager.default.removeItem(at: appURL) }
        let applier = NSWorkspaceIconApplier()
        let inspector = FinderInfoBundleIconInspector()
        let image = JournalIconCompositor.appIcon(spec: nil)

        try #require(applier.apply(image, toBundleAt: appURL.path))
        #expect(inspector.hasCustomIcon(atBundleAt: appURL.path))

        try #require(applier.clearCustomIcon(atBundleAt: appURL.path))
        #expect(!inspector.hasCustomIcon(atBundleAt: appURL.path))
    }

    private func makeFixture() throws -> IconManagerFixture {
        let suiteName = "app.solstone.journal.icon-manager.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suiteName)!
        defaults.removePersistentDomain(forName: suiteName)
        let bundleURL = try makeScratchAppBundle()
        return IconManagerFixture(
            suiteName: suiteName,
            defaults: defaults,
            config: JournalAppConfig(defaults: defaults, loginItemManager: IconFakeLoginItemManager()),
            bundleURL: bundleURL
        )
    }
}

private func drainIconWork() async {
    for _ in 0..<4 {
        await Task.yield()
    }
}

@MainActor
private func waitForIconCondition(_ condition: @MainActor () -> Bool) async {
    let deadline = ContinuousClock.now.advanced(by: .seconds(1))
    while ContinuousClock.now < deadline, !condition() {
        await Task.yield()
        try? await Task.sleep(for: .milliseconds(5))
    }
}

private func makeScratchAppBundle() throws -> URL {
    let appURL = FileManager.default.temporaryDirectory
        .appendingPathComponent("journal-icon-manager-\(UUID().uuidString).app", isDirectory: true)
    let contentsURL = appURL.appendingPathComponent("Contents", isDirectory: true)
    try FileManager.default.createDirectory(at: contentsURL, withIntermediateDirectories: true)
    let info = """
    <?xml version="1.0" encoding="UTF-8"?>
    <!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
    <plist version="1.0"><dict><key>CFBundleIdentifier</key><string>app.solstone.journal.icon-manager-test</string></dict></plist>
    """
    try info.data(using: .utf8)?.write(to: contentsURL.appendingPathComponent("Info.plist"))
    return appURL
}

private struct IconManagerFixture {
    let suiteName: String
    let defaults: UserDefaults
    let config: JournalAppConfig
    let bundleURL: URL

    @MainActor
    func clear() {
        defaults.removePersistentDomain(forName: suiteName)
        try? FileManager.default.removeItem(at: bundleURL)
    }
}

@MainActor
private final class FakeIconApplier: JournalIconApplying {
    var applyResult = true
    var onApply: (() -> Void)?
    private(set) var applyCalls = 0
    private(set) var clearCalls = 0
    private(set) var dockSetCalls = 0
    private(set) var appliedBundlePaths: [String] = []
    private var lastDockImage: NSImage?
    private var hasDockImageValue = false

    var lastDockImageWasNil: Bool {
        hasDockImageValue && lastDockImage == nil
    }

    func apply(_ image: NSImage, toBundleAt path: String) -> Bool {
        applyCalls += 1
        appliedBundlePaths.append(path)
        onApply?()
        return applyResult
    }

    func clearCustomIcon(atBundleAt path: String) -> Bool {
        clearCalls += 1
        return true
    }

    func setDockImage(_ image: NSImage?) {
        dockSetCalls += 1
        hasDockImageValue = true
        lastDockImage = image
    }
}

@MainActor
private final class FakeBundleIconInspector: JournalBundleIconInspecting {
    private let hasCustomIcon: Bool

    init(hasCustomIcon: Bool) {
        self.hasCustomIcon = hasCustomIcon
    }

    func hasCustomIcon(atBundleAt path: String) -> Bool {
        hasCustomIcon
    }
}

@MainActor
private final class FakeIconLogger: JournalIconLogging {
    private(set) var notices: [String] = []

    func notice(_ message: String) {
        notices.append(message)
    }
}

@MainActor
private final class IconFakeLoginItemManager: LoginItemManaging {
    var status: SMAppService.Status = .notRegistered

    func register() throws {}
    func unregister() throws {}
    func unregisterAwaitingCompletion() async throws {}
}
