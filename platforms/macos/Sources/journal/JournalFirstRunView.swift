// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import AppKit
import JournalMarkKit
import JournalRuntime
import SetupChrome
import SolstoneCore
import SwiftUI

struct JournalFirstRunView: View {
    @Environment(\.colorScheme) private var colorScheme
    @Bindable var model: JournalFirstRunModel
    var minSize: CGSize = CGSize(width: 880, height: 580)
    var background: SunArcBackgroundView = SunArcBackgroundView()
    @State private var returnTarget: URL?

    var body: some View {
        Group {
            if model.route.showsSetupWindow {
                SetupWindow(appLabel: JournalFirstRunCopy.yourJournal, windowTitle: JournalFirstRunCopy.setUpSolstone, background: background) {
                    rootContent
                }
                .frame(minWidth: minSize.width, minHeight: minSize.height)
            } else {
                // Deciding is not setup: the plain spinner on the sun, at the window's usual minimum.
                rootContent
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .background(background)
                    .frame(minWidth: 720, minHeight: 500)
            }
        }
        .overlay {
            AXStateCompanion(
                id: AXID.Journal.Ritual.operationPendingState,
                value: model.pendingOperation.axToken
            )
        }
        .overlay {
            AXStateCompanion(
                id: AXID.Journal.Ritual.operationErrorState,
                value: model.operationError.axToken
            )
        }
        .overlay {
            AXStateCompanion(
                id: AXID.Journal.Ritual.outboundState,
                value: model.outbound.axToken
            )
        }
        .alert(
            returnTarget.map { JournalLocationCopy.switchConfirmTitle(target: $0) } ?? "",
            isPresented: Binding(get: { returnTarget != nil }, set: { if !$0 { returnTarget = nil } })
        ) {
            Button(JournalLocationCopy.switchButton) {
                if let target = returnTarget {
                    Task { await model.switchToJournal(target) }
                }
                returnTarget = nil
            }
            .accessibilityIdentifier(AXID.Journal.Pane.switchJournalConfirm)
            Button(JournalLocationCopy.cancel, role: .cancel) { returnTarget = nil }
        } message: {
            Text(returnTarget.map { model.switchConfirmMessage(target: $0) } ?? "")
        }
    }

    @ViewBuilder
    private var rootContent: some View {
        switch model.route {
        case .deciding:
            decidingView
        case .ritual(let step):
            // A full-size base keeps the root and its route companion present even while a step
            // has no setup copy to render yet.
            ZStack {
                Color.clear
                ritualView(step)
            }
                .accessibilityElement(children: .contain)
                .accessibilityIdentifier(AXID.Journal.Ritual.root)
                .overlay {
                    AXStateCompanion(id: AXID.Journal.Ritual.routeState, value: model.route.axToken)
                }
        case .adopting:
            adoptView
                .accessibilityElement(children: .contain)
                .accessibilityIdentifier(AXID.Journal.Adopt.root)
        case .home:
            EmptyView()
        }
    }

    private var decidingView: some View {
        VStack {
            ProgressView()
                .controlSize(.large)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .overlay {
            AXStateCompanion(id: AXID.Journal.Ritual.routeState, value: model.route.axToken)
        }
    }

    @ViewBuilder
    private func ritualView(_ step: JournalFirstRunStep) -> some View {
        if let refusal = model.handoffRefusal {
            refusalView(refusal)
        } else if let handoffError = model.handoffError, !handoffError.isEmpty {
            handoffErrorView(handoffError)
        } else {
            switch step {
            case .welcome:
                welcomeView
            case .folder:
                folderView
            case .mark:
                markView
            case .thinking:
                thinkingView
            case .nameLocation:
                nameLocationView
            case .setupProgress:
                setupProgressView
            case .finishInBrowser:
                finishInBrowserView
            }
        }
    }

    private func refusalView(_ refusal: JournalSetupRefusalResolution) -> some View {
        SetupStep(headline: "", explanations: []) {
            SetupCard {
                VStack(alignment: .leading, spacing: 14) {
                    Text(refusal.reason == .heldPairing ? JournalFirstRunCopy.handoffHeldPairing : JournalFirstRunCopy.handoffCompetingSetup)
                        .font(.system(size: 14))
                        .foregroundStyle(SetupChromeColor.ink(for: colorScheme))
                        .fixedSize(horizontal: false, vertical: true)

                    Button(JournalFirstRunCopy.openYourJournal) {
                        Task { await model.openJournalAfterRefusal() }
                    }
                    .buttonStyle(SetupPrimaryButtonStyle())
                    .keyboardShortcut(.defaultAction)
                }
            }
            .accessibilityElement(children: .contain)
            .accessibilityIdentifier(AXID.Journal.Ritual.card)
        } footer: {
            stepFooter(downloads: showsDownloadFooter)
        }
    }

    private func handoffErrorView(_ message: String) -> some View {
        SetupStep(headline: "", explanations: []) {
            SetupCard {
                VStack(alignment: .leading, spacing: 14) {
                    SetupErrorText(message)
                        .fixedSize(horizontal: false, vertical: true)
                    AXStateCompanion(id: AXID.Journal.Ritual.handoffErrorState, value: message)
                    Button(model.setupCopy?.downloads.retry ?? "") {
                        Task { await model.retryHandoff() }
                    }
                    .buttonStyle(SetupSecondaryButtonStyle())
                    .accessibilityIdentifier(AXID.Journal.Ritual.handoffRetry)
                }
            }
            .accessibilityElement(children: .contain)
            .accessibilityIdentifier(AXID.Journal.Ritual.card)
        } footer: {
            stepFooter(downloads: showsDownloadFooter)
        }
    }

    @ViewBuilder
    private var welcomeView: some View {
        if let copy = model.setupCopy {
            let explanations: [String] = {
                var list = [copy.welcome.body]
                if let close = copy.welcome.howItFits.last,
                   close != copy.welcome.title, close != copy.welcome.body {
                    list.append(close)
                }
                return list
            }()
            let progress = JournalSetupChrome.progress(
                route: model.route,
                hasRefusal: model.handoffRefusal != nil,
                handoffError: model.handoffError
            )
            SetupStep(headline: copy.welcome.title, explanations: explanations) {
                SetupCard(heading: JournalFirstRunCopy.whatThisSetsUp) {
                    VStack(alignment: .leading, spacing: 14) {
                        welcomeRow(
                            glyph: "text.book.closed",
                            term: JournalFirstRunCopy.yourJournal,
                            line: JournalFirstRunCopy.whatThisSetsUpJournal
                        )
                        welcomeRow(
                            glyph: "sun.max",
                            term: JournalFirstRunCopy.theSolstoneApp,
                            line: JournalFirstRunCopy.theSolstoneAppDescription
                        )
                        welcomeRow(
                            glyph: "brain",
                            term: JournalFirstRunCopy.thinking,
                            line: JournalFirstRunCopy.thinkingDescription
                        )

                        Rectangle()
                            .fill(SetupChromeColor.hairline(for: colorScheme))
                            .frame(height: 1)

                        SetupToggle(
                            isOn: model.welcomeLaunchAtLogin,
                            label: copy.login.label,
                            help: JournalFirstRunCopy.startAutomaticallyHelp,
                            accessibilityIdentifier: AXID.Journal.Ritual.loginToggle
                        )

                        AXStateCompanion(
                            id: AXID.Journal.Ritual.loginFailureState,
                            value: model.loginFailure ?? ""
                        )
                        if let failure = model.loginFailure, !failure.isEmpty {
                            SetupErrorText(failure)
                        }
                    }
                }
                .accessibilityElement(children: .contain)
                .accessibilityIdentifier(AXID.Journal.Ritual.card)
            } footer: {
                stepFooter(
                    progress: progress,
                    offersReturn: true,
                    primary: AnyView(
                        Button(JournalFirstRunCopy.startSetup) {
                            model.continueFromWelcome()
                        }
                        .buttonStyle(SetupPrimaryButtonStyle())
                        .keyboardShortcut(.defaultAction)
                        .accessibilityIdentifier(AXID.Journal.Ritual.welcomeContinue)
                    )
                )
            }
        }
    }

    private func welcomeRow(glyph: String, term: String, line: String) -> some View {
        HStack(alignment: .top, spacing: 12) {
            ZStack {
                RoundedRectangle(cornerRadius: 8)
                    .fill(SetupChromeColor.orangeWash(for: colorScheme))
                    .frame(width: 36, height: 36)
                Image(systemName: glyph)
                    .font(.system(size: 17, weight: .semibold))
                    .foregroundStyle(SetupChromeColor.orangeText(for: colorScheme))
            }
            VStack(alignment: .leading, spacing: 2) {
                Text(term)
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundStyle(SetupChromeColor.ink(for: colorScheme))
                Text(line)
                    .font(.system(size: 13))
                    .foregroundStyle(SetupChromeColor.softInk(for: colorScheme))
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    @ViewBuilder
    private var folderView: some View {
        if let copy = model.setupCopy {
            let locked = model.userOperationInFlight || model.pendingOperation != .none
            let progress = JournalSetupChrome.progress(
                route: model.route,
                hasRefusal: model.handoffRefusal != nil,
                handoffError: model.handoffError
            )
            let back = JournalSetupChrome.canGoBackFromFolder(
                route: model.route,
                userOperationInFlight: model.userOperationInFlight,
                pendingOperation: model.pendingOperation,
                outbound: model.outbound,
                hasRefusal: model.handoffRefusal != nil,
                handoffError: model.handoffError
            )
            SetupStep(
                headline: JournalFirstRunCopy.whereYourJournalLives,
                explanations: [JournalFirstRunCopy.whereYourJournalLivesDescription]
            ) {
                SetupCard {
                    VStack(alignment: .leading, spacing: 14) {
                        Text(JournalFirstRunCopy.journalFolder)
                            .font(.system(size: 14, weight: .semibold))
                            .foregroundStyle(SetupChromeColor.softInk(for: colorScheme))

                        HStack(spacing: 8) {
                            TextField(
                                copy.folder.title,
                                text: Binding(
                                    get: { model.journalRoot.path },
                                    set: { model.journalRoot = URL(fileURLWithPath: $0, isDirectory: true) }
                                )
                            )
                            .font(.system(.body, design: .monospaced))
                            .textFieldStyle(.roundedBorder)
                            .accessibilityIdentifier(AXID.Journal.Ritual.folderField)
                            .disabled(locked)

                            Button(copy.folder.change) {
                                chooseJournalLocation()
                            }
                            .buttonStyle(SetupSecondaryButtonStyle())
                            .accessibilityIdentifier(AXID.Journal.Ritual.folderChoose)
                            .disabled(locked)
                        }

                        if let error = model.errorMessage, !error.isEmpty {
                            SetupErrorText(error)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                    }
                }
                .accessibilityElement(children: .contain)
                .accessibilityIdentifier(AXID.Journal.Ritual.card)
            } footer: {
                stepFooter(
                    progress: progress,
                    back: back,
                    offersReturn: true,
                    primary: AnyView(
                        Button(copy.folder.continueLabel) {
                            Task { await model.continueFromFolder() }
                        }
                        .buttonStyle(SetupPrimaryButtonStyle())
                        .keyboardShortcut(.defaultAction)
                        .accessibilityIdentifier(AXID.Journal.Ritual.folderContinue)
                        .disabled(locked)
                    )
                )
            }
        }
    }

    @ViewBuilder
    private var markView: some View {
        if let copy = model.setupCopy {
            let locked = model.userOperationInFlight || model.pendingOperation != .none
            let progress = JournalSetupChrome.progress(
                route: model.route,
                hasRefusal: model.handoffRefusal != nil,
                handoffError: model.handoffError
            )
            SetupStep(
                headline: copy.mark.title,
                explanations: [copy.mark.help]
            ) {
                SetupCard {
                    VStack(alignment: .center, spacing: 14) {
                        if let mark = model.validatedMark {
                            JournalMarkView(mark: mark, isConfirmed: model.setupState?.mark.state == .locked)
                                .frame(maxWidth: .infinity, alignment: .center)
                        } else if model.lockedMarkIsUnreadable {
                            JournalMarkUnavailableView()
                                .frame(maxWidth: .infinity, alignment: .center)
                        } else {
                            JournalMarkView(mark: nil)
                                .frame(maxWidth: .infinity, alignment: .center)
                        }

                        if model.showsMarkRegenerate {
                            Button(copy.mark.tryAnother) {
                                Task { await model.regenerateDisplayedMark() }
                            }
                            .buttonStyle(SetupSecondaryButtonStyle())
                            .accessibilityIdentifier(AXID.Journal.Ritual.markRegenerate)
                            .disabled(locked)
                        }

                        if model.validatedMark != nil {
                            Text(copy.mark.permanence)
                                .font(.system(size: 13))
                                .foregroundStyle(SetupChromeColor.faintInk(for: colorScheme))
                                .fixedSize(horizontal: false, vertical: true)
                                .frame(maxWidth: .infinity, alignment: .leading)
                        }

                        if model.setupState?.mark.state != .locked {
                            Button(JournalFirstRunCopy.restoreFromBackup) {
                                model.openRestoreInBrowser()
                            }
                            .buttonStyle(.link)
                            .disabled(locked)
                        }

                        if let failure = model.markFailure, !failure.isEmpty {
                            SetupErrorText(failure)
                                .fixedSize(horizontal: false, vertical: true)
                                .frame(maxWidth: .infinity, alignment: .leading)
                        } else if model.validatedMark == nil {
                            SetupErrorText(copy.mark.error)
                                .fixedSize(horizontal: false, vertical: true)
                                .frame(maxWidth: .infinity, alignment: .leading)
                        }

                        AXStateCompanion(
                            id: AXID.Journal.Ritual.markProposalState,
                            value: model.displayedProposalID ?? ""
                        )
                        AXStateCompanion(
                            id: AXID.Journal.Ritual.markErrorState,
                            value: model.markFailure ?? ""
                        )
                    }
                    .frame(maxWidth: .infinity, alignment: .center)
                }
                .accessibilityElement(children: .contain)
                .accessibilityIdentifier(AXID.Journal.Ritual.card)
            } footer: {
                stepFooter(
                    progress: progress,
                    offersReturn: true,
                    downloads: showsDownloadFooter,
                    primary: AnyView(
                        Button(copy.mark.continueLabel) {
                            Task { await model.commitDisplayedMark() }
                        }
                        .buttonStyle(SetupPrimaryButtonStyle())
                        .keyboardShortcut(.defaultAction)
                        .accessibilityIdentifier(AXID.Journal.Ritual.markContinue)
                        .disabled(!model.canCommitDisplayedMark || locked)
                    )
                )
            }
        }
    }

    @ViewBuilder
    private var thinkingView: some View {
        if let copy = model.setupCopy {
            let locked = model.userOperationInFlight || model.pendingOperation != .none
            let progress = JournalSetupChrome.progress(
                route: model.route,
                hasRefusal: model.handoffRefusal != nil,
                handoffError: model.handoffError
            )
            let choices = JournalSetupChrome.filteredThinkingChoices(
                choices: copy.thinking.choices,
                offersLocalThinking: model.offersLocalThinking
            )
            SetupStep(
                headline: JournalFirstRunCopy.whereDoesThinkingHappen,
                explanations: [JournalFirstRunCopy.whereDoesThinkingHappenDescription]
            ) {
                SetupCard {
                    VStack(alignment: .leading, spacing: 14) {
                        ForEach(choices, id: \.id) { choice in
                            let selected = model.displayedThinkingChoice?.rawValue == choice.id
                            let rowData: (label: String, subLabel: String?, description: String) = {
                                if choice.id == JournalThinkingChoice.local.rawValue {
                                    let local = JournalSetupChrome.localChoiceRow(
                                        downloadBytes: model.setupState?.thinking.downloadBytes,
                                        sizeUnknownText: copy.thinking.sizeUnknown
                                    )
                                    return (local.label, local.subLabel, local.description)
                                } else {
                                    return (choice.label, choice.sub, choice.description)
                                }
                            }()

                            SetupOptionRow(
                                isSelected: selected,
                                label: rowData.label,
                                subLabel: rowData.subLabel,
                                description: rowData.description,
                                action: { model.selectThinking(choice.id) }
                            )
                            .modifier(ThinkingChoiceIdentifier(choiceID: choice.id))
                            .accessibilityAddTraits(selected ? .isSelected : [])
                            .disabled(locked)
                        }

                        if JournalSetupChrome.shouldShowChooseLater(
                            offersLocalThinking: model.offersLocalThinking,
                            displayedChoice: model.displayedThinkingChoice
                        ) {
                            Text(copy.thinking.chooseLater)
                                .font(.system(size: 13.5))
                                .foregroundStyle(SetupChromeColor.softInk(for: colorScheme))
                                .fixedSize(horizontal: false, vertical: true)
                        }

                        Button(copy.thinking.later) {
                            model.selectThinking("later")
                        }
                        .buttonStyle(SetupTextButtonStyle())
                        .accessibilityIdentifier(AXID.Journal.Ritual.thinkingLater)
                        .accessibilityAddTraits(model.displayedThinkingChoice == .later ? .isSelected : [])
                        .disabled(locked)

                        if let failure = model.thinkingFailure, !failure.isEmpty {
                            SetupErrorText(failure)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                    }
                    // The companions ride behind the stack: as stack items each took a row of
                    // spacing, which pushed the card's bottom under the footer at 880 x 580.
                    .background(alignment: .topLeading) {
                        ZStack(alignment: .topLeading) {
                            AXStateCompanion(
                                id: AXID.Journal.Ritual.thinkingChoiceState,
                                value: model.displayedThinkingChoice?.rawValue ?? ""
                            )
                            AXStateCompanion(
                                id: AXID.Journal.Ritual.thinkingErrorState,
                                value: model.thinkingFailure ?? ""
                            )
                        }
                    }
                }
                .accessibilityElement(children: .contain)
                .accessibilityIdentifier(AXID.Journal.Ritual.card)
            } footer: {
                stepFooter(
                    progress: progress,
                    offersReturn: true,
                    downloads: showsDownloadFooter,
                    primary: model.canContinueThinking ? AnyView(
                        Button(JournalFirstRunCopy.continueInTheSolstoneApp) {
                            Task { await model.continueThinking() }
                        }
                        .buttonStyle(SetupPrimaryButtonStyle())
                        .keyboardShortcut(.defaultAction)
                        .accessibilityIdentifier(AXID.Journal.Ritual.thinkingContinue)
                        .disabled(locked)
                    ) : nil
                )
            }
        }
    }

    private var setupProgressView: some View {
        SetupStep(
            headline: JournalFirstRunCopy.setupTitle,
            explanations: [JournalFirstRunCopy.setupSubtitle]
        ) {
            SetupCard {
                VStack(alignment: .leading, spacing: 14) {
                    if model.errorMessage == nil {
                        ProgressView()
                            .controlSize(.large)
                            .accessibilityIdentifier(AXID.Journal.Ritual.setupProgress)
                    }

                    // The runtime's step names are internal ("brain", "doctor"); they stay in
                    // the accessibility companion and never show as owner text.
                    AXStateCompanion(id: AXID.Journal.Ritual.setupStepState, value: model.currentStep ?? "")

                    if !model.setupRenderedLog.isEmpty {
                        ScrollView {
                            Text(model.setupRenderedLog)
                                .font(.system(.caption, design: .monospaced))
                                .foregroundStyle(SetupChromeColor.softInk(for: colorScheme))
                                .frame(maxWidth: .infinity, alignment: .leading)
                        }
                        .frame(maxHeight: 160)
                    }
                    AXStateCompanion(id: AXID.Journal.Ritual.setupLogState, value: model.setupRenderedLog)

                    if let error = model.errorMessage {
                        SetupErrorText(error)
                            .fixedSize(horizontal: false, vertical: true)
                        AXStateCompanion(id: AXID.Journal.Ritual.setupErrorState, value: error)
                        Button(JournalFirstRunCopy.tryAgain) {
                            Task { await model.retrySetup() }
                        }
                        .buttonStyle(SetupSecondaryButtonStyle())
                        .accessibilityIdentifier(AXID.Journal.Ritual.setupRetry)
                    } else {
                        AXStateCompanion(id: AXID.Journal.Ritual.setupErrorState, value: "")
                    }
                }
            }
            .accessibilityElement(children: .contain)
            .accessibilityIdentifier(AXID.Journal.Ritual.card)
        } footer: {
            // Setting up never had a download footer; the rows belong to mark and thinking.
            // A journal that can't be set up or started can still be left for another one.
            stepFooter(offersReturn: model.errorMessage != nil)
        }
    }

    private var nameLocationView: some View {
        SetupStep(
            headline: JournalFirstRunCopy.nameLocationTitle,
            explanations: []
        ) {
            SetupCard {
                VStack(alignment: .leading, spacing: 14) {
                    Text(JournalFirstRunCopy.locationField)
                        .font(.system(size: 14, weight: .semibold))
                        .foregroundStyle(SetupChromeColor.softInk(for: colorScheme))

                    HStack(spacing: 8) {
                        TextField(
                            JournalFirstRunCopy.locationField,
                            text: Binding(
                                get: { model.journalRoot.path },
                                set: { model.journalRoot = URL(fileURLWithPath: $0, isDirectory: true) }
                            )
                        )
                        .font(.system(.body, design: .monospaced))
                        .textFieldStyle(.roundedBorder)
                        .accessibilityIdentifier(AXID.Journal.Ritual.locationField)

                        Button(JournalFirstRunCopy.chooseLocation) {
                            chooseJournalLocation()
                        }
                        .buttonStyle(SetupSecondaryButtonStyle())
                        .accessibilityIdentifier(AXID.Journal.Ritual.locationChoose)
                    }

                    if let error = model.errorMessage {
                        SetupErrorText(error)
                            .fixedSize(horizontal: false, vertical: true)
                        AXStateCompanion(id: AXID.Journal.Ritual.nameLocationErrorState, value: error)
                    } else {
                        AXStateCompanion(id: AXID.Journal.Ritual.nameLocationErrorState, value: "")
                    }

                    Button(JournalFirstRunCopy.continueButton) {
                        Task { await model.continueFromNameLocation() }
                    }
                    .buttonStyle(SetupPrimaryButtonStyle())
                    .keyboardShortcut(.defaultAction)
                    .accessibilityIdentifier(AXID.Journal.Ritual.nameLocationContinue)
                }
            }
            .accessibilityElement(children: .contain)
            .accessibilityIdentifier(AXID.Journal.Ritual.card)
        } footer: {
            stepFooter(offersReturn: true)
        }
    }

    private var finishInBrowserView: some View {
        SetupStep(
            headline: JournalFirstRunCopy.finishInBrowserTitle,
            explanations: []
        ) {
            SetupCard {
                VStack(alignment: .leading, spacing: 14) {
                    HStack(alignment: .firstTextBaseline, spacing: 10) {
                        ProgressView()
                            .controlSize(.small)
                        Text(JournalFirstRunCopy.finishInBrowserLine)
                            .font(.system(size: 14))
                            .foregroundStyle(SetupChromeColor.softInk(for: colorScheme))
                            .fixedSize(horizontal: false, vertical: true)
                    }

                    Button(JournalFirstRunCopy.reopenInBrowserButton) {
                        model.reopenInBrowser()
                    }
                    .buttonStyle(SetupPrimaryButtonStyle())
                    .keyboardShortcut(.defaultAction)
                    .accessibilityIdentifier(AXID.Journal.Ritual.browserReopen)
                }
            }
            .accessibilityElement(children: .contain)
            .accessibilityIdentifier(AXID.Journal.Ritual.card)
        } footer: {
            stepFooter(offersReturn: true)
        }
    }

    private var adoptView: some View {
        SetupStep(
            headline: JournalFirstRunCopy.adoptTitle,
            explanations: []
        ) {
            SetupCard {
                VStack(alignment: .leading, spacing: 14) {
                    if model.errorMessage == nil, model.adoptState != .landed {
                        ProgressView()
                            .controlSize(.large)
                    }

                    Text(model.adoptMessage ?? JournalFirstRunCopy.adoptOpening)
                        .font(.system(size: 14))
                        .foregroundStyle(SetupChromeColor.softInk(for: colorScheme))
                    AXStateCompanion(id: AXID.Journal.Adopt.statusState, value: model.adoptState.axToken)
                    AXStateCompanion(id: AXID.Journal.Adopt.messageState, value: model.adoptMessage ?? "")
                    AXStateCompanion(id: AXID.Journal.Adopt.locationPathState, value: model.journalRoot.path)

                    if let error = model.errorMessage {
                        SetupErrorText(error)
                            .fixedSize(horizontal: false, vertical: true)
                        AXStateCompanion(id: AXID.Journal.Adopt.errorState, value: error)
                        Button(JournalFirstRunCopy.tryAgain) {
                            Task { await model.retryAdopt() }
                        }
                        .buttonStyle(SetupSecondaryButtonStyle())
                        .accessibilityIdentifier(AXID.Journal.Adopt.continueButton)
                    } else {
                        AXStateCompanion(id: AXID.Journal.Adopt.errorState, value: "")
                    }
                }
            }
            .accessibilityElement(children: .contain)
            .accessibilityIdentifier(AXID.Journal.Ritual.card)
        } footer: {
            SetupFooter()
                .accessibilityElement(children: .contain)
                .accessibilityIdentifier(AXID.Journal.Ritual.footer)
        }
    }

    @ViewBuilder
    private func stepFooter(
        progress: JournalProgressInfo? = nil,
        back: Bool = false,
        offersReturn: Bool = false,
        downloads: Bool = false,
        primary: AnyView? = nil
    ) -> some View {
        SetupFooter(
            back: {
                HStack(spacing: 16) {
                    if back {
                        Button {
                            model.backFromFolder()
                        } label: {
                            HStack(spacing: 4) {
                                Image(systemName: "chevron.left")
                                Text(JournalFirstRunCopy.back)
                            }
                        }
                        .buttonStyle(SetupTextButtonStyle())
                        .accessibilityIdentifier(AXID.Journal.Ritual.back)
                    }
                    if offersReturn, let root = model.returnJournalRoot, model.canChangeJournal {
                        Button(JournalFirstRunCopy.goBack(to: root)) {
                            returnTarget = root
                        }
                        .buttonStyle(SetupTextButtonStyle())
                        .accessibilityIdentifier(AXID.Journal.Ritual.returnToJournal)
                    }
                }
            },
            progress: {
                if let progress {
                    SetupProgress(index: progress.index, count: progress.count, label: progress.label)
                        .accessibilityIdentifier(AXID.Journal.Ritual.progress)
                }
            },
            downloads: {
                if downloads, model.setupCopy != nil {
                    // The state companions sit beside the rows, so they are present even when no
                    // row shows (idle, complete), as they always were.
                    ZStack(alignment: .leading) {
                        AXStateCompanion(
                            id: AXID.Journal.Ritual.downloadSpeechState,
                            value: model.setupState?.downloads.speech.state.rawValue ?? ""
                        )
                        AXStateCompanion(
                            id: AXID.Journal.Ritual.downloadThinkingState,
                            value: model.setupState?.downloads.thinking.state.rawValue ?? ""
                        )
                        downloadRowsView
                    }
                }
            },
            primary: {
                // While an operation runs (a mark commit, the thinking save and hand-off, a
                // refusal's open), a small spinner shows beside the step's button, in place of the
                // window-wide one that sat in the band over the sun. It rides in the gap before
                // the button and takes no width, so the button keeps its line.
                if let primary {
                    primary
                        .overlay(alignment: .leading) {
                            if showsFooterBusy {
                                ProgressView()
                                    .controlSize(.mini)
                                    .frame(width: 12, height: 12)
                                    .offset(x: -14)
                                    .allowsHitTesting(false)
                            }
                        }
                } else if showsFooterBusy {
                    ProgressView()
                        .controlSize(.small)
                }
            }
        )
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier(AXID.Journal.Ritual.footer)
    }

    @ViewBuilder
    private var downloadRowsView: some View {
        if let copy = model.setupCopy {
            let locked = model.userOperationInFlight || model.pendingOperation != .none
            let speechRow = JournalSetupChrome.speechDownloadRow(
                job: model.setupState?.downloads.speech,
                copy: copy.downloads,
                speechFailure: model.speechFailure,
                canRetry: model.showsSpeechRetry,
                locked: locked,
                downloadLabel: { model.downloadLabel(for: $0) }
            )
            let thinkingRow = JournalSetupChrome.thinkingDownloadRow(
                job: model.setupState?.downloads.thinking,
                copy: copy.downloads,
                thinkingFailure: model.thinkingFailure,
                operationError: model.operationError,
                canRetry: model.showsThinkingRetry,
                canCancel: model.showsThinkingCancel,
                locked: locked,
                downloadLabel: { model.downloadLabel(for: $0) }
            )
            if speechRow != nil || thinkingRow != nil {
                VStack(alignment: .leading, spacing: 6) {
                    if let speechRow {
                        SetupDownloadRow(
                            name: speechRow.name,
                            detail: speechRow.detail,
                            fraction: speechRow.fraction,
                            indeterminate: speechRow.indeterminate,
                            actionTitle: speechRow.actionTitle,
                            actionEnabled: speechRow.actionEnabled,
                            actionIdentifier: speechRow.actionIdentifier,
                            action: { Task { await model.retrySpeechDownload() } }
                        )
                    }
                    if let thinkingRow {
                        SetupDownloadRow(
                            name: thinkingRow.name,
                            detail: thinkingRow.detail,
                            fraction: thinkingRow.fraction,
                            indeterminate: thinkingRow.indeterminate,
                            actionTitle: thinkingRow.actionTitle,
                            actionEnabled: thinkingRow.actionEnabled,
                            actionIdentifier: thinkingRow.actionIdentifier,
                            action: {
                                Task {
                                    if thinkingRow.actionKind == .cancelThinking {
                                        await model.cancelThinkingDownload()
                                    } else {
                                        await model.retryThinkingDownload()
                                    }
                                }
                            }
                        )
                    }
                }
            }
        }
    }

    private var showsFooterBusy: Bool {
        guard model.userOperationInFlight || model.pendingOperation != .none else { return false }
        // Only the setting-up card has its own spinner, and only while it shows no error. A
        // refusal notice or a hand-off error shown on the setting-up route replaces that card.
        let notice = model.handoffRefusal != nil || !(model.handoffError ?? "").isEmpty
        let cardSpinner = model.route == .ritual(.setupProgress) && !notice && model.errorMessage == nil
        return !cardSpinner
    }

    private var showsDownloadFooter: Bool {
        guard model.setupState != nil else { return false }
        switch model.route {
        case .ritual(.mark), .ritual(.thinking), .ritual(.setupProgress): return true
        default: return false
        }
    }

    private func chooseJournalLocation() {
        let panel = NSOpenPanel()
        panel.canChooseFiles = false
        panel.canChooseDirectories = true
        panel.canCreateDirectories = true
        panel.allowsMultipleSelection = false
        panel.directoryURL = model.journalRoot.deletingLastPathComponent()
        if panel.runModal() == .OK, let url = panel.url {
            model.journalRoot = url.standardizedFileURL
        }
    }
}

private struct ThinkingChoiceIdentifier: ViewModifier {
    let choiceID: String

    func body(content: Content) -> some View {
        if let identifier = AXID.Journal.Ritual.thinkingChoice(choiceID) {
            content.accessibilityIdentifier(identifier)
        } else {
            content
        }
    }
}
