// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import Foundation
import JournalRuntime
import SolstoneCore

enum JournalDownloadActionKind: Equatable, Sendable {
    case cancelThinking
    case retrySpeech
    case retryThinking
}

struct JournalDownloadRowModel: Equatable, Sendable {
    let name: String
    let detail: String?
    let fraction: Double?
    let indeterminate: Bool
    let actionTitle: String?
    let actionEnabled: Bool
    let actionIdentifier: String?
    let actionKind: JournalDownloadActionKind?
}

struct JournalProgressInfo: Equatable, Sendable {
    let index: Int
    let count: Int
    let label: String
}

enum JournalSetupChrome {
    static func progress(
        route: JournalFirstRunRoute,
        hasRefusal: Bool,
        handoffError: String?
    ) -> JournalProgressInfo? {
        if hasRefusal { return nil }
        if let error = handoffError, !error.isEmpty { return nil }
        guard case .ritual(let step) = route else { return nil }
        let index: Int
        switch step {
        case .welcome: index = 1
        case .folder: index = 2
        case .mark: index = 3
        case .thinking: index = 4
        case .nameLocation, .setupProgress, .finishInBrowser: return nil
        }
        return JournalProgressInfo(
            index: index,
            count: 7,
            label: JournalFirstRunCopy.stepProgress(index)
        )
    }

    static func canGoBackFromFolder(
        route: JournalFirstRunRoute,
        userOperationInFlight: Bool,
        pendingOperation: JournalRitualOperationToken,
        outbound: JournalOutboundToken,
        hasRefusal: Bool,
        handoffError: String?
    ) -> Bool {
        guard route == .ritual(.folder) else { return false }
        guard !userOperationInFlight else { return false }
        guard pendingOperation == .none else { return false }
        guard outbound == .none else { return false }
        guard !hasRefusal else { return false }
        guard handoffError == nil || handoffError?.isEmpty == true else { return false }
        return true
    }

    /// The footer row for the speech download. A row shows while downloading, on an error, or
    /// whenever the model offers its retry; the retry is exactly the model's own `showsSpeechRetry`.
    static func speechDownloadRow(
        job: JournalSetupState.Job?,
        copy: JournalSetupCopy.Downloads,
        speechFailure: String?,
        canRetry: Bool = false,
        locked: Bool,
        downloadLabel: (JournalDownloadJobState) -> String
    ) -> JournalDownloadRowModel? {
        guard let job else { return nil }
        let failure = nonEmpty(speechFailure) ?? nonEmpty(job.error?.detail)
        guard job.state == .downloading || job.state == .error || canRetry || failure != nil else { return nil }
        let (detail, fraction, indeterminate) = progressDetail(job: job, failure: failure, downloadLabel: downloadLabel)
        return JournalDownloadRowModel(
            name: copy.speech,
            detail: detail,
            fraction: fraction,
            indeterminate: indeterminate,
            actionTitle: canRetry ? copy.retry : nil,
            actionEnabled: !locked,
            actionIdentifier: canRetry ? AXID.Journal.Ritual.downloadSpeechRetry : nil,
            actionKind: canRetry ? .retrySpeech : nil
        )
    }

    /// The footer row for the local thinking download. Cancel and retry are exactly the model's
    /// own `showsThinkingCancel` and `showsThinkingRetry`; a thinking failure shows only when the
    /// failed operation was the thinking download, as before.
    static func thinkingDownloadRow(
        job: JournalSetupState.Job?,
        copy: JournalSetupCopy.Downloads,
        thinkingFailure: String?,
        operationError: JournalRitualOperationToken,
        canRetry: Bool = false,
        canCancel: Bool = false,
        locked: Bool,
        downloadLabel: (JournalDownloadJobState) -> String
    ) -> JournalDownloadRowModel? {
        guard let job else { return nil }
        let failure = (operationError == .thinkingDownload ? nonEmpty(thinkingFailure) : nil) ?? nonEmpty(job.error?.detail)
        guard job.state == .downloading || job.state == .error || canRetry || canCancel || failure != nil else { return nil }
        let (detail, fraction, indeterminate) = progressDetail(job: job, failure: failure, downloadLabel: downloadLabel)
        let actionKind: JournalDownloadActionKind? = canCancel ? .cancelThinking : (canRetry ? .retryThinking : nil)
        return JournalDownloadRowModel(
            name: copy.thinking,
            detail: detail,
            fraction: fraction,
            indeterminate: indeterminate,
            actionTitle: canCancel ? copy.cancel : (canRetry ? copy.retry : nil),
            actionEnabled: !locked,
            actionIdentifier: canCancel
                ? AXID.Journal.Ritual.downloadThinkingCancel
                : (canRetry ? AXID.Journal.Ritual.downloadThinkingRetry : nil),
            actionKind: actionKind
        )
    }

    private static func nonEmpty(_ text: String?) -> String? {
        guard let text, !text.isEmpty else { return nil }
        return text
    }

    /// A failure wins the detail slot in any state; otherwise a download shows its bytes and
    /// anything else shows its state word.
    private static func progressDetail(
        job: JournalSetupState.Job,
        failure: String?,
        downloadLabel: (JournalDownloadJobState) -> String
    ) -> (String?, Double?, Bool) {
        if job.state == .downloading {
            let formatter = ByteCountFormatter()
            formatter.countStyle = .file
            let done = formatter.string(fromByteCount: Int64(clamping: job.bytesDone))
            if let total = job.bytesTotal, total > 0 {
                let totalText = formatter.string(fromByteCount: Int64(clamping: total))
                return (failure ?? JournalCopy.setupChromeOf(done, totalText), min(1.0, Double(job.bytesDone) / Double(total)), false)
            }
            return (failure ?? done, nil, true)
        }
        return (failure ?? downloadLabel(job.state), nil, false)
    }

    static func filteredThinkingChoices(
        choices: [JournalSetupCopy.Choice],
        offersLocalThinking: Bool
    ) -> [JournalSetupCopy.Choice] {
        choices.filter { $0.id != JournalThinkingChoice.local.rawValue || offersLocalThinking }
    }

    static func localChoiceRow(
        downloadBytes: UInt64?,
        sizeUnknownText: String
    ) -> (label: String, subLabel: String, description: String) {
        let sizeText: String
        if let bytes = downloadBytes, bytes > 0 {
            let formatter = ByteCountFormatter()
            formatter.countStyle = .file
            sizeText = formatter.string(fromByteCount: Int64(clamping: bytes))
        } else {
            sizeText = sizeUnknownText
        }
        return (
            label: JournalFirstRunCopy.onThisMac,
            subLabel: sizeText,
            description: JournalFirstRunCopy.onThisMacDescription
        )
    }

    static func shouldShowChooseLater(
        offersLocalThinking: Bool,
        displayedChoice: JournalThinkingChoice?
    ) -> Bool {
        // As before the restyle: the line stands in for a choice not yet made, or "later".
        displayedChoice == nil || displayedChoice == .later
    }
}
