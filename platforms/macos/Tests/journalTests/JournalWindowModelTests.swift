// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import Foundation
import JournalMarkKit
import JournalRuntime
import JournalRuntimeTestSupport
import MacTestSupport
import ServiceManagement
import Testing
@testable import journal

@MainActor
@Suite("JournalWindowModel")
struct JournalWindowModelTests {
    @Test func windowOpenWaitsForReadinessAndThenShowsTheMarkInBothPanes() async throws {
        let fixture = try makeConfiguredFixture()
        defer { fixture.clear() }
        let supervisor = try makeSupervisor()
        let counter = IdentityCounter(read: .mark(.uiTestSample))
        let model = makeModel(config: fixture.config, supervisor: supervisor,
                              fetchIdentity: { _ in await counter.fetch() })

        model.prepareForWindowOpen()
        await model.loadForWindowOpen()
        #expect(await counter.count == 0)
        #expect(model.markPresentation == .loading)
        #expect(model.devicesModel.markPresentation == .loading)

        _ = await supervisor.start(journalRoot: try #require(fixture.config.journalRoot))
        await model.loadForWindowOpen()
        #expect(model.markPresentation == .mark(.uiTestSample))
        #expect(model.devicesModel.markPresentation == .mark(.uiTestSample))
    }

    @Test func failedStartupReadCanRecoverAtReadinessWithoutReopening() async throws {
        let fixture = try makeConfiguredFixture()
        defer { fixture.clear() }
        let supervisor = try makeSupervisor()
        let reads = IdentitySequence(reads: [.unavailable, .unavailable, .unavailable, .mark(.uiTestSample)])
        let capture = IdentityMarkCapture()
        let model = makeModel(config: fixture.config, supervisor: supervisor,
                              fetchIdentity: { _ in await reads.fetch() },
                              onIdentityMark: { capture.append($0) })

        model.prepareForWindowOpen()
        await model.fetchIdentityIfNeeded()
        #expect(model.markPresentation == .unavailable)
        #expect(await reads.count == 3)

        _ = await supervisor.start(journalRoot: try #require(fixture.config.journalRoot))
        await model.loadForWindowOpen()
        #expect(model.markPresentation == .mark(.uiTestSample))
        #expect(model.devicesModel.markPresentation == .mark(.uiTestSample))
        #expect(capture.snapshot() == [.uiTestSample])
    }

    @Test func missingFirstRunMarkRetriesAndUpdatesHomeAndDevices() async throws {
        let fixture = try makeConfiguredFixture()
        defer { fixture.clear() }
        let supervisor = try makeSupervisor()
        _ = await supervisor.start(journalRoot: try #require(fixture.config.journalRoot))
        let reads = IdentitySequence(reads: [.unavailable, .mark(.uiTestSample)])
        let capture = IdentityMarkCapture()
        let model = makeModel(config: fixture.config, supervisor: supervisor,
                              fetchIdentity: { _ in await reads.fetch() },
                              onIdentityMark: { capture.append($0) })

        model.applyFirstRunLanding()
        #expect(model.markPresentation == .loading)
        #expect(model.devicesModel.markPresentation == .loading)
        await model.loadForWindowOpen()
        await model.loadForWindowOpen()

        #expect(await reads.count == 2)
        #expect(model.markPresentation == .mark(.uiTestSample))
        #expect(model.devicesModel.markPresentation == .mark(.uiTestSample))
        #expect(capture.snapshot() == [.uiTestSample])
    }

    @Test func concurrentLoadsShareOneFetchAndDoNotReplaceTheMarkWhileWaiting() async throws {
        let fixture = try makeConfiguredFixture()
        defer { fixture.clear() }
        let fetch = PausedIdentityFetch()
        let model = makeModel(config: fixture.config, fetchIdentity: { _ in await fetch.fetch() })
        let first = Task { await model.fetchIdentityIfNeeded() }
        await fetch.waitUntilStarted()
        let second = Task { await model.fetchIdentityIfNeeded() }
        #expect(model.markPresentation == .loading)
        #expect(model.devicesModel.markPresentation == .loading)
        await fetch.resume(with: .mark(.uiTestSample))
        await first.value
        await second.value

        #expect(await fetch.count == 1)
        #expect(model.markPresentation == .mark(.uiTestSample))
        #expect(model.devicesModel.markPresentation == .mark(.uiTestSample))
    }

    @Test func missingLandingMarkDuringAFetchStillGetsANewRead() async throws {
        let fixture = try makeConfiguredFixture()
        defer { fixture.clear() }
        let fetch = PausedIdentityFetch()
        let recovery = IdentityCounter(read: .mark(.uiTestSample))
        let model = makeModel(config: fixture.config, fetchIdentity: { _ in
            if await fetch.count == 0 { return await fetch.fetch() }
            return await recovery.fetch()
        })
        let loading = Task { await model.fetchIdentityIfNeeded() }
        await fetch.waitUntilStarted()
        model.applyFirstRunLanding()
        var landingLoad: Task<Void, Never>?
        await withCheckedContinuation { (started: CheckedContinuation<Void, Never>) in
            landingLoad = Task {
                started.resume()
                await model.fetchIdentityIfNeeded()
            }
        }
        var concurrentLoad: Task<Void, Never>?
        await withCheckedContinuation { (started: CheckedContinuation<Void, Never>) in
            concurrentLoad = Task {
                started.resume()
                await model.fetchIdentityIfNeeded()
            }
        }
        await fetch.resume(with: .unavailable)
        await loading.value
        try await #require(landingLoad).value
        try await #require(concurrentLoad).value

        #expect(await recovery.count == 1)
        #expect(model.markPresentation == .mark(.uiTestSample))
        #expect(model.devicesModel.markPresentation == .mark(.uiTestSample))
    }

    @Test func identityFetchRunsOncePerWindowOpenAndDoesNotPoll() async throws {
        let fixture = try makeConfiguredFixture()
        defer { fixture.clear() }
        let counter = IdentityCounter(read: .mark(.uiTestSample))
        let supervisor = try makeSupervisor()
        _ = await supervisor.start(journalRoot: try #require(fixture.config.journalRoot))
        let model = makeModel(
            config: fixture.config,
            supervisor: supervisor,
            fetchIdentity: { _ in await counter.fetch() }
        )

        model.prepareForWindowOpen()
        await model.loadForWindowOpen()
        await model.loadForWindowOpen()

        #expect(await counter.count == 1)
        #expect(model.markPresentation == .mark(.uiTestSample))

        model.prepareForWindowOpen()
        await model.loadForWindowOpen()
        #expect(await counter.count == 2)
    }

    @Test func malformedIdentityResponseReturnsUnavailable() async throws {
        let store = ObserverURLProtocolStore()
        store.enqueue(body: ##"{"committed":true,"mark":{"icon1":{"name":"bad","color":{"hex":"#abc"},"rot":12,"svg":""},"icon2":{"name":"bad","color":{"hex":"#abc"},"rot":12,"svg":""},"words":["only"]}}"##)
        let session = URLSession(configuration: observerURLProtocolConfiguration(store: store))
        let fetcher = JournalIdentityFetcher(session: session)
        let fixture = try makeConfiguredFixture()
        defer { fixture.clear() }
        let model = makeModel(
            config: fixture.config,
            fetchIdentity: { baseURL in await fetcher.fetch(baseURL: baseURL) }
        )

        await model.fetchIdentityIfNeeded()

        #expect(model.markPresentation == .unavailable)
    }

    @Test func onIdentityMarkFiresAfterSuccessfulFetch() async throws {
        let fixture = try makeConfiguredFixture()
        defer { fixture.clear() }
        let capture = IdentityMarkCapture()
        let model = makeModel(
            config: fixture.config,
            fetchIdentity: { _ in .mark(.uiTestSample) },
            onIdentityMark: { mark in capture.append(mark) }
        )

        await model.fetchIdentityIfNeeded()

        #expect(capture.snapshot() == [.uiTestSample])
    }

    @Test func onIdentityMarkDoesNotFireForUnavailableFetch() async throws {
        let fixture = try makeConfiguredFixture()
        defer { fixture.clear() }
        let capture = IdentityMarkCapture()
        let model = makeModel(
            config: fixture.config,
            fetchIdentity: { _ in .unavailable },
            onIdentityMark: { mark in capture.append(mark) }
        )

        await model.fetchIdentityIfNeeded()

        #expect(capture.snapshot().isEmpty)
    }

    @Test func markPresentationDerivesCorrectlyFromConfigAndRead() async throws {
        let unconfigured = makeUnconfiguredFixture()
        defer { unconfigured.clear() }
        let unconfModel = makeModel(config: unconfigured.config)
        #expect(unconfModel.markPresentation == .generic)

        let fixture = try makeConfiguredFixture()
        defer { fixture.clear() }

        let notFetched = makeModel(config: fixture.config)
        #expect(notFetched.markPresentation == .loading)
        #expect(notFetched.devicesModel.markPresentation == .loading)

        let marked = makeModel(
            config: fixture.config,
            fetchIdentity: { _ in .mark(.uiTestSample) }
        )
        await marked.fetchIdentityIfNeeded()
        #expect(marked.markPresentation == .mark(.uiTestSample))

        let uncommitted = makeModel(
            config: fixture.config,
            fetchIdentity: { _ in .uncommitted }
        )
        await uncommitted.fetchIdentityIfNeeded()
        #expect(uncommitted.markPresentation == .generic)

        let unavailable = makeModel(
            config: fixture.config,
            fetchIdentity: { _ in .unavailable }
        )
        await unavailable.fetchIdentityIfNeeded()
        #expect(unavailable.markPresentation == .unavailable)
    }

    @Test func runDisplayFailsClosedForLesserStates() {
        let diagnostic = JournalDiagnostic(commandLabel: "journal", outputExcerpt: "no")

        #expect(JournalRunDisplay.derive(state: .running, runtimeStatus: .running) == .running)
        #expect(JournalRunDisplay.derive(state: .running, runtimeStatus: .stopped(diagnostic)) == .stopped)
        #expect(JournalRunDisplay.derive(state: .running, runtimeStatus: .restarting(generation: nil)) == .starting)
        #expect(JournalRunDisplay.derive(state: .running, runtimeStatus: .restarting(generation: 7)) == .starting)
        #expect(JournalRunDisplay.derive(state: .running, runtimeStatus: .unobserved) == .unknown)
        #expect(JournalRunDisplay.derive(state: .starting, runtimeStatus: .running) == .starting)
        #expect(JournalRunDisplay.derive(state: .waitingForReadiness, runtimeStatus: .running) == .starting)
        #expect(JournalRunDisplay.derive(state: .blocked(diagnostic), runtimeStatus: .running) == .blocked)
        #expect(JournalRunDisplay.derive(state: .failed(diagnostic), runtimeStatus: .running) == .unknown)
        #expect(JournalRunDisplay.derive(state: .idle, runtimeStatus: .running) == .unknown)
    }

    @Test func postReadyStoppedStatusDropsDisplayOutOfRunning() async throws {
        let fixture = try makeConfiguredFixture()
        defer { fixture.clear() }
        let supervisor = JournalSupervisor(
            gate: MockSingleSupervisorGate(),
            materializer: MockRuntimeMaterializer(result: .success(try makeRuntime())),
            runner: MockSupervisedChildRunner(),
            readinessGate: MockJournalReadinessGate(result: .ready)
        )
        _ = configureInMemoryReceiptContext(supervisor)
        let model = makeModel(config: fixture.config, supervisor: supervisor)
        _ = await supervisor.start(journalRoot: try #require(fixture.config.journalRoot))
        supervisor.applyRuntimeStatus(.running)
        #expect(model.runDisplay == .running)
        #expect(model.homeOffer == .door)

        let diagnostic = JournalDiagnostic(commandLabel: "journal", outputExcerpt: "exit")
        supervisor.applyRuntimeStatus(.stopped(diagnostic))

        #expect(model.runDisplay == .stopped)
        #expect(model.homeOffer == .start)
    }

    @Test func stoppedReasonShowsOnlyTheRunnersOwnStopReasons() async throws {
        let fixture = try makeConfiguredFixture()
        defer { fixture.clear() }
        let supervisor = JournalSupervisor(
            gate: MockSingleSupervisorGate(),
            materializer: MockRuntimeMaterializer(result: .success(try makeRuntime())),
            runner: MockSupervisedChildRunner(),
            readinessGate: MockJournalReadinessGate(result: .ready)
        )
        _ = configureInMemoryReceiptContext(supervisor)
        let model = makeModel(config: fixture.config, supervisor: supervisor)
        _ = await supervisor.start(journalRoot: try #require(fixture.config.journalRoot))
        supervisor.applyRuntimeStatus(.running)
        #expect(model.stoppedReason == nil)

        for reason in [UICopy.JOURNAL_CHILD_CONTAINMENT_UNRESOLVED, UICopy.JOURNAL_CHILD_BREAKER_TRIPPED] {
            supervisor.applyRuntimeStatus(.stopped(JournalDiagnostic(commandLabel: "solstone journal start --hosted-parent", outputExcerpt: reason)))
            #expect(model.homeOffer == .start)
            #expect(model.stoppedReason == reason)
        }

        supervisor.applyRuntimeStatus(.stopped(JournalDiagnostic(commandLabel: "journal", outputExcerpt: "raw command output")))
        #expect(model.stoppedReason == nil)

        supervisor.applyRuntimeStatus(.stoppedByUser)
        #expect(model.stoppedReason == nil)
    }

    @Test func homeOfferFollowsConfiguredRunDisplay() async throws {
        let unconfigured = makeUnconfiguredFixture()
        defer { unconfigured.clear() }
        #expect(makeModel(config: unconfigured.config).homeOffer == .unconfigured)

        let fixture = try makeConfiguredFixture()
        defer { fixture.clear() }

        #expect(makeModel(config: fixture.config).homeOffer == .runState)

        let stoppedSupervisor = JournalSupervisor()
        stoppedSupervisor.applyRuntimeStatus(.stoppedByUser)
        #expect(makeModel(config: fixture.config, supervisor: stoppedSupervisor).homeOffer == .start)

        let startingSupervisor = JournalSupervisor()
        startingSupervisor.applyRuntimeStatus(.restarting(generation: nil))
        #expect(makeModel(config: fixture.config, supervisor: startingSupervisor).homeOffer == .none)

        let runningSupervisor = JournalSupervisor(
            gate: MockSingleSupervisorGate(),
            materializer: MockRuntimeMaterializer(result: .success(try makeRuntime())),
            runner: MockSupervisedChildRunner(),
            readinessGate: MockJournalReadinessGate(result: .ready)
        )
        _ = configureInMemoryReceiptContext(runningSupervisor)
        let runningModel = makeModel(config: fixture.config, supervisor: runningSupervisor)
        _ = await runningSupervisor.start(journalRoot: try #require(fixture.config.journalRoot))
        runningSupervisor.applyRuntimeStatus(.running)
        #expect(runningModel.homeOffer == .door)

        let diagnostic = JournalDiagnostic(commandLabel: "gate", outputExcerpt: "port busy")
        let blockage = SingleSupervisorGateBlockage.portConflict(diagnostic)
        let blockedSupervisor = JournalSupervisor(
            gate: MockSingleSupervisorGate(),
            materializer: MockRuntimeMaterializer(result: .success(try makeRuntime())),
            runner: MockSupervisedChildRunner(startError: SupervisedJournalRunnerError.gateBlocked(blockage)),
            readinessGate: MockJournalReadinessGate(result: .ready)
        )
        _ = configureInMemoryReceiptContext(blockedSupervisor)
        _ = await blockedSupervisor.start(journalRoot: try #require(fixture.config.journalRoot))
        #expect(makeModel(config: fixture.config, supervisor: blockedSupervisor).homeOffer == .runState)
    }

    @Test func openJournalUsesDefaultBaseWithTrailingSlash() {
        let fixture = makeUnconfiguredFixture()
        defer { fixture.clear() }
        let model = makeModel(config: fixture.config)
        let capture = OpenURLCapture()
        model.openJournal(using: { url in
            capture.append(url)
            return true
        })
        #expect(capture.absoluteStrings == ["http://127.0.0.1:5015/"])
    }

    @Test func openJournalUsesInjectedHostAndPort() {
        let fixture = makeUnconfiguredFixture()
        defer { fixture.clear() }
        let model = makeModel(config: fixture.config, baseURL: "http://10.1.2.3:9999/")
        let capture = OpenURLCapture()
        model.openJournal(using: { url in
            capture.append(url)
            return true
        })
        #expect(capture.absoluteStrings == ["http://10.1.2.3:9999/"])
    }

    @Test func openJournalFailureDoesNotRetryOrStoreError() throws {
        let fixture = try makeConfiguredFixture()
        defer { fixture.clear() }
        let model = makeModel(config: fixture.config)
        #expect(model.homeOffer == .runState)
        let capture = OpenURLCapture()
        model.openJournal(using: { url in
            capture.append(url)
            return false
        })
        #expect(capture.urls.count == 1)
        #expect(model.homeOffer == .runState)
    }


    @Test func bundledVersionProbeRunsBeforeStartAndHealthUsesTheActiveRuntime() async throws {
        let fixture = try makeConfiguredFixture()
        defer { fixture.clear() }
        let runtime = try makeRuntime()
        let supervisor = JournalSupervisor(
            gate: MockSingleSupervisorGate(),
            materializer: MockRuntimeMaterializer(result: .success(runtime)),
            runner: MockSupervisedChildRunner(),
            readinessGate: MockJournalReadinessGate(result: .ready)
        )
        _ = configureInMemoryReceiptContext(supervisor)
        let executableURL = URL(fileURLWithPath: "/Applications/Journal.app/Contents/MacOS/journal")
        let expectedCommandURL = JournalCommandLine.commandLineURL(executableURL: executableURL)
        let model = makeModel(
            config: fixture.config,
            supervisor: supervisor,
            fetchHealth: { binary, environment in
                #expect(binary == runtime.layout.solstoneBinary)
                #expect(environment == runtime.environment)
                return .healthy
            },
            fetchVersion: { binary, environment in
                #expect(binary == expectedCommandURL)
                #expect(environment == nil)
                return "1.2.3"
            },
            versionExecutableURL: { executableURL },
            aboutOSVersion: { "15.6" },
            aboutArch: { nil },
            appBuild: "67"
        )

        await model.refreshRunState()
        #expect(model.healthDisplay == .unknown)
        #expect(model.journalVersion == "1.2.3")
        #expect(model.aboutBlock == "journal 1.2.3 (67) · macos 15.6")

        _ = await supervisor.start(journalRoot: try #require(fixture.config.journalRoot))
        await model.refreshRunState()

        #expect(model.healthDisplay == .healthy)
        #expect(model.journalVersion == "1.2.3")
    }

    @Test func stopJournalRefreshesHealthAndVersionRowsToUnknown() async throws {
        let fixture = try makeConfiguredFixture()
        defer { fixture.clear() }
        let supervisor = JournalSupervisor(
            gate: MockSingleSupervisorGate(),
            materializer: MockRuntimeMaterializer(result: .success(try makeRuntime())),
            runner: MockSupervisedChildRunner(),
            readinessGate: MockJournalReadinessGate(result: .ready)
        )
        _ = configureInMemoryReceiptContext(supervisor)
        let model = makeModel(
            config: fixture.config,
            supervisor: supervisor,
            fetchHealth: { _, _ in .healthy },
            fetchVersion: { _, _ in "1.2.3" }
        )
        _ = await supervisor.start(journalRoot: try #require(fixture.config.journalRoot))
        await model.refreshRunState()
        #expect(model.healthDisplay == .healthy)
        #expect(model.journalVersion == "1.2.3")

        model.stopJournal()
        let deadline = ContinuousClock.now.advanced(by: .seconds(1))
        while ContinuousClock.now < deadline,
              !(model.healthDisplay == .unknown) {
            try await Task.sleep(for: .milliseconds(10))
        }

        #expect(model.healthDisplay == .unknown)
        #expect(model.journalVersion == "1.2.3")
    }

    @Test func unconfiguredLaunchDoesNotStartSupervisorWork() async throws {
        let fixture = makeUnconfiguredFixture()
        defer { fixture.clear() }
        let gate = MockSingleSupervisorGate()
        let supervisor = JournalSupervisor(
            gate: gate,
            materializer: MockRuntimeMaterializer(result: .success(try makeRuntime())),
            runner: MockSupervisedChildRunner(),
            readinessGate: MockJournalReadinessGate(result: .ready)
        )
        let appModel = JournalAppModel(config: fixture.config, supervisor: supervisor)

        appModel.launch()
        try await Task.sleep(for: .milliseconds(25))

        #expect(gate.prepareCalls == 0)
        #expect(appModel.windowModel.unconfiguredMessage == "nothing here yet. creating your journal comes next.")
    }

    @Test func launchDoesNotBlockOnAwaitedUnregister() async throws {
        let suiteName = "app.solstone.journal.window-model.launch.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suiteName)!
        defaults.removePersistentDomain(forName: suiteName)
        defer { defaults.removePersistentDomain(forName: suiteName) }

        let fake = LaunchSuspendingLoginItemManager()
        let config = JournalAppConfig(
            defaults: defaults,
            loginItemManager: fake,
            loginItemEligibility: { true },
            loginItemBundleIdentity: { (URL(fileURLWithPath: "/Applications/Journal.app"), "2") }
        )

        let gate = MockSingleSupervisorGate()
        let supervisor = JournalSupervisor(
            gate: gate,
            materializer: MockRuntimeMaterializer(result: .success(try makeRuntime())),
            runner: MockSupervisedChildRunner(),
            readinessGate: MockJournalReadinessGate(result: .ready)
        )
        let appModel = JournalAppModel(config: config, supervisor: supervisor)

        appModel.launch()

        await withCheckedContinuation { cont in
            fake.onUnregisterStarted = cont
        }

        #expect(fake.registerCalls == 0)
        #expect(appModel.windowModel.launchAtLoginFailure == nil)

        fake.shouldThrowOnRegister = true
        await withCheckedContinuation { cont in
            fake.onRegisterFinished = cont
            let hold = fake.unregisterHoldContinuation
            fake.unregisterHoldContinuation = nil
            hold?.resume()
        }

        await Task.yield()

        #expect(appModel.windowModel.launchAtLoginFailure == config.launchAtLoginFailureSentence)
        #expect(appModel.firstRunModel.loginFailure == config.launchAtLoginFailureSentence)
        #expect(config.launchAtLoginIssue == .failed)
        #expect(appModel.windowModel.launchAtLoginEnabled)
    }

    @Test func launchAtLoginToggleUsesConfig() {
        let fixture = makeUnconfiguredFixture()
        defer { fixture.clear() }
        let model = makeModel(config: fixture.config)

        model.setLaunchAtLoginEnabled(false)
        #expect(!model.launchAtLoginEnabled)

        model.setLaunchAtLoginEnabled(true)
        #expect(model.launchAtLoginEnabled)
    }

    private func makeConfiguredFixture() throws -> AppFixture {
        let fixture = makeUnconfiguredFixture()
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("journal-window-model-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        fixture.config.journalRoot = root
        return fixture
    }

    private func makeSupervisor() throws -> JournalSupervisor {
        let supervisor = JournalSupervisor(
            gate: MockSingleSupervisorGate(),
            materializer: MockRuntimeMaterializer(result: .success(try makeRuntime())),
            runner: MockSupervisedChildRunner(),
            readinessGate: MockJournalReadinessGate(result: .ready)
        )
        _ = configureInMemoryReceiptContext(supervisor)
        return supervisor
    }

    private func makeUnconfiguredFixture() -> AppFixture {
        let suiteName = "app.solstone.journal.window-model.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suiteName)!
        defaults.removePersistentDomain(forName: suiteName)
        let config = JournalAppConfig(defaults: defaults, loginItemManager: WindowModelFakeLoginItemManager())
        return AppFixture(suiteName: suiteName, defaults: defaults, config: config)
    }

    private func makeModel(
        config: JournalAppConfig,
        supervisor: JournalSupervisor = JournalSupervisor(),
        baseURL: String = "http://127.0.0.1:5015",
        fetchIdentity: JournalWindowModel.IdentityFetch? = { _ in .unavailable },
        identityRetryPause: @escaping JournalWindowModel.IdentityRetryPause = {},
        fetchDiskUsage: JournalWindowModel.DiskUsageFetch? = { _ in 0 },
        fetchHealth: JournalWindowModel.HealthFetch? = { _, _ in .unknown(JournalDiagnostic(commandLabel: "health")) },
        fetchVersion: JournalWindowModel.VersionFetch? = { _, _ in nil },
        versionExecutableURL: @escaping JournalWindowModel.VersionExecutableURLProvider = { JournalCommandLine.currentExecutableURL() },
        aboutOSVersion: @escaping JournalWindowModel.AboutStringProvider = { "15.6" },
        aboutArch: @escaping JournalWindowModel.AboutArchProvider = { "arm64" },
        appBuild: String = "67",
        onIdentityMark: JournalWindowModel.IdentityMarkObserver? = nil
    ) -> JournalWindowModel {
        JournalWindowModel(
            config: config,
            supervisor: supervisor,
            baseURL: baseURL,
            fetchIdentity: fetchIdentity,
            identityRetryPause: identityRetryPause,
            fetchDiskUsage: fetchDiskUsage,
            fetchHealth: fetchHealth,
            fetchVersion: fetchVersion,
            versionExecutableURL: versionExecutableURL,
            aboutOSVersion: aboutOSVersion,
            aboutArch: aboutArch,
            appBuild: appBuild,
            onIdentityMark: onIdentityMark
        )
    }
}

private enum TestError: Error {
    case requested
}

private struct AppFixture {
    let suiteName: String
    let defaults: UserDefaults
    let config: JournalAppConfig

    @MainActor
    func clear() {
        if let root = config.journalRoot {
            try? FileManager.default.removeItem(at: root)
        }
        defaults.removePersistentDomain(forName: suiteName)
    }
}

private actor IdentityCounter {
    private let read: JournalIdentityRead
    private var calls = 0

    init(read: JournalIdentityRead) {
        self.read = read
    }

    var count: Int { calls }

    func fetch() -> JournalIdentityRead {
        calls += 1
        return read
    }
}

private actor IdentitySequence {
    private var reads: [JournalIdentityRead]
    private(set) var count = 0

    init(reads: [JournalIdentityRead]) { self.reads = reads }

    func fetch() -> JournalIdentityRead {
        count += 1
        return reads.isEmpty ? .unavailable : reads.removeFirst()
    }
}

private actor PausedIdentityFetch {
    private var result: CheckedContinuation<JournalIdentityRead, Never>?
    private var started: CheckedContinuation<Void, Never>?
    private(set) var count = 0

    func fetch() async -> JournalIdentityRead {
        count += 1
        return await withCheckedContinuation { continuation in
            result = continuation
            started?.resume()
            started = nil
        }
    }

    func waitUntilStarted() async {
        guard result == nil else { return }
        await withCheckedContinuation { started = $0 }
    }

    func resume(with read: JournalIdentityRead) {
        result?.resume(returning: read)
        result = nil
    }
}

@MainActor
private final class OpenURLCapture {
    private(set) var urls: [URL] = []

    var absoluteStrings: [String] { urls.map(\.absoluteString) }

    func append(_ url: URL) {
        urls.append(url)
    }
}

@MainActor
private final class IdentityMarkCapture {
    private var marks: [JournalMark] = []

    func append(_ mark: JournalMark) {
        marks.append(mark)
    }

    func snapshot() -> [JournalMark] {
        marks
    }
}

@MainActor
private final class WindowModelFakeLoginItemManager: LoginItemManaging {
    var status: SMAppService.Status = .notRegistered

    func register() throws {}
    func unregister() throws {}
    func unregisterAwaitingCompletion() async throws {}
}

@MainActor
private final class LaunchSuspendingLoginItemManager: LoginItemManaging {
    var status: SMAppService.Status = .enabled
    var registerCalls = 0
    var shouldThrowOnRegister = false
    var onUnregisterStarted: CheckedContinuation<Void, Never>?
    var unregisterHoldContinuation: CheckedContinuation<Void, Never>?
    var onRegisterFinished: CheckedContinuation<Void, Never>?

    func register() throws {
        registerCalls += 1
        if shouldThrowOnRegister {
            let finished = onRegisterFinished
            onRegisterFinished = nil
            finished?.resume()
            throw CocoaError(.fileWriteUnknown)
        }
    }

    func unregister() throws {}

    func unregisterAwaitingCompletion() async throws {
        await withCheckedContinuation { cont in
            unregisterHoldContinuation = cont
            let started = onUnregisterStarted
            onUnregisterStarted = nil
            started?.resume()
        }
    }
}
