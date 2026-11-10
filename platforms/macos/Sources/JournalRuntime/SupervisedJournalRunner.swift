// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import Darwin
@preconcurrency import Foundation
import os
import SolstoneCore

public struct SupervisedChildIdentity: Equatable, Sendable {
    public let pid: pid_t
    public let kernelStartTime: Double
    public let generation: UInt64

    public init(pid: pid_t, kernelStartTime: Double, generation: UInt64) {
        self.pid = pid
        self.kernelStartTime = kernelStartTime
        self.generation = generation
    }
}

private struct ParentLossLedgerProcessIdentity: Decodable {
    let pid: pid_t
    let birth: ParentLossLedgerProcessBirth
}

private struct ParentLossLedgerProcessBirth: Decodable {
    let epochMicros: Int64

    private enum CodingKeys: String, CodingKey {
        case epochMicros = "epoch_micros"
    }
}

private struct ParentLossLedgerActiveGeneration: Decodable {
    let schema: Int
    let coordinator: ParentLossLedgerProcessIdentity
    let supervisor: ParentLossLedgerProcessIdentity
}

public protocol SupervisedChildRunning: Sendable {
    func start(
        runtime: MaterializedRuntime,
        journalRoot: URL,
        port: Int,
        receiptContext: JournalRuntimeEntryReceiptContext
    ) async throws
    func restart() async throws
    func stop() async
    func stopForTermination() async
    func currentRuntimeKey() async -> String?
    func currentIdentity() async -> SupervisedChildIdentity?
    func terminalReason() async -> JournalDiagnostic?
    func markReady(identity: SupervisedChildIdentity) async -> Bool
}

public enum SupervisedJournalRunnerError: LocalizedError, Sendable, Equatable {
    case alreadyStopped
    case gateBlocked(SingleSupervisorGateBlockage)
    case launchFailed(String)

    public var errorDescription: String? {
        switch self {
        case .alreadyStopped:
            return "journal child is not configured"
        case .gateBlocked(let blockage):
            return blockage.ownerMessage
        case .launchFailed(let message):
            return message
        }
    }
}

internal struct SupervisedJournalSpawnRequest: Sendable {
    let executableURL: URL
    let currentDirectoryURL: URL
    let arguments: [String]
    let environment: [String: String]
}

internal protocol SupervisedJournalProcessSpawning: Sendable {
    func makeChildProcess(for request: SupervisedJournalSpawnRequest) -> any SupervisedJournalChildProcess
}

internal protocol SupervisedJournalChildProcess: AnyObject, Sendable {
    var processIdentifier: pid_t { get }
    var isRunning: Bool { get }

    func setTerminationHandler(_ handler: @escaping @Sendable (Int32, pid_t) -> Void)
    func run() throws
    func closeParentInput()
}

private struct FoundationSupervisedJournalProcessSpawner: SupervisedJournalProcessSpawning {
    func makeChildProcess(for request: SupervisedJournalSpawnRequest) -> any SupervisedJournalChildProcess {
        FoundationSupervisedJournalChildProcess(request: request)
    }
}

private final class FoundationSupervisedJournalChildProcess: SupervisedJournalChildProcess, @unchecked Sendable {
    private let process = Process()
    private let parentWriteHandle: FileHandle
    private let stdoutReadHandle: FileHandle
    private let stderrReadHandle: FileHandle
    private let outputHandler = SupervisedJournalChildOutputHandler()
    private let lock = NSLock()
    private var parentInputClosed = false
    private var finished = false

    init(request: SupervisedJournalSpawnRequest) {
        process.executableURL = request.executableURL
        process.currentDirectoryURL = request.currentDirectoryURL
        process.arguments = request.arguments
        process.environment = request.environment

        let parentPipe = Pipe()
        process.standardInput = parentPipe.fileHandleForReading
        parentWriteHandle = parentPipe.fileHandleForWriting

        let stdoutPipe = Pipe()
        stdoutReadHandle = stdoutPipe.fileHandleForReading
        process.standardOutput = stdoutPipe
        let outputHandler = self.outputHandler
        stdoutReadHandle.readabilityHandler = { handle in
            consumeSupervisedJournalChildOutput(from: handle, stream: .stdout, handler: outputHandler)
        }

        let stderrPipe = Pipe()
        stderrReadHandle = stderrPipe.fileHandleForReading
        process.standardError = stderrPipe
        stderrReadHandle.readabilityHandler = { handle in
            consumeSupervisedJournalChildOutput(from: handle, stream: .stderr, handler: outputHandler)
        }
    }

    var processIdentifier: pid_t {
        process.processIdentifier
    }

    var isRunning: Bool {
        process.isRunning
    }

    func setTerminationHandler(_ handler: @escaping @Sendable (Int32, pid_t) -> Void) {
        process.terminationHandler = { [weak self] child in
            self?.finish(exitStatus: child.terminationStatus)
            handler(child.terminationStatus, child.processIdentifier)
        }
    }

    func run() throws {
        try process.run()
    }

    func closeParentInput() {
        lock.withLock {
            guard !parentInputClosed else { return }
            parentInputClosed = true
            parentWriteHandle.readabilityHandler = nil
            try? parentWriteHandle.close()
        }
    }

    deinit {
        finish(exitStatus: nil)
        closeParentInput()
    }

    private func finish(exitStatus: Int32?) {
        let shouldFinish = lock.withLock { () -> Bool in
            guard !finished else { return false }
            finished = true
            return true
        }
        guard shouldFinish else { return }
        stdoutReadHandle.readabilityHandler = nil
        stderrReadHandle.readabilityHandler = nil
        consumeSupervisedJournalChildOutput(from: stdoutReadHandle, stream: .stdout, handler: outputHandler)
        consumeSupervisedJournalChildOutput(from: stderrReadHandle, stream: .stderr, handler: outputHandler)
        outputHandler.end(exitStatus: exitStatus)
    }
}

public actor SupervisedJournalRunner: SupervisedChildRunning {
    private struct LaunchRequest: Sendable {
        let runtime: MaterializedRuntime
        let journalRoot: URL
        let port: Int
        let receiptContext: JournalRuntimeEntryReceiptContext
    }

    private enum GenerationPhase: Sendable {
        case launching
        case active
        case stopping
        case containing
        case exited
        case admissionFailed
    }

    private struct GenerationRecord: Sendable {
        let rawPID: pid_t
        let context: JournalRuntimeEntryReceiptContext
        var identity: SupervisedChildIdentity?
        var containmentDomain: JournalContainmentDomain?
        var receiptRecorded = false
        var observedMembers: [JournalContainmentObservedMember] = []
        var readinessCensusIndeterminate = false
        var phase: GenerationPhase
    }

    private let clock: any MonotonicClock
    private let statusSink: @Sendable (JournalRuntimeStatus) -> Void
    private let gate: any SingleSupervisorGating
    private let containmentEvidenceReader: any JournalProcessContainmentEvidenceReading
    private let processSpawner: any SupervisedJournalProcessSpawning
    private let pidExists: @Sendable (pid_t) -> Bool
    private let terminate: @Sendable (pid_t, Int32) -> Int32
    private let backoffSchedule: [Duration] = [
        .seconds(1),
        .seconds(2),
        .seconds(4),
        .seconds(8),
        .seconds(16),
        .seconds(30)
    ]
    private let restartWindow: Duration = .seconds(60)
    private let restartLimit = 5
    private let stabilityThreshold: Duration = .seconds(30)
    private let terminationWait: Duration = .seconds(10)
    private let terminationGrace: Duration = .seconds(2)

    private var process: (any SupervisedJournalChildProcess)?
    private var launchRequest: LaunchRequest?
    private var currentKey: String?
    private var stopping = false
    private var breakerTripped = false
    private var consecutiveGateBlocks = 0
    private var terminalDiagnostic: JournalDiagnostic?
    private var generationRecords: [UInt64: GenerationRecord] = [:]
    private var activeGeneration: UInt64?
    private var launchGeneration: UInt64 = 0
    private var backoffIndex = 0
    private var unexpectedExitTimes: [Duration] = []
    private var stabilityTask: Task<Void, Never>?
    private var relaunchTask: Task<Void, Never>?
    private var pendingRelaunchGeneration: UInt64?
    private let requiredModels: (any JournalRequiredModelsReconciling)?
    private var preparationGeneration: UInt64 = 0
    private var requiredModelsTask: Task<Void, Error>?

    public init(
        clock: any MonotonicClock = SystemMonotonicClock(),
        statusSink: @escaping @Sendable (JournalRuntimeStatus) -> Void,
        gate: any SingleSupervisorGating = SingleSupervisorGate(),
        requiredModels: any JournalRequiredModelsReconciling = JournalRequiredModelsReconciler(),
        pidExists: @escaping @Sendable (pid_t) -> Bool = { pid in
            if Darwin.kill(pid, 0) == 0 { return true }
            return errno == EPERM
        },
        terminate: @escaping @Sendable (pid_t, Int32) -> Int32 = { pid, signal in
            Darwin.kill(pid, signal)
        }
    ) {
        self.init(
            clock: clock,
            statusSink: statusSink,
            gate: gate,
            requiredModels: requiredModels,
            containmentEvidenceReader: LiveJournalProcessContainmentEvidenceReader(),
            processSpawner: FoundationSupervisedJournalProcessSpawner(),
            pidExists: pidExists,
            terminate: terminate
        )
    }

    init(
        clock: any MonotonicClock = SystemMonotonicClock(),
        statusSink: @escaping @Sendable (JournalRuntimeStatus) -> Void,
        gate: any SingleSupervisorGating = SingleSupervisorGate(),
        requiredModels: (any JournalRequiredModelsReconciling)? = nil,
        containmentEvidenceReader: any JournalProcessContainmentEvidenceReading = LiveJournalProcessContainmentEvidenceReader(),
        processSpawner: any SupervisedJournalProcessSpawning,
        pidExists: @escaping @Sendable (pid_t) -> Bool = { pid in
            if Darwin.kill(pid, 0) == 0 { return true }
            return errno == EPERM
        },
        terminate: @escaping @Sendable (pid_t, Int32) -> Int32 = { pid, signal in
            Darwin.kill(pid, signal)
        }
    ) {
        self.clock = clock
        self.statusSink = statusSink
        self.gate = gate
        self.requiredModels = requiredModels
        self.containmentEvidenceReader = containmentEvidenceReader
        self.processSpawner = processSpawner
        self.pidExists = pidExists
        self.terminate = terminate
    }

    public func start(
        runtime: MaterializedRuntime,
        journalRoot: URL,
        port: Int,
        receiptContext: JournalRuntimeEntryReceiptContext
    ) async throws {
        guard !hasContainingGeneration else {
            throw SupervisedJournalRunnerError.launchFailed("journal child containment is in progress")
        }
        preparationGeneration &+= 1
        let operation = preparationGeneration
        await cancelRequiredModelsPreparation()
        Logger.journal.notice("journal-lifecycle: runner-start port=\(port, privacy: .public)")
        await cancelPendingRelaunch()
        try Task.checkCancellation()
        guard operation == preparationGeneration else { throw CancellationError() }
        launchRequest = LaunchRequest(
            runtime: runtime,
            journalRoot: journalRoot,
            port: port,
            receiptContext: receiptContext
        )
        currentKey = runtime.key
        breakerTripped = false
        consecutiveGateBlocks = 0
        terminalDiagnostic = nil
        stopping = false
        try await launch(
            runtime: runtime,
            journalRoot: journalRoot,
            port: port,
            receiptContext: receiptContext
        )
    }

    public func restart() async throws {
        guard let launchRequest else {
            throw SupervisedJournalRunnerError.alreadyStopped
        }
        guard !hasContainingGeneration else {
            throw SupervisedJournalRunnerError.launchFailed("journal child containment is in progress")
        }
        preparationGeneration &+= 1
        let operation = preparationGeneration
        await cancelRequiredModelsPreparation()
        try Task.checkCancellation()
        guard operation == preparationGeneration else { throw CancellationError() }
        Logger.journal.notice("journal-lifecycle: runner-restart transition=restarting")
        statusSink(.restarting(generation: nil))
        stopping = true
        await cancelPendingRelaunch()
        await stopCurrentProcess()
        try Task.checkCancellation()
        guard operation == preparationGeneration else { throw CancellationError() }
        stopping = false
        breakerTripped = false
        consecutiveGateBlocks = 0
        terminalDiagnostic = nil
        try await launch(
            runtime: launchRequest.runtime,
            journalRoot: launchRequest.journalRoot,
            port: launchRequest.port,
            receiptContext: launchRequest.receiptContext
        )
    }

    public func stop() async {
        stopping = true
        preparationGeneration &+= 1
        await cancelRequiredModelsPreparation()
        Logger.journal.notice("journal-lifecycle: runner-stop")
        await cancelPendingRelaunch()
        await stopCurrentProcess()
        launchRequest = nil
        currentKey = nil
        terminalDiagnostic = nil
        stabilityTask?.cancel()
        stabilityTask = nil
    }

    public func stopForTermination() async {
        stopping = true
        preparationGeneration &+= 1
        await cancelRequiredModelsPreparation()
        Logger.journal.notice("journal-lifecycle: runner-stop-for-termination")
        await cancelPendingRelaunch()
        await stopCurrentProcess()
    }

    public func currentRuntimeKey() async -> String? {
        currentKey
    }

    public func currentIdentity() async -> SupervisedChildIdentity? {
        guard let generation = activeGeneration,
              let record = generationRecords[generation],
              record.phase == .active else {
            return nil
        }
        return record.identity
    }

    public func terminalReason() async -> JournalDiagnostic? {
        terminalDiagnostic
    }

    public func markReady(identity: SupervisedChildIdentity) async -> Bool {
        guard let generation = activeGeneration,
              var record = generationRecords[generation],
              record.phase == .active,
              record.identity == identity else {
            return false
        }
        if let domain = record.containmentDomain {
            let retainedAuthority = launchRequest.flatMap {
                parentLossCoordinatorAuthority(journalRoot: $0.journalRoot, domain: domain)
            }
            let scopedDomain = JournalContainmentDomain(
                processGroupID: domain.processGroupID,
                birthKernelStartTime: domain.birthKernelStartTime,
                generation: domain.generation,
                leaderIdentity: domain.leaderIdentity,
                retainedAuthority: retainedAuthority
            )
            if let observedMembers = captureObservedMembers(domain: scopedDomain) {
                record.containmentDomain = scopedDomain
                record.observedMembers = observedMembers
            } else {
                record.readinessCensusIndeterminate = true
            }
            generationRecords[generation] = record
        }
        statusSink(.running)
        stabilityTask?.cancel()
        stabilityTask = Task { [weak self] in
            guard let self else { return }
            await self.sleepForStabilityAndReset()
        }
        return true
    }

    private func cancelRequiredModelsPreparation() async {
        let task = requiredModelsTask
        let operation = preparationGeneration
        task?.cancel()
        _ = try? await task?.value
        if operation == preparationGeneration { requiredModelsTask = nil }
    }

    private func launch(
        runtime: MaterializedRuntime,
        journalRoot: URL,
        port: Int,
        receiptContext: JournalRuntimeEntryReceiptContext,
        automaticReplacement: Bool = false
    ) async throws {
        preparationGeneration &+= 1
        let preparation = preparationGeneration
        if process?.isRunning == true {
            await stopCurrentProcess()
        }
        try Task.checkCancellation()
        guard preparation == preparationGeneration, !stopping else { throw CancellationError() }

        let canonicalJournalRoot = URL(fileURLWithPath: canonicalPath(journalRoot), isDirectory: true)
        let spawnRequest = SupervisedJournalSpawnRequest(
            executableURL: runtime.layout.solstoneBinary,
            currentDirectoryURL: canonicalJournalRoot,
            arguments: ["journal", "start", "--hosted-parent", String(port)],
            environment: runtime.environment
        )
        switch await gate.prepareForSpawn(journalRoot: canonicalJournalRoot) {
        case .success:
            Logger.journal.notice("journal-lifecycle: runner-gate-open")
        case .blocked(let blockage):
            throw SupervisedJournalRunnerError.gateBlocked(blockage)
        }

        try Task.checkCancellation()
        guard preparation == preparationGeneration, !stopping else {
            throw CancellationError()
        }
        if let requiredModels {
            let task = Task {
                try await requiredModels.reconcile(runtime: runtime, journalRoot: canonicalJournalRoot)
            }
            requiredModelsTask = task
            do {
                try await withTaskCancellationHandler {
                    try await task.value
                } onCancel: {
                    task.cancel()
                }
            } catch {
                if preparation == preparationGeneration { requiredModelsTask = nil }
                throw error
            }
            if preparation == preparationGeneration { requiredModelsTask = nil }
            // Preparation yields while repairing files. Recheck exclusion before spawn.
            try Task.checkCancellation()
            guard preparation == preparationGeneration, !stopping else {
                throw CancellationError()
            }
            switch await gate.prepareForSpawn(journalRoot: canonicalJournalRoot) {
            case .success:
                break
            case .blocked(let blockage):
                throw SupervisedJournalRunnerError.gateBlocked(blockage)
            }
        }
        try Task.checkCancellation()
        guard preparation == preparationGeneration, !stopping else {
            throw CancellationError()
        }

        let child = processSpawner.makeChildProcess(for: spawnRequest)
        launchGeneration += 1
        let generation = launchGeneration
        child.setTerminationHandler { [weak self] status, pid in
            Task {
                await self?.childExited(status: status, pid: pid, generation: generation)
            }
        }

        do {
            process = child
            try child.run()
            // This is intentionally synchronous and immediately follows run().
            // A Process termination callback may be queued already, but it cannot
            // enter this actor before this raw generation marker exists.
            generationRecords[generation] = GenerationRecord(
                rawPID: child.processIdentifier,
                context: receiptContext,
                phase: .launching
            )
            activeGeneration = generation

            guard let evidence = containmentEvidenceReader.containmentEvidence(for: child.processIdentifier),
                  evidence.pid == child.processIdentifier,
                  evidence.processGroupID > 0,
                  evidence.processGroupID != getpgrp(),
                  let startTime = evidence.kernelStartTime,
                  startTime.isFinite else {
                if var record = generationRecords[generation] {
                    record.phase = .admissionFailed
                    generationRecords[generation] = record
                }
                activeGeneration = nil
                Logger.journal.error("journal-lifecycle: runner-child-admission-failed pid=\(child.processIdentifier, privacy: .public)")
                await stopChild(child)
                if process?.processIdentifier == child.processIdentifier {
                    process = nil
                }
                generationRecords.removeValue(forKey: generation)
                throw SupervisedJournalRunnerError.launchFailed("journal child admission could not be verified")
            }

            let identity = SupervisedChildIdentity(
                pid: child.processIdentifier,
                kernelStartTime: startTime,
                generation: generation
            )
            let domain = JournalContainmentDomain(
                processGroupID: evidence.processGroupID,
                birthKernelStartTime: startTime,
                generation: generation,
                leaderIdentity: identity
            )
            if var record = generationRecords[generation] {
                record.identity = identity
                record.containmentDomain = domain
                record.phase = .active
                generationRecords[generation] = record
            }
            if let draft = receiptContext.payloadEntryDraft(identity: identity) {
                _ = await receiptContext.sink.append(draft)
            }
            Logger.journal.notice("journal-lifecycle: runner-child-launched pid=\(child.processIdentifier, privacy: .public) port=\(port, privacy: .public)")
            if automaticReplacement,
               activeGeneration == generation,
               generationRecords[generation]?.phase == .active {
                statusSink(.restarting(generation: generation))
            }
        } catch {
            process = nil
            activeGeneration = nil
            generationRecords.removeValue(forKey: generation)
            if let runnerError = error as? SupervisedJournalRunnerError {
                throw runnerError
            }
            throw SupervisedJournalRunnerError.launchFailed(error.localizedDescription)
        }
    }

    private func childExited(status: Int32, pid: pid_t, generation: UInt64) async {
        guard let record = generationRecords[generation], record.rawPID == pid else { return }
        guard activeGeneration == generation, record.phase == .active else {
            await recordExitReceipt(generation: generation, pid: pid, status: status, expectedStop: true)
            if activeGeneration == generation {
                activeGeneration = nil
                if process?.processIdentifier == pid {
                    process = nil
                }
            }
            if record.phase == .stopping || record.phase == .admissionFailed {
                generationRecords.removeValue(forKey: generation)
            }
            return
        }

        var containingRecord = record
        containingRecord.phase = .containing
        generationRecords[generation] = containingRecord
        activeGeneration = nil
        if process?.processIdentifier == pid {
            process = nil
        }
        stabilityTask?.cancel()
        stabilityTask = nil
        await cancelPendingRelaunch()

        let expectedStop = stopping
        await recordExitReceipt(generation: generation, pid: pid, status: status, expectedStop: expectedStop)
        if expectedStop {
            Logger.journal.notice("journal-lifecycle: child-exit status=\(status, privacy: .public) expected=true")
            generationRecords.removeValue(forKey: generation)
            return
        }
        guard !breakerTripped else {
            generationRecords.removeValue(forKey: generation)
            return
        }

        let containmentResult: JournalContainmentResult
        if containingRecord.readinessCensusIndeterminate {
            containmentResult = .unresolved([.readinessCensusIndeterminate])
        } else if let domain = containingRecord.containmentDomain {
            let containment = JournalProcessContainment(
                evidenceReader: containmentEvidenceReader,
                terminate: terminate,
                clock: clock,
                gracePeriod: terminationGrace,
                pidExists: pidExists
            )
            containmentResult = await containment.retire(
                domain: domain,
                observedMembers: containingRecord.observedMembers
            )
        } else {
            // Defensive fallback: .active currently always admits its domain synchronously.
            containmentResult = .unresolved([.missingDomainAdmission])
        }
        guard !stopping,
              !breakerTripped,
              activeGeneration == nil,
              generationRecords[generation]?.phase == .containing else {
            generationRecords.removeValue(forKey: generation)
            return
        }
        switch containmentResult {
        case .unresolved:
            let diagnostic = JournalDiagnostic(
                commandLabel: "solstone journal start --hosted-parent",
                exitCode: status,
                outputExcerpt: UICopy.JOURNAL_CHILD_CONTAINMENT_UNRESOLVED
            )
            terminalDiagnostic = diagnostic
            breakerTripped = true
            generationRecords.removeValue(forKey: generation)
            statusSink(.stopped(diagnostic))
            return
        case .clean, .noActiveGeneration:
            break
        }

        generationRecords[generation]?.phase = .exited
        recordUnexpectedExit()
        Logger.journal.warning("journal-lifecycle: child-exit status=\(status, privacy: .public) expected=false unexpectedCount=\(self.unexpectedExitTimes.count, privacy: .public)")
        if unexpectedExitTimes.count >= restartLimit {
            breakerTripped = true
            Logger.journal.error("journal-lifecycle: runner-breaker-tripped status=\(status, privacy: .public) unexpectedCount=\(self.unexpectedExitTimes.count, privacy: .public)")
            let diagnostic = JournalDiagnostic(
                commandLabel: "solstone journal start --hosted-parent",
                exitCode: status,
                outputExcerpt: UICopy.JOURNAL_CHILD_BREAKER_TRIPPED
            )
            terminalDiagnostic = diagnostic
            generationRecords.removeValue(forKey: generation)
            statusSink(.stopped(diagnostic))
            return
        }

        guard launchRequest != nil else { return }
        scheduleBackoffRelaunch(afterGeneration: generation)
    }

    private func scheduleBackoffRelaunch(afterGeneration generation: UInt64) {
        let delay = backoffSchedule[min(backoffIndex, backoffSchedule.count - 1)]
        backoffIndex = min(backoffIndex + 1, backoffSchedule.count - 1)
        pendingRelaunchGeneration = generation
        relaunchTask = Task { [weak self] in
            await self?.performBackoffRelaunch(delay: delay, afterGeneration: generation)
        }
    }

    private func cancelPendingRelaunch() async {
        let task = relaunchTask
        let generation = pendingRelaunchGeneration
        pendingRelaunchGeneration = nil
        task?.cancel()
        await task?.value
        relaunchTask = nil
        if let generation, generationRecords[generation]?.phase == .exited {
            generationRecords.removeValue(forKey: generation)
        }
    }

    private func performBackoffRelaunch(delay: Duration, afterGeneration generation: UInt64) async {
        await clock.sleep(for: delay)
        guard !Task.isCancelled,
              pendingRelaunchGeneration == generation,
              generationRecords[generation]?.phase == .exited,
              !stopping,
              !breakerTripped,
              let launchRequest else { return }
        pendingRelaunchGeneration = nil
        relaunchTask = nil
        let exitedRecord = generationRecords.removeValue(forKey: generation)
        Logger.journal.notice("journal-lifecycle: runner-backoff-relaunch delaySeconds=\(delay.components.seconds, privacy: .public)")
        let preparationBeforeLaunch = preparationGeneration
        do {
            try await launch(
                runtime: launchRequest.runtime,
                journalRoot: launchRequest.journalRoot,
                port: launchRequest.port,
                receiptContext: launchRequest.receiptContext,
                automaticReplacement: true
            )
            consecutiveGateBlocks = 0
        } catch {
            // A start, restart or stop that began while this launch was in
            // flight owns the runner now; this attempt has nothing to report,
            // and reporting it would stop the journal that start brought up.
            guard preparationGeneration == preparationBeforeLaunch &+ 1,
                  !stopping,
                  activeGeneration == nil,
                  pendingRelaunchGeneration == nil,
                  !(error is CancellationError) else { return }
            if case SupervisedJournalRunnerError.gateBlocked(let blockage) = error {
                // A blocked gate is one more failed attempt, not the end: the
                // check may not have been able to run (launchd slow under memory
                // pressure, ports not yet released). Counted without a time
                // window, so a slow gate cannot retry forever.
                consecutiveGateBlocks += 1
                if consecutiveGateBlocks < restartLimit, let exitedRecord {
                    Logger.journal.warning("journal-lifecycle: runner-backoff-gate-blocked retry=true count=\(self.consecutiveGateBlocks, privacy: .public)")
                    generationRecords[generation] = exitedRecord
                    statusSink(.restarting(generation: nil))
                    scheduleBackoffRelaunch(afterGeneration: generation)
                    return
                }
                Logger.journal.error("journal-lifecycle: runner-backoff-gate-blocked retry=false count=\(self.consecutiveGateBlocks, privacy: .public)")
                let diagnostic = JournalDiagnostic(
                    commandLabel: blockage.diagnostic.commandLabel,
                    outputExcerpt: blockage.ownerMessage
                )
                terminalDiagnostic = diagnostic
                breakerTripped = true
                statusSink(.stopped(diagnostic))
                return
            }
            Logger.journal.error("journal-lifecycle: runner-backoff-relaunch-failed")
            let diagnostic = JournalDiagnostic(
                commandLabel: "solstone journal start --hosted-parent",
                outputExcerpt: sanitizeJournalDiagnosticOutput(error.localizedDescription)
            )
            terminalDiagnostic = diagnostic
            statusSink(.stopped(diagnostic))
        }
    }

    private func recordUnexpectedExit() {
        let now = clock.now()
        unexpectedExitTimes = unexpectedExitTimes
            .filter { now - $0 <= restartWindow }
        unexpectedExitTimes.append(now)
    }

    private func sleepForStabilityAndReset() async {
        await clock.sleep(for: stabilityThreshold)
        guard !Task.isCancelled, process?.isRunning == true else { return }
        backoffIndex = 0
        unexpectedExitTimes.removeAll()
    }

    private func stopCurrentProcess() async {
        guard let child = process else { return }
        if let generation = activeGeneration, var record = generationRecords[generation], record.rawPID == child.processIdentifier {
            record.phase = .stopping
            generationRecords[generation] = record
        }
        await stopChild(child)
        if process?.processIdentifier == child.processIdentifier {
            process = nil
        }
    }

    private func stopChild(_ child: any SupervisedJournalChildProcess) async {
        child.closeParentInput()
        await waitForProcessExit(child, timeout: terminationWait)
        if child.isRunning {
            _ = terminate(child.processIdentifier, SIGTERM)
            await waitForProcessExit(child, timeout: terminationGrace)
        }
        if child.isRunning {
            _ = terminate(child.processIdentifier, SIGKILL)
        }
    }

    private func parentLossCoordinatorAuthority(
        journalRoot: URL,
        domain: JournalContainmentDomain
    ) -> JournalContainmentObservedMember? {
        let activeGenerationURL = journalRoot.appendingPathComponent("health/parent-loss/active-generation.json")
        guard let data = try? Data(contentsOf: activeGenerationURL),
              let active = try? JSONDecoder().decode(ParentLossLedgerActiveGeneration.self, from: data),
              active.schema == 1 else {
            return nil
        }

        let supervisorStartTime = Double(active.supervisor.birth.epochMicros) / 1_000_000
        guard active.supervisor.pid == domain.leaderIdentity.pid,
              abs(supervisorStartTime - domain.leaderIdentity.kernelStartTime) <= journalSupervisorStartTimeToleranceSeconds else {
            return nil
        }

        let coordinatorStartTime = Double(active.coordinator.birth.epochMicros) / 1_000_000
        guard active.coordinator.pid > 1,
              active.coordinator.pid != domain.leaderIdentity.pid,
              active.coordinator.pid != getpid(),
              coordinatorStartTime.isFinite,
              coordinatorStartTime >= domain.birthKernelStartTime,
              coordinatorStartTime <= Date().timeIntervalSince1970,
              let evidence = containmentEvidenceReader.containmentEvidence(for: active.coordinator.pid),
              evidence.pid == active.coordinator.pid,
              evidence.uid == getuid(),
              evidence.username == currentUsername(),
              evidence.processGroupID != getpgrp(),
              let actualStartTime = evidence.kernelStartTime,
              actualStartTime.isFinite,
              abs(actualStartTime - coordinatorStartTime) <= journalSupervisorStartTimeToleranceSeconds else {
            return nil
        }

        return JournalContainmentObservedMember(pid: active.coordinator.pid, kernelStartTime: coordinatorStartTime)
    }

    private func captureObservedMembers(domain: JournalContainmentDomain) -> [JournalContainmentObservedMember]? {
        guard let pids = containmentEvidenceReader.descendantProcessIDs(of: domain.leaderIdentity.pid) else {
            return nil
        }
        var observed: [JournalContainmentObservedMember] = []
        for pid in Set(pids).sorted() {
            guard let evidence = containmentEvidenceReader.containmentEvidence(for: pid),
                  evidence.pid == pid,
                  let startTime = evidence.kernelStartTime,
                  startTime.isFinite else {
                return nil
            }
            observed.append(JournalContainmentObservedMember(pid: pid, kernelStartTime: startTime))
        }
        return observed
    }

    private func currentActiveIdentity() -> SupervisedChildIdentity? {
        guard let generation = activeGeneration,
              let record = generationRecords[generation],
              record.phase == .active else {
            return nil
        }
        return record.identity
    }

    private var hasContainingGeneration: Bool {
        generationRecords.values.contains { $0.phase == .containing }
    }

    private func recordExitReceipt(
        generation: UInt64,
        pid: pid_t,
        status: Int32,
        expectedStop: Bool
    ) async {
        guard var record = generationRecords[generation],
              record.rawPID == pid,
              !record.receiptRecorded,
              let identity = record.identity else { return }
        record.receiptRecorded = true
        generationRecords[generation] = record
        if let draft = record.context.payloadExitDraft(
            identity: identity,
            expectedStop: expectedStop,
            terminationStatus: status
        ) {
            _ = await record.context.sink.append(draft)
        }
    }

    private func waitForProcessExit(_ child: any SupervisedJournalChildProcess, timeout: Duration) async {
        let deadline = clock.now() + timeout
        while clock.now() < deadline {
            if !child.isRunning || !pidExists(child.processIdentifier) {
                return
            }
            await clock.sleep(for: .milliseconds(100))
        }
    }
}
