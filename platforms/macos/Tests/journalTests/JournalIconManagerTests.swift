// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import AppKit
import JournalMarkKit
import Testing
@testable import journal

@MainActor
@Suite("JournalIconManager")
struct JournalIconManagerTests {
    @Test func launchClearsPreviouslyPublishedBundleIcon() {
        let applier = FakeIconApplier()
        let inspector = FakeBundleIconInspector(hasCustomIcon: true)
        let manager = JournalIconManager(applier: applier, inspector: inspector, logger: FakeIconLogger(), bundlePath: "/test.app")
        manager.start()
        #expect(applier.clearCalls == 1)
        #expect(applier.lastDockImageWasNil)
        manager.handleIdentityMark(.uiTestSample)
        #expect(applier.clearCalls == 1)
        #expect(!applier.lastDockImageWasNil)
    }

    @Test func aGenericBundleIsNeverWritten() {
        let applier = FakeIconApplier()
        let manager = JournalIconManager(applier: applier, inspector: FakeBundleIconInspector(hasCustomIcon: false), logger: FakeIconLogger())
        manager.start()
        manager.handleIdentityMark(.uiTestSample)
        #expect(applier.clearCalls == 0)
        #expect(applier.dockSetCalls == 2)
        #expect(!applier.lastDockImageWasNil)
    }

    @Test func failedClearIsReported() {
        let applier = FakeIconApplier()
        applier.clearResult = false
        let logger = FakeIconLogger()
        let manager = JournalIconManager(applier: applier, inspector: FakeBundleIconInspector(hasCustomIcon: true), logger: logger)
        manager.start()
        #expect(applier.clearCalls == 1)
        #expect(logger.notices.count == 1)
    }
}

@MainActor
private final class FakeIconApplier: JournalIconApplying {
    var clearResult = true
    var clearCalls = 0
    var dockSetCalls = 0
    var lastDockImageWasNil = true
    func clearCustomIcon(atBundleAt path: String) -> Bool { clearCalls += 1; return clearResult }
    func setDockImage(_ image: NSImage?) { dockSetCalls += 1; lastDockImageWasNil = image == nil }
}

@MainActor
private struct FakeBundleIconInspector: JournalBundleIconInspecting {
    var hasCustomIcon: Bool
    func hasCustomIcon(atBundleAt path: String) -> Bool { hasCustomIcon }
}

@MainActor
private final class FakeIconLogger: JournalIconLogging {
    var notices: [String] = []
    func notice(_ message: String) { notices.append(message) }
}
