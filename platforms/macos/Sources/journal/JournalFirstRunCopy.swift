// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import JournalRuntime
import Foundation

enum JournalFirstRunCopy {
    static let nameLocationTitle = JournalCopy.setupNameLocationTitle()
    static let adoptLandingLine = JournalCopy.setupAdoptLandingLine()

    static let locationField = JournalCopy.setupLocationField()
    static let chooseLocation = JournalCopy.setupChooseLocation()
    static let continueButton = JournalCopy.setupContinueButton()
    static let restoreFromBackup = JournalCopy.setupRestoreFromBackup()
    static let setupTitle = JournalCopy.setupSetupTitle()
    static let setupSubtitle = JournalCopy.setupSetupSubtitle()
    static let finishInBrowserTitle = JournalCopy.setupFinishInBrowserTitle()
    static let finishInBrowserLine = JournalCopy.setupFinishInBrowserLine()
    static let reopenInBrowserButton = JournalCopy.setupReopenInBrowserButton()
    static let adoptTitle = JournalCopy.setupAdoptTitle()
    static let adoptOpening = JournalCopy.setupAdoptOpening()
    static let adoptFailed = JournalCopy.setupAdoptFailed()
    static let tryAgain = JournalCopy.setupTryAgain()
    static let setupProgressNotSaved = JournalCopy.setupSetupProgressNotSaved()
    static let loginItemChangeFailed = JournalCopy.setupLoginItemChangeFailed()
    static let loginItemUnavailable = JournalCopy.setupLoginItemUnavailable()
    static let handoffHeldPairing = JournalCopy.setupHandoffHeldPairing()
    static let handoffCompetingSetup = JournalCopy.setupHandoffCompetingSetup()
    static let handoffPending = JournalCopy.setupHandoffPending()
    static let openYourJournal = JournalCopy.setupOpenYourJournal()
    static let handoffNotDelivered = JournalCopy.setupHandoffNotDelivered()
    static let thinkingFailed = JournalCopy.setupThinkingFailed()

    static let setUpSolstone = JournalCopy.setupSetUpSolstone()
    static let yourJournal = JournalCopy.setupYourJournal()
    static let whatThisSetsUp = JournalCopy.setupWhatThisSetsUp()
    static let whatThisSetsUpJournal = JournalCopy.setupWhatThisSetsUpJournal()
    static let theSolstoneApp = JournalCopy.setupTheSolstoneApp()
    static let theSolstoneAppDescription = JournalCopy.setupTheSolstoneAppDescription()
    static let thinking = JournalCopy.setupThinking()
    static let thinkingDescription = JournalCopy.setupThinkingDescription()
    static let startAutomaticallyHelp = JournalCopy.setupStartAutomaticallyHelp()
    static let startSetup = JournalCopy.setupStartSetup()
    static let whereYourJournalLives = JournalCopy.setupWhereYourJournalLives()
    static let whereYourJournalLivesDescription = JournalCopy.setupWhereYourJournalLivesDescription()
    static let journalFolder = JournalCopy.setupJournalFolder()
    static let whereDoesThinkingHappen = JournalCopy.setupWhereDoesThinkingHappen()
    static let whereDoesThinkingHappenDescription = JournalCopy.setupWhereDoesThinkingHappenDescription()
    static let onThisMac = JournalCopy.setupOnThisMac()
    static let onThisMacDescription = JournalCopy.setupOnThisMacDescription()
    static let continueInTheSolstoneApp = JournalCopy.setupContinueInTheSolstoneApp()
    static let back = JournalCopy.setupBack()
    static let stepProgressTemplate = JournalCopy.setupStepProgress("{n}")
    static let folderHoldsAnotherJournal = JournalCopy.setupFolderHoldsAnotherJournal()
    static let goBackTemplate = JournalCopy.setupGoBackToTheJournalIn("{path}")

    static func goBack(to root: URL) -> String {
        JournalCopy.setupGoBackToTheJournalIn(JournalLocationCopy.displayPath(root))
    }

    static func stepProgress(_ n: Int) -> String {
        JournalCopy.setupStepProgress(String(n))
    }

    static let all = [
        nameLocationTitle,
        adoptLandingLine,
        locationField,
        chooseLocation,
        continueButton,
        setupTitle,
        setupSubtitle,
        finishInBrowserTitle,
        finishInBrowserLine,
        reopenInBrowserButton,
        adoptTitle,
        adoptOpening,
        adoptFailed,
        tryAgain,
        setupProgressNotSaved,
        loginItemChangeFailed,
        loginItemUnavailable,
        handoffNotDelivered,
        handoffHeldPairing,
        handoffCompetingSetup,
        handoffPending,
        openYourJournal,
        thinkingFailed,
        setUpSolstone,
        yourJournal,
        whatThisSetsUp,
        whatThisSetsUpJournal,
        theSolstoneApp,
        theSolstoneAppDescription,
        thinking,
        thinkingDescription,
        startAutomaticallyHelp,
        startSetup,
        whereYourJournalLives,
        whereYourJournalLivesDescription,
        journalFolder,
        whereDoesThinkingHappen,
        whereDoesThinkingHappenDescription,
        onThisMac,
        onThisMacDescription,
        continueInTheSolstoneApp,
        back,
        stepProgressTemplate,
        folderHoldsAnotherJournal,
        goBackTemplate,
    ]
}
