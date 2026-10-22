// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import Foundation
import JournalRuntime

/// Result of one native `journal` command run by the integration fixture.
public struct NativeCommandResult: Sendable {
    public let exitCode: Int32
    public let terminationReason: Process.TerminationReason
    public let stdout: String
    public let stderr: String
    public let elapsed: Duration

    public var succeeded: Bool {
        terminationReason == .exit && exitCode == 0
    }
}

public enum NativeRuntimeFixtureError: LocalizedError {
    case runtimeMissing(String)

    public var errorDescription: String? {
        switch self {
        case .runtimeMissing(let path):
            return """
            native journal runtime is missing at \(path); stage the accepted archive with \
            `make journal-native-runtime-accepted ...` or point \
            \(NativeRuntimeFixture.runtimeDirectoryEnvironmentKey) at a runtime tree that contains bin/solstone
            """
        }
    }
}

/// Runs the app's real Journal-runtime code against the staged accepted native `journal`
/// binary inside an isolated home. Every instance owns a fresh temporary `HOME`, journal root
/// and scratch directory, so concurrent runs on one host never share files, and no test here
/// reads or writes the host user's own journal, login items or wrapper commands.
///
/// The suites that use this fixture run only when `SOLSTONE_NATIVE_INTEGRATION=1`; the plain
/// unit gate lists them as skipped. Use `make integration-native`.
public struct NativeRuntimeFixture: Sendable {
    public static let enableEnvironmentKey = "SOLSTONE_NATIVE_INTEGRATION"
    public static let runtimeDirectoryEnvironmentKey = "SOLSTONE_NATIVE_RUNTIME_DIR"
    public static let defaultRuntimeRelativePath = ".build/journal-native-runtime"

    /// True when the caller opted into the native tier for this process.
    public static var isEnabled: Bool {
        ProcessInfo.processInfo.environment[enableEnvironmentKey] == "1"
    }

    /// The package checkout this test binary was compiled from.
    public static var packageRoot: URL {
        URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()   // NativeRuntimeFixture.swift
            .deletingLastPathComponent()   // JournalRuntimeTestSupport
            .deletingLastPathComponent()   // Tests
            .standardizedFileURL
    }

    /// Resolve the runtime tree: the environment override first, then the checkout's staged
    /// accepted runtime. Refuses by name when neither holds an executable `bin/solstone`.
    public static func resolveRuntimeRoot(
        environment: [String: String] = ProcessInfo.processInfo.environment
    ) throws -> URL {
        let candidate: URL
        if let override = environment[runtimeDirectoryEnvironmentKey], !override.isEmpty {
            candidate = URL(fileURLWithPath: override, isDirectory: true).standardizedFileURL
        } else {
            candidate = packageRoot.appendingPathComponent(defaultRuntimeRelativePath, isDirectory: true)
        }
        let layout = SolstoneRuntimeLayout(rootURL: candidate)
        guard FileManager.default.isExecutableFile(atPath: layout.solstoneBinary.path) else {
            throw NativeRuntimeFixtureError.runtimeMissing(layout.solstoneBinary.path)
        }
        return candidate
    }

    public let runtimeRoot: URL
    public let runtime: MaterializedRuntime
    /// Fresh temporary home. Native setup follows `$HOME` for login items and wrapper
    /// commands, so this is what keeps the host user's own files out of reach.
    public let home: URL
    public let journalRoot: URL
    public let scratch: URL

    public init(label: String) throws {
        runtimeRoot = try Self.resolveRuntimeRoot()
        // Keep this short. The supervisor binds `health/callosum.sock` under the journal root,
        // and a Unix socket path is capped at 104 bytes on macOS; a descriptive temporary
        // directory name pushed the root past that and the supervisor refused to boot.
        let suffix = UUID().uuidString.replacingOccurrences(of: "-", with: "").prefix(8)
        let base = URL(fileURLWithPath: "/private/var/tmp", isDirectory: true)
            .appendingPathComponent("sni-\(label)-\(suffix)", isDirectory: true)
        home = base.appendingPathComponent("home", isDirectory: true)
        journalRoot = home.appendingPathComponent("journal", isDirectory: true)
        scratch = base.appendingPathComponent("tmp", isDirectory: true)
        for directory in [home, journalRoot, scratch] {
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        }
        let layout = SolstoneRuntimeLayout(rootURL: runtimeRoot)
        runtime = MaterializedRuntime(
            key: "native-integration-\(runtimeRoot.lastPathComponent)",
            layout: layout,
            environment: Self.isolatedEnvironment(home: home, scratch: scratch, layout: layout)
        )
    }

    /// The environment every native command runs with: the isolated home, a scratch `TMPDIR`,
    /// the runtime's `bin` first on `PATH`, and no inherited journal selection.
    public var environment: [String: String] {
        runtime.environment
    }

    public var journalBinary: URL {
        runtime.layout.solstoneBinary
    }

    private static func isolatedEnvironment(
        home: URL,
        scratch: URL,
        layout: SolstoneRuntimeLayout
    ) -> [String: String] {
        let inherited = ProcessInfo.processInfo.environment
        var environment: [String: String] = [
            "HOME": home.path,
            "TMPDIR": scratch.path,
            "PATH": layout.binDir.path + ":/usr/bin:/bin:/usr/sbin:/sbin",
            "LANG": inherited["LANG"] ?? "en_US.UTF-8",
        ]
        for key in ["USER", "LOGNAME", "SHELL"] {
            if let value = inherited[key] {
                environment[key] = value
            }
        }
        return environment
    }

    /// Run one native `journal` command with the journal root as the working directory, the
    /// isolated environment, and a hard deadline. Never throws on a non-zero exit; callers
    /// assert on the result so a refusal is an observation, not a test crash.
    public func run(
        _ arguments: [String],
        timeout: Duration = .seconds(120),
        currentDirectory: URL? = nil
    ) async throws -> NativeCommandResult {
        let runner = SubprocessRunner(currentDirectoryURL: currentDirectory ?? journalRoot)
        let stdout = LockedText()
        let stderr = LockedText()
        let started = ContinuousClock.now
        let result = try await runner.run(
            executable: journalBinary,
            arguments: ["journal"] + arguments,
            environment: environment,
            timeout: timeout,
            stdoutHandler: { stdout.append($0) },
            stderrHandler: { stderr.append($0) }
        )
        return NativeCommandResult(
            exitCode: result.exitCode,
            terminationReason: result.terminationReason,
            stdout: stdout.string,
            stderr: stderr.string,
            elapsed: started.duration(to: .now)
        )
    }

    /// Paths under the isolated home that native setup must not have created when the app's
    /// own `--skip-wrapper --skip-service` arguments were honoured. Empty means clean.
    public func forbiddenHomeArtifacts() -> [String] {
        let candidates = [
            "Library/LaunchAgents",
            ".local/bin/journal",
            ".local/bin/solstone",
        ]
        return candidates.filter { relative in
            FileManager.default.fileExists(atPath: home.appendingPathComponent(relative).path)
        }
    }

    /// Everything native setup wrote under the isolated home other than the journal itself,
    /// for the record.
    public func homeArtifactsOutsideJournal() -> [String] {
        guard let enumerator = FileManager.default.enumerator(
            at: home,
            includingPropertiesForKeys: [.isRegularFileKey],
            options: []
        ) else { return [] }
        var found: [String] = []
        for case let url as URL in enumerator {
            let path = url.path
            if path.hasPrefix(journalRoot.path) { continue }
            guard (try? url.resourceValues(forKeys: [.isRegularFileKey]))?.isRegularFile == true else { continue }
            found.append(String(path.dropFirst(home.path.count + 1)))
        }
        return found.sorted()
    }

    /// Reserve a currently free loopback TCP port by binding port 0 and reading it back.
    /// The socket is closed before returning, so the caller races only with other processes
    /// choosing ports the same way; the native commands here bind on loopback only.
    public static func reserveLoopbackPort() throws -> Int {
        let socketDescriptor = socket(AF_INET, SOCK_STREAM, 0)
        guard socketDescriptor >= 0 else { throw POSIXError(.init(rawValue: errno) ?? .EIO) }
        defer { close(socketDescriptor) }
        var address = sockaddr_in()
        address.sin_family = sa_family_t(AF_INET)
        address.sin_port = 0
        address.sin_addr.s_addr = inet_addr("127.0.0.1")
        let bound = withUnsafePointer(to: &address) { pointer in
            pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) { sockaddrPointer in
                bind(socketDescriptor, sockaddrPointer, socklen_t(MemoryLayout<sockaddr_in>.size))
            }
        }
        guard bound == 0 else { throw POSIXError(.init(rawValue: errno) ?? .EIO) }
        var assigned = sockaddr_in()
        var length = socklen_t(MemoryLayout<sockaddr_in>.size)
        let read = withUnsafeMutablePointer(to: &assigned) { pointer in
            pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) { sockaddrPointer in
                getsockname(socketDescriptor, sockaddrPointer, &length)
            }
        }
        guard read == 0 else { throw POSIXError(.init(rawValue: errno) ?? .EIO) }
        return Int(UInt16(bigEndian: assigned.sin_port))
    }

    /// Remove the isolated home and scratch tree. Callers stop every process they started
    /// first; this never signals anything.
    public func clear() {
        try? FileManager.default.removeItem(at: home.deletingLastPathComponent())
    }
}

private final class LockedText: @unchecked Sendable {
    private let lock = NSLock()
    private var data = Data()

    func append(_ chunk: Data) {
        lock.withLock { data.append(chunk) }
    }

    var string: String {
        lock.withLock { String(decoding: data, as: UTF8.self) }
    }
}
