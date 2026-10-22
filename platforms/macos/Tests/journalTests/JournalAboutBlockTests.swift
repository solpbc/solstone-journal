// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import Foundation
import JournalRuntime
import JournalRuntimeTestSupport
import ServiceManagement
import Testing
@testable import journal

@MainActor
@Suite("JournalAboutBlock")
struct JournalAboutBlockTests {
    @Test func stdoutVersionAndBundleBuildRenderBeforeStartAndAfterStop() async throws {
        let fixture = makeFixture()
        defer { fixture.clear() }
        let executableURL = URL(fileURLWithPath: "/var/tmp/Journal.app/Contents/MacOS/journal")
        let commandURL = JournalCommandLine.commandLineURL(executableURL: executableURL)
        let runner = FakeSubprocessRunner()
        runner.enqueue("--version", .success(stdout: Data("journal 9.8.7 build wrapper-short-is-2.0.29\n".utf8)))
        runner.enqueue("--version", .success(stdout: Data("journal 9.8.8\n".utf8)))
        let materializer = MockRuntimeMaterializer(result: .failure(TestFailure.requested))
        let child = MockSupervisedChildRunner()
        let supervisor = JournalSupervisor(
            gate: MockSingleSupervisorGate(),
            materializer: materializer,
            runner: child,
            readinessGate: MockJournalReadinessGate(result: .ready)
        )
        let model = makeModel(
            config: fixture.config,
            supervisor: supervisor,
            executableURL: executableURL,
            runner: runner,
            appBuild: "67"
        )

        await model.refreshRunState()
        #expect(model.healthDisplay == .unknown)
        #expect(model.journalVersion == "9.8.7")
        #expect(model.aboutBlock == "journal 9.8.7 (67) · macos 15.6 · arm64")
        #expect(runner.invocations.last?.executable == commandURL)
        #expect(runner.invocations.last?.arguments == ["journal", "--version"])

        #expect(await supervisor.stop())
        await model.refreshRunState()
        #expect(model.healthDisplay == .unknown)
        #expect(model.journalVersion == "9.8.8")
        #expect(model.aboutBlock == "journal 9.8.8 (67) · macos 15.6 · arm64")
        #expect(runner.invocations.count == 2)
        #expect(runner.invocations.allSatisfy { $0.executable == commandURL && $0.arguments == ["journal", "--version"] })
        #expect(materializer.materializeCalls == 0)
        #expect(child.startCalls == 0)
    }

    @Test func failedProbeDoesNotMaterializeOrStartAndFailedArchIsOmitted() async throws {
        let fixture = makeFixture()
        defer { fixture.clear() }
        let executableURL = URL(fileURLWithPath: "/var/tmp/Journal.app/Contents/MacOS/journal")
        let materializer = MockRuntimeMaterializer(result: .failure(TestFailure.requested))
        let child = MockSupervisedChildRunner()
        let supervisor = JournalSupervisor(
            gate: MockSingleSupervisorGate(),
            materializer: materializer,
            runner: child,
            readinessGate: MockJournalReadinessGate(result: .ready)
        )
        let model = JournalWindowModel(
            config: fixture.config,
            supervisor: supervisor,
            fetchVersion: { _, _ in nil },
            versionExecutableURL: { executableURL },
            aboutOSVersion: { "26.0" },
            aboutArch: { nil },
            appBuild: "67"
        )

        await model.refreshRunState()
        #expect(model.journalVersion == "unknown")
        #expect(model.aboutBlock == "journal unknown")
        #expect(materializer.materializeCalls == 0)
        #expect(child.startCalls == 0)
    }

    @Test func missingOrEmptyBundleBuildIsOmitted() async throws {
        for appBuild in [nil, ""] as [String?] {
            let fixture = makeFixture()
            defer { fixture.clear() }
            let executableURL = URL(fileURLWithPath: "/var/tmp/Journal.app/Contents/MacOS/journal")
            let runner = FakeSubprocessRunner()
            runner.enqueue("--version", .success(stdout: Data("journal 9.8.7\n".utf8)))
            let supervisor = JournalSupervisor(
                gate: MockSingleSupervisorGate(),
                materializer: MockRuntimeMaterializer(result: .failure(TestFailure.requested)),
                runner: MockSupervisedChildRunner(),
                readinessGate: MockJournalReadinessGate(result: .ready)
            )
            let model = makeModel(
                config: fixture.config,
                supervisor: supervisor,
                executableURL: executableURL,
                runner: runner,
                appBuild: appBuild
            )

            await model.refreshRunState()

            #expect(model.aboutBlock == "journal 9.8.7 · macos 15.6 · arm64")
        }
    }

    private func makeModel(
        config: JournalAppConfig,
        supervisor: JournalSupervisor,
        executableURL: URL,
        runner: FakeSubprocessRunner,
        appBuild: String?
    ) -> JournalWindowModel {
        JournalWindowModel(
            config: config,
            supervisor: supervisor,
            fetchVersion: { binary, environment in
                await JournalHealthCheck.version(journalBinary: binary, runner: runner, environment: environment)
            },
            versionExecutableURL: { executableURL },
            aboutOSVersion: { "15.6" },
            aboutArch: { "arm64" },
            appBuild: appBuild
        )
    }

    private func makeFixture() -> Fixture {
        let suiteName = "journal.about-block.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suiteName)!
        let config = JournalAppConfig(defaults: defaults, loginItemManager: TestLoginItems())
        return Fixture(suiteName: suiteName, defaults: defaults, config: config)
    }
}

@MainActor
private final class TestLoginItems: LoginItemManaging {
    var status: SMAppService.Status = .notRegistered

    func register() throws {}
    func unregister() throws {}
    func unregisterAwaitingCompletion() async throws {}
}

private struct Fixture {
    let suiteName: String
    let defaults: UserDefaults
    let config: JournalAppConfig

    @MainActor
    func clear() {
        defaults.removePersistentDomain(forName: suiteName)
    }
}

private enum TestFailure: Error {
    case requested
}
