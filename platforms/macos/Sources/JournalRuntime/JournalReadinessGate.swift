// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import Foundation
import SolstoneCore

public protocol JournalReadinessChecking: Sendable {
    func waitUntilReady(
        journalRoot: URL,
        runtime: MaterializedRuntime,
        timeout: Duration,
        terminalCheck: @escaping @Sendable () async -> JournalDiagnostic?,
        identityProvider: @escaping @Sendable () async -> SupervisedChildIdentity?,
        readinessAcceptance: @escaping @Sendable (SupervisedChildIdentity) async -> Bool
    ) async -> JournalReadinessResult
}

public enum JournalReadinessResult: Equatable, Sendable {
    case ready
    case failed(JournalDiagnostic)
    case failedTerminal(JournalDiagnostic)
}

public struct JournalReadinessGate: JournalReadinessChecking {
    private let clock: any MonotonicClock
    private let pollInterval: Duration
    private let startTimeToleranceSeconds: Double

    public init(
        clock: any MonotonicClock = SystemMonotonicClock(),
        pollInterval: Duration = .milliseconds(250),
        startTimeToleranceSeconds: Double = journalSupervisorStartTimeToleranceSeconds
    ) {
        self.clock = clock
        self.pollInterval = pollInterval
        self.startTimeToleranceSeconds = startTimeToleranceSeconds
    }

    public func waitUntilReady(
        journalRoot: URL,
        runtime: MaterializedRuntime,
        timeout: Duration,
        terminalCheck: @escaping @Sendable () async -> JournalDiagnostic?,
        identityProvider: @escaping @Sendable () async -> SupervisedChildIdentity?,
        readinessAcceptance: @escaping @Sendable (SupervisedChildIdentity) async -> Bool
    ) async -> JournalReadinessResult {
        let deadline = clock.now() + timeout

        while clock.now() < deadline {
            if let terminalDiagnostic = await terminalCheck() {
                return .failedTerminal(terminalDiagnostic)
            }
            if let identity = await markerIsReady(journalRoot: journalRoot, identityProvider: identityProvider),
               await readinessAcceptance(identity) {
                return .ready
            }
            await clock.sleep(for: pollInterval)
        }

        if let terminalDiagnostic = await terminalCheck() {
            return .failedTerminal(terminalDiagnostic)
        }

        return .failed(JournalDiagnostic(
            commandLabel: "journal readiness",
            timedOut: true,
            outputExcerpt: UICopy.JOURNAL_READINESS_TIMEOUT
        ))
    }

    private func markerIsReady(
        journalRoot: URL,
        identityProvider: @escaping @Sendable () async -> SupervisedChildIdentity?
    ) async -> SupervisedChildIdentity? {
        guard let marker = readSupervisorReadyMarker(
            from: journalRoot.appendingPathComponent("health/supervisor.ready")
        ),
            let recordedPID = readSupervisorPositivePID(from: journalRoot.appendingPathComponent("health/supervisor.pid")),
            let recordedStartTime = readSupervisorDouble(from: journalRoot.appendingPathComponent("health/supervisor.start_time")),
            let identity = await identityProvider() else {
            return nil
        }

        guard marker.pid == recordedPID,
              recordedPID == identity.pid else {
            return nil
        }
        guard abs(recordedStartTime - identity.kernelStartTime) <= startTimeToleranceSeconds else {
            return nil
        }
        // Python's _valid_marker() parses payload start_time but does not bind
        // it. signal_ready() writes this value from supervisor.start_time, so a
        // fresh marker matches; this stricter check only rejects stale markers
        // for a recycled PID.
        guard abs(marker.startTime - identity.kernelStartTime) <= startTimeToleranceSeconds else {
            return nil
        }
        return identity
    }

}

private struct SupervisorReadyMarker: Decodable {
    let pid: pid_t
    let readyAt: Double
    let startTime: Double

    private enum CodingKeys: String, CodingKey {
        case pid
        case readyAt = "ready_at"
        case startTime = "start_time"
    }
}

private func readSupervisorReadyMarker(from url: URL) -> SupervisorReadyMarker? {
    guard let data = try? Data(contentsOf: url),
          let marker = try? JSONDecoder().decode(SupervisorReadyMarker.self, from: data),
          marker.pid > 0,
          marker.readyAt.isFinite,
          marker.startTime.isFinite else {
        return nil
    }
    return marker
}

private func readSupervisorPositivePID(from url: URL) -> pid_t? {
    guard let text = try? String(contentsOf: url, encoding: .utf8),
          let value = Int32(text.trimmingCharacters(in: .whitespacesAndNewlines)),
          value > 0 else {
        return nil
    }
    return pid_t(value)
}

private func readSupervisorDouble(from url: URL) -> Double? {
    guard let text = try? String(contentsOf: url, encoding: .utf8),
          let value = Double(text.trimmingCharacters(in: .whitespacesAndNewlines)),
          value.isFinite else {
        return nil
    }
    return value
}

public func liveSupervisorMatchesReadyFiles(journalRoot: URL) -> Bool {
    liveSupervisorMatchesReadyFiles(journalRoot: journalRoot) { pid in
        liveJournalProcessEvidence(for: pid)?.kernelStartTime
    }
}

public func liveSupervisorMatchesReadyFiles(
    journalRoot: URL,
    kernelStartTime: (pid_t) -> Double?
) -> Bool {
    liveReadySupervisorPID(journalRoot: journalRoot, kernelStartTime: kernelStartTime) != nil
}

/// The pid of the supervisor the journal's ready files name, when that process
/// is alive and is the one that wrote them.
public func liveReadySupervisorPID(journalRoot: URL) -> pid_t? {
    liveReadySupervisorPID(journalRoot: journalRoot) { pid in
        liveJournalProcessEvidence(for: pid)?.kernelStartTime
    }
}

func liveReadySupervisorPID(
    journalRoot: URL,
    kernelStartTime: (pid_t) -> Double?
) -> pid_t? {
    guard let marker = readSupervisorReadyMarker(
        from: journalRoot.appendingPathComponent("health/supervisor.ready")
    ),
        let recordedPID = readSupervisorPositivePID(
            from: journalRoot.appendingPathComponent("health/supervisor.pid")
        ),
        let recordedStartTime = readSupervisorDouble(
            from: journalRoot.appendingPathComponent("health/supervisor.start_time")
        ),
        marker.pid == recordedPID,
        let kernel = kernelStartTime(recordedPID),
        kernel.isFinite,
        abs(recordedStartTime - kernel) <= journalSupervisorStartTimeToleranceSeconds,
        abs(marker.startTime - kernel) <= journalSupervisorStartTimeToleranceSeconds else {
        return nil
    }
    return recordedPID
}
