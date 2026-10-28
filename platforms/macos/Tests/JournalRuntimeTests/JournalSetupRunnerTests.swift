// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import Foundation
import JournalRuntimeTestSupport
import SolstoneCore
import Testing
@testable import JournalRuntime

@Suite("JournalSetupRunnerTests")
struct JournalSetupRunnerTests {
    @Test func productionDefaultUsesOnlyNativeRuntimeMaterializer() {
        #expect(JournalSetupRunner().usesNativeRuntimeMaterializer)
    }

    @Test func nativeRuntimeDoesNotCreateDirectoriesInsideTheAppBundle() async throws {
        let workspace = try makeTemporaryDirectory()
        defer { try? FileManager.default.removeItem(at: workspace) }
        let runtimeRoot = try makeNativeRuntimeBundle(in: workspace)
        let subprocess = FakeSubprocessRunner()
        subprocess.enqueue("setup", .success(stdout: Data(Self.okSetupJSONL.utf8)))
        let runner = JournalSetupRunner(
            subprocessRunner: subprocess,
            gate: MockSingleSupervisorGate(),
            materializer: NativeJournalRuntimeMaterializer(
                bundleURL: workspace.appendingPathComponent("Journal.app", isDirectory: true),
                environment: [:]
            )
        )

        _ = try await runner.run(journalRoot: workspace.appendingPathComponent("journal", isDirectory: true))

        let layout = SolstoneRuntimeLayout(rootURL: runtimeRoot)
        #expect(!FileManager.default.fileExists(atPath: runtimeRoot.appendingPathComponent("python").path))
        #expect(!FileManager.default.fileExists(atPath: runtimeRoot.appendingPathComponent("cache").path))
        #expect(!FileManager.default.fileExists(atPath: runtimeRoot.appendingPathComponent("tools").path))
        let setup = try #require(subprocess.invocations.first { $0.arguments.count >= 2 && $0.arguments[0] == "journal" && $0.arguments[1] == "setup" })
        #expect(setup.executable == layout.solstoneBinary)
    }

    @Test(arguments: ["ambient", "empty", "missing"])
    func nativeSetupResolvesBundledCommandsBeforeAmbientInstallation(pathMode: String) async throws {
        let workspace = try makeTemporaryDirectory()
        defer { try? FileManager.default.removeItem(at: workspace) }
        let runtimeRoot = try makeNativeRuntimeBundle(in: workspace)
        let ambientBin = workspace.appendingPathComponent("other installation/bin", isDirectory: true)
        try FileManager.default.createDirectory(at: ambientBin, withIntermediateDirectories: true)
        for name in ["solstone"] {
            for (directory, status) in [(runtimeRoot.appendingPathComponent("bin"), 0), (ambientBin, 91)] {
                let executable = directory.appendingPathComponent(name)
                try Data("#!/bin/sh\nexit \(status)\n".utf8).write(to: executable)
                try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: executable.path)
            }
        }
        let ambientPath = ambientBin.path + ":/usr/bin:/bin"
        var environment = ["HOME": workspace.path, "SETUP_TEST_SENTINEL": "preserved"]
        if pathMode != "missing" {
            environment["PATH"] = pathMode == "empty" ? "" : ambientPath
        }
        let subprocess = FakeSubprocessRunner()
        subprocess.enqueue("setup", .success(stdout: Data(Self.okSetupJSONL.utf8)))
        let runner = JournalSetupRunner(
            subprocessRunner: subprocess,
            gate: MockSingleSupervisorGate(),
            materializer: NativeJournalRuntimeMaterializer(
                bundleURL: workspace.appendingPathComponent("Journal.app", isDirectory: true),
                environment: environment
            )
        )
        let journalRoot = workspace.appendingPathComponent("journal", isDirectory: true)
        _ = try await runner.run(journalRoot: journalRoot)
        let layout = SolstoneRuntimeLayout(rootURL: runtimeRoot)
        let invocation = try #require(subprocess.invocations.first { $0.arguments.count >= 2 && $0.arguments[0] == "journal" && $0.arguments[1] == "setup" })
        #expect(invocation.executable == layout.solstoneBinary)
        #expect(invocation.arguments == ["journal"] + JournalSetupCommand.setupArguments(journalURL: journalRoot, skipService: true))
        let setupEnvironment = try #require(invocation.environment)
        #expect(setupEnvironment["HOME"] == workspace.path)
        #expect(setupEnvironment["SETUP_TEST_SENTINEL"] == "preserved")
        let processRunner = SubprocessRunner()
        for command in ["solstone"] {
            let control = try await processRunner.run(
                executable: URL(fileURLWithPath: "/usr/bin/env"), arguments: [command],
                environment: ["PATH": ambientPath], timeout: .seconds(5),
                stdoutHandler: { _ in }, stderrHandler: { _ in }
            )
            #expect(control.exitCode == 91)
            let bundled = try await processRunner.run(
                executable: URL(fileURLWithPath: "/usr/bin/env"), arguments: [command],
                environment: setupEnvironment, timeout: .seconds(5),
                stdoutHandler: { _ in }, stderrHandler: { _ in }
            )
            #expect(bundled.exitCode == 0)
        }
        let hostTool = try await processRunner.run(
            executable: URL(fileURLWithPath: "/usr/bin/env"), arguments: ["true"],
            environment: setupEnvironment, timeout: .seconds(5),
            stdoutHandler: { _ in }, stderrHandler: { _ in }
        )
        #expect(hostTool.exitCode == 0)
    }

    @Test func setupUsesExactSharedArgumentsIncludingSkipWrapper() async throws {
        let runtime = try makeRuntime()
        defer { try? FileManager.default.removeItem(at: runtime.layout.rootURL) }
        let subprocess = FakeSubprocessRunner()
        subprocess.enqueue("setup", .success(stdout: Data(Self.okSetupJSONL.utf8)))
        let runner = JournalSetupRunner(
            subprocessRunner: subprocess,
            gate: MockSingleSupervisorGate(),
            materializer: MockRuntimeMaterializer(result: .success(runtime))
        )
        let journalRoot = try makeTemporaryDirectory()
        defer { try? FileManager.default.removeItem(at: journalRoot) }

        _ = try await runner.run(journalRoot: journalRoot, skipService: true)

        let invocation = try #require(subprocess.invocations.first { $0.arguments.count >= 2 && $0.arguments[0] == "journal" && $0.arguments[1] == "setup" })
        #expect(invocation.executable == runtime.layout.solstoneBinary)
        #expect(invocation.arguments == [
            "journal",
            "setup",
            "--jsonl",
            "--yes",
            "--skip-models",
            "--skip-wrapper",
            "--accept-existing-journal",
            "--journal",
            journalRoot.standardizedFileURL.path,
            "--skip-service",
        ])
        #expect(invocation.timeout == .seconds(180))
        #expect(JournalSetupCommand.setupArguments(journalURL: journalRoot, skipService: false) == [
            "setup",
            "--jsonl",
            "--yes",
            "--skip-models",
            "--skip-wrapper",
            "--accept-existing-journal",
            "--journal",
            journalRoot.path,
        ])
    }

    @Test func gateBlockedShortCircuitsMaterializeAndRun() async throws {
        let runtime = try makeRuntime()
        defer { try? FileManager.default.removeItem(at: runtime.layout.rootURL) }
        let subprocess = FakeSubprocessRunner()
        let materializer = MockRuntimeMaterializer(result: .success(runtime))
        let diagnostic = JournalDiagnostic(commandLabel: "gate", outputExcerpt: "busy")
        let blockage = SingleSupervisorGateBlockage.portConflict(diagnostic)
        let runner = JournalSetupRunner(
            subprocessRunner: subprocess,
            gate: MockSingleSupervisorGate(result: .blocked(blockage)),
            materializer: materializer
        )

        do {
            _ = try await runner.run(journalRoot: try makeTemporaryDirectory())
            Issue.record("expected gateBlocked")
        } catch let error as JournalSetupRunnerError {
            #expect(error == .gateBlocked(blockage))
        }

        #expect(materializer.materializeCalls == 0)
        #expect(subprocess.invocations.isEmpty)
    }

    @Test func materializeFailureMapsToTypedError() async throws {
        let subprocess = FakeSubprocessRunner()
        let runner = JournalSetupRunner(
            subprocessRunner: subprocess,
            gate: MockSingleSupervisorGate(),
            materializer: MockRuntimeMaterializer(result: .failure(TestError(message: "materialize boom")))
        )

        do {
            _ = try await runner.run(journalRoot: try makeTemporaryDirectory())
            Issue.record("expected materializeFailed")
        } catch let error as JournalSetupRunnerError {
            #expect(error == .materializeFailed(message: "materialize boom"))
        }

        #expect(subprocess.invocations.isEmpty)
    }

    @Test func successfulJSONLEmitsOrderedProgressAndReturnsResult() async throws {
        let runtime = try makeRuntime()
        defer { try? FileManager.default.removeItem(at: runtime.layout.rootURL) }
        let subprocess = FakeSubprocessRunner()
        subprocess.enqueue("setup", .success(stdout: Data(Self.progressSetupJSONL.utf8)))
        let events = EventRecorder()
        let runner = JournalSetupRunner(
            subprocessRunner: subprocess,
            gate: MockSingleSupervisorGate(),
            materializer: MockRuntimeMaterializer(result: .success(runtime))
        )

        let result = try await runner.run(journalRoot: try makeTemporaryDirectory()) { event in
            await events.append(event)
        }

        #expect(await events.snapshot() == [
            .stepStarted(step: "doctor", index: 1, total: 2),
            .warning(step: "doctor", text: "port looks busy", fixHint: "try again"),
            .completed(status: "ok"),
        ])
        #expect(result.runtime.key == runtime.key)
        #expect(result.stdoutTail.contains(#""setup.completed""#))
        #expect(result.renderedLog.contains("setup ok"))
    }

    @Test func optionalInstallationIsAwaitedBoundedAndRemainsBestEffort() async throws {
        let runtime = try makeRuntime()
        defer { try? FileManager.default.removeItem(at: runtime.layout.rootURL) }
        let journalRoot = try makeTemporaryDirectory()
        defer { try? FileManager.default.removeItem(at: journalRoot) }
        let marker = journalRoot.appendingPathComponent("models-finished")
        let subprocess = FakeSubprocessRunner()
        subprocess.enqueue("setup", .success(stdout: Data(Self.okSetupJSONL.utf8)))
        subprocess.enqueue("install-models", .success(exitCode: 65, delay: .milliseconds(50), sideEffect: {
            FileManager.default.createFile(atPath: marker.path, contents: Data())
        }))
        let runner = JournalSetupRunner(
            subprocessRunner: subprocess, gate: MockSingleSupervisorGate(),
            materializer: MockRuntimeMaterializer(result: .success(runtime)), setupTimeout: .seconds(3)
        )
        _ = try await runner.run(journalRoot: journalRoot)
        #expect(FileManager.default.fileExists(atPath: marker.path))
        let install = try #require(subprocess.invocations.last)
        #expect(install.executable == runtime.layout.solstoneBinary)
        #expect(install.arguments == ["journal", "install-models"])
        #expect(install.timeout == .seconds(3))
        #expect(install.environment?["SOLSTONE_JOURNAL"] == nil)
        #expect(install.environment?["PATH"]?.hasPrefix(runtime.layout.binDir.path + ":") == true)
    }

    @Test func cancellationDuringOptionalInstallationDoesNotReturnSetupSuccess() async throws {
        let runtime = try makeRuntime()
        defer { try? FileManager.default.removeItem(at: runtime.layout.rootURL) }
        let journalRoot = try makeTemporaryDirectory()
        defer { try? FileManager.default.removeItem(at: journalRoot) }
        let subprocess = FakeSubprocessRunner()
        subprocess.enqueue("setup", .success(stdout: Data(Self.okSetupJSONL.utf8)))
        subprocess.enqueue("install-models", .success(delay: .seconds(3)))
        let runner = JournalSetupRunner(
            subprocessRunner: subprocess, gate: MockSingleSupervisorGate(),
            materializer: MockRuntimeMaterializer(result: .success(runtime))
        )
        let task = Task { try await runner.run(journalRoot: journalRoot) }
        let deadline = ContinuousClock.now.advanced(by: .seconds(2))
        while !subprocess.invocations.contains(where: { $0.arguments == ["journal", "install-models"] }), ContinuousClock.now < deadline {
            await Task.yield()
        }
        #expect(subprocess.invocations.contains(where: { $0.arguments == ["journal", "install-models"] }))
        task.cancel()
        await #expect(throws: CancellationError.self) { try await task.value }
    }

    @Test func timeoutFailsWithTypedTimeout() async throws {
        try await expectSetupError(
            response: .success(delay: .milliseconds(5)),
            setupTimeout: .milliseconds(5),
            expected: .timedOut(timeout: .milliseconds(5))
        )
    }

    @Test func duplicateCompletionFails() async throws {
        try await expectSetupError(
            response: .success(stdout: Data("""
            {"event":"setup.completed","status":"ok"}
            {"event":"setup.completed","status":"ok"}

            """.utf8)),
            expected: .duplicateCompletion
        )
    }

    @Test func nonzeroExitFailsWithStderrMessage() async throws {
        try await expectSetupError(
            response: .success(
                stdout: Data(Self.okSetupJSONL.utf8),
                stderr: Data("journal failed\n".utf8),
                exitCode: 2
            ),
            expected: .setupFailed(errorCode: nil, message: "journal failed")
        )
    }

    @Test func stepFailureFailsWithStepError() async throws {
        try await expectSetupError(
            response: .success(stdout: Data("""
            {"event":"step.failed","step":"doctor","error":{"code":"port_in_use_non_interactive","message":"port failed","details":"","exit_code":2}}
            {"event":"setup.completed","status":"ok"}

            """.utf8)),
            expected: .setupFailed(errorCode: "port_in_use_non_interactive", message: "port failed")
        )
    }

    @Test func failedSetupCompletedFails() async throws {
        try await expectSetupError(
            response: .success(stdout: Data("""
            {"event":"setup.completed","status":"failed","failed_step":"doctor"}

            """.utf8)),
            expected: .setupFailed(errorCode: nil, message: "setup failed at doctor")
        )
    }

    @Test func missingSetupCompletedFails() async throws {
        try await expectSetupError(
            response: .success(stdout: Data(#"{"event":"step.started","step":"doctor","index":1,"total":2}"#.utf8)),
            expected: .missingCompletion
        )
    }

    @Test func optionalModelTimeoutIsBoundedWithoutFailingSuccessfulSetup() async throws {
        let runtime = try makeRuntime()
        defer { try? FileManager.default.removeItem(at: runtime.layout.rootURL) }
        let subprocess = FakeSubprocessRunner()
        subprocess.enqueue("setup", .success(stdout: Data(Self.okSetupJSONL.utf8)))
        subprocess.enqueue("install-models", .success(delay: .seconds(2)))
        let runner = JournalSetupRunner(
            subprocessRunner: subprocess,
            gate: MockSingleSupervisorGate(),
            materializer: MockRuntimeMaterializer(result: .success(runtime)),
            setupTimeout: .milliseconds(50)
        )
        let start = ContinuousClock.now

        _ = try await runner.run(journalRoot: try makeTemporaryDirectory())
        let elapsed = start.duration(to: .now)

        #expect(elapsed < .seconds(1))
        let sawInstallModels = await waitForInvocation(subprocess) { invocation in
            invocation.arguments == ["journal", "install-models"]
        }
        #expect(sawInstallModels)
    }

    @Test func skipModelInstallDoesNotRecordInstallModels() async throws {
        let runtime = try makeRuntime()
        defer { try? FileManager.default.removeItem(at: runtime.layout.rootURL) }
        let subprocess = FakeSubprocessRunner()
        subprocess.enqueue("setup", .success(stdout: Data(Self.okSetupJSONL.utf8)))
        let runner = JournalSetupRunner(
            subprocessRunner: subprocess,
            gate: MockSingleSupervisorGate(),
            materializer: MockRuntimeMaterializer(result: .success(runtime))
        )

        _ = try await runner.run(
            journalRoot: try makeTemporaryDirectory(),
            skipModelInstall: true
        )

        #expect(subprocess.invocations.contains { $0.arguments.first == "journal" && $0.arguments.dropFirst().first == "setup" })
        #expect(!subprocess.invocations.contains { $0.arguments == ["journal", "install-models"] })
    }

    private static let okSetupJSONL = """
    {"event":"setup.completed","status":"ok","duration_ms":10}

    """

    private static let progressSetupJSONL = """
    {"event":"step.started","step":"doctor","index":1,"total":2}
    {"event":"step.warning","step":"doctor","text":"port looks busy","fix_hint":"try again"}
    {"event":"setup.completed","status":"ok","duration_ms":10}

    """

    private func expectSetupError(
        response: FakeSubprocessRunner.Response,
        setupTimeout: Duration = .seconds(180),
        expected: JournalSetupRunnerError
    ) async throws {
        let runtime = try makeRuntime()
        defer { try? FileManager.default.removeItem(at: runtime.layout.rootURL) }
        let subprocess = FakeSubprocessRunner()
        subprocess.enqueue("setup", response)
        let runner = JournalSetupRunner(
            subprocessRunner: subprocess,
            gate: MockSingleSupervisorGate(),
            materializer: MockRuntimeMaterializer(result: .success(runtime)),
            setupTimeout: setupTimeout
        )

        do {
            _ = try await runner.run(journalRoot: try makeTemporaryDirectory())
            Issue.record("expected \(expected)")
        } catch let error as JournalSetupRunnerError {
            #expect(error == expected)
        }
    }

    private func waitForInvocation(
        _ runner: FakeSubprocessRunner,
        timeout: Duration = .seconds(1),
        matching predicate: (SubprocessInvocation) -> Bool
    ) async -> Bool {
        let deadline = ContinuousClock.now.advanced(by: timeout)
        while ContinuousClock.now < deadline {
            if runner.invocations.contains(where: predicate) {
                return true
            }
            try? await Task.sleep(for: .milliseconds(10))
        }
        return runner.invocations.contains(where: predicate)
    }

    private func makeTemporaryDirectory() throws -> URL {
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("journal-setup-runner-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        return url
    }

    private func makeNativeRuntimeBundle(in workspace: URL) throws -> URL {
        let bundleURL = workspace.appendingPathComponent("Journal.app", isDirectory: true)
        let contentsURL = bundleURL.appendingPathComponent("Contents", isDirectory: true)
        let runtimeRoot = contentsURL.appendingPathComponent("Resources/solstone-runtime", isDirectory: true)
        let binURL = runtimeRoot.appendingPathComponent("bin", isDirectory: true)
        try FileManager.default.createDirectory(at: binURL, withIntermediateDirectories: true)
        let info: [String: String] = [
            "CFBundleIdentifier": "app.solpbc.journal.tests",
            "CFBundleShortVersionString": "0.1.0",
            "CFBundleVersion": "1",
        ]
        let infoData = try PropertyListSerialization.data(fromPropertyList: info, format: .xml, options: 0)
        try infoData.write(to: contentsURL.appendingPathComponent("Info.plist"))
        for name in ["solstone"] {
            let executable = binURL.appendingPathComponent(name)
            try Data("#!/bin/sh\\nexit 0\\n".utf8).write(to: executable)
            try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: executable.path)
        }
        return runtimeRoot
    }
}

private actor EventRecorder {
    private var events: [JournalSetupProgressEvent] = []

    func append(_ event: JournalSetupProgressEvent) {
        events.append(event)
    }

    func snapshot() -> [JournalSetupProgressEvent] {
        events
    }
}

private struct TestError: LocalizedError {
    let message: String

    var errorDescription: String? { message }
}
