// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import Foundation

/// The location pane's words for keeping another journal on this mac.
enum JournalLocationCopy {
    static let startNewJournal = "start a new journal"
    static let startNewJournalHelp = "your journal stops and stays in its folder, unchanged. during setup you can restore from a backup, and you can come back to this one here."
    static let startNewJournalConfirmTitle = "start a new journal?"
    static let startNewJournalConfirmTemplate = "your journal in {path} stops and stays there, unchanged. setup starts next, where you can restore from a backup."
    static let otherJournals = "other journals on this mac"
    static let switchToJournal = "switch to this journal"
    static let switchConfirmTitleTemplate = "switch to the journal in {path}?"
    static let switchConfirmTemplate = "your journal in {path} stops and stays there, unchanged. you can come back to it here."
    static let sharedHostedTemplate = "the one in {path} shares this encrypted backup operated by sol pbc, and only one can use it. if this journal stops backing up, turn it on again in the backup pane."
    static let newJournalHostedShare = "if you restore this journal's backup there, the new journal takes it over, and this one stops backing up until you turn it on again."
    static let sameMarkTemplate = "both have the same mark, so your devices reach whichever one is running."
    static let leaveSetupConfirm = "setup stops."
    static let switchButton = "switch"
    static let cancel = "cancel"

    static func startNewJournalConfirm(current: URL) -> String {
        startNewJournalConfirmTemplate.replacingOccurrences(of: "{path}", with: displayPath(current))
    }

    static func switchConfirmTitle(target: URL) -> String {
        switchConfirmTitleTemplate.replacingOccurrences(of: "{path}", with: displayPath(target))
    }

    static func switchConfirm(current: URL) -> String {
        switchConfirmTemplate.replacingOccurrences(of: "{path}", with: displayPath(current))
    }

    /// Only sol pbc's storage has something to say: one journal at a time can use it. On the
    /// owner's own storage a restore keeps the backups made before it, so nothing is lost there.
    static func shared(_ share: JournalBackupShare) -> String? {
        switch share.place {
        case .hosted:
            return sharedHostedTemplate.replacingOccurrences(of: "{path}", with: displayPath(share.other))
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
