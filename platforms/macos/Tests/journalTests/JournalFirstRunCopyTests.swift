// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import Testing
@testable import journal

@Suite("JournalFirstRunCopy")
struct JournalFirstRunCopyTests {
    @Test func constantsMatchPinnedCopy() {
        #expect(JournalFirstRunCopy.nameLocationTitle == "let's create your journal on this mac")
        #expect(JournalFirstRunCopy.adoptLandingLine == "nothing moved. your journal was always here. you know it by its mark.")
        #expect(JournalFirstRunCopy.locationField == "location")
        #expect(JournalFirstRunCopy.chooseLocation == "choose")
        #expect(JournalFirstRunCopy.continueButton == "continue")
        #expect(JournalFirstRunCopy.setupTitle == "setting up your journal")
        #expect(JournalFirstRunCopy.setupSubtitle == "this can take a minute.")
        #expect(JournalFirstRunCopy.finishInBrowserTitle == "finish setting up in your browser")
        #expect(JournalFirstRunCopy.finishInBrowserLine == "this window moves on when you finish.")
        #expect(JournalFirstRunCopy.reopenInBrowserButton == "open the setup page")
        #expect(JournalFirstRunCopy.adoptTitle == "journal found")
        #expect(JournalFirstRunCopy.adoptOpening == "opening this journal")
        #expect(JournalFirstRunCopy.adoptFailed == "couldn't open this journal")
        #expect(JournalFirstRunCopy.tryAgain == "try again")
        #expect(JournalFirstRunCopy.setUpSolstone == "set up solstone")
        #expect(JournalFirstRunCopy.yourJournal == "your journal")
        #expect(JournalFirstRunCopy.whatThisSetsUp == "what this sets up")
        #expect(JournalFirstRunCopy.whatThisSetsUpJournal == "lives on this mac and keeps everything that goes into it.")
        #expect(JournalFirstRunCopy.theSolstoneApp == "the solstone app")
        #expect(JournalFirstRunCopy.theSolstoneAppDescription == "runs on this mac. your microphone, screen and computer sound each stay off until you turn them on.")
        #expect(JournalFirstRunCopy.thinking == "thinking")
        #expect(JournalFirstRunCopy.thinkingDescription == "helps your journal make sense of what it keeps. you choose where during setup, or later.")
        #expect(JournalFirstRunCopy.startAutomaticallyHelp == "start automatically after you sign in to this mac.")
        #expect(JournalFirstRunCopy.startSetup == "start setup")
        #expect(JournalFirstRunCopy.whereYourJournalLives == "where your journal lives")
        #expect(JournalFirstRunCopy.whereYourJournalLivesDescription == "everything your journal keeps stays in this folder, on this mac.")
        #expect(JournalFirstRunCopy.journalFolder == "journal folder")
        #expect(JournalFirstRunCopy.whereDoesThinkingHappen == "where does the thinking happen?")
        #expect(JournalFirstRunCopy.whereDoesThinkingHappenDescription == "your journal stays on this mac whatever you choose. this only decides where thinking happens.")
        #expect(JournalFirstRunCopy.onThisMac == "on this mac")
        #expect(JournalFirstRunCopy.onThisMacDescription == "model requests stay on this mac. continue starts the download.")
        #expect(JournalFirstRunCopy.continueInTheSolstoneApp == "continue in the solstone app")
        #expect(JournalFirstRunCopy.back == "back")
        #expect(JournalFirstRunCopy.stepProgressTemplate == "step {n} of 7")
        #expect(JournalFirstRunCopy.stepProgress(1) == "step 1 of 7")
    }

    @Test func firstRunCopyIsNonEmpty() {
        for copy in JournalFirstRunCopy.all {
            #expect(!copy.isEmpty)
        }
    }
}
