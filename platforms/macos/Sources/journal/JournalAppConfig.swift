// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import Foundation
import JournalMarkKit
import os
import ServiceManagement
import SolstoneCore

@MainActor
protocol LoginItemManaging: AnyObject {
    var status: SMAppService.Status { get }
    func register() throws
    func unregister() throws
    func unregisterAwaitingCompletion() async throws
}

enum LaunchAtLoginUpdate: Equatable {
    case unchanged
    case applied
    case deferred
    case failed
}

enum LaunchAtLoginIssue: Equatable {
    case deferred
    case failed
}

@MainActor
final class LiveJournalLoginItemManager: LoginItemManaging {
    private static let watchdogPlistName = "app.solstone.journal.watchdog.plist"

    var status: SMAppService.Status { service.status }

    func register() throws {
        try service.register()
    }

    func unregister() throws {
        try service.unregister()
    }

    func unregisterAwaitingCompletion() async throws {
        try await service.unregister()
    }

    private var service: SMAppService {
        SMAppService.agent(plistName: Self.watchdogPlistName)
    }
}

private struct LoginItemRegistrationReceipt: Codable, Equatable {
    var bundlePath: String
    var build: String
}

@MainActor
final class JournalAppConfig {
    static let suiteName = "app.solstone.journal"

    private enum Keys {
        static let journalRoot = "journalRoot"
        static let launchAtLoginEnabled = "launchAtLoginEnabled"
        static let journalIconMarkAppliedAtLeastOnce = "journalIconMarkAppliedAtLeastOnce"
        static let journalIconCachedMarkJSON = "journalIconCachedMarkJSON"
        static let loginItemRegistrationReceipt = "loginItemRegistrationReceipt"
    }

    private let defaults: UserDefaults
    private let loginItemManager: any LoginItemManaging
    private let loginItemEligibility: () -> Bool
    private let loginItemBundleIdentity: () -> (url: URL, build: String?)
    private let loginItemUnregisterTimeoutSeconds: Double
    private(set) var launchAtLoginIssue: LaunchAtLoginIssue?

    init(
        defaults: UserDefaults = UserDefaults(suiteName: suiteName) ?? .standard,
        loginItemManager: any LoginItemManaging = LiveJournalLoginItemManager(),
        loginItemEligibility: @escaping () -> Bool = {
            let fileManager = FileManager.default
            return WatchdogAppLocationEligibility.isEligible(
                enclosingAppURL: Bundle.main.bundleURL,
                cachesURL: fileManager.urls(for: .cachesDirectory, in: .userDomainMask)[0],
                temporaryDirectoryURL: fileManager.temporaryDirectory
            )
        },
        loginItemBundleIdentity: @escaping () -> (url: URL, build: String?) = {
            let url = Bundle.main.bundleURL
            let build = (try? WatchdogBundleInfoReader.read(fromEnclosingBundleAt: url))?.buildVersion
            return (url, build)
        },
        loginItemUnregisterTimeoutSeconds: Double = 10
    ) {
        self.defaults = defaults
        self.loginItemManager = loginItemManager
        self.loginItemEligibility = loginItemEligibility
        self.loginItemBundleIdentity = loginItemBundleIdentity
        self.loginItemUnregisterTimeoutSeconds = loginItemUnregisterTimeoutSeconds
        self.launchAtLoginIssue = nil
    }

    var journalRoot: URL? {
        get {
            defaults.string(forKey: Keys.journalRoot).map {
                URL(fileURLWithPath: $0, isDirectory: true)
            }
        }
        set {
            defaults.set(newValue?.path, forKey: Keys.journalRoot)
        }
    }

    var resolvedJournalRoot: URL {
        journalRoot ?? ExpectedExitMarker
            .markerURL(for: ExpectedExitMarker.journalMarkerDiscriminator)
            .deletingLastPathComponent()
            .appendingPathComponent("journal", isDirectory: true)
    }

    var launchAtLoginEnabled: Bool {
        guard defaults.object(forKey: Keys.launchAtLoginEnabled) != nil else {
            return true
        }
        return defaults.bool(forKey: Keys.launchAtLoginEnabled)
    }

    var launchAtLoginFailureSentence: String? {
        switch launchAtLoginIssue {
        case .deferred:
            return JournalFirstRunCopy.loginItemUnavailable
        case .failed:
            return JournalFirstRunCopy.loginItemChangeFailed
        case nil:
            return nil
        }
    }

    private func readRegistrationReceipt() -> LoginItemRegistrationReceipt? {
        guard let data = defaults.data(forKey: Keys.loginItemRegistrationReceipt) else {
            return nil
        }
        return try? JSONDecoder().decode(LoginItemRegistrationReceipt.self, from: data)
    }

    @discardableResult
    private func writeRegistrationReceipt(bundleURL: URL, build: String) -> Bool {
        let normalizedPath = WatchdogAppLocationEligibility.normalized(bundleURL).path
        let receipt = LoginItemRegistrationReceipt(bundlePath: normalizedPath, build: build)
        guard let data = try? JSONEncoder().encode(receipt) else {
            defaults.removeObject(forKey: Keys.loginItemRegistrationReceipt)
            Logger.journalApp.error("journal login item registration could not be saved")
            return false
        }
        defaults.set(data, forKey: Keys.loginItemRegistrationReceipt)
        return true
    }

    private func clearRegistrationReceipt() {
        defaults.removeObject(forKey: Keys.loginItemRegistrationReceipt)
    }

    private func recordSuccessfulPreferenceChange(enabled: Bool) -> LaunchAtLoginUpdate {
        defaults.set(enabled, forKey: Keys.launchAtLoginEnabled)
        launchAtLoginIssue = nil
        if enabled {
            let identity = loginItemBundleIdentity()
            if let build = identity.build, loginItemManager.status == .enabled {
                writeRegistrationReceipt(bundleURL: identity.url, build: build)
            }
        } else {
            clearRegistrationReceipt()
        }
        return .applied
    }

    @discardableResult
    func setLaunchAtLoginEnabled(_ enabled: Bool) -> LaunchAtLoginUpdate {
        if enabled == launchAtLoginEnabled {
            return .unchanged
        }
        if enabled, !loginItemEligibility() {
            launchAtLoginIssue = .deferred
            return .deferred
        }
        do {
            if enabled {
                try loginItemManager.register()
            } else {
                try loginItemManager.unregister()
            }
        } catch {
            launchAtLoginIssue = .failed
            return .failed
        }
        return recordSuccessfulPreferenceChange(enabled: enabled)
    }

    @discardableResult
    func setLaunchAtLoginEnabledReportingFailure(_ enabled: Bool) throws -> LaunchAtLoginUpdate {
        if enabled == launchAtLoginEnabled {
            return .unchanged
        }
        if enabled, !loginItemEligibility() {
            launchAtLoginIssue = .deferred
            return .deferred
        }
        do {
            if enabled {
                try loginItemManager.register()
            } else {
                try loginItemManager.unregister()
            }
        } catch {
            launchAtLoginIssue = .failed
            throw error
        }
        return recordSuccessfulPreferenceChange(enabled: enabled)
    }

    var iconMarkAppliedAtLeastOnce: Bool {
        get {
            defaults.bool(forKey: Keys.journalIconMarkAppliedAtLeastOnce)
        }
        set {
            defaults.set(newValue, forKey: Keys.journalIconMarkAppliedAtLeastOnce)
        }
    }

    func cachedIconMark() -> JournalMark? {
        guard let data = defaults.data(forKey: Keys.journalIconCachedMarkJSON),
              let decoded = try? JSONDecoder().decode(JournalMark.self, from: data) else {
            return nil
        }
        return JournalMark.validate(decoded)
    }

    func setCachedIconMark(_ mark: JournalMark) {
        guard let validated = JournalMark.validate(mark),
              let data = try? JSONEncoder().encode(validated) else {
            return
        }
        defaults.set(data, forKey: Keys.journalIconCachedMarkJSON)
    }

    @discardableResult
    func applyLaunchAtLoginPreference() async -> LaunchAtLoginUpdate {
        if !launchAtLoginEnabled {
            do {
                try loginItemManager.unregister()
            } catch {
                launchAtLoginIssue = .failed
                Logger.journalApp.error("journal login item update failed: \(error.localizedDescription, privacy: .public)")
                return .failed
            }
            launchAtLoginIssue = nil
            return .applied
        }

        // A quarantined app can execute from AppTranslocation while its
        // bundled login item resolves the original download. Registering
        // there would let the watchdog relaunch a different identity
        // after an ordinary quit, so transient locations never enroll.
        guard loginItemEligibility() else {
            launchAtLoginIssue = .deferred
            return .deferred
        }

        let identity = loginItemBundleIdentity()
        let path = WatchdogAppLocationEligibility.normalized(identity.url).path

        let receipt = readRegistrationReceipt()
        if let build = identity.build,
           loginItemManager.status == .enabled,
           receipt == nil || receipt?.bundlePath != path || receipt?.build != build {
            clearRegistrationReceipt()
            // An enabled agent with no saved registration still points at the bundle
            // that first registered it. Unregister once so this install can register
            // the bundle now running. The saved registration keeps later launches
            // from doing it again.
            do {
                let loginItemManager = loginItemManager
                try await withTimeout(seconds: loginItemUnregisterTimeoutSeconds) { @MainActor [loginItemManager] in
                    try await loginItemManager.unregisterAwaitingCompletion()
                }
            } catch {
                Logger.journalApp.error("journal login item unregister failed: \(error.localizedDescription, privacy: .public)")
                return .failed
            }

            if !launchAtLoginEnabled {
                return .applied
            }
        }

        do {
            try loginItemManager.register()
        } catch {
            launchAtLoginIssue = .failed
            Logger.journalApp.error("journal login item update failed: \(error.localizedDescription, privacy: .public)")
            return .failed
        }

        guard let build = identity.build else {
            Logger.journalApp.info("journal login item registration skipped: bundle version unavailable")
            launchAtLoginIssue = nil
            return .applied
        }

        switch loginItemManager.status {
        case .enabled:
            if writeRegistrationReceipt(bundleURL: identity.url, build: build) {
                launchAtLoginIssue = nil
                return .applied
            } else {
                return .failed
            }
        case .requiresApproval:
            launchAtLoginIssue = nil
            return .applied
        case .notRegistered, .notFound:
            fallthrough
        @unknown default:
            Logger.journalApp.error("journal login item registration did not enable")
            return .failed
        }
    }
}
