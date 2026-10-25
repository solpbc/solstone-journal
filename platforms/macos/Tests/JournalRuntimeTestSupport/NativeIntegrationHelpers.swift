// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import Darwin
import Foundation
import JournalRuntime

/// Shared helpers for the opt-in native runtime integration tier, usable from every test
/// target. See `NativeRuntimeFixture` for the isolation contract and `make integration-native`
/// for the entry point.
public enum NativeIntegrationHelpers {
    /// Exit code the native `install-models --required-only --check` uses for a missing,
    /// invalid or mismatched RF-DETR sidecar (`sysexits` `EX_DATAERR`).
    public static let requiredModelsCheckDataError: Int32 = 65

    /// Relative path of the RF-DETR install sidecar under a journal root.
    public static let rfdetrSidecarRelativePath = "cache/providers/rfdetr/.rfdetr-install.json"

    public static let requiredModelsCheckArguments = ["install-models", "--required-only", "--check"]

    /// Run the app's exact native setup argv (the same builder production uses) against the
    /// fixture's journal root, honouring the fixture's isolated home. Setup runs from the home,
    /// not from inside the journal, because the app's own setup runner never sets a working
    /// directory either.
    public static func runAppSetup(_ fixture: NativeRuntimeFixture) async throws -> NativeCommandResult {
        try await fixture.run(
            JournalSetupCommand.setupArguments(journalURL: fixture.journalRoot, skipService: true),
            timeout: .seconds(180),
            currentDirectory: fixture.home
        )
    }

    /// The native check the app's reconciler is expected to satisfy.
    public static func requiredModelsCheck(_ fixture: NativeRuntimeFixture) async throws -> NativeCommandResult {
        try await fixture.run(requiredModelsCheckArguments, timeout: .seconds(60))
    }

    public static func sidecarURL(_ fixture: NativeRuntimeFixture) -> URL {
        fixture.journalRoot.appendingPathComponent(rfdetrSidecarRelativePath)
    }

    /// Rewrite the sidecar's recorded engine digest so the cache reads as a previous runtime's:
    /// the state a Journal upgrade leaves behind.
    public static func staleSidecar(_ fixture: NativeRuntimeFixture) throws {
        let url = sidecarURL(fixture)
        let data = try Data(contentsOf: url)
        guard var object = try JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            throw NativeIntegrationHelperError.sidecarUnreadable(url.path)
        }
        object["engine_sha256"] = String(repeating: "0", count: 64)
        let rewritten = try JSONSerialization.data(withJSONObject: object, options: [.prettyPrinted, .sortedKeys])
        try rewritten.write(to: url, options: .atomic)
    }

    /// Write the paired-device door port both sides read from `config/journal.json`
    /// (`pairing.direct_port`), preserving every other key native setup wrote.
    public static func configureDirectDoorPort(_ fixture: NativeRuntimeFixture, port: Int) throws {
        let url = fixture.journalRoot
            .appendingPathComponent("config", isDirectory: true)
            .appendingPathComponent("journal.json")
        var object: [String: Any] = [:]
        if let data = try? Data(contentsOf: url),
           let existing = try? JSONSerialization.jsonObject(with: data) as? [String: Any] {
            object = existing
        }
        var pairing = object["pairing"] as? [String: Any] ?? [:]
        pairing["direct_port"] = port
        object["pairing"] = pairing
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        let data = try JSONSerialization.data(withJSONObject: object, options: [.prettyPrinted, .sortedKeys])
        try data.write(to: url, options: .atomic)
    }

    /// The supervisor's record of the paired-device door: `{state, port}` under `health/`.
    public struct DirectDoorRecord: Sendable, Equatable {
        public let state: String
        public let port: Int
    }

    public static func readDirectDoorRecord(_ fixture: NativeRuntimeFixture) -> DirectDoorRecord? {
        let url = fixture.journalRoot
            .appendingPathComponent("health", isDirectory: true)
            .appendingPathComponent("direct-door.json")
        guard let data = try? Data(contentsOf: url),
              let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let state = object["state"] as? String,
              let port = object["port"] as? Int else {
            return nil
        }
        return DirectDoorRecord(state: state, port: port)
    }

    /// A receipt context that records nothing: no app identity means no payload receipts are
    /// drafted, which keeps this tier from inventing an app bundle it does not have.
    public static func silentReceiptContext() -> JournalRuntimeEntryReceiptContext {
        JournalRuntimeEntryReceiptContext(
            attemptID: JournalRuntimeEntryAttemptID(),
            sink: NullReceiptSink(),
            appIdentity: nil,
            candidateProvenance: nil
        )
    }

    public static func processExists(_ pid: pid_t) -> Bool {
        if Darwin.kill(pid, 0) == 0 { return true }
        return errno == EPERM
    }

    /// Poll until `condition` holds or the deadline passes. Returns whether it held.
    public static func waitUntil(
        timeout: Duration,
        pollEvery interval: Duration = .milliseconds(50),
        _ condition: @Sendable () async -> Bool
    ) async -> Bool {
        let deadline = ContinuousClock.now.advanced(by: timeout)
        while ContinuousClock.now < deadline {
            if await condition() { return true }
            try? await Task.sleep(for: interval)
        }
        return await condition()
    }

    /// Best-effort teardown for processes a test admitted and then lost track of. Signals
    /// only the exact pids it is given, never a group or a name.
    public static func killSurvivors(_ pids: [pid_t]) {
        for pid in pids where processExists(pid) {
            _ = Darwin.kill(pid, SIGKILL)
        }
    }

    /// True when a TCP connect to 127.0.0.1:port succeeds.
    public static func loopbackPortAccepts(_ port: Int) -> Bool {
        let descriptor = socket(AF_INET, SOCK_STREAM, 0)
        guard descriptor >= 0 else { return false }
        defer { close(descriptor) }
        var address = sockaddr_in()
        address.sin_family = sa_family_t(AF_INET)
        address.sin_port = UInt16(port).bigEndian
        address.sin_addr.s_addr = inet_addr("127.0.0.1")
        let connected = withUnsafePointer(to: &address) { pointer in
            pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) { sockaddrPointer in
                connect(descriptor, sockaddrPointer, socklen_t(MemoryLayout<sockaddr_in>.size))
            }
        }
        return connected == 0
    }
}

public enum NativeIntegrationHelperError: Error, CustomStringConvertible {
    case sidecarUnreadable(String)

    public var description: String {
        switch self {
        case .sidecarUnreadable(let path):
            return "rf-detr sidecar at \(path) is not a JSON object"
        }
    }
}

/// Never receives a draft in this tier (no app identity), and records nothing if it does.
public struct NullReceiptSink: JournalRuntimeEntryReceiptSinking {
    public init() {}

    public func appendSynchronously(_ draft: JournalRuntimeEntryReceiptDraft) -> JournalRuntimeEntryReceiptWriteResult {
        .recorded
    }
}

public final class RuntimeStatusRecorder: @unchecked Sendable {
    private let lock = NSLock()
    private var statuses: [JournalRuntimeStatus] = []

    public init() {}

    public func append(_ status: JournalRuntimeStatus) {
        lock.withLock { statuses.append(status) }
    }

    public func snapshot() -> [JournalRuntimeStatus] {
        lock.withLock { statuses }
    }
}

/// Pids a test admitted, so teardown can name exactly what it must reap.
public final class AdmittedProcesses: @unchecked Sendable {
    private let lock = NSLock()
    private var pids: Set<pid_t> = []

    public init() {}

    public func record(_ fresh: [pid_t]) {
        lock.withLock { pids.formUnion(fresh) }
    }

    public func all() -> [pid_t] {
        lock.withLock { Array(pids) }
    }
}
