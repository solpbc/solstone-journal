// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import JournalRuntime
import Foundation

/// The location pane's words for keeping another journal on this mac.
enum JournalLocationCopy {
    static let startNewJournal = JournalCopy.locationStartNewJournal()
    static let startNewJournalHelp = JournalCopy.locationStartNewJournalHelp()
    static let startNewJournalConfirmTitle = JournalCopy.locationStartNewJournalConfirmTitle()
    static let startNewJournalConfirmTemplate = JournalCopy.locationStartNewJournalConfirmTemplate("{path}")
    static let otherJournals = JournalCopy.locationOtherJournals()
    static let switchToJournal = JournalCopy.locationSwitchToJournal()
    static let switchConfirmTitleTemplate = JournalCopy.locationSwitchConfirmTitleTemplate("{path}")
    static let switchConfirmTemplate = JournalCopy.locationSwitchConfirmTemplate("{path}")
    static let sharedHostedTemplate = JournalCopy.locationSharedHostedTemplate("{path}")
    static let newJournalHostedShare = JournalCopy.locationNewJournalHostedShare()
    static let sameMarkTemplate = JournalCopy.locationSameMarkTemplate()
    static let leaveSetupConfirm = JournalCopy.locationLeaveSetupConfirm()
    static let switchButton = JournalCopy.locationSwitchButton()
    static let cancel = JournalCopy.locationCancel()

    static func startNewJournalConfirm(current: URL) -> String {
        JournalCopy.locationStartNewJournalConfirmTemplate(displayPath(current))
    }

    static func switchConfirmTitle(target: URL) -> String {
        JournalCopy.locationSwitchConfirmTitleTemplate(displayPath(target))
    }

    static func switchConfirm(current: URL) -> String {
        JournalCopy.locationSwitchConfirmTemplate(displayPath(current))
    }

    /// Only sol pbc's storage has something to say: one journal at a time can use it. On the
    /// owner's own storage a restore keeps the backups made before it, so nothing is lost there.
    static func shared(_ share: JournalBackupShare) -> String? {
        switch share.place {
        case .hosted:
            return JournalCopy.locationSharedHostedTemplate(displayPath(share.other))
        case .own:
            return nil
        }
    }

    /// A folder as the owner reads it in Finder's path bar: home shown as `~`.
    static func displayPath(_ root: URL) -> String {
        (root.standardizedFileURL.path as NSString).abbreviatingWithTildeInPath
    }

    static let all = [
        startNewJournal,
        startNewJournalHelp,
        startNewJournalConfirmTitle,
        startNewJournalConfirmTemplate,
        otherJournals,
        switchToJournal,
        switchConfirmTitleTemplate,
        switchConfirmTemplate,
        sharedHostedTemplate,
        newJournalHostedShare,
        sameMarkTemplate,
        leaveSetupConfirm,
        switchButton,
        cancel,
    ]
}
