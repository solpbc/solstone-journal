// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import AppKit
import Foundation
@testable import JournalRuntime
import ServiceManagement
import SwiftUI
import Testing
@testable import journal

@MainActor
private final class ThrowingLoginItems: LoginItemManaging {
    var status: SMAppService.Status = .notRegistered

    func register() throws { throw CocoaError(.fileWriteUnknown) }
    func unregister() throws { throw CocoaError(.fileWriteUnknown) }
    func unregisterAwaitingCompletion() async throws {}
}

@Suite("JournalSetupChrome")
@MainActor
struct JournalSetupChromeTests {

    @Test func progressIndexesAndNilRoutes() {
        // Welcome, folder, mark, thinking: indexes 1 to 4 of 7
        let welcomeProg = JournalSetupChrome.progress(
            route: .ritual(.welcome),
            hasRefusal: false,
            handoffError: nil
        )
        #expect(welcomeProg?.index == 1)
        #expect(welcomeProg?.count == 7)
        #expect(welcomeProg?.label == "step 1 of 7")

        let folderProg = JournalSetupChrome.progress(
            route: .ritual(.folder),
            hasRefusal: false,
            handoffError: nil
        )
        #expect(folderProg?.index == 2)
        #expect(folderProg?.count == 7)
        #expect(folderProg?.label == "step 2 of 7")

        let markProg = JournalSetupChrome.progress(
            route: .ritual(.mark),
            hasRefusal: false,
            handoffError: nil
        )
        #expect(markProg?.index == 3)
        #expect(markProg?.count == 7)
        #expect(markProg?.label == "step 3 of 7")

        let thinkingProg = JournalSetupChrome.progress(
            route: .ritual(.thinking),
            hasRefusal: false,
            handoffError: nil
        )
        #expect(thinkingProg?.index == 4)
        #expect(thinkingProg?.count == 7)
        #expect(thinkingProg?.label == "step 4 of 7")

        // Unnumbered or completion ritual routes return nil
        #expect(JournalSetupChrome.progress(route: .ritual(.nameLocation), hasRefusal: false, handoffError: nil) == nil)
        #expect(JournalSetupChrome.progress(route: .ritual(.setupProgress), hasRefusal: false, handoffError: nil) == nil)
        #expect(JournalSetupChrome.progress(route: .ritual(.finishInBrowser), hasRefusal: false, handoffError: nil) == nil)
        #expect(JournalSetupChrome.progress(route: .deciding, hasRefusal: false, handoffError: nil) == nil)
        #expect(JournalSetupChrome.progress(route: .adopting, hasRefusal: false, handoffError: nil) == nil)
        #expect(JournalSetupChrome.progress(route: .home, hasRefusal: false, handoffError: nil) == nil)

        // Refusal and non-empty handoff error return nil
        #expect(JournalSetupChrome.progress(route: .ritual(.welcome), hasRefusal: true, handoffError: nil) == nil)
        #expect(JournalSetupChrome.progress(route: .ritual(.folder), hasRefusal: false, handoffError: "network error") == nil)

        // Empty handoff error still returns progress
        let emptyErrorProg = JournalSetupChrome.progress(
            route: .ritual(.welcome),
            hasRefusal: false,
            handoffError: ""
        )
        #expect(emptyErrorProg?.index == 1)
    }

    @Test func backPredicateAndModelNavigation() {
        // Base case: on folder, everything quiet -> can go back
        #expect(JournalSetupChrome.canGoBackFromFolder(
            route: .ritual(.folder),
            userOperationInFlight: false,
            pendingOperation: .none,
            outbound: .none,
            hasRefusal: false,
            handoffError: nil
        ))

        // Folder with error message still returns true
        #expect(JournalSetupChrome.canGoBackFromFolder(
            route: .ritual(.folder),
            userOperationInFlight: false,
            pendingOperation: .none,
            outbound: .none,
            hasRefusal: false,
            handoffError: nil
        ))

        // Blocking conditions return false
        #expect(!JournalSetupChrome.canGoBackFromFolder(
            route: .ritual(.welcome),
            userOperationInFlight: false,
            pendingOperation: .none,
            outbound: .none,
            hasRefusal: false,
            handoffError: nil
        ))
        #expect(!JournalSetupChrome.canGoBackFromFolder(
            route: .ritual(.folder),
            userOperationInFlight: true,
            pendingOperation: .none,
            outbound: .none,
            hasRefusal: false,
            handoffError: nil
        ))
        #expect(!JournalSetupChrome.canGoBackFromFolder(
            route: .ritual(.folder),
            userOperationInFlight: false,
            pendingOperation: .setup,
            outbound: .none,
            hasRefusal: false,
            handoffError: nil
        ))
        #expect(!JournalSetupChrome.canGoBackFromFolder(
            route: .ritual(.folder),
            userOperationInFlight: false,
            pendingOperation: .none,
            outbound: .delivering,
            hasRefusal: false,
            handoffError: nil
        ))
        #expect(!JournalSetupChrome.canGoBackFromFolder(
            route: .ritual(.folder),
            userOperationInFlight: false,
            pendingOperation: .none,
            outbound: .none,
            hasRefusal: true,
            handoffError: nil
        ))
        #expect(!JournalSetupChrome.canGoBackFromFolder(
            route: .ritual(.folder),
            userOperationInFlight: false,
            pendingOperation: .none,
            outbound: .none,
            hasRefusal: false,
            handoffError: "failed"
        ))

        // backFromFolder navigation and state safety
        let fixture = makeModel(startResults: [], probeResults: [])
        let model = fixture.model
        model.route = .ritual(.folder)
        let initialRoot = model.journalRoot
        let initialConfigRoot = fixture.config.journalRoot

        model.backFromFolder()

        #expect(model.route == .ritual(.welcome))
        #expect(model.journalRoot == initialRoot)
        #expect(fixture.config.journalRoot == initialConfigRoot)

        // Round trip: a folder the owner chose survives back and start setup, and a stale
        // folder error does not come back with it.
        let chosen = URL(fileURLWithPath: "/tmp/chosen-journal-\(UUID().uuidString)", isDirectory: true)
        model.route = .ritual(.folder)
        model.journalRoot = chosen
        model.errorMessage = "stale folder error"
        model.backFromFolder()
        #expect(model.errorMessage == nil)
        model.continueFromWelcome()
        #expect(model.route == .ritual(.folder))
        #expect(model.journalRoot == chosen)
    }

    @Test func downloadRowsVisibilityAndActions() {
        let copy = JournalSetupCopy.Downloads(
            speech: "speech",
            thinking: "thinking",
            progress: "progress",
            cancel: "cancel",
            retry: "retry",
            error: "error",
            idle: "idle",
            complete: "complete",
            cancelled: "cancelled"
        )

        // Speech download row visibility
        #expect(JournalSetupChrome.speechDownloadRow(job: nil, copy: copy, speechFailure: nil, locked: false, downloadLabel: { _ in "dl" }) == nil)
        let speechIdle = JournalSetupState.Job(state: .idle, attemptID: nil, bytesDone: 0, bytesTotal: nil)
        #expect(JournalSetupChrome.speechDownloadRow(job: speechIdle, copy: copy, speechFailure: nil, locked: false, downloadLabel: { _ in "dl" }) == nil)
        let speechComplete = JournalSetupState.Job(state: .complete, attemptID: nil, bytesDone: 100, bytesTotal: 100)
        #expect(JournalSetupChrome.speechDownloadRow(job: speechComplete, copy: copy, speechFailure: nil, locked: false, downloadLabel: { _ in "dl" }) == nil)
        let speechCancelled = JournalSetupState.Job(state: .cancelled, attemptID: nil, bytesDone: 50, bytesTotal: 100)
        #expect(JournalSetupChrome.speechDownloadRow(job: speechCancelled, copy: copy, speechFailure: nil, locked: false, downloadLabel: { _ in "dl" }) == nil)

        // Speech downloading (determinate and indeterminate)
        let speechDownloadingDet = JournalSetupState.Job(state: .downloading, attemptID: "1", bytesDone: 500, bytesTotal: 1000)
        let speechRowDet = JournalSetupChrome.speechDownloadRow(job: speechDownloadingDet, copy: copy, speechFailure: nil, locked: false, downloadLabel: { _ in "dl" })
        #expect(speechRowDet != nil)
        #expect(speechRowDet?.name == "speech")
        #expect(speechRowDet?.fraction == 0.5)
        #expect(speechRowDet?.indeterminate == false)
        #expect(speechRowDet?.detail?.contains("500") == true)
        #expect(speechRowDet?.detail?.contains("of") == true)
        #expect(speechRowDet?.actionTitle == nil)

        let speechDownloadingIndet = JournalSetupState.Job(state: .downloading, attemptID: "1", bytesDone: 500, bytesTotal: nil)
        let speechRowIndet = JournalSetupChrome.speechDownloadRow(job: speechDownloadingIndet, copy: copy, speechFailure: nil, locked: false, downloadLabel: { _ in "dl" })
        #expect(speechRowIndet?.fraction == nil)
        #expect(speechRowIndet?.indeterminate == true)
        #expect(speechRowIndet?.detail?.contains("500") == true)

        // Speech error and retry action
        let speechError = JournalSetupState.Job(state: .error, attemptID: "1", bytesDone: 500, bytesTotal: 1000, error: .init(code: "err", detail: "network timeout"))
        let speechRowErr = JournalSetupChrome.speechDownloadRow(job: speechError, copy: copy, speechFailure: nil, canRetry: true, locked: false, downloadLabel: { _ in "fallback" })
        // The retry follows the model's showsSpeechRetry, never the row's own reading.
        #expect(JournalSetupChrome.speechDownloadRow(job: speechError, copy: copy, speechFailure: nil, canRetry: false, locked: false, downloadLabel: { _ in "fallback" })?.actionTitle == nil)
        #expect(speechRowErr?.actionTitle == "retry")
        #expect(speechRowErr?.actionIdentifier == AXID.Journal.Ritual.downloadSpeechRetry)
        #expect(speechRowErr?.actionKind == .retrySpeech)
        #expect(speechRowErr?.detail == "network timeout")

        // Failure detail precedence on speech
        let speechRowErrPrecedence = JournalSetupChrome.speechDownloadRow(job: speechError, copy: copy, speechFailure: "custom failure", locked: false, downloadLabel: { _ in "fallback" })
        #expect(speechRowErrPrecedence?.detail == "custom failure")

        // Thinking download row visibility
        #expect(JournalSetupChrome.thinkingDownloadRow(job: nil, copy: copy, thinkingFailure: nil, operationError: .none, locked: false, downloadLabel: { _ in "dl" }) == nil)
        let thinkingIdle = JournalSetupState.Job(state: .idle, attemptID: nil, bytesDone: 0, bytesTotal: nil)
        #expect(JournalSetupChrome.thinkingDownloadRow(job: thinkingIdle, copy: copy, thinkingFailure: nil, operationError: .none, locked: false, downloadLabel: { _ in "dl" }) == nil)
        let thinkingComplete = JournalSetupState.Job(state: .complete, attemptID: nil, bytesDone: 100, bytesTotal: 100)
        #expect(JournalSetupChrome.thinkingDownloadRow(job: thinkingComplete, copy: copy, thinkingFailure: nil, operationError: .none, locked: false, downloadLabel: { _ in "dl" }) == nil)

        // Thinking downloading: cancel action wins over retry
        let thinkingDownloading = JournalSetupState.Job(state: .downloading, attemptID: "att-1", bytesDone: 200, bytesTotal: 1000)
        let thinkingRowDownloading = JournalSetupChrome.thinkingDownloadRow(
            job: thinkingDownloading,
            copy: copy,
            thinkingFailure: "previous error",
            operationError: .thinkingDownload,
            canRetry: true,
            canCancel: true,
            locked: false,
            downloadLabel: { _ in "dl" }
        )
        #expect(thinkingRowDownloading != nil)
        #expect(thinkingRowDownloading?.actionTitle == "cancel")
        #expect(thinkingRowDownloading?.actionIdentifier == AXID.Journal.Ritual.downloadThinkingCancel)
        #expect(thinkingRowDownloading?.actionKind == .cancelThinking)

        // Thinking cancelled state offers retry
        let thinkingCancelled = JournalSetupState.Job(state: .cancelled, attemptID: nil, bytesDone: 200, bytesTotal: 1000)
        let thinkingRowCancelled = JournalSetupChrome.thinkingDownloadRow(
            job: thinkingCancelled,
            copy: copy,
            thinkingFailure: nil,
            operationError: .none,
            canRetry: true,
            locked: false,
            downloadLabel: { _ in "cancelled" }
        )
        #expect(thinkingRowCancelled?.actionTitle == "retry")
        #expect(thinkingRowCancelled?.actionIdentifier == AXID.Journal.Ritual.downloadThinkingRetry)
        #expect(thinkingRowCancelled?.actionKind == .retryThinking)

        // Thinking error state and failure precedence
        let thinkingError = JournalSetupState.Job(state: .error, attemptID: nil, bytesDone: 200, bytesTotal: 1000, error: .init(code: "fail", detail: "disk full"))
        let thinkingRowErr = JournalSetupChrome.thinkingDownloadRow(
            job: thinkingError,
            copy: copy,
            thinkingFailure: "cannot retry",
            operationError: .thinkingDownload,
            canRetry: true,
            locked: true,
            downloadLabel: { _ in "fallback" }
        )
        #expect(thinkingRowErr?.detail == "cannot retry")
        #expect(thinkingRowErr?.actionTitle == "retry")
        #expect(thinkingRowErr?.actionEnabled == false) // locked
    }

    @Test func thinkingChoicesFilteringAndChooseLater() {
        let choices = [
            JournalSetupCopy.Choice(id: "local", label: "Local", sub: "Fast", description: "Runs locally"),
            JournalSetupCopy.Choice(id: "byo", label: "BYO", sub: "Cloud", description: "Bring your own")
        ]

        // When offersLocalThinking is false, local choice is omitted
        let filteredWithoutLocal = JournalSetupChrome.filteredThinkingChoices(choices: choices, offersLocalThinking: false)
        #expect(filteredWithoutLocal.map(\.id) == ["byo"])

        // When offersLocalThinking is true, local choice is kept
        let filteredWithLocal = JournalSetupChrome.filteredThinkingChoices(choices: choices, offersLocalThinking: true)
        #expect(filteredWithLocal.map(\.id) == ["local", "byo"])

        // chooseLater rule
        #expect(JournalSetupChrome.shouldShowChooseLater(offersLocalThinking: false, displayedChoice: nil))
        #expect(!JournalSetupChrome.shouldShowChooseLater(offersLocalThinking: false, displayedChoice: .local))
        #expect(JournalSetupChrome.shouldShowChooseLater(offersLocalThinking: false, displayedChoice: .later))

        #expect(JournalSetupChrome.shouldShowChooseLater(offersLocalThinking: true, displayedChoice: .later))
        #expect(!JournalSetupChrome.shouldShowChooseLater(offersLocalThinking: true, displayedChoice: .local))
        #expect(!JournalSetupChrome.shouldShowChooseLater(offersLocalThinking: true, displayedChoice: .byo))
        #expect(JournalSetupChrome.shouldShowChooseLater(offersLocalThinking: true, displayedChoice: nil))
    }

    @Test func loginBindingHandlesFailure() {
        let suite = "app.solstone.journal.first-run.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defaults.removePersistentDomain(forName: suite)
        defaults.set(false, forKey: "launchAtLoginEnabled")

        let config = JournalAppConfig(defaults: defaults, loginItemManager: ThrowingLoginItems())
        let fixture = makeModel(config: config, startResults: [], probeResults: [])
        let model = fixture.model
        model.route = .ritual(.welcome)

        model.welcomeLaunchAtLogin.wrappedValue = true

        #expect(model.route == .ritual(.welcome))
        #expect(model.loginFailure?.isEmpty == false)
        #expect(defaults.bool(forKey: "launchAtLoginEnabled") == false)
        #expect(!model.launchAtLoginEnabled)
    }

    @Test func windowConfiguratorTransitionsAndReopenPredicate() {
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 700, height: 450),
            styleMask: [.titled, .closable, .resizable],
            backing: .buffered,
            defer: false
        )
        window.identifier = NSUserInterfaceItemIdentifier("journal")
        window.title = "journal"

        #expect(!window.styleMask.contains(.fullSizeContentView))

        // Apply setup configuration
        JournalSetupWindowConfigurator.apply(setupShowing: true, to: window)

        #expect(window.title == "set up solstone")
        #expect(window.titlebarAppearsTransparent == true)
        #expect(window.titleVisibility == .hidden)
        #expect(window.styleMask.contains(.fullSizeContentView))
        #expect(window.contentMinSize == CGSize(width: 880, height: 580))
        let contentSizeAfterSetup = window.contentRect(forFrameRect: window.frame).size
        #expect(contentSizeAfterSetup.width >= 880)
        #expect(contentSizeAfterSetup.height >= 580)

        // Grow content size and reapply setup: grown size must be preserved
        window.setContentSize(NSSize(width: 950, height: 650))
        JournalSetupWindowConfigurator.apply(setupShowing: true, to: window)
        let contentSizeAfterGrow = window.contentRect(forFrameRect: window.frame).size
        #expect(contentSizeAfterGrow.width == 950)
        #expect(contentSizeAfterGrow.height == 650)

        // Apply not-showing: restore home window chrome
        JournalSetupWindowConfigurator.apply(setupShowing: false, to: window)
        #expect(window.title == "journal")
        #expect(!window.styleMask.contains(.fullSizeContentView))
        #expect(window.titlebarAppearsTransparent == false)
        #expect(window.titleVisibility == .visible)
        #expect(window.contentMinSize == CGSize(width: 720, height: 500))

        // Dock reopen predicate
        window.title = "set up solstone"
        window.identifier = NSUserInterfaceItemIdentifier("journal")
        #expect(JournalAppDelegate.isJournalWindow(window))

        window.title = "journal"
        window.identifier = NSUserInterfaceItemIdentifier("other")
        #expect(JournalAppDelegate.isJournalWindow(window))

        let otherWindow = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 500, height: 300),
            styleMask: [.titled],
            backing: .buffered,
            defer: false
        )
        otherWindow.title = "other"
        otherWindow.identifier = NSUserInterfaceItemIdentifier("other")
        #expect(!JournalAppDelegate.isJournalWindow(otherWindow))
    }
}
