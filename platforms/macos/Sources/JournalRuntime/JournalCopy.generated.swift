// Generated from the Journal for Mac catalog by scripts/catalog.py.
extension JournalCopy {
    static let packagedLanguages = ["en-US"]

    public static func adminTerminalFailureTitle() -> String {
        render("m6a6f75726e616c2e6d61632e61646d696e2e7465726d696e616c2e6661696c7572652e7469746c65", [])
    }

    public static func adminTerminalMenuItemTitle() -> String {
        render("m6a6f75726e616c2e6d61632e61646d696e2e7465726d696e616c2e6d656e752e6974656d2e7469746c65", [])
    }

    public static func appJournal() -> String {
        render("m6a6f75726e616c2e6d61632e6170702e6a6f75726e616c", [])
    }

    public static func backupSetupHint() -> String {
        render("m6a6f75726e616c2e6d61632e6261636b75702e73657475702e68696e74", [])
    }

    public static func connectionInvalidResponse() -> String {
        render("m6a6f75726e616c2e6d61632e636f6e6e656374696f6e2e696e76616c69642e726573706f6e7365", [])
    }

    public static func connectionLinkFailed() -> String {
        render("m6a6f75726e616c2e6d61632e636f6e6e656374696f6e2e6c696e6b2e6661696c6564", [])
    }

    public static func connectionRefused() -> String {
        render("m6a6f75726e616c2e6d61632e636f6e6e656374696f6e2e72656675736564", [])
    }

    public static func connectionTimedOut() -> String {
        render("m6a6f75726e616c2e6d61632e636f6e6e656374696f6e2e74696d65642e6f7574", [])
    }

    public static func connectionUnavailable() -> String {
        render("m6a6f75726e616c2e6d61632e636f6e6e656374696f6e2e756e617661696c61626c65", [])
    }

    public static func devicesAddDevice() -> String {
        render("m6a6f75726e616c2e6d61632e646576696365732e6164642e646576696365", [])
    }

    public static func devicesCancel() -> String {
        render("m6a6f75726e616c2e6d61632e646576696365732e63616e63656c", [])
    }

    public static func devicesClose() -> String {
        render("m6a6f75726e616c2e6d61632e646576696365732e636c6f7365", [])
    }

    public static func devicesCopied() -> String {
        render("m6a6f75726e616c2e6d61632e646576696365732e636f70696564", [])
    }

    public static func devicesCopyLink() -> String {
        render("m6a6f75726e616c2e6d61632e646576696365732e636f70792e6c696e6b", [])
    }

    public static func devicesDetailNetwork(_ network: String, _ activity: String) -> String {
        render("m6a6f75726e616c2e6d61632e646576696365732e64657461696c2e6e6574776f726b", [network, activity])
    }

    public static func devicesDetailNetworkPaired(_ network: String, _ activity: String, _ date: String) -> String {
        render("m6a6f75726e616c2e6d61632e646576696365732e64657461696c2e6e6574776f726b2e706169726564", [network, activity, date])
    }

    public static func devicesDetailPaired(_ activity: String, _ date: String) -> String {
        render("m6a6f75726e616c2e6d61632e646576696365732e64657461696c2e706169726564", [activity, date])
    }

    public static func devicesEmptyBody() -> String {
        render("m6a6f75726e616c2e6d61632e646576696365732e656d7074792e626f6479", [])
    }

    public static func devicesEmptyTitle() -> String {
        render("m6a6f75726e616c2e6d61632e646576696365732e656d7074792e7469746c65", [])
    }

    public static func devicesExpiresIn(_ minutes: String, _ seconds: String) -> String {
        render("m6a6f75726e616c2e6d61632e646576696365732e657870697265732e696e", [minutes, seconds])
    }

    public static func devicesLinkExpiredBody() -> String {
        render("m6a6f75726e616c2e6d61632e646576696365732e6c696e6b2e657870697265642e626f6479", [])
    }

    public static func devicesLinkExpiredTitle() -> String {
        render("m6a6f75726e616c2e6d61632e646576696365732e6c696e6b2e657870697265642e7469746c65", [])
    }

    public static func devicesLoading() -> String {
        render("m6a6f75726e616c2e6d61632e646576696365732e6c6f6164696e67", [])
    }

    public static func devicesModelLastSeen(_ time: String) -> String {
        render("m6a6f75726e616c2e6d61632e646576696365732e6d6f64656c2e6c6173742e7365656e", [time])
    }

    public static func devicesModelLastSeenJustNow() -> String {
        render("m6a6f75726e616c2e6d61632e646576696365732e6d6f64656c2e6c6173742e7365656e2e6a7573742e6e6f77", [])
    }

    public static func devicesModelLastSeenUnknown() -> String {
        render("m6a6f75726e616c2e6d61632e646576696365732e6d6f64656c2e6c6173742e7365656e2e756e6b6e6f776e", [])
    }

    public static func devicesModelNeverConnected() -> String {
        render("m6a6f75726e616c2e6d61632e646576696365732e6d6f64656c2e6e657665722e636f6e6e6563746564", [])
    }

    public static func devicesModelStatus(_ status: String) -> String {
        render("m6a6f75726e616c2e6d61632e646576696365732e6d6f64656c2e737461747573", [status])
    }

    public static func devicesNotReadyBody() -> String {
        render("m6a6f75726e616c2e6d61632e646576696365732e6e6f742e72656164792e626f6479", [])
    }

    public static func devicesNotReadyTitle() -> String {
        render("m6a6f75726e616c2e6d61632e646576696365732e6e6f742e72656164792e7469746c65", [])
    }

    public static func devicesNotRunningBody() -> String {
        render("m6a6f75726e616c2e6d61632e646576696365732e6e6f742e72756e6e696e672e626f6479", [])
    }

    public static func devicesNotRunningTitle() -> String {
        render("m6a6f75726e616c2e6d61632e646576696365732e6e6f742e72756e6e696e672e7469746c65", [])
    }

    public static func devicesOpenFreshLink() -> String {
        render("m6a6f75726e616c2e6d61632e646576696365732e6f70656e2e66726573682e6c696e6b", [])
    }

    public static func devicesPairedDone() -> String {
        render("m6a6f75726e616c2e6d61632e646576696365732e7061697265642e646f6e65", [])
    }

    public static func devicesPairedHeading(_ label: String) -> String {
        render("m6a6f75726e616c2e6d61632e646576696365732e7061697265642e68656164696e67", [label])
    }

    public static func devicesPairedHeadingUnnamed() -> String {
        render("m6a6f75726e616c2e6d61632e646576696365732e7061697265642e68656164696e672e756e6e616d6564", [])
    }

    public static func devicesPairedNote() -> String {
        render("m6a6f75726e616c2e6d61632e646576696365732e7061697265642e6e6f7465", [])
    }

    public static func devicesPairedNoteAnywhere() -> String {
        render("m6a6f75726e616c2e6d61632e646576696365732e7061697265642e6e6f74652e616e797768657265", [])
    }

    public static func devicesPairedRemoveFailed() -> String {
        render("m6a6f75726e616c2e6d61632e646576696365732e7061697265642e72656d6f76652e6661696c6564", [])
    }

    public static func devicesPairedRemoved() -> String {
        render("m6a6f75726e616c2e6d61632e646576696365732e7061697265642e72656d6f766564", [])
    }

    public static func devicesPairedUnpair() -> String {
        render("m6a6f75726e616c2e6d61632e646576696365732e7061697265642e756e70616972", [])
    }

    public static func devicesPairingCode() -> String {
        render("m6a6f75726e616c2e6d61632e646576696365732e70616972696e672e636f6465", [])
    }

    public static func devicesPairingFailedTitle() -> String {
        render("m6a6f75726e616c2e6d61632e646576696365732e70616972696e672e6661696c65642e7469746c65", [])
    }

    public static func devicesPairingInstructions() -> String {
        render("m6a6f75726e616c2e6d61632e646576696365732e70616972696e672e696e737472756374696f6e73", [])
    }

    public static func devicesPairingLinkLabel() -> String {
        render("m6a6f75726e616c2e6d61632e646576696365732e70616972696e672e6c696e6b2e6c6162656c", [])
    }

    public static func devicesPairingOpening() -> String {
        render("m6a6f75726e616c2e6d61632e646576696365732e70616972696e672e6f70656e696e67", [])
    }

    public static func devicesPairingTitle() -> String {
        render("m6a6f75726e616c2e6d61632e646576696365732e70616972696e672e7469746c65", [])
    }

    public static func devicesPaneTitle() -> String {
        render("m6a6f75726e616c2e6d61632e646576696365732e70616e652e7469746c65", [])
    }

    public static func devicesPeerJournalsHeader() -> String {
        render("m6a6f75726e616c2e6d61632e646576696365732e706565722e6a6f75726e616c732e686561646572", [])
    }

    public static func devicesRemove() -> String {
        render("m6a6f75726e616c2e6d61632e646576696365732e72656d6f7665", [])
    }

    public static func devicesRevokeBody(_ name: String, _ detail: String) -> String {
        render("m6a6f75726e616c2e6d61632e646576696365732e7265766f6b652e626f6479", [name, detail])
    }

    public static func devicesRevokeFailed() -> String {
        render("m6a6f75726e616c2e6d61632e646576696365732e7265766f6b652e6661696c6564", [])
    }

    public static func devicesRevokeTitle(_ name: String) -> String {
        render("m6a6f75726e616c2e6d61632e646576696365732e7265766f6b652e7469746c65", [name])
    }

    public static func devicesTryAgain() -> String {
        render("m6a6f75726e616c2e6d61632e646576696365732e7472792e616761696e", [])
    }

    public static func devicesUnnamedDevice() -> String {
        render("m6a6f75726e616c2e6d61632e646576696365732e756e6e616d65642e646576696365", [])
    }

    public static func devicesUnnamedJournal() -> String {
        render("m6a6f75726e616c2e6d61632e646576696365732e756e6e616d65642e6a6f75726e616c", [])
    }

    public static func devicesYourDevicesHeader() -> String {
        render("m6a6f75726e616c2e6d61632e646576696365732e796f75722e646576696365732e686561646572", [])
    }

    public static func eventsCompleted() -> String {
        render("m6a6f75726e616c2e6d61632e6576656e74732e636f6d706c65746564", [])
    }

    public static func eventsCompletedDuration(_ duration_ms: String) -> String {
        render("m6a6f75726e616c2e6d61632e6576656e74732e636f6d706c657465642e6475726174696f6e", [duration_ms])
    }

    public static func eventsDoctorCheck(_ label: String, _ name: String, _ detail: String) -> String {
        render("m6a6f75726e616c2e6d61632e6576656e74732e646f63746f722e636865636b", [label, name, detail])
    }

    public static func eventsDoctorStarted() -> String {
        render("m6a6f75726e616c2e6d61632e6576656e74732e646f63746f722e73746172746564", [])
    }

    public static func eventsDoctorStatus(_ status: String) -> String {
        render("m6a6f75726e616c2e6d61632e6576656e74732e646f63746f722e737461747573", [status])
    }

    public static func eventsDoctorSummary(_ total: Int, _ failed: String, _ warnings: String, _ skipped: String) -> String {
        renderPlural("m6a6f75726e616c2e6d61632e6576656e74732e646f63746f722e73756d6d617279", [Int64(total), failed, warnings, skipped, String(total)])
    }

    public static func eventsDone(_ step: String, _ outcome: String) -> String {
        render("m6a6f75726e616c2e6d61632e6576656e74732e646f6e65", [step, outcome])
    }

    public static func eventsDoneDuration(_ step: String, _ outcome: String, _ duration_ms: String) -> String {
        render("m6a6f75726e616c2e6d61632e6576656e74732e646f6e652e6475726174696f6e", [step, outcome, duration_ms])
    }

    public static func eventsFail() -> String {
        render("m6a6f75726e616c2e6d61632e6576656e74732e6661696c", [])
    }

    public static func eventsFailed() -> String {
        render("m6a6f75726e616c2e6d61632e6576656e74732e6661696c6564", [])
    }

    public static func eventsFailedStep(_ failed_step: String) -> String {
        render("m6a6f75726e616c2e6d61632e6576656e74732e6661696c65642e73746570", [failed_step])
    }

    public static func eventsMalformed(_ raw: String) -> String {
        render("m6a6f75726e616c2e6d61632e6576656e74732e6d616c666f726d6564", [raw])
    }

    public static func eventsOk() -> String {
        render("m6a6f75726e616c2e6d61632e6576656e74732e6f6b", [])
    }

    public static func eventsSkip() -> String {
        render("m6a6f75726e616c2e6d61632e6576656e74732e736b6970", [])
    }

    public static func eventsSkipped(_ step: String) -> String {
        render("m6a6f75726e616c2e6d61632e6576656e74732e736b6970706564", [step])
    }

    public static func eventsSkippedReason(_ step: String, _ reason: String) -> String {
        render("m6a6f75726e616c2e6d61632e6576656e74732e736b69707065642e726561736f6e", [step, reason])
    }

    public static func eventsStarted() -> String {
        render("m6a6f75726e616c2e6d61632e6576656e74732e73746172746564", [])
    }

    public static func eventsStartedVersion(_ version: String, _ mode: String) -> String {
        render("m6a6f75726e616c2e6d61632e6576656e74732e737461727465642e76657273696f6e", [version, mode])
    }

    public static func eventsStep(_ step: String) -> String {
        render("m6a6f75726e616c2e6d61632e6576656e74732e73746570", [step])
    }

    public static func eventsStepFailed(_ step: String, _ message: String) -> String {
        render("m6a6f75726e616c2e6d61632e6576656e74732e737465702e6661696c6564", [step, message])
    }

    public static func eventsStepIndexed(_ index: String, _ total: String, _ step: String) -> String {
        render("m6a6f75726e616c2e6d61632e6576656e74732e737465702e696e6465786564", [index, total, step])
    }

    public static func eventsUnknown() -> String {
        render("m6a6f75726e616c2e6d61632e6576656e74732e756e6b6e6f776e", [])
    }

    public static func eventsUnparseable(_ raw: String) -> String {
        render("m6a6f75726e616c2e6d61632e6576656e74732e756e706172736561626c65", [raw])
    }

    public static func eventsUnrecognized(_ raw: String) -> String {
        render("m6a6f75726e616c2e6d61632e6576656e74732e756e7265636f676e697a6564", [raw])
    }

    public static func eventsWarn() -> String {
        render("m6a6f75726e616c2e6d61632e6576656e74732e7761726e", [])
    }

    public static func eventsWarning(_ text: String) -> String {
        render("m6a6f75726e616c2e6d61632e6576656e74732e7761726e696e67", [text])
    }

    public static func eventsWarningFix(_ text: String, _ fix_hint: String) -> String {
        render("m6a6f75726e616c2e6d61632e6576656e74732e7761726e696e672e666978", [text, fix_hint])
    }

    public static func firstRunModelSetupDidNotFinish() -> String {
        render("m6a6f75726e616c2e6d61632e66697273742e72756e2e6d6f64656c2e73657475702e6469642e6e6f742e66696e697368", [])
    }

    public static func firstRunModelSetupReportedCompletionTwice() -> String {
        render("m6a6f75726e616c2e6d61632e66697273742e72756e2e6d6f64656c2e73657475702e7265706f727465642e636f6d706c6574696f6e2e7477696365", [])
    }

    public static func firstRunModelSetupTimedOut() -> String {
        render("m6a6f75726e616c2e6d61632e66697273742e72756e2e6d6f64656c2e73657475702e74696d65642e6f7574", [])
    }

    public static func healthCheckDenied() -> String {
        render("m6a6f75726e616c2e6d61632e6865616c74682e636865636b2e64656e696564", [])
    }

    public static func healthCheckGranted() -> String {
        render("m6a6f75726e616c2e6d61632e6865616c74682e636865636b2e6772616e746564", [])
    }

    public static func healthCheckMicrophone() -> String {
        render("m6a6f75726e616c2e6d61632e6865616c74682e636865636b2e6d6963726f70686f6e65", [])
    }

    public static func healthCheckScreenRecording() -> String {
        render("m6a6f75726e616c2e6d61632e6865616c74682e636865636b2e73637265656e2e7265636f7264696e67", [])
    }

    public static func healthCheckUnknownCheckingPermissions() -> String {
        render("m6a6f75726e616c2e6d61632e6865616c74682e636865636b2e756e6b6e6f776e2e636865636b696e672e7065726d697373696f6e73", [])
    }

    public static func healthMicrophonePermissionHelp() -> String {
        render("m6a6f75726e616c2e6d61632e6865616c74682e6d6963726f70686f6e652e7065726d697373696f6e2e68656c70", [])
    }

    public static func healthScreenPermissionHelp() -> String {
        render("m6a6f75726e616c2e6d61632e6865616c74682e73637265656e2e7065726d697373696f6e2e68656c70", [])
    }

    public static func homeBrowserExplanation() -> String {
        render("m6a6f75726e616c2e6d61632e686f6d652e62726f777365722e6578706c616e6174696f6e", [])
    }

    public static func homeUnconfigured() -> String {
        render("m6a6f75726e616c2e6d61632e686f6d652e756e636f6e66696775726564", [])
    }

    public static func infoCfbundleDisplayName() -> String {
        render("m6a6f75726e616c2e6d61632e696e666f2e636662756e646c652e646973706c61792e6e616d65", [])
    }

    public static func infoCfbundleName() -> String {
        render("m6a6f75726e616c2e6d61632e696e666f2e636662756e646c652e6e616d65", [])
    }

    public static func infoNshumanReadableCopyright() -> String {
        render("m6a6f75726e616c2e6d61632e696e666f2e6e7368756d616e2e7265616461626c652e636f70797269676874", [])
    }

    public static func localEndpointUnavailable() -> String {
        render("m6a6f75726e616c2e6d61632e6c6f63616c2e656e64706f696e742e756e617661696c61626c65", [])
    }

    public static func locationCancel() -> String {
        render("m6a6f75726e616c2e6d61632e6c6f636174696f6e2e63616e63656c", [])
    }

    public static func locationLeaveSetupConfirm() -> String {
        render("m6a6f75726e616c2e6d61632e6c6f636174696f6e2e6c656176652e73657475702e636f6e6669726d", [])
    }

    public static func locationNewJournalHostedShare() -> String {
        render("m6a6f75726e616c2e6d61632e6c6f636174696f6e2e6e65772e6a6f75726e616c2e686f737465642e7368617265", [])
    }

    public static func locationOtherJournals() -> String {
        render("m6a6f75726e616c2e6d61632e6c6f636174696f6e2e6f746865722e6a6f75726e616c73", [])
    }

    public static func locationSameMarkTemplate() -> String {
        render("m6a6f75726e616c2e6d61632e6c6f636174696f6e2e73616d652e6d61726b2e74656d706c617465", [])
    }

    public static func locationSharedHostedTemplate(_ path: String) -> String {
        render("m6a6f75726e616c2e6d61632e6c6f636174696f6e2e7368617265642e686f737465642e74656d706c617465", [path])
    }

    public static func locationStartNewJournal() -> String {
        render("m6a6f75726e616c2e6d61632e6c6f636174696f6e2e73746172742e6e65772e6a6f75726e616c", [])
    }

    public static func locationStartNewJournalConfirmTemplate(_ path: String) -> String {
        render("m6a6f75726e616c2e6d61632e6c6f636174696f6e2e73746172742e6e65772e6a6f75726e616c2e636f6e6669726d2e74656d706c617465", [path])
    }

    public static func locationStartNewJournalConfirmTitle() -> String {
        render("m6a6f75726e616c2e6d61632e6c6f636174696f6e2e73746172742e6e65772e6a6f75726e616c2e636f6e6669726d2e7469746c65", [])
    }

    public static func locationStartNewJournalHelp() -> String {
        render("m6a6f75726e616c2e6d61632e6c6f636174696f6e2e73746172742e6e65772e6a6f75726e616c2e68656c70", [])
    }

    public static func locationSwitchButton() -> String {
        render("m6a6f75726e616c2e6d61632e6c6f636174696f6e2e7377697463682e627574746f6e", [])
    }

    public static func locationSwitchConfirmTemplate(_ path: String) -> String {
        render("m6a6f75726e616c2e6d61632e6c6f636174696f6e2e7377697463682e636f6e6669726d2e74656d706c617465", [path])
    }

    public static func locationSwitchConfirmTitleTemplate(_ path: String) -> String {
        render("m6a6f75726e616c2e6d61632e6c6f636174696f6e2e7377697463682e636f6e6669726d2e7469746c652e74656d706c617465", [path])
    }

    public static func locationSwitchToJournal() -> String {
        render("m6a6f75726e616c2e6d61632e6c6f636174696f6e2e7377697463682e746f2e6a6f75726e616c", [])
    }

    public static func restartJournalPathUnreadable() -> String {
        render("m6a6f75726e616c2e6d61632e726573746172742e6a6f75726e616c2e706174682e756e7265616461626c65", [])
    }

    public static func restartRestartFailedJournalDidNotComeBack() -> String {
        render("m6a6f75726e616c2e6d61632e726573746172742e726573746172742e6661696c65642e6a6f75726e616c2e6469642e6e6f742e636f6d652e6261636b", [])
    }

    public static func restartRestartFailedJournalDidNotRestart() -> String {
        render("m6a6f75726e616c2e6d61632e726573746172742e726573746172742e6661696c65642e6a6f75726e616c2e6469642e6e6f742e72657374617274", [])
    }

    public static func runtimeBundleMissing(_ path: String) -> String {
        render("m6a6f75726e616c2e6d61632e72756e74696d652e62756e646c652e6d697373696e67", [path])
    }

    public static func runtimeChildAdmissionFailed() -> String {
        render("m6a6f75726e616c2e6d61632e72756e74696d652e6368696c642e61646d697373696f6e2e6661696c6564", [])
    }

    public static func runtimeChildContainmentPending() -> String {
        render("m6a6f75726e616c2e6d61632e72756e74696d652e6368696c642e636f6e7461696e6d656e742e70656e64696e67", [])
    }

    public static func runtimeChildUnconfigured() -> String {
        render("m6a6f75726e616c2e6d61632e72756e74696d652e6368696c642e756e636f6e66696775726564", [])
    }

    public static func runtimeCouldnTGetReadyContinuing(_ library: String) -> String {
        render("m6a6f75726e616c2e6d61632e72756e74696d652e636f756c646e2e742e6765742e72656164792e636f6e74696e75696e67", [library])
    }

    public static func runtimeExecutableMissing(_ path: String) -> String {
        render("m6a6f75726e616c2e6d61632e72756e74696d652e65786563757461626c652e6d697373696e67", [path])
    }

    public static func runtimeInstallerReadinessGateFailed() -> String {
        render("m6a6f75726e616c2e6d61632e72756e74696d652e696e7374616c6c65722e72656164696e6573732e676174652e6661696c6564", [])
    }

    public static func runtimeJournalChildBreakerTripped() -> String {
        render("m6a6f75726e616c2e6d61632e72756e74696d652e6a6f75726e616c2e6368696c642e627265616b65722e74726970706564", [])
    }

    public static func runtimeJournalChildContainmentUnresolved() -> String {
        render("m6a6f75726e616c2e6d61632e72756e74696d652e6a6f75726e616c2e6368696c642e636f6e7461696e6d656e742e756e7265736f6c766564", [])
    }

    public static func runtimeJournalDriveCannotRun() -> String {
        render("m6a6f75726e616c2e6d61632e72756e74696d652e6a6f75726e616c2e64726976652e63616e6e6f742e72756e", [])
    }

    public static func runtimeJournalMaterializeFailed() -> String {
        render("m6a6f75726e616c2e6d61632e72756e74696d652e6a6f75726e616c2e6d6174657269616c697a652e6661696c6564", [])
    }

    public static func runtimeJournalReadinessTimeout() -> String {
        render("m6a6f75726e616c2e6d61632e72756e74696d652e6a6f75726e616c2e72656164696e6573732e74696d656f7574", [])
    }

    public static func runtimeJournalSetupNeededBeforeUpgrade() -> String {
        render("m6a6f75726e616c2e6d61632e72756e74696d652e6a6f75726e616c2e73657475702e6e65656465642e6265666f72652e75706772616465", [])
    }

    public static func runtimeJournalSpawnBlockedLegacyService() -> String {
        render("m6a6f75726e616c2e6d61632e72756e74696d652e6a6f75726e616c2e737061776e2e626c6f636b65642e6c65676163792e73657276696365", [])
    }

    public static func runtimeJournalSpawnBlockedPorts() -> String {
        render("m6a6f75726e616c2e6d61632e72756e74696d652e6a6f75726e616c2e737061776e2e626c6f636b65642e706f727473", [])
    }

    public static func runtimeJournalSpawnPortCheckFailed() -> String {
        render("m6a6f75726e616c2e6d61632e72756e74696d652e6a6f75726e616c2e737061776e2e706f72742e636865636b2e6661696c6564", [])
    }

    public static func runtimeJournalSpawnPortHeldElsewhere() -> String {
        render("m6a6f75726e616c2e6d61632e72756e74696d652e6a6f75726e616c2e737061776e2e706f72742e68656c642e656c73657768657265", [])
    }

    public static func runtimeJournalSpawnServiceCheckFailed() -> String {
        render("m6a6f75726e616c2e6d61632e72756e74696d652e6a6f75726e616c2e737061776e2e736572766963652e636865636b2e6661696c6564", [])
    }

    public static func runtimeReleaseIdentityMissing() -> String {
        render("m6a6f75726e616c2e6d61632e72756e74696d652e72656c656173652e6964656e746974792e6d697373696e67", [])
    }

    public static func runtimeRequiredModelsFailed(_ exit_code: String, _ reason: String) -> String {
        render("m6a6f75726e616c2e6d61632e72756e74696d652e72657175697265642e6d6f64656c732e6661696c6564", [exit_code, reason])
    }

    public static func settingsBackup() -> String {
        render("m6a6f75726e616c2e6d61632e73657474696e67732e6261636b7570", [])
    }

    public static func settingsCopied() -> String {
        render("m6a6f75726e616c2e6d61632e73657474696e67732e636f70696564", [])
    }

    public static func settingsCopy() -> String {
        render("m6a6f75726e616c2e6d61632e73657474696e67732e636f7079", [])
    }

    public static func settingsCouldnTCopySelectTheTextAndCopyIt() -> String {
        render("m6a6f75726e616c2e6d61632e73657474696e67732e636f756c646e2e742e636f70792e73656c6563742e7468652e746578742e616e642e636f70792e6974", [])
    }

    public static func settingsDiskUsed() -> String {
        render("m6a6f75726e616c2e6d61632e73657474696e67732e6469736b2e75736564", [])
    }

    public static func settingsHealth() -> String {
        render("m6a6f75726e616c2e6d61632e73657474696e67732e6865616c7468", [])
    }

    public static func settingsLaunchTheJournalWhenYouLogIn() -> String {
        render("m6a6f75726e616c2e6d61632e73657474696e67732e6c61756e63682e7468652e6a6f75726e616c2e7768656e2e796f752e6c6f672e696e", [])
    }

    public static func settingsLocation() -> String {
        render("m6a6f75726e616c2e6d61632e73657474696e67732e6c6f636174696f6e", [])
    }

    public static func settingsOpenBackup() -> String {
        render("m6a6f75726e616c2e6d61632e73657474696e67732e6f70656e2e6261636b7570", [])
    }

    public static func settingsOpenYourJournal() -> String {
        render("m6a6f75726e616c2e6d61632e73657474696e67732e6f70656e2e796f75722e6a6f75726e616c", [])
    }

    public static func settingsRestart() -> String {
        render("m6a6f75726e616c2e6d61632e73657474696e67732e72657374617274", [])
    }

    public static func settingsRunState() -> String {
        render("m6a6f75726e616c2e6d61632e73657474696e67732e72756e2e7374617465", [])
    }

    public static func settingsRunStateLink() -> String {
        render("m6a6f75726e616c2e6d61632e73657474696e67732e72756e2e73746174652e6c696e6b", [])
    }

    public static func settingsStart() -> String {
        render("m6a6f75726e616c2e6d61632e73657474696e67732e7374617274", [])
    }

    public static func settingsStartup() -> String {
        render("m6a6f75726e616c2e6d61632e73657474696e67732e73746172747570", [])
    }

    public static func settingsStop() -> String {
        render("m6a6f75726e616c2e6d61632e73657474696e67732e73746f70", [])
    }

    public static func settingsTheJournalAppIsAvailable(_ version: String) -> String {
        render("m6a6f75726e616c2e6d61632e73657474696e67732e7468652e6a6f75726e616c2e6170702e69732e617661696c61626c65", [version])
    }

    public static func settingsTheJournalUpdateCheckFailed() -> String {
        render("m6a6f75726e616c2e6d61632e73657474696e67732e7468652e6a6f75726e616c2e7570646174652e636865636b2e6661696c6564", [])
    }

    public static func settingsUpdateFailedWithVersion(_ version: String) -> String {
        render("m6a6f75726e616c2e6d61632e73657474696e67732e7570646174652e6661696c65642e776974682e76657273696f6e", [version])
    }

    public static func settingsUpdateNeedsAttention() -> String {
        render("m6a6f75726e616c2e6d61632e73657474696e67732e7570646174652e6e656564732e617474656e74696f6e", [])
    }

    public static func settingsViewJournalUpdates() -> String {
        render("m6a6f75726e616c2e6d61632e73657474696e67732e766965772e6a6f75726e616c2e75706461746573", [])
    }

    public static func settingsYourJournalAtAGlance() -> String {
        render("m6a6f75726e616c2e6d61632e73657474696e67732e796f75722e6a6f75726e616c2e61742e612e676c616e6365", [])
    }

    public static func settingsYourJournalMarkNotReadYet() -> String {
        render("m6a6f75726e616c2e6d61632e73657474696e67732e796f75722e6a6f75726e616c2e6d61726b2e6e6f742e726561642e796574", [])
    }

    public static func setupAdoptFailed() -> String {
        render("m6a6f75726e616c2e6d61632e73657475702e61646f70742e6661696c6564", [])
    }

    public static func setupAdoptLandingLine() -> String {
        render("m6a6f75726e616c2e6d61632e73657475702e61646f70742e6c616e64696e672e6c696e65", [])
    }

    public static func setupAdoptOpening() -> String {
        render("m6a6f75726e616c2e6d61632e73657475702e61646f70742e6f70656e696e67", [])
    }

    public static func setupAdoptTitle() -> String {
        render("m6a6f75726e616c2e6d61632e73657475702e61646f70742e7469746c65", [])
    }

    public static func setupBack() -> String {
        render("m6a6f75726e616c2e6d61632e73657475702e6261636b", [])
    }

    public static func setupChooseLocation() -> String {
        render("m6a6f75726e616c2e6d61632e73657475702e63686f6f73652e6c6f636174696f6e", [])
    }

    public static func setupChromeOf(_ done: String, _ total: String) -> String {
        render("m6a6f75726e616c2e6d61632e73657475702e6368726f6d652e6f66", [done, total])
    }

    public static func setupContinueButton() -> String {
        render("m6a6f75726e616c2e6d61632e73657475702e636f6e74696e75652e627574746f6e", [])
    }

    public static func setupContinueInTheSolstoneApp() -> String {
        render("m6a6f75726e616c2e6d61632e73657475702e636f6e74696e75652e696e2e7468652e736f6c73746f6e652e617070", [])
    }

    public static func setupDownloadBytes(_ done: String, _ total: String) -> String {
        render("m6a6f75726e616c2e6d61632e73657475702e646f776e6c6f61642e6279746573", [done, total])
    }

    public static func setupFinishInBrowserLine() -> String {
        render("m6a6f75726e616c2e6d61632e73657475702e66696e6973682e696e2e62726f777365722e6c696e65", [])
    }

    public static func setupFinishInBrowserTitle() -> String {
        render("m6a6f75726e616c2e6d61632e73657475702e66696e6973682e696e2e62726f777365722e7469746c65", [])
    }

    public static func setupFolderHoldsAnotherJournal() -> String {
        render("m6a6f75726e616c2e6d61632e73657475702e666f6c6465722e686f6c64732e616e6f746865722e6a6f75726e616c", [])
    }

    public static func setupGoBackToTheJournalIn(_ path: String) -> String {
        render("m6a6f75726e616c2e6d61632e73657475702e676f2e6261636b2e746f2e7468652e6a6f75726e616c2e696e", [path])
    }

    public static func setupHandoffCompetingSetup() -> String {
        render("m6a6f75726e616c2e6d61632e73657475702e68616e646f66662e636f6d706574696e672e7365747570", [])
    }

    public static func setupHandoffHeldPairing() -> String {
        render("m6a6f75726e616c2e6d61632e73657475702e68616e646f66662e68656c642e70616972696e67", [])
    }

    public static func setupHandoffNotDelivered() -> String {
        render("m6a6f75726e616c2e6d61632e73657475702e68616e646f66662e6e6f742e64656c697665726564", [])
    }

    public static func setupHandoffPending() -> String {
        render("m6a6f75726e616c2e6d61632e73657475702e68616e646f66662e70656e64696e67", [])
    }

    public static func setupJournalFolder() -> String {
        render("m6a6f75726e616c2e6d61632e73657475702e6a6f75726e616c2e666f6c646572", [])
    }

    public static func setupLocationField() -> String {
        render("m6a6f75726e616c2e6d61632e73657475702e6c6f636174696f6e2e6669656c64", [])
    }

    public static func setupLoginItemChangeFailed() -> String {
        render("m6a6f75726e616c2e6d61632e73657475702e6c6f67696e2e6974656d2e6368616e67652e6661696c6564", [])
    }

    public static func setupLoginItemUnavailable() -> String {
        render("m6a6f75726e616c2e6d61632e73657475702e6c6f67696e2e6974656d2e756e617661696c61626c65", [])
    }

    public static func setupNameLocationTitle() -> String {
        render("m6a6f75726e616c2e6d61632e73657475702e6e616d652e6c6f636174696f6e2e7469746c65", [])
    }

    public static func setupOnThisMac() -> String {
        render("m6a6f75726e616c2e6d61632e73657475702e6f6e2e746869732e6d6163", [])
    }

    public static func setupOnThisMacDescription() -> String {
        render("m6a6f75726e616c2e6d61632e73657475702e6f6e2e746869732e6d61632e6465736372697074696f6e", [])
    }

    public static func setupOpenYourJournal() -> String {
        render("m6a6f75726e616c2e6d61632e73657475702e6f70656e2e796f75722e6a6f75726e616c", [])
    }

    public static func setupReopenInBrowserButton() -> String {
        render("m6a6f75726e616c2e6d61632e73657475702e72656f70656e2e696e2e62726f777365722e627574746f6e", [])
    }

    public static func setupRestoreFromBackup() -> String {
        render("m6a6f75726e616c2e6d61632e73657475702e726573746f72652e66726f6d2e6261636b7570", [])
    }

    public static func setupRunnerSolstoneJournalSetupFailed() -> String {
        render("m6a6f75726e616c2e6d61632e73657475702e72756e6e65722e736f6c73746f6e652e6a6f75726e616c2e73657475702e6661696c6564", [])
    }

    public static func setupSetUpSolstone() -> String {
        render("m6a6f75726e616c2e6d61632e73657475702e7365742e75702e736f6c73746f6e65", [])
    }

    public static func setupSetupProgressNotSaved() -> String {
        render("m6a6f75726e616c2e6d61632e73657475702e73657475702e70726f67726573732e6e6f742e7361766564", [])
    }

    public static func setupSetupSubtitle() -> String {
        render("m6a6f75726e616c2e6d61632e73657475702e73657475702e7375627469746c65", [])
    }

    public static func setupSetupTitle() -> String {
        render("m6a6f75726e616c2e6d61632e73657475702e73657475702e7469746c65", [])
    }

    public static func setupStartAutomaticallyHelp() -> String {
        render("m6a6f75726e616c2e6d61632e73657475702e73746172742e6175746f6d61746963616c6c792e68656c70", [])
    }

    public static func setupStartSetup() -> String {
        render("m6a6f75726e616c2e6d61632e73657475702e73746172742e7365747570", [])
    }

    public static func setupStepProgress(_ step: String) -> String {
        render("m6a6f75726e616c2e6d61632e73657475702e737465702e70726f6772657373", [step])
    }

    public static func setupTheSolstoneApp() -> String {
        render("m6a6f75726e616c2e6d61632e73657475702e7468652e736f6c73746f6e652e617070", [])
    }

    public static func setupTheSolstoneAppDescription() -> String {
        render("m6a6f75726e616c2e6d61632e73657475702e7468652e736f6c73746f6e652e6170702e6465736372697074696f6e", [])
    }

    public static func setupThinking() -> String {
        render("m6a6f75726e616c2e6d61632e73657475702e7468696e6b696e67", [])
    }

    public static func setupThinkingDescription() -> String {
        render("m6a6f75726e616c2e6d61632e73657475702e7468696e6b696e672e6465736372697074696f6e", [])
    }

    public static func setupThinkingFailed() -> String {
        render("m6a6f75726e616c2e6d61632e73657475702e7468696e6b696e672e6661696c6564", [])
    }

    public static func setupTryAgain() -> String {
        render("m6a6f75726e616c2e6d61632e73657475702e7472792e616761696e", [])
    }

    public static func setupWhatThisSetsUp() -> String {
        render("m6a6f75726e616c2e6d61632e73657475702e776861742e746869732e736574732e7570", [])
    }

    public static func setupWhatThisSetsUpJournal() -> String {
        render("m6a6f75726e616c2e6d61632e73657475702e776861742e746869732e736574732e75702e6a6f75726e616c", [])
    }

    public static func setupWhereDoesThinkingHappen() -> String {
        render("m6a6f75726e616c2e6d61632e73657475702e77686572652e646f65732e7468696e6b696e672e68617070656e", [])
    }

    public static func setupWhereDoesThinkingHappenDescription() -> String {
        render("m6a6f75726e616c2e6d61632e73657475702e77686572652e646f65732e7468696e6b696e672e68617070656e2e6465736372697074696f6e", [])
    }

    public static func setupWhereYourJournalLives() -> String {
        render("m6a6f75726e616c2e6d61632e73657475702e77686572652e796f75722e6a6f75726e616c2e6c69766573", [])
    }

    public static func setupWhereYourJournalLivesDescription() -> String {
        render("m6a6f75726e616c2e6d61632e73657475702e77686572652e796f75722e6a6f75726e616c2e6c697665732e6465736372697074696f6e", [])
    }

    public static func setupYourJournal() -> String {
        render("m6a6f75726e616c2e6d61632e73657475702e796f75722e6a6f75726e616c", [])
    }

    public static func updatesDeferred() -> String {
        render("m6a6f75726e616c2e6d61632e757064617465732e6465666572726564", [])
    }

    public static func updatesReadyToInstall(_ version: String) -> String {
        render("m6a6f75726e616c2e6d61632e757064617465732e72656164792e746f2e696e7374616c6c", [version])
    }

    public static func windowModelBackup() -> String {
        render("m6a6f75726e616c2e6d61632e77696e646f772e6d6f64656c2e6261636b7570", [])
    }

    public static func windowModelBlocked() -> String {
        render("m6a6f75726e616c2e6d61632e77696e646f772e6d6f64656c2e626c6f636b6564", [])
    }

    public static func windowModelDevices() -> String {
        render("m6a6f75726e616c2e6d61632e77696e646f772e6d6f64656c2e64657669636573", [])
    }

    public static func windowModelHealthy() -> String {
        render("m6a6f75726e616c2e6d61632e77696e646f772e6d6f64656c2e6865616c746879", [])
    }

    public static func windowModelHome() -> String {
        render("m6a6f75726e616c2e6d61632e77696e646f772e6d6f64656c2e686f6d65", [])
    }

    public static func windowModelJournal() -> String {
        render("m6a6f75726e616c2e6d61632e77696e646f772e6d6f64656c2e6a6f75726e616c", [])
    }

    public static func windowModelLocation() -> String {
        render("m6a6f75726e616c2e6d61632e77696e646f772e6d6f64656c2e6c6f636174696f6e", [])
    }

    public static func windowModelNotSet() -> String {
        render("m6a6f75726e616c2e6d61632e77696e646f772e6d6f64656c2e6e6f742e736574", [])
    }

    public static func windowModelRunState() -> String {
        render("m6a6f75726e616c2e6d61632e77696e646f772e6d6f64656c2e72756e2e7374617465", [])
    }

    public static func windowModelRunning() -> String {
        render("m6a6f75726e616c2e6d61632e77696e646f772e6d6f64656c2e72756e6e696e67", [])
    }

    public static func windowModelStarting() -> String {
        render("m6a6f75726e616c2e6d61632e77696e646f772e6d6f64656c2e7374617274696e67", [])
    }

    public static func windowModelStartup() -> String {
        render("m6a6f75726e616c2e6d61632e77696e646f772e6d6f64656c2e73746172747570", [])
    }

    public static func windowModelStopped() -> String {
        render("m6a6f75726e616c2e6d61632e77696e646f772e6d6f64656c2e73746f70706564", [])
    }

    public static func windowModelUnknown() -> String {
        render("m6a6f75726e616c2e6d61632e77696e646f772e6d6f64656c2e756e6b6e6f776e", [])
    }

    public static func windowModelUpdates() -> String {
        render("m6a6f75726e616c2e6d61632e77696e646f772e6d6f64656c2e75706461746573", [])
    }

}
