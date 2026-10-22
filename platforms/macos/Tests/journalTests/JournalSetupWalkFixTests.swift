// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import AppKit
import JournalMarkKit
import Foundation
import Observation
@testable import JournalRuntime
import SetupChrome
import ServiceManagement
import SwiftUI
import Testing
@testable import journal

@MainActor
private final class AcceptingLoginItems: LoginItemManaging {
    var status: SMAppService.Status = .notRegistered

    func register() throws {}
    func unregister() throws {}
    func unregisterAwaitingCompletion() async throws {}
}

private final class ChangeFlag: @unchecked Sendable {
    var fired = false
}

/// The by-eye walk of released Journal.app 2.0.52 (2026-10-20) found these on its setup steps.
/// Each test fails on the code the walk ran.
@Suite("Journal setup — walk fixes")
@MainActor
struct JournalSetupWalkFixTests {
    private static func source(_ file: String) throws -> String {
        let url = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .appendingPathComponent("Sources/journal/\(file)")
        return try String(contentsOf: url, encoding: .utf8)
    }

    // D2: the welcome switch redraws when a click flips the saved value. Observation has to see
    // the read through to the saved setting, or SwiftUI never re-renders the switch.
    @Test func theWelcomeLoginSwitchRedrawsWhenItsSavedValueFlips() {
        let suite = "app.solstone.journal.walk-fix.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        defaults.set(true, forKey: "launchAtLoginEnabled")
        let config = JournalAppConfig(defaults: defaults, loginItemManager: AcceptingLoginItems())
        let fixture = makeModel(config: config, startResults: [], probeResults: [])
        let model = fixture.model
        model.route = .ritual(.welcome)
        let binding = model.welcomeLaunchAtLogin

        for expected in [false, true, false] {
            let flag = ChangeFlag()
            let shown = withObservationTracking {
                binding.wrappedValue
            } onChange: {
                flag.fired = true
            }
            #expect(shown == !expected)
            binding.wrappedValue = expected
            #expect(flag.fired, "the switch was not told its value changed to \(expected)")
            #expect(binding.wrappedValue == expected)
            #expect(defaults.bool(forKey: "launchAtLoginEnabled") == expected)
        }
    }

    // D4: after "decide later" the thinking card keeps its bottom padding and corners above the
    // footer at the 880 x 580 minimum. Four points above the footer is sheet, not card.
    @Test func theThinkingCardFitsAboveTheFooterAfterDecideLater() async throws {
        JournalMarkFont.register()
        let copy = try loadSetupFixtureCopy()
        let state = JournalSetupState(
            schema: "solstone.setup-state.v1",
            journalPath: "/tmp/journal",
            done: false,
            mark: .init(render: .uiTestSample, state: .locked, proposalID: "prop-1"),
            thinking: .init(choice: nil, saved: false, capability: .ok, downloadBytes: 1_500_000_000),
            downloads: .init(
                speech: .init(state: .idle, attemptID: nil, bytesDone: 0, bytesTotal: nil),
                thinking: .init(state: .idle, attemptID: nil, bytesDone: 0, bytesTotal: nil)
            )
        )
        let fixture = makeModel(startResults: [], probeResults: [])
        fixture.model.applySnapshotState(route: .ritual(.thinking), setupState: state, setupCopy: copy, draftThinkingChoice: .later)

        let size = CGSize(width: 880, height: 580)
        let view = JournalFirstRunView(
            model: fixture.model,
            minSize: size,
            background: SunArcBackgroundView(fixedDate: Date(timeIntervalSince1970: 1_790_190_000), timeZone: TimeZone(identifier: "America/Denver")!)
        )
        .frame(width: size.width, height: size.height)
        .environment(\.colorScheme, .dark)
        let host = NSHostingView(rootView: view)
        host.appearance = NSAppearance(named: .darkAqua)
        host.frame = NSRect(origin: .zero, size: size)
        host.layoutSubtreeIfNeeded()
        let rep = try #require(host.bitmapImageRepForCachingDisplay(in: host.bounds))
        host.cacheDisplay(in: host.bounds, to: rep)
        let scale = CGFloat(rep.pixelsHigh) / size.height
        // The card's column, 4 pt above the footer's top hairline (18 pt inset + 66 pt bar + 1 pt).
        let y = rep.pixelsHigh - Int((18 + 66 + 1 + 4) * scale)
        let x = Int(size.width * 0.75 * scale)
        let color = try #require(rep.colorAt(x: x, y: y)?.usingColorSpace(.sRGB))
        let red = Int((color.redComponent * 255).rounded())
        // Dark sheet #221C19 (red 34); dark card #2B231C (red 43).
        #expect(red <= 38, "card fill runs into the footer: red \(red)")
    }

    // D8: the setting-up card never shows the runtime's internal step name, and no second
    // spinner sits in the band over the sun. The step name stays in its AX companion.
    @Test func settingUpShowsNoInternalStepNameAndNoBandSpinner() throws {
        let view = try Self.source("JournalFirstRunView.swift")
        #expect(!view.contains("Text(model.currentStep"), "the internal step name is shown as text")
        #expect(view.contains("AXStateCompanion(id: AXID.Journal.Ritual.setupStepState, value: model.currentStep ?? \"\")"))
        #expect(!view.contains(".overlay(alignment: .top) {\n            if model.userOperationInFlight"), "the band spinner is back")
    }

    // D8's re-diff: the band spinner was the only progress shown while the mark commits, the
    // thinking choice saves and hands off, or a refusal opens the journal. That progress now sits
    // in the footer beside the step's button, on every screen but the setting-up card, which has
    // its own. Rendered: a footer pixel left of the primary is the spinner's, not the footer fill.
    private func footerSpinnerPixels(
        route: JournalFirstRunRoute,
        pending: JournalRitualOperationToken,
        refusal: Bool = false,
        errorMessage: String? = nil
    ) throws -> Int {
        JournalMarkFont.register()
        let copy = try loadSetupFixtureCopy()
        let state = JournalSetupState(
            schema: "solstone.setup-state.v1",
            journalPath: "/tmp/journal",
            done: false,
            mark: .init(render: .uiTestSample, state: .proposed, proposalID: "prop-1"),
            thinking: .init(choice: nil, saved: false, capability: .ok, downloadBytes: 1_500_000_000),
            downloads: .init(
                speech: .init(state: .idle, attemptID: nil, bytesDone: 0, bytesTotal: nil),
                thinking: .init(state: .idle, attemptID: nil, bytesDone: 0, bytesTotal: nil)
            )
        )
        let fixture = makeModel(startResults: [], probeResults: [])
        fixture.model.applySnapshotState(
            route: route,
            setupState: state,
            setupCopy: copy,
            handoffRefusal: refusal
                ? JournalSetupRefusalResolution(handoffID: UUID(), journalRootPath: "/tmp/journal", reason: .heldPairing)
                : nil,
            errorMessage: errorMessage,
            pendingOperation: pending
        )
        let size = CGSize(width: 880, height: 580)
        let view = JournalFirstRunView(
            model: fixture.model,
            minSize: size,
            background: SunArcBackgroundView(fixedDate: Date(timeIntervalSince1970: 1_790_190_000), timeZone: TimeZone(identifier: "America/Denver")!)
        )
        .frame(width: size.width, height: size.height)
        .environment(\.colorScheme, .light)
        let host = NSHostingView(rootView: view)
        host.appearance = NSAppearance(named: .aqua)
        host.frame = NSRect(origin: .zero, size: size)
        host.layoutSubtreeIfNeeded()
        let rep = try #require(host.bitmapImageRepForCachingDisplay(in: host.bounds))
        host.cacheDisplay(in: host.bounds, to: rep)
        // Count dark pixels in the footer's right third, outside the orange primary: the
        // spinner's spokes are grey on the light footer fill.
        let scale = CGFloat(rep.pixelsHigh) / size.height
        let top = rep.pixelsHigh - Int((18 + 66) * scale)
        let bottom = rep.pixelsHigh - Int(18 * scale)
        var count = 0
        for y in stride(from: top, to: bottom, by: 1) {
            for x in stride(from: Int(size.width * 0.55 * scale), to: Int((size.width - 18) * scale), by: 1) {
                guard let c = rep.colorAt(x: x, y: y)?.usingColorSpace(.sRGB) else { continue }
                let r = c.redComponent, g = c.greenComponent, b = c.blueComponent
                let grey = abs(r - g) < 0.06 && abs(g - b) < 0.06 && r < 0.75
                if grey { count += 1 }
            }
        }
        return count
    }

    @Test func anOperationInFlightShowsProgressInTheFooter() throws {
        // The mark commit: no spinner before, one while it runs.
        let idle = try footerSpinnerPixels(route: .ritual(.mark), pending: .none)
        let busy = try footerSpinnerPixels(route: .ritual(.mark), pending: .mark)
        #expect(busy > idle + 20, "no footer progress while the mark commits: idle \(idle), busy \(busy)")
        // A refusal shown over the setting-up route, opening the journal.
        let refusalIdle = try footerSpinnerPixels(route: .ritual(.setupProgress), pending: .none, refusal: true)
        let refusalBusy = try footerSpinnerPixels(route: .ritual(.setupProgress), pending: .outbound, refusal: true)
        #expect(refusalBusy > refusalIdle + 20, "no footer progress while a refusal opens the journal: idle \(refusalIdle), busy \(refusalBusy)")
        // Setting up keeps only its card's spinner.
        let setupIdle = try footerSpinnerPixels(route: .ritual(.setupProgress), pending: .none)
        let setupBusy = try footerSpinnerPixels(route: .ritual(.setupProgress), pending: .setup)
        #expect(abs(setupBusy - setupIdle) <= 4, "a second spinner on setting up: idle \(setupIdle), busy \(setupBusy)")
        // A setting-up card showing an error hides its spinner, so the footer shows progress.
        let errorIdle = try footerSpinnerPixels(route: .ritual(.setupProgress), pending: .none, errorMessage: "setup did not finish")
        let errorBusy = try footerSpinnerPixels(route: .ritual(.setupProgress), pending: .outbound, errorMessage: "setup did not finish")
        #expect(errorBusy > errorIdle + 20, "no progress while retrying after a setup error: idle \(errorIdle), busy \(errorBusy)")
    }

    // D1: the setup window names itself "set up solstone" through SwiftUI, which owns the bar
    // while setup shows; without it macOS 27 drew "journal" on the sun.
    @Test func theSetupWindowNamesItself() throws {
        let view = try Self.source("JournalFirstRunView.swift")
        #expect(view.contains("SetupWindow(appLabel: JournalFirstRunCopy.yourJournal, windowTitle: JournalFirstRunCopy.setUpSolstone"))
        #expect(JournalFirstRunCopy.setUpSolstone == "set up solstone")
    }

    // The 2026-10-20 re-walk: with SwiftUI owning the setup title bar, the title can already be
    // hidden when the configurator first sees the window; home must still show its title.
    @Test func homeShowsItsTitleEvenIfSetupHidItFirst() {
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 700, height: 450),
            styleMask: [.titled, .closable, .resizable],
            backing: .buffered,
            defer: false
        )
        window.titlebarAppearsTransparent = true
        window.titleVisibility = .hidden
        JournalSetupWindowConfigurator.apply(setupShowing: true, to: window)
        JournalSetupWindowConfigurator.apply(setupShowing: false, to: window)
        #expect(window.title == "journal")
        #expect(window.titleVisibility == .visible)
        #expect(window.titlebarAppearsTransparent == false)
    }
}
