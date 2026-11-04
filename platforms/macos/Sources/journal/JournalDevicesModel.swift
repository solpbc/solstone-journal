// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import AppKit
import Foundation
import JournalMarkKit
import Observation
import SolstoneCore

enum JournalDevicesLoadState: String, CaseIterable, Sendable {
    case loading
    case loaded
    case empty
    case notRunning = "not_running"
    case notReady = "not_ready"
}

enum PairingState {
    case idle
    case opening
    case open(link: String, qr: NSImage, deadline: Duration, nonce: String)
    case paired(device: NonceStatusResponse.Device?, relay: Bool)
    case expired(previousLink: String)
    case openFailed(detail: String)
}

extension PairingState: @unchecked Sendable {}

extension PairingState: Equatable {
    static func == (lhs: PairingState, rhs: PairingState) -> Bool {
        switch (lhs, rhs) {
        case (.idle, .idle), (.opening, .opening):
            return true
        case let (.paired(lhsDevice, lhsRelay), .paired(rhsDevice, rhsRelay)):
            return lhsDevice == rhsDevice && lhsRelay == rhsRelay
        case let (.open(lhsLink, _, lhsDeadline, lhsNonce), .open(rhsLink, _, rhsDeadline, rhsNonce)):
            return lhsLink == rhsLink && lhsDeadline == rhsDeadline && lhsNonce == rhsNonce
        case let (.expired(lhsLink), .expired(rhsLink)):
            return lhsLink == rhsLink
        case let (.openFailed(lhsDetail), .openFailed(rhsDetail)):
            return lhsDetail == rhsDetail
        default:
            return false
        }
    }
}

struct JournalDeviceGroup: Identifiable, Equatable {
    let id: String
    let title: String
    let rows: [DeviceRow]

    var count: Int { rows.count }
}

@MainActor
@Observable
final class JournalDevicesModel {
    @ObservationIgnored private let client: any JournalDevicesClientProtocol
    @ObservationIgnored private let clock: any MonotonicClock
    @ObservationIgnored private let copyToClipboard: @MainActor @Sendable (String) -> Void
    @ObservationIgnored private var activeAttemptID: UUID?
    @ObservationIgnored private var pairingTask: Task<Void, Never>?

    var devices: [DeviceRow] = []
    var loadState: JournalDevicesLoadState = .loading
    var loadErrorDetail: String?
    var revokeCandidate: DeviceRow?
    var revokeInFlightFingerprint: String?
    var revokeError: String?
    var isPairingPresented = false
    var pairingState: PairingState = .idle
    var pairedUnpairState: JournalPairedUnpairState = .idle
    var pairingNow: Duration = .zero
    // This journal's own mark presentation, so the pairing window can show "this is your journal"
    // the whole time an owner is pairing a new device — mirrors the connecting device's own
    // confirm dialog, which shows the same mark for comparison. Kept in sync by the owning
    // JournalWindowModel.
    var markPresentation: JournalMarkPresentation = .generic

    init(
        client: any JournalDevicesClientProtocol = JournalDevicesClient(),
        clock: any MonotonicClock = SystemMonotonicClock(),
        copyToClipboard: @escaping @MainActor @Sendable (String) -> Void = {
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString($0, forType: .string)
        }
    ) {
        self.client = client
        self.clock = clock
        self.copyToClipboard = copyToClipboard
        self.pairingNow = clock.now()
    }

    var yourDevicesGroup: JournalDeviceGroup {
        JournalDeviceGroup(
            id: "yourDevices",
            title: DevicesCopy.yourDevicesHeader,
            rows: sortedDeviceRows(devices.filter { !isPeerJournal($0) })
        )
    }

    var peerJournalsGroup: JournalDeviceGroup {
        JournalDeviceGroup(
            id: "peerJournals",
            title: DevicesCopy.peerJournalsHeader,
            rows: sortedDeviceRows(devices.filter(isPeerJournal))
        )
    }

    var groups: [JournalDeviceGroup] {
        [yourDevicesGroup, peerJournalsGroup].filter { !$0.rows.isEmpty }
    }

    var pairingStatusToken: String {
        pairingState.axToken
    }

    func resetTransientState() {
        closePairing()
        revokeCandidate = nil
        revokeInFlightFingerprint = nil
        revokeError = nil
    }

    func loadDevices() async {
        loadState = .loading
        do {
            let loaded = try await client.listDevices()
            applyDevices(loaded)
            loadErrorDetail = nil
            loadState = loaded.isEmpty ? .empty : .loaded
        } catch is CancellationError {
            return
        } catch {
            applyLoadError(error)
        }
    }

    func displayName(for row: DeviceRow) -> String {
        firstNonEmpty(row.displayLabel, row.deviceLabel, row.observerHandle)
            ?? (isPeerJournal(row) ? DevicesCopy.unnamedJournal : DevicesCopy.unnamedDevice)
    }

    func detailLine(for row: DeviceRow, now: Date) -> String {
        var parts: [String] = []
        if let network = firstNonEmpty(row.network) {
            parts.append(network)
        }
        parts.append(livenessDetail(for: row, now: now))
        if let pairedAt = DeviceTimestampParser.parseDeviceTimestamp(row.pairedAt) {
            parts.append("paired \(DeviceTimestampParser.pairedDateString(from: pairedAt))")
        }
        return parts.joined(separator: " · ")
    }

    func neverConnected(_ row: DeviceRow) -> Bool {
        row.lastSeenAt == nil
    }

    func beginRevoke(_ row: DeviceRow) {
        revokeCandidate = row
        revokeError = nil
    }

    func cancelRevoke() {
        guard revokeInFlightFingerprint == nil else { return }
        revokeCandidate = nil
        revokeError = nil
    }

    func isRevoking(_ row: DeviceRow) -> Bool {
        revokeInFlightFingerprint == row.fingerprint
    }

    func confirmRevoke() async {
        guard let row = revokeCandidate else { return }
        let fingerprint = row.fingerprint
        revokeInFlightFingerprint = fingerprint
        revokeError = nil

        do {
            _ = try await client.unpairDevice(fingerprint: fingerprint)
            devices.removeAll { $0.fingerprint == fingerprint }
            revokeCandidate = nil
            revokeInFlightFingerprint = nil
            await loadDevices()
        } catch is CancellationError {
            revokeInFlightFingerprint = nil
        } catch {
            if isPairedDeviceNotFound(error) {
                revokeCandidate = nil
                revokeInFlightFingerprint = nil
                revokeError = nil
                await loadDevices()
                return
            }
            revokeInFlightFingerprint = nil
            revokeError = detail(for: error) ?? DevicesCopy.revokeFailed
        }
    }

    func openPairing() {
        let attemptID = UUID()
        activeAttemptID = attemptID
        pairingTask?.cancel()
        pairingNow = clock.now()
        pairingState = .opening
        pairedUnpairState = .idle
        isPairingPresented = true

        pairingTask = Task { @MainActor [weak self] in
            await self?.drivePairing(attemptID: attemptID)
        }
    }

    func closePairing() {
        activeAttemptID = nil
        pairingTask?.cancel()
        pairingTask = nil
        isPairingPresented = false
        pairingState = .idle
        pairedUnpairState = .idle
        pairingNow = clock.now()
    }

    /// Removes the device a used link just added, from the view that still shows the mark.
    func unpairPairedDevice() async {
        guard case let .paired(device?, _) = pairingState,
              pairedUnpairState != .busy, pairedUnpairState != .removed else { return }
        pairedUnpairState = .busy
        do {
            _ = try await client.unpairDevice(fingerprint: device.cid)
        } catch is CancellationError {
            pairedUnpairState = .idle
            return
        } catch {
            guard isPairedDeviceNotFound(error) else {
                pairedUnpairState = .failed
                return
            }
        }
        pairedUnpairState = .removed
        await loadDevices()
    }

    func copyOpenPairingLink() {
        guard case let .open(link, _, _, _) = pairingState else { return }
        copyToClipboard(link)
    }

    func remainingPairingSeconds() -> Int {
        guard case let .open(_, _, deadline, _) = pairingState else { return 0 }
        return Self.ceilingSeconds(deadline - pairingNow)
    }

    private func drivePairing(attemptID: UUID) async {
        do {
            let pair = try await client.startPairing()
            guard isActive(attemptID) else { return }
            let qr = try JournalQRImage.make(from: pair.pairLink)
            let deadline = clock.now() + .seconds(Int64(pair.expiresIn))
            pairingNow = clock.now()
            pairingState = .open(
                link: pair.pairLink,
                qr: qr,
                deadline: deadline,
                nonce: pair.nonce
            )

            var tick = 0
            while isActive(attemptID), !Task.isCancelled {
                pairingNow = clock.now()
                guard pairingNow < deadline else {
                    expirePairing(previousLink: pair.pairLink, attemptID: attemptID)
                    return
                }

                if tick > 0 && tick % 2 == 0 {
                    await pollPairingNonce(
                        pair.nonce,
                        deadline: deadline,
                        attemptID: attemptID,
                        previousLink: pair.pairLink,
                        relay: pair.relay == true
                    )
                    guard isActive(attemptID), !Task.isCancelled else { return }
                    if case .paired = pairingState { return }
                    if case .expired = pairingState { return }
                }

                await clock.sleep(for: .seconds(1))
                tick += 1
            }
        } catch is CancellationError {
            return
        } catch {
            guard isActive(attemptID) else { return }
            pairingState = .openFailed(detail: detail(for: error) ?? DevicesCopy.pairingFailedTitle)
            pairingTask = nil
        }
    }

    private func pollPairingNonce(
        _ nonce: String,
        deadline: Duration,
        attemptID: UUID,
        previousLink: String,
        relay: Bool
    ) async {
        do {
            let status = try await client.nonceStatus(nonce: nonce)
            pairingNow = clock.now()
            guard isActive(attemptID), !Task.isCancelled else { return }
            guard pairingNow < deadline else {
                expirePairing(previousLink: previousLink, attemptID: attemptID)
                return
            }
            if status.used {
                // The other device now asks its owner to compare this journal's mark, so the
                // mark stays on screen until the owner closes this view.
                pairingState = .paired(device: status.device, relay: relay)
                activeAttemptID = nil
                pairingTask = nil
                await loadDevices()
            }
        } catch is CancellationError {
            return
        } catch {
            return
        }
    }

    private func expirePairing(previousLink: String, attemptID: UUID) {
        guard isActive(attemptID) else { return }
        pairingState = .expired(previousLink: previousLink)
        pairingTask = nil
    }

    private func isActive(_ attemptID: UUID) -> Bool {
        activeAttemptID == attemptID
    }

    private func applyDevices(_ loaded: [DeviceRow]) {
        devices = loaded
    }

    private func applyLoadError(_ error: Error) {
        devices = []
        loadErrorDetail = detail(for: error)
        if isConnectionRefusedTransport(error) {
            loadState = .notRunning
        } else {
            loadState = .notReady
        }
    }

    private func isPeerJournal(_ row: DeviceRow) -> Bool {
        row.role?.trimmingCharacters(in: .whitespacesAndNewlines).lowercased() == "peer"
    }

    private func sortedDeviceRows(_ rows: [DeviceRow]) -> [DeviceRow] {
        rows.sorted { lhs, rhs in
            let lhsNeverConnected = neverConnected(lhs)
            let rhsNeverConnected = neverConnected(rhs)
            if lhsNeverConnected != rhsNeverConnected {
                return !lhsNeverConnected && rhsNeverConnected
            }
            return lhs.fingerprint < rhs.fingerprint
        }
    }

    private func livenessDetail(for row: DeviceRow, now: Date) -> String {
        guard row.lastSeenAt != nil else {
            return "never connected"
        }
        guard let lastSeenAt = DeviceTimestampParser.parseDeviceTimestamp(row.lastSeenAt) else {
            return "last seen unknown"
        }
        if now.timeIntervalSince(lastSeenAt) < 60 {
            return "last seen just now"
        }
        let formatter = RelativeDateTimeFormatter()
        formatter.dateTimeStyle = .named
        formatter.unitsStyle = .full
        return "last seen \(formatter.localizedString(for: lastSeenAt, relativeTo: now))"
    }

    private func firstNonEmpty(_ values: String?...) -> String? {
        values
            .compactMap { $0?.trimmingCharacters(in: .whitespacesAndNewlines) }
            .first { !$0.isEmpty }
    }

    private func detail(for error: Error) -> String? {
        guard let error = error as? JournalDevicesClientError else {
            return nil
        }
        switch error {
        case .server(let envelope):
            return firstNonEmpty(envelope.detail, envelope.error)
        case .serverStatus(let status):
            return "status \(status)"
        case .notReady, .decoding, .invalidURL, .transport:
            return nil
        }
    }

    private func isPairedDeviceNotFound(_ error: Error) -> Bool {
        guard case .server(let envelope) = error as? JournalDevicesClientError else {
            return false
        }
        return envelope.reasonCode == JournalDevicesErrorEnvelope.pairedDeviceNotFound
    }

    private func isConnectionRefusedTransport(_ error: Error) -> Bool {
        guard case .transport(let description) = error as? JournalDevicesClientError else {
            return false
        }
        return description == "\(NSURLErrorDomain):\(URLError.cannotConnectToHost.rawValue)" ||
            description == "\(NSURLErrorDomain):\(URLError.cannotFindHost.rawValue)" ||
            description == "\(NSURLErrorDomain):\(URLError.networkConnectionLost.rawValue)"
    }

    private static func ceilingSeconds(_ duration: Duration) -> Int {
        guard duration > .zero else { return 0 }
        let components = duration.components
        let extraSecond: Int64 = components.attoseconds > 0 ? 1 : 0
        let seconds = max(0, components.seconds + extraSecond)
        return seconds > Int64(Int.max) ? Int.max : Int(seconds)
    }
}

@MainActor
private enum DeviceTimestampParser {
    private static let internetDateTimeFormatter: ISO8601DateFormatter = {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime]
        return formatter
    }()

    private static let fractionalInternetDateTimeFormatter: ISO8601DateFormatter = {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return formatter
    }()

    private static let pairedDateFormatter: DateFormatter = {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.timeZone = .current
        formatter.dateFormat = "MMM d, yyyy"
        return formatter
    }()

    // solstone-journal think/link/auth.py:119,148,200 writes strftime("%Y-%m-%dT%H:%M:%SZ").
    fileprivate static func parseDeviceTimestamp(_ rawValue: String?) -> Date? {
        guard let rawValue else {
            return nil
        }
        let trimmed = rawValue.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else {
            return nil
        }
        return fractionalInternetDateTimeFormatter.date(from: trimmed) ??
            internetDateTimeFormatter.date(from: trimmed)
    }

    fileprivate static func pairedDateString(from date: Date) -> String {
        pairedDateFormatter.string(from: date)
    }
}
