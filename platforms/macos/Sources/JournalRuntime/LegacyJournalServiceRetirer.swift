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
    // exited. launchd gives a job 20 seconds (its default ExitTimeOut) before
    // killing it, and the journal's own shutdown is budgeted inside that, so the
    // wait for the job to go has to clear it.
    private let absenceTimeout: Duration
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
        case .retire(let verdict, let loaded):
            Logger.setup.notice("journal-lifecycle: legacy-service verdict=\(verdict.rawValue, privacy: .public) action=retire")
            if loaded {
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
                return .blocked(.inspectionFailure, diagnostic("legacy service plist could not be removed"))
            }
        case .block(let verdict, let message):
            Logger.setup.warning("journal-lifecycle: legacy-service verdict=\(verdict.rawValue, privacy: .public) action=block")
            return .blocked(verdict, diagnostic(message))
        }
    }

    private enum RetirementPlan {
        case noop(LegacyJournalServiceVerdict)
        case retire(LegacyJournalServiceVerdict, loaded: Bool)
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

        let label: String
        let programArguments: [String]
        // Written by the earlier service install, which logged to
        // <journal>/health/service.log.
        let standardOutPath: String?
        let standardErrorPath: String?
        // The current service install writes no log paths. It names its
        // journal in this environment value: the path's bytes, lower-case hex.
        let journalToken: String?
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
                ? .retire(.provenMatchUnloaded, loaded: false)
                : .retire(.provenMatchLoaded, loaded: true)
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
        let tokenLookup: String??
        if let environment = plist["EnvironmentVariables"] {
            guard let environment = environment as? [String: Any] else { return nil }
            tokenLookup = optionalString(environment[ServicePlist.journalTokenKey])
        } else {
            tokenLookup = .some(nil)
        }
        guard let journalToken = tokenLookup else { return nil }
        return ServicePlist(
            label: label,
            programArguments: arguments,
            standardOutPath: stdout,
            standardErrorPath: stderr,
            journalToken: journalToken
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
    var hasLegacyJournalShape: Bool {
        guard label == LegacyJournalServiceRetirer.label else { return false }
        if programArguments.count >= 3
            && URL(fileURLWithPath: programArguments[0]).lastPathComponent == "journal"
            && programArguments[1] == "start"
            && programArguments[2] == "5015" {
            return true
        }
        if programArguments.count >= 4
            && URL(fileURLWithPath: programArguments[0]).lastPathComponent == "solstone"
            && programArguments[1] == "journal"
            && programArguments[2] == "start"
            && programArguments[3] == "5015" {
            return true
        }
        return false
    }

    func rootMatch(journalRoot: URL) -> RootMatch {
        if let journalToken {
            guard let tokenPath = decodeJournalToken(journalToken) else {
                return .malformed
            }
            let actualPath = canonicalPath(URL(fileURLWithPath: tokenPath, isDirectory: true))
            return actualPath == canonicalPath(journalRoot) ? .same : .different
        }
        guard let standardOutPath,
              standardOutPath == standardErrorPath,
              standardOutPath.hasPrefix("/") else {
            return .malformed
        }
        let expected = journalRoot
            .appendingPathComponent("health/service.log")
        let actualPath = canonicalPath(URL(fileURLWithPath: standardOutPath))
        let expectedPath = canonicalPath(expected)
        return actualPath == expectedPath ? .same : .different
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
