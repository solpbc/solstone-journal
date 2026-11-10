// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import Foundation
import UpdateKit

enum AXContract {
    static let idPattern = #"^(journal|updates)(\.[a-z][a-zA-Z0-9-]*)+$"#
    static let tokenPattern = #"^[a-z][a-z_]*$"#

    private static let generatedMarker =
        "DO NOT EDIT. Generated from Sources/journal/AXID.swift + AXToken.swift. Run `make ax-contract` to regenerate; `make ci` fails on drift."

    static let staticIDs: [String] = [
        AXID.Journal.Home.updates,
        AXID.Journal.Home.markCard,
        AXID.Journal.Home.runDisplayGlanceState,
        AXID.Journal.Home.openJournal,
        AXID.Journal.Home.unconfiguredMessageState,
        AXID.Journal.Home.start,
        AXID.Journal.Home.runState,
        AXID.Journal.Ritual.root,
        AXID.Journal.Ritual.routeState,
        AXID.Journal.Ritual.locationField,
        AXID.Journal.Ritual.locationChoose,
        AXID.Journal.Ritual.nameLocationContinue,
        AXID.Journal.Ritual.nameLocationErrorState,
        AXID.Journal.Ritual.setupProgress,
        AXID.Journal.Ritual.setupStepState,
        AXID.Journal.Ritual.setupLogState,
        AXID.Journal.Ritual.setupErrorState,
        AXID.Journal.Ritual.setupRetry,
        AXID.Journal.Ritual.browserReopen,
        AXID.Journal.Ritual.welcomeContinue,
        AXID.Journal.Ritual.loginToggle,
        AXID.Journal.Ritual.loginFailureState,
        AXID.Journal.Ritual.folderField,
        AXID.Journal.Ritual.folderChoose,
        AXID.Journal.Ritual.folderContinue,
        AXID.Journal.Ritual.markContinue,
        AXID.Journal.Ritual.markRegenerate,
        AXID.Journal.Ritual.markProposalState,
        AXID.Journal.Ritual.markErrorState,
        AXID.Journal.Ritual.thinkingContinue,
        AXID.Journal.Ritual.thinkingLater,
        AXID.Journal.Ritual.thinkingChoiceState,
        AXID.Journal.Ritual.thinkingErrorState,
        AXID.Journal.Ritual.downloadSpeechState,
        AXID.Journal.Ritual.downloadThinkingState,
        AXID.Journal.Ritual.downloadSpeechRetry,
        AXID.Journal.Ritual.downloadThinkingRetry,
        AXID.Journal.Ritual.downloadThinkingCancel,
        AXID.Journal.Ritual.operationPendingState,
        AXID.Journal.Ritual.operationErrorState,
        AXID.Journal.Ritual.outboundState,
        AXID.Journal.Ritual.handoffErrorState,
        AXID.Journal.Ritual.handoffRetry,
        AXID.Journal.Ritual.back,
        AXID.Journal.Ritual.returnToJournal,
        AXID.Journal.Ritual.progress,
        AXID.Journal.Ritual.card,
        AXID.Journal.Ritual.footer,
        AXID.Journal.Adopt.root,
        AXID.Journal.Adopt.statusState,
        AXID.Journal.Adopt.messageState,
        AXID.Journal.Adopt.locationPathState,
        AXID.Journal.Adopt.continueButton,
        AXID.Journal.Adopt.errorState,
        AXID.Journal.Pane.locationPathState,
        AXID.Journal.Pane.diskUsageState,
        AXID.Journal.Pane.startNewJournal,
        AXID.Journal.Pane.startNewJournalConfirm,
        AXID.Journal.Pane.switchJournal,
        AXID.Journal.Pane.switchJournalConfirm,
        AXID.Journal.Pane.otherJournalsState,
        AXID.Journal.RunState.start,
        AXID.Journal.RunState.stop,
        AXID.Journal.RunState.restart,
        AXID.Journal.RunState.displayState,
        AXID.Journal.RunState.blockedReasonState,
        AXID.Journal.RunState.healthState,
        AXID.Journal.RunState.aboutState,
        AXID.Journal.RunState.aboutCopy,
        AXID.Journal.RunState.aboutCopyFeedbackState,
        AXID.Journal.Backup.openBackup,
        AXID.Journal.Backup.messageState,
        AXID.Journal.Startup.launchAtLogin,
        AXID.Journal.Startup.launchAtLoginState,
        AXID.Journal.Devices.root,
        AXID.Journal.Devices.loadState,
        AXID.Journal.Devices.yourDevicesHeader,
        AXID.Journal.Devices.yourDevicesCountState,
        AXID.Journal.Devices.peerJournalsHeader,
        AXID.Journal.Devices.peerJournalsCountState,
        AXID.Journal.Devices.addDevice,
        AXID.Journal.Devices.Pairing.sheet,
        AXID.Journal.Devices.Pairing.markCard,
        AXID.Journal.Devices.Pairing.linkField,
        AXID.Journal.Devices.Pairing.copyLink,
        AXID.Journal.Devices.Pairing.copyLinkCopiedState,
        AXID.Journal.Devices.Pairing.qr,
        AXID.Journal.Devices.Pairing.countdown,
        AXID.Journal.Devices.Pairing.countdownState,
        AXID.Journal.Devices.Pairing.status,
        AXID.Journal.Devices.Pairing.statusState,
        AXID.Journal.Devices.Pairing.reopen,
        AXID.Journal.Devices.Pairing.unpair,
        AXID.Journal.Devices.Pairing.unpairState,
        AXID.Journal.Devices.RevokeConfirm.dialog,
        AXID.Journal.Devices.RevokeConfirm.messageState,
        AXID.Journal.Devices.RevokeConfirm.confirm,
        AXID.Journal.Devices.RevokeConfirm.cancel,
        UpdatesAXID.statusState,
        UpdatesAXID.unavailable,
        UpdatesAXID.notRunning,
        UpdatesAXID.notRunningRetry,
        UpdatesAXID.notRunningReason,
        UpdatesAXID.check,
        UpdatesAXID.checkState,
        UpdatesAXID.cancel,
        UpdatesAXID.download,
        UpdatesAXID.downloadState,
        UpdatesAXID.install,
        UpdatesAXID.dismiss,
        UpdatesAXID.dismissStaged,
        UpdatesAXID.retry,
        UpdatesAXID.retryState,
        UpdatesAXID.checkAgainState,
        UpdatesAXID.releaseNotes,
        UpdatesAXID.releaseNotesOnline,
        UpdatesAXID.downloadProgress,
        UpdatesAXID.extractProgress,
        UpdatesAXID.deferredInstallState,
        UpdatesAXID.automaticChecks,
        UpdatesAXID.enableAutomaticChecks,
        UpdatesAXID.frequencyPicker,
        UpdatesAXID.frequencyState,
        UpdatesAXID.automaticDownloads,
        UpdatesAXID.debugStatePicker
    ]

    static var enumerableIDs: [String] {
        var ids = staticIDs
        for pane in JournalPane.allCases {
            ids.append(AXID.Journal.Sidebar.tab(pane))
            ids.append(AXID.Journal.Sidebar.tabState(pane))
        }
        return ids
    }

    static var parameterizedTemplates: [ParameterizedIdentifier] {
        [
            ParameterizedIdentifier(
                template: "journal.sidebar.tab.{pane}",
                key: "JournalPane.rawValue",
                expansions: JournalPane.allCases.map(\.rawValue)
            ),
            ParameterizedIdentifier(
                template: "journal.sidebar.tab.{pane}.state",
                key: "JournalPane.rawValue",
                expansions: JournalPane.allCases.map(\.rawValue)
            ),
            ParameterizedIdentifier(
                template: "journal.devices.row.fingerprint-{fingerprint}",
                key: "DeviceRow.fingerprint",
                runtime: true
            ),
            ParameterizedIdentifier(
                template: "journal.devices.row.fingerprint-{fingerprint}.label",
                key: "DeviceRow.fingerprint",
                runtime: true
            ),
            ParameterizedIdentifier(
                template: "journal.devices.row.fingerprint-{fingerprint}.detail.state",
                key: "DeviceRow.fingerprint",
                runtime: true
            ),
            ParameterizedIdentifier(
                template: "journal.devices.row.fingerprint-{fingerprint}.revoke",
                key: "DeviceRow.fingerprint",
                runtime: true
            ),
            ParameterizedIdentifier(
                template: "journal.ritual.thinking.choice.{choice}",
                key: "ThinkingChoice.id",
                runtime: true
            )
        ].sorted { $0.template < $1.template }
    }

    static var vocabularies: [String: [String]] {
        [
            "JournalSidebarTabState": JournalSidebarTabState.axTokens,
            "JournalRunDisplay": JournalRunDisplay.axTokens,
            "JournalHealthDisplay": JournalHealthDisplay.axTokens,
            "JournalEnabledState": JournalEnabledState.axTokens,
            "JournalDevicesLoadState": JournalDevicesLoadState.axTokens,
            "JournalDevicesPairingState": PairingState.axTokens,
            "JournalDevicesCopiedState": JournalDevicesCopiedState.axTokens,
            "JournalPairedUnpairState": JournalPairedUnpairState.axTokens,
            "JournalFirstRunRouteState": JournalFirstRunRouteState.axTokens,
            "JournalAdoptState": JournalAdoptState.axTokens,
            "JournalRitualOperationToken": JournalRitualOperationToken.axTokens,
            "JournalOutboundToken": JournalOutboundToken.axTokens,
            "UpdateActivity": UpdateActivity.axTokens,
            "FrequencyOption": FrequencyOption.allCases.map(\.rawValue),
            "UpdateStatus": UpdateStatus.axTokens
        ]
    }

    static var states: [String: StateBinding] {
        [
            "journal.sidebar.tab.{pane}.state": .enum("JournalSidebarTabState"),
            AXID.Journal.Home.runDisplayGlanceState: .enum("JournalRunDisplay"),
            AXID.Journal.Home.unconfiguredMessageState: .freeform,
            AXID.Journal.Ritual.routeState: .enum("JournalFirstRunRouteState"),
            AXID.Journal.Ritual.nameLocationErrorState: .freeform,
            AXID.Journal.Ritual.setupStepState: .freeform,
            AXID.Journal.Ritual.setupLogState: .freeform,
            AXID.Journal.Ritual.setupErrorState: .freeform,
            AXID.Journal.Ritual.loginFailureState: .freeform,
            AXID.Journal.Ritual.markProposalState: .freeform,
            AXID.Journal.Ritual.markErrorState: .freeform,
            AXID.Journal.Ritual.thinkingChoiceState: .freeform,
            AXID.Journal.Ritual.thinkingErrorState: .freeform,
            AXID.Journal.Ritual.downloadSpeechState: .freeform,
            AXID.Journal.Ritual.downloadThinkingState: .freeform,
            AXID.Journal.Ritual.handoffErrorState: .freeform,
            AXID.Journal.Ritual.operationPendingState: .enum("JournalRitualOperationToken"),
            AXID.Journal.Ritual.operationErrorState: .enum("JournalRitualOperationToken"),
            AXID.Journal.Ritual.outboundState: .enum("JournalOutboundToken"),
            AXID.Journal.Adopt.statusState: .enum("JournalAdoptState"),
            AXID.Journal.Adopt.messageState: .freeform,
            AXID.Journal.Adopt.locationPathState: .freeform,
            AXID.Journal.Adopt.errorState: .freeform,
            AXID.Journal.Pane.locationPathState: .freeform,
            AXID.Journal.Pane.diskUsageState: .numeric,
            AXID.Journal.Pane.otherJournalsState: .freeform,
            AXID.Journal.RunState.displayState: .enum("JournalRunDisplay"),
            AXID.Journal.RunState.blockedReasonState: .freeform,
            AXID.Journal.RunState.healthState: .enum("JournalHealthDisplay"),
            AXID.Journal.RunState.aboutState: .freeform,
            AXID.Journal.RunState.aboutCopyFeedbackState: .freeform,
            AXID.Journal.Backup.messageState: .freeform,
            AXID.Journal.Startup.launchAtLoginState: .enum("JournalEnabledState"),
            AXID.Journal.Devices.loadState: .enum("JournalDevicesLoadState"),
            AXID.Journal.Devices.yourDevicesCountState: .numeric,
            AXID.Journal.Devices.peerJournalsCountState: .numeric,
            "journal.devices.row.fingerprint-{fingerprint}.detail.state": .freeform,
            AXID.Journal.Devices.Pairing.copyLinkCopiedState: .enum("JournalDevicesCopiedState"),
            AXID.Journal.Devices.Pairing.countdownState: .numeric,
            AXID.Journal.Devices.Pairing.statusState: .enum("JournalDevicesPairingState"),
            AXID.Journal.Devices.Pairing.unpairState: .enum("JournalPairedUnpairState"),
            AXID.Journal.Devices.RevokeConfirm.messageState: .freeform,
            UpdatesAXID.statusState: .enum("UpdateStatus"),
            UpdatesAXID.checkState: .freeform,
            UpdatesAXID.downloadState: .freeform,
            UpdatesAXID.retryState: .freeform,
            UpdatesAXID.checkAgainState: .freeform,
            UpdatesAXID.downloadProgress: .numeric,
            UpdatesAXID.extractProgress: .numeric,
            UpdatesAXID.deferredInstallState: .freeform,
            UpdatesAXID.frequencyState: .enum("FrequencyOption"),
            // updates.debug.state is a DEBUG picker control, not a product state-value companion.
            UpdatesAXID.debugStatePicker: .freeform
        ]
    }

    static var requiredStateKeys: Set<String> {
        var keys = Set(
            enumerableIDs
                .filter { $0.hasSuffix(".state") }
                .map(stateKey(for:))
        )
        for template in parameterizedTemplates where template.template.hasSuffix(".state") {
            keys.insert(template.template)
        }
        return keys
    }

    static func stateKey(for id: String) -> String {
        if id.hasPrefix("journal.sidebar.tab."), id.hasSuffix(".state") {
            return "journal.sidebar.tab.{pane}.state"
        }
        if id.hasPrefix("journal.devices.row.fingerprint-"), id.hasSuffix(".detail.state") {
            return "journal.devices.row.fingerprint-{fingerprint}.detail.state"
        }
        return id
    }

    static func generate() -> String {
        let contract = Contract(
            generated: generatedMarker,
            version: 1,
            grammar: Grammar(identifier: idPattern, token: tokenPattern),
            surfaces: ["journal", "updates"],
            vocabularies: vocabularies,
            identifiers: IdentifierSet(
                staticIDs: staticIDs.sorted(),
                parameterized: parameterizedTemplates
            ),
            states: states
        )

        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes]
        let data = try! encoder.encode(contract)
        return String(data: data, encoding: .utf8)! + "\n"
    }
}

struct ParameterizedIdentifier: Codable, Equatable {
    let template: String
    let key: String
    let expansions: [String]?
    let runtime: Bool?

    init(template: String, key: String, expansions: [String]) {
        self.template = template
        self.key = key
        self.expansions = expansions
        self.runtime = nil
    }

    init(template: String, key: String, runtime: Bool) {
        self.template = template
        self.key = key
        self.expansions = nil
        self.runtime = runtime
    }
}

struct StateBinding: Codable, Equatable {
    enum Kind: String, Codable {
        case `enum`
        case numeric
        case freeform
    }

    let kind: Kind
    let vocabulary: String?

    static func `enum`(_ vocabulary: String) -> StateBinding {
        StateBinding(kind: .enum, vocabulary: vocabulary)
    }

    static let numeric = StateBinding(kind: .numeric, vocabulary: nil)
    static let freeform = StateBinding(kind: .freeform, vocabulary: nil)
}

private struct Contract: Codable {
    let generated: String
    let version: Int
    let grammar: Grammar
    let surfaces: [String]
    let vocabularies: [String: [String]]
    let identifiers: IdentifierSet
    let states: [String: StateBinding]

    enum CodingKeys: String, CodingKey {
        case generated = "_generated"
        case version
        case grammar
        case surfaces
        case vocabularies
        case identifiers
        case states
    }
}

private struct Grammar: Codable {
    let identifier: String
    let token: String
}

private struct IdentifierSet: Codable {
    let staticIDs: [String]
    let parameterized: [ParameterizedIdentifier]

    enum CodingKeys: String, CodingKey {
        case staticIDs = "static"
        case parameterized
    }
}
