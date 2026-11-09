// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import Darwin
import Foundation
import os
import SolstoneCore

public protocol LegacyJournalServiceRetiring: Sendable {
    func retireLegacyService(journalRoot: URL) async -> LegacyJournalServiceRetirementResult
    func currentLegacyServicePID() async -> LegacyJournalServicePIDLookup
}

public enum LegacyJournalServicePIDLookup: Equatable, Sendable {
    case known(pid_t?)
    case failed
}

public enum LegacyJournalServiceVerdict: String, Equatable, Sendable {
    case noService
    case provenMatchLoaded
    case provenMatchUnloaded
    case differentRoot
    case labelWithoutPlist
    case plistVsJobDisagreement
    case malformed
    case inspectionFailure
}

public enum LegacyJournalServiceRetirementResult: Equatable, Sendable {
    case success(LegacyJournalServiceVerdict)
    case blocked(LegacyJournalServiceVerdict, JournalDiagnostic)

    public var verdict: LegacyJournalServiceVerdict {
        switch self {
        case .success(let verdict), .blocked(let verdict, _):
            return verdict
        }
    }
}

public struct LegacyJournalServiceRetirer: LegacyJournalServiceRetiring, @unchecked Sendable {
    public static let label = "org.solpbc.solstone"
    private static let notLoadedMarkers = [
        "could not find",
        "service not found",
        "no such process",
        "not currently loaded"
    ]

    private let runner: SubprocessRunning
    private let clock: any MonotonicClock
    private let launchctlURL: URL
    private let plistURL: URL
    private let uid: uid_t
    private let fileManager: FileManager
    private let commandTimeout: Duration
    // `launchctl bootout` returns at once; the job stays loaded until it has
    // exited. launchd kills it 5 s after SIGTERM, or after the plist's
    // ExitTimeOut when it sets one (20 s for launchd's own default stop).
    private let absenceTimeout: Duration
    // The journal's own shutdown is budgeted at 15 s.
    private let gracefulStopTimeout: Duration
    private let absencePollInterval: Duration

    public init(
        runner: SubprocessRunning = SubprocessRunner(),
        clock: any MonotonicClock = SystemMonotonicClock(),
        launchctlURL: URL = URL(fileURLWithPath: "/bin/launchctl"),
        plistURL: URL = FileManager.default
            .homeDirectoryForCurrentUser
            .appendingPathComponent("Library/LaunchAgents/\(LegacyJournalServiceRetirer.label).plist"),
        uid: uid_t = getuid(),
        fileManager: FileManager = .default,
        commandTimeout: Duration = .seconds(10),
        absenceTimeout: Duration = .seconds(30),
        gracefulStopTimeout: Duration = .seconds(20),
        absencePollInterval: Duration = .milliseconds(250)
    ) {
        self.runner = runner
        self.clock = clock
        self.launchctlURL = launchctlURL
        self.plistURL = plistURL
        self.uid = uid
        self.fileManager = fileManager
        self.commandTimeout = commandTimeout
        self.absenceTimeout = absenceTimeout
        self.gracefulStopTimeout = gracefulStopTimeout
        self.absencePollInterval = absencePollInterval
    }

    public func currentLegacyServicePID() async -> LegacyJournalServicePIDLookup {
        switch await loadLaunchdJob() {
        case .loaded(let job):
            return .known(job.pid)
        case .notLoaded:
            return .known(nil)
        case .failed:
            return .failed
        }
    }

    public func retireLegacyService(journalRoot: URL) async -> LegacyJournalServiceRetirementResult {
        let plan = await retirementPlan(journalRoot: journalRoot)
        switch plan {
        case .noop(let verdict):
            Logger.setup.notice("journal-lifecycle: legacy-service verdict=\(verdict.rawValue, privacy: .public) action=noop")
            return .success(verdict)
        case .retire(let verdict, let loaded, let runningPID):
            Logger.setup.notice("journal-lifecycle: legacy-service verdict=\(verdict.rawValue, privacy: .public) action=retire")
            if loaded {
                if let runningPID {
                    await stopRunningJob(pid: runningPID)
                }
                let booted = await bootoutAndWaitAbsent()
                guard booted else {
                    return .blocked(.inspectionFailure, diagnostic("legacy service could not be unloaded"))
                }
            }
            do {
                if fileManager.fileExists(atPath: plistURL.path) {
                    try fileManager.removeItem(at: plistURL)
                }
                return .success(verdict)
            } catch {
                if !fileManager.fileExists(atPath: plistURL.path) {
                    return .success(verdict)
                }
                return .blocked(.inspectionFailure, diagnostic("legacy service plist could not be removed"))
            }
        case .block(let verdict, let message):
            Logger.setup.warning("journal-lifecycle: legacy-service verdict=\(verdict.rawValue, privacy: .public) action=block")
            return .blocked(verdict, diagnostic(message))
        }
    }

    private enum RetirementPlan {
        case noop(LegacyJournalServiceVerdict)
        case retire(LegacyJournalServiceVerdict, loaded: Bool, runningPID: pid_t?)
        case block(LegacyJournalServiceVerdict, String)
    }

    private enum LaunchdLoadState {
        case loaded(LaunchdJob)
        case notLoaded
        case failed
    }

    fileprivate struct LaunchdJob: Equatable {
        var path: String?
        var program: String?
        var arguments: [String]
        var state: String?
        var pid: pid_t?
    }

    fileprivate struct ServicePlist: Equatable {
        static let journalTokenKey = "SOLSTONE_INSTALLATION_JOURNAL_TOKEN"
        static let journalOverrideKey = "_SOLSTONE_JOURNAL_OVERRIDE"

        let label: String
        let programArguments: [String]
        // Written by the earlier service install, which logged to
        // <journal>/health/service.log.
        let standardOutPath: String?
        let standardErrorPath: String?
        // The current service install writes no log paths. It names its
        // journal in this environment value: the path's bytes, lower-case hex.
        let journalToken: String?
        // Some earlier installs named a non-default journal here.
        let journalOverride: String?
    }

    private func retirementPlan(journalRoot: URL) async -> RetirementPlan {
        let launchdState = await loadLaunchdJob()
        if case .failed = launchdState {
            return .block(.inspectionFailure, "legacy service could not be inspected")
        }
        let loadedJob: LaunchdJob?
        if case .loaded(let job) = launchdState {
            loadedJob = job
        } else {
            loadedJob = nil
        }

        guard fileManager.fileExists(atPath: plistURL.path) else {
            return loadedJob == nil
                ? .noop(.noService)
                : .block(.labelWithoutPlist, "legacy service is loaded without a plist")
        }
        guard let plist = readServicePlist() else {
            return .block(.malformed, "legacy service plist is malformed")
        }
        guard plist.hasLegacyJournalShape else {
            return .block(.malformed, "legacy service plist has an unexpected shape")
        }
        if let loadedJob, !loadedJob.agrees(with: plist, plistURL: plistURL) {
            return .block(.plistVsJobDisagreement, "legacy service job does not match its plist")
        }

        switch plist.rootMatch(journalRoot: journalRoot) {
        case .same:
            return loadedJob == nil
                ? .retire(.provenMatchUnloaded, loaded: false, runningPID: nil)
                : .retire(.provenMatchLoaded, loaded: true, runningPID: loadedJob?.pid)
        case .different:
            return .block(.differentRoot, "legacy service belongs to a different journal")
        case .malformed:
            return .block(.malformed, "legacy service plist has malformed output paths")
        }
    }

    private func loadLaunchdJob() async -> LaunchdLoadState {
        let output = LockedLegacyServiceOutput()
        let result: SubprocessResult
        do {
            result = try await runner.run(
                executable: launchctlURL,
                arguments: ["print", serviceDomain],
                environment: nil,
                timeout: commandTimeout,
                stdoutHandler: { data in output.appendStdout(data) },
                stderrHandler: { data in output.appendStderr(data) }
            )
        } catch {
            return .failed
        }

        if result.exitCode == 0 {
            let job = parseLaunchdJob(output.stdoutString())
            guard job.hasRequiredFields else {
                return .failed
            }
            return .loaded(job)
        }
        if isNotLoaded(stderr: output.stderrString()) {
            return .notLoaded
        }
        return .failed
    }

    /// Asks a running job to stop and waits for it, before bootout unloads it.
    /// bootout sends SIGTERM and, unless the plist sets ExitTimeOut (the
    /// service install does not), SIGKILLs the job 5 seconds later: shorter than
    /// the journal's own shutdown. The journal exits 0 on a signal, so KeepAlive
    /// (SuccessfulExit = false) leaves it stopped. If it is not gone in time,
    /// bootout below still unloads it.
    private func stopRunningJob(pid: pid_t) async {
        let output = LockedLegacyServiceOutput()
        do {
            let result = try await runner.run(
                executable: launchctlURL,
                arguments: ["kill", "SIGTERM", serviceDomain],
                environment: nil,
                timeout: commandTimeout,
                stdoutHandler: { data in output.appendStdout(data) },
                stderrHandler: { data in output.appendStderr(data) }
            )
            guard result.exitCode == 0 else { return }
        } catch {
            return
        }
        let deadline = clock.now() + gracefulStopTimeout
        repeat {
            guard case .loaded(let job) = await loadLaunchdJob(), job.pid == pid else { return }
            await clock.sleep(for: absencePollInterval)
        } while clock.now() < deadline
        Logger.setup.warning("journal-lifecycle: legacy-service graceful-stop timed-out")
    }

    private func bootoutAndWaitAbsent() async -> Bool {
        let output = LockedLegacyServiceOutput()
        do {
            let result = try await runner.run(
                executable: launchctlURL,
                arguments: ["bootout", serviceDomain],
                environment: nil,
                timeout: commandTimeout,
                stdoutHandler: { data in output.appendStdout(data) },
                stderrHandler: { data in output.appendStderr(data) }
            )
            guard result.exitCode == 0 else {
                // Another start may have unloaded it a moment earlier.
                if case .notLoaded = await loadLaunchdJob() { return true }
                return false
            }
        } catch {
            return false
        }

        let deadline = clock.now() + absenceTimeout
        repeat {
            if case .notLoaded = await loadLaunchdJob() {
                return true
            }
            await clock.sleep(for: absencePollInterval)
        } while clock.now() < deadline
        if case .notLoaded = await loadLaunchdJob() {
            return true
        }
        return false
    }

    private var serviceDomain: String {
        "gui/\(uid)/\(Self.label)"
    }

    private func diagnostic(_ message: String) -> JournalDiagnostic {
        JournalDiagnostic(
            commandLabel: "legacy journal service",
            outputExcerpt: message
        )
    }

    private func isNotLoaded(stderr: String) -> Bool {
        let lowercased = stderr.lowercased()
        return Self.notLoadedMarkers.contains { lowercased.contains($0) }
    }

    private func readServicePlist() -> ServicePlist? {
        guard let data = try? Data(contentsOf: plistURL),
              let plist = try? PropertyListSerialization.propertyList(from: data, format: nil) as? [String: Any],
              let label = plist["Label"] as? String,
              let arguments = plist["ProgramArguments"] as? [String] else {
            return nil
        }
        guard let stdout = optionalString(plist["StandardOutPath"]),
              let stderr = optionalString(plist["StandardErrorPath"]) else {
            return nil
        }
        let environment: [String: Any]
        if let value = plist["EnvironmentVariables"] {
            guard let dictionary = value as? [String: Any] else { return nil }
            environment = dictionary
        } else {
            environment = [:]
        }
        guard let journalToken = optionalString(environment[ServicePlist.journalTokenKey]),
              let journalOverride = optionalString(environment[ServicePlist.journalOverrideKey]) else {
            return nil
        }
        return ServicePlist(
            label: label,
            programArguments: arguments,
            standardOutPath: stdout,
            standardErrorPath: stderr,
            journalToken: journalToken,
            journalOverride: journalOverride
        )
    }

    /// `.some(nil)` when the key is absent, `nil` when it holds something that
    /// is not a string.
    private func optionalString(_ value: Any?) -> String?? {
        guard let value else { return .some(nil) }
        guard let string = value as? String else { return nil }
        return .some(string)
    }

    private func parseLaunchdJob(_ output: String) -> LaunchdJob {
        var job = LaunchdJob(path: nil, program: nil, arguments: [], state: nil, pid: nil)
        var collectingArguments = false

        for rawLine in output.split(whereSeparator: \.isNewline) {
            let line = rawLine.trimmingCharacters(in: .whitespaces)
            if collectingArguments {
                if line == "}" {
                    collectingArguments = false
                    continue
                }
                if !line.isEmpty {
                    job.arguments.append(String(line))
                }
                continue
            }
            if line == "arguments = {" {
                collectingArguments = true
                continue
            }
            if let value = value(after: "path =", in: line) {
                job.path = value
            } else if let value = value(after: "program =", in: line) {
                job.program = value
            } else if let value = value(after: "state =", in: line) {
                job.state = value
            } else if let value = value(after: "pid =", in: line), let pid = Int32(value), pid > 0 {
                job.pid = pid_t(pid)
            }
        }
        return job
    }

    private func value(after prefix: String, in line: String) -> String? {
        guard line.hasPrefix(prefix) else { return nil }
        let value = line.dropFirst(prefix.count).trimmingCharacters(in: .whitespaces)
        return value.isEmpty ? nil : String(value)
    }
}

private enum RootMatch {
    case same
    case different
    case malformed
}

private extension LegacyJournalServiceRetirer.ServicePlist {
    /// Every command line a journal service install has written: an absolute
    /// `journal`, `sol` or `solstone` launcher, then `journal` (always, for
    /// `solstone`), then `start <port>`, `supervisor <port>` or a bare
    /// `supervisor`.
    var hasLegacyJournalShape: Bool {
        guard label == LegacyJournalServiceRetirer.label,
              let launcher = programArguments.first,
              launcher.hasPrefix("/") else {
            return false
        }
        let name = URL(fileURLWithPath: launcher).lastPathComponent
        guard ["journal", "solstone", "sol"].contains(name) else { return false }
        var tail = programArguments.dropFirst()
        if tail.first == "journal" {
            tail = tail.dropFirst()
        } else if name == "solstone" {
            return false
        }
        let words = Array(tail)
        if words == ["supervisor"] {
            return true
        }
        guard words.count == 2, words[0] == "start" || words[0] == "supervisor" else {
            return false
        }
        let port = words[1]
        return !port.isEmpty && port.utf8.allSatisfy { (UInt8(ascii: "0")...UInt8(ascii: "9")).contains($0) }
    }

    /// Which journal the service runs: its token, else the journal it was
    /// pointed at, else the journal whose `health` folder holds its logs.
    func rootMatch(journalRoot: URL) -> RootMatch {
        let serviceRoot: String
        if let journalToken {
            guard let tokenPath = decodeJournalToken(journalToken) else {
                return .malformed
            }
            serviceRoot = tokenPath
        } else if let journalOverride {
            guard journalOverride.hasPrefix("/") else { return .malformed }
            serviceRoot = journalOverride
        } else {
            guard let standardOutPath, let standardErrorPath,
                  standardOutPath.hasPrefix("/"), standardErrorPath.hasPrefix("/") else {
                return .malformed
            }
            let outDirectory = URL(fileURLWithPath: standardOutPath).deletingLastPathComponent()
            let errDirectory = URL(fileURLWithPath: standardErrorPath).deletingLastPathComponent()
            guard outDirectory.lastPathComponent == "health",
                  canonicalPath(outDirectory) == canonicalPath(errDirectory) else {
                return .malformed
            }
            serviceRoot = outDirectory.deletingLastPathComponent().path
        }
        let actualPath = canonicalPath(URL(fileURLWithPath: serviceRoot, isDirectory: true))
        return actualPath == canonicalPath(journalRoot) ? .same : .different
    }

    private func decodeJournalToken(_ token: String) -> String? {
        let digits = Array(token.utf8)
        guard !digits.isEmpty, digits.count.isMultiple(of: 2) else { return nil }
        var bytes: [UInt8] = []
        bytes.reserveCapacity(digits.count / 2)
        var index = 0
        while index < digits.count {
            guard let high = lowerHexValue(digits[index]),
                  let low = lowerHexValue(digits[index + 1]) else {
                return nil
            }
            bytes.append(high << 4 | low)
            index += 2
        }
        guard let path = String(bytes: bytes, encoding: .utf8),
              path.hasPrefix("/") else {
            return nil
        }
        return path
    }

    private func lowerHexValue(_ digit: UInt8) -> UInt8? {
        switch digit {
        case UInt8(ascii: "0")...UInt8(ascii: "9"):
            return digit - UInt8(ascii: "0")
        case UInt8(ascii: "a")...UInt8(ascii: "f"):
            return digit - UInt8(ascii: "a") + 10
        default:
            return nil
        }
    }
}

private extension LegacyJournalServiceRetirer.LaunchdJob {
    var hasRequiredFields: Bool {
        path?.isEmpty == false
            && program?.isEmpty == false
            && !arguments.isEmpty
            && state?.isEmpty == false
    }

    func agrees(with plist: LegacyJournalServiceRetirer.ServicePlist, plistURL: URL) -> Bool {
        guard let path,
              let program,
              !arguments.isEmpty else {
            return false
        }
        if canonicalPath(URL(fileURLWithPath: path)) != canonicalPath(plistURL) {
            return false
        }
        if canonicalPath(URL(fileURLWithPath: program)) != canonicalPath(URL(fileURLWithPath: plist.programArguments[0])) {
            return false
        }
        return arguments == plist.programArguments
    }
}

private final class LockedLegacyServiceOutput: @unchecked Sendable {
    private let lock = NSLock()
    private var stdout = Data()
    private var stderr = Data()

    func appendStdout(_ chunk: Data) {
        lock.withLock { stdout.append(chunk) }
    }

    func appendStderr(_ chunk: Data) {
        lock.withLock { stderr.append(chunk) }
    }

    func stdoutString() -> String {
        lock.withLock { String(data: stdout, encoding: .utf8) ?? "" }
    }

    func stderrString() -> String {
        lock.withLock { String(data: stderr, encoding: .utf8) ?? "" }
    }
}
