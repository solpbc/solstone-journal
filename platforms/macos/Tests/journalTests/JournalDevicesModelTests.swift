// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import Foundation
import SolstoneCore
import Testing
@testable import journal

@MainActor
@Suite("JournalDevicesModel")
struct JournalDevicesModelTests {
    @Test func twoGroupProjectionUsesHonestCounts() async throws {
        let client = FakeDevicesClient(listResults: [.success([
            device(label: "phone", role: "phone", fingerprint: "a"),
            device(label: "mac", role: "observer", fingerprint: "b"),
            device(label: "blank", role: "", fingerprint: "c"),
            device(label: "unknown", role: "new-role", fingerprint: "d"),
            device(label: "peer", role: "peer", fingerprint: "e"),
            device(label: "nil role", fingerprint: "f"),
        ])])
        let model = JournalDevicesModel(client: client)

        await model.loadDevices()

        #expect(model.loadState == .loaded)
        #expect(model.yourDevicesGroup.count == 5)
        #expect(model.peerJournalsGroup.count == 1)
        #expect(model.groups.map(\.title) == [DevicesCopy.yourDevicesHeader, DevicesCopy.peerJournalsHeader])
    }

    @Test func sameBaseRowsRenderDifferentDetailLines() async throws {
        let model = JournalDevicesModel(client: FakeDevicesClient())
        let now = fixedDate("2026-08-01T12:04:00Z")
        // Midday UTC keeps the paired date stable across realistic host time zones.
        let pairedAt = "2026-08-01T12:00:00Z"
        let live = device(
            label: "iPhone (2)",
            deviceLabel: "iPhone",
            pairedAt: pairedAt,
            lastSeenAt: "2026-08-01T12:00:00Z",
            fingerprint: "live"
        )
        let never = device(
            label: "iPhone",
            deviceLabel: "iPhone",
            pairedAt: pairedAt,
            lastSeenAt: nil,
            fingerprint: "never"
        )

        #expect(model.detailLine(for: live, now: now) == "last seen 4 minutes ago · paired Aug 1, 2026")
        #expect(model.detailLine(for: never, now: now) == "never connected · paired Aug 1, 2026")
    }

    @Test func revokeConfirmBodiesDifferForSameBaseRows() {
        let model = JournalDevicesModel(client: FakeDevicesClient())
        let now = fixedDate("2026-08-01T12:04:00Z")
        let live = device(
            label: "iPhone",
            deviceLabel: "iPhone",
            pairedAt: "2026-08-01T12:00:00Z",
            lastSeenAt: "2026-08-01T12:00:00Z",
            fingerprint: "live"
        )
        let never = device(
            label: "iPhone",
            deviceLabel: "iPhone",
            pairedAt: "2026-07-30T12:00:00Z",
            lastSeenAt: nil,
            fingerprint: "never"
        )

        let liveBody = DevicesCopy.revokeBody(
            model.displayName(for: live),
            detail: model.detailLine(for: live, now: now)
        )
        let neverBody = DevicesCopy.revokeBody(
            model.displayName(for: never),
            detail: model.detailLine(for: never, now: now)
        )

        #expect(liveBody != neverBody)
    }

    @Test func groupsSortConnectedThenUnknownThenNeverConnectedByFingerprint() async throws {
        let client = FakeDevicesClient(listResults: [.success([
            device(label: "never", lastSeenAt: nil, fingerprint: "z"),
            device(label: "live", lastSeenAt: "2026-08-01T12:00:00Z", fingerprint: "b"),
            device(label: "unknown", lastSeenAt: "123", fingerprint: "a"),
            device(label: "peer never", role: "peer", lastSeenAt: nil, fingerprint: "peer-z"),
            device(label: "peer live", role: "peer", lastSeenAt: "2026-08-01T12:00:00Z", fingerprint: "peer-b"),
            device(label: "peer unknown", role: "peer", lastSeenAt: "now", fingerprint: "peer-a"),
        ])])
        let model = JournalDevicesModel(client: client)

        await model.loadDevices()

        #expect(model.yourDevicesGroup.rows.map(\.fingerprint) == ["a", "b", "z"])
        #expect(model.peerJournalsGroup.rows.map(\.fingerprint) == ["peer-a", "peer-b", "peer-z"])
    }

    @Test func loadStatesMapEmptyNotRunningAndNotReady() async throws {
        let empty = JournalDevicesModel(client: FakeDevicesClient(listResults: [.success([])]))
        await empty.loadDevices()
        #expect(empty.loadState == .empty)

        let notRunning = JournalDevicesModel(client: FakeDevicesClient(listResults: [
            .failure(.transport("\(NSURLErrorDomain):\(URLError.cannotConnectToHost.rawValue)")),
        ]))
        await notRunning.loadDevices()
        #expect(notRunning.loadState == .notRunning)

        let notReady = JournalDevicesModel(client: FakeDevicesClient(listResults: [.failure(.notReady)]))
        await notReady.loadDevices()
        #expect(notReady.loadState == .notReady)
    }

    @Test func detailLineRendersWhenPartsAreMissingWithoutDanglingSeparators() {
        let model = JournalDevicesModel(client: FakeDevicesClient())
        let detail = model.detailLine(
            for: device(label: "phone", pairedAt: nil, lastSeenAt: nil, fingerprint: "a"),
            now: fixedDate("2026-08-01T12:04:00Z")
        )

        #expect(detail == "never connected")
        #expect(!detail.hasPrefix(" · "))
        #expect(!detail.hasSuffix(" · "))
        #expect(!detail.contains(" ·  · "))
    }

    @Test func parserAcceptsGroundedISOShapesAndRejectsLossyStringsAsUnknown() {
        let model = JournalDevicesModel(client: FakeDevicesClient())
        let now = fixedDate("2026-08-01T12:04:00Z")
        let accepted = [
            "2026-08-01T12:00:00Z",
            "2026-08-01T12:00:00.000Z",
            "2026-08-01T08:00:00-04:00",
        ]
        for value in accepted {
            let detail = model.detailLine(
                for: device(label: "phone", lastSeenAt: value, fingerprint: value),
                now: now
            )
            #expect(detail == "last seen 4 minutes ago")
        }

        for value in ["123", "true", "now", "", "   "] {
            let detail = model.detailLine(
                for: device(label: "phone", lastSeenAt: value, fingerprint: value),
                now: now
            )
            #expect(detail == "last seen unknown")
        }

        #expect(
            model.detailLine(for: device(label: "phone", lastSeenAt: nil, fingerprint: "nil"), now: now)
                == "never connected"
        )
    }

    @Test func unparseableLastSeenIsUnknownAndDoesNotSortLast() async throws {
        let client = FakeDevicesClient(listResults: [.success([
            device(label: "never", lastSeenAt: nil, fingerprint: "z"),
            device(label: "unknown", lastSeenAt: "123", fingerprint: "a"),
        ])])
        let model = JournalDevicesModel(client: client)
        await model.loadDevices()
        let now = fixedDate("2026-08-01T12:04:00Z")

        #expect(model.detailLine(for: model.yourDevicesGroup.rows[0], now: now) == "last seen unknown")
        #expect(model.detailLine(for: model.yourDevicesGroup.rows[1], now: now) == "never connected")
        #expect(model.yourDevicesGroup.rows.map(\.fingerprint) == ["a", "z"])
    }

    @Test func revokeSuccessDismissesAndRefreshes() async throws {
        let client = FakeDevicesClient(
            listResults: [
                .success([device(label: "phone", fingerprint: "a")]),
                .success([]),
            ],
            unpairResults: [.success(UnpairResponse(unpaired: "a"))]
        )
        let model = JournalDevicesModel(client: client)
        await model.loadDevices()
        let row = try #require(model.devices.first)
        model.beginRevoke(row)

        await model.confirmRevoke()

        #expect(model.revokeCandidate == nil)
        #expect(model.revokeError == nil)
        #expect(model.loadState == .empty)
        #expect(await client.unpairRequests() == ["a"])
        #expect(await client.listCallCount() == 2)
    }

    @Test func revokeFailureKeepsDialogOpenAndSurfacesDetail() async throws {
        let client = FakeDevicesClient(
            listResults: [.success([device(label: "phone", fingerprint: "a")])],
            unpairResults: [.failure(.server(.init(error: "no", detail: "try again later")))]
        )
        let model = JournalDevicesModel(client: client)
        await model.loadDevices()
        let row = try #require(model.devices.first)
        model.beginRevoke(row)

        await model.confirmRevoke()

        #expect(model.revokeCandidate == row)
        #expect(model.revokeError == "try again later")
        #expect(await client.listCallCount() == 1)
    }

    @Test func pairedDeviceNotFoundRefreshesAndDismissesWithoutError() async throws {
        let client = FakeDevicesClient(
            listResults: [
                .success([device(label: "phone", fingerprint: "a")]),
                .success([]),
            ],
            unpairResults: [
                .failure(.server(.init(reasonCode: JournalDevicesErrorEnvelope.pairedDeviceNotFound, detail: "gone"))),
            ]
        )
        let model = JournalDevicesModel(client: client)
        await model.loadDevices()
        let row = try #require(model.devices.first)
        model.beginRevoke(row)

        await model.confirmRevoke()

        #expect(model.revokeCandidate == nil)
        #expect(model.revokeError == nil)
        #expect(model.loadState == .empty)
        #expect(await client.listCallCount() == 2)
    }

    @Test func pairOpenThenUsedKeepsTheViewUpAndRefreshes() async throws {
        let clock = ManualDevicesClock()
        let client = FakeDevicesClient(
            listResults: [.success([device(label: "paired", fingerprint: "p")])],
            pairResults: [.success(pair(nonce: "n", link: "https://go.solstone.app/p#abc", expiresIn: 30))],
            nonceResults: [.success(NonceStatusResponse(
                present: true,
                used: true,
                device: .init(cid: "p", displayLabel: "paired")
            ))],
            unpairResults: [.success(UnpairResponse(unpaired: "p"))]
        )
        let model = JournalDevicesModel(client: client, clock: clock)

        model.openPairing()
        // Pairing tests own the product polling task; close it on every exit path.
        defer { model.closePairing() }
        try await waitUntil { if case .open = model.pairingState { return true }; return false }
        try await waitUntil { clock.sleepCallCount() >= 1 }
        clock.advance(by: .seconds(1))
        try await waitUntil { clock.sleepCallCount() >= 2 }
        clock.advance(by: .seconds(1))
        try await waitUntil { model.loadState == .loaded && model.pairingStatusToken == "paired" }

        #expect(model.isPairingPresented)
        #expect(model.devices.first?.displayLabel == "paired")
        #expect(await client.nonceRequests() == ["n"])

        await model.unpairPairedDevice()
        #expect(model.pairedUnpairState == .removed)
        #expect(await client.unpairRequests() == ["p"])
        #expect(model.isPairingPresented)

        model.closePairing()
        #expect(model.isPairingPresented == false)
        #expect(model.pairedUnpairState == .idle)
    }

    @Test func pairStartFailureRendersOpenFailed() async throws {
        let client = FakeDevicesClient(pairResults: [.failure(.server(.init(detail: "not ready")))])
        let model = JournalDevicesModel(client: client, clock: ManualDevicesClock())

        model.openPairing()
        defer { model.closePairing() }
        try await waitUntil { if case .openFailed = model.pairingState { return true }; return false }

        #expect(model.pairingState == .openFailed(detail: "not ready"))
    }

    @Test func localCountdownExpiryWinsOverStalePresentTrueResponse() async throws {
        let clock = ManualDevicesClock()
        let client = FakeDevicesClient(
            pairResults: [.success(pair(nonce: "n", link: "https://go.solstone.app/p#abc", expiresIn: 3))],
            nonceResults: [.success(NonceStatusResponse(present: true, used: true))],
            holdNonce: true
        )
        let model = JournalDevicesModel(client: client, clock: clock)

        model.openPairing()
        defer { model.closePairing() }
        try await waitUntil { if case .open = model.pairingState { return true }; return false }
        try await waitUntil { clock.sleepCallCount() >= 1 }
        clock.advance(by: .seconds(1))
        try await waitUntil { clock.sleepCallCount() >= 2 }
        clock.advance(by: .seconds(1))
        try await waitUntil { await client.nonceCallCount() == 1 }
        clock.advance(by: .seconds(2))
        await client.releaseHeldNonce()

        try await waitUntil { if case .expired = model.pairingState { return true }; return false }
        #expect(model.isPairingPresented)
        #expect(model.pairingStatusToken == "expired")
    }

    @Test func stalePollFromPreviousGenerationCannotPairAfterReopen() async throws {
        let clock = ManualDevicesClock()
        let client = FakeDevicesClient(
            pairResults: [
                .success(pair(nonce: "old", link: "https://go.solstone.app/p#old", expiresIn: 30)),
                .success(pair(nonce: "new", link: "https://go.solstone.app/p#new", expiresIn: 30)),
            ],
            nonceResults: [.success(NonceStatusResponse(present: true, used: true))],
            holdNonce: true
        )
        let model = JournalDevicesModel(client: client, clock: clock)

        model.openPairing()
        defer { model.closePairing() }
        try await waitUntil { if case let .open(link, _, _, _) = model.pairingState { return link.hasSuffix("old") }; return false }
        try await waitUntil { clock.sleepCallCount() >= 1 }
        clock.advance(by: .seconds(1))
        try await waitUntil { clock.sleepCallCount() >= 2 }
        clock.advance(by: .seconds(1))
        try await waitUntil { await client.nonceCallCount() == 1 }

        model.openPairing()
        try await waitUntil { if case let .open(link, _, _, _) = model.pairingState { return link.hasSuffix("new") }; return false }
        await client.releaseHeldNonce()
        await Task.yield()

        #expect(model.isPairingPresented)
        if case let .open(link, _, _, nonce) = model.pairingState {
            #expect(link.hasSuffix("new"))
            #expect(nonce == "new")
        } else {
            Issue.record("expected new open state")
        }
    }

    @Test func closingSheetStopsPolling() async throws {
        let clock = ManualDevicesClock()
        let client = FakeDevicesClient(
            pairResults: [.success(pair(nonce: "n", link: "https://go.solstone.app/p#abc", expiresIn: 30))]
        )
        let model = JournalDevicesModel(client: client, clock: clock)

        model.openPairing()
        defer { model.closePairing() }
        try await waitUntil { if case .open = model.pairingState { return true }; return false }
        try await waitUntil { clock.sleepCallCount() >= 1 }
        model.closePairing()
        clock.advance(by: .seconds(10))
        await Task.yield()

        #expect(model.pairingState == .idle)
        #expect(await client.nonceCallCount() == 0)
    }

    private func device(
        label: String,
        deviceLabel: String? = nil,
        role: String? = nil,
        pairedAt: String? = nil,
        lastSeenAt: String? = nil,
        fingerprint: String
    ) -> DeviceRow {
        DeviceRow(
            displayLabel: label,
            deviceLabel: deviceLabel,
            role: role,
            pairedAt: pairedAt,
            lastSeenAt: lastSeenAt,
            fingerprint: fingerprint
        )
    }

    private func pair(nonce: String, link: String, expiresIn: Int) -> PairStartResponse {
        PairStartResponse(
            nonce: nonce,
            pairLink: link,
            expiresIn: expiresIn,
            deviceLabel: "",
            caFingerprint: "ca"
        )
    }

    private func fixedDate(_ value: String) -> Date {
        ISO8601DateFormatter().date(from: value)!
    }
}

private enum FakeOutcome<Value: Sendable>: Sendable {
    case success(Value)
    case failure(JournalDevicesClientError)

    func get() throws -> Value {
        switch self {
        case .success(let value):
            return value
        case .failure(let error):
            throw error
        }
    }
}

private actor FakeDevicesClient: JournalDevicesClientProtocol {
    private var listResults: [FakeOutcome<[DeviceRow]>]
    private var pairResults: [FakeOutcome<PairStartResponse>]
    private var nonceResults: [FakeOutcome<NonceStatusResponse>]
    private var unpairResults: [FakeOutcome<UnpairResponse>]
    private var holdNonce: Bool
    private var heldNonceContinuation: CheckedContinuation<Void, Never>?

    private(set) var listCalls = 0
    private(set) var nonceCalls = 0
    private var recordedUnpairRequests: [String] = []
    private var recordedNonceRequests: [String] = []

    init(
        listResults: [FakeOutcome<[DeviceRow]>] = [],
        pairResults: [FakeOutcome<PairStartResponse>] = [],
        nonceResults: [FakeOutcome<NonceStatusResponse>] = [],
        unpairResults: [FakeOutcome<UnpairResponse>] = [],
        holdNonce: Bool = false
    ) {
        self.listResults = listResults
        self.pairResults = pairResults
        self.nonceResults = nonceResults
        self.unpairResults = unpairResults
        self.holdNonce = holdNonce
    }

    func listDevices() async throws -> [DeviceRow] {
        listCalls += 1
        return try next(&listResults).get()
    }

    func startPairing() async throws -> PairStartResponse {
        try next(&pairResults).get()
    }

    func nonceStatus(nonce: String) async throws -> NonceStatusResponse {
        nonceCalls += 1
        recordedNonceRequests.append(nonce)
        if holdNonce {
            await withCheckedContinuation { continuation in
                heldNonceContinuation = continuation
            }
        }
        return try next(&nonceResults).get()
    }

    func unpairDevice(fingerprint: String) async throws -> UnpairResponse {
        recordedUnpairRequests.append(fingerprint)
        return try next(&unpairResults).get()
    }

    func releaseHeldNonce() {
        holdNonce = false
        let continuation = heldNonceContinuation
        heldNonceContinuation = nil
        continuation?.resume()
    }

    func listCallCount() -> Int { listCalls }
    func nonceCallCount() -> Int { nonceCalls }
    func unpairRequests() -> [String] { recordedUnpairRequests }
    func nonceRequests() -> [String] { recordedNonceRequests }

    private func next<Value>(_ results: inout [FakeOutcome<Value>]) -> FakeOutcome<Value> {
        guard !results.isEmpty else {
            return .failure(.serverStatus(599))
        }
        return results.removeFirst()
    }
}

private final class ManualDevicesClock: MonotonicClock, @unchecked Sendable {
    private let lock = NSLock()
    private var value: Duration = .zero
    private var continuations: [(deadline: Duration, continuation: CheckedContinuation<Void, Never>)] = []
    private var sleepRequests = 0

    func now() -> Duration {
        lock.withLock { value }
    }

    func sleep(for duration: Duration) async {
        await withCheckedContinuation { continuation in
            var shouldResume = false
            lock.withLock {
                sleepRequests += 1
                let deadline = value + duration
                if value >= deadline {
                    shouldResume = true
                } else {
                    continuations.append((deadline: deadline, continuation: continuation))
                }
            }
            if shouldResume {
                continuation.resume()
            }
        }
    }

    func sleepCallCount() -> Int {
        lock.withLock { sleepRequests }
    }

    func advance(by duration: Duration) {
        let pending: [CheckedContinuation<Void, Never>]
        lock.lock()
        value += duration
        pending = continuations
            .filter { $0.deadline <= value }
            .map(\.continuation)
        continuations.removeAll { $0.deadline <= value }
        lock.unlock()

        for continuation in pending {
            continuation.resume()
        }
    }
}

/// Pumps the MainActor until `condition` holds, measuring progress in cooperative
/// scheduling turns rather than wall-clock time.
///
/// The pairing tests drive all time deterministically through `ManualDevicesClock`,
/// so a wait only needs to let the ready product `pairingTask` reach its next
/// suspension. Counting turns (not seconds) keeps these waits immune to MainActor
/// saturation from concurrently-running `@MainActor` suites (e.g. snapshot rendering):
/// a wall-clock deadline would elapse during such a freeze and time out spuriously,
/// even though no test progress was pending. Suites that exercise real product
/// `Task.sleep` timers (Tunnel/Mark) correctly use the wall-clock `waitUntil` instead.
@MainActor
private func waitUntil(
    maxTurns: Int = 2000,
    _ condition: @escaping @MainActor () async -> Bool
) async throws {
    for _ in 0..<max(1, maxTurns) {
        if await condition() { return }
        await Task.yield()
    }
    Issue.record("timed out waiting for condition")
}
