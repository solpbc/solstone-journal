// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import Foundation
import Darwin
import JournalRuntimeTestSupport
import SolstoneCore
import Testing
@testable import JournalRuntime

@Suite("SupervisedJournalRunner")
struct SupervisedJournalRunnerTests {
    @Test func waitsForRequiredModelRepairBeforeSpawning() async throws {
        let fixture = try RunnerFixture()
        defer { fixture.clear() }
        let repair = DeferredRequiredModels()
        let spawner = RecordingProcessSpawner(pid: 4242)
        let gate = RecordingRunnerGate(result: .success)
        let runner = SupervisedJournalRunner(
            statusSink: { _ in }, gate: gate, requiredModels: repair,
            containmentEvidenceReader: FixedStartTimeReader(startTime: 1_000),
            processSpawner: spawner, pidExists: { _ in false }
        )
        let start = Task {
            try await runner.start(runtime: fixture.runtime, journalRoot: fixture.realJournalRoot,
                                   port: 5015, receiptContext: makeReceiptFixture().context)
        }
        let deadline = ContinuousClock.now.advanced(by: .seconds(2))
        while !(await repair.entered), ContinuousClock.now < deadline { await Task.yield() }
        #expect(await repair.entered)
        #expect(spawner.spawnRequests().isEmpty)
        await repair.finish()
        try await start.value
        #expect(spawner.spawnRequests().count == 1)
        #expect(gate.roots().count == 2)
        await runner.stop()
    }

    @Test func terminationCancelsAndReapsBlockedModelInstallerBeforeReturning() async throws {
        let fixture = try RunnerFixture()
        defer { fixture.clear() }
        let helper = fixture.runtime.layout.solstoneBinary
        try Data("#!/bin/sh\nprintf '%s\\n' \"$$\" > preparation.pid\nexec /bin/sleep 120\n".utf8).write(to: helper)
        try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: helper.path)
        let spawner = RecordingProcessSpawner(pid: 4242)
        let runner = SupervisedJournalRunner(
            statusSink: { _ in }, gate: RecordingRunnerGate(result: .success),
            requiredModels: JournalRequiredModelsReconciler(),
            containmentEvidenceReader: FixedStartTimeReader(startTime: 1_000),
            processSpawner: spawner, pidExists: { _ in false }
        )
        let start = Task {
            try await runner.start(runtime: fixture.runtime, journalRoot: fixture.realJournalRoot,
                                   port: 5015, receiptContext: makeReceiptFixture().context)
        }
        let pidFile = fixture.realJournalRoot.appendingPathComponent("preparation.pid")
        let deadline = ContinuousClock.now.advanced(by: .seconds(5))
        while !FileManager.default.fileExists(atPath: pidFile.path), ContinuousClock.now < deadline {
            try await Task.sleep(for: .milliseconds(10))
        }
        let pidText = try? String(contentsOf: pidFile, encoding: .utf8)
        let pid = pidText.flatMap { Int32($0.trimmingCharacters(in: .whitespacesAndNewlines)) }
        #expect(pid != nil)
        #expect(spawner.spawnRequests().isEmpty)
        let stoppedAt = ContinuousClock.now
        await runner.stopForTermination()
        #expect(stoppedAt.duration(to: .now) < .seconds(5))
        await #expect(throws: CancellationError.self) { try await start.value }
        if let pid {
            #expect(Darwin.kill(pid, 0) == -1)
            #expect(errno == ESRCH)
        }
        #expect(spawner.spawnRequests().isEmpty)
        #expect(await runner.currentIdentity() == nil)
    }

    @Test func requiredModelFailureNeverSpawnsOrAdmitsRuntime() async throws {
        let fixture = try RunnerFixture()
        defer { fixture.clear() }
        let subprocess = FakeSubprocessRunner()
        subprocess.enqueue("install-models", .success(exitCode: 65))
        let spawner = RecordingProcessSpawner(pid: 4242)
        let receipts = makeReceiptFixture()
        let runner = SupervisedJournalRunner(
            statusSink: { _ in },
            gate: RecordingRunnerGate(result: .success),
            requiredModels: JournalRequiredModelsReconciler(makeRunner: { _ in subprocess }),
            containmentEvidenceReader: FixedStartTimeReader(startTime: 1_000),
            processSpawner: spawner,
            pidExists: { _ in false }
        )
        await #expect(throws: SupervisedJournalRunnerError.self) {
            try await runner.start(runtime: fixture.runtime, journalRoot: fixture.realJournalRoot,
                                   port: 5015, receiptContext: receipts.context)
        }
        #expect(spawner.spawnRequests().isEmpty)
        #expect(await runner.currentIdentity() == nil)
        #expect(payloadEntries(receipts.sink.storedRecords(attemptID: receipts.context.attemptID)).isEmpty)
        #expect(subprocess.invocations.count == 1)
    }

    @Test func blockedSupervisorGateDoesNotMutateRequiredModels() async throws {
        let fixture = try RunnerFixture()
        defer { fixture.clear() }
        let subprocess = FakeSubprocessRunner()
        let diagnostic = JournalDiagnostic(commandLabel: "gate", outputExcerpt: "blocked")
        let spawner = RecordingProcessSpawner(pid: 4242)
        let runner = SupervisedJournalRunner(
            statusSink: { _ in },
            gate: RecordingRunnerGate(result: .blocked(.portConflict(diagnostic))),
            requiredModels: JournalRequiredModelsReconciler(makeRunner: { _ in subprocess }),
            containmentEvidenceReader: FixedStartTimeReader(startTime: 1_000),
            processSpawner: spawner,
            pidExists: { _ in false }
        )
        await #expect(throws: SupervisedJournalRunnerError.self) {
            try await runner.start(runtime: fixture.runtime, journalRoot: fixture.realJournalRoot,
                                   port: 5015, receiptContext: makeReceiptFixture().context)
        }
        #expect(subprocess.invocations.isEmpty)
        #expect(spawner.spawnRequests().isEmpty)
    }

    @Test func supervisorGateClosingDuringModelRepairPreventsSpawn() async throws {
        let fixture = try RunnerFixture()
        defer { fixture.clear() }
        let spawner = RecordingProcessSpawner(pid: 4242)
        let runner = SupervisedJournalRunner(
            statusSink: { _ in }, gate: GateClosingAfterPreparation(),
            requiredModels: JournalRequiredModelsReconciler(makeRunner: { _ in FakeSubprocessRunner() }),
            containmentEvidenceReader: FixedStartTimeReader(startTime: 1_000),
            processSpawner: spawner, pidExists: { _ in false }
        )
        await #expect(throws: SupervisedJournalRunnerError.self) {
            try await runner.start(runtime: fixture.runtime, journalRoot: fixture.realJournalRoot,
                                   port: 5015, receiptContext: makeReceiptFixture().context)
        }
        #expect(spawner.spawnRequests().isEmpty)
    }

    @Test func launchGatesWithCanonicalRootAndDoesNotStampJournalEnvironment() async throws {
        let fixture = try RunnerFixture()
        defer { fixture.clear() }
        let gate = RecordingRunnerGate(result: .success)
        let spawner = RecordingProcessSpawner(pid: 4242)
        let runner = SupervisedJournalRunner(
            clock: NoopRunnerClock(),
            statusSink: { _ in },
            gate: gate,
            containmentEvidenceReader: FixedStartTimeReader(startTime: 1_000.0),
            processSpawner: spawner,
            pidExists: { _ in false }
        )

        try await runner.start(
            runtime: fixture.runtime,
            journalRoot: fixture.linkedJournalRoot,
            port: 5015,
            receiptContext: makeReceiptFixture().context
        )

        let requests = spawner.spawnRequests()
        #expect(requests.count == 1)
        #expect(requests.first?.executableURL == fixture.runtime.layout.solstoneBinary)
        #expect(requests.first?.currentDirectoryURL.path == canonicalPath(fixture.realJournalRoot))
        #expect(requests.first?.arguments == ["journal", "start", "--hosted-parent", "5015"])
        #expect(requests.first?.environment["SOLSTONE_JOURNAL"] == nil)
        #expect(gate.roots() == [canonicalPath(fixture.realJournalRoot)])
        #expect(await runner.currentIdentity()?.pid == 4242)
        #expect(await runner.currentIdentity()?.kernelStartTime == 1_000.0)
        await runner.stop()
    }

    @Test func gateBlockedPreventsSpawn() async throws {
        let fixture = try RunnerFixture()
        defer { fixture.clear() }
        let diagnostic = JournalDiagnostic(commandLabel: "gate", outputExcerpt: "blocked")
        let blockage = SingleSupervisorGateBlockage.portConflict(diagnostic)
        let spawner = RecordingProcessSpawner(pid: 4242)
        let runner = SupervisedJournalRunner(
            clock: NoopRunnerClock(),
            statusSink: { _ in },
            gate: RecordingRunnerGate(result: .blocked(blockage)),
            containmentEvidenceReader: FixedStartTimeReader(startTime: 1_000.0),
            processSpawner: spawner,
            pidExists: { _ in false }
        )

        do {
            try await runner.start(
                runtime: fixture.runtime,
                journalRoot: fixture.realJournalRoot,
                port: 5015,
                receiptContext: makeReceiptFixture().context
            )
            Issue.record("expected gateBlocked")
        } catch let error as SupervisedJournalRunnerError {
            #expect(error == .gateBlocked(blockage))
        }

        #expect(spawner.spawnRequests().isEmpty)
    }

    @Test func writesPayloadEntryForInitialRestartAndBackoffAdmissionsBeforeExit() async throws {
        let fixture = try RunnerFixture()
        defer { fixture.clear() }
        let receipts = makeReceiptFixture()
        let spawner = RecordingProcessSpawner(pids: [4242, 4243, 4244])
        let runner = SupervisedJournalRunner(
            clock: NoopRunnerClock(),
            statusSink: { _ in },
            gate: RecordingRunnerGate(result: .success),
            containmentEvidenceReader: FixedStartTimeReader(startTimes: [1_000, 2_000, 3_000]),
            processSpawner: spawner,
            pidExists: { _ in false }
        )

        try await runner.start(
            runtime: fixture.runtime,
            journalRoot: fixture.realJournalRoot,
            port: 5015,
            receiptContext: receipts.context
        )
        try await runner.restart()
        spawner.exit(spawn: 1, status: 9)
        try await waitForSpawnCount(spawner, count: 3)

        let entries = payloadEntries(receipts.sink.storedRecords(attemptID: receipts.context.attemptID))
        #expect(entries.map(\.generation) == [1, 2, 3])
        #expect(payloadExits(receipts.sink.storedRecords(attemptID: receipts.context.attemptID)).count == 1)
    }

    @Test func writesExactlyOneTerminalExitAfterObservedExpectedAndUnexpectedExit() async throws {
        let fixture = try RunnerFixture()
        defer { fixture.clear() }
        let expected = makeReceiptFixture()
        let expectedSpawner = RecordingProcessSpawner(pids: [4242])
        let expectedRunner = SupervisedJournalRunner(
            clock: NoopRunnerClock(), statusSink: { _ in }, gate: RecordingRunnerGate(result: .success),
            containmentEvidenceReader: FixedStartTimeReader(startTime: 1_000), processSpawner: expectedSpawner, pidExists: { _ in false }
        )
        try await expectedRunner.start(runtime: fixture.runtime, journalRoot: fixture.realJournalRoot, port: 5015, receiptContext: expected.context)
        await expectedRunner.stop()
        #expect(payloadExits(expected.sink.storedRecords(attemptID: expected.context.attemptID)).isEmpty)
        expectedSpawner.exit(spawn: 0, status: 0)
        try await waitForPayloadExitCount(expected.sink, attemptID: expected.context.attemptID, count: 1)
        #expect(payloadExits(expected.sink.storedRecords(attemptID: expected.context.attemptID)).map(\.expectedStop) == [true])

        let unexpected = makeReceiptFixture()
        let unexpectedSpawner = RecordingProcessSpawner(pids: [4243, 4244])
        let unexpectedRunner = SupervisedJournalRunner(
            clock: NoopRunnerClock(), statusSink: { _ in }, gate: RecordingRunnerGate(result: .success),
            containmentEvidenceReader: FixedStartTimeReader(startTimes: [2_000, 3_000]), processSpawner: unexpectedSpawner, pidExists: { _ in false }
        )
        try await unexpectedRunner.start(runtime: fixture.runtime, journalRoot: fixture.realJournalRoot, port: 5015, receiptContext: unexpected.context)
        unexpectedSpawner.exit(spawn: 0, status: 9)
        try await waitForSpawnCount(unexpectedSpawner, count: 2)
        try await waitForPayloadExitCount(unexpected.sink, attemptID: unexpected.context.attemptID, count: 1)
        #expect(payloadExits(unexpected.sink.storedRecords(attemptID: unexpected.context.attemptID)).map(\.expectedStop) == [false])
    }

    @Test func admissionFailureWritesNoReceiptAndStillLiveStoppedChildWritesNoExit() async throws {
        let fixture = try RunnerFixture()
        defer { fixture.clear() }
        let unadmitted = makeReceiptFixture()
        let unadmittedSpawner = RecordingProcessSpawner(pids: [4242])
        let unadmittedRunner = SupervisedJournalRunner(
            clock: NoopRunnerClock(), statusSink: { _ in }, gate: RecordingRunnerGate(result: .success),
            containmentEvidenceReader: FixedStartTimeReader(startTimes: [nil]), processSpawner: unadmittedSpawner, pidExists: { _ in false }
        )
        await #expect(throws: SupervisedJournalRunnerError.self) {
            try await unadmittedRunner.start(
                runtime: fixture.runtime,
                journalRoot: fixture.realJournalRoot,
                port: 5015,
                receiptContext: unadmitted.context
            )
        }
        unadmittedSpawner.exit(spawn: 0, status: 9)
        await Task.yield()
        #expect(payloadEntries(unadmitted.sink.storedRecords(attemptID: unadmitted.context.attemptID)).isEmpty)
        #expect(payloadExits(unadmitted.sink.storedRecords(attemptID: unadmitted.context.attemptID)).isEmpty)

        let live = makeReceiptFixture()
        let liveSpawner = RecordingProcessSpawner(pids: [4243], closeParentInputStopsProcess: false)
        let liveRunner = SupervisedJournalRunner(
            clock: AdvancingRunnerClock(), statusSink: { _ in }, gate: RecordingRunnerGate(result: .success),
            containmentEvidenceReader: FixedStartTimeReader(startTime: 3_000), processSpawner: liveSpawner, pidExists: { _ in true },
            terminate: { _, _ in 0 }
        )
        try await liveRunner.start(runtime: fixture.runtime, journalRoot: fixture.realJournalRoot, port: 5015, receiptContext: live.context)
        await liveRunner.stop()
        #expect(payloadExits(live.sink.storedRecords(attemptID: live.context.attemptID)).isEmpty)
    }

    @Test(arguments: [
        "missing",
        "CandidateProvenanceMalformed",
        "CandidateProvenanceDuplicate",
        "CandidateProvenanceTargetMismatch"
    ])
    func missingOrRejectedProvenanceWritesOnlyOuterEntry(named name: String) async throws {
        let fixture = try RunnerFixture()
        defer { fixture.clear() }
        let appBundle = try makeTestBundle(at: fixture.root, named: "JournalAppIdentity")
        let provenanceBundle: Bundle
        if name == "missing" {
            provenanceBundle = try makeTestBundle(at: fixture.root, named: "MissingProvenance")
        } else {
            provenanceBundle = try candidateProvenanceFixtureBundle(named: name)
        }
        let receipts = makeLaunchReceiptFixture(
            appBundle: appBundle,
            provenanceBundle: provenanceBundle
        )

        try await assertOuterOnlyClosedChainAfterExit(fixture: fixture, receipts: receipts)
    }

    @Test func provenanceResolutionIgnoresUnselectedBundleDecoy() async throws {
        let fixture = try RunnerFixture()
        defer { fixture.clear() }

        let validProvenance = try validCandidateProvenanceData()
        let unselectedBundle = try makeTestBundle(
            at: fixture.root,
            named: "UnselectedValidProvenance",
            provenanceData: validProvenance
        )
        #expect(FileManager.default.fileExists(atPath: unselectedBundle.bundleURL.path))

        let appBundle = try makeTestBundle(at: fixture.root, named: "JournalAppIdentity")
        let emptyProvenanceBundle = try makeTestBundle(at: fixture.root, named: "MissingProvenance")
        let receipts = makeLaunchReceiptFixture(
            appBundle: appBundle,
            provenanceBundle: emptyProvenanceBundle
        )

        try await assertOuterOnlyClosedChainAfterExit(fixture: fixture, receipts: receipts)
    }

    @Test func provenanceResolverDoesNotUseProcessGlobalFallbacks() throws {
        // The package root (platforms/macos), from this file's own path, so the scan never
        // depends on the runner's working directory.
        let packageRoot = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let sourceURL = packageRoot.appendingPathComponent("Sources/JournalRuntime/JournalRuntimeEntryReceiptProvenance.swift")
        let source = try String(contentsOf: sourceURL, encoding: .utf8)
        let forbiddenFallbacks = [
            "currentDirectoryPath",
            "ProcessInfo",
            ".environment",
            "Bundle.main",
            "solstone-runtime"
        ]

        for fallback in forbiddenFallbacks {
            #expect(!source.contains(fallback), "provenance resolver must not use \(fallback) fallback")
        }
    }

    @Test func keepsPIDReuseExitsBoundToTheirAdmittedKernelStartTime() async throws {
        let fixture = try RunnerFixture()
        defer { fixture.clear() }
        let receipts = makeReceiptFixture()
        let spawner = RecordingProcessSpawner(pids: [4242, 4242])
        let runner = SupervisedJournalRunner(
            clock: NoopRunnerClock(), statusSink: { _ in }, gate: RecordingRunnerGate(result: .success),
            containmentEvidenceReader: FixedStartTimeReader(startTimes: [1_000, 2_000]), processSpawner: spawner, pidExists: { _ in false }
        )
        try await runner.start(runtime: fixture.runtime, journalRoot: fixture.realJournalRoot, port: 5015, receiptContext: receipts.context)
        try await runner.restart()
        await runner.stop()
        spawner.exit(spawn: 0, status: 0)
        try await waitForPayloadExitCount(receipts.sink, attemptID: receipts.context.attemptID, count: 1)
        spawner.exit(spawn: 1, status: 0)
        try await waitForPayloadExitCount(receipts.sink, attemptID: receipts.context.attemptID, count: 2)

        let records = receipts.sink.storedRecords(attemptID: receipts.context.attemptID)
        #expect(payloadEntries(records).map(\.childKernelStartTimeMicroseconds) == [1_000_000_000, 2_000_000_000])
        #expect(payloadExits(records).map(\.childKernelStartTimeMicroseconds) == [1_000_000_000, 2_000_000_000])
    }

    @Test func synchronousExitDuringRunIsAdmittedAndProducesOneReplacement() async throws {
        let fixture = try RunnerFixture()
        defer { fixture.clear() }
        let receipts = makeReceiptFixture()
        let spawner = RecordingProcessSpawner(
            pids: [4242, 4243],
            exitFirstRunWithStatus: 9
        )
        let runner = SupervisedJournalRunner(
            clock: NoopRunnerClock(),
            statusSink: { _ in },
            gate: RecordingRunnerGate(result: .success),
            containmentEvidenceReader: FixedStartTimeReader(startTimes: [1_000, 2_000]),
            processSpawner: spawner,
            pidExists: { _ in false }
        )

        try await runner.start(
            runtime: fixture.runtime,
            journalRoot: fixture.realJournalRoot,
            port: 5015,
            receiptContext: receipts.context
        )
        try await waitForSpawnCount(spawner, count: 2)

        #expect(await runner.currentIdentity() == .init(pid: 4243, kernelStartTime: 2_000, generation: 2))
        #expect(payloadEntries(receipts.sink.storedRecords(attemptID: receipts.context.attemptID)).map(\.generation) == [1, 2])
        #expect(payloadExits(receipts.sink.storedRecords(attemptID: receipts.context.attemptID)).map(\.expectedStop) == [false])
    }

    @Test func duplicateExitCallbacksScheduleOnlyOneReplacement() async throws {
        let fixture = try RunnerFixture()
        defer { fixture.clear() }
        let spawner = RecordingProcessSpawner(pids: [4242, 4243])
        let runner = SupervisedJournalRunner(
            clock: NoopRunnerClock(),
            statusSink: { _ in },
            gate: RecordingRunnerGate(result: .success),
            containmentEvidenceReader: FixedStartTimeReader(startTimes: [1_000, 2_000]),
            processSpawner: spawner,
            pidExists: { _ in false }
        )

        try await runner.start(runtime: fixture.runtime, journalRoot: fixture.realJournalRoot, port: 5015, receiptContext: makeReceiptFixture().context)
        spawner.exit(spawn: 0, status: 9)
        spawner.exit(spawn: 0, status: 9)
        try await waitForSpawnCount(spawner, count: 2)
        try await Task.sleep(for: .milliseconds(20))

        #expect(spawner.spawnRequests().count == 2)
        #expect(await runner.currentIdentity()?.generation == 2)
    }

    @Test func cleanContainmentPreservesExistingFiveExitBreakerThreshold() async throws {
        let fixture = try RunnerFixture()
        defer { fixture.clear() }
        let statuses = RunnerStatusRecorder()
        let spawner = RecordingProcessSpawner(pids: [4242, 4243, 4244, 4245, 4246])
        let runner = SupervisedJournalRunner(
            clock: NoopRunnerClock(),
            statusSink: { statuses.append($0) },
            gate: RecordingRunnerGate(result: .success),
            containmentEvidenceReader: FixedStartTimeReader(startTimes: [1_000, 2_000, 3_000, 4_000, 5_000]),
            processSpawner: spawner,
            pidExists: { _ in false }
        )

        try await runner.start(runtime: fixture.runtime, journalRoot: fixture.realJournalRoot, port: 5015, receiptContext: makeReceiptFixture().context)
        for spawn in 0..<4 {
            spawner.exit(spawn: spawn, status: 9)
            try await waitForSpawnCount(spawner, count: spawn + 2)
        }
        spawner.exit(spawn: 4, status: 9)
        try await waitForStatusCount(statuses, count: 5)

        #expect(spawner.spawnRequests().count == 5)
        guard case .stopped(let diagnostic) = statuses.snapshot().last else {
            Issue.record("expected terminal breaker status")
            return
        }
        #expect(diagnostic.outputExcerpt == UICopy.JOURNAL_CHILD_BREAKER_TRIPPED)
    }

    @Test func automaticRelaunchRetriesAGateThatCouldNotCheck() async throws {
        let fixture = try RunnerFixture()
        defer { fixture.clear() }
        let statuses = RunnerStatusRecorder()
        let spawner = RecordingProcessSpawner(pids: [4242, 4243])
        let blockage = SingleSupervisorGateBlockage.legacyServiceVerificationFailed(
            JournalDiagnostic(commandLabel: "legacy journal service", outputExcerpt: "legacy service could not be inspected")
        )
        let runner = SupervisedJournalRunner(
            clock: NoopRunnerClock(),
            statusSink: { statuses.append($0) },
            gate: ScriptedRunnerGate([.success, .blocked(blockage), .success]),
            containmentEvidenceReader: FixedStartTimeReader(startTimes: [1_000, 2_000]),
            processSpawner: spawner,
            pidExists: { _ in false }
        )

        try await runner.start(runtime: fixture.runtime, journalRoot: fixture.realJournalRoot, port: 5015, receiptContext: makeReceiptFixture().context)
        spawner.exit(spawn: 0, status: 9)
        try await waitForSpawnCount(spawner, count: 2)

        #expect(await runner.currentIdentity()?.generation == 2)
        #expect(!statuses.snapshot().contains { if case .stopped = $0 { return true } else { return false } })
    }

    @Test func automaticRelaunchThatStaysBlockedStopsWithTheGatesReason() async throws {
        let fixture = try RunnerFixture()
        defer { fixture.clear() }
        let statuses = RunnerStatusRecorder()
        let spawner = RecordingProcessSpawner(pids: [4242])
        let blockage = SingleSupervisorGateBlockage.legacyServiceVerificationFailed(
            JournalDiagnostic(commandLabel: "legacy journal service", outputExcerpt: "legacy service could not be inspected")
        )
        let runner = SupervisedJournalRunner(
            clock: NoopRunnerClock(),
            statusSink: { statuses.append($0) },
            gate: ScriptedRunnerGate([.success], then: .blocked(blockage)),
            containmentEvidenceReader: FixedStartTimeReader(startTimes: [1_000]),
            processSpawner: spawner,
            pidExists: { _ in false }
        )

        try await runner.start(runtime: fixture.runtime, journalRoot: fixture.realJournalRoot, port: 5015, receiptContext: makeReceiptFixture().context)
        spawner.exit(spawn: 0, status: 9)
        try await waitForStatus(statuses) { if case .stopped = $0 { return true } else { return false } }

        #expect(spawner.spawnRequests().count == 1)
        guard case .stopped(let diagnostic) = statuses.snapshot().last else {
            Issue.record("expected a terminal stop")
            return
        }
        #expect(diagnostic.outputExcerpt == UICopy.JOURNAL_SPAWN_SERVICE_CHECK_FAILED)
    }

    @Test func slowGateBlocksStillStopAfterTheLimitAndShowStartingMeanwhile() async throws {
        let fixture = try RunnerFixture()
        defer { fixture.clear() }
        let statuses = RunnerStatusRecorder()
        let spawner = RecordingProcessSpawner(pids: [4242])
        let clock = AdvancingRunnerClock()
        let blockage = SingleSupervisorGateBlockage.legacyServiceVerificationFailed(
            JournalDiagnostic(commandLabel: "legacy journal service", outputExcerpt: "legacy service could not be inspected")
        )
        // Each blocked check takes 40 s, as a launchctl timeout would: five
        // failures never fall inside one minute.
        let runner = SupervisedJournalRunner(
            clock: clock,
            statusSink: { statuses.append($0) },
            gate: ScriptedRunnerGate([.success], then: .blocked(blockage), blockedCost: .seconds(40), clock: clock),
            containmentEvidenceReader: FixedStartTimeReader(startTimes: [1_000]),
            processSpawner: spawner,
            pidExists: { _ in false }
        )

        try await runner.start(runtime: fixture.runtime, journalRoot: fixture.realJournalRoot, port: 5015, receiptContext: makeReceiptFixture().context)
        spawner.exit(spawn: 0, status: 9)
        try await waitForStatus(statuses) { if case .stopped = $0 { return true } else { return false } }

        let snapshot = statuses.snapshot()
        #expect(snapshot.filter { $0 == .restarting(generation: nil) }.count == 4)
        guard case .stopped(let diagnostic) = snapshot.last else {
            Issue.record("expected a terminal stop")
            return
        }
        #expect(diagnostic.outputExcerpt == UICopy.JOURNAL_SPAWN_SERVICE_CHECK_FAILED)
        #expect(spawner.spawnRequests().count == 1)
    }

    @Test func restartDuringAnAutomaticRelaunchIsNotStoppedByIt() async throws {
        let fixture = try RunnerFixture()
        defer { fixture.clear() }
        let statuses = RunnerStatusRecorder()
        let spawner = RecordingProcessSpawner(pids: [4242, 4243])
        let gate = HoldingRunnerGate()
        let runner = SupervisedJournalRunner(
            clock: NoopRunnerClock(),
            statusSink: { statuses.append($0) },
            gate: gate,
            containmentEvidenceReader: FixedStartTimeReader(startTimes: [1_000, 2_000]),
            processSpawner: spawner,
            pidExists: { _ in false }
        )

        try await runner.start(runtime: fixture.runtime, journalRoot: fixture.realJournalRoot, port: 5015, receiptContext: makeReceiptFixture().context)
        spawner.exit(spawn: 0, status: 9)
        // The automatic relaunch is now inside its (slow) gate.
        try await gate.waitForHeldCall()
        try await runner.restart()
        #expect(spawner.spawnRequests().count == 2)
        let restarted = try #require(await runner.currentIdentity())
        await gate.releaseHeldCall()
        try await Task.sleep(for: .milliseconds(50))

        #expect(await runner.currentIdentity() == restarted)
        #expect(await runner.terminalReason() == nil)
        #expect(!statuses.snapshot().contains { if case .stopped = $0 { return true } else { return false } })
        #expect(spawner.spawnRequests().count == 2)
    }

    @Test func unresolvedContainmentSignalsOnlyVerifiedMembersAndNeverReachesRelaunchGate() async throws {
        let fixture = try RunnerFixture()
        defer { fixture.clear() }
        let leaderPID: pid_t = 4242
        let foreignPID: pid_t = 5252
        let startTime = Date().timeIntervalSince1970 - 1
        let reader = ScriptedContainmentEvidenceReader(
            memberships: [[leaderPID, foreignPID], [], []],
            evidenceByPID: [
                leaderPID: .init(
                    pid: leaderPID,
                    processGroupID: leaderPID,
                    uid: getuid(),
                    username: currentUsername(),
                    kernelStartTime: startTime
                ),
                foreignPID: .init(
                    pid: foreignPID,
                    processGroupID: leaderPID,
                    uid: getuid() + 1,
                    username: "foreign",
                    kernelStartTime: startTime
                ),
                5353: .init(
                    pid: 5353,
                    processGroupID: leaderPID,
                    uid: getuid(),
                    username: currentUsername(),
                    kernelStartTime: startTime
                )
            ]
        )
        let signals = RunnerSignalRecorder()
        let statuses = RunnerStatusRecorder()
        let gate = RecordingRunnerGate(result: .success)
        let spawner = RecordingProcessSpawner(pid: leaderPID)
        let runner = SupervisedJournalRunner(
            clock: NoopRunnerClock(),
            statusSink: { statuses.append($0) },
            gate: gate,
            containmentEvidenceReader: reader,
            processSpawner: spawner,
            pidExists: { _ in false },
            terminate: { pid, signal in
                signals.append(pid: pid, signal: signal)
                return 0
            }
        )

        try await runner.start(runtime: fixture.runtime, journalRoot: fixture.realJournalRoot, port: 5015, receiptContext: makeReceiptFixture().context)
        spawner.exit(spawn: 0, status: 9)
        try await waitForStatusCount(statuses, count: 1)

        #expect(spawner.spawnRequests().count == 1)
        #expect(gate.roots().count == 1)
        #expect(signals.snapshot() == [.init(pid: leaderPID, signal: SIGTERM)])
        guard case .stopped(let diagnostic) = statuses.snapshot().last else {
            Issue.record("expected containment terminal status")
            return
        }
        #expect(diagnostic.outputExcerpt == UICopy.JOURNAL_CHILD_CONTAINMENT_UNRESOLVED)
    }

    @Test func retainsValidatedParentLossCoordinatorUntilItRetires() async throws {
        let fixture = try RunnerFixture()
        defer { fixture.clear() }
        let leaderPID: pid_t = 4242
        let coordinatorPID: pid_t = 4243
        let startTime = Double(Int64(Date().timeIntervalSince1970))
        let birthMicros = Int64(startTime * 1_000_000)
        let reader = ScriptedContainmentEvidenceReader(
            memberships: [[leaderPID, coordinatorPID], [], []],
            evidenceByPID: [
                leaderPID: .init(
                    pid: leaderPID,
                    processGroupID: leaderPID,
                    uid: getuid(),
                    username: currentUsername(),
                    kernelStartTime: startTime
                ),
                coordinatorPID: .init(
                    pid: coordinatorPID,
                    processGroupID: coordinatorPID,
                    uid: getuid(),
                    username: currentUsername(),
                    kernelStartTime: startTime
                )
            ]
        )
        let signals = RunnerSignalRecorder()
        let statuses = RunnerStatusRecorder()
        let gate = RecordingRunnerGate(result: .success)
        let spawner = RecordingProcessSpawner(pid: leaderPID)
        let ledgerDirectory = fixture.realJournalRoot.appendingPathComponent(
            "health/parent-loss",
            isDirectory: true
        )
        try FileManager.default.createDirectory(at: ledgerDirectory, withIntermediateDirectories: true)
        let ledger = "{\"schema\":1,\"coordinator\":{\"pid\":\(coordinatorPID),\"birth\":{\"epoch_micros\":\(birthMicros)}},\"supervisor\":{\"pid\":\(leaderPID),\"birth\":{\"epoch_micros\":\(birthMicros)}}}"
        try Data(ledger.utf8).write(to: ledgerDirectory.appendingPathComponent("active-generation.json"))
        let runner = SupervisedJournalRunner(
            clock: NoopRunnerClock(),
            statusSink: { statuses.append($0) },
            gate: gate,
            containmentEvidenceReader: reader,
            processSpawner: spawner,
            pidExists: { _ in false },
            terminate: { pid, signal in
                signals.append(pid: pid, signal: signal)
                return 0
            }
        )

        try await runner.start(runtime: fixture.runtime, journalRoot: fixture.realJournalRoot, port: 5015, receiptContext: makeReceiptFixture().context)
        let current = try #require(await runner.currentIdentity())
        #expect(await runner.markReady(identity: current))
        spawner.exit(spawn: 0, status: 9)
        try await waitForStatusCount(statuses, count: 2)

        #expect(spawner.spawnRequests().count == 1)
        #expect(gate.roots().count == 1)
        #expect(signals.snapshot() == [.init(pid: leaderPID, signal: SIGTERM)])
        guard case .stopped(let diagnostic) = statuses.snapshot().last else {
            Issue.record("expected retained-authority containment status")
            return
        }
        #expect(diagnostic.outputExcerpt == UICopy.JOURNAL_CHILD_CONTAINMENT_UNRESOLVED)
    }

    @Test func markReadyRejectsStaleGenerationIdentity() async throws {
        let fixture = try RunnerFixture()
        defer { fixture.clear() }
        let statuses = RunnerStatusRecorder()
        let runner = SupervisedJournalRunner(
            clock: NoopRunnerClock(),
            statusSink: { statuses.append($0) },
            gate: RecordingRunnerGate(result: .success),
            containmentEvidenceReader: FixedStartTimeReader(startTime: 1_000),
            processSpawner: RecordingProcessSpawner(pid: 4242),
            pidExists: { _ in false }
        )
        try await runner.start(runtime: fixture.runtime, journalRoot: fixture.realJournalRoot, port: 5015, receiptContext: makeReceiptFixture().context)

        let stale = SupervisedChildIdentity(pid: 4242, kernelStartTime: 1_000, generation: 2)
        #expect(!(await runner.markReady(identity: stale)))
        #expect(statuses.snapshot().isEmpty)

        let current = try #require(await runner.currentIdentity())
        #expect(await runner.markReady(identity: current))
        #expect(statuses.snapshot() == [.running])
    }
}

private struct ReceiptFixture {
    let context: JournalRuntimeEntryReceiptContext
    let sink: InMemoryJournalRuntimeEntryReceiptSink
}

private func makeLaunchReceiptFixture(
    appBundle: Bundle,
    provenanceBundle: Bundle
) -> ReceiptFixture {
    let sink = InMemoryJournalRuntimeEntryReceiptSink()
    let processID = getpid()
    let processUID = getuid()
    let context = JournalRuntimeEntryReceiptLaunch.begin(
        bundle: appBundle,
        provenanceBundle: provenanceBundle,
        sink: sink,
        processEvidenceLookup: { pid in
            guard pid == processID else { return nil }
            return JournalProcessEvidence(
                pid: pid,
                ppid: 1,
                uid: processUID,
                username: "test",
                kernelStartTime: 1_234.567_891
            )
        }
    )
    return ReceiptFixture(context: context, sink: sink)
}

private func makeReceiptFixture() -> ReceiptFixture {
    let sink = InMemoryJournalRuntimeEntryReceiptSink()
    let attemptID = JournalRuntimeEntryAttemptID()
    let identity = JournalRuntimeEntryReceiptAppIdentity(
        appPID: 1,
        bundleIdentifier: "app.solstone.journal",
        bundleShortVersion: "2.0.0",
        bundleVersion: "25",
        locationClass: .standard,
        appKernelStartTimeMicroseconds: 1_000_000
    )
    _ = sink.appendSynchronously(.outerEntry(.init(
        attemptID: attemptID,
        observedAtUnixMilliseconds: 1,
        appIdentity: identity
    )))
    let context = JournalRuntimeEntryReceiptContext(
        attemptID: attemptID,
        sink: sink,
        appIdentity: identity,
        candidateProvenance: JournalRuntimeEntryCandidateProvenance(
            source: "J",
            target: .init(bundleIdentifier: "app.solstone.journal", bundleShortVersion: "2.0.0", bundleVersion: "25"),
            runtimeArchiveSHA256: String(repeating: "a", count: 64),
            manifestSHA256: String(repeating: "b", count: 64),
            releaseReceiptSHA256: String(repeating: "c", count: 64),
            signingReceiptSHA256: String(repeating: "d", count: 64),
            runtimeTreeSHA256: String(repeating: "e", count: 64)
        )
    )
    return ReceiptFixture(context: context, sink: sink)
}

private func payloadEntries(_ records: [JournalRuntimeEntryReceipt]) -> [JournalRuntimeEntryReceiptPayloadEntryDraft] {
    records.compactMap {
        guard case let .payloadEntry(entry) = $0 else { return nil }
        return entry.draft
    }
}

private func payloadExits(_ records: [JournalRuntimeEntryReceipt]) -> [JournalRuntimeEntryReceiptPayloadExitDraft] {
    records.compactMap {
        guard case let .payloadExit(entry) = $0 else { return nil }
        return entry.draft
    }
}

private func assertOuterOnlyClosedChainAfterExit(
    fixture: RunnerFixture,
    receipts: ReceiptFixture
) async throws {
    #expect(receipts.context.candidateProvenance == nil)
    let spawner = RecordingProcessSpawner(pid: 4242)
    let runner = SupervisedJournalRunner(
        statusSink: { _ in },
        gate: RecordingRunnerGate(result: .success),
        containmentEvidenceReader: FixedStartTimeReader(startTime: 1_000),
        processSpawner: spawner,
        pidExists: { _ in false }
    )

    try await runner.start(
        runtime: fixture.runtime,
        journalRoot: fixture.realJournalRoot,
        port: 5015,
        receiptContext: receipts.context
    )
    spawner.exit(spawn: 0, status: 0)
    try await waitForNoActiveChild(runner)
    await Task.yield()
    await runner.stop()

    let records = receipts.sink.storedRecords(attemptID: receipts.context.attemptID)
    let validation = receipts.sink.validate(attemptID: receipts.context.attemptID)
    #expect(records.filter { $0.kind == .outerEntry }.count == 1)
    #expect(payloadEntries(records).isEmpty)
    #expect(payloadExits(records).isEmpty)
    #expect(validation != .invalid(.missingPeer))
    #expect(validation.isValidClosed)
}

private func waitForSpawnCount(_ spawner: RecordingProcessSpawner, count: Int) async throws {
    let deadline = ContinuousClock.now.advanced(by: .seconds(1))
    while ContinuousClock.now < deadline {
        if spawner.spawnRequests().count >= count { return }
        try await Task.sleep(for: .milliseconds(5))
    }
    Issue.record("timed out waiting for child spawn")
}

private func waitForPayloadExitCount(
    _ sink: InMemoryJournalRuntimeEntryReceiptSink,
    attemptID: JournalRuntimeEntryAttemptID,
    count: Int
) async throws {
    let deadline = ContinuousClock.now.advanced(by: .seconds(1))
    while ContinuousClock.now < deadline {
        if payloadExits(sink.storedRecords(attemptID: attemptID)).count >= count { return }
        try await Task.sleep(for: .milliseconds(5))
    }
    Issue.record("timed out waiting for payload exit receipt")
}

private func waitForNoActiveChild(_ runner: SupervisedJournalRunner) async throws {
    let deadline = ContinuousClock.now.advanced(by: .seconds(1))
    while ContinuousClock.now < deadline {
        if await runner.currentIdentity() == nil {
            return
        }
        try await Task.sleep(for: .milliseconds(5))
    }
    Issue.record("timed out waiting for child exit")
}

private func waitForStatus(_ recorder: RunnerStatusRecorder, where match: (JournalRuntimeStatus) -> Bool) async throws {
    let deadline = ContinuousClock.now.advanced(by: .seconds(2))
    while ContinuousClock.now < deadline {
        if recorder.snapshot().contains(where: match) { return }
        try await Task.sleep(for: .milliseconds(5))
    }
    Issue.record("timed out waiting for runtime status")
}

private func waitForStatusCount(_ recorder: RunnerStatusRecorder, count: Int) async throws {
    let deadline = ContinuousClock.now.advanced(by: .seconds(1))
    while ContinuousClock.now < deadline {
        if recorder.snapshot().count >= count { return }
        try await Task.sleep(for: .milliseconds(5))
    }
    Issue.record("timed out waiting for runtime status")
}

private func candidateProvenanceFixtureBundle(named name: String) throws -> Bundle {
    guard let url = Bundle.module.url(
        forResource: name,
        withExtension: "bundle",
        subdirectory: "Fixtures"
    ), let bundle = Bundle(url: url) else {
        throw RunnerTestError.invalidBundle
    }
    return bundle
}

private func validCandidateProvenanceData() throws -> Data {
    let bundle = try candidateProvenanceFixtureBundle(named: "CandidateProvenanceValid")
    guard let url = bundle.url(
        forResource: "runtime-entry-candidate-provenance",
        withExtension: "json",
        subdirectory: "Resources"
    ) else {
        throw RunnerTestError.invalidBundle
    }
    return try Data(contentsOf: url)
}

private func makeTestBundle(
    at root: URL,
    named name: String,
    provenanceData: Data? = nil
) throws -> Bundle {
    let bundleURL = root.appendingPathComponent("\(name).bundle", isDirectory: true)
    let contentsURL = bundleURL.appendingPathComponent("Contents", isDirectory: true)
    try FileManager.default.createDirectory(at: contentsURL, withIntermediateDirectories: true)
    let info = [
        "CFBundleIdentifier": "app.solstone.journal",
        "CFBundleShortVersionString": "2.0.0",
        "CFBundleVersion": "25"
    ]
    let infoData = try PropertyListSerialization.data(
        fromPropertyList: info,
        format: .xml,
        options: 0
    )
    try infoData.write(to: contentsURL.appendingPathComponent("Info.plist"))
    if let provenanceData {
        let resourcesURL = contentsURL.appendingPathComponent("Resources/Resources", isDirectory: true)
        try FileManager.default.createDirectory(at: resourcesURL, withIntermediateDirectories: true)
        try provenanceData.write(to: resourcesURL.appendingPathComponent("runtime-entry-candidate-provenance.json"))
    }
    guard let bundle = Bundle(url: bundleURL) else {
        throw RunnerTestError.invalidBundle
    }
    return bundle
}

private extension JournalRuntimeEntryReceiptChainValidationResult {
    var isValidClosed: Bool {
        if case .validClosed = self { return true }
        return false
    }
}

private enum RunnerTestError: Error {
    case invalidBundle
}

private struct RunnerFixture {
    let root: URL
    let runtime: MaterializedRuntime
    let realJournalRoot: URL
    let linkedJournalRoot: URL

    init() throws {
        root = FileManager.default.temporaryDirectory
            .appendingPathComponent("supervised-runner-\(UUID().uuidString)", isDirectory: true)
        let runtimeRoot = root.appendingPathComponent("runtime", isDirectory: true)
        let layout = SolstoneRuntimeLayout(rootURL: runtimeRoot)
        try layout.ensureCreated()
        runtime = MaterializedRuntime(key: "test-key", layout: layout)
        realJournalRoot = root.appendingPathComponent("real journal", isDirectory: true)
        linkedJournalRoot = root.appendingPathComponent("journal-link", isDirectory: true)
        try FileManager.default.createDirectory(at: realJournalRoot, withIntermediateDirectories: true)
        try FileManager.default.createSymbolicLink(at: linkedJournalRoot, withDestinationURL: realJournalRoot)
    }

    func clear() {
        try? FileManager.default.removeItem(at: root)
    }
}

private final class RecordingRunnerGate: SingleSupervisorGating, @unchecked Sendable {
    private let lock = NSLock()
    private let result: SingleSupervisorGateResult
    private var journalRoots: [String] = []

    init(result: SingleSupervisorGateResult) {
        self.result = result
    }

    func prepareForSpawn(journalRoot: URL) async -> SingleSupervisorGateResult {
        lock.withLock {
            journalRoots.append(canonicalPath(journalRoot))
        }
        return result
    }

    func roots() -> [String] {
        lock.withLock { journalRoots }
    }
}

private actor ScriptedRunnerGate: SingleSupervisorGating {
    private var script: [SingleSupervisorGateResult]
    private let fallback: SingleSupervisorGateResult
    private let blockedCost: Duration
    private let clock: (any MonotonicClock)?

    init(
        _ script: [SingleSupervisorGateResult],
        then fallback: SingleSupervisorGateResult = .success,
        blockedCost: Duration = .zero,
        clock: (any MonotonicClock)? = nil
    ) {
        self.script = script
        self.fallback = fallback
        self.blockedCost = blockedCost
        self.clock = clock
    }

    func prepareForSpawn(journalRoot: URL) async -> SingleSupervisorGateResult {
        let result = script.isEmpty ? fallback : script.removeFirst()
        if case .blocked = result, let clock {
            await clock.sleep(for: blockedCost)
        }
        return result
    }
}

/// Passes the first call, holds the second (an automatic relaunch) until
/// released, and passes every call after it.
private actor HoldingRunnerGate: SingleSupervisorGating {
    private var calls = 0
    private var held: CheckedContinuation<Void, Never>?

    func prepareForSpawn(journalRoot: URL) async -> SingleSupervisorGateResult {
        calls += 1
        if calls == 2 {
            await withCheckedContinuation { held = $0 }
        }
        return .success
    }

    func waitForHeldCall() async throws {
        let deadline = ContinuousClock.now.advanced(by: .seconds(1))
        while held == nil {
            guard ContinuousClock.now < deadline else {
                Issue.record("the relaunch never reached the gate")
                return
            }
            try await Task.sleep(for: .milliseconds(5))
        }
    }

    func releaseHeldCall() {
        held?.resume()
        held = nil
    }
}

private final class RecordingProcessSpawner: SupervisedJournalProcessSpawning, @unchecked Sendable {
    private let lock = NSLock()
    private let pids: [pid_t]
    private let closeParentInputStopsProcess: Bool
    private let exitFirstRunWithStatus: Int32?
    private var requests: [SupervisedJournalSpawnRequest] = []
    private var children: [RecordingChildProcess] = []

    convenience init(pid: pid_t) {
        self.init(pids: [pid])
    }

    init(
        pids: [pid_t],
        closeParentInputStopsProcess: Bool = true,
        exitFirstRunWithStatus: Int32? = nil
    ) {
        self.pids = pids
        self.closeParentInputStopsProcess = closeParentInputStopsProcess
        self.exitFirstRunWithStatus = exitFirstRunWithStatus
    }

    func makeChildProcess(for request: SupervisedJournalSpawnRequest) -> any SupervisedJournalChildProcess {
        lock.withLock {
            let index = children.count
            let child = RecordingChildProcess(
                pid: pids[min(index, pids.count - 1)],
                request: request,
                spawner: self,
                closeParentInputStopsProcess: closeParentInputStopsProcess,
                exitDuringRunStatus: index == 0 ? exitFirstRunWithStatus : nil
            )
            children.append(child)
            return child
        }
    }

    func spawnRequests() -> [SupervisedJournalSpawnRequest] {
        lock.withLock { requests }
    }

    fileprivate func recordSpawn(_ request: SupervisedJournalSpawnRequest) {
        lock.withLock {
            requests.append(request)
        }
    }

    func exit(spawn: Int, status: Int32) {
        let child = lock.withLock {
            children.indices.contains(spawn) ? children[spawn] : nil
        }
        child?.exit(status: status)
    }
}

private final class RecordingChildProcess: SupervisedJournalChildProcess, @unchecked Sendable {
    private let lock = NSLock()
    private let pid: pid_t
    private let request: SupervisedJournalSpawnRequest
    private let spawner: RecordingProcessSpawner
    private let closeParentInputStopsProcess: Bool
    private let exitDuringRunStatus: Int32?
    private var running = false
    private var terminationHandler: (@Sendable (Int32, pid_t) -> Void)?

    init(
        pid: pid_t,
        request: SupervisedJournalSpawnRequest,
        spawner: RecordingProcessSpawner,
        closeParentInputStopsProcess: Bool,
        exitDuringRunStatus: Int32?
    ) {
        self.pid = pid
        self.request = request
        self.spawner = spawner
        self.closeParentInputStopsProcess = closeParentInputStopsProcess
        self.exitDuringRunStatus = exitDuringRunStatus
    }

    var processIdentifier: pid_t {
        pid
    }

    var isRunning: Bool {
        lock.withLock { running }
    }

    func setTerminationHandler(_ handler: @escaping @Sendable (Int32, pid_t) -> Void) {
        lock.withLock {
            terminationHandler = handler
        }
    }

    func run() throws {
        lock.withLock {
            running = true
        }
        spawner.recordSpawn(request)
        if let exitDuringRunStatus {
            exit(status: exitDuringRunStatus)
        }
    }

    func closeParentInput() {
        if closeParentInputStopsProcess {
            lock.withLock {
                running = false
            }
        }
    }

    func exit(status: Int32) {
        let handler = lock.withLock { () -> (@Sendable (Int32, pid_t) -> Void)? in
            running = false
            return terminationHandler
        }
        handler?(status, pid)
    }
}

private final class FixedStartTimeReader: JournalProcessContainmentEvidenceReading, @unchecked Sendable {
    private let lock = NSLock()
    private var startTimes: [Double?]

    convenience init(startTime: Double) {
        self.init(startTimes: [startTime])
    }

    init(startTimes: [Double?]) {
        self.startTimes = startTimes
    }

    func containmentEvidence(for pid: pid_t) -> JournalContainmentMemberEvidence? {
        let startTime = lock.withLock {
            startTimes.isEmpty ? nil : startTimes.removeFirst()
        }
        return JournalContainmentMemberEvidence(
            pid: pid,
            processGroupID: pid,
            uid: getuid(),
            username: currentUsername(),
            kernelStartTime: startTime
        )
    }

    func processIDs(inProcessGroup processGroupID: pid_t) -> [pid_t]? {
        []
    }
}

private final class NoopRunnerClock: MonotonicClock, @unchecked Sendable {
    func now() -> Duration { .zero }
    func sleep(for duration: Duration) async {}
}

private final class AdvancingRunnerClock: MonotonicClock, @unchecked Sendable {
    private let lock = NSLock()
    private var current: Duration = .zero

    func now() -> Duration {
        lock.withLock { current }
    }

    func sleep(for duration: Duration) async {
        lock.withLock { current += duration }
    }
}

private final class ScriptedContainmentEvidenceReader: JournalProcessContainmentEvidenceReading, @unchecked Sendable {
    private let lock = NSLock()
    private var memberships: [[pid_t]?]
    private let evidenceByPID: [pid_t: JournalContainmentMemberEvidence]

    init(memberships: [[pid_t]?], evidenceByPID: [pid_t: JournalContainmentMemberEvidence]) {
        self.memberships = memberships
        self.evidenceByPID = evidenceByPID
    }

    func containmentEvidence(for pid: pid_t) -> JournalContainmentMemberEvidence? {
        evidenceByPID[pid]
    }

    func processIDs(inProcessGroup processGroupID: pid_t) -> [pid_t]? {
        lock.withLock {
            guard !memberships.isEmpty else { return [] }
            return memberships.removeFirst()
        }
    }
}

private final class RunnerSignalRecorder: @unchecked Sendable {
    struct Signal: Equatable, Sendable {
        let pid: pid_t
        let signal: Int32
    }

    private let lock = NSLock()
    private var values: [Signal] = []

    func append(pid: pid_t, signal: Int32) {
        lock.withLock { values.append(.init(pid: pid, signal: signal)) }
    }

    func snapshot() -> [Signal] {
        lock.withLock { values }
    }
}

private final class RunnerStatusRecorder: @unchecked Sendable {
    private let lock = NSLock()
    private var statuses: [JournalRuntimeStatus] = []

    func append(_ status: JournalRuntimeStatus) {
        lock.withLock { statuses.append(status) }
    }

    func snapshot() -> [JournalRuntimeStatus] {
        lock.withLock { statuses }
    }
}

private actor DeferredRequiredModels: JournalRequiredModelsReconciling {
    private(set) var entered = false
    private var continuation: CheckedContinuation<Void, Never>?

    func reconcile(runtime: MaterializedRuntime, journalRoot: URL) async throws {
        await withCheckedContinuation { continuation in
            self.continuation = continuation
            entered = true
        }
    }

    func finish() {
        continuation?.resume()
        continuation = nil
    }
}

private actor GateClosingAfterPreparation: SingleSupervisorGating {
    private var calls = 0

    func prepareForSpawn(journalRoot: URL) async -> SingleSupervisorGateResult {
        calls += 1
        if calls == 1 { return .success }
        return .blocked(.portConflict(JournalDiagnostic(commandLabel: "gate", outputExcerpt: "new owner")))
    }
}
