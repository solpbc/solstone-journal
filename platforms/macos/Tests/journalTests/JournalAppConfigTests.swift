// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import Foundation
import JournalMarkKit
import ServiceManagement
import Testing
@testable import journal

@MainActor
@Suite("JournalAppConfig")
struct JournalAppConfigTests {
    private static let cachedMarkKey = "journalIconCachedMarkJSON"

    @Test func launchAtLoginDefaultsOnAndRegisters() async {
        let fixture = makeDefaults()
        defer { fixture.clear() }
        let loginItems = FakeLoginItemManager()
        let config = JournalAppConfig(defaults: fixture.defaults, loginItemManager: loginItems)

        await config.applyLaunchAtLoginPreference()

        #expect(config.launchAtLoginEnabled)
        #expect(loginItems.registerCalls == 1)
        #expect(loginItems.unregisterCalls == 0)
        #expect(loginItems.awaitedUnregisterCalls == 0)
    }

    @Test func disablingLaunchAtLoginDeregistersAndPersists() {
        let fixture = makeDefaults()
        defer { fixture.clear() }
        let loginItems = FakeLoginItemManager()
        let config = JournalAppConfig(defaults: fixture.defaults, loginItemManager: loginItems)

        config.setLaunchAtLoginEnabled(false)
        let reloaded = JournalAppConfig(defaults: fixture.defaults, loginItemManager: FakeLoginItemManager())

        #expect(!config.launchAtLoginEnabled)
        #expect(!reloaded.launchAtLoginEnabled)
        #expect(loginItems.registerCalls == 0)
        #expect(loginItems.unregisterCalls == 1)
    }

    @Test func ineligibleCurrentBundleDoesNotRegisterLoginItem() async {
        let fixture = makeDefaults()
        defer { fixture.clear() }
        let loginItems = FakeLoginItemManager()
        let config = JournalAppConfig(
            defaults: fixture.defaults,
            loginItemManager: loginItems,
            loginItemEligibility: { false }
        )

        let applied = await config.applyLaunchAtLoginPreference()

        #expect(applied == .deferred)
        #expect(config.launchAtLoginEnabled)
        #expect(loginItems.registerCalls == 0)
        #expect(loginItems.unregisterCalls == 0)
        #expect(fixture.defaults.object(forKey: "launchAtLoginEnabled") == nil)

        config.setLaunchAtLoginEnabled(false)
        #expect(loginItems.unregisterCalls == 1)
    }

    @Test func reportingFailureDoesNotStoreWhenRegisterThrows() {
        let fixture = makeDefaults()
        defer { fixture.clear() }
        fixture.defaults.set(false, forKey: "launchAtLoginEnabled")
        let loginItems = ThrowingLoginItemManager()
        let config = JournalAppConfig(defaults: fixture.defaults, loginItemManager: loginItems)

        #expect(throws: (any Error).self) {
            try config.setLaunchAtLoginEnabledReportingFailure(true)
        }

        #expect(fixture.defaults.bool(forKey: "launchAtLoginEnabled") == false)
        #expect(!config.launchAtLoginEnabled)
        #expect(loginItems.registerCalls == 1)
    }

    @Test func ineligibleEnablePersistsWithoutRegister() throws {
        let fixture = makeDefaults()
        defer { fixture.clear() }
        fixture.defaults.set(false, forKey: "launchAtLoginEnabled")
        let loginItems = FakeLoginItemManager()
        let config = JournalAppConfig(
            defaults: fixture.defaults,
            loginItemManager: loginItems,
            loginItemEligibility: { false }
        )

        let result = try config.setLaunchAtLoginEnabledReportingFailure(true)

        #expect(result == .deferred)
        #expect(!config.launchAtLoginEnabled)
        #expect(loginItems.registerCalls == 0)
        #expect(fixture.defaults.bool(forKey: "launchAtLoginEnabled") == false)
    }

    @Test func thrownUnregisterDoesNotWrite() {
        let fixture = makeDefaults()
        defer { fixture.clear() }
        fixture.defaults.set(true, forKey: "launchAtLoginEnabled")
        let loginItems = UnregisterThrowingLoginItemManager()
        let config = JournalAppConfig(defaults: fixture.defaults, loginItemManager: loginItems)

        let result = config.setLaunchAtLoginEnabled(false)

        #expect(result == .failed)
        #expect(config.launchAtLoginEnabled)
        #expect(fixture.defaults.bool(forKey: "launchAtLoginEnabled") == true)
        #expect(loginItems.unregisterCalls == 1)
    }

    @Test func unchangedValueDoesNotWriteOrRegister() {
        let fixture = makeDefaults()
        defer { fixture.clear() }
        let loginItems = FakeLoginItemManager()
        let config = JournalAppConfig(defaults: fixture.defaults, loginItemManager: loginItems)

        #expect(config.setLaunchAtLoginEnabled(true) == .unchanged)
        #expect(loginItems.registerCalls == 0)
        #expect(loginItems.unregisterCalls == 0)
        #expect(fixture.defaults.object(forKey: "launchAtLoginEnabled") == nil)

        fixture.defaults.set(false, forKey: "launchAtLoginEnabled")
        #expect(config.setLaunchAtLoginEnabled(false) == .unchanged)
        #expect(loginItems.unregisterCalls == 0)
        #expect(fixture.defaults.bool(forKey: "launchAtLoginEnabled") == false)
    }

    @Test func startupApplyFailureDoesNotWriteAndIsNotSuccess() async {
        let fixture = makeDefaults()
        defer { fixture.clear() }
        let loginItems = ThrowingLoginItemManager()
        let config = JournalAppConfig(defaults: fixture.defaults, loginItemManager: loginItems)

        let enabled = await config.applyLaunchAtLoginPreference()

        #expect(enabled == .failed)
        #expect(enabled != .applied)
        #expect(fixture.defaults.object(forKey: "launchAtLoginEnabled") == nil)
        #expect(config.launchAtLoginEnabled)
        #expect(loginItems.registerCalls == 1)

        fixture.defaults.set(false, forKey: "launchAtLoginEnabled")
        let unregistering = UnregisterThrowingLoginItemManager()
        let disabled = await JournalAppConfig(defaults: fixture.defaults, loginItemManager: unregistering)
            .applyLaunchAtLoginPreference()
        #expect(disabled == .failed)
        #expect(fixture.defaults.bool(forKey: "launchAtLoginEnabled") == false)
        #expect(unregistering.unregisterCalls == 1)
    }

    @Test func cachedIconMarkJSONRoundTripsValidatedMark() throws {
        let fixture = makeDefaults()
        defer { fixture.clear() }
        let config = JournalAppConfig(defaults: fixture.defaults, loginItemManager: FakeLoginItemManager())

        config.setCachedIconMark(.uiTestSample)

        #expect(config.cachedIconMark() == .uiTestSample)
    }

    @Test func cachedIconMarkFailsClosedForGarbageOrInvalidJSON() throws {
        let fixture = makeDefaults()
        defer { fixture.clear() }
        let config = JournalAppConfig(defaults: fixture.defaults, loginItemManager: FakeLoginItemManager())

        fixture.defaults.set(Data([0x00, 0x01]), forKey: Self.cachedMarkKey)
        #expect(config.cachedIconMark() == nil)

        let invalid = JournalMark(
            icon1: JournalMark.uiTestSample.icon1,
            icon2: JournalMark.uiTestSample.icon2,
            words: ["only-one-word"]
        )
        fixture.defaults.set(try JSONEncoder().encode(invalid), forKey: Self.cachedMarkKey)
        #expect(config.cachedIconMark() == nil)
    }

    @Test func appliedAtLeastOncePersistsIndependentlyOfCachedMark() {
        let fixture = makeDefaults()
        defer { fixture.clear() }
        let config = JournalAppConfig(defaults: fixture.defaults, loginItemManager: FakeLoginItemManager())

        config.iconMarkAppliedAtLeastOnce = true
        let reloaded = JournalAppConfig(defaults: fixture.defaults, loginItemManager: FakeLoginItemManager())

        #expect(reloaded.iconMarkAppliedAtLeastOnce)
        #expect(reloaded.cachedIconMark() == nil)
    }

    private func makeDefaults() -> DefaultsFixture {
        let suiteName = "app.solstone.journal.tests.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suiteName)!
        defaults.removePersistentDomain(forName: suiteName)
        return DefaultsFixture(suiteName: suiteName, defaults: defaults)
    }
}

private struct DefaultsFixture {
    let suiteName: String
    let defaults: UserDefaults

    func clear() {
        defaults.removePersistentDomain(forName: suiteName)
    }
}

@MainActor
private final class FakeLoginItemManager: LoginItemManaging {
    var status: SMAppService.Status = .notRegistered
    private(set) var registerCalls = 0
    private(set) var unregisterCalls = 0
    private(set) var awaitedUnregisterCalls = 0

    func register() throws {
        registerCalls += 1
    }

    func unregister() throws {
        unregisterCalls += 1
    }

    func unregisterAwaitingCompletion() async throws {
        awaitedUnregisterCalls += 1
    }
}

@MainActor
private final class ThrowingLoginItemManager: LoginItemManaging {
    var status: SMAppService.Status = .notRegistered
    private(set) var registerCalls = 0

    func register() throws {
        registerCalls += 1
        throw CocoaError(.fileWriteUnknown)
    }

    func unregister() throws {}

    func unregisterAwaitingCompletion() async throws {}
}

@MainActor
private final class UnregisterThrowingLoginItemManager: LoginItemManaging {
    var status: SMAppService.Status = .notRegistered
    private(set) var unregisterCalls = 0

    func register() throws {}

    func unregister() throws {
        unregisterCalls += 1
        throw CocoaError(.fileWriteUnknown)
    }

    func unregisterAwaitingCompletion() async throws {}
}
