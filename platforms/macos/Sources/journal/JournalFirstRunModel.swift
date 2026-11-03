// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import AppKit
import Foundation
import JournalMarkKit
import JournalRuntime
import Observation
import os
import SolstoneCore
import SwiftUI

enum JournalFirstRunRoute: Equatable {
    case deciding
    case ritual(JournalFirstRunStep)
    case adopting
    case home
}

enum JournalFirstRunStep: Equatable {
    case welcome
    case folder
    case nameLocation
    case mark
    case thinking
    case setupProgress
    case finishInBrowser
}

protocol JournalSetupRunning: Sendable {
    func run(
        journalRoot: URL,
        skipService: Bool,
        skipModelInstall: Bool,
        progress: @escaping @Sendable (JournalSetupProgressEvent) async -> Void
    ) async throws -> JournalSetupResult
}

extension JournalSetupRunner: JournalSetupRunning {}

protocol JournalInitClienting: Sendable {
    func probeSetupComplete() async throws -> JournalInitSetupProbe
}

extension JournalInitClient: JournalInitClienting {}

@MainActor
@Observable
final class JournalFirstRunModel {
    typealias SupervisorStart = @MainActor @Sendable (URL) async -> Bool
    typealias URLOpener = @MainActor (URL) -> Bool

    static let defaultInitPageURL = URL(string: "http://127.0.0.1:5015/init")!
    static let setupBaseURL = "http://127.0.0.1:5015"

    @ObservationIgnored private let config: JournalAppConfig
    @ObservationIgnored private let setupRunner: any JournalSetupRunning
    @ObservationIgnored private let initClient: any JournalInitClienting
    @ObservationIgnored private let startSupervisor: SupervisorStart
    @ObservationIgnored private let handoffStore: any JournalHandoffStoring
    @ObservationIgnored private let journalFileReader: any OnDiskJournalFileReading
    @ObservationIgnored private let discoveryQualificationTimeout: TimeInterval
    @ObservationIgnored private let browserURL: (@Sendable (String) async throws -> URL)?
    @ObservationIgnored private let initPageURL: URL
    @ObservationIgnored private let openURL: URLOpener
    @ObservationIgnored private let browserPollInterval: Duration
    @ObservationIgnored private weak var windowModel: JournalWindowModel?
    @ObservationIgnored private let supervisorIsLive: @MainActor (URL) -> Bool
    @ObservationIgnored private let externalSupervisorIsLive: @Sendable (URL) -> Bool
    @ObservationIgnored private let loadSetupCopy: @Sendable () async -> JournalSetupCopy?
    @ObservationIgnored private let localSetupCapability: @Sendable () -> Bool
    @ObservationIgnored private let stepAsideAfterHandoff: @MainActor () -> Void
    @ObservationIgnored private let peer: any JournalSetupPeerResolving
    @ObservationIgnored private let setupClient: any JournalSetupClienting
    @ObservationIgnored private let identity: any JournalIdentityResolving
    @ObservationIgnored private let mailbox: SetupMailboxStore
    @ObservationIgnored private let continuation: JournalSetupContinuationStore
    @ObservationIgnored private let refreshSleep: @Sendable () async -> Void

    var route: JournalFirstRunRoute = .deciding
    /// Set by back from the folder step so returning to it keeps the owner's chosen folder.
    private var keepsChosenRootOnReturn = false
    var journalRoot: URL {
        didSet { noteSelectedRootChange(from: oldValue) }
    }
    var setupEvents: [JournalSetupProgressEvent] = []
    var setupRenderedLog = ""
    var currentStep: String?
    var adoptMessage: String?
    var errorMessage: String?
    private(set) var handoffRefusal: JournalSetupRefusalResolution?
    var handoffError: String?
    var loginFailure: String?
    var markFailure: String?
    var thinkingFailure: String?
    var speechFailure: String?
    var nativeFolderSetup = false
    var welcomeLaunchAtLogin: Binding<Bool> {
        Binding(
            get: { [weak self] in self?.launchAtLoginEnabled ?? false },
            set: { [weak self] in self?.setWelcomeLaunchAtLogin($0) }
        )
    }
    private(set) var draftThinkingChoice: JournalThinkingChoice?
    private(set) var reopenCount = 0
    private(set) var setupCopy: JournalSetupCopy?
    private(set) var setupState: JournalSetupState?
    private(set) var displayedProposalID: String?
    private(set) var epoch: UInt64 = 0
    private(set) var pendingOperation: JournalRitualOperationToken = .none
    private(set) var operationError: JournalRitualOperationToken = .none
    private(set) var userOperationInFlight = false
    private(set) var outbound: JournalOutboundToken = .none

    @ObservationIgnored private var setupTask: Task<JournalSetupResult, Error>?
    @ObservationIgnored private var browserWaitTask: Task<Void, Never>?
    @ObservationIgnored private var browserWaitGeneration = 0
    @ObservationIgnored private var refreshTask: Task<Void, Never>?
    @ObservationIgnored private var actionInFlight = false
    @ObservationIgnored private var nativeQualified = false
    @ObservationIgnored private var didAutoRegenerate = false
    @ObservationIgnored private var suppressRootEpoch = false
    @ObservationIgnored private var wholeOperationDepth = 0
    @ObservationIgnored private var actionEpoch: UInt64?
    @ObservationIgnored private var activationDrainScheduled = false
    @ObservationIgnored private var activationRunning = false
    @ObservationIgnored private var activationFollowUp = false
    @ObservationIgnored private var activationWaiters: [CheckedContinuation<Void, Never>] = []
    @ObservationIgnored var onBeforeStep1Commit: (@MainActor () -> Void)?
    private var isTerminating = false

    private var pendingDiscoveryHandoff: JournalHandoff?

    static func yieldActivationThenHide(
        yield: @MainActor (String) -> Void = { bundleIdentifier in
            NSApp?.yieldActivation(toApplicationWithBundleIdentifier: bundleIdentifier)
        },
        hide: @MainActor () -> Void = { NSApp?.hide(nil) }
    ) {
        yield(JournalSetupPeerResolver.bundleIdentifier)
        hide()
    }

    #if DEBUG
    func applySnapshotState(
        route: JournalFirstRunRoute,
        setupState: JournalSetupState? = nil,
        setupCopy: JournalSetupCopy? = nil,
        handoffRefusal: JournalSetupRefusalResolution? = nil,
        draftThinkingChoice: JournalThinkingChoice? = nil,
        loginFailure: String? = nil,
        errorMessage: String? = nil,
        thinkingFailure: String? = nil,
        speechFailure: String? = nil,
        operationError: JournalRitualOperationToken = .none,
        pendingOperation: JournalRitualOperationToken = .none
    ) {
        self.route = route
        self.pendingOperation = pendingOperation
        self.setupState = setupState
        self.setupCopy = setupCopy
        self.handoffRefusal = handoffRefusal
        self.draftThinkingChoice = draftThinkingChoice
        self.loginFailure = loginFailure
        self.errorMessage = errorMessage
        self.thinkingFailure = thinkingFailure
        self.speechFailure = speechFailure
        self.operationError = operationError
    }
    #endif

    init(
        config: JournalAppConfig,
        setupRunner: any JournalSetupRunning = JournalSetupRunner(materializer: NativeJournalRuntimeMaterializer()),
        initClient: any JournalInitClienting = JournalInitClient(),
        startSupervisor: @escaping SupervisorStart,
        handoffStore: any JournalHandoffStoring = JournalHandoffStore(),
        journalFileReader: any OnDiskJournalFileReading = LiveOnDiskJournalFileReader(),
        discoveryQualificationTimeout: TimeInterval = 1.0,
        initPageURL: URL = JournalFirstRunModel.defaultInitPageURL,
        browserURL: (@Sendable (String) async throws -> URL)? = nil,
        openURL: @escaping URLOpener = { NSWorkspace.shared.open($0) },
        browserPollInterval: Duration = .seconds(2),
        windowModel: JournalWindowModel? = nil,
        supervisorIsLive: @escaping @MainActor (URL) -> Bool = { _ in false },
        externalSupervisorIsLive: @escaping @Sendable (URL) -> Bool = {
            liveSupervisorMatchesReadyFiles(journalRoot: $0)
        },
        loadSetupCopy: @escaping @Sendable () async -> JournalSetupCopy? = {
            do {
                return try await JournalSetupCopyLoader().load()
            } catch {
                return nil
            }
        },
        localSetupCapability: @escaping @Sendable () -> Bool = {
            journalSupportsSetupSequence(Bundle.main.object(forInfoDictionaryKey: "SolstoneSetupCapabilities"))
        },
        peer: any JournalSetupPeerResolving = JournalSetupPeerResolver(),
        // Once setup continues in the solstone app, yield activation to that app
        // and then hide. Hiding alone leaves focus with whichever app was frontmost.
        stepAsideAfterHandoff: @escaping @MainActor () -> Void = {
            JournalFirstRunModel.yieldActivationThenHide()
        },
        setupClient: any JournalSetupClienting = JournalSetupClient(),
        identity: any JournalIdentityResolving = JournalIdentityResolver(),
        mailbox: SetupMailboxStore = SetupMailboxStore(),
        continuation: JournalSetupContinuationStore = JournalSetupContinuationStore(),
        refreshSleep: @escaping @Sendable () async -> Void = {
            try? await Task.sleep(for: .seconds(1))
        }
    ) {
        self.config = config
        self.setupRunner = setupRunner
        self.initClient = initClient
        self.startSupervisor = startSupervisor
        self.handoffStore = handoffStore
        self.journalFileReader = journalFileReader
        self.discoveryQualificationTimeout = discoveryQualificationTimeout
        self.browserURL = browserURL
        self.initPageURL = initPageURL
        self.openURL = openURL
        self.browserPollInterval = browserPollInterval
        self.windowModel = windowModel
        self.supervisorIsLive = supervisorIsLive
        self.externalSupervisorIsLive = externalSupervisorIsLive
        self.loadSetupCopy = loadSetupCopy
        self.localSetupCapability = localSetupCapability
        self.peer = peer
        self.stepAsideAfterHandoff = stepAsideAfterHandoff
        self.setupClient = setupClient
        self.identity = identity
        self.mailbox = mailbox
        self.continuation = continuation
        self.refreshSleep = refreshSleep

        self.journalRoot = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("journal", isDirectory: true)
    }

    /// Legacy reopen only raises the window. Native setup still ingests peer continuations.
    func reconcileForReopen() async {
        let incoming: Bool
        if case .ready = mailbox.loadStep1() { incoming = true } else { incoming = false }
        if case .ritual = route, !incoming, handoffError == nil, handoffRefusal == nil { return }
        guard nativeQualified || incoming else { return }
        await reconcileActivation()
    }

    /// A second call waits and runs one follow-up body.
    func reconcileActivation() async {
        guard !isTerminating else { return }
        if activationRunning || wholeOperationDepth > 0 {
            activationFollowUp = true
            await withCheckedContinuation { continuation in
                activationWaiters.append(continuation)
            }
            return
        }
        activationRunning = true
        defer {
            activationRunning = false
            if wholeOperationDepth == 0 || isTerminating { settleActivationWaiters() }
        }
        repeat {
            activationFollowUp = false
            await reconcileActivationBody()
        } while activationFollowUp && wholeOperationDepth == 0 && !isTerminating && !Task.isCancelled
    }

    private func settleActivationWaiters() {
        let waiters = activationWaiters
        activationWaiters.removeAll()
        for waiter in waiters { waiter.resume() }
    }

    private func finishWholeOperation() {
        precondition(wholeOperationDepth > 0)
        wholeOperationDepth -= 1
        guard wholeOperationDepth == 0 else { return }
        userOperationInFlight = false
        if isTerminating {
            settleActivationWaiters()
        } else if activationFollowUp, !activationRunning, !activationDrainScheduled {
            activationDrainScheduled = true
            Task { @MainActor [weak self] in
                guard let self else { return }
                self.activationDrainScheduled = false
                await self.reconcileActivation()
            }
        }
    }

    private func reconcileActivationBody() async {
        epoch &+= 1
        let captured = epoch
        pendingDiscoveryHandoff = nil
        didAutoRegenerate = false
        guard stillCurrent(captured) else { return }
        if await qualifyForNativeSetup() {
            nativeQualified = true
            guard stillCurrent(captured) else { return }
            await reconcileQualified(captured: captured)
            return
        }
        if let preserved = config.journalRoot?.standardizedFileURL { assignRoot(preserved) }
        nativeQualified = false
        setupCopy = nil
        guard stillCurrent(captured) else { return }
        await decideLegacyLaunchRoute()
    }

    private func stillCurrent(_ captured: UInt64) -> Bool {
        epoch == captured && !Task.isCancelled && !isTerminating
    }

    private func assignRoot(_ root: URL) {
        suppressRootEpoch = true
        journalRoot = root.standardizedFileURL
        suppressRootEpoch = false
    }

    private func noteSelectedRootChange(from oldValue: URL) {
        guard journalRoot.standardizedFileURL.path != oldValue.standardizedFileURL.path else { return }
        guard !suppressRootEpoch else { return }
        draftThinkingChoice = nil
        epoch &+= 1
    }

    private func decideLegacyLaunchRoute() async {
        do {
            guard let handoff = try handoffStore.load() else {
                await continueWithoutHandoff()
                return
            }

            switch handoff.provenance {
            case JournalHandoffProvenance.bundledMigration:
                route = .adopting
                await adoptFromHandoff()
            case JournalHandoffProvenance.observerDiscovery:
                await routeDiscoveryHandoff(handoff)
            default:
                consumeHandoffBestEffort()
                beginCreateAtDefaultLocation()
            }
        } catch {
            consumeHandoffBestEffort()
            beginCreateAtDefaultLocation()
        }
    }

    private func continueWithoutHandoff() async {
        guard let root = config.journalRoot else {
            beginCreate()
            return
        }

        await resumeConfiguredRoot(root)
    }

    private func routeDiscoveryHandoff(_ handoff: JournalHandoff) async {
        let root = URL(fileURLWithPath: handoff.journalRootPath, isDirectory: true)
            .standardizedFileURL
        let fileReader = journalFileReader
        let timeout = discoveryQualificationTimeout
        let qualifies: Bool
        do {
            qualifies = try await withTimeout(seconds: timeout) {
                await journalDirectoryQualifies(at: root.path, using: fileReader)
            }
        } catch {
            qualifies = false
        }

        guard qualifies else {
            consumeHandoffBestEffort()
            beginCreateAtDefaultLocation()
            return
        }

        assignRoot(root)
        pendingDiscoveryHandoff = handoff
        errorMessage = nil
        adoptMessage = nil
        route = .ritual(.nameLocation)
    }

    private func consumeHandoffBestEffort() {
        try? handoffStore.consume()
    }

    private func beginCreateAtDefaultLocation() {
        errorMessage = nil
        adoptMessage = nil
        assignRoot(defaultJournalRoot)
        config.journalRoot = nil
        route = .ritual(.nameLocation)
    }

    private var defaultJournalRoot: URL {
        FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("journal", isDirectory: true)
            .standardizedFileURL
    }

    func beginCreate() {
        errorMessage = nil
        adoptMessage = nil
        if let configuredRoot = config.journalRoot {
            assignRoot(configuredRoot)
        } else {
            assignRoot(defaultJournalRoot)
        }
        route = .ritual(.nameLocation)
    }

    func continueFromNameLocation() async {
        if pendingDiscoveryHandoff != nil {
            consumeHandoffBestEffort()
            pendingDiscoveryHandoff = nil
        }
        config.journalRoot = journalRoot.standardizedFileURL
        if nativeQualified {
            guard beginUserAction(.setup) else { return }

            defer { finishWholeOperation() }
            await runQualifiedSetup(skipModelInstall: true, captured: epoch, actionOpen: true)
            return
        }
        await runSetupThenStartSupervisor()
    }

    func runSetupThenStartSupervisor() async {
        route = .ritual(.setupProgress)
        errorMessage = nil

        do {
            try await runSetup(at: journalRoot, skipModelInstall: false)
            guard await startSupervisor(journalRoot.standardizedFileURL) else {
                errorMessage = supervisorFailureMessage()
                return
            }
            await routeAfterProbe()
        } catch {
            errorMessage = readableMessage(for: error)
        }
    }

    func prepareForTermination() async {
        isTerminating = true
        settleActivationWaiters()
        epoch &+= 1
        stopRefresh()
        browserWaitTask?.cancel()
        browserWaitTask = nil
        let task = setupTask
        task?.cancel()
        _ = try? await task?.value
    }

    func resetAfterFailedUpdaterInstall() {
        isTerminating = false
        if route == .ritual(.finishInBrowser) {
            startBrowserWaitIfNeeded()
        }
    }

    private func runSetup(at journalRoot: URL, skipModelInstall: Bool) async throws {
        guard !isTerminating, setupTask == nil else { throw CancellationError() }
        setupEvents = []
        setupRenderedLog = ""
        currentStep = nil

        let task = Task { [setupRunner, weak self] in
            try await setupRunner.run(
                journalRoot: journalRoot,
                skipService: true,
                skipModelInstall: skipModelInstall,
                progress: { [weak self] event in
                    await MainActor.run { self?.applySetupEvent(event) }
                }
            )
        }
        setupTask = task
        defer { setupTask = nil }
        let result = try await withTaskCancellationHandler {
            try await task.value
        } onCancel: {
            task.cancel()
        }
        try Task.checkCancellation()
        guard !isTerminating, !task.isCancelled else { throw CancellationError() }
        setupRenderedLog = result.renderedLog
    }

    func resumeConfiguredRoot(_ root: URL) async {
        assignRoot(root)
        config.journalRoot = journalRoot
        errorMessage = nil

        guard await ensureSupervisor(journalRoot) else {
            await runSetupThenStartSupervisor()
            return
        }

        await routeAfterProbe()
    }

    func adoptFromHandoff() async {
        route = .adopting
        adoptMessage = JournalFirstRunCopy.adoptOpening
        errorMessage = nil

        do {
            guard let handoff = try handoffStore.load() else {
                beginCreate()
                return
            }
            let root = URL(fileURLWithPath: handoff.journalRootPath, isDirectory: true).standardizedFileURL
            assignRoot(root)
            config.journalRoot = root

            try await runSetup(at: root, skipModelInstall: false)
            guard await startSupervisor(root) else {
                errorMessage = supervisorFailureMessage()
                return
            }

            try await completeAdoptAfterSuccessfulStart()
        } catch {
            errorMessage = readableMessage(for: error)
        }
    }

    /// Qualified adoption retries stay on the native ceremony. Legacy adoption repeats the browser handoff.
    func retryAdopt() async {
        guard !isTerminating else { return }
        if nativeQualified {
            epoch &+= 1
            await adoptQualified(captured: epoch)
        } else {
            await adoptFromHandoff()
        }
    }

    private func completeAdoptAfterSuccessfulStart() async throws {
        let probe = try await initClient.probeSetupComplete()
        try handoffStore.consume()
        adoptMessage = JournalFirstRunCopy.adoptLandingLine
        switch probe {
        case .complete:
            await landHome()
        case .incomplete:
            await finishInBrowser()
        }
    }

    private func routeAfterProbe() async {
        let probe: JournalInitSetupProbe
        do {
            probe = try await initClient.probeSetupComplete()
        } catch {
            errorMessage = readableMessage(for: error)
            return
        }
        switch probe {
        case .complete:
            await landHome()
        case .incomplete:
            await finishInBrowser()
        }
    }

    /// The journal's own first-run page finishes creating the journal: it asks for the mark and
    /// the owner's choices, then records the first run as done. This app only opens it in the
    /// owner's browser and waits for the journal to report it done.
    private func finishInBrowser() async {
        errorMessage = nil
        route = .ritual(.finishInBrowser)
        openInitPage()
        await startBrowserWaitIfNeeded().value
    }

    /// Opens the page again, for an owner who closed it before finishing.
    func reopenInBrowser() {
        reopenCount += 1
        openInitPage()
        startBrowserWaitIfNeeded()
    }

    private func openInitPage() {
        if let browserURL {
            Task { @MainActor in
                do { _ = openURL(try await browserURL("/init")) }
                catch { errorMessage = error.localizedDescription }
            }
            return
        }
        if !openURL(initPageURL) {
            Logger.journalApp.error("failed to open \(self.initPageURL.absoluteString, privacy: .public) in the browser")
        }
    }

    @discardableResult
    private func startBrowserWaitIfNeeded() -> Task<Void, Never> {
        if let browserWaitTask {
            return browserWaitTask
        }
        browserWaitGeneration += 1
        let generation = browserWaitGeneration
        let task = Task { @MainActor [weak self] in
            await self?.waitForSetupComplete()
            if self?.browserWaitGeneration == generation {
                self?.browserWaitTask = nil
            }
        }
        browserWaitTask = task
        return task
    }

    private func waitForSetupComplete() async {
        while !Task.isCancelled, !isTerminating {
            do {
                if try await initClient.probeSetupComplete() == .complete {
                    guard !Task.isCancelled, !isTerminating else { return }
                    await landHome()
                    return
                }
            } catch {
                Logger.journalApp.debug("first-run probe while waiting on the browser: \(String(describing: error), privacy: .public)")
            }
            do {
                try await Task.sleep(for: browserPollInterval)
            } catch {
                return
            }
        }
    }

    private func landHome() async {
        stopRefresh()
        actionInFlight = false
        handoffError = nil
        windowModel?.applyFirstRunLanding()
        route = .home
        await windowModel?.fetchIdentityIfNeeded()
    }

    private func applySetupEvent(_ event: JournalSetupProgressEvent) {
        setupEvents.append(event)
        switch event {
        case .stepStarted(let step, _, _):
            currentStep = step
        case .stepFailed(let step, _, let message):
            currentStep = step
            errorMessage = message
        case .warning, .completed:
            break
        }
    }

    private func supervisorFailureMessage() -> String {
        windowModel?.supervisor.blockedReason
            ?? JournalFirstRunCopy.adoptFailed
    }

    private func readableMessage(for error: Error) -> String {
        if let runnerError = error as? JournalSetupRunnerError {
            switch runnerError {
            case .gateBlocked(let blockage):
                return blockage.ownerMessage
            case .materializeFailed(let message),
                 .runtimeDirectoryFailed(let message),
                 .subprocessLaunchFailed(let message),
                 .setupFailed(_, let message):
                return message
            case .timedOut:
                return "setup timed out"
            case .duplicateCompletion:
                return "setup reported completion twice"
            case .missingCompletion:
                return "setup did not finish"
            }
        }
        return error.localizedDescription
    }

    var isRefreshLoopRunning: Bool { refreshTask != nil }

    /// Reads through to the saved setting, which Observation cannot see on its own: the read is
    /// registered here and the write in `setWelcomeLaunchAtLogin`, so a control bound to it redraws.
    var launchAtLoginEnabled: Bool {
        access(keyPath: \.launchAtLoginEnabled)
        return config.launchAtLoginEnabled
    }

    var validatedMark: JournalMark? {
        guard let render = setupState?.mark.render else { return nil }
        return JournalMark.validate(render)
    }

    /// A locked journal has a mark even when this read of it can't be drawn. Anything else without
    /// a drawable mark has no identity to show yet.
    var lockedMarkIsUnreadable: Bool {
        validatedMark == nil && setupState?.mark.state == .locked
    }

    var canCommitDisplayedMark: Bool {
        guard handoffError == nil, setupState?.mark.state == .proposed,
              validatedMark != nil, let displayedProposalID, !displayedProposalID.isEmpty else { return false }
        return setupState?.mark.proposalID == displayedProposalID
    }

    var offersLocalReady: Bool {
        guard handoffError == nil, case .ritual(.thinking) = route else { return false }
        guard setupState?.thinking.saved == false else { return false }
        return setupState?.thinking.capability == .ok
    }

    var canFinalizeThinking: Bool {
        handoffError == nil
            && setupState?.mark.state == .locked
            && setupState?.thinking.saved == true
    }

    var displayedThinkingChoice: JournalThinkingChoice? {
        if let draftThinkingChoice { return draftThinkingChoice }
        if setupState?.thinking.saved == true { return setupState?.thinking.choice }
        return offersLocalThinking ? .local : .later
    }

    /// Local thinking is offered only when this computer is known to run it; otherwise it is not shown at all.
    var offersLocalThinking: Bool { setupState?.thinking.capability == .ok }

    var canContinueThinking: Bool {
        guard handoffError == nil, setupState?.mark.state == .locked,
              let choice = displayedThinkingChoice else { return false }
        return choice != .local || offersLocalThinking
    }

    func selectThinking(_ raw: String) {
        guard !userOperationInFlight, !isTerminating, let choice = JournalThinkingChoice(rawValue: raw),
              choice != .local || offersLocalThinking else { return }
        draftThinkingChoice = choice
    }

    func continueThinking() async {
        guard wholeOperationDepth == 0, !isTerminating,
              canContinueThinking, let choice = displayedThinkingChoice else { return }
        wholeOperationDepth += 1
        userOperationInFlight = true
        defer { finishWholeOperation() }
        let root = journalRoot.standardizedFileURL
        if setupState?.thinking.saved != true || setupState?.thinking.choice != choice {
            guard await saveThinking(choice.rawValue, continuing: true), journalRoot.standardizedFileURL == root,
                  setupState?.thinking.saved == true, setupState?.thinking.choice == choice else { return }
        }
        await finalizeThinking(continuing: true)
    }

    var showsMarkRegenerate: Bool {
        guard handoffError == nil, case .ritual(.mark) = route else { return false }
        guard let mark = setupState?.mark else { return false }
        switch mark.state {
        case .locked:
            return false
        case .unavailable:
            return mark.render == nil
        case .proposed:
            return true
        }
    }

    var showsSpeechRetry: Bool { setupState?.downloads.speech.state == .error }

    var showsThinkingRetry: Bool {
        let state = setupState?.downloads.thinking.state
        return state == .error || state == .cancelled
    }

    var showsThinkingCancel: Bool { canCancel(setupState?.downloads.thinking) }

    var selectedThinkingDescription: String? {
        guard let choice = displayedThinkingChoice else { return nil }
        return setupCopy?.thinking.choices.first { $0.id == choice.rawValue }?.description
    }

    var thinkingSizeLine: String {
        guard let copy = setupCopy else { return "" }
        let replacement: String
        if let bytes = setupState?.thinking.downloadBytes {
            replacement = ByteCountFormatter.string(fromByteCount: Int64(clamping: bytes), countStyle: .file)
        } else {
            replacement = copy.thinking.sizeUnknown
        }
        return copy.thinking.defaultLine.replacingOccurrences(of: "{size}", with: replacement)
    }

    func downloadLabel(for state: JournalDownloadJobState) -> String {
        guard let downloads = setupCopy?.downloads else { return "" }
        switch state {
        case .downloading: return downloads.progress
        case .idle: return downloads.idle
        case .complete: return downloads.complete
        case .cancelled: return downloads.cancelled
        case .error: return downloads.error
        }
    }

    func downloadProgressText(for job: JournalSetupState.Job) -> String? {
        switch job.state {
        case .complete:
            break
        case .downloading:
            guard let attempt = job.attemptID, !attempt.isEmpty else { return nil }
        case .idle, .cancelled, .error:
            return nil
        }
        guard let total = job.bytesTotal, total > 0 else {
            return formattedByteCount(job.bytesDone)
        }
        return "\(formattedByteCount(job.bytesDone)) / \(formattedByteCount(total))"
    }

    func setWelcomeLaunchAtLogin(_ enabled: Bool) {
        withMutation(keyPath: \.launchAtLoginEnabled) {
            _ = config.setLaunchAtLoginEnabled(enabled)
        }
        loginFailure = config.launchAtLoginFailureSentence
    }

    func continueFromWelcome() {
        handoffError = nil
        // Back from the folder step keeps the folder the owner had chosen.
        if config.journalRoot == nil && !keepsChosenRootOnReturn {
            assignRoot(defaultJournalRoot)
        }
        keepsChosenRootOnReturn = false
        route = .ritual(.folder)
    }

    func backFromFolder() {
        guard JournalSetupChrome.canGoBackFromFolder(
            route: route,
            userOperationInFlight: userOperationInFlight,
            pendingOperation: pendingOperation,
            outbound: outbound,
            hasRefusal: handoffRefusal != nil,
            handoffError: handoffError
        ) else { return }
        keepsChosenRootOnReturn = true
        errorMessage = nil
        route = .ritual(.welcome)
    }

    func continueFromFolder() async {
        guard beginUserAction(.setup) else { return }

        defer { finishWholeOperation() }
        let captured = epoch
        let root = journalRoot.standardizedFileURL
        do {
            if let active = try continuation.loadActive() {
                try continuation.bindRoot(step1: active.owner, root: root)
            }
        } catch {
            if failUserAction(captured, operation: .setup) {
                errorMessage = JournalFirstRunCopy.setupProgressNotSaved
            }
            return
        }
        assignRoot(root)
        config.journalRoot = root
        nativeFolderSetup = true
        guard stillCurrent(captured) else {
            _ = endUserAction(captured)
            return
        }
        await runQualifiedSetup(skipModelInstall: true, captured: captured, actionOpen: true)
    }

    func retrySetup() async {
        if nativeQualified {
            guard beginUserAction(.setup) else { return }

            defer { finishWholeOperation() }
            await runQualifiedSetup(skipModelInstall: true, captured: epoch, actionOpen: true)
        } else {
            await runSetupThenStartSupervisor()
        }
    }

    func openJournalAfterRefusal() async {
        guard let resolution = handoffRefusal, beginUserAction(.outbound) else { return }
        let captured = epoch
        defer {
            endUserAction(captured)
            finishWholeOperation()
        }
        let root = URL(fileURLWithPath: resolution.journalRootPath, isDirectory: true)
        guard config.journalRoot?.standardizedFileURL.path == root.path else { return }
        guard await ensureSupervisor(root), stillCurrent(captured) else { return }
        var opened = false
        if let browserURL {
            do { opened = openURL(try await browserURL("/")) }
            catch { errorMessage = error.localizedDescription; return }
        } else if let windowModel {
            windowModel.openJournal(using: { url in
                opened = self.openURL(url)
                return opened
            })
        } else if let url = URL(string: Self.setupBaseURL + "/") {
            opened = openURL(url)
        }
        guard opened else { return }
        do {
            try continuation.acknowledgeRefusal(resolution.handoffID)
            handoffRefusal = nil
            await landHome()
        } catch { presentPersistenceFailure() }
    }

    func retryHandoff() async {
        guard beginUserAction(.outbound) else { return }

        defer { finishWholeOperation() }
        let captured = epoch
        handoffError = nil
        _ = endUserAction(captured)
        await reconcileQualified(captured: epoch, explicitRetry: true)
    }

    func commitDisplayedMark() async {
        guard canCommitDisplayedMark, let proposalID = displayedProposalID else { return }
        guard beginUserAction(.mark) else { return }

        defer { finishWholeOperation() }
        let captured = epoch
        do {
            let state = try await setupClient.perform(.commitMark(proposalID: proposalID))
            guard endUserAction(captured) else { return }
            guard stateIsConsistent(state, root: journalRoot) else { return }
            presentInteractiveState(state)
        } catch {
            await handleActionError(error, captured: captured, operation: .mark, conflictStaysOnMark: true)
        }
    }

    func regenerateDisplayedMark() async {
        guard showsMarkRegenerate else { return }
        await regenerateMark(userInitiated: true)
    }

    @discardableResult
    func saveThinking(_ choice: String, continuing: Bool = false) async -> Bool {
        if choice == "local", setupState?.thinking.capability != .ok { return false }
        guard beginUserAction(.thinkingSave, continuing: continuing) else { return false }

        defer { finishWholeOperation() }
        let captured = epoch
        do {
            let state = try await setupClient.perform(.saveThinking(choice: choice))
            guard stateIsConsistent(state, root: journalRoot), state.thinking.saved,
                  state.thinking.choice?.rawValue == choice else {
                if failUserAction(captured, operation: .thinkingSave) {
                    thinkingFailure = JournalFirstRunCopy.thinkingFailed
                }
                return false
            }
            guard endUserAction(captured) else { return false }
            presentInteractiveState(state)
            return true
        } catch {
            await handleActionError(error, captured: captured, operation: .thinkingSave, conflictStaysOnMark: false)
            return false
        }
    }

    func finalizeThinking(continuing: Bool = false) async {
        guard canFinalizeThinking else { return }
        guard beginUserAction(.thinkingFinalize, continuing: continuing) else { return }

        defer { finishWholeOperation() }
        let captured = epoch
        let root = journalRoot.standardizedFileURL
        let handoffID: UUID
        do {
            handoffID = try continuation.storeLocal(root: root)
            _ = try continuation.mintPending(step1: handoffID)
        } catch {
            if failUserAction(captured, operation: .thinkingFinalize) {
                thinkingFailure = JournalFirstRunCopy.setupProgressNotSaved
            }
            return
        }
        do {
            let state = try await setupClient.perform(
                .finalize(timezone: TimeZone.autoupdatingCurrent.identifier)
            )
            guard endUserAction(captured) else { return }
            if stateIsConsistent(state, root: root) {
                setupState = state
                displayedProposalID = state.mark.proposalID
            }
            let next = epoch
            let probe: JournalInitSetupProbe
            do {
                probe = try await initClient.probeSetupComplete()
            } catch {
                if stillCurrent(next) {
                    failedOperationToken(.thinkingFinalize)
                    thinkingFailure = JournalFirstRunCopy.thinkingFailed
                }
                return
            }
            guard stillCurrent(next) else { return }
            guard probe == .complete else {
                failedOperationToken(.thinkingFinalize)
                thinkingFailure = JournalFirstRunCopy.thinkingFailed
                return
            }
            await deliverStep5(root: root, step1: handoffID, captured: next, mintIfNeeded: false)
        } catch {
            await handleActionError(error, captured: captured, operation: .thinkingFinalize, conflictStaysOnMark: false)
        }
    }

    func cancelThinkingDownload() async {
        guard let job = setupState?.downloads.thinking,
              job.state == .downloading,
              let attemptID = job.attemptID,
              !attemptID.isEmpty else { return }
        guard beginUserAction(.thinkingDownload) else { return }

        defer { finishWholeOperation() }
        let captured = epoch
        do {
            let state = try await setupClient.perform(.cancelThinkingDownload(attemptID: attemptID))
            guard endUserAction(captured) else { return }
            guard stateIsConsistent(state, root: journalRoot) else { return }
            setupState = state
            displayedProposalID = state.mark.proposalID
        } catch {
            await handleActionError(error, captured: captured, operation: .thinkingDownload, conflictStaysOnMark: false)
        }
    }

    func retrySpeechDownload() async {
        guard showsSpeechRetry else { return }
        guard beginUserAction(.speechDownload) else { return }

        defer { finishWholeOperation() }
        let captured = epoch
        do {
            let state = try await setupClient.perform(.retrySpeechDownload)
            guard endUserAction(captured) else { return }
            guard stateIsConsistent(state, root: journalRoot) else { return }
            setupState = state
            displayedProposalID = state.mark.proposalID
        } catch {
            await handleActionError(error, captured: captured, operation: .speechDownload, conflictStaysOnMark: false)
        }
    }

    func retryThinkingDownload() async {
        guard showsThinkingRetry else { return }
        guard beginUserAction(.thinkingDownload) else { return }

        defer { finishWholeOperation() }
        let captured = epoch
        do {
            let state = try await setupClient.perform(.startThinkingDownload)
            guard endUserAction(captured) else { return }
            guard stateIsConsistent(state, root: journalRoot) else { return }
            setupState = state
            displayedProposalID = state.mark.proposalID
        } catch {
            await handleActionError(error, captured: captured, operation: .thinkingDownload, conflictStaysOnMark: false)
        }
    }

    func advanceEpoch() {
        epoch &+= 1
    }

    private func qualifyForNativeSetup() async -> Bool {
        guard localSetupCapability() else { return false }
        guard let copy = await loadSetupCopy() else { return false }
        guard let url = peer.resolve(), await peer.verify(url) else { return false }
        setupCopy = copy
        return true
    }

    private func reconcileQualified(captured: UInt64, explicitRetry: Bool = false) async {
        let receipt: SetupHandoffReceipt?
        let refusal: SetupHandoffRefusal?
        do {
            _ = try continuation.loadActive()
            try continuation.assertMailboxAllowed()
            let loaded = try mailbox.readStep5Receipt()
            receipt = loaded?.schema == SetupHandoffReceipt.schemaV1 && loaded?.stepIndex == 5 ? loaded : nil
            refusal = try mailbox.readStep5Refusal()
        } catch {
            presentPersistenceFailure()
            return
        }
        guard stillCurrent(captured) else { return }
        do {
            try continuation.repairActiveAcceptance()
            // A new explicit intent must not join an obligation already refused by its peer.
            // Matching acceptance retains the existing duplicate/intake ordering.
            if let active = try continuation.loadActive(), receipt?.handoffID != active.pending {
                if let refusal { try continuation.resolveRefusal(refusal) }
                try continuation.repairActiveRefusal()
            }
        } catch {
            presentPersistenceFailure()
            return
        }
        if case .ready(let handoff) = mailbox.loadStep1() {
            guard await acceptStep1(handoff, captured: captured) else { return }
        }
        guard stillCurrent(captured) else { return }
        do {
            try continuation.repairActiveAcceptance()
            if let receipt { try continuation.retire(matchingPendingStep5: receipt.handoffID) }
            if let refusal { try continuation.resolveRefusal(refusal) }
            try continuation.repairActiveRefusal()
            handoffRefusal = nil
            if try continuation.loadActive() == nil, let resolution = try continuation.loadRefusal(),
               !resolution.acknowledged,
               config.journalRoot?.standardizedFileURL.path == resolution.journalRootPath {
                handoffRefusal = resolution
                handoffError = nil
                route = .ritual(.setupProgress)
                return
            }
        } catch {
            presentPersistenceFailure()
            return
        }
        guard stillCurrent(captured) else { return }
        if await resumeActive(allowDelivery: explicitRetry, allowMint: true, captured: captured) { return }
        guard stillCurrent(captured) else { return }
        await routeWithoutIntent(captured: captured)
    }

    /// Only accepts input. Routing and retirement belong to the caller's receipt snapshot.
    private func acceptStep1(_ handoff: SetupHandoff, captured: UInt64) async -> Bool {
        onBeforeStep1Commit?()
        guard stillCurrent(captured) else { return false }
        do {
            _ = try continuation.loadActive()
            guard continuation.acceptance(of: handoff.handoffID) != .unreadable else {
                throw JournalSetupContinuationError.failedRecord
            }
            continuation.prepareAcceptanceAttempt()
            try continuation.assertMailboxAllowed()
            let rootPath = config.journalRoot?.standardizedFileURL.path
            let step1 = handoff.handoffID
            let continuationStore = continuation
            let result = try mailbox.commitStep1(
                loadedID: step1,
                alreadyAccepted: continuation.acceptance(of: step1) == .accepted,
                refuseNewAcceptance: false,
                storeProgress: {
                    try continuationStore.storeProgress(step1: step1, journalRootPath: rootPath)
                }
            )
            guard stillCurrent(captured) else { return false }
            switch result {
            case .accepted, .republished: return true
            case .rejected(.superseded):
                await presentSuperseded(captured: captured)
                return false
            case .lockTimeout:
                presentPersistenceFailure()
                return false
            default: return false
            }
        } catch {
            if stillCurrent(captured) { presentPersistenceFailure() }
            return false
        }
    }

    private func presentSuperseded(captured: UInt64) async {
        handoffError = nil
        markFailure = nil
        guard config.journalRoot != nil else {
            showWelcome()
            return
        }
        let probe: JournalInitSetupProbe
        do {
            probe = try await initClient.probeSetupComplete()
        } catch {
            showFolder()
            return
        }
        guard stillCurrent(captured) else { return }
        if probe == .complete {
            await landHomeIfCurrent(captured)
        } else {
            showFolder()
        }
    }

    /// A failed or conflicting active selection is handled, never treated as absence.
    private func resumeActive(allowDelivery: Bool, allowMint: Bool, captured: UInt64) async -> Bool {
        do {
            guard let active = try continuation.loadActive() else { return false }
            guard let path = active.rootPath else {
                showWelcome()
                return true
            }
            if let configured = config.journalRoot,
               configured.standardizedFileURL.path != JournalSetupContinuationStore.standardizedPath(path) {
                throw JournalSetupContinuationError.conflictingRoot
            }
            await resumeOwned(
                handoffID: active.owner, rootPath: path, pending: active.pending,
                allowDelivery: allowDelivery, allowMint: allowMint, captured: captured
            )
            return true
        } catch {
            presentPersistenceFailure()
            return true
        }
    }

    private func resumeOwned(
        handoffID: UUID,
        rootPath: String,
        pending: UUID?,
        allowDelivery: Bool,
        allowMint: Bool,
        captured: UInt64
    ) async {
        let root = URL(fileURLWithPath: rootPath, isDirectory: true).standardizedFileURL
        assignRoot(root)
        if config.journalRoot?.standardizedFileURL.path != root.path {
            config.journalRoot = root
        }
        let live = supervisorIsLive(root) || externalSupervisorIsLive(root)
        if !live {
            guard await startSupervisor(root) else {
                errorMessage = supervisorFailureMessage()
                handoffError = nil
                failedOperationToken(.setup)
                route = .ritual(.setupProgress)
                return
            }
        }
        guard stillCurrent(captured) else { return }
        let probe: JournalInitSetupProbe
        do {
            probe = try await initClient.probeSetupComplete()
        } catch {
            errorMessage = readableMessage(for: error)
            failedOperationToken(.setup)
            return
        }
        guard stillCurrent(captured) else { return }
        if probe == .complete {
            if allowDelivery || (pending == nil && allowMint) {
                await deliverStep5(
                    root: root,
                    step1: handoffID,
                    captured: captured,
                    mintIfNeeded: pending == nil
                )
                return
            }
            if pending != nil {
                handoffError = JournalFirstRunCopy.handoffPending
                route = .ritual(.setupProgress)
                return
            }
            await landHomeIfCurrent(captured)
            return
        }
        await presentServerStep(root: root, captured: captured)
    }

    private func presentServerStep(root: URL, captured: UInt64) async {
        do {
            let state = try await setupClient.state()
            guard stillCurrent(captured) else { return }
            guard stateIsConsistent(state, root: root) else { return }
            presentInteractiveState(state)
        } catch {
            guard stillCurrent(captured) else { return }
            await handleStateError(error)
        }
    }

    private func routeWithoutIntent(captured: UInt64) async {
        let hasIntent: Bool
        if case .ready = continuation.loadContinuation() {
            hasIntent = true
        } else {
            hasIntent = false
        }
        let hasRoot = config.journalRoot != nil
        do {
            if let legacy = try handoffStore.load() {
                switch legacy.provenance {
                case JournalHandoffProvenance.bundledMigration:
                    if !hasRoot && !hasIntent {
                        await adoptQualified(captured: captured)
                        return
                    }
                    consumeHandoffBestEffort()
                case JournalHandoffProvenance.observerDiscovery:
                    if !hasRoot && !hasIntent {
                        await routeDiscoveryHandoff(legacy)
                        return
                    }
                    consumeHandoffBestEffort()
                default:
                    consumeHandoffBestEffort()
                }
            }
        } catch {
            consumeHandoffBestEffort()
        }
        guard stillCurrent(captured) else { return }
        guard let root = config.journalRoot?.standardizedFileURL else {
            showWelcome()
            return
        }
        assignRoot(root)
        let live = supervisorIsLive(root) || externalSupervisorIsLive(root)
        if !live {
            guard await startSupervisor(root) else {
                errorMessage = supervisorFailureMessage()
                failedOperationToken(.setup)
                route = .ritual(.setupProgress)
                return
            }
        }
        guard stillCurrent(captured) else { return }
        let probe: JournalInitSetupProbe
        do {
            probe = try await initClient.probeSetupComplete()
        } catch {
            errorMessage = readableMessage(for: error)
            return
        }
        guard stillCurrent(captured) else { return }
        switch probe {
        case .complete:
            await landHomeIfCurrent(captured)
        case .incomplete:
            showFolder()
        }
    }

    private func showWelcome() {
        errorMessage = nil
        handoffError = nil
        adoptMessage = nil
        markFailure = nil
        if config.journalRoot == nil {
            assignRoot(defaultJournalRoot)
        }
        loginFailure = loginFailure ?? config.launchAtLoginFailureSentence
        route = .ritual(.welcome)
        syncRefresh()
    }

    private func showFolder() {
        if let root = config.journalRoot?.standardizedFileURL {
            assignRoot(root)
        } else {
            assignRoot(defaultJournalRoot)
        }
        handoffError = nil
        route = .ritual(.folder)
        syncRefresh()
    }

    private func adoptQualified(captured: UInt64) async {
        route = .adopting
        adoptMessage = JournalFirstRunCopy.adoptOpening
        errorMessage = nil
        do {
            let handoff = try handoffStore.load()
            let root: URL
            if let handoff {
                root = URL(fileURLWithPath: handoff.journalRootPath, isDirectory: true).standardizedFileURL
                assignRoot(root)
                config.journalRoot = root
                try await runSetup(at: root, skipModelInstall: true)
                guard stillCurrent(captured) else { return }
            } else if let configured = config.journalRoot?.standardizedFileURL {
                root = configured
                assignRoot(root)
            } else {
                showWelcome()
                return
            }
            guard await ensureSupervisor(root) else {
                guard stillCurrent(captured) else { return }
                errorMessage = supervisorFailureMessage()
                return
            }
            guard stillCurrent(captured) else { return }
            let probe = try await initClient.probeSetupComplete()
            guard stillCurrent(captured) else { return }
            if handoff != nil {
                try handoffStore.consume()
            }
            adoptMessage = JournalFirstRunCopy.adoptLandingLine
            switch probe {
            case .complete:
                await landHomeIfCurrent(captured)
            case .incomplete:
                await presentServerStep(root: root, captured: captured)
            }
        } catch {
            guard stillCurrent(captured) else { return }
            errorMessage = readableMessage(for: error)
        }
    }

    private func runQualifiedSetup(skipModelInstall: Bool, captured: UInt64, actionOpen: Bool) async {
        route = .ritual(.setupProgress)
        errorMessage = nil
        handoffError = nil
        let root = journalRoot.standardizedFileURL
        do {
            try await runSetup(at: root, skipModelInstall: skipModelInstall)
            guard stillCurrent(captured) else {
                if actionOpen { _ = endUserAction(captured) }
                return
            }
            let live = supervisorIsLive(root) || externalSupervisorIsLive(root)
            if !live {
                guard await startSupervisor(root) else {
                    let ok = stillCurrent(captured)
                    if actionOpen { _ = failUserAction(captured, operation: .setup) }
                    if ok { errorMessage = supervisorFailureMessage() }
                    return
                }
            }
            guard stillCurrent(captured) else {
                if actionOpen { _ = endUserAction(captured) }
                return
            }
            let probe = try await initClient.probeSetupComplete()
            let ok = stillCurrent(captured)
            if actionOpen { _ = endUserAction(captured) }
            guard ok else { return }
            let next = epoch
            switch probe {
            case .complete:
                if await resumeActive(allowDelivery: true, allowMint: true, captured: next) { return }
                await landHomeIfCurrent(next)
            case .incomplete:
                await presentServerStep(root: root, captured: next)
            }
        } catch {
            if actionOpen {
                _ = failUserAction(captured, operation: .setup)
            }
            errorMessage = readableMessage(for: error)
        }
    }

    private func deliverStep5(root: URL, step1: UUID, captured: UInt64, mintIfNeeded: Bool) async {
        guard stillCurrent(captured) else { return }
        outbound = .delivering
        pendingOperation = .outbound
        guard let selected = peer.resolve() else {
            presentOutboundFailure()
            return
        }
        let selectedVerified = await peer.verify(selected)
        guard stillCurrent(captured) else { return }
        guard selectedVerified else {
            presentOutboundFailure()
            return
        }
        let selectedIdentity = peerIdentity(selected)
        let resolution = await identity.resolveIdentity(baseURL: Self.setupBaseURL)
        guard stillCurrent(captured) else { return }
        guard case .mark(_, let instanceID) = resolution else {
            outbound = .none
            if pendingOperation == .outbound { pendingOperation = .none }
            return
        }
        let network: JournalNetworkStatus
        do {
            network = try await setupClient.networkStatus()
        } catch {
            guard stillCurrent(captured) else { return }
            presentOutboundFailure()
            return
        }
        guard stillCurrent(captured) else { return }
        guard network.instanceID == instanceID,
              let fingerprint = network.caFingerprint,
              isFingerprint(fingerprint) else {
            outbound = .none
            if pendingOperation == .outbound { pendingOperation = .none }
            return
        }
        let pending: UUID
        do {
            if mintIfNeeded {
                pending = try continuation.mintPending(step1: step1)
            } else if let existing = try continuation.pendingStep5(ownedBy: step1) {
                pending = existing
            } else {
                outbound = .none
                if pendingOperation == .outbound { pendingOperation = .none }
                return
            }
        } catch {
            guard stillCurrent(captured) else { return }
            presentPersistenceFailure()
            return
        }
        guard stillCurrent(captured) else { return }
        guard let again = peer.resolve() else {
            presentOutboundFailure()
            return
        }
        let againVerified = await peer.verify(again)
        guard stillCurrent(captured) else { return }
        guard againVerified, peerIdentity(again) == selectedIdentity else {
            presentOutboundFailure()
            return
        }
        do {
            try continuation.assertMailboxAllowed()
            try mailbox.writeStep5(SetupHandoff(
                handoffID: pending,
                stepIndex: 5,
                journalRootPath: root.standardizedFileURL.path,
                expectedInstanceID: instanceID,
                expectedCAFingerprint: fingerprint
            ))
        } catch {
            guard stillCurrent(captured) else { return }
            presentOutboundFailure()
            return
        }
        do {
            try await peer.launch(again)
        } catch {
            guard stillCurrent(captured) else { return }
            presentOutboundFailure()
            return
        }
        guard stillCurrent(captured) else { return }
        outbound = .delivered
        if pendingOperation == .outbound { pendingOperation = .none }
        if operationError == .outbound { operationError = .none }
        handoffError = nil
        await landHomeIfCurrent(captured)
        guard stillCurrent(captured) else { return }
        stepAsideAfterHandoff()
    }

    private func presentInteractiveState(_ state: JournalSetupState) {
        setupState = state
        displayedProposalID = state.mark.proposalID
        if state.mark.state == .unavailable, state.mark.render == nil, !didAutoRegenerate {
            didAutoRegenerate = true
            Task { await self.regenerateMark(userInitiated: false) }
            return
        }
        handoffError = nil
        markFailure = nil
        if state.mark.state == .locked {
            route = .ritual(.thinking)
        } else {
            route = .ritual(.mark)
        }
        syncRefresh()
    }

    private func regenerateMark(userInitiated _: Bool) async {
        guard beginUserAction(.mark) else { return }

        defer { finishWholeOperation() }
        await sendRegenerate(captured: epoch)
    }

    private func sendRegenerate(captured: UInt64) async {
        let openedAction = actionInFlight && pendingOperation == .mark
        do {
            let state = try await setupClient.perform(.regenerateMark)
            if openedAction {
                guard endUserAction(captured) else { return }
            } else {
                guard stillCurrent(captured) else { return }
            }
            guard stateIsConsistent(state, root: journalRoot) else { return }
            presentInteractiveState(state)
        } catch {
            await handleActionError(error, captured: captured, operation: .mark, conflictStaysOnMark: true)
        }
    }

    private func handleStateError(_ error: Error) async {
        if let accessError = error as? LocalHTTPError {
            setupState = nil
            errorMessage = accessError.localizedDescription
            route = .ritual(.setupProgress)
            stopRefresh()
            return
        }
        guard let clientError = error as? JournalSetupClientError,
              case .server(let envelope) = clientError else {
            return
        }
        markFailure = markEnvelopeMessage(envelope)
        handoffError = nil
        errorMessage = nil
        operationError = .mark
        route = .ritual(.mark)
        syncRefresh()
    }

    private func handleActionError(
        _ error: Error,
        captured: UInt64,
        operation: JournalRitualOperationToken,
        conflictStaysOnMark: Bool
    ) async {
        guard let clientError = error as? JournalSetupClientError,
              case .server(let envelope) = clientError else {
            if failUserAction(captured, operation: operation) {
                applyGenericFailure(operation, error: error)
            }
            return
        }
        switch envelope.reasonCode {
        case "speech_admission_pending":
            let refreshed = try? await setupClient.state()
            let ok = endUserAction(captured)
            if ok, let refreshed, stateIsConsistent(refreshed, root: journalRoot) {
                applyRefreshedState(refreshed)
            }
        case "proposal_conflict":
            let refreshed = try? await setupClient.state()
            let ok = endUserAction(captured)
            if ok, let refreshed, stateIsConsistent(refreshed, root: journalRoot) {
                setupState = refreshed
                displayedProposalID = refreshed.mark.proposalID
                markFailure = nil
                handoffError = nil
                if conflictStaysOnMark {
                    route = .ritual(.mark)
                }
                syncRefresh()
            }
        case "link_identity_error":
            if failUserAction(captured, operation: .mark) {
                applyLinkIdentity(envelope)
            }
        default:
            if failUserAction(captured, operation: operation) {
                applyEnvelopeFailure(operation, envelope)
            }
        }
    }

    private func applyLinkIdentity(_ envelope: JournalDevicesErrorEnvelope) {
        markFailure = markEnvelopeMessage(envelope)
        handoffError = nil
        errorMessage = nil
        route = .ritual(.mark)
        syncRefresh()
    }

    private func applyGenericFailure(_ operation: JournalRitualOperationToken, error: Error) {
        switch operation {
        case .setup:
            if errorMessage == nil { errorMessage = supervisorFailureMessage() }
        case .mark:
            markFailure = setupCopy?.mark.error ?? ""
        case .thinkingSave, .thinkingFinalize:
            thinkingFailure = JournalFirstRunCopy.thinkingFailed
        case .thinkingDownload:
            thinkingFailure = downloadFailureText(for: error)
        case .speechDownload:
            speechFailure = downloadFailureText(for: error)
        case .outbound, .none:
            break
        }
    }

    private func downloadFailureText(for error: Error) -> String {
        if let message = setupCopy?.downloads.error, !message.isEmpty {
            return message
        }
        return readableMessage(for: error)
    }

    private func applyEnvelopeFailure(
        _ operation: JournalRitualOperationToken,
        _ envelope: JournalDevicesErrorEnvelope
    ) {
        switch operation {
        case .mark:
            markFailure = markEnvelopeMessage(envelope)
            handoffError = nil
            route = .ritual(.mark)
        case .thinkingSave, .thinkingFinalize, .thinkingDownload:
            thinkingFailure = thinkingEnvelopeMessage(envelope)
        case .speechDownload:
            speechFailure = firstNonEmpty(envelope.detail, envelope.error) ?? setupCopy?.downloads.error
        case .setup:
            errorMessage = firstNonEmpty(envelope.detail, envelope.error) ?? readableMessage(for: JournalSetupClientError.server(envelope))
        case .outbound:
            presentOutboundFailure()
        case .none:
            break
        }
    }

    private func markEnvelopeMessage(_ envelope: JournalDevicesErrorEnvelope) -> String {
        if let message = firstNonEmpty(envelope.detail, envelope.error) {
            return message
        }
        return setupCopy?.mark.error ?? ""
    }

    private func thinkingEnvelopeMessage(_ envelope: JournalDevicesErrorEnvelope) -> String {
        firstNonEmpty(envelope.detail, envelope.error) ?? ""
    }

    private func firstNonEmpty(_ first: String?, _ second: String?) -> String? {
        if let first, !first.isEmpty { return first }
        if let second, !second.isEmpty { return second }
        return nil
    }

    private func presentOutboundFailure() {
        outbound = .failed
        if pendingOperation == .outbound { pendingOperation = .none }
        operationError = .outbound
        handoffError = JournalFirstRunCopy.handoffNotDelivered
        revealHandoffFailure()
    }

    private func presentPersistenceFailure() {
        if pendingOperation != .none { pendingOperation = .none }
        operationError = .setup
        outbound = .none
        handoffError = JournalFirstRunCopy.setupProgressNotSaved
        revealHandoffFailure()
    }

    /// The deciding placeholder has no handoff card. A failed accept or delivery stays on a ritual step so the owner can retry.
    private func revealHandoffFailure() {
        if case .deciding = route {
            route = .ritual(.setupProgress)
        }
    }

    private func applyRefreshedState(_ state: JournalSetupState) {
        setupState = state
        if handoffError == nil {
            displayedProposalID = state.mark.proposalID
        }
        syncRefresh()
    }

    private func landHomeIfCurrent(_ captured: UInt64) async {
        guard stillCurrent(captured) else { return }
        await landHome()
    }

    private func ensureSupervisor(_ root: URL) async -> Bool {
        if supervisorIsLive(root) || externalSupervisorIsLive(root) {
            return true
        }
        return await startSupervisor(root)
    }

    private func beginUserAction(_ operation: JournalRitualOperationToken, continuing: Bool = false) -> Bool {
        guard !actionInFlight, !isTerminating,
              wholeOperationDepth == 0 || (continuing && wholeOperationDepth == 1) else { return false }
        wholeOperationDepth += 1
        userOperationInFlight = true
        actionInFlight = true
        epoch &+= 1
        actionEpoch = epoch
        pendingOperation = operation
        if operationError == operation {
            operationError = .none
        }
        switch operation {
        case .setup:
            errorMessage = nil
        case .mark:
            markFailure = nil
        case .thinkingSave, .thinkingFinalize, .thinkingDownload:
            thinkingFailure = nil
        case .speechDownload:
            speechFailure = nil
        case .outbound:
            handoffError = nil
        case .none:
            break
        }
        return true
    }

    @discardableResult
    private func endUserAction(_ captured: UInt64) -> Bool {
        guard actionEpoch == captured else { return false }
        let ok = stillCurrent(captured)
        if ok { epoch &+= 1 }
        actionEpoch = nil
        pendingOperation = .none
        actionInFlight = false
        return ok
    }

    @discardableResult
    private func failUserAction(_ captured: UInt64, operation: JournalRitualOperationToken) -> Bool {
        guard actionEpoch == captured else { return false }
        let ok = stillCurrent(captured)
        if ok { epoch &+= 1 }
        actionEpoch = nil
        pendingOperation = .none
        actionInFlight = false
        if ok {
            operationError = operation
        }
        return ok
    }

    private func failedOperationToken(_ operation: JournalRitualOperationToken) {
        pendingOperation = .none
        operationError = operation
    }

    private func syncRefresh() {
        if refreshRouteActive {
            startRefreshIfNeeded()
        } else {
            stopRefresh()
        }
    }

    private var refreshRouteActive: Bool {
        guard nativeQualified else { return false }
        switch route {
        case .ritual(.mark), .ritual(.thinking):
            return supervisorIsLive(journalRoot) || externalSupervisorIsLive(journalRoot)
        default:
            return false
        }
    }

    private func startRefreshIfNeeded() {
        guard refreshTask == nil else { return }
        let sleep = refreshSleep
        refreshTask = Task { @MainActor [weak self] in
            while !Task.isCancelled {
                await sleep()
                if Task.isCancelled { return }
                await self?.refreshOnce()
            }
        }
    }

    private func stopRefresh() {
        refreshTask?.cancel()
        refreshTask = nil
    }

    private func refreshOnce() async {
        let captured = epoch
        let root = journalRoot.standardizedFileURL
        do {
            let state = try await setupClient.state()
            guard stillCurrent(captured) else { return }
            guard !actionInFlight, wholeOperationDepth == 0 else { return }
            guard journalRoot.standardizedFileURL.path == root.path else { return }
            guard stateIsConsistent(state, root: root) else { return }
            applyRefreshedState(state)
        } catch {
            return
        }
    }

    private func stateIsConsistent(_ state: JournalSetupState, root: URL) -> Bool {
        guard samePath(state.journalPath, root) else { return false }
        switch state.mark.state {
        case .proposed:
            guard let id = state.mark.proposalID, !id.isEmpty else { return false }
        case .unavailable:
            guard state.mark.render == nil else { return false }
        case .locked:
            guard let render = state.mark.render, JournalMark.validate(render) != nil else { return false }
        }
        if state.thinking.saved {
            guard state.thinking.choice != nil else { return false }
        } else {
            guard state.thinking.choice == nil else { return false }
        }
        return true
    }

    private func canCancel(_ job: JournalSetupState.Job?) -> Bool {
        guard let job else { return false }
        return job.state == .downloading && !(job.attemptID ?? "").isEmpty
    }

    private func samePath(_ path: String, _ root: URL) -> Bool {
        JournalSetupContinuationStore.standardizedPath(path) == root.standardizedFileURL.path
    }

    private func peerIdentity(_ url: URL) -> URL {
        url.standardizedFileURL.resolvingSymlinksInPath()
    }

    private func isFingerprint(_ value: String) -> Bool {
        value.range(of: "^[0-9a-f]{64}$", options: .regularExpression) != nil
    }

    private func formattedByteCount(_ bytes: UInt64) -> String {
        ByteCountFormatter.string(fromByteCount: Int64(clamping: bytes), countStyle: .file)
    }
}
