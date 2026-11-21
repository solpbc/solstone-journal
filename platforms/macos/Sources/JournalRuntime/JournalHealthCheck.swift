import Foundation
import os
import SolstoneCore

public struct JournalDiagnostic: Equatable, Sendable {
    public let commandLabel: String
    public let timedOut: Bool
    public let exitCode: Int32?
    public let outputExcerpt: String?

    public init(
        commandLabel: String,
        timedOut: Bool = false,
        exitCode: Int32? = nil,
        outputExcerpt: String? = nil
    ) {
        self.commandLabel = commandLabel
        self.timedOut = timedOut
        self.exitCode = exitCode
        self.outputExcerpt = outputExcerpt
    }
}

public enum JournalRuntimeStatus: Equatable, Sendable {
    case unobserved
    case running
    case stopped(JournalDiagnostic)
    case stoppedByUser
    case restarting(generation: UInt64?)
    case setupNeeded
    case unknown(JournalDiagnostic)

    public var diagnostic: JournalDiagnostic? {
        switch self {
        case .stopped(let diagnostic), .unknown(let diagnostic):
            return diagnostic
        case .unobserved, .running, .stoppedByUser, .restarting(_), .setupNeeded:
            return nil
        }
    }

    public var isRestarting: Bool {
        if case .restarting(_) = self { return true }
        return false
    }

    public var isSetupNeeded: Bool {
        if case .setupNeeded = self { return true }
        return false
    }

    public var isStoppedByUser: Bool {
        if case .stoppedByUser = self { return true }
        return false
    }

}

public enum JournalHealthCheckResult: Equatable, Sendable {
    case healthy
    case stopped(JournalDiagnostic)
    case unknown(JournalDiagnostic)
}

public enum JournalDoctorResult: Equatable, Sendable {
    case report(DoctorReport)
    case setupNeeded
    case stopped(JournalDiagnostic)
    case unknown(JournalDiagnostic)
}

public struct DoctorCheck: Decodable, Equatable, Sendable {
    public let name: String
    public let status: DoctorStatus
    public let severity: String?
    public let detail: String?
    public let fix: String?

    public init(name: String, status: DoctorStatus, severity: String?, detail: String?, fix: String?) {
        self.name = name
        self.status = status
        self.severity = severity
        self.detail = detail
        self.fix = fix
    }
}

public enum DoctorStatus: Decodable, Equatable, Sendable {
    case ok
    case warn
    case fail
    case skip
    case unknown(String)

    public init(from decoder: Decoder) throws {
        let raw = try decoder.singleValueContainer().decode(String.self)
        switch raw {
        case "ok":
            self = .ok
        case "warn":
            self = .warn
        case "fail":
            self = .fail
        case "skip":
            self = .skip
        default:
            self = .unknown(raw)
        }
    }
}

public struct DoctorReport: Decodable, Equatable, Sendable {
    public let checks: [DoctorCheck]
    public let summary: DoctorSummary?

    public init(checks: [DoctorCheck], summary: DoctorSummary?) {
        self.checks = checks
        self.summary = summary
    }
}

public enum JournalHealthCheck {
    public static func run(
        journalBinary: URL,
        runner: SubprocessRunning = SubprocessRunner(),
        environment: [String: String]? = nil
    ) async -> JournalHealthCheckResult {
        let output = LockedHealthOutput()
        do {
            let result = try await withTimeout(seconds: 5.0) {
                try await runner.run(
                    executable: journalBinary,
                    arguments: ["journal", "health"],
                    environment: environment,
                    stdoutHandler: { data in output.append(data) },
                    stderrHandler: { data in output.append(data) }
                )
            }
            if result.exitCode == 0 {
                Logger.setup.info("solstone journal health: ok")
                return .healthy
            }
            let diagnostic = JournalDiagnostic(
                commandLabel: "solstone journal health",
                exitCode: result.exitCode,
                outputExcerpt: sanitizeJournalDiagnosticOutput(output.string)
            )
            Logger.setup.info("solstone journal health: stopped exit=\(result.exitCode, privacy: .public)")
            return .stopped(diagnostic)
        } catch is TimeoutError {
            return .unknown(JournalDiagnostic(
                commandLabel: "solstone journal health",
                timedOut: true,
                outputExcerpt: sanitizeJournalDiagnosticOutput(output.string)
            ))
        } catch {
            return .unknown(JournalDiagnostic(
                commandLabel: "solstone journal health",
                outputExcerpt: sanitizeJournalDiagnosticOutput(error.localizedDescription)
            ))
        }
    }

    public static func version(
        journalBinary: URL,
        runner: SubprocessRunning = SubprocessRunner(),
        environment: [String: String]? = nil,
        timeout: Duration? = .seconds(5)
    ) async -> String? {
        let output = LockedHealthOutput()
        do {
            let result = try await runner.run(
                executable: journalBinary,
                arguments: ["journal", "--version"],
                environment: environment,
                timeout: timeout,
                stdoutHandler: { data in output.append(data) },
                stderrHandler: { _ in }
            )
            guard result.exitCode == 0 else { return nil }
            return SolVersionParser.parse(output.string)
        } catch {
            return nil
        }
    }

    public static func doctor(
        journalBinary: URL,
        runner: SubprocessRunning = SubprocessRunner(),
        fileExists: @Sendable (String) -> Bool = { FileManager.default.isExecutableFile(atPath: $0) }
    ) async -> JournalDoctorResult {
        guard fileExists(journalBinary.path) else {
            return .setupNeeded
        }

        let output = LockedSplitHealthOutput()
        let result: SubprocessResult
        do {
            result = try await withTimeout(seconds: 10.0) {
                try await runner.run(
                    executable: journalBinary,
                    arguments: ["journal", "doctor", "--json"],
                    environment: nil,
                    stdoutHandler: { data in output.append(data, stream: .stdout) },
                    stderrHandler: { data in output.append(data, stream: .stderr) }
                )
            }
        } catch is TimeoutError {
            return .unknown(JournalDiagnostic(
                commandLabel: "solstone journal doctor",
                timedOut: true,
                outputExcerpt: sanitizeJournalDiagnosticOutput(output.combinedString)
            ))
        } catch {
            return .unknown(JournalDiagnostic(
                commandLabel: "solstone journal doctor",
                outputExcerpt: sanitizeJournalDiagnosticOutput(error.localizedDescription)
            ))
        }

        let stdout = output.stdoutString
        let trimmedStdout = stdout.trimmingCharacters(in: .whitespacesAndNewlines)
        if !trimmedStdout.isEmpty {
            do {
                return .report(try JSONDecoder().decode(DoctorReport.self, from: Data(stdout.utf8)))
            } catch {
                if result.exitCode != 0 {
                    return .stopped(JournalDiagnostic(
                        commandLabel: "solstone journal doctor",
                        exitCode: result.exitCode,
                        outputExcerpt: sanitizeJournalDiagnosticOutput(output.combinedString)
                    ))
                }
                return .unknown(JournalDiagnostic(
                    commandLabel: "solstone journal doctor",
                    outputExcerpt: sanitizeJournalDiagnosticOutput(error.localizedDescription)
                ))
            }
        }
        if result.exitCode != 0 {
            return .stopped(JournalDiagnostic(
                commandLabel: "solstone journal doctor",
                exitCode: result.exitCode,
                outputExcerpt: sanitizeJournalDiagnosticOutput(output.combinedString)
            ))
        }
        return .unknown(JournalDiagnostic(
            commandLabel: "solstone journal doctor",
            outputExcerpt: "no output"
        ))
    }
}

public enum JournalRuntimeProbeOutcome: Equatable, Sendable {
    case reachable
    case unreachable(JournalDiagnostic)
    case unknown(JournalDiagnostic)
    case binaryMissing
}

public enum JournalRuntimeProbe {
    public static func run(
        journalBinary: URL,
        runner: SubprocessRunning = SubprocessRunner(),
        fileExists: @Sendable (String) -> Bool = { FileManager.default.isExecutableFile(atPath: $0) }
    ) async -> JournalRuntimeProbeOutcome {
        guard fileExists(journalBinary.path) else {
            return .binaryMissing
        }

        switch await JournalHealthCheck.run(journalBinary: journalBinary, runner: runner) {
        case .healthy:
            return .reachable
        case .stopped(let diagnostic):
            return .unreachable(diagnostic)
        case .unknown(let diagnostic):
            return .unknown(diagnostic)
        }
    }
}

public func appProbeChecks(
    screenRecordingGranted: Bool,
    microphoneGranted: Bool,
    permissionCheckComplete: Bool
) -> [DoctorCheck] {
    [
        permissionProbeCheck(
            name: JournalCopy.healthCheckScreenRecording(),
            granted: screenRecordingGranted,
            permissionCheckComplete: permissionCheckComplete,
            fix: JournalCopy.healthScreenPermissionHelp()
        ),
        permissionProbeCheck(
            name: JournalCopy.healthCheckMicrophone(),
            granted: microphoneGranted,
            permissionCheckComplete: permissionCheckComplete,
            fix: JournalCopy.healthMicrophonePermissionHelp()
        ),
    ]
}

internal func sanitizeJournalDiagnosticOutput(
    _ output: String?,
    homeDirectory: String = NSHomeDirectory(),
    limit: Int = 200
) -> String? {
    guard var value = output else { return nil }
    value = value.replacingOccurrences(of: homeDirectory, with: "~")
    value = value.components(separatedBy: .whitespacesAndNewlines)
        .filter { !$0.isEmpty }
        .joined(separator: " ")
    guard !value.isEmpty else { return nil }
    guard value.count > limit else { return value }
    let prefix = value.prefix(max(0, limit - 1))
    return "\(prefix)…"
}

private func permissionProbeCheck(
    name: String,
    granted: Bool,
    permissionCheckComplete: Bool,
    fix: String
) -> DoctorCheck {
    if !permissionCheckComplete {
        return DoctorCheck(
            name: name,
            status: .warn,
            severity: nil,
            detail: JournalCopy.healthCheckUnknownCheckingPermissions(),
            fix: nil
        )
    }

    if granted {
        return DoctorCheck(
            name: name,
            status: .ok,
            severity: nil,
            detail: JournalCopy.healthCheckGranted(),
            fix: nil
        )
    }

    return DoctorCheck(
        name: name,
        status: .fail,
        severity: nil,
        detail: JournalCopy.healthCheckDenied(),
        fix: fix
    )
}

private final class LockedHealthOutput: @unchecked Sendable {
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

private final class LockedSplitHealthOutput: @unchecked Sendable {
    enum Stream {
        case stdout
        case stderr
    }

    private let lock = NSLock()
    private var stdout = Data()
    private var stderr = Data()

    var stdoutString: String {
        lock.withLock { String(data: stdout, encoding: .utf8) ?? "" }
    }

    var combinedString: String {
        lock.withLock {
            [String(data: stdout, encoding: .utf8) ?? "", String(data: stderr, encoding: .utf8) ?? ""]
                .filter { !$0.isEmpty }
                .joined(separator: "\n")
        }
    }

    func append(_ chunk: Data, stream: Stream) {
        lock.withLock {
            switch stream {
            case .stdout:
                stdout.append(chunk)
            case .stderr:
                stderr.append(chunk)
            }
        }
    }
}
