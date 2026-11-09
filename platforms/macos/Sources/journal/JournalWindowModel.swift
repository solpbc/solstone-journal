// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import Foundation
import JournalMarkKit
import JournalRuntime
import Observation
import os
import SolstoneCore

enum JournalPane: String, CaseIterable, Hashable, Identifiable {
    case home
    case journal
    case runState
    case devices
    case backup
    case startup
    case updates

    var id: String { rawValue }

    var title: String {
        switch self {
        case .home: return "home"
        case .journal: return "location"
        case .runState: return "run state"
        case .devices: return "devices"
        case .backup: return "backup"
        case .startup: return "startup"
        case .updates: return "updates"
        }
    }

    var systemImage: String {
        switch self {
        case .home: return "house"
        case .journal: return "book.closed"
        case .runState: return "waveform.path.ecg"
        case .devices: return "iphone.gen3"
        case .backup: return "externaldrive"
        case .startup: return "power"
        case .updates: return "arrow.down.circle"
        }
    }
}

public enum JournalMarkPresentation: Sendable, Equatable {
    case generic
    case loading
    case unavailable
    case mark(JournalMark)
}

enum JournalRunDisplay: String, CaseIterable, Sendable {
    case starting
    case running
    case stopped
    case blocked
    case unknown

    var label: String {
        switch self {
        case .starting: return "starting…"
        case .running: return "running"
        case .stopped: return "stopped"
        case .blocked: return "blocked"
        case .unknown: return "unknown"
        }
    }

    static func derive(state: JournalSupervisorState, runtimeStatus: JournalRuntimeStatus) -> JournalRunDisplay {
        switch state {
        case .blocked:
            return .blocked
        case .materializing, .starting, .waitingForReadiness:
            return .starting
        case .terminating:
            return .stopped
        case .running:
            switch runtimeStatus {
            case .running:
                return .running
            case .restarting(_):
                return .starting
            case .stopped, .stoppedByUser:
                return .stopped
            case .unobserved, .setupNeeded, .unknown:
                return .unknown
            }
        case .failed:
            switch runtimeStatus {
            case .stopped, .stoppedByUser:
                return .stopped
            case .restarting(_):
                return .starting
            case .unobserved, .running, .setupNeeded, .unknown:
                return .unknown
            }
        case .idle:
            switch runtimeStatus {
            case .stopped, .stoppedByUser:
                return .stopped
            case .restarting(_):
                return .starting
            case .unobserved, .running, .setupNeeded, .unknown:
                return .unknown
            }
        }
    }
}

enum JournalHomeOffer: Equatable {
    case unconfigured
    case door
    case start
    case none
    case runState
}

enum JournalHealthDisplay: String, CaseIterable, Sendable {
    case healthy
    case stopped
    case unknown

    var label: String {
        switch self {
        case .healthy: return "healthy"
        case .stopped: return "stopped"
        case .unknown: return "unknown"
        }
    }
}

@MainActor
@Observable
final class JournalWindowModel {
    typealias IdentityFetch = @Sendable (String) async -> JournalIdentityRead
    typealias IdentityRetryPause = @Sendable () async throws -> Void
    typealias DiskUsageFetch = @Sendable (URL) async -> Int64
    typealias HealthFetch = @Sendable (URL, [String: String]?) async -> JournalHealthCheckResult
    typealias VersionFetch = @Sendable (URL, [String: String]?) async -> String?
    typealias VersionExecutableURLProvider = @Sendable () -> URL?
    typealias AboutStringProvider = @Sendable () -> String
    typealias AboutArchProvider = @Sendable () -> String?
    typealias NowProvider = @Sendable () -> Date
    typealias IdentityMarkObserver = @MainActor @Sendable (JournalMark) -> Void

    @ObservationIgnored private let config: JournalAppConfig
    let supervisor: JournalSupervisor
    @ObservationIgnored private let checkLocalAccess: (@Sendable () async throws -> Void)?
    @ObservationIgnored private let browserLink: @Sendable (String) async throws -> URL
    @ObservationIgnored private let baseURL: String
    @ObservationIgnored private let fetchIdentity: IdentityFetch
    @ObservationIgnored private let identityRetryPause: IdentityRetryPause
    @ObservationIgnored private let fetchDiskUsage: DiskUsageFetch
    @ObservationIgnored private let fetchHealth: HealthFetch
    @ObservationIgnored private let fetchVersion: VersionFetch
    @ObservationIgnored private let versionExecutableURL: VersionExecutableURLProvider
    @ObservationIgnored private let aboutOSVersion: AboutStringProvider
    @ObservationIgnored private let aboutArch: AboutArchProvider
    @ObservationIgnored private let appBuild: String?
    @ObservationIgnored private let now: NowProvider
    @ObservationIgnored private let diskCacheDuration: TimeInterval
    @ObservationIgnored var onIdentityMark: IdentityMarkObserver?
    let devicesModel: JournalDevicesModel

    var selectedPane: JournalPane = .home
    var localAccessFailure: String?
    /// The last open didn't get a link from the journal. Cleared by the next open that does.
    var openFailure: String?
    var identityRead: JournalIdentityRead?
    var diskUsageBytes: Int64?
    var healthDisplay: JournalHealthDisplay = .unknown
    var journalVersion = "unknown"

    @ObservationIgnored private var identityFetchTask: Task<Void, Never>?
    private var identityFetchCompleted = false
    private var identityLandingGeneration = 0
    private var diskUsageLoadedAt: Date?

    init(
        config: JournalAppConfig,
        supervisor: JournalSupervisor,
        baseURL: String = "http://127.0.0.1:5015",
        identitySession: URLSession = .shared,
        dataLoader: (@Sendable (URLRequest) async throws -> (Data, URLResponse))? = nil,
        checkLocalAccess: (@Sendable () async throws -> Void)? = nil,
        browserLink: @escaping @Sendable (String) async throws -> URL = { _ in throw LocalHTTPError.unavailable },
        fetchIdentity: IdentityFetch? = nil,
        identityRetryPause: @escaping IdentityRetryPause = { try await Task.sleep(for: .milliseconds(500)) },
        fetchDiskUsage: DiskUsageFetch? = nil,
        fetchHealth: HealthFetch? = nil,
        fetchVersion: VersionFetch? = nil,
        versionExecutableURL: @escaping VersionExecutableURLProvider = { JournalCommandLine.currentExecutableURL() },
        aboutOSVersion: @escaping AboutStringProvider = {
            SolstoneCoreAbout.numericOSVersion(ProcessInfo.processInfo.operatingSystemVersion)
        },
        aboutArch: @escaping AboutArchProvider = { SolstoneCoreAbout.nativeMacOSArch() },
        appBuild: String? = Bundle.main.infoDictionary?["CFBundleVersion"] as? String,
        devicesModel: JournalDevicesModel? = nil,
        onIdentityMark: IdentityMarkObserver? = nil,
        now: @escaping NowProvider = { Date() },
        diskCacheDuration: TimeInterval = 30
    ) {
        let defaultIdentityFetcher = JournalIdentityFetcher(session: identitySession, dataLoader: dataLoader)
        let trimmedBaseURL = baseURL.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        self.checkLocalAccess = checkLocalAccess
        self.browserLink = browserLink
        self.config = config
        self.supervisor = supervisor
        self.baseURL = trimmedBaseURL
        self.fetchIdentity = fetchIdentity ?? { baseURL in
            await defaultIdentityFetcher.fetch(baseURL: baseURL)
        }
        self.identityRetryPause = identityRetryPause
        self.fetchDiskUsage = fetchDiskUsage ?? { await JournalDiskUsage.calculateBytes(under: $0) }
        self.fetchHealth = fetchHealth ?? { binary, environment in
            await JournalHealthCheck.run(journalBinary: binary, environment: environment)
        }
        self.fetchVersion = fetchVersion ?? { binary, environment in
            await JournalHealthCheck.version(journalBinary: binary, environment: environment)
        }
        self.versionExecutableURL = versionExecutableURL
        self.aboutOSVersion = aboutOSVersion
        self.aboutArch = aboutArch
        self.appBuild = appBuild.flatMap { $0.isEmpty ? nil : $0 }
        self.now = now
        self.diskCacheDuration = diskCacheDuration
        self.onIdentityMark = onIdentityMark
        self.devicesModel = devicesModel ?? JournalDevicesModel(client: JournalDevicesClient(baseURL: trimmedBaseURL, dataLoader: dataLoader))
        self.devicesModel.markPresentation = markPresentation
    }

    var aboutBlock: String {
        SolstoneCoreAbout.renderLine(
            name: "journal",
            version: journalVersion == "unknown" ? nil : journalVersion,
            build: appBuild,
            os: "macos",
            osVersion: aboutOSVersion(),
            arch: aboutArch()
        )
    }

    var isConfigured: Bool {
        config.journalRoot != nil
    }

    var journalRootPath: String {
        config.journalRoot?.path ?? "not set"
    }

    var launchAtLoginEnabled: Bool {
        config.launchAtLoginEnabled
    }

    private(set) var launchAtLoginFailure: String?

    func noteLaunchAtLoginApplied() {
        launchAtLoginFailure = config.launchAtLoginFailureSentence
    }

    var runDisplay: JournalRunDisplay {
        JournalRunDisplay.derive(state: supervisor.state, runtimeStatus: supervisor.runtimeStatus)
    }

    var markPresentation: JournalMarkPresentation {
        guard isConfigured else { return .generic }
        guard localAccessFailure == nil else { return .unavailable }
        guard let identityRead else { return .loading }
        switch identityRead {
        case .mark(let mark): return .mark(mark)
        case .uncommitted: return .generic
        case .unavailable: return .unavailable
        }
    }

    var unconfiguredMessage: String? {
        isConfigured ? nil : "nothing here yet. creating your journal comes next."
    }

    /// The runner's own reason for a stop it chose, shown above start. Other stop diagnostics carry
    /// raw command output and stay out of the window.
    var stoppedReason: String? {
        guard runDisplay == .stopped,
              case .stopped(let diagnostic) = supervisor.runtimeStatus,
              let excerpt = diagnostic.outputExcerpt,
              Self.ownerStopReasons.contains(excerpt) else { return nil }
        return excerpt
    }

    private static let ownerStopReasons: Set<String> = [
        UICopy.JOURNAL_CHILD_CONTAINMENT_UNRESOLVED,
        UICopy.JOURNAL_CHILD_BREAKER_TRIPPED,
        UICopy.JOURNAL_SPAWN_BLOCKED_LEGACY_SERVICE,
        UICopy.JOURNAL_SPAWN_SERVICE_CHECK_FAILED,
        UICopy.JOURNAL_SPAWN_BLOCKED_PORTS,
        UICopy.JOURNAL_SPAWN_PORT_CHECK_FAILED,
    ]

    var homeOffer: JournalHomeOffer {
        guard isConfigured else { return .unconfigured }
        guard localAccessFailure == nil else { return .runState }
        switch runDisplay {
        case .running:
            return .door
        case .stopped:
            return .start
        case .starting:
            return .none
        case .blocked, .unknown:
            return .runState
        }
    }

    @discardableResult
    func openJournal(using openURL: @escaping @MainActor (URL) -> Bool) -> Task<Void, Never> {
        openBrowser(path: "/", using: openURL)
    }

    @discardableResult
    func openBackup(using openURL: @escaping @MainActor (URL) -> Bool) -> Task<Void, Never> {
        openBrowser(path: "/app/backup", using: openURL)
    }

    /// Opens a one-time link from the checked journal. There is no plain-address fallback: a journal
    /// with local access closed answers a plain address with its landing page.
    private func openBrowser(path: String, using openURL: @escaping @MainActor (URL) -> Bool) -> Task<Void, Never> {
        Task { @MainActor in
            do {
                let link = try await browserLink(path)
                openFailure = nil
                if !openURL(link) {
                    Logger.journalApp.error("failed to open the journal at \(path, privacy: .public) in the browser")
                }
            } catch LocalHTTPError.linkFailed {
                openFailure = LocalHTTPError.linkFailed.localizedDescription
            } catch {
                localAccessFailure = error.localizedDescription
                identityRead = .unavailable
            }
        }
    }

    var diskUsageValue: String {
        guard let diskUsageBytes else { return "unknown" }
        return ByteCountFormatter.string(fromByteCount: diskUsageBytes, countStyle: .file)
    }

    func prepareForWindowOpen() {
        selectedPane = .home
        identityFetchCompleted = false
        if identityRead == .unavailable {
            identityRead = nil
        }
        devicesModel.markPresentation = markPresentation
        devicesModel.resetTransientState()
    }

    func loadForWindowOpen() async {
        guard isConfigured, supervisor.state == .running else { return }
        await fetchIdentityIfNeeded()
    }

    /// Forgets any identity read from before the first run finished. The caller then reads the
    /// journal's identity again, which reports a committed mark through `onIdentityMark`.
    func applyFirstRunLanding() {
        identityLandingGeneration += 1
        identityFetchCompleted = false
        identityRead = isConfigured ? nil : .uncommitted
        devicesModel.markPresentation = markPresentation
    }

    func handlePaneOpen(_ pane: JournalPane) {
        switch pane {
        case .journal:
            invalidateDiskUsage()
            Task { await loadDiskUsageIfNeeded() }
        case .runState:
            Task { await refreshRunState() }
        case .devices:
            Task { await devicesModel.loadDevices() }
        case .home, .backup, .startup, .updates:
            break
        }
    }

    func fetchIdentityIfNeeded() async {
        if let checkLocalAccess {
            do { try await checkLocalAccess(); localAccessFailure = nil }
            catch {
                localAccessFailure = error.localizedDescription
                identityRead = .unavailable
                devicesModel.markPresentation = .unavailable
                return
            }
        }
        guard !identityFetchCompleted else { return }
        while let identityFetchTask {
            await identityFetchTask.value
            guard !identityFetchCompleted, identityRead == nil else { return }
        }
        let generation = identityLandingGeneration
        let task = Task { @MainActor in
            defer { identityFetchTask = nil }
            for attempt in 0..<3 {
                let read = await fetchIdentity(baseURL)
                guard generation == identityLandingGeneration else { return }
                if read == .unavailable, attempt < 2 {
                    do {
                        try await identityRetryPause()
                    } catch {
                        return
                    }
                    guard generation == identityLandingGeneration else { return }
                    continue
                }
                identityRead = read
                identityFetchCompleted = read != .unavailable
                devicesModel.markPresentation = markPresentation
                if case .mark(let mark) = read, let validatedMark = JournalMark.validate(mark) {
                    onIdentityMark?(validatedMark)
                }
                return
            }
        }
        identityFetchTask = task
        await task.value
    }

    func setLaunchAtLoginEnabled(_ enabled: Bool) {
        _ = config.setLaunchAtLoginEnabled(enabled)
        launchAtLoginFailure = config.launchAtLoginFailureSentence
    }

    func startJournal() {
        guard let root = config.journalRoot else { return }
        Task {
            _ = await supervisor.start(journalRoot: root)
            await refreshRunState()
        }
    }

    func stopJournal() {
        Task {
            _ = await supervisor.stop()
            await refreshRunState()
        }
    }

    func restartJournal() {
        Task {
            _ = await supervisor.restart()
            await refreshRunState()
        }
    }

    func loadDiskUsageIfNeeded() async {
        guard let root = config.journalRoot else {
            diskUsageBytes = nil
            return
        }
        if let diskUsageLoadedAt, now().timeIntervalSince(diskUsageLoadedAt) < diskCacheDuration {
            return
        }
        diskUsageBytes = await fetchDiskUsage(root)
        diskUsageLoadedAt = now()
    }

    func invalidateDiskUsage() {
        diskUsageLoadedAt = nil
    }

    func refreshRunState() async {
        if let binary = supervisor.journalBinaryURL {
            let environment = supervisor.journalRuntimeEnvironment
            switch await fetchHealth(binary, environment) {
            case .healthy:
                healthDisplay = .healthy
            case .stopped:
                healthDisplay = .stopped
            case .unknown:
                healthDisplay = .unknown
            }
        } else {
            healthDisplay = .unknown
        }

        if let executableURL = versionExecutableURL() {
            let commandURL = JournalCommandLine.commandLineURL(executableURL: executableURL)
            journalVersion = await fetchVersion(commandURL, nil) ?? "unknown"
        } else {
            journalVersion = "unknown"
        }
    }
}
