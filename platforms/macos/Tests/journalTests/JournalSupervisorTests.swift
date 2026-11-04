// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import Foundation
import JournalRuntime
import JournalRuntimeTestSupport
import MacTestSupport
import SolstoneCore
import Testing
@testable import journal

@MainActor
@Suite("JournalSupervisor")
struct JournalSupervisorTests {
    @Test func blockedRunnerGateDoesNoSpawnOrReadiness() async throws {
        let events = EventRecorder()
        let diagnostic = JournalDiagnostic(commandLabel: "gate", outputExcerpt: "blocked")
        let blockage = SingleSupervisorGateBlockage.portConflict(diagnostic)
        let runner = RecordingRunner(
            events: events,
            gate: RecordingGate(events: events, result: .blocked(blockage))
        )
        let supervisor = JournalSupervisor(
            gate: RecordingGate(events: events, result: .success),
            materializer: try await RecordingMaterializer(events: events),
            runner: runner,
            readinessGate: RecordingReadinessGate(events: events, result: .ready)
        )
        _ = configureInMemoryReceiptContext(supervisor)

        let started = await supervisor.start(journalRoot: try makeTemporaryDirectory())

        #expect(!started)
        #expect(await events.snapshot() == ["materialize", "gate"])
        #expect(supervisor.state == .blocked(diagnostic))
        #expect(supervisor.blockedReason == blockage.ownerMessage)
    }

    @Test func aDriveThatCannotRunTheJournalSaysSoBeforeAnythingStarts() async throws {
        let events = EventRecorder()
        let supervisor = JournalSupervisor(
            gate: RecordingGate(events: events, result: .success),
            materializer: try await RecordingMaterializer(events: events),
            runner: RecordingRunner(events: events, gate: RecordingGate(events: events, result: .success)),
            readinessGate: RecordingReadinessGate(events: events, result: .ready),
            driveCannotRun: { _ in true }
        )
        _ = configureInMemoryReceiptContext(supervisor)

        let started = await supervisor.start(journalRoot: try makeTemporaryDirectory())

        #expect(!started)
        #expect(await events.snapshot().isEmpty)
        #expect(supervisor.blockedReason == UICopy.JOURNAL_DRIVE_CANNOT_RUN)
    }

    @Test func openGateRunsMaterializeSpawnReadinessInOrder() async throws {
        let events = EventRecorder()
        let runner = RecordingRunner(
            events: events,
            gate: RecordingGate(events: events, result: .success)
        )
        let supervisor = JournalSupervisor(
            gate: RecordingGate(events: events, result: .success),
            materializer: try await RecordingMaterializer(events: events),
            runner: runner,
            readinessGate: RecordingReadinessGate(events: events, result: .ready)
        )
        let receiptContext = configureInMemoryReceiptContext(supervisor)

        let started = await supervisor.start(journalRoot: try makeTemporaryDirectory())

        #expect(started)
        #expect(await events.snapshot() == ["materialize", "gate", "spawn", "readiness", "markReady"])
        #expect(supervisor.state == .running)
        #expect(supervisor.journalBinaryURL != nil)
        #expect(await runner.receiptAttemptIDs() == [receiptContext.attemptID])
    }

    @Test func runtimeStatusCanBeAppliedDirectlyForSinkSimulation() async throws {
        let supervisor = JournalSupervisor(
            gate: RecordingGate(events: EventRecorder(), result: .success),
            materializer: try await RecordingMaterializer(events: EventRecorder()),
            runner: RecordingRunner(events: EventRecorder()),
            readinessGate: RecordingReadinessGate(events: EventRecorder(), result: .ready)
        )
        _ = configureInMemoryReceiptContext(supervisor)
        let diagnostic = JournalDiagnostic(commandLabel: "journal", outputExcerpt: "done")

        supervisor.applyRuntimeStatus(.stopped(diagnostic))

        #expect(supervisor.runtimeStatus == .stopped(diagnostic))
    }

    @Test func bootFailuresEmitSupervisorNotBootingWithoutExcerpt() async throws {
        let marker = "AC9-SUPERVISOR-EXCERPT-MARKER"
        let log = RecordingClassifiedLogSink()
        let events = EventRecorder()
        let diagnostic = JournalDiagnostic(commandLabel: "gate", outputExcerpt: marker)
        let blockage = SingleSupervisorGateBlockage.portConflict(diagnostic)
        let supervisor = JournalSupervisor(
            gate: RecordingGate(events: events, result: .success),
            materializer: try await RecordingMaterializer(events: events),
            runner: RecordingRunner(
                events: events,
                gate: RecordingGate(events: events, result: .blocked(blockage))
            ),
            readinessGate: RecordingReadinessGate(events: events, result: .ready),
            classifiedLog: log
        )
        _ = configureInMemoryReceiptContext(supervisor)

        let started = await supervisor.start(journalRoot: try makeTemporaryDirectory())

        #expect(!started)
        let boot = try #require(log.emissions.first { $0.classification == "journal-lifecycle: supervisor-not-booting" })
        #expect(boot.level == .warning)
        #expect(boot.publicFields["reason"] == "gate-blocked")
        #expect(boot.publicFields["commandLabel"] == "gate")
        let concatenated = log.emissions.map {
            $0.classification + $0.publicFields.values.joined() + $0.publicFields.keys.joined()
        }.joined()
        #expect(!concatenated.contains(marker))
    }

    @Test func runtimeStatusOmitsDiagnosticDump() async throws {
        let marker = "AC6-STATUS-DUMP-MARKER"
        let log = RecordingClassifiedLogSink()
        let supervisor = JournalSupervisor(
            gate: RecordingGate(events: EventRecorder(), result: .success),
            materializer: try await RecordingMaterializer(events: EventRecorder()),
            runner: RecordingRunner(events: EventRecorder()),
            readinessGate: RecordingReadinessGate(events: EventRecorder(), result: .ready),
            classifiedLog: log
        )
        let diagnostic = JournalDiagnostic(commandLabel: "journal", outputExcerpt: marker)

        supervisor.applyRuntimeStatus(.stopped(diagnostic))

        let emission = try #require(log.emissions.first)
        #expect(emission.classification == "journal-lifecycle: runtime-status")
        #expect(emission.publicFields["kind"] == "stopped")
        #expect(emission.publicFields["commandLabel"] == "journal")
        let concatenated = emission.classification + emission.publicFields.values.joined()
        #expect(!concatenated.contains(marker))
        #expect(!concatenated.contains("outputExcerpt"))
    }

    @Test func restartSpawnFailureEmitsSupervisorNotBooting() async throws {
        let log = RecordingClassifiedLogSink()
        let events = EventRecorder()
        let runner = RecordingRunner(events: events, restartFails: true)
        let supervisor = JournalSupervisor(
            gate: RecordingGate(events: events, result: .success),
            materializer: try await RecordingMaterializer(events: events),
            runner: runner,
            readinessGate: RecordingReadinessGate(events: events, result: .ready),
            classifiedLog: log
        )
        _ = configureInMemoryReceiptContext(supervisor)
        _ = await supervisor.start(journalRoot: try makeTemporaryDirectory())
        let before = log.emissions.count

        let restarted = await supervisor.restart()

        #expect(!restarted)
        let boot = try #require(log.emissions.dropFirst(before).first { $0.classification == "journal-lifecycle: supervisor-not-booting" })
        #expect(boot.level == .error)
        #expect(boot.publicFields["reason"] == "spawn-failed")
    }

    @Test func stopAppliesStoppedByUserBecauseRunnerStopIsSilent() async throws {
        let events = EventRecorder()
        let runner = RecordingRunner(
            events: events,
            gate: RecordingGate(events: events, result: .success)
        )
        let supervisor = JournalSupervisor(
            gate: RecordingGate(events: events, result: .success),
            materializer: try await RecordingMaterializer(events: events),
            runner: runner,
            readinessGate: RecordingReadinessGate(events: events, result: .ready)
        )
        _ = configureInMemoryReceiptContext(supervisor)
        _ = await supervisor.start(journalRoot: try makeTemporaryDirectory())
        supervisor.applyRuntimeStatus(.running)

        let stopped = await supervisor.stop()

        #expect(stopped)
        #expect(await events.snapshot() == ["materialize", "gate", "spawn", "readiness", "markReady", "stop"])
        #expect(supervisor.state == .idle)
        #expect(supervisor.runtimeStatus == .stoppedByUser)
        #expect(supervisor.journalBinaryURL == nil)
    }

    @Test func restartPassthroughRerunsReadinessBeforeRunning() async throws {
        let events = EventRecorder()
        let runner = RecordingRunner(
            events: events,
            gate: RecordingGate(events: events, result: .success)
        )
        let supervisor = JournalSupervisor(
            gate: RecordingGate(events: events, result: .success),
            materializer: try await RecordingMaterializer(events: events),
            runner: runner,
            readinessGate: RecordingReadinessGate(events: events, result: .ready)
        )
        _ = configureInMemoryReceiptContext(supervisor)
        let root = try makeTemporaryDirectory()
        _ = await supervisor.start(journalRoot: root)
        supervisor.applyRuntimeStatus(.running)

        let restarted = await supervisor.restart()

        #expect(restarted)
        #expect(await events.snapshot() == [
            "materialize", "gate", "spawn", "readiness", "markReady",
            "gate", "restart", "readiness", "markReady"
        ])
        #expect(supervisor.state == .running)
        #expect(supervisor.activeJournalRoot == root.standardizedFileURL)
    }

    @Test func restartGateBlockedMapsToBlockedState() async throws {
        let events = EventRecorder()
        let diagnostic = JournalDiagnostic(commandLabel: "gate", outputExcerpt: "blocked")
        let blockage = SingleSupervisorGateBlockage.portConflict(diagnostic)
        let gate = RecordingGate(events: events, results: [.success, .blocked(blockage)])
        let runner = RecordingRunner(events: events, gate: gate)
        let supervisor = JournalSupervisor(
            gate: RecordingGate(events: events, result: .success),
            materializer: try await RecordingMaterializer(events: events),
            runner: runner,
            readinessGate: RecordingReadinessGate(events: events, result: .ready)
        )
        _ = configureInMemoryReceiptContext(supervisor)
        let root = try makeTemporaryDirectory()
        _ = await supervisor.start(journalRoot: root)
        supervisor.applyRuntimeStatus(.running)

        let restarted = await supervisor.restart()

        #expect(!restarted)
        #expect(await events.snapshot() == [
            "materialize", "gate", "spawn", "readiness", "markReady",
            "gate"
        ])
        #expect(supervisor.state == .blocked(diagnostic))
        #expect(supervisor.blockedReason == blockage.ownerMessage)
    }

    @Test func restartWithoutActiveRuntimeFailsClosed() async throws {
        let events = EventRecorder()
        let supervisor = JournalSupervisor(
            gate: RecordingGate(events: events, result: .success),
            materializer: try await RecordingMaterializer(events: events),
            runner: RecordingRunner(events: events),
            readinessGate: RecordingReadinessGate(events: events, result: .ready)
        )
        _ = configureInMemoryReceiptContext(supervisor)

        let restarted = await supervisor.restart()

        #expect(!restarted)
        #expect(await events.snapshot().isEmpty)
        if case .failed(let diagnostic) = supervisor.state {
            #expect(diagnostic.commandLabel == "solstone journal start --hosted-parent")
        } else {
            Issue.record("expected failed restart state")
        }
        if case .unknown(let diagnostic) = supervisor.runtimeStatus {
            #expect(diagnostic.commandLabel == "solstone journal start --hosted-parent")
        } else {
            Issue.record("expected unknown restart status")
        }
    }

    @Test func lazyReceiptContextFactoryIsRedirectedAndResolvedOnlyOnce() async throws {
        let events = EventRecorder()
        let runner = RecordingRunner(events: events, gate: RecordingGate(events: events, result: .success))
        let baseURL = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(".journal-supervisor-receipts-tests-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: baseURL, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: baseURL) }
        var factoryCalls = 0
        let supervisor = JournalSupervisor(
            gate: RecordingGate(events: events, result: .success),
            materializer: try await RecordingMaterializer(events: events),
            runner: runner,
            readinessGate: RecordingReadinessGate(events: events, result: .ready),
            receiptContextFactory: {
                factoryCalls += 1
                return JournalRuntimeEntryReceiptLaunch.begin(applicationSupportBaseURL: baseURL)
            }
        )

        _ = await supervisor.start(journalRoot: try makeTemporaryDirectory())
        _ = await supervisor.restart()

        #expect(factoryCalls == 1)
        #expect(await runner.receiptAttemptIDs().count == 1)
    }

    @Test func terminateWritesJournalMarkerBeforeStopLadder() async throws {
        let events = EventRecorder()
        let baseURL = try makeTemporaryDirectory()
        let markerURL = ExpectedExitMarker.markerURL(
            for: ExpectedExitMarker.journalMarkerDiscriminator,
            applicationSupportBaseURL: baseURL
        )
        defer { try? FileManager.default.removeItem(at: baseURL) }
        let runner = RecordingRunner(events: events, markerURLForStopCheck: markerURL)
        let supervisor = JournalSupervisor(
            gate: RecordingGate(events: events, result: .success),
            materializer: try await RecordingMaterializer(events: events),
            runner: runner,
            readinessGate: RecordingReadinessGate(events: events, result: .ready),
            markerURL: markerURL
        )
        _ = configureInMemoryReceiptContext(supervisor)

        await supervisor.terminate(reason: "journal-test-quit")
        let marker = ExpectedExitMarker.read(at: markerURL)

        #expect(await events.snapshot() == ["stopForTermination-after-marker"])
        #expect(marker?.reason == "journal-test-quit")
    }

    @Test func automaticReplacementReadinessUsesRunnerGenerationAndReturnsToRunning() async throws {
        let events = EventRecorder()
        let runner = RecordingRunner(events: events, gate: RecordingGate(events: events, result: .success))
        let supervisor = JournalSupervisor(
            gate: RecordingGate(events: events, result: .success),
            materializer: try await RecordingMaterializer(events: events),
            runner: runner,
            readinessGate: RecordingReadinessGate(events: events, results: [.ready, .ready])
        )
        _ = configureInMemoryReceiptContext(supervisor)
        _ = await supervisor.start(journalRoot: try makeTemporaryDirectory())
        await runner.setIdentity(.init(pid: 4243, kernelStartTime: 2_000, generation: 2))

        supervisor.applyRuntimeStatus(.restarting(generation: 2))
        try await waitForEvent(events, event: "markReady", count: 2)

        #expect(supervisor.state == .running)
        #expect(supervisor.runtimeStatus == .restarting(generation: 2))
    }

    @Test func automaticReplacementReadinessRecoversAfterRejectedManualRestart() async throws {
        let events = EventRecorder()
        let runner = RecordingRunner(events: events, restartFails: true)
        let supervisor = JournalSupervisor(
            gate: RecordingGate(events: events, result: .success),
            materializer: try await RecordingMaterializer(events: events),
            runner: runner,
            readinessGate: RecordingReadinessGate(events: events, results: [.ready, .ready])
        )
        _ = configureInMemoryReceiptContext(supervisor)
        _ = await supervisor.start(journalRoot: try makeTemporaryDirectory())

        let restarted = await supervisor.restart()

        #expect(!restarted)
        if case .failed(let diagnostic) = supervisor.state {
            #expect(diagnostic.commandLabel == "solstone journal start --hosted-parent")
        } else {
            Issue.record("expected failed restart state")
        }

        await runner.setIdentity(.init(pid: 4243, kernelStartTime: 2_000, generation: 2))
        supervisor.applyRuntimeStatus(.restarting(generation: 2))
        try await waitForEvent(events, event: "markReady", count: 2)

        #expect(supervisor.state == .running)
        #expect(supervisor.runtimeStatus == .restarting(generation: 2))
    }

    @Test func replacementReadinessFailureUsesExistingFailedUnknownOutcome() async throws {
        let events = EventRecorder()
        let diagnostic = JournalDiagnostic(commandLabel: "journal readiness", outputExcerpt: "no replacement marker")
        let runner = RecordingRunner(events: events, gate: RecordingGate(events: events, result: .success))
        let supervisor = JournalSupervisor(
            gate: RecordingGate(events: events, result: .success),
            materializer: try await RecordingMaterializer(events: events),
            runner: runner,
            readinessGate: RecordingReadinessGate(events: events, results: [.ready, .failed(diagnostic)])
        )
        _ = configureInMemoryReceiptContext(supervisor)
        _ = await supervisor.start(journalRoot: try makeTemporaryDirectory())
        await runner.setIdentity(.init(pid: 4243, kernelStartTime: 2_000, generation: 2))

        supervisor.applyRuntimeStatus(.restarting(generation: 2))
        try await waitForSupervisorState(supervisor, expected: .failed(diagnostic))

        #expect(supervisor.runtimeStatus == .unknown(diagnostic))
        #expect(await events.snapshot().last == "stop")
    }

    private func makeTemporaryDirectory() throws -> URL {
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("journal-supervisor-tests-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        return url
    }

    private func waitForEvent(_ events: EventRecorder, event: String, count: Int) async throws {
        let deadline = ContinuousClock.now.advanced(by: .seconds(1))
        while ContinuousClock.now < deadline {
            if await events.snapshot().filter({ $0 == event }).count >= count { return }
            try await Task.sleep(for: .milliseconds(5))
        }
        Issue.record("timed out waiting for \(event)")
    }

    private func waitForSupervisorState(
        _ supervisor: JournalSupervisor,
        expected: JournalSupervisorState
    ) async throws {
        let deadline = ContinuousClock.now.advanced(by: .seconds(1))
        while ContinuousClock.now < deadline {
            if supervisor.state == expected { return }
            try await Task.sleep(for: .milliseconds(5))
        }
        Issue.record("timed out waiting for supervisor state")
    }
}

private actor EventRecorder {
    private var events: [String] = []

    func append(_ event: String) {
        events.append(event)
    }

    func snapshot() -> [String] {
        events
    }
}

private actor RecordingGate: SingleSupervisorGating {
    private let events: EventRecorder
    private var results: [SingleSupervisorGateResult]

    init(events: EventRecorder, result: SingleSupervisorGateResult) {
        self.events = events
        results = [result]
    }

    init(events: EventRecorder, results: [SingleSupervisorGateResult]) {
        self.events = events
        self.results = results
    }

    func prepareForSpawn(journalRoot: URL) async -> SingleSupervisorGateResult {
        await events.append("gate")
        guard !results.isEmpty else { return .success }
        if results.count == 1 {
            return results[0]
        }
        return results.removeFirst()
    }
}

private actor RecordingMaterializer: RuntimeMaterializing {
    private let events: EventRecorder
    private let runtime: MaterializedRuntime

    init(events: EventRecorder) async throws {
        self.events = events
        runtime = try makeRuntime()
    }

    func materialize(excludingLiveKey liveKey: String?) async throws -> MaterializedRuntime {
        await events.append("materialize")
        return runtime
    }
}

private actor RecordingRunner: SupervisedChildRunning {
    private let events: EventRecorder
    private let markerURLForStopCheck: URL?
    private let gate: RecordingGate?
    private let restartFails: Bool
    private var runtimeKey: String?
    private var receiptContexts: [JournalRuntimeEntryReceiptContext] = []
    private var identity: SupervisedChildIdentity?

    init(
        events: EventRecorder,
        markerURLForStopCheck: URL? = nil,
        gate: RecordingGate? = nil,
        restartFails: Bool = false
    ) {
        self.events = events
        self.markerURLForStopCheck = markerURLForStopCheck
        self.gate = gate
        self.restartFails = restartFails
    }

    func start(
        runtime: MaterializedRuntime,
        journalRoot: URL,
        port: Int,
        receiptContext: JournalRuntimeEntryReceiptContext
    ) async throws {
        if let gate {
            switch await gate.prepareForSpawn(journalRoot: journalRoot) {
            case .success:
                break
            case .blocked(let blockage):
                throw SupervisedJournalRunnerError.gateBlocked(blockage)
            }
        }
        await events.append("spawn")
        receiptContexts.append(receiptContext)
        runtimeKey = runtime.key
        identity = .init(pid: 4242, kernelStartTime: 1_000, generation: 1)
    }

    func restart() async throws {
        if restartFails {
            throw RecordingRunnerError.restartRejected
        }
        if let gate {
            switch await gate.prepareForSpawn(journalRoot: URL(fileURLWithPath: "/tmp/journal-supervisor-restart")) {
            case .success:
                break
            case .blocked(let blockage):
                throw SupervisedJournalRunnerError.gateBlocked(blockage)
            }
        }
        await events.append("restart")
    }

    func stop() async {
        await events.append("stop")
        runtimeKey = nil
        identity = nil
    }

    func stopForTermination() async {
        if let markerURLForStopCheck,
           FileManager.default.fileExists(atPath: markerURLForStopCheck.path) {
            await events.append("stopForTermination-after-marker")
        } else {
            await events.append("stopForTermination-before-marker")
        }
        runtimeKey = nil
        identity = nil
    }

    func currentRuntimeKey() async -> String? {
        runtimeKey
    }

    func receiptAttemptIDs() -> [JournalRuntimeEntryAttemptID] {
        receiptContexts.map(\.attemptID)
    }

    func currentIdentity() async -> SupervisedChildIdentity? {
        identity
    }

    func terminalReason() async -> JournalDiagnostic? {
        nil
    }

    func markReady(identity: SupervisedChildIdentity) async -> Bool {
        guard identity == self.identity else { return false }
        await events.append("markReady")
        return true
    }

    func setIdentity(_ identity: SupervisedChildIdentity?) {
        self.identity = identity
    }
}

private enum RecordingRunnerError: LocalizedError, Sendable {
    case restartRejected

    var errorDescription: String? {
        "journal child containment is in progress"
    }
}

private actor RecordingReadinessGate: JournalReadinessChecking {
    private let events: EventRecorder
    private var results: [JournalReadinessResult]

    init(events: EventRecorder, result: JournalReadinessResult) {
        self.events = events
        results = [result]
    }

    init(events: EventRecorder, results: [JournalReadinessResult]) {
        self.events = events
        self.results = results
    }

    func waitUntilReady(
        journalRoot: URL,
        runtime: MaterializedRuntime,
        timeout: Duration,
        terminalCheck: @escaping @Sendable () async -> JournalDiagnostic?,
        identityProvider: @escaping @Sendable () async -> SupervisedChildIdentity?,
        readinessAcceptance: @escaping @Sendable (SupervisedChildIdentity) async -> Bool
    ) async -> JournalReadinessResult {
        await events.append("readiness")
        let result = results.count > 1 ? results.removeFirst() : (results.first ?? .ready)
        if case .ready = result, let identity = await identityProvider() {
            _ = await readinessAcceptance(identity)
        }
        return result
    }
}
