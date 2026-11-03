// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import AppKit
import Foundation
import JournalMarkKit
import JournalRuntime
import SetupChrome
import SwiftUI
import Testing
@testable import journal

@Suite("JournalSetupChromeSnapshot")
@MainActor
struct JournalSetupChromeSnapshotTests {
    private let fixedDate = Date(timeIntervalSince1970: 1_790_190_000)
    private let denver = TimeZone(identifier: "America/Denver")!

    @Test func renderSetupChromeStates() async throws {
        JournalMarkFont.register()
        #expect(JournalMarkFont.isRegistered)

        let renderer = JournalSnapshotTests()
        let copy = try loadSetupFixtureCopy()
        let background = SunArcBackgroundView(fixedDate: fixedDate, timeZone: denver)

        let sizes: [(label: String, size: CGSize)] = [
            ("880", CGSize(width: 880, height: 580)),
            ("720", CGSize(width: 720, height: 580))
        ]

        let appearances: [(name: String, appearance: NSAppearance.Name)] = [
            ("light", .aqua),
            ("dark", .darkAqua)
        ]

        struct StateFixture {
            let name: String
            let configure: (JournalFirstRunModel) -> Void
        }

        let markProposed = JournalSetupState(
            schema: "solstone.setup-state.v1",
            journalPath: "/tmp/journal",
            done: false,
            mark: .init(render: .uiTestSample, state: .proposed, proposalID: "prop-1"),
            thinking: .init(choice: nil, saved: false, capability: .ok, downloadBytes: nil),
            downloads: .init(
                speech: .init(state: .idle, attemptID: nil, bytesDone: 0, bytesTotal: nil),
                thinking: .init(state: .idle, attemptID: nil, bytesDone: 0, bytesTotal: nil)
            )
        )

        let markLocked = JournalSetupState(
            schema: "solstone.setup-state.v1",
            journalPath: "/tmp/journal",
            done: false,
            mark: .init(render: .uiTestSample, state: .locked, proposalID: "prop-1"),
            thinking: .init(choice: nil, saved: false, capability: .ok, downloadBytes: nil),
            downloads: .init(
                speech: .init(state: .idle, attemptID: nil, bytesDone: 0, bytesTotal: nil),
                thinking: .init(state: .idle, attemptID: nil, bytesDone: 0, bytesTotal: nil)
            )
        )

        let thinkingCapable = JournalSetupState(
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

        let thinkingCantRun = JournalSetupState(
            schema: "solstone.setup-state.v1",
            journalPath: "/tmp/journal",
            done: false,
            mark: .init(render: .uiTestSample, state: .locked, proposalID: "prop-1"),
            thinking: .init(choice: nil, saved: false, capability: .blocked, downloadBytes: nil),
            downloads: .init(
                speech: .init(state: .idle, attemptID: nil, bytesDone: 0, bytesTotal: nil),
                thinking: .init(state: .idle, attemptID: nil, bytesDone: 0, bytesTotal: nil)
            )
        )

        let thinkingByo = JournalSetupState(
            schema: "solstone.setup-state.v1",
            journalPath: "/tmp/journal",
            done: false,
            mark: .init(render: .uiTestSample, state: .locked, proposalID: "prop-1"),
            thinking: .init(choice: .byo, saved: false, capability: .ok, downloadBytes: 1_500_000_000),
            downloads: .init(
                speech: .init(state: .idle, attemptID: nil, bytesDone: 0, bytesTotal: nil),
                thinking: .init(state: .idle, attemptID: nil, bytesDone: 0, bytesTotal: nil)
            )
        )

        let thinkingDownloading = JournalSetupState(
            schema: "solstone.setup-state.v1",
            journalPath: "/tmp/journal",
            done: false,
            mark: .init(render: .uiTestSample, state: .locked, proposalID: "prop-1"),
            thinking: .init(choice: .local, saved: false, capability: .ok, downloadBytes: 1_500_000_000),
            downloads: .init(
                speech: .init(state: .idle, attemptID: nil, bytesDone: 0, bytesTotal: nil),
                thinking: .init(state: .downloading, attemptID: "att-1", bytesDone: 500_000_000, bytesTotal: 1_500_000_000)
            )
        )

        let thinkingFailed = JournalSetupState(
            schema: "solstone.setup-state.v1",
            journalPath: "/tmp/journal",
            done: false,
            mark: .init(render: .uiTestSample, state: .locked, proposalID: "prop-1"),
            thinking: .init(choice: .local, saved: false, capability: .ok, downloadBytes: 1_500_000_000),
            downloads: .init(
                speech: .init(state: .idle, attemptID: nil, bytesDone: 0, bytesTotal: nil),
                thinking: .init(state: .error, attemptID: "att-1", bytesDone: 500_000_000, bytesTotal: 1_500_000_000, error: .init(code: "err", detail: "network connection lost"))
            )
        )

        let settingUpState = JournalSetupState(
            schema: "solstone.setup-state.v1",
            journalPath: "/tmp/journal",
            done: false,
            mark: .init(render: .uiTestSample, state: .locked, proposalID: "prop-1"),
            thinking: .init(choice: .local, saved: true, capability: .ok, downloadBytes: 1_500_000_000),
            downloads: .init(
                speech: .init(state: .downloading, attemptID: "sp-1", bytesDone: 10_000_000, bytesTotal: 50_000_000),
                thinking: .init(state: .complete, attemptID: "att-1", bytesDone: 1_500_000_000, bytesTotal: 1_500_000_000)
            )
        )

        // The walk's footer case: speech recognition downloading on the mark step, its bytes in
        // the format that wrapped the name ("243 bytes of 483.3 MB").
        let markDownloading = JournalSetupState(
            schema: "solstone.setup-state.v1",
            journalPath: "/tmp/journal",
            done: false,
            mark: .init(render: .uiTestSample, state: .proposed, proposalID: "prop-1"),
            thinking: .init(choice: nil, saved: false, capability: .ok, downloadBytes: nil),
            downloads: .init(
                speech: .init(state: .downloading, attemptID: "sp-1", bytesDone: 243, bytesTotal: 483_300_000),
                thinking: .init(state: .idle, attemptID: nil, bytesDone: 0, bytesTotal: nil)
            )
        )

        let markLockedUnreadable = JournalSetupState(
            schema: "solstone.setup-state.v1",
            journalPath: "/tmp/journal",
            done: false,
            mark: .init(render: nil, state: .locked, proposalID: nil),
            thinking: .init(choice: nil, saved: false, capability: .ok, downloadBytes: nil),
            downloads: .init(
                speech: .init(state: .idle, attemptID: nil, bytesDone: 0, bytesTotal: nil),
                thinking: .init(state: .idle, attemptID: nil, bytesDone: 0, bytesTotal: nil)
            )
        )

        let markProposedNoRender = JournalSetupState(
            schema: "solstone.setup-state.v1",
            journalPath: "/tmp/journal",
            done: false,
            mark: .init(render: nil, state: .proposed, proposalID: "prop-1"),
            thinking: .init(choice: nil, saved: false, capability: .ok, downloadBytes: nil),
            downloads: .init(
                speech: .init(state: .idle, attemptID: nil, bytesDone: 0, bytesTotal: nil),
                thinking: .init(state: .idle, attemptID: nil, bytesDone: 0, bytesTotal: nil)
            )
        )

        let states: [StateFixture] = [
            StateFixture(name: "welcome") { model in
                model.applySnapshotState(route: .ritual(.welcome), setupCopy: copy, loginFailure: "Could not set start at login")
            },
            StateFixture(name: "folder") { model in
                model.applySnapshotState(route: .ritual(.folder), setupCopy: copy)
            },
            StateFixture(name: "mark-proposed") { model in
                model.applySnapshotState(route: .ritual(.mark), setupState: markProposed, setupCopy: copy)
            },
            StateFixture(name: "mark-locked") { model in
                model.applySnapshotState(route: .ritual(.mark), setupState: markLocked, setupCopy: copy)
            },
            StateFixture(name: "mark-locked-unreadable") { model in
                model.applySnapshotState(route: .ritual(.mark), setupState: markLockedUnreadable, setupCopy: copy)
                #expect(model.lockedMarkIsUnreadable)
            },
            StateFixture(name: "mark-proposed-no-render") { model in
                model.applySnapshotState(route: .ritual(.mark), setupState: markProposedNoRender, setupCopy: copy)
                #expect(!model.lockedMarkIsUnreadable)
            },
            StateFixture(name: "thinking-capable") { model in
                model.applySnapshotState(route: .ritual(.thinking), setupState: thinkingCapable, setupCopy: copy)
            },
            StateFixture(name: "mark-downloading") { model in
                model.applySnapshotState(route: .ritual(.mark), setupState: markDownloading, setupCopy: copy)
            },
            StateFixture(name: "thinking-later") { model in
                model.applySnapshotState(route: .ritual(.thinking), setupState: thinkingCapable, setupCopy: copy, draftThinkingChoice: .later)
            },
            StateFixture(name: "thinking-downloading-busy") { model in
                model.applySnapshotState(route: .ritual(.thinking), setupState: thinkingDownloading, setupCopy: copy, draftThinkingChoice: .local, pendingOperation: .thinkingSave)
            },
            StateFixture(name: "refusal-busy") { model in
                model.applySnapshotState(
                    route: .ritual(.setupProgress),
                    setupCopy: copy,
                    handoffRefusal: JournalSetupRefusalResolution(
                        handoffID: UUID(),
                        journalRootPath: "/tmp/journal",
                        reason: .heldPairing
                    ),
                    pendingOperation: .outbound
                )
            },
            StateFixture(name: "thinking-cant-run") { model in
                model.applySnapshotState(route: .ritual(.thinking), setupState: thinkingCantRun, setupCopy: copy)
            },
            StateFixture(name: "thinking-byo") { model in
                model.applySnapshotState(route: .ritual(.thinking), setupState: thinkingByo, setupCopy: copy, draftThinkingChoice: .byo)
            },
            StateFixture(name: "thinking-downloading") { model in
                model.applySnapshotState(route: .ritual(.thinking), setupState: thinkingDownloading, setupCopy: copy, draftThinkingChoice: .local)
            },
            StateFixture(name: "thinking-failed") { model in
                model.applySnapshotState(route: .ritual(.thinking), setupState: thinkingFailed, setupCopy: copy, draftThinkingChoice: .local)
            },
            StateFixture(name: "refusal") { model in
                model.applySnapshotState(
                    route: .ritual(.welcome),
                    setupCopy: copy,
                    handoffRefusal: JournalSetupRefusalResolution(
                        handoffID: UUID(),
                        journalRootPath: "/tmp/journal",
                        reason: .heldPairing
                    )
                )
            },
            StateFixture(name: "setting-up") { model in
                model.applySnapshotState(route: .ritual(.setupProgress), setupState: settingUpState, setupCopy: copy)
                model.currentStep = "Preparing journal index"
            }
        ]

        for state in states {
            for (appLabel, appearance) in appearances {
                for (widthLabel, size) in sizes {
                    let fixture = makeModel(startResults: [], probeResults: [])
                    state.configure(fixture.model)

                    let view = JournalFirstRunView(
                        model: fixture.model,
                        minSize: size,
                        background: background
                    )

                    let filename = "journal-setup-\(state.name)-\(appLabel)-\(widthLabel).png"
                    try await renderer.render(
                        view.frame(width: size.width, height: size.height),
                        size: size,
                        appearance: appearance,
                        validateContent: true,
                        to: filename
                    )
                }
            }
        }
    }
}
