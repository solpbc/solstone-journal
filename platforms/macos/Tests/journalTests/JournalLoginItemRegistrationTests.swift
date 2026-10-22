// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import Foundation
import ServiceManagement
import SolstoneCore
import Testing
@testable import journal

@MainActor
@Suite("JournalLoginItemRegistration")
struct JournalLoginItemRegistrationTests {
    private let testURL = URL(fileURLWithPath: "/Applications/Journal.app")

    @Test func matchingNormalizedPathAndBuildDoesNotUnregister() async {
        let fixture = makeDefaults()
        defer { fixture.clear() }
        let normalizedPath = WatchdogAppLocationEligibility.normalized(testURL).path
        seedReceipt(defaults: fixture.defaults, bundlePath: normalizedPath, build: "102")

        let fake = ScriptedLoginItemManager(status: .enabled)
        let config = JournalAppConfig(
            defaults: fixture.defaults,
            loginItemManager: fake,
            loginItemEligibility: { true },
            loginItemBundleIdentity: { (self.testURL, "102") }
        )

        let result = await config.applyLaunchAtLoginPreference()
        #expect(result == .applied)
        #expect(fake.callLog == ["register"])
        #expect(readReceipt(defaults: fixture.defaults) == TestReceipt(bundlePath: normalizedPath, build: "102"))
        #expect(config.launchAtLoginIssue == nil)
    }

    @Test func buildDifferenceCyclesAndPersistsNewReceipt() async {
        let fixture = makeDefaults()
        defer { fixture.clear() }
        let normalizedPath = WatchdogAppLocationEligibility.normalized(testURL).path
        seedReceipt(defaults: fixture.defaults, bundlePath: normalizedPath, build: "102")

        let fake = ScriptedLoginItemManager(status: .enabled)
        fake.statusAfterRegister = .enabled
        var receiptPresentDuringRegister = true
        fake.onRegister = { [fixture] in
            receiptPresentDuringRegister = fixture.defaults.object(forKey: "loginItemRegistrationReceipt") != nil
        }

        let config = JournalAppConfig(
            defaults: fixture.defaults,
            loginItemManager: fake,
            loginItemEligibility: { true },
            loginItemBundleIdentity: { (self.testURL, "103") }
        )

        let result = await config.applyLaunchAtLoginPreference()
        #expect(result == .applied)
        #expect(fake.callLog == ["unregisterAwaiting", "register"])
        #expect(!receiptPresentDuringRegister)
        #expect(readReceipt(defaults: fixture.defaults) == TestReceipt(bundlePath: normalizedPath, build: "103"))
        #expect(config.launchAtLoginIssue == nil)
    }

    @Test func pathDifferenceCyclesAndNormalizingPathDoesNotUnregister() async {
        let fixture = makeDefaults()
        defer { fixture.clear() }
        let normalizedPath = WatchdogAppLocationEligibility.normalized(testURL).path

        // Subtest 1: Path mismatch cycles
        seedReceipt(defaults: fixture.defaults, bundlePath: "/Applications/OldJournal.app", build: "102")
        let fakeMismatch = ScriptedLoginItemManager(status: .enabled)
        fakeMismatch.statusAfterRegister = .enabled
        let configMismatch = JournalAppConfig(
            defaults: fixture.defaults,
            loginItemManager: fakeMismatch,
            loginItemEligibility: { true },
            loginItemBundleIdentity: { (self.testURL, "102") }
        )
        let resultMismatch = await configMismatch.applyLaunchAtLoginPreference()
        #expect(resultMismatch == .applied)
        #expect(fakeMismatch.callLog == ["unregisterAwaiting", "register"])
        #expect(readReceipt(defaults: fixture.defaults) == TestReceipt(bundlePath: normalizedPath, build: "102"))

        // Subtest 2: Raw URL with .. that normalizes to receipt path does not cycle
        let unnormalizedURL = URL(fileURLWithPath: "/Applications/Subdir/../Journal.app")
        let fakeNormalized = ScriptedLoginItemManager(status: .enabled)
        let configNormalized = JournalAppConfig(
            defaults: fixture.defaults,
            loginItemManager: fakeNormalized,
            loginItemEligibility: { true },
            loginItemBundleIdentity: { (unnormalizedURL, "102") }
        )
        let resultNormalized = await configNormalized.applyLaunchAtLoginPreference()
        #expect(resultNormalized == .applied)
        #expect(fakeNormalized.callLog == ["register"])
    }

    @Test func absentReceiptCyclesOnceAndSecondRunDoesNotUnregister() async {
        let fixture = makeDefaults()
        defer { fixture.clear() }
        let normalizedPath = WatchdogAppLocationEligibility.normalized(testURL).path

        let fake = ScriptedLoginItemManager(status: .enabled)
        fake.statusAfterRegister = .enabled
        let config = JournalAppConfig(
            defaults: fixture.defaults,
            loginItemManager: fake,
            loginItemEligibility: { true },
            loginItemBundleIdentity: { (self.testURL, "102") }
        )

        let firstResult = await config.applyLaunchAtLoginPreference()
        #expect(firstResult == .applied)
        #expect(fake.callLog == ["unregisterAwaiting", "register"])
        #expect(readReceipt(defaults: fixture.defaults) == TestReceipt(bundlePath: normalizedPath, build: "102"))

        let secondResult = await config.applyLaunchAtLoginPreference()
        #expect(secondResult == .applied)
        #expect(fake.callLog == ["unregisterAwaiting", "register", "register"])
    }

    @Test func unreadableReceiptBytesCycles() async {
        let fixture = makeDefaults()
        defer { fixture.clear() }
        fixture.defaults.set(Data([0x00, 0x01, 0x02]), forKey: "loginItemRegistrationReceipt")
        let normalizedPath = WatchdogAppLocationEligibility.normalized(testURL).path

        let fake = ScriptedLoginItemManager(status: .enabled)
        fake.statusAfterRegister = .enabled
        let config = JournalAppConfig(
            defaults: fixture.defaults,
            loginItemManager: fake,
            loginItemEligibility: { true },
            loginItemBundleIdentity: { (self.testURL, "102") }
        )

        let result = await config.applyLaunchAtLoginPreference()
        #expect(result == .applied)
        #expect(fake.callLog == ["unregisterAwaiting", "register"])
        #expect(readReceipt(defaults: fixture.defaults) == TestReceipt(bundlePath: normalizedPath, build: "102"))
    }

    @Test func notRegisteredAndNotFoundPerformPlainRegister() async {
        let fixture = makeDefaults()
        defer { fixture.clear() }
        let normalizedPath = WatchdogAppLocationEligibility.normalized(testURL).path

        // Variant 1: .notRegistered with register setting .enabled -> receipt written
        let fakeEnabled = ScriptedLoginItemManager(status: .notRegistered)
        fakeEnabled.statusAfterRegister = .enabled
        let configEnabled = JournalAppConfig(
            defaults: fixture.defaults,
            loginItemManager: fakeEnabled,
            loginItemEligibility: { true },
            loginItemBundleIdentity: { (self.testURL, "102") }
        )
        let resultEnabled = await configEnabled.applyLaunchAtLoginPreference()
        #expect(resultEnabled == .applied)
        #expect(fakeEnabled.callLog == ["register"])
        #expect(readReceipt(defaults: fixture.defaults) == TestReceipt(bundlePath: normalizedPath, build: "102"))

        // Variant 2: .notRegistered with register leaving .notRegistered -> no receipt, result .failed, issue not .failed
        fixture.clear()
        let fakeUnchanged = ScriptedLoginItemManager(status: .notRegistered)
        fakeUnchanged.statusAfterRegister = .notRegistered
        let configUnchanged = JournalAppConfig(
            defaults: fixture.defaults,
            loginItemManager: fakeUnchanged,
            loginItemEligibility: { true },
            loginItemBundleIdentity: { (self.testURL, "102") }
        )
        let resultUnchanged = await configUnchanged.applyLaunchAtLoginPreference()
        #expect(resultUnchanged == .failed)
        #expect(fakeUnchanged.callLog == ["register"])
        #expect(readReceipt(defaults: fixture.defaults) == nil)
        #expect(configUnchanged.launchAtLoginIssue == nil)

        // Variant 3: .notFound with register setting .enabled -> receipt written
        fixture.clear()
        let fakeNotFound = ScriptedLoginItemManager(status: .notFound)
        fakeNotFound.statusAfterRegister = .enabled
        let configNotFound = JournalAppConfig(
            defaults: fixture.defaults,
            loginItemManager: fakeNotFound,
            loginItemEligibility: { true },
            loginItemBundleIdentity: { (self.testURL, "102") }
        )
        let resultNotFound = await configNotFound.applyLaunchAtLoginPreference()
        #expect(resultNotFound == .applied)
        #expect(fakeNotFound.callLog == ["register"])
        #expect(readReceipt(defaults: fixture.defaults) == TestReceipt(bundlePath: normalizedPath, build: "102"))
    }

    @Test func requiresApprovalLeavesStatusWithoutReceipt() async {
        let fixture = makeDefaults()
        defer { fixture.clear() }

        let fake = ScriptedLoginItemManager(status: .requiresApproval)
        fake.statusAfterRegister = .requiresApproval
        let config = JournalAppConfig(
            defaults: fixture.defaults,
            loginItemManager: fake,
            loginItemEligibility: { true },
            loginItemBundleIdentity: { (self.testURL, "102") }
        )

        let result = await config.applyLaunchAtLoginPreference()
        #expect(result == .applied)
        #expect(fake.callLog == ["register"])
        #expect(readReceipt(defaults: fixture.defaults) == nil)
        #expect(config.launchAtLoginIssue == nil)
    }

    @Test func ineligibleDefersAndPreferenceOffRunsSyncUnregisterOnly() async {
        let fixture = makeDefaults()
        defer { fixture.clear() }
        let normalizedPath = WatchdogAppLocationEligibility.normalized(testURL).path

        // Subtest 1: Ineligible defers, no service calls, no receipt
        let fakeIneligible = ScriptedLoginItemManager(status: .notRegistered)
        let configIneligible = JournalAppConfig(
            defaults: fixture.defaults,
            loginItemManager: fakeIneligible,
            loginItemEligibility: { false },
            loginItemBundleIdentity: { (self.testURL, "102") }
        )
        let resultIneligible = await configIneligible.applyLaunchAtLoginPreference()
        #expect(resultIneligible == .deferred)
        #expect(fakeIneligible.callLog.isEmpty)
        #expect(readReceipt(defaults: fixture.defaults) == nil)

        // Subtest 2: Preference off runs sync unregister, no receipt written, existing receipt left in place
        seedReceipt(defaults: fixture.defaults, bundlePath: normalizedPath, build: "102")
        fixture.defaults.set(false, forKey: "launchAtLoginEnabled")
        let fakeOff = ScriptedLoginItemManager(status: .enabled)
        let configOff = JournalAppConfig(
            defaults: fixture.defaults,
            loginItemManager: fakeOff,
            loginItemEligibility: { true },
            loginItemBundleIdentity: { (self.testURL, "102") }
        )
        let resultOff = await configOff.applyLaunchAtLoginPreference()
        #expect(resultOff == .applied)
        #expect(fakeOff.callLog == ["unregister"])
        #expect(readReceipt(defaults: fixture.defaults) == TestReceipt(bundlePath: normalizedPath, build: "102"))
    }

    @Test func awaitedUnregisterThrowFailsWithoutRegistering() async {
        let fixture = makeDefaults()
        defer { fixture.clear() }

        let fake = ScriptedLoginItemManager(status: .enabled)
        fake.unregisterAwaitingError = CocoaError(.fileWriteUnknown)
        let config = JournalAppConfig(
            defaults: fixture.defaults,
            loginItemManager: fake,
            loginItemEligibility: { true },
            loginItemBundleIdentity: { (self.testURL, "102") }
        )

        let result = await config.applyLaunchAtLoginPreference()
        #expect(result == .failed)
        #expect(fake.callLog == ["unregisterAwaiting"])
        #expect(readReceipt(defaults: fixture.defaults) == nil)
        #expect(config.launchAtLoginIssue == nil)
    }

    @Test func awaitedUnregisterTimeoutCancelsAndFails() async {
        let fixture = makeDefaults()
        defer { fixture.clear() }

        let fake = ScriptedLoginItemManager(status: .enabled)
        fake.onAwaitedUnregister = {
            try await Task.sleep(for: .seconds(30))
        }
        let config = JournalAppConfig(
            defaults: fixture.defaults,
            loginItemManager: fake,
            loginItemEligibility: { true },
            loginItemBundleIdentity: { (self.testURL, "102") },
            loginItemUnregisterTimeoutSeconds: 0.05
        )

        let result = await config.applyLaunchAtLoginPreference()
        #expect(result == .failed)
        #expect(fake.callLog == ["unregisterAwaiting"])
        #expect(readReceipt(defaults: fixture.defaults) == nil)
        #expect(config.launchAtLoginIssue == nil)
    }

    @Test func plainRegisterThrowFailsWithIssue() async {
        let fixture = makeDefaults()
        defer { fixture.clear() }

        let fake = ScriptedLoginItemManager(status: .notRegistered)
        fake.registerError = CocoaError(.fileWriteUnknown)
        let config = JournalAppConfig(
            defaults: fixture.defaults,
            loginItemManager: fake,
            loginItemEligibility: { true },
            loginItemBundleIdentity: { (self.testURL, "102") }
        )

        let result = await config.applyLaunchAtLoginPreference()
        #expect(result == .failed)
        #expect(fake.callLog == ["register"])
        #expect(readReceipt(defaults: fixture.defaults) == nil)
        #expect(config.launchAtLoginIssue == .failed)
    }

    @Test func setterDisablingClearsReceiptOnlyOnSuccess() {
        let fixture = makeDefaults()
        defer { fixture.clear() }
        let normalizedPath = WatchdogAppLocationEligibility.normalized(testURL).path

        // setLaunchAtLoginEnabled(false) clears seeded receipt
        seedReceipt(defaults: fixture.defaults, bundlePath: normalizedPath, build: "102")
        fixture.defaults.set(true, forKey: "launchAtLoginEnabled")
        let fake1 = ScriptedLoginItemManager()
        let config1 = JournalAppConfig(defaults: fixture.defaults, loginItemManager: fake1)
        #expect(config1.setLaunchAtLoginEnabled(false) == .applied)
        #expect(readReceipt(defaults: fixture.defaults) == nil)
        #expect(!config1.launchAtLoginEnabled)

        // setLaunchAtLoginEnabledReportingFailure(false) clears seeded receipt
        seedReceipt(defaults: fixture.defaults, bundlePath: normalizedPath, build: "102")
        fixture.defaults.set(true, forKey: "launchAtLoginEnabled")
        let fake2 = ScriptedLoginItemManager()
        let config2 = JournalAppConfig(defaults: fixture.defaults, loginItemManager: fake2)
        #expect((try? config2.setLaunchAtLoginEnabledReportingFailure(false)) == .applied)
        #expect(readReceipt(defaults: fixture.defaults) == nil)
        #expect(!config2.launchAtLoginEnabled)

        // Thrown unregister leaves receipt and preference unchanged
        seedReceipt(defaults: fixture.defaults, bundlePath: normalizedPath, build: "102")
        fixture.defaults.set(true, forKey: "launchAtLoginEnabled")
        let fakeThrowing = ScriptedLoginItemManager()
        fakeThrowing.unregisterError = CocoaError(.fileWriteUnknown)
        let configThrowing = JournalAppConfig(defaults: fixture.defaults, loginItemManager: fakeThrowing)
        #expect(configThrowing.setLaunchAtLoginEnabled(false) == .failed)
        #expect(readReceipt(defaults: fixture.defaults) != nil)
        #expect(configThrowing.launchAtLoginEnabled)

        #expect(throws: (any Error).self) {
            try configThrowing.setLaunchAtLoginEnabledReportingFailure(false)
        }
        #expect(readReceipt(defaults: fixture.defaults) != nil)
        #expect(configThrowing.launchAtLoginEnabled)

        // Unchanged setLaunchAtLoginEnabled(true) does no service call
        let fakeUnchanged = ScriptedLoginItemManager()
        let configUnchanged = JournalAppConfig(defaults: fixture.defaults, loginItemManager: fakeUnchanged)
        #expect(configUnchanged.setLaunchAtLoginEnabled(true) == .unchanged)
        #expect(fakeUnchanged.callLog.isEmpty)
        #expect(readReceipt(defaults: fixture.defaults) != nil)
    }

    @Test func turningOnFromFalseWritesReceiptOnlyWhenStatusBecomesEnabled() {
        let fixture = makeDefaults()
        defer { fixture.clear() }
        let normalizedPath = WatchdogAppLocationEligibility.normalized(testURL).path

        // Variant 1: register leaves status .enabled -> receipt written
        fixture.defaults.set(false, forKey: "launchAtLoginEnabled")
        let fakeEnabled = ScriptedLoginItemManager(status: .notRegistered)
        fakeEnabled.statusAfterRegister = .enabled
        let configEnabled = JournalAppConfig(
            defaults: fixture.defaults,
            loginItemManager: fakeEnabled,
            loginItemEligibility: { true },
            loginItemBundleIdentity: { (self.testURL, "102") }
        )
        #expect(configEnabled.setLaunchAtLoginEnabled(true) == .applied)
        #expect(configEnabled.launchAtLoginEnabled)
        #expect(readReceipt(defaults: fixture.defaults) == TestReceipt(bundlePath: normalizedPath, build: "102"))

        // Variant 2: register leaves status .requiresApproval -> preference true, receipt absent
        fixture.clear()
        fixture.defaults.set(false, forKey: "launchAtLoginEnabled")
        let fakeApproval = ScriptedLoginItemManager(status: .notRegistered)
        fakeApproval.statusAfterRegister = .requiresApproval
        let configApproval = JournalAppConfig(
            defaults: fixture.defaults,
            loginItemManager: fakeApproval,
            loginItemEligibility: { true },
            loginItemBundleIdentity: { (self.testURL, "102") }
        )
        #expect(configApproval.setLaunchAtLoginEnabled(true) == .applied)
        #expect(configApproval.launchAtLoginEnabled)
        #expect(readReceipt(defaults: fixture.defaults) == nil)

        // Variant 3: register leaves status .notRegistered -> preference true, receipt absent
        fixture.clear()
        fixture.defaults.set(false, forKey: "launchAtLoginEnabled")
        let fakeNotReg = ScriptedLoginItemManager(status: .notRegistered)
        fakeNotReg.statusAfterRegister = .notRegistered
        let configNotReg = JournalAppConfig(
            defaults: fixture.defaults,
            loginItemManager: fakeNotReg,
            loginItemEligibility: { true },
            loginItemBundleIdentity: { (self.testURL, "102") }
        )
        #expect(configNotReg.setLaunchAtLoginEnabled(true) == .applied)
        #expect(configNotReg.launchAtLoginEnabled)
        #expect(readReceipt(defaults: fixture.defaults) == nil)
    }

    @Test func disablingDuringCycleSkipsRegisterAndLeavesNoReceipt() async {
        let fixture = makeDefaults()
        defer { fixture.clear() }

        let fake = ScriptedLoginItemManager(status: .enabled)
        fake.statusAfterRegister = .enabled

        var unregisterHoldContinuation: CheckedContinuation<Void, Never>?
        fake.onAwaitedUnregister = {
            await withCheckedContinuation { cont in
                unregisterHoldContinuation = cont
            }
        }

        let config = JournalAppConfig(
            defaults: fixture.defaults,
            loginItemManager: fake,
            loginItemEligibility: { true },
            loginItemBundleIdentity: { (self.testURL, "102") }
        )

        let task = Task { @MainActor in
            await config.applyLaunchAtLoginPreference()
        }

        while unregisterHoldContinuation == nil {
            await Task.yield()
        }

        // Set launchAtLoginEnabled defaults key directly to false
        fixture.defaults.set(false, forKey: "launchAtLoginEnabled")

        let hold = unregisterHoldContinuation
        unregisterHoldContinuation = nil
        hold?.resume()

        let result = await task.value
        #expect(result == .applied)
        #expect(fake.callLog == ["unregisterAwaiting"])
        #expect(readReceipt(defaults: fixture.defaults) == nil)
    }

    private func makeDefaults() -> DefaultsFixture {
        let suiteName = "app.solstone.journal.tests.registration.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suiteName)!
        defaults.removePersistentDomain(forName: suiteName)
        return DefaultsFixture(suiteName: suiteName, defaults: defaults)
    }

    private func seedReceipt(defaults: UserDefaults, bundlePath: String, build: String) {
        let receipt = TestReceipt(bundlePath: bundlePath, build: build)
        let data = try! JSONEncoder().encode(receipt)
        defaults.set(data, forKey: "loginItemRegistrationReceipt")
    }

    private func readReceipt(defaults: UserDefaults) -> TestReceipt? {
        guard let data = defaults.data(forKey: "loginItemRegistrationReceipt"),
              let receipt = try? JSONDecoder().decode(TestReceipt.self, from: data) else {
            return nil
        }
        return receipt
    }
}

private struct TestReceipt: Codable, Equatable {
    var bundlePath: String
    var build: String
}

private struct DefaultsFixture {
    let suiteName: String
    let defaults: UserDefaults

    func clear() {
        defaults.removePersistentDomain(forName: suiteName)
    }
}

@MainActor
private final class ScriptedLoginItemManager: LoginItemManaging {
    var status: SMAppService.Status
    var statusAfterRegister: SMAppService.Status?
    var registerError: Error?
    var unregisterError: Error?
    var unregisterAwaitingError: Error?
    var onAwaitedUnregister: (() async throws -> Void)?
    var onRegister: (() -> Void)?

    private(set) var callLog: [String] = []

    init(status: SMAppService.Status = .notRegistered) {
        self.status = status
    }

    func register() throws {
        callLog.append("register")
        onRegister?()
        if let error = registerError {
            throw error
        }
        if let next = statusAfterRegister {
            status = next
        }
    }

    func unregister() throws {
        callLog.append("unregister")
        if let error = unregisterError {
            throw error
        }
    }

    func unregisterAwaitingCompletion() async throws {
        callLog.append("unregisterAwaiting")
        if let hook = onAwaitedUnregister {
            try await hook()
        }
        if let error = unregisterAwaitingError {
            throw error
        }
    }
}
