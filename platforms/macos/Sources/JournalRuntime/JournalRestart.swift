// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import Darwin
import Foundation
import os
import SolstoneCore

public enum JournalRestartStep: CaseIterable, Sendable {
    case resolveJournal
    case orphanSweep
    case staleStateMoveAside
    case serviceRestart
    case reProbe

    public var rawValue: String {
        switch self {
        case .resolveJournal:
            return "resolve-journal"
        case .orphanSweep:
            return "orphan-sweep"
        case .staleStateMoveAside:
            return "stale-state-move-aside"
        case .serviceRestart:
            return "journal-restart"
        case .reProbe:
            return "re-probe"
        }
    }
}

public struct JournalRestartFailure: Error, Equatable, Sendable {
    public let step: JournalRestartStep
    public let ownerMessage: String
    public let diagnostic: JournalDiagnostic

    public init(step: JournalRestartStep, ownerMessage: String, diagnostic: JournalDiagnostic) {
        self.step = step
        self.ownerMessage = ownerMessage
        self.diagnostic = diagnostic
    }
}

public enum JournalRestartOutcome: Equatable, Sendable {
    case success
    case failure(JournalRestartFailure)
}

public struct JournalRestartLogEvent: Equatable, Sendable {
    public let step: JournalRestartStep
    public let outcome: String
    public let detail: String?

    public init(step: JournalRestartStep, outcome: String, detail: String?) {
        self.step = step
        self.outcome = outcome
        self.detail = detail
    }
}

public let staleStateRelativePaths: [String] = [
    "health/supervisor.ready",
    "health/supervisor.pid",
    "health/supervisor.start_time",
    "health/supervisor.lock",
    "health/callosum.sock",
    "health/convey.port"
]

public func parseJournalPath(from output: String) -> String? {
    for line in output.split(separator: "\n", omittingEmptySubsequences: false) {
        let trimmed = line.trimmingCharacters(in: .whitespacesAndNewlines)
        guard trimmed.hasPrefix("path: ") else { continue }
        let path = String(trimmed.dropFirst("path: ".count)).trimmingCharacters(in: .whitespacesAndNewlines)
        guard path.hasPrefix("/") else { return nil }
        return path.isEmpty ? nil : path
    }
    return nil
}

public func moveAsideStaleStateFiles(
    journalRoot: URL,
    fileManager: FileManager = .default
) -> [JournalRestartLogEvent] {
    staleStateRelativePaths.map { relativePath in
        let fileURL = journalRoot.appendingPathComponent(relativePath)
        let backupURL = URL(fileURLWithPath: fileURL.path + ".bak")

        guard fileManager.fileExists(atPath: fileURL.path) else {
            return JournalRestartLogEvent(
                step: .staleStateMoveAside,
                outcome: "noop",
                detail: "\(relativePath):missing"
            )
        }

        do {
            if fileManager.fileExists(atPath: backupURL.path) {
                try fileManager.removeItem(at: backupURL)
            }
            try fileManager.moveItem(at: fileURL, to: backupURL)
            return JournalRestartLogEvent(
                step: .staleStateMoveAside,
                outcome: "success",
                detail: "\(relativePath):moved"
            )
        } catch {
            return JournalRestartLogEvent(
                step: .staleStateMoveAside,
                outcome: "error",
                detail: "\(relativePath):move-failed"
            )
        }
    }
}

public struct JournalRestartRunner: @unchecked Sendable {
    private let runner: SubprocessRunning
    private let journalPathProvider: @Sendable (URL) async -> String?
    private let terminate: @Sendable (pid_t, Int32) -> Int32
    private let pidExists: @Sendable (pid_t) -> Bool
    private let evidenceReader: any JournalProcessEvidenceReading
    private let fileManager: FileManager
    private let reprobe: @Sendable () async -> JournalRuntimeProbeOutcome
    private let logSink: (@Sendable (JournalRestartLogEvent) -> Void)?
    private let journalBinary: URL

    public init(
        runner: SubprocessRunning = SubprocessRunner(),
        journalPathProvider: (@Sendable (URL) async -> String?)? = nil,
        terminate: @escaping @Sendable (pid_t, Int32) -> Int32 = { pid, signal in
            Darwin.kill(pid, signal)
        },
        pidExists: @escaping @Sendable (pid_t) -> Bool = { pid in
            if Darwin.kill(pid, 0) == 0 {
                return true
            }
            return errno == EPERM
        },
        evidenceReader: any JournalProcessEvidenceReading = LiveJournalProcessEvidenceReader(),
        fileManager: FileManager = .default,
        reprobe: @escaping @Sendable () async -> JournalRuntimeProbeOutcome,
        logSink: (@Sendable (JournalRestartLogEvent) -> Void)? = nil,
        journalBinary: URL
    ) {
        self.runner = runner
        self.journalPathProvider = journalPathProvider ?? { journalBinary in
            await Self.defaultJournalPathProvider(journalBinary: journalBinary, runner: runner)
        }
        self.terminate = terminate
        self.pidExists = pidExists
        self.evidenceReader = evidenceReader
        self.fileManager = fileManager
        self.reprobe = reprobe
        self.logSink = logSink
        self.journalBinary = journalBinary
    }

    public func run() async -> JournalRestartOutcome {
        guard let journalPath = await journalPathProvider(journalBinary) else {
            let diagnostic = JournalDiagnostic(
                commandLabel: "solstone journal config show",
                outputExcerpt: "no journal path"
            )
            let failure = JournalRestartFailure(
                step: .resolveJournal,
                ownerMessage: JournalCopy.restartJournalPathUnreadable(),
                diagnostic: diagnostic
            )
            emit(step: .resolveJournal, outcome: "error", detail: "no-journal-path")
            return .failure(failure)
        }
        emit(step: .resolveJournal, outcome: "success", detail: journalPath)

        let journalRoot = URL(fileURLWithPath: journalPath, isDirectory: true)
        // The rooted orphan sweep must read supervisor.pid/start_time before
        // moveAsideStaleStateFiles renames them. If a prior restart already
        // moved them aside, provenance is unavailable; that intentionally
        // protects the process and lets the later port check block clearly.
        await runOrphanSweep(journalRoot: journalRoot)

        let moveEvents = moveAsideStaleStateFiles(
            journalRoot: journalRoot,
            fileManager: fileManager
        )
        emitMoveAsideSummary(moveEvents)

        let output = LockedJournalOutput()
        let restartResult: SubprocessResult
        do {
            restartResult = try await runner.run(
                executable: journalBinary,
                arguments: ["journal", "service", "restart"],
                environment: nil,
                stdoutHandler: { data in output.append(data) },
                stderrHandler: { data in output.append(data) }
            )
        } catch {
            let failure = JournalRestartFailure(
                step: .serviceRestart,
                ownerMessage: JournalCopy.restartRestartFailedJournalDidNotRestart(),
                diagnostic: JournalDiagnostic(
                    commandLabel: "solstone journal service restart",
                    outputExcerpt: sanitizeJournalDiagnosticOutput(error.localizedDescription)
                )
            )
            emit(step: .serviceRestart, outcome: "error", detail: "launch-failed")
            return .failure(failure)
        }
        guard restartResult.exitCode == 0 else {
            let failure = JournalRestartFailure(
                step: .serviceRestart,
                ownerMessage: JournalCopy.restartRestartFailedJournalDidNotRestart(),
                diagnostic: JournalDiagnostic(
                    commandLabel: "solstone journal service restart",
                    exitCode: restartResult.exitCode,
                    outputExcerpt: sanitizeJournalDiagnosticOutput(output.string)
                )
            )
            emit(step: .serviceRestart, outcome: "error", detail: "exit=\(restartResult.exitCode)")
            return .failure(failure)
        }
        emit(step: .serviceRestart, outcome: "success", detail: nil)

        do {
            let reprobe = self.reprobe
            let outcome = try await withTimeout(seconds: 10.0) {
                await reprobe()
            }
            guard outcome == .reachable else {
                let diagnostic = Self.postRestartDiagnostic(outcome: outcome)
                let failure = JournalRestartFailure(
                    step: .reProbe,
                    ownerMessage: JournalCopy.restartRestartFailedJournalDidNotComeBack(),
                    diagnostic: diagnostic
                )
                emit(step: .reProbe, outcome: "error", detail: probeOutcomeKind(outcome))
                return .failure(failure)
            }
            emit(step: .reProbe, outcome: "success", detail: nil)
            return .success
        } catch {
            let failure = JournalRestartFailure(
                step: .reProbe,
                ownerMessage: JournalCopy.restartRestartFailedJournalDidNotComeBack(),
                diagnostic: JournalDiagnostic(
                    commandLabel: "solstone journal health",
                    timedOut: error is TimeoutError,
                    outputExcerpt: sanitizeJournalDiagnosticOutput(error.localizedDescription)
                )
            )
            emit(step: .reProbe, outcome: "error", detail: error is TimeoutError ? "timed-out" : "launch-failed")
            return .failure(failure)
        }
    }

    private static func postRestartDiagnostic(outcome: JournalRuntimeProbeOutcome) -> JournalDiagnostic {
        switch outcome {
        case .unreachable(let diagnostic), .unknown(let diagnostic):
            return JournalDiagnostic(
                commandLabel: diagnostic.commandLabel,
                timedOut: diagnostic.timedOut,
                exitCode: diagnostic.exitCode,
                outputExcerpt: diagnostic.outputExcerpt ?? "post-restart check failed"
            )
        case .binaryMissing:
            return JournalDiagnostic(commandLabel: "solstone journal health", outputExcerpt: "journal binary missing after restart")
        case .reachable:
            return JournalDiagnostic(commandLabel: "solstone journal health")
        }
    }

    private static func defaultJournalPathProvider(
        journalBinary: URL,
        runner: SubprocessRunning
    ) async -> String? {
        let output = LockedJournalOutput()
        do {
            let result = try await runner.run(
                executable: journalBinary,
                arguments: ["journal", "config", "show"],
                environment: nil,
                stdoutHandler: { data in output.append(data) },
                stderrHandler: { _ in }
            )
            guard result.exitCode == 0 else { return nil }
            return parseJournalPath(from: output.string)
        } catch {
            return nil
        }
    }

    private func runOrphanSweep(journalRoot: URL) async {
        let failure = await runJournalOrphanSweep(
            journalRoot: journalRoot,
            runner: runner,
            evidenceReader: evidenceReader,
            pidExists: pidExists,
            terminate: terminate,
            gracePeriod: .zero,
            clock: SystemMonotonicClock()
        )
        if let failure {
            emit(step: .orphanSweep, outcome: "error", detail: failure.message)
        } else {
            emit(step: .orphanSweep, outcome: "success", detail: nil)
        }
    }

    private func emitMoveAsideSummary(_ events: [JournalRestartLogEvent]) {
        let moved = events.filter { $0.outcome == "success" }.count
        let errors = events.filter { $0.outcome == "error" }.count
        if errors > 0 {
            emit(step: .staleStateMoveAside, outcome: "error", detail: "moved=\(moved) errors=\(errors)")
        } else if moved == 0 {
            emit(step: .staleStateMoveAside, outcome: "noop", detail: "moved=0")
        } else {
            emit(step: .staleStateMoveAside, outcome: "success", detail: "moved=\(moved)")
        }
    }

    private func probeOutcomeKind(_ outcome: JournalRuntimeProbeOutcome) -> String {
        switch outcome {
        case .reachable: "reachable"
        case .unreachable: "unreachable"
        case .unknown: "unknown"
        case .binaryMissing: "binary-missing"
        }
    }

    private func emit(step: JournalRestartStep, outcome: String, detail: String?) {
        let event = JournalRestartLogEvent(step: step, outcome: outcome, detail: detail)
        let detailSuffix = detail.map { " detail=\($0)" } ?? ""
        if outcome == "error" {
            Logger.setup.warning("journal-restart step=\(step.rawValue, privacy: .public) outcome=\(outcome, privacy: .public)\(detailSuffix, privacy: .public)")
        } else if step == .orphanSweep {
            Logger.setup.notice("journal-restart step=\(step.rawValue, privacy: .public) outcome=\(outcome, privacy: .public)\(detailSuffix, privacy: .public)")
        } else {
            Logger.setup.info("journal-restart step=\(step.rawValue, privacy: .public) outcome=\(outcome, privacy: .public)\(detailSuffix, privacy: .public)")
        }
        logSink?(event)
    }
}

private final class LockedJournalOutput: @unchecked Sendable {
    private let lock = NSLock()
    private var data = Data()

    var string: String {
        lock.withLock { String(data: data, encoding: .utf8) ?? "" }
    }

    func append(_ chunk: Data) {
        lock.withLock {
            data.append(chunk)
        }
    }
}
