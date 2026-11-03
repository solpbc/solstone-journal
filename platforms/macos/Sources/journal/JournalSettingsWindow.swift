// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import AppKit
import JournalMarkKit
import SolstoneCore
import SwiftUI
import UpdateKit

struct JournalSettingsWindow: View {
    private enum AboutCopyState: Equatable {
        case copied
        case failed
    }

    @Bindable var model: JournalWindowModel
    @Bindable var updateController: UpdateController
    var openURL: @MainActor (URL) -> Bool
    private let clipboardWrite: @MainActor (String) -> Bool
    @State private var aboutCopyState: AboutCopyState?

    init(
        model: JournalWindowModel,
        updateController: UpdateController,
        openURL: @escaping @MainActor (URL) -> Bool = { NSWorkspace.shared.open($0) },
        clipboardWrite: @escaping @MainActor (String) -> Bool = { text in
            let pasteboard = NSPasteboard.general
            pasteboard.clearContents()
            return pasteboard.setString(text, forType: .string)
        }
    ) {
        self.model = model
        self.updateController = updateController
        self.openURL = openURL
        self.clipboardWrite = clipboardWrite
    }

    var body: some View {
        NavigationSplitView(columnVisibility: .constant(.all)) {
            List(selection: $model.selectedPane) {
                ForEach(JournalPane.allCases) { pane in
                    HStack {
                        Label(pane.title, systemImage: pane.systemImage)
                        if pane == .updates, updateController.durableUpdateStatus.needsAttention {
                            Spacer()
                            Image(systemName: "circle.fill")
                                .font(.system(size: 6))
                                .accessibilityLabel("update needs attention")
                        }
                    }
                        .tag(pane)
                        .accessibilityIdentifier(AXID.Journal.Sidebar.tab(pane))
                        .overlay {
                            AXStateCompanion(
                                id: AXID.Journal.Sidebar.tabState(pane),
                                value: (model.selectedPane == pane ? JournalSidebarTabState.selected : .unselected).axToken
                            )
                        }
                }
            }
            .listStyle(.sidebar)
            .navigationSplitViewColumnWidth(min: 180, ideal: 200)
        } detail: {
            ScrollView {
                detailContent
                    .padding(20)
                    .frame(maxWidth: .infinity, alignment: .topLeading)
            }
            .background(.regularMaterial)
        }
        .frame(minWidth: 720, minHeight: 500)
        .onAppear {
            model.handlePaneOpen(model.selectedPane)
        }
        .onChange(of: model.selectedPane) { _, newValue in
            model.handlePaneOpen(newValue)
        }
    }

    @ViewBuilder
    private var detailContent: some View {
        switch model.selectedPane {
        case .home:
            homePane
        case .journal:
            journalPane
        case .runState:
            runStatePane
        case .devices:
            JournalDevicesPane(model: model.devicesModel)
        case .backup:
            backupPane
        case .startup:
            startupPane
        case .updates:
            UpdatesTabView(controller: updateController, copy: UpdatesCopy(provider: .journal))
        }
    }

    private var homePane: some View {
        VStack(alignment: .leading, spacing: 20) {
            Text("your journal, at a glance")
                .font(.title2.weight(.semibold))

            // journal-mark.md section 4.3 — the org-wide "no journal identity yet"
            // treatment applies when presentation is generic. An unavailable mark renders the
            // unavailable card.
            JournalMarkPresentationView(presentation: model.markPresentation)
                .accessibilityElement(children: .contain)
                .accessibilityIdentifier(AXID.Journal.Home.markCard)

            VStack(alignment: .leading, spacing: 8) {
                statusLine(model.runDisplay.label, systemImage: "circle.fill")
                AXStateCompanion(id: AXID.Journal.Home.runDisplayGlanceState, value: model.runDisplay.axToken)
            }

            if let message = updateAttentionMessage {
                GroupBox {
                    VStack(alignment: .leading, spacing: 8) {
                        Text(message)
                        Button("view journal updates") { model.selectedPane = .updates }
                            .accessibilityIdentifier(AXID.Journal.Home.updates)
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
            }

            switch model.homeOffer {
            case .unconfigured:
                if let message = model.unconfiguredMessage {
                    Text(message)
                        .foregroundStyle(.secondary)
                    AXStateCompanion(id: AXID.Journal.Home.unconfiguredMessageState, value: message)
                }
            case .door, .start, .none, .runState:
                Text("this app keeps your journal running on this mac. your journal itself opens in your browser.")
                    .foregroundStyle(.secondary)
                switch model.homeOffer {
                case .door:
                    Button {
                        model.openJournal(using: openURL)
                    } label: {
                        Label("open your journal", systemImage: "arrow.up.right.square")
                    }
                    .buttonStyle(.borderedProminent)
                    .controlSize(.large)
                    .accessibilityIdentifier(AXID.Journal.Home.openJournal)
                case .start:
                    if let stoppedReason = model.stoppedReason {
                        Text(stoppedReason)
                            .foregroundStyle(.secondary)
                    }
                    Button {
                        model.startJournal()
                    } label: {
                        Label("start", systemImage: "play.fill")
                    }
                    .accessibilityIdentifier(AXID.Journal.Home.start)
                case .runState:
                    if let message = model.localAccessFailure ?? model.supervisor.blockedReason {
                        Text(message).font(.callout)
                    }
                    Button {
                        model.selectedPane = .runState
                    } label: {
                        Label("run state →", systemImage: "waveform.path.ecg")
                    }
                    .accessibilityIdentifier(AXID.Journal.Home.runState)
                case .none, .unconfigured:
                    EmptyView()
                }
            }
        }
    }

    private var updateAttentionMessage: String? {
        switch updateController.durableUpdateStatus {
        case .available(let version, _): "the journal app \(version) is available."
        case .staged(let version, _): "the journal app \(version) is downloaded and ready to install."
        case .deferred: "your journal update will continue after your journal is ready."
        case .failedWithAvailable(let version): "the update check failed. the journal app \(version) was found earlier."
        case .failed: "the journal update check failed."
        case .idle, .upToDate: nil
        }
    }

    private var journalPane: some View {
        VStack(alignment: .leading, spacing: 20) {
            Text(JournalPane.journal.title)
                .font(.title2.weight(.semibold))

            infoRow("location", value: model.journalRootPath)
            AXStateCompanion(id: AXID.Journal.Pane.locationPathState, value: model.journalRootPath)

            infoRow("disk used", value: model.diskUsageValue)
            AXStateCompanion(
                id: AXID.Journal.Pane.diskUsageState,
                value: String(model.diskUsageBytes ?? 0)
            )
        }
    }

    private var runStatePane: some View {
        VStack(alignment: .leading, spacing: 20) {
            Text("run state")
                .font(.title2.weight(.semibold))

            VStack(alignment: .leading, spacing: 6) {
                statusLine(model.runDisplay.label, systemImage: "circle.fill")
                if let blockedReason = model.localAccessFailure ?? (model.runDisplay == .blocked ? model.supervisor.blockedReason : nil) {
                    Text(blockedReason)
                        .foregroundStyle(.secondary)
                }
                if let stoppedReason = model.stoppedReason {
                    Text(stoppedReason)
                        .foregroundStyle(.secondary)
                }
                VStack(alignment: .leading, spacing: 0) {
                    AXStateCompanion(id: AXID.Journal.RunState.displayState, value: model.runDisplay.axToken)
                    AXStateCompanion(
                        id: AXID.Journal.RunState.blockedReasonState,
                        value: model.runDisplay == .blocked ? (model.supervisor.blockedReason ?? "") : ""
                    )
                }
            }

            HStack(spacing: 8) {
                Button {
                    model.startJournal()
                } label: {
                    Label("start", systemImage: "play.fill")
                }
                .accessibilityIdentifier(AXID.Journal.RunState.start)

                Button {
                    model.stopJournal()
                } label: {
                    Label("stop", systemImage: "stop.fill")
                }
                .accessibilityIdentifier(AXID.Journal.RunState.stop)

                Button {
                    model.restartJournal()
                } label: {
                    Label("restart", systemImage: "arrow.clockwise")
                }
                .accessibilityIdentifier(AXID.Journal.RunState.restart)
            }

            infoRow("health", value: model.healthDisplay.label)
            AXStateCompanion(id: AXID.Journal.RunState.healthState, value: model.healthDisplay.axToken)

            VStack(alignment: .leading, spacing: 6) {
                Text(model.aboutBlock)
                    .textSelection(.enabled)
                    .accessibilityIdentifier(AXID.Journal.RunState.aboutState)
                    .accessibilityValue(model.aboutBlock)
                AXStateCompanion(id: AXID.Journal.RunState.aboutState, value: model.aboutBlock)
                Button("copy") {
                    let snapshot = model.aboutBlock
                    aboutCopyState = clipboardWrite(snapshot) ? .copied : .failed
                }
                .accessibilityIdentifier(AXID.Journal.RunState.aboutCopy)
                if let aboutCopyState {
                    let message = aboutCopyState == .copied ? "copied" : "couldn't copy. select the text and copy it."
                    Text(message)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .accessibilityIdentifier(AXID.Journal.RunState.aboutCopyFeedbackState)
                    AXStateCompanion(id: AXID.Journal.RunState.aboutCopyFeedbackState, value: message)
                }
            }
        }
    }

    private var backupPane: some View {
        VStack(alignment: .leading, spacing: 20) {
            Text("backup")
                .font(.title2.weight(.semibold))
            Text("backup keeps your journal safe. set it up from your journal.")
                .foregroundStyle(.secondary)
            AXStateCompanion(
                id: AXID.Journal.Backup.messageState,
                value: "backup keeps your journal safe. set it up from your journal."
            )

            Button {
                model.openBackup(using: { openURL($0) })
            } label: {
                Label("open backup", systemImage: "arrow.up.right.square")
            }
            .accessibilityIdentifier(AXID.Journal.Backup.openBackup)
        }
    }

    private var startupPane: some View {
        VStack(alignment: .leading, spacing: 20) {
            Text("startup")
                .font(.title2.weight(.semibold))
            Toggle(
                "launch the journal when you log in",
                isOn: Binding(
                    get: { model.launchAtLoginEnabled },
                    set: { model.setLaunchAtLoginEnabled($0) }
                )
            )
            .accessibilityIdentifier(AXID.Journal.Startup.launchAtLogin)
            AXStateCompanion(
                id: AXID.Journal.Startup.launchAtLoginState,
                value: (model.launchAtLoginEnabled ? JournalEnabledState.enabled : .disabled).axToken
            )
            if let failure = model.launchAtLoginFailure, !failure.isEmpty {
                Text(failure)
                    .foregroundStyle(.red)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    private func infoRow(_ label: String, value: String) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(label)
                .font(.headline)
            Text(value)
                .foregroundStyle(.secondary)
        }
    }

    private func statusLine(_ text: String, systemImage: String) -> some View {
        Label {
            Text(text)
        } icon: {
            Image(systemName: systemImage)
                .font(.system(size: 8))
        }
    }
}

struct JournalMarkPresentationView: View {
    let presentation: JournalMarkPresentation

    var body: some View {
        switch presentation {
        case .mark(let mark):
            JournalMarkView(mark: mark, isConfirmed: true)
        case .generic:
            JournalMarkView(mark: nil, isConfirmed: true)
        case .loading:
            JournalMarkView(mark: nil, isConfirmed: true)
                .accessibilityValue("your journal, mark not read yet")
        case .unavailable:
            JournalMarkUnavailableView()
        }
    }
}
