// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import Foundation
import SolstoneCore

public protocol JournalRequiredModelsReconciling: Sendable {
    func reconcile(runtime: MaterializedRuntime, journalRoot: URL) async throws
}

/// Reconcile the installed model cache against this signed runtime before starting it.
/// The native command checks existing bytes and repairs only bundled required assets.
public struct JournalRequiredModelsReconciler: JournalRequiredModelsReconciling {
    private let makeRunner: @Sendable (URL) -> any SubprocessRunning
    private let timeout: Duration
    private let log: any ClassifiedLogSinking

    public init(
        timeout: Duration = .seconds(120),
        log: any ClassifiedLogSinking = LoggerClassifiedLogSink.journal
    ) {
        self.timeout = timeout
        self.log = log
        makeRunner = { SubprocessRunner(currentDirectoryURL: $0) }
    }

    init(
        timeout: Duration = .seconds(120),
        makeRunner: @escaping @Sendable (URL) -> any SubprocessRunning,
        log: any ClassifiedLogSinking = LoggerClassifiedLogSink.journal
    ) {
        self.timeout = timeout
        self.makeRunner = makeRunner
        self.log = log
    }

    public func reconcile(runtime: MaterializedRuntime, journalRoot: URL) async throws {
        let runner = makeRunner(journalRoot)
        var environment = runtime.environment
        // Select this journal through cwd, without an inherited journal override.
        environment.removeValue(forKey: "SOLSTONE_JOURNAL")
        let inheritedPath = environment["PATH"].flatMap { $0.isEmpty ? nil : $0 }
            ?? "/usr/bin:/bin:/usr/sbin:/sbin"
        environment["PATH"] = runtime.layout.binDir.path + ":" + inheritedPath
        let commandEnvironment = environment
        let tally = RequiredModelsStreamTally()
        var exitCode: Int32?
        var terminationReason: Int?
        defer {
            var fields = [
                "stdoutBytes": String(tally.stdoutBytes),
                "stderrBytes": String(tally.stderrBytes),
                "stdoutEmpty": tally.stdoutBytes == 0 ? "true" : "false",
                "stderrEmpty": tally.stderrBytes == 0 ? "true" : "false",
                "stdoutUTF8": tally.stdoutUTF8 ? "true" : "false",
                "stderrUTF8": tally.stderrUTF8 ? "true" : "false",
            ]
            if let exitCode {
                fields["exit"] = String(exitCode)
            }
            if let terminationReason {
                fields["termination"] = String(terminationReason)
            }
            log.emit(
                ClassifiedLogEmission(
                    level: .notice,
                    classification: "journal-lifecycle: required-models",
                    publicFields: fields
                )
            )
        }
        try Task.checkCancellation()
        let result = try await withTaskCancellationHandler {
            try await runner.run(
                executable: runtime.layout.solstoneBinary,
                arguments: ["journal", "install-models", "--required-only"],
                environment: commandEnvironment,
                timeout: timeout,
                stdoutHandler: { data in
                    tally.append(data, stream: .stdout)
                },
                stderrHandler: { data in
                    tally.append(data, stream: .stderr)
                }
            )
        } onCancel: {
            runner.cancelAll()
        }
        exitCode = result.exitCode
        terminationReason = result.terminationReason.rawValue
        try Task.checkCancellation()
        guard result.terminationReason == .exit, result.exitCode == 0 else {
            throw SupervisedJournalRunnerError.launchFailed(
                JournalCopy.runtimeRequiredModelsFailed(String(result.exitCode), String(result.terminationReason.rawValue))
            )
        }
    }
}

private final class RequiredModelsStreamTally: @unchecked Sendable {
    private let lock = NSLock()
    private var stdout = 0
    private var stderr = 0
    private var stdoutIsUTF8 = true
    private var stderrIsUTF8 = true

    var stdoutBytes: Int { lock.withLock { stdout } }
    var stderrBytes: Int { lock.withLock { stderr } }
    var stdoutUTF8: Bool { lock.withLock { stdoutIsUTF8 } }
    var stderrUTF8: Bool { lock.withLock { stderrIsUTF8 } }

    func append(_ data: Data, stream: SupervisedJournalChildOutputStream) {
        lock.withLock {
            let isUTF8 = String(data: data, encoding: .utf8) != nil
            switch stream {
            case .stdout:
                stdout += data.count
                if !isUTF8 { stdoutIsUTF8 = false }
            case .stderr:
                stderr += data.count
                if !isUTF8 { stderrIsUTF8 = false }
            }
        }
    }
}
