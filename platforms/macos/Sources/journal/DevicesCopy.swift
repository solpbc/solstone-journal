// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import JournalRuntime
import Foundation

enum DevicesCopy {
    static let paneTitle = JournalCopy.devicesPaneTitle()
    static let yourDevicesHeader = JournalCopy.devicesYourDevicesHeader()
    static let peerJournalsHeader = JournalCopy.devicesPeerJournalsHeader()
    static let loading = JournalCopy.devicesLoading()
    static let emptyTitle = JournalCopy.devicesEmptyTitle()
    static let emptyBody = JournalCopy.devicesEmptyBody()
    static let notRunningTitle = JournalCopy.devicesNotRunningTitle()
    static let notRunningBody = JournalCopy.devicesNotRunningBody()
    static let notReadyTitle = JournalCopy.devicesNotReadyTitle()
    static let notReadyBody = JournalCopy.devicesNotReadyBody()
    static let tryAgain = JournalCopy.devicesTryAgain()
    static let addDevice = JournalCopy.devicesAddDevice()
    static let unnamedDevice = JournalCopy.devicesUnnamedDevice()
    static let unnamedJournal = JournalCopy.devicesUnnamedJournal()
    static let remove = JournalCopy.devicesRemove()
    static let revokeFailed = JournalCopy.devicesRevokeFailed()
    static let cancel = JournalCopy.devicesCancel()
    static let pairingTitle = JournalCopy.devicesPairingTitle()
    static let pairingOpening = JournalCopy.devicesPairingOpening()
    static let pairingInstructions = JournalCopy.devicesPairingInstructions()
    static let pairingLinkLabel = JournalCopy.devicesPairingLinkLabel()
    static let copyLink = JournalCopy.devicesCopyLink()
    static let copied = JournalCopy.devicesCopied()
    static let pairingCode = JournalCopy.devicesPairingCode()
    static let linkExpiredTitle = JournalCopy.devicesLinkExpiredTitle()
    static let linkExpiredBody = JournalCopy.devicesLinkExpiredBody()
    static let openFreshLink = JournalCopy.devicesOpenFreshLink()
    static let pairingFailedTitle = JournalCopy.devicesPairingFailedTitle()
    static let close = JournalCopy.devicesClose()
    static let pairedHeadingUnnamed = JournalCopy.devicesPairedHeadingUnnamed()
    static let pairedNote = JournalCopy.devicesPairedNote()
    static let pairedNoteAnywhere = JournalCopy.devicesPairedNoteAnywhere()
    static let pairedDone = JournalCopy.devicesPairedDone()
    static let pairedUnpair = JournalCopy.devicesPairedUnpair()
    static let pairedRemoved = JournalCopy.devicesPairedRemoved()
    static let pairedRemoveFailed = JournalCopy.devicesPairedRemoveFailed()

    static func pairedHeading(_ label: String) -> String {
        JournalCopy.devicesPairedHeading(label)
    }

    static func revokeTitle(_ name: String) -> String {
        JournalCopy.devicesRevokeTitle(name)
    }

    static func revokeBody(_ name: String, detail: String) -> String {
        JournalCopy.devicesRevokeBody(name, detail)
    }

    static func countdown(seconds: Int) -> String {
        let clamped = max(0, seconds)
        return JournalCopy.devicesExpiresIn(String(clamped / 60), String(format: "%02d", clamped % 60))
    }
}
