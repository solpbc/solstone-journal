// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import Foundation
import JournalRuntimeTestSupport
import SolstoneCore
import Testing
@testable import JournalRuntime

@Suite("LegacyJournalServiceRetirer")
struct LegacyJournalServiceRetirerTests {
    @Test func sameRootLoadedServiceBootsOutPollsAbsentAndUnlinks() async throws {
        let fixture = try ServiceFixture()
        defer { fixture.clear() }
        try fixture.writeSameRootPlist()
        let runner = FakeSubprocessRunner()
        runner.enqueue("print", .success(stdout: Data(loadedLaunchctlOutput(plistPath: fixture.plistURL.path).utf8)))
        runner.enqueue("kill", .success())
        runner.enqueue("print", .success(stdout: Data(stoppedLaunchctlOutput(plistPath: fixture.plistURL.path).utf8)))
        runner.enqueue("bootout", .success())
        runner.enqueue("print", .success(stderr: Data(notFoundLaunchctlError.utf8), exitCode: 113))
        let retirer = LegacyJournalServiceRetirer(
            runner: runner,
            clock: NoopLegacyClock(),
            plistURL: fixture.plistURL,
            uid: 501
        )

        let result = await retirer.retireLegacyService(journalRoot: fixture.sameRoot)

        #expect(result == .success(.provenMatchLoaded))
        #expect(!FileManager.default.fileExists(atPath: fixture.plistURL.path))
        #expect(runner.invocations.map(\.arguments.first) == ["print", "kill", "print", "bootout", "print"])
    }

    @Test func noServiceRequiresNotFoundMarkerNotExitCodeAlone() async throws {
        let fixture = try ServiceFixture()
        defer { fixture.clear() }
        let runner = FakeSubprocessRunner()
        runner.enqueue("print", .success(stderr: Data("launchctl failed\n".utf8), exitCode: 113))
        let retirer = LegacyJournalServiceRetirer(
            runner: runner,
            clock: NoopLegacyClock(),
            plistURL: fixture.plistURL,
            uid: 501
        )

        let result = await retirer.retireLegacyService(journalRoot: fixture.sameRoot)

        guard case .blocked(.inspectionFailure, let diagnostic) = result else {
            Issue.record("expected inspection failure, got \(result)")
            return
        }
        #expect(diagnostic.commandLabel == "legacy journal service")
        #expect(!runner.invocations.contains { $0.arguments.first == "bootout" })
    }

    @Test func launchctlThrowBlocksWithoutBootoutOrUnlink() async throws {
        let fixture = try ServiceFixture()
        defer { fixture.clear() }
        try fixture.writeSameRootPlist()
        let runner = FakeSubprocessRunner()
        runner.enqueue("print", .failure("launchctl failed"))
        let retirer = LegacyJournalServiceRetirer(
            runner: runner,
            clock: NoopLegacyClock(),
            plistURL: fixture.plistURL,
            uid: 501
        )

        let result = await retirer.retireLegacyService(journalRoot: fixture.sameRoot)

        guard case .blocked(.inspectionFailure, _) = result else {
            Issue.record("expected inspection failure, got \(result)")
            return
        }
        #expect(FileManager.default.fileExists(atPath: fixture.plistURL.path))
        #expect(!runner.invocations.contains { $0.arguments.first == "bootout" })
    }

    @Test func launchctlTimeoutBlocksWithoutBootoutOrUnlink() async throws {
        let fixture = try ServiceFixture()
        defer { fixture.clear() }
        try fixture.writeSameRootPlist()
        let runner = FakeSubprocessRunner()
        runner.enqueue("print", .success(delay: .milliseconds(1)))
        let retirer = LegacyJournalServiceRetirer(
            runner: runner,
            clock: NoopLegacyClock(),
            plistURL: fixture.plistURL,
            uid: 501,
            commandTimeout: .milliseconds(1)
        )

        let result = await retirer.retireLegacyService(journalRoot: fixture.sameRoot)

        guard case .blocked(.inspectionFailure, _) = result else {
            Issue.record("expected inspection failure, got \(result)")
            return
        }
        #expect(FileManager.default.fileExists(atPath: fixture.plistURL.path))
        #expect(!runner.invocations.contains { $0.arguments.first == "bootout" })
    }

    @Test func launchctlNonNotFoundNonzeroBlocksWithoutBootoutOrUnlink() async throws {
        let fixture = try ServiceFixture()
        defer { fixture.clear() }
        try fixture.writeSameRootPlist()
        let runner = FakeSubprocessRunner()
        runner.enqueue("print", .success(stderr: Data("launchctl failed\n".utf8), exitCode: 113))
        let retirer = LegacyJournalServiceRetirer(
            runner: runner,
            clock: NoopLegacyClock(),
            plistURL: fixture.plistURL,
            uid: 501
        )

        let result = await retirer.retireLegacyService(journalRoot: fixture.sameRoot)

        guard case .blocked(.inspectionFailure, _) = result else {
            Issue.record("expected inspection failure, got \(result)")
            return
        }
        #expect(FileManager.default.fileExists(atPath: fixture.plistURL.path))
        #expect(!runner.invocations.contains { $0.arguments.first == "bootout" })
    }

    @Test func malformedLoadedLaunchctlBlocksWithoutBootoutOrUnlink() async throws {
        let fixture = try ServiceFixture()
        defer { fixture.clear() }
        try fixture.writeSameRootPlist()
        let runner = FakeSubprocessRunner()
        runner.enqueue("print", .success(stdout: Data("""
        gui/501/org.solpbc.solstone = {
            path = \(fixture.plistURL.path)
            state = running
        }

        """.utf8)))
        let retirer = LegacyJournalServiceRetirer(
            runner: runner,
            clock: NoopLegacyClock(),
            plistURL: fixture.plistURL,
            uid: 501
        )

        let result = await retirer.retireLegacyService(journalRoot: fixture.sameRoot)

        guard case .blocked(.inspectionFailure, _) = result else {
            Issue.record("expected inspection failure, got \(result)")
            return
        }
        #expect(FileManager.default.fileExists(atPath: fixture.plistURL.path))
        #expect(!runner.invocations.contains { $0.arguments.first == "bootout" })
    }

    @Test func loadedButNotRunningStillRetiresOnSameRootMatch() async throws {
        let fixture = try ServiceFixture()
        defer { fixture.clear() }
        try fixture.writeSameRootPlist()
        let runner = FakeSubprocessRunner()
        runner.enqueue("print", .success(stdout: Data("""
        gui/501/org.solpbc.solstone = {
        \tpath = \(fixture.plistURL.path)
        \ttype = LaunchAgent
        \tstate = not running
        \tprogram = /Users/jer/.local/bin/journal
        \targuments = {
        \t\t/Users/jer/.local/bin/journal
        \t\tstart
        \t\t5015
        \t}
        }

        """.utf8)))
        runner.enqueue("bootout", .success())
        runner.enqueue("print", .success(stderr: Data(notFoundLaunchctlError.utf8), exitCode: 113))
        let retirer = LegacyJournalServiceRetirer(
            runner: runner,
            clock: NoopLegacyClock(),
            plistURL: fixture.plistURL,
            uid: 501
        )

        let result = await retirer.retireLegacyService(journalRoot: fixture.sameRoot)

        #expect(result == .success(.provenMatchLoaded))
        #expect(!FileManager.default.fileExists(atPath: fixture.plistURL.path))
        #expect(runner.invocations.map(\.arguments.first) == ["print", "bootout", "print"])
    }

    @Test func notLoadedSameRootPlistIsUnlinked() async throws {
        let fixture = try ServiceFixture()
        defer { fixture.clear() }
        try fixture.writeSameRootPlist()
        let runner = FakeSubprocessRunner()
        runner.enqueue("print", .success(stderr: Data(notFoundLaunchctlError.utf8), exitCode: 113))
        let retirer = LegacyJournalServiceRetirer(
            runner: runner,
            clock: NoopLegacyClock(),
            plistURL: fixture.plistURL,
            uid: 501
        )

        let result = await retirer.retireLegacyService(journalRoot: fixture.sameRoot)

        #expect(result == .success(.provenMatchUnloaded))
        #expect(!FileManager.default.fileExists(atPath: fixture.plistURL.path))
        #expect(!runner.invocations.contains { $0.arguments.first == "bootout" })
    }

    @Test func differentRootBlocksWithoutMutation() async throws {
        let fixture = try ServiceFixture()
        defer { fixture.clear() }
        try fixture.writePlist(replacing: ["/Users/jer/journal": "/Users/jer/other journal"])
        let runner = FakeSubprocessRunner()
        runner.enqueue("print", .success(stderr: Data(notFoundLaunchctlError.utf8), exitCode: 113))
        let retirer = LegacyJournalServiceRetirer(
            runner: runner,
            clock: NoopLegacyClock(),
            plistURL: fixture.plistURL,
            uid: 501
        )

        let result = await retirer.retireLegacyService(journalRoot: fixture.sameRoot)

        guard case .blocked(.differentRoot, _) = result else {
            Issue.record("expected different-root block, got \(result)")
            return
        }
        #expect(FileManager.default.fileExists(atPath: fixture.plistURL.path))
        #expect(!runner.invocations.contains { $0.arguments.first == "bootout" })
    }

    @Test func malformedPlistBlocksWithoutMutation() async throws {
        let fixture = try ServiceFixture()
        defer { fixture.clear() }
        try fixture.writePlist(replacing: [
            "<key>StandardErrorPath</key>\n\t<string>/Users/jer/journal/health/service.log</string>":
                "<key>StandardErrorPath</key>\n\t<string>/Users/jer/other-journal/health/service.log</string>"
        ])
        let runner = FakeSubprocessRunner()
        runner.enqueue("print", .success(stderr: Data(notFoundLaunchctlError.utf8), exitCode: 113))
        let retirer = LegacyJournalServiceRetirer(
            runner: runner,
            clock: NoopLegacyClock(),
            plistURL: fixture.plistURL,
            uid: 501
        )

        let result = await retirer.retireLegacyService(journalRoot: fixture.sameRoot)

        guard case .blocked(.malformed, _) = result else {
            Issue.record("expected malformed block, got \(result)")
            return
        }
        #expect(FileManager.default.fileExists(atPath: fixture.plistURL.path))
        #expect(!runner.invocations.contains { $0.arguments.first == "bootout" })
    }

    @Test func missingRequiredPlistKeyBlocksWithoutBootoutOrUnlink() async throws {
        let fixture = try ServiceFixture()
        defer { fixture.clear() }
        try fixture.writePlist(replacing: [
            "\t<key>StandardOutPath</key>\n\t<string>/Users/jer/journal/health/service.log</string>\n": ""
        ])
        let runner = FakeSubprocessRunner()
        runner.enqueue("print", .success(stderr: Data(notFoundLaunchctlError.utf8), exitCode: 113))
        let retirer = LegacyJournalServiceRetirer(
            runner: runner,
            clock: NoopLegacyClock(),
            plistURL: fixture.plistURL,
            uid: 501
        )

        let result = await retirer.retireLegacyService(journalRoot: fixture.sameRoot)

        guard case .blocked(.malformed, _) = result else {
            Issue.record("expected malformed block, got \(result)")
            return
        }
        #expect(FileManager.default.fileExists(atPath: fixture.plistURL.path))
        #expect(!runner.invocations.contains { $0.arguments.first == "bootout" })
    }

    @Test func labelWithoutPlistBlocksWithoutBootout() async throws {
        let fixture = try ServiceFixture()
        defer { fixture.clear() }
        let runner = FakeSubprocessRunner()
        runner.enqueue("print", .success(stdout: Data(loadedLaunchctlOutput(plistPath: fixture.plistURL.path).utf8)))
        let retirer = LegacyJournalServiceRetirer(
            runner: runner,
            clock: NoopLegacyClock(),
            plistURL: fixture.plistURL,
            uid: 501
        )

        let result = await retirer.retireLegacyService(journalRoot: fixture.sameRoot)

        guard case .blocked(.labelWithoutPlist, _) = result else {
            Issue.record("expected label-without-plist block, got \(result)")
            return
        }
        #expect(!runner.invocations.contains { $0.arguments.first == "bootout" })
    }

    @Test func plistVsJobDisagreementBlocksWithoutBootoutOrUnlink() async throws {
        let fixture = try ServiceFixture()
        defer { fixture.clear() }
        try fixture.writeSameRootPlist()
        let runner = FakeSubprocessRunner()
        runner.enqueue("print", .success(stdout: Data(loadedLaunchctlOutput(plistPath: fixture.root.appendingPathComponent("other.plist").path).utf8)))
        let retirer = LegacyJournalServiceRetirer(
            runner: runner,
            clock: NoopLegacyClock(),
            plistURL: fixture.plistURL,
            uid: 501
        )

        let result = await retirer.retireLegacyService(journalRoot: fixture.sameRoot)

        guard case .blocked(.plistVsJobDisagreement, _) = result else {
            Issue.record("expected plist/job disagreement, got \(result)")
            return
        }
        #expect(FileManager.default.fileExists(atPath: fixture.plistURL.path))
        #expect(!runner.invocations.contains { $0.arguments.first == "bootout" })
    }

    @Test func bootoutNonzeroBlocksWithoutUnlink() async throws {
        let fixture = try ServiceFixture()
        defer { fixture.clear() }
        try fixture.writeSameRootPlist()
        let runner = FakeSubprocessRunner()
        runner.enqueue("print", .success(stdout: Data(loadedLaunchctlOutput(plistPath: fixture.plistURL.path).utf8)))
        runner.enqueue("kill", .success())
        runner.enqueue("print", .success(stdout: Data(stoppedLaunchctlOutput(plistPath: fixture.plistURL.path).utf8)))
        runner.enqueue("bootout", .success(stderr: Data("bootout failed\n".utf8), exitCode: 5))
        let retirer = LegacyJournalServiceRetirer(
            runner: runner,
            clock: NoopLegacyClock(),
            plistURL: fixture.plistURL,
            uid: 501
        )

        let result = await retirer.retireLegacyService(journalRoot: fixture.sameRoot)

        guard case .blocked(.inspectionFailure, _) = result else {
            Issue.record("expected inspection failure, got \(result)")
            return
        }
        #expect(FileManager.default.fileExists(atPath: fixture.plistURL.path))
        #expect(runner.invocations.map(\.arguments.first) == ["print", "kill", "print", "bootout", "print"])
    }

    @Test func bootoutTimeoutBlocksWithoutUnlink() async throws {
        let fixture = try ServiceFixture()
        defer { fixture.clear() }
        try fixture.writeSameRootPlist()
        let runner = FakeSubprocessRunner()
        runner.enqueue("print", .success(stdout: Data(loadedLaunchctlOutput(plistPath: fixture.plistURL.path).utf8)))
        runner.enqueue("kill", .success())
        runner.enqueue("print", .success(stdout: Data(stoppedLaunchctlOutput(plistPath: fixture.plistURL.path).utf8)))
        runner.enqueue("bootout", .success(delay: .milliseconds(1)))
        let retirer = LegacyJournalServiceRetirer(
            runner: runner,
            clock: NoopLegacyClock(),
            plistURL: fixture.plistURL,
            uid: 501,
            commandTimeout: .milliseconds(1)
        )

        let result = await retirer.retireLegacyService(journalRoot: fixture.sameRoot)

        guard case .blocked(.inspectionFailure, _) = result else {
            Issue.record("expected inspection failure, got \(result)")
            return
        }
        #expect(FileManager.default.fileExists(atPath: fixture.plistURL.path))
        #expect(runner.invocations.map(\.arguments.first) == ["print", "kill", "print", "bootout", "print"])
    }

    @Test func absencePollTimeoutBlocksWithoutUnlink() async throws {
        let fixture = try ServiceFixture()
        defer { fixture.clear() }
        try fixture.writeSameRootPlist()
        let runner = FakeSubprocessRunner()
        let loaded = Data(loadedLaunchctlOutput(plistPath: fixture.plistURL.path).utf8)
        runner.enqueue("print", .success(stdout: loaded))
        runner.enqueue("kill", .success())
        runner.enqueue("print", .success(stdout: Data(stoppedLaunchctlOutput(plistPath: fixture.plistURL.path).utf8)))
        runner.enqueue("bootout", .success())
        runner.enqueue("print", .success(stdout: loaded))
        runner.enqueue("print", .success(stdout: loaded))
        let retirer = LegacyJournalServiceRetirer(
            runner: runner,
            clock: NoopLegacyClock(),
            plistURL: fixture.plistURL,
            uid: 501,
            absenceTimeout: .milliseconds(1),
            absencePollInterval: .milliseconds(1)
        )

        let result = await retirer.retireLegacyService(journalRoot: fixture.sameRoot)

        guard case .blocked(.inspectionFailure, _) = result else {
            Issue.record("expected inspection failure, got \(result)")
            return
        }
        #expect(FileManager.default.fileExists(atPath: fixture.plistURL.path))
        #expect(runner.invocations.map(\.arguments.first) == ["print", "kill", "print", "bootout", "print", "print"])
    }

    @Test func unlinkFailureBlocksAfterProofWithoutRemovingPlist() async throws {
        let fixture = try ServiceFixture()
        defer { fixture.clear() }
        try fixture.writeSameRootPlist()
        let runner = FakeSubprocessRunner()
        runner.enqueue("print", .success(stderr: Data(notFoundLaunchctlError.utf8), exitCode: 113))
        let retirer = LegacyJournalServiceRetirer(
            runner: runner,
            clock: NoopLegacyClock(),
            plistURL: fixture.plistURL,
            uid: 501,
            fileManager: RemovalFailingLegacyFileManager(failingName: fixture.plistURL.lastPathComponent)
        )

        let result = await retirer.retireLegacyService(journalRoot: fixture.sameRoot)

        guard case .blocked(.inspectionFailure, _) = result else {
            Issue.record("expected inspection failure, got \(result)")
            return
        }
        #expect(FileManager.default.fileExists(atPath: fixture.plistURL.path))
        #expect(!runner.invocations.contains { $0.arguments.first == "bootout" })
    }

    @Test func cancellationDuringBootoutBlocksWithoutUnlink() async throws {
        let fixture = try ServiceFixture()
        defer { fixture.clear() }
        try fixture.writeSameRootPlist()
        let runner = BootoutCancellingLegacyRunner(
            loadedOutput: Data(loadedLaunchctlOutput(plistPath: fixture.plistURL.path).utf8)
        )
        let retirer = LegacyJournalServiceRetirer(
            runner: runner,
            clock: NoopLegacyClock(),
            plistURL: fixture.plistURL,
            uid: 501
        )

        let result = await retirer.retireLegacyService(journalRoot: fixture.sameRoot)

        guard case .blocked(.inspectionFailure, _) = result else {
            Issue.record("expected inspection failure, got \(result)")
            return
        }
        #expect(FileManager.default.fileExists(atPath: fixture.plistURL.path))
        #expect(runner.invocations.map(\.arguments.first).last == "bootout")
    }

    @Test func canonicalLoadedSameRootServiceBootsOutPollsAbsentAndUnlinks() async throws {
        let fixture = try ServiceFixture()
        defer { fixture.clear() }
        let journalRoot = fixture.root.appendingPathComponent("journal", isDirectory: true)
        let logPath = journalRoot.appendingPathComponent("health/service.log").path
        let solstonePath = fixture.root.appendingPathComponent("bin/solstone").path
        let argv = [solstonePath, "journal", "start", "5015"]
        try fixture.writeCustomPlist(programArguments: argv, standardOutPath: logPath, standardErrorPath: logPath)
        let runner = FakeSubprocessRunner()
        runner.enqueue("print", .success(stdout: Data(canonicalLoadedLaunchctlOutput(plistPath: fixture.plistURL.path, programArguments: argv).utf8)))
        runner.enqueue("kill", .success())
        runner.enqueue("print", .success(stdout: Data(stoppedLaunchctlOutput(plistPath: fixture.plistURL.path).utf8)))
        runner.enqueue("bootout", .success())
        runner.enqueue("print", .success(stderr: Data(notFoundLaunchctlError.utf8), exitCode: 113))
        let retirer = LegacyJournalServiceRetirer(
            runner: runner,
            clock: NoopLegacyClock(),
            plistURL: fixture.plistURL,
            uid: 501
        )

        let result = await retirer.retireLegacyService(journalRoot: journalRoot)

        #expect(result == .success(.provenMatchLoaded))
        #expect(!FileManager.default.fileExists(atPath: fixture.plistURL.path))
        #expect(runner.invocations.map(\.arguments.first) == ["print", "kill", "print", "bootout", "print"])
    }

    @Test func canonicalNotLoadedSameRootPlistIsUnlinked() async throws {
        let fixture = try ServiceFixture()
        defer { fixture.clear() }
        let journalRoot = fixture.root.appendingPathComponent("journal", isDirectory: true)
        let logPath = journalRoot.appendingPathComponent("health/service.log").path
        let solstonePath = fixture.root.appendingPathComponent("bin/solstone").path
        let argv = [solstonePath, "journal", "start", "5015"]
        try fixture.writeCustomPlist(programArguments: argv, standardOutPath: logPath, standardErrorPath: logPath)
        let runner = FakeSubprocessRunner()
        runner.enqueue("print", .success(stderr: Data(notFoundLaunchctlError.utf8), exitCode: 113))
        let retirer = LegacyJournalServiceRetirer(
            runner: runner,
            clock: NoopLegacyClock(),
            plistURL: fixture.plistURL,
            uid: 501
        )

        let result = await retirer.retireLegacyService(journalRoot: journalRoot)

        #expect(result == .success(.provenMatchUnloaded))
        #expect(!FileManager.default.fileExists(atPath: fixture.plistURL.path))
        #expect(runner.invocations.map(\.arguments.first) == ["print"])
    }

    @Test func canonicalDifferentRootBlocksWithoutMutation() async throws {
        let fixture = try ServiceFixture()
        defer { fixture.clear() }
        let journalRoot = fixture.root.appendingPathComponent("journal", isDirectory: true)
        let otherJournalRoot = fixture.root.appendingPathComponent("other-journal", isDirectory: true)
        let otherLogPath = otherJournalRoot.appendingPathComponent("health/service.log").path
        let solstonePath = fixture.root.appendingPathComponent("bin/solstone").path
        let argv = [solstonePath, "journal", "start", "5015"]
        try fixture.writeCustomPlist(programArguments: argv, standardOutPath: otherLogPath, standardErrorPath: otherLogPath)
        let runner = FakeSubprocessRunner()
        runner.enqueue("print", .success(stderr: Data(notFoundLaunchctlError.utf8), exitCode: 113))
        let retirer = LegacyJournalServiceRetirer(
            runner: runner,
            clock: NoopLegacyClock(),
            plistURL: fixture.plistURL,
            uid: 501
        )

        let result = await retirer.retireLegacyService(journalRoot: journalRoot)

        guard case .blocked(.differentRoot, _) = result else {
            Issue.record("expected differentRoot, got \(result)")
            return
        }
        #expect(FileManager.default.fileExists(atPath: fixture.plistURL.path))
        #expect(runner.invocations.map(\.arguments.first) == ["print"])
    }

    @Test func installedServiceForThisJournalIsRetiredWithRoomForItsShutdown() async throws {
        let fixture = try ServiceFixture()
        defer { fixture.clear() }
        let journalRoot = fixture.root.appendingPathComponent("journal", isDirectory: true)
        try FileManager.default.createDirectory(at: journalRoot, withIntermediateDirectories: true)
        let argv = [fixture.root.appendingPathComponent("bin/solstone").path, "journal", "start", "5015"]
        try fixture.writeInstalledServicePlist(programArguments: argv, journalTokenPath: journalRoot.path)
        let runner = FakeSubprocessRunner()
        let loaded = Data(canonicalLoadedLaunchctlOutput(plistPath: fixture.plistURL.path, programArguments: argv).utf8)
        runner.enqueue("print", .success(stdout: loaded))
        runner.enqueue("kill", .success())
        // The journal takes 12 s to shut down after SIGTERM (polled every 250 ms),
        // then sits stopped: KeepAlive does not restart a clean exit.
        for _ in 0..<48 {
            runner.enqueue("print", .success(stdout: loaded))
        }
        runner.enqueue("print", .success(stdout: Data(stoppedLaunchctlOutput(plistPath: fixture.plistURL.path, programArguments: argv).utf8)))
        runner.enqueue("bootout", .success())
        runner.enqueue("print", .success(stderr: Data(notFoundLaunchctlError.utf8), exitCode: 113))
        let retirer = LegacyJournalServiceRetirer(
            runner: runner,
            clock: NoopLegacyClock(),
            plistURL: fixture.plistURL,
            uid: 501
        )

        let result = await retirer.retireLegacyService(journalRoot: journalRoot)

        #expect(result == .success(.provenMatchLoaded))
        #expect(!FileManager.default.fileExists(atPath: fixture.plistURL.path))
        let order = runner.invocations.compactMap(\.arguments.first)
        #expect(order.first == "print" && order.dropFirst().first == "kill")
        #expect(order.filter { $0 == "bootout" }.count == 1)
        // bootout comes only once the journal's own shutdown has finished.
        #expect(order.firstIndex(of: "bootout") == order.count - 2)
        let kill = try #require(runner.invocations.first { $0.arguments.first == "kill" })
        #expect(kill.arguments == ["kill", "SIGTERM", "gui/501/org.solpbc.solstone"])
    }

    @Test func runningServiceThatIgnoresTermIsStillUnloaded() async throws {
        let fixture = try ServiceFixture()
        defer { fixture.clear() }
        try fixture.writeSameRootPlist()
        let runner = FakeSubprocessRunner()
        let loaded = Data(loadedLaunchctlOutput(plistPath: fixture.plistURL.path).utf8)
        runner.enqueue("print", .success(stdout: loaded))
        runner.enqueue("kill", .success())
        for _ in 0..<200 {
            runner.enqueue("print", .success(stdout: loaded))
        }
        let retirer = LegacyJournalServiceRetirer(
            runner: runner,
            clock: NoopLegacyClock(),
            plistURL: fixture.plistURL,
            uid: 501,
            absenceTimeout: .seconds(1),
            gracefulStopTimeout: .seconds(1)
        )
        runner.enqueue("bootout", .success())

        _ = await retirer.retireLegacyService(journalRoot: fixture.sameRoot)

        #expect(runner.invocations.contains { $0.arguments.first == "bootout" })
    }

    @Test func failedTermStillUnloadsTheService() async throws {
        let fixture = try ServiceFixture()
        defer { fixture.clear() }
        try fixture.writeSameRootPlist()
        let runner = FakeSubprocessRunner()
        runner.enqueue("print", .success(stdout: Data(loadedLaunchctlOutput(plistPath: fixture.plistURL.path).utf8)))
        runner.enqueue("kill", .success(stderr: Data("kill failed\n".utf8), exitCode: 3))
        runner.enqueue("bootout", .success())
        runner.enqueue("print", .success(stderr: Data(notFoundLaunchctlError.utf8), exitCode: 113))
        let retirer = LegacyJournalServiceRetirer(
            runner: runner,
            clock: NoopLegacyClock(),
            plistURL: fixture.plistURL,
            uid: 501
        )

        let result = await retirer.retireLegacyService(journalRoot: fixture.sameRoot)

        #expect(result == .success(.provenMatchLoaded))
        #expect(runner.invocations.map(\.arguments.first) == ["print", "kill", "bootout", "print"])
    }

    @Test func installedServiceReachedThroughALinkIsTheSameJournal() async throws {
        let fixture = try ServiceFixture()
        defer { fixture.clear() }
        let journalRoot = fixture.root.appendingPathComponent("journal", isDirectory: true)
        let linkedRoot = fixture.root.appendingPathComponent("journal-link", isDirectory: true)
        try FileManager.default.createDirectory(at: journalRoot, withIntermediateDirectories: true)
        try FileManager.default.createSymbolicLink(at: linkedRoot, withDestinationURL: journalRoot)
        let argv = [fixture.root.appendingPathComponent("bin/solstone").path, "journal", "start", "5015"]
        try fixture.writeInstalledServicePlist(programArguments: argv, journalTokenPath: linkedRoot.path)
        let runner = FakeSubprocessRunner()
        runner.enqueue("print", .success(stderr: Data(notFoundLaunchctlError.utf8), exitCode: 113))
        let retirer = LegacyJournalServiceRetirer(
            runner: runner,
            clock: NoopLegacyClock(),
            plistURL: fixture.plistURL,
            uid: 501
        )

        let result = await retirer.retireLegacyService(journalRoot: journalRoot)

        #expect(result == .success(.provenMatchUnloaded))
        #expect(!FileManager.default.fileExists(atPath: fixture.plistURL.path))
    }

    @Test func installedServiceForAnotherJournalBlocksWithoutMutation() async throws {
        let fixture = try ServiceFixture()
        defer { fixture.clear() }
        let journalRoot = fixture.root.appendingPathComponent("journal", isDirectory: true)
        let otherRoot = fixture.root.appendingPathComponent("other-journal", isDirectory: true)
        let argv = [fixture.root.appendingPathComponent("bin/solstone").path, "journal", "start", "5015"]
        try fixture.writeInstalledServicePlist(programArguments: argv, journalTokenPath: otherRoot.path)
        let runner = FakeSubprocessRunner()
        runner.enqueue("print", .success(stdout: Data(canonicalLoadedLaunchctlOutput(plistPath: fixture.plistURL.path, programArguments: argv).utf8)))
        let retirer = LegacyJournalServiceRetirer(
            runner: runner,
            clock: NoopLegacyClock(),
            plistURL: fixture.plistURL,
            uid: 501
        )

        let result = await retirer.retireLegacyService(journalRoot: journalRoot)

        guard case .blocked(.differentRoot, _) = result else {
            Issue.record("expected different-root block, got \(result)")
            return
        }
        #expect(FileManager.default.fileExists(atPath: fixture.plistURL.path))
        #expect(!runner.invocations.contains { $0.arguments.first == "bootout" })
    }

    @Test(arguments: ["", "2f7", "2F55736572732F6A6572", "zz", "6a6f75726e616c"])
    func unreadableJournalTokenBlocksWithoutMutation(token: String) async throws {
        let fixture = try ServiceFixture()
        defer { fixture.clear() }
        let journalRoot = fixture.root.appendingPathComponent("journal", isDirectory: true)
        let argv = [fixture.root.appendingPathComponent("bin/solstone").path, "journal", "start", "5015"]
        try fixture.writeInstalledServicePlist(programArguments: argv, rawJournalToken: token)
        let runner = FakeSubprocessRunner()
        runner.enqueue("print", .success(stderr: Data(notFoundLaunchctlError.utf8), exitCode: 113))
        let retirer = LegacyJournalServiceRetirer(
            runner: runner,
            clock: NoopLegacyClock(),
            plistURL: fixture.plistURL,
            uid: 501
        )

        let result = await retirer.retireLegacyService(journalRoot: journalRoot)

        guard case .blocked(.malformed, _) = result else {
            Issue.record("expected malformed block, got \(result)")
            return
        }
        #expect(FileManager.default.fileExists(atPath: fixture.plistURL.path))
    }

    /// Command lines and journal markers earlier service installs wrote; each
    /// names this journal, so each is retired.
    @Test(arguments: InstalledShape.retired)
    func everyInstalledShapeForThisJournalIsRetired(shape: InstalledShape) async throws {
        let fixture = try ServiceFixture()
        defer { fixture.clear() }
        let journalRoot = fixture.root.appendingPathComponent("journal", isDirectory: true)
        try FileManager.default.createDirectory(at: journalRoot, withIntermediateDirectories: true)
        try fixture.writeDictionary(shape.plist(root: fixture.root, journal: journalRoot.path))
        let runner = FakeSubprocessRunner()
        runner.enqueue("print", .success(stderr: Data(notFoundLaunchctlError.utf8), exitCode: 113))
        let retirer = LegacyJournalServiceRetirer(runner: runner, clock: NoopLegacyClock(), plistURL: fixture.plistURL, uid: 501)

        let result = await retirer.retireLegacyService(journalRoot: journalRoot)

        #expect(result == .success(.provenMatchUnloaded), "\(shape)")
        #expect(!FileManager.default.fileExists(atPath: fixture.plistURL.path))
    }

    @Test(arguments: InstalledShape.malformed)
    func unrecognisedShapeBlocksWithoutMutation(shape: InstalledShape) async throws {
        let fixture = try ServiceFixture()
        defer { fixture.clear() }
        let journalRoot = fixture.root.appendingPathComponent("journal", isDirectory: true)
        try fixture.writeDictionary(shape.plist(root: fixture.root, journal: journalRoot.path))
        let runner = FakeSubprocessRunner()
        runner.enqueue("print", .success(stderr: Data(notFoundLaunchctlError.utf8), exitCode: 113))
        let retirer = LegacyJournalServiceRetirer(runner: runner, clock: NoopLegacyClock(), plistURL: fixture.plistURL, uid: 501)

        let result = await retirer.retireLegacyService(journalRoot: journalRoot)

        guard case .blocked(.malformed, _) = result else {
            Issue.record("expected malformed for \(shape), got \(result)")
            return
        }
        #expect(FileManager.default.fileExists(atPath: fixture.plistURL.path))
    }

    @Test func overrideNamingAnotherJournalBlocksWithoutMutation() async throws {
        let fixture = try ServiceFixture()
        defer { fixture.clear() }
        let journalRoot = fixture.root.appendingPathComponent("journal", isDirectory: true)
        try fixture.writeDictionary([
            "Label": "org.solpbc.solstone",
            "ProgramArguments": [fixture.root.appendingPathComponent("bin/journal").path, "supervisor", "5015"],
            "EnvironmentVariables": ["_SOLSTONE_JOURNAL_OVERRIDE": fixture.root.appendingPathComponent("other").path],
        ])
        let runner = FakeSubprocessRunner()
        runner.enqueue("print", .success(stderr: Data(notFoundLaunchctlError.utf8), exitCode: 113))
        let retirer = LegacyJournalServiceRetirer(runner: runner, clock: NoopLegacyClock(), plistURL: fixture.plistURL, uid: 501)

        let result = await retirer.retireLegacyService(journalRoot: journalRoot)

        guard case .blocked(.differentRoot, _) = result else {
            Issue.record("expected different-root block, got \(result)")
            return
        }
        #expect(FileManager.default.fileExists(atPath: fixture.plistURL.path))
    }

    @Test func bootoutRefusedBecauseTheJobIsAlreadyGoneStillRetires() async throws {
        let fixture = try ServiceFixture()
        defer { fixture.clear() }
        try fixture.writeSameRootPlist()
        let runner = FakeSubprocessRunner()
        runner.enqueue("print", .success(stdout: Data(stoppedLaunchctlOutput(plistPath: fixture.plistURL.path).utf8)))
        runner.enqueue("bootout", .success(stderr: Data("Boot-out failed: 3: No such process\n".utf8), exitCode: 3))
        runner.enqueue("print", .success(stderr: Data(notFoundLaunchctlError.utf8), exitCode: 113))
        let retirer = LegacyJournalServiceRetirer(runner: runner, clock: NoopLegacyClock(), plistURL: fixture.plistURL, uid: 501)

        let result = await retirer.retireLegacyService(journalRoot: fixture.sameRoot)

        #expect(result == .success(.provenMatchLoaded))
        #expect(!FileManager.default.fileExists(atPath: fixture.plistURL.path))
    }

    @Test func theServiceIsTheLiveSupervisorOnlyWhenItsJobIsThatPidForThisJournal() async throws {
        let fixture = try ServiceFixture()
        defer { fixture.clear() }
        let journalRoot = fixture.root.appendingPathComponent("journal", isDirectory: true)
        try FileManager.default.createDirectory(at: journalRoot, withIntermediateDirectories: true)
        let argv = [fixture.root.appendingPathComponent("bin/solstone").path, "journal", "start", "5015"]
        let loaded = Data(canonicalLoadedLaunchctlOutput(plistPath: fixture.plistURL.path, programArguments: argv, pid: 777).utf8)
        func retirer(_ print: FakeSubprocessRunner.Response) -> LegacyJournalServiceRetirer {
            let runner = FakeSubprocessRunner()
            runner.enqueue("print", print)
            return LegacyJournalServiceRetirer(runner: runner, clock: NoopLegacyClock(), plistURL: fixture.plistURL, uid: 501)
        }

        try fixture.writeInstalledServicePlist(programArguments: argv, journalTokenPath: journalRoot.path)
        #expect(await retirer(.success(stdout: loaded)).runsLiveSupervisor(journalRoot: journalRoot, supervisorPID: 777))
        // Another process runs the journal.
        #expect(!(await retirer(.success(stdout: loaded)).runsLiveSupervisor(journalRoot: journalRoot, supervisorPID: 778)))
        // The job is loaded but not running.
        #expect(!(await retirer(.success(stdout: Data(stoppedLaunchctlOutput(plistPath: fixture.plistURL.path, programArguments: argv).utf8))).runsLiveSupervisor(journalRoot: journalRoot, supervisorPID: 777)))
        // launchd could not be asked.
        #expect(!(await retirer(.failure("launchctl failed")).runsLiveSupervisor(journalRoot: journalRoot, supervisorPID: 777)))

        try fixture.writeInstalledServicePlist(programArguments: argv, journalTokenPath: fixture.root.appendingPathComponent("other").path)
        #expect(!(await retirer(.success(stdout: loaded)).runsLiveSupervisor(journalRoot: journalRoot, supervisorPID: 777)))

        try FileManager.default.removeItem(at: fixture.plistURL)
        #expect(!(await retirer(.success(stdout: loaded)).runsLiveSupervisor(journalRoot: journalRoot, supervisorPID: 777)))
    }

    @Test func basenameOnlySolstoneBlocksWithoutMutation() async throws {
        let fixture = try ServiceFixture()
        defer { fixture.clear() }
        let journalRoot = fixture.root.appendingPathComponent("journal", isDirectory: true)
        let logPath = journalRoot.appendingPathComponent("health/service.log").path
        let solstonePath = fixture.root.appendingPathComponent("bin/solstone").path
        let argv = [solstonePath, "start", "5015"]
        try fixture.writeCustomPlist(programArguments: argv, standardOutPath: logPath, standardErrorPath: logPath)
        let runner = FakeSubprocessRunner()
        runner.enqueue("print", .success(stderr: Data(notFoundLaunchctlError.utf8), exitCode: 113))
        let retirer = LegacyJournalServiceRetirer(
            runner: runner,
            clock: NoopLegacyClock(),
            plistURL: fixture.plistURL,
            uid: 501
        )

        let result = await retirer.retireLegacyService(journalRoot: journalRoot)

        guard case .blocked(.malformed, _) = result else {
            Issue.record("expected malformed, got \(result)")
            return
        }
        #expect(FileManager.default.fileExists(atPath: fixture.plistURL.path))
        #expect(runner.invocations.map(\.arguments.first) == ["print"])
    }
}

private let notFoundLaunchctlError = """
Bad request.
Could not find service "org.solpbc.solstone" in domain for user gui: 501

"""

private func loadedLaunchctlOutput(plistPath: String, pid: pid_t = 777) -> String {
    """
gui/501/org.solpbc.solstone = {
    path = \(plistPath)
    state = running
    program = /Users/jer/.local/bin/journal
    arguments = {
        /Users/jer/.local/bin/journal
        start
        5015
    }
    pid = \(pid)
}

"""
}

/// A loaded job that is not running: no pid line.
private func stoppedLaunchctlOutput(
    plistPath: String,
    programArguments: [String] = ["/Users/jer/.local/bin/journal", "start", "5015"]
) -> String {
    let argsText = programArguments.map { "        \($0)" }.joined(separator: "\n")
    return """
gui/501/org.solpbc.solstone = {
    path = \(plistPath)
    state = not running
    program = \(programArguments[0])
    arguments = {
\(argsText)
    }
    last exit code = 0
}

"""
}

private func canonicalLoadedLaunchctlOutput(
    plistPath: String,
    programArguments: [String],
    pid: pid_t = 777
) -> String {
    let argsText = programArguments.map { "        \($0)" }.joined(separator: "\n")
    return """
gui/501/org.solpbc.solstone = {
    path = \(plistPath)
    state = running
    program = \(programArguments[0])
    arguments = {
\(argsText)
    }
    pid = \(pid)
}

"""
}

struct InstalledShape: CustomStringConvertible, Sendable {
    let name: String
    let launcher: String
    let arguments: [String]
    let logs: (out: String, err: String)?
    let override: Bool
    let token: Bool

    var description: String { name }

    func plist(root: URL, journal: String) -> [String: Any] {
        var dict: [String: Any] = [
            "Label": "org.solpbc.solstone",
            "ProgramArguments": [root.appendingPathComponent("bin/\(launcher)").path] + arguments,
            "RunAtLoad": true,
            "KeepAlive": ["SuccessfulExit": false],
        ]
        var environment: [String: String] = ["HOME": "/Users/jer", "PATH": "/usr/bin:/bin"]
        if override { environment["_SOLSTONE_JOURNAL_OVERRIDE"] = journal }
        if token { environment["SOLSTONE_INSTALLATION_JOURNAL_TOKEN"] = journal.utf8.map { String(format: "%02x", $0) }.joined() }
        dict["EnvironmentVariables"] = environment
        if let logs {
            dict["StandardOutPath"] = logs.out.replacingOccurrences(of: "<J>", with: journal)
            dict["StandardErrorPath"] = logs.err.replacingOccurrences(of: "<J>", with: journal)
        }
        return dict
    }

    static let retired: [InstalledShape] = [
        .init(name: "journal start, one service log", launcher: "journal", arguments: ["start", "5015"], logs: ("<J>/health/service.log", "<J>/health/service.log"), override: false, token: false),
        .init(name: "journal supervisor on another port", launcher: "journal", arguments: ["supervisor", "6123"], logs: ("<J>/health/service.log", "<J>/health/service.log"), override: false, token: false),
        .init(name: "journal supervisor, split launchd logs", launcher: "journal", arguments: ["supervisor", "5015"], logs: ("<J>/health/launchd-stdout.log", "<J>/health/launchd-stderr.log"), override: false, token: false),
        .init(name: "bare supervisor named by override", launcher: "journal", arguments: ["supervisor"], logs: ("<J>/health/launchd-stdout.log", "<J>/health/launchd-stderr.log"), override: true, token: false),
        .init(name: "sol supervisor named by override, no logs", launcher: "sol", arguments: ["supervisor", "5015"], logs: nil, override: true, token: false),
        .init(name: "journal journal start", launcher: "journal", arguments: ["journal", "start", "5015"], logs: ("<J>/health/service.log", "<J>/health/service.log"), override: false, token: false),
        .init(name: "solstone journal start, token, another port", launcher: "solstone", arguments: ["journal", "start", "6123"], logs: nil, override: false, token: true),
        .init(name: "solstone journal supervisor, token", launcher: "solstone", arguments: ["journal", "supervisor", "5015"], logs: nil, override: false, token: true),
    ]

    static let malformed: [InstalledShape] = [
        .init(name: "no journal named anywhere", launcher: "journal", arguments: ["supervisor", "5015"], logs: nil, override: false, token: false),
        .init(name: "logs outside a health folder", launcher: "journal", arguments: ["start", "5015"], logs: ("<J>/service.log", "<J>/service.log"), override: false, token: false),
        .init(name: "only one log path", launcher: "journal", arguments: ["start", "5015"], logs: ("<J>/health/service.log", ""), override: false, token: false),
        .init(name: "port that is not a number", launcher: "journal", arguments: ["start", "50x5"], logs: ("<J>/health/service.log", "<J>/health/service.log"), override: false, token: false),
        .init(name: "trailing argument", launcher: "journal", arguments: ["start", "5015", "extra"], logs: ("<J>/health/service.log", "<J>/health/service.log"), override: false, token: false),
        .init(name: "unknown verb", launcher: "journal", arguments: ["serve", "5015"], logs: ("<J>/health/service.log", "<J>/health/service.log"), override: false, token: false),
        .init(name: "unknown launcher", launcher: "python3", arguments: ["start", "5015"], logs: ("<J>/health/service.log", "<J>/health/service.log"), override: false, token: false),
    ]
}

private struct ServiceFixture {
    let root: URL
    let plistURL: URL
    let sameRoot = URL(fileURLWithPath: "/Users/jer/journal", isDirectory: true)

    init() throws {
        root = FileManager.default.temporaryDirectory
            .appendingPathComponent("legacy-service-retirer-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        plistURL = root.appendingPathComponent("org.solpbc.solstone.plist")
    }

    func writeSameRootPlist() throws {
        try writePlist(replacing: [:])
    }

    func writePlist(replacing replacements: [String: String]) throws {
        let sourceURL = try #require(Bundle.module.url(
            forResource: "org.solpbc.solstone.same-root",
            withExtension: "plist",
            subdirectory: "Fixtures/legacy-service"
        ))
        var text = try String(contentsOf: sourceURL, encoding: .utf8)
        for (needle, replacement) in replacements {
            text = text.replacingOccurrences(of: needle, with: replacement)
        }
        try Data(text.utf8).write(to: plistURL)
    }

    func writeCustomPlist(
        programArguments: [String],
        standardOutPath: String,
        standardErrorPath: String,
        label: String = "org.solpbc.solstone"
    ) throws {
        let dict: [String: Any] = [
            "Label": label,
            "ProgramArguments": programArguments,
            "StandardOutPath": standardOutPath,
            "StandardErrorPath": standardErrorPath,
        ]
        let data = try PropertyListSerialization.data(fromPropertyList: dict, format: .xml, options: 0)
        try data.write(to: plistURL)
    }

    /// The keys the installed service's launchd plist carries: no log paths,
    /// and the journal named by its token in the environment.
    func writeInstalledServicePlist(programArguments: [String], journalTokenPath: String) throws {
        let token = journalTokenPath.utf8.map { String(format: "%02x", $0) }.joined()
        try writeInstalledServicePlist(programArguments: programArguments, rawJournalToken: token)
    }

    func writeInstalledServicePlist(programArguments: [String], rawJournalToken: String) throws {
        let dict: [String: Any] = [
            "Label": "org.solpbc.solstone",
            "ProgramArguments": programArguments,
            "EnvironmentVariables": [
                "HOME": "/Users/jer",
                "PATH": "/Users/jer/.local/share/solstone/runtime:/usr/bin:/bin",
                "SOLSTONE_INSTALLATION_NAMESPACE": String(repeating: "0", count: 64),
                "SOLSTONE_INSTALLATION_ID": String(repeating: "1", count: 32),
                "SOLSTONE_INSTALLATION_GENERATION": "1",
                "SOLSTONE_INSTALLATION_JOURNAL_TOKEN": rawJournalToken,
            ],
            "RunAtLoad": true,
            "KeepAlive": ["SuccessfulExit": false],
            "SoftResourceLimits": ["NumberOfFiles": 4096],
        ]
        let data = try PropertyListSerialization.data(fromPropertyList: dict, format: .xml, options: 0)
        try data.write(to: plistURL)
    }

    func writeDictionary(_ dict: [String: Any]) throws {
        let data = try PropertyListSerialization.data(fromPropertyList: dict, format: .xml, options: 0)
        try data.write(to: plistURL)
    }

    func clear() {
        try? FileManager.default.removeItem(at: root)
    }
}

private final class NoopLegacyClock: MonotonicClock, @unchecked Sendable {
    private let lock = NSLock()
    private var value: Duration = .zero

    func now() -> Duration {
        lock.withLock { value }
    }

    func sleep(for duration: Duration) async {
        lock.withLock { value += duration }
    }
}

private final class RemovalFailingLegacyFileManager: FileManager {
    private let failingName: String

    init(failingName: String) {
        self.failingName = failingName
        super.init()
    }

    override func removeItem(at url: URL) throws {
        if url.lastPathComponent == failingName {
            throw CocoaError(.fileWriteNoPermission)
        }
        try super.removeItem(at: url)
    }
}

private final class BootoutCancellingLegacyRunner: SubprocessRunning, @unchecked Sendable {
    private let lock = NSLock()
    private let loadedOutput: Data
    private var recordedInvocations: [SubprocessInvocation] = []

    var invocations: [SubprocessInvocation] {
        lock.withLock { recordedInvocations }
    }

    init(loadedOutput: Data) {
        self.loadedOutput = loadedOutput
    }

    func run(
        executable: URL,
        arguments: [String],
        environment: [String: String]?,
        timeout: Duration?,
        stdoutHandler: @escaping @Sendable (Data) -> Void,
        stderrHandler: @escaping @Sendable (Data) -> Void
    ) async throws -> SubprocessResult {
        lock.withLock {
            recordedInvocations.append(SubprocessInvocation(
                executable: executable,
                arguments: arguments,
                environment: environment,
                timeout: timeout
            ))
        }
        if arguments.first == "bootout" {
            throw CancellationError()
        }
        if arguments.first == "print" {
            stdoutHandler(loadedOutput)
        }
        return SubprocessResult(exitCode: 0, terminationReason: .exit)
    }

    func cancelAll() {}
}
