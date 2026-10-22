// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import Foundation
import ServiceManagement
import Testing
@testable import journal

@Suite("JournalCommandLine")
struct JournalCommandLineTests {
    private let executable = "/Applications/journal.app/Contents/MacOS/journal"

    @Test func appLaunchesStayWithTheApp() {
        let launches: [[String]] = [
            [executable],
            [executable, "-psn_0_1234567"],
            [executable, "-NSDocumentRevisionsDebugMode", "YES"],
            [executable, "-AppleLanguages", "(en)"],
            [executable, "-ApplePersistenceIgnoreState", "YES"],
        ]
        for arguments in launches {
            #expect(JournalCommandLine.route(arguments: arguments) == .app, "\(arguments)")
        }
    }

    @Test func commandLinesGoToTheJournalCommand() {
        let commandLines: [[String]] = [
            [executable, "--version"],
            [executable, "--help"],
            [executable, "-h"],
            [executable, "-V"],
            [executable, "doctor"],
            [executable, "install-provider", "local"],
        ]
        for arguments in commandLines {
            #expect(JournalCommandLine.route(arguments: arguments) == .commandLine, "\(arguments)")
        }
    }

    @Test func commandLineIsTheBundledRuntimeJournal() {
        let url = JournalCommandLine.commandLineURL(executableURL: URL(fileURLWithPath: executable))
        #expect(url.path == "/Applications/journal.app/Contents/Resources/solstone-runtime/bin/solstone")
    }

    @Test func commandLineResolvesThroughALinkToTheApp() throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("journal-command-line-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let macOS = root.appendingPathComponent("journal.app/Contents/MacOS", isDirectory: true)
        try FileManager.default.createDirectory(at: macOS, withIntermediateDirectories: true)
        let app = macOS.appendingPathComponent("journal")
        try Data().write(to: app)
        let link = root.appendingPathComponent("bin/journal")
        try FileManager.default.createDirectory(at: link.deletingLastPathComponent(), withIntermediateDirectories: true)
        try FileManager.default.createSymbolicLink(at: link, withDestinationURL: app)

        let url = JournalCommandLine.commandLineURL(executableURL: link)
        let contents = app.resolvingSymlinksInPath().deletingLastPathComponent().deletingLastPathComponent()
        #expect(url.path == contents.appendingPathComponent("Resources/solstone-runtime/bin/solstone").path)
    }

    @Test func execArgvBuildsPrefixedArgvWithFlagsAndSpacedPaths() {
        let executableURL = URL(fileURLWithPath: executable)
        let arguments = [executable, "--journal", "/tmp/my journal"]
        let argv = JournalCommandLine.execArgv(executableURL: executableURL, arguments: arguments)
        #expect(argv == [
            "/Applications/journal.app/Contents/Resources/solstone-runtime/bin/solstone",
            "journal",
            "--journal",
            "/tmp/my journal",
        ])
    }

    @MainActor
    @Test func cleanUninstallUnregistersBeforeForwardingTheOriginalArguments() {
        let executableURL = URL(fileURLWithPath: executable)
        let arguments = [executable, "setup", "--clean-uninstall", "--yes"]
        let loginItems = DispatchFakeLoginItemManager()

        let action = JournalCommandLine.dispatch(arguments: arguments, loginItems: loginItems)

        #expect(loginItems.unregisterCalls == 1)
        #expect(loginItems.unregisterAwaitingCompletionCalls == 0)
        guard case .execAfterWatchdogUnregister(let forwardedArguments) = action else {
            Issue.record("Expected clean uninstall to unregister the watchdog before forwarding.")
            return
        }
        #expect(forwardedArguments == arguments)

        let argv = JournalCommandLine.execArgv(executableURL: executableURL, arguments: forwardedArguments)
        #expect(argv == [
            JournalCommandLine.commandLineURL(executableURL: executableURL).path,
            "journal",
            "setup",
            "--clean-uninstall",
            "--yes",
        ])
    }

    @MainActor
    @Test func missingWatchdogIsIdempotentForTheServiceManagementDomain() {
        let arguments = [executable, "setup", "--clean-uninstall", "--yes"]
        let loginItems = DispatchFakeLoginItemManager()
        loginItems.errorToThrow = NSError(
            domain: SMAppServiceErrorDomain,
            code: Int(kSMErrorJobNotFound)
        )

        let action = JournalCommandLine.dispatch(arguments: arguments, loginItems: loginItems)

        #expect(loginItems.unregisterCalls == 1)
        guard case .execAfterWatchdogUnregister(let forwardedArguments) = action else {
            Issue.record("Expected missing watchdog to continue with the original arguments.")
            return
        }
        #expect(forwardedArguments == arguments)
    }

    @MainActor
    @Test func jobNotFoundInAnotherDomainStopsWithExit78() {
        let arguments = [executable, "setup", "--clean-uninstall", "--yes"]
        let loginItems = DispatchFakeLoginItemManager()
        loginItems.errorToThrow = NSError(
            domain: NSCocoaErrorDomain,
            code: Int(kSMErrorJobNotFound)
        )

        let action = JournalCommandLine.dispatch(arguments: arguments, loginItems: loginItems)

        #expect(loginItems.unregisterCalls == 1)
        guard case .stop(let exitCode, _) = action else {
            Issue.record("Expected unregister failure to stop command dispatch.")
            return
        }
        #expect(exitCode == 78)
    }

    @MainActor
    @Test func otherServiceManagementErrorsStopWithExit78() {
        let arguments = [executable, "setup", "--clean-uninstall", "--yes"]
        let loginItems = DispatchFakeLoginItemManager()
        loginItems.errorToThrow = NSError(domain: SMAppServiceErrorDomain, code: 22)

        let action = JournalCommandLine.dispatch(arguments: arguments, loginItems: loginItems)

        #expect(loginItems.unregisterCalls == 1)
        guard case .stop(let exitCode, _) = action else {
            Issue.record("Expected unregister failure to stop command dispatch.")
            return
        }
        #expect(exitCode == 78)
    }

    @MainActor
    @Test func otherArgumentsPreserveTheExistingRouteWithoutUnregistering() {
        let appArguments = [
            [executable],
            [executable, "-psn_0_1"],
        ]
        let commandLineArguments = [
            [executable, "--help"],
            [executable, "doctor"],
            [executable, "setup", "--clean-uninstall"],
            [executable, "setup", "--yes", "--clean-uninstall"],
            [executable, "setup", "--clean-uninstall", "-y"],
            [executable, "setup", "--clean-uninstall", "--non-interactive"],
            [executable, "--verbose", "setup", "--clean-uninstall", "--yes"],
            [executable, "setup", "--clean-uninstall", "--help"],
            [executable, "setup", "--clean-uninstall", "--yes", "--force"],
            [executable, "setup", "--clean-uninstall", "--yes", "--unknown"],
        ]
        let rows: [([String], JournalCommandLine.Route)] =
            appArguments.map { ($0, .app) } + commandLineArguments.map { ($0, .commandLine) }

        for (arguments, expectedRoute) in rows {
            let loginItems = DispatchFakeLoginItemManager()
            let action = JournalCommandLine.dispatch(arguments: arguments, loginItems: loginItems)

            #expect(JournalCommandLine.route(arguments: arguments) == expectedRoute)
            #expect(loginItems.unregisterCalls == 0)

            switch expectedRoute {
            case .app:
                guard case .app = action else {
                    Issue.record("Expected app arguments to remain on the app route.")
                    continue
                }
            case .commandLine:
                guard case .exec(let forwardedArguments) = action else {
                    Issue.record("Expected non-matching command-line arguments to forward unchanged.")
                    continue
                }
                #expect(forwardedArguments == arguments)
            }
        }
    }
}

@MainActor
private final class DispatchFakeLoginItemManager: LoginItemManaging {
    var status: SMAppService.Status = .notRegistered
    private(set) var unregisterCalls = 0
    private(set) var unregisterAwaitingCompletionCalls = 0
    var errorToThrow: Error?

    func register() throws {}

    func unregister() throws {
        unregisterCalls += 1
        if let errorToThrow {
            throw errorToThrow
        }
    }

    func unregisterAwaitingCompletion() async throws {
        unregisterAwaitingCompletionCalls += 1
        if let errorToThrow {
            throw errorToThrow
        }
    }
}
