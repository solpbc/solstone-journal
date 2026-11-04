// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import Foundation

enum AXID {
    enum Journal {
        enum Sidebar {
            static func tab(_ pane: JournalPane) -> String {
                "journal.sidebar.tab.\(pane.rawValue)"
            }

            static func tabState(_ pane: JournalPane) -> String {
                "journal.sidebar.tab.\(pane.rawValue).state"
            }
        }

        enum Home {
            static let updates = "journal.home.updates"
            static let markCard = "journal.home.mark.card"
            static let runDisplayGlanceState = "journal.home.runDisplay.glance.state"
            static let openJournal = "journal.home.open"
            static let unconfiguredMessageState = "journal.home.unconfigured.message.state"
            static let start = "journal.home.start"
            static let runState = "journal.home.runState"
        }

        enum Ritual {
            static let root = "journal.ritual.root"
            static let routeState = "journal.ritual.route.state"
            static let locationField = "journal.ritual.location.field"
            static let locationChoose = "journal.ritual.location.choose"
            static let nameLocationContinue = "journal.ritual.nameLocation.continue"
            static let nameLocationErrorState = "journal.ritual.nameLocation.error.state"
            static let setupProgress = "journal.ritual.setup.progress"
            static let setupStepState = "journal.ritual.setup.step.state"
            static let setupLogState = "journal.ritual.setup.log.state"
            static let setupErrorState = "journal.ritual.setup.error.state"
            static let setupRetry = "journal.ritual.setup.retry"
            static let browserReopen = "journal.ritual.browser.reopen"
            static let welcomeContinue = "journal.ritual.welcome.continue"
            static let loginToggle = "journal.ritual.login.toggle"
            static let loginFailureState = "journal.ritual.login.failure.state"
            static let folderField = "journal.ritual.folder.field"
            static let folderChoose = "journal.ritual.folder.choose"
            static let folderContinue = "journal.ritual.folder.continue"
            static let markContinue = "journal.ritual.mark.continue"
            static let markRegenerate = "journal.ritual.mark.regenerate"
            static let markProposalState = "journal.ritual.mark.proposal.state"
            static let markErrorState = "journal.ritual.mark.error.state"
            static let thinkingContinue = "journal.ritual.thinking.continue"
            static let thinkingLater = "journal.ritual.thinking.later"
            static let thinkingChoiceState = "journal.ritual.thinking.choice.state"
            static let thinkingErrorState = "journal.ritual.thinking.error.state"
            static func thinkingChoice(_ choice: String) -> String? {
                guard choice.range(
                    of: #"^[a-z0-9]+(?:-[a-z0-9]+)*$"#,
                    options: .regularExpression
                ) != nil else {
                    return nil
                }
                return "journal.ritual.thinking.choice.\(choice)"
            }
            static let downloadSpeechState = "journal.ritual.download.speech.state"
            static let downloadThinkingState = "journal.ritual.download.thinking.state"
            static let downloadSpeechRetry = "journal.ritual.download.speech.retry"
            static let downloadThinkingRetry = "journal.ritual.download.thinking.retry"
            static let downloadThinkingCancel = "journal.ritual.download.thinking.cancel"
            static let operationPendingState = "journal.ritual.operation.pending.state"
            static let operationErrorState = "journal.ritual.operation.error.state"
            static let outboundState = "journal.ritual.outbound.state"
            static let handoffErrorState = "journal.ritual.handoff.error.state"
            static let handoffRetry = "journal.ritual.handoff.retry"
            static let back = "journal.ritual.back"
            static let progress = "journal.ritual.progress"
            static let card = "journal.ritual.card"
            static let footer = "journal.ritual.footer"
        }

        enum Adopt {
            static let root = "journal.adopt.root"
            static let statusState = "journal.adopt.status.state"
            static let messageState = "journal.adopt.message.state"
            static let locationPathState = "journal.adopt.location.path.state"
            static let continueButton = "journal.adopt.continue"
            static let errorState = "journal.adopt.error.state"
        }

        enum Pane {
            static let locationPathState = "journal.journal.location.path.state"
            static let diskUsageState = "journal.journal.diskUsage.state"
        }

        enum RunState {
            static let start = "journal.runState.start"
            static let stop = "journal.runState.stop"
            static let restart = "journal.runState.restart"
            static let displayState = "journal.runState.display.state"
            static let blockedReasonState = "journal.runState.blockedReason.state"
            static let healthState = "journal.runState.health.state"
            static let aboutState = "journal.runState.about.state"
            static let aboutCopy = "journal.runState.about.copy"
            static let aboutCopyFeedbackState = "journal.runState.about.copyFeedback.state"
        }

        enum Backup {
            static let openBackup = "journal.backup.open"
            static let messageState = "journal.backup.message.state"
        }

        enum Startup {
            static let launchAtLogin = "journal.startup.launchAtLogin"
            static let launchAtLoginState = "journal.startup.launchAtLogin.state"
        }

        enum Devices {
            static let root = "journal.devices.root"
            static let loadState = "journal.devices.load.state"
            static let yourDevicesHeader = "journal.devices.yourDevices.header"
            static let yourDevicesCountState = "journal.devices.yourDevices.count.state"
            static let peerJournalsHeader = "journal.devices.peerJournals.header"
            static let peerJournalsCountState = "journal.devices.peerJournals.count.state"
            static let addDevice = "journal.devices.addDevice"

            enum Row {
                static func container(_ fingerprint: String) -> String {
                    "\(prefix(fingerprint))"
                }

                static func label(_ fingerprint: String) -> String {
                    "\(prefix(fingerprint)).label"
                }

                static func detailState(_ fingerprint: String) -> String {
                    "\(prefix(fingerprint)).detail.state"
                }

                static func revoke(_ fingerprint: String) -> String {
                    "\(prefix(fingerprint)).revoke"
                }

                static func sanitizedFingerprint(_ fingerprint: String) -> String {
                    var sanitized = ""
                    for scalar in fingerprint.lowercased().unicodeScalars {
                        switch scalar.value {
                        case 48...57, 97...122:
                            sanitized.unicodeScalars.append(scalar)
                        case 45:
                            sanitized.append("-")
                        default:
                            sanitized.append("-")
                        }
                    }
                    let trimmed = sanitized.trimmingCharacters(in: CharacterSet(charactersIn: "-"))
                    return trimmed.isEmpty ? "unknown" : trimmed
                }

                private static func prefix(_ fingerprint: String) -> String {
                    "journal.devices.row.fingerprint-\(sanitizedFingerprint(fingerprint))"
                }
            }

            enum Pairing {
                static let sheet = "journal.devices.pairing.sheet"
                static let markCard = "journal.devices.pairing.markCard"
                static let linkField = "journal.devices.pairing.link.field"
                static let copyLink = "journal.devices.pairing.copyLink"
                static let copyLinkCopiedState = "journal.devices.pairing.copyLink.copied.state"
                static let qr = "journal.devices.pairing.qr"
                static let countdown = "journal.devices.pairing.countdown"
                static let countdownState = "journal.devices.pairing.countdown.state"
                static let status = "journal.devices.pairing.status"
                static let statusState = "journal.devices.pairing.status.state"
                static let reopen = "journal.devices.pairing.reopen"
                static let unpair = "journal.devices.pairing.unpair"
                static let unpairState = "journal.devices.pairing.unpair.state"
            }

            enum RevokeConfirm {
                static let dialog = "journal.devices.revokeConfirm.dialog"
                static let messageState = "journal.devices.revokeConfirm.message.state"
                static let confirm = "journal.devices.revokeConfirm.confirm"
                static let cancel = "journal.devices.revokeConfirm.cancel"
            }
        }
    }
}
