// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import Darwin
import Foundation
import JournalRuntimeTestSupport
import SolstoneCore
import Testing
@testable import JournalRuntime

@Suite("SingleSupervisorGate")
struct SingleSupervisorGateTests {
    @Test func configuredDirectDoorPortDoesNotProbeLegacyDefault() async throws {
        let fixture = try JournalRootFixture()
        defer { fixture.clear() }
        try fixture.writeConfig(#"{"pairing":{"direct_port":9000}}"#)
        let runner = FakeSubprocessRunner()
        runner.enqueueLsof(port: 7657, .success(exitCode: 0))
        runner.enqueueLsof(port: 9000, .success(exitCode: 1))
        runner.enqueueLsof(port: 5015, .success(exitCode: 1))

        let result = await makeGate(runner: runner, clock: GateClock()).prepareForSpawn(journalRoot: fixture.rootURL)

        #expect(result == .success)
        #expect(runner.lsofPorts() == [9000, 5015])
        #expect(!runner.lsofPorts().contains(7657))
        #expect(runner.invocations.allSatisfy { $0.timeout == .seconds(10) })
    }

    @Test func missingConfigProbesNormalDefaultDirectDoorThenConvey() async throws {
        let fixture = try JournalRootFixture()
        defer { fixture.clear() }
        let runner = FakeSubprocessRunner()
        runner.enqueueLsof(port: 7657, .success(exitCode: 1))
        runner.enqueueLsof(port: 5015, .success(exitCode: 1))

        let result = await makeGate(runner: runner, clock: GateClock()).prepareForSpawn(journalRoot: fixture.rootURL)

        #expect(result == .success)
        #expect(runner.lsofPorts() == [7657, 5015])
    }

    @Test func corruptConfigFallsBackToNormalDefaultDirectDoorThenConvey() async throws {
        let fixture = try JournalRootFixture()
        defer { fixture.clear() }
        try fixture.writeConfig("{")
        let runner = FakeSubprocessRunner()
        runner.enqueueLsof(port: 7657, .success(exitCode: 1))
        runner.enqueueLsof(port: 5015, .success(exitCode: 1))

        let result = await makeGate(runner: runner, clock: GateClock()).prepareForSpawn(journalRoot: fixture.rootURL)

        #expect(result == .success)
        #expect(runner.lsofPorts() == [7657, 5015])
    }

    @Test func busyConfiguredDirectDoorPortRetriesBeforeBlocking() async throws {
        let fixture = try JournalRootFixture()
        defer { fixture.clear() }
        try fixture.writeConfig(#"{"pairing":{"direct_port":9000}}"#)
        let runner = FakeSubprocessRunner()
        for _ in 0..<3 {
            runner.enqueueLsof(port: 9000, .success(exitCode: 0))
        }
        let clock = GateClock()

        let result = await makeGate(runner: runner, clock: clock).prepareForSpawn(journalRoot: fixture.rootURL)

        guard case .blocked(.portConflict(let diagnostic)) = result else {
            Issue.record("expected configured direct-door port conflict, got \(result)")
            return
        }
        #expect(diagnostic.outputExcerpt?.contains("port 9000") == true)
        #expect(runner.lsofPorts() == [9000, 9000, 9000])
        #expect(clock.sleptDurations == [.seconds(2), .seconds(3)])
    }

    @Test func busyConveyPortBlocksAfterConfiguredDirectDoorPortIsFree() async throws {
        let fixture = try JournalRootFixture()
        defer { fixture.clear() }
        try fixture.writeConfig(#"{"pairing":{"direct_port":9000}}"#)
        let runner = FakeSubprocessRunner()
        runner.enqueueLsof(port: 9000, .success(exitCode: 1))
        for _ in 0..<3 {
            runner.enqueueLsof(port: 5015, .success(exitCode: 0))
        }
        let clock = GateClock()

        let result = await makeGate(runner: runner, clock: clock).prepareForSpawn(journalRoot: fixture.rootURL)

        guard case .blocked(.portConflict(let diagnostic)) = result else {
            Issue.record("expected Convey port conflict, got \(result)")
            return
        }
        #expect(diagnostic.outputExcerpt?.contains("port 5015") == true)
        #expect(runner.lsofPorts() == [9000, 5015, 5015, 5015])
        #expect(clock.sleptDurations == [.seconds(2), .seconds(3)])
    }

    @Test func hangingStartupPortProbeFailsClosedBeforeSetupCanStart() async throws {
        let fixture = try JournalRootFixture()
        defer { fixture.clear() }
        let runner = FakeSubprocessRunner()
        runner.enqueueLsof(port: 7657, .success(exitCode: 1, delay: .milliseconds(2)))

        let result = await makeGate(
            runner: runner,
            clock: GateClock(),
            portProbeTimeout: .milliseconds(1)
        ).prepareForSpawn(journalRoot: fixture.rootURL)

        guard case .blocked(.portVerificationFailed(let diagnostic)) = result else {
            Issue.record("expected unverifiable startup ports to fail closed, got \(result)")
            return
        }
        #expect(diagnostic.outputExcerpt == "lsof did not complete probing port 7657")
        #expect(runner.invocations.last?.timeout == .milliseconds(1))
        #expect(runner.invocations.allSatisfy { $0.arguments.first != "setup" })
    }

    @Test func portHeldElsewhereIsCheckedOnlyAfterTheServiceAndPortsClear() async throws {
        let fixture = try JournalRootFixture()
        defer { fixture.clear() }
        let runner = FakeSubprocessRunner()
        runner.enqueueLsof(port: 7657, .success(exitCode: 1))
        runner.enqueueLsof(port: 5015, .success(exitCode: 1))
        let probes = BindProbeCounter()
        let gate = SingleSupervisorGate(
            runner: runner,
            serviceRetirer: NoLegacyServiceRetirer(),
            evidenceReader: NoJournalProcessEvidenceReader(),
            pidExists: { _ in false },
            terminate: { _, _ in 0 },
            clock: GateClock(),
            orphanGracePeriod: .zero,
            portBindable: { probes.probe(); return false }
        )

        let result = await gate.prepareForSpawn(journalRoot: fixture.rootURL)

        guard case .blocked(let blockage) = result, case .portHeldElsewhere = blockage else {
            Issue.record("expected port-held-elsewhere, got \(result)")
            return
        }
        #expect(blockage.ownerMessage == UICopy.JOURNAL_SPAWN_PORT_HELD_ELSEWHERE)
        #expect(probes.count == 1)
        #expect(runner.lsofPorts() == [7657, 5015])
    }

    @Test func aServiceForThisJournalIsRetiredBeforeThePortIsTested() async throws {
        let fixture = try JournalRootFixture()
        defer { fixture.clear() }
        let probes = BindProbeCounter()
        let gate = SingleSupervisorGate(
            runner: FakeSubprocessRunner(),
            serviceRetirer: BlockingLegacyServiceRetirer(),
            evidenceReader: NoJournalProcessEvidenceReader(),
            pidExists: { _ in false },
            terminate: { _, _ in 0 },
            clock: GateClock(),
            orphanGracePeriod: .zero,
            portBindable: { probes.probe(); return false }
        )

        let result = await gate.prepareForSpawn(journalRoot: fixture.rootURL)

        guard case .blocked(.legacyServiceVerificationFailed) = result else {
            Issue.record("expected the service check to answer first, got \(result)")
            return
        }
        #expect(probes.count == 0)
    }

    private func makeGate(
        runner: FakeSubprocessRunner,
        clock: GateClock,
        portProbeTimeout: Duration = journalPortProbeTimeout
    ) -> SingleSupervisorGate {
        SingleSupervisorGate(
            runner: runner,
            serviceRetirer: NoLegacyServiceRetirer(),
            evidenceReader: NoJournalProcessEvidenceReader(),
            pidExists: { _ in false },
            terminate: { _, _ in 0 },
            clock: clock,
            orphanGracePeriod: .zero,
            portProbeTimeout: portProbeTimeout
        )
    }

}

private final class BindProbeCounter: @unchecked Sendable {
    private let lock = NSLock()
    private var probes = 0
    var count: Int { lock.withLock { probes } }
    func probe() { lock.withLock { probes += 1 } }
}

private struct BlockingLegacyServiceRetirer: LegacyJournalServiceRetiring {
    func retireLegacyService(journalRoot: URL) async -> LegacyJournalServiceRetirementResult {
        .blocked(.inspectionFailure, JournalDiagnostic(commandLabel: "legacy journal service", outputExcerpt: "legacy service could not be inspected"))
    }

    func currentLegacyServicePID() async -> LegacyJournalServicePIDLookup {
        .known(nil)
    }
}

private struct NoLegacyServiceRetirer: LegacyJournalServiceRetiring {
    func retireLegacyService(journalRoot: URL) async -> LegacyJournalServiceRetirementResult {
        .success(.noService)
    }

    func currentLegacyServicePID() async -> LegacyJournalServicePIDLookup {
        .known(nil)
    }
}

private struct NoJournalProcessEvidenceReader: JournalProcessEvidenceReading {
    func evidence(for pid: pid_t) async -> JournalProcessEvidence? {
        nil
    }
}

private final class GateClock: MonotonicClock, @unchecked Sendable {
    private let lock = NSLock()
    private var value: Duration = .zero
    private var durations: [Duration] = []

    var sleptDurations: [Duration] {
        lock.withLock { durations }
    }

    func now() -> Duration {
        lock.withLock { value }
    }

    func sleep(for duration: Duration) async {
        lock.withLock {
            value += duration
            durations.append(duration)
        }
    }
}
