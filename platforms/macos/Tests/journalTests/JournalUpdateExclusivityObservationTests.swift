// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import Foundation
import JournalRuntime
import JournalRuntimeTestSupport
import os
import Testing
@testable import journal
@testable import UpdateKit

@MainActor
@Suite("JournalUpdateExclusivityObservation")
struct JournalUpdateExclusivityObservationTests {
    @Test func deferredBackgroundCheckRefiresWhenSupervisorLeavesStartupState() async throws {
        let materializer = PausingRuntimeMaterializer(runtime: try makeRuntime())
        let runner = ObservationRunner()
        let supervisor = JournalSupervisor(
            gate: MockSingleSupervisorGate(),
            materializer: materializer,
            runner: runner,
            readinessGate: MockJournalReadinessGate(result: .ready)
        )
        _ = configureInMemoryReceiptContext(supervisor)
        let root = try makeTemporaryDirectory()
        let startTask = Task { @MainActor in
            await supervisor.start(journalRoot: root)
        }
        try await waitUntil(timeout: .seconds(2)) {
            supervisor.state == .materializing
        }
        let defaults = JournalUpdateIsolatedUserDefaults()
        defer { defaults.clear() }
        let spy = JournalSpyUpdater()
        let controller = UpdateController(
            feedURL: "https://updates.solstone.app/journal-macos/appcast.xml",
            publicKey: "5EP/CLtfMrN2qC8zWsHeIWcPVPjqFH7hW4m8cGX7Qg0=",
            log: Logger.updates,
            errorDomain: "app.solstone.journal.updates",
            exclusivity: { [weak supervisor] in
                guard let supervisor else { return false }
                switch supervisor.state {
                case .materializing, .starting, .waitingForReadiness:
                    return true
                default:
                    return false
                }
            },
            defaults: defaults.defaults
        ) { _, _ in spy }

        #expect(controller.exclusiveOperationInProgress)
        #expect(!controller.shouldAllowSparkleUpdateCheck(.updatesInBackground))
        #expect(spy.checkForUpdatesCallCount == 0)

        materializer.release()
        #expect(await startTask.value)
        #expect(supervisor.state == .running)
        try await waitUntil(timeout: .seconds(2)) {
            spy.checkForUpdatesCallCount == 1
        }
    }
}

private final class PausingRuntimeMaterializer: RuntimeMaterializing, @unchecked Sendable {
    private let lock = NSLock()
    private let runtime: MaterializedRuntime
    private var continuation: CheckedContinuation<Void, Never>?
    private var released = false

    init(runtime: MaterializedRuntime) {
        self.runtime = runtime
    }

    func materialize(excludingLiveKey liveKey: String?) async throws -> MaterializedRuntime {
        await withCheckedContinuation { continuation in
            let shouldResume = lock.withLock {
                if released {
                    return true
                }
                self.continuation = continuation
                return false
            }
            if shouldResume {
                continuation.resume()
            }
        }
        return runtime
    }

    func release() {
        let continuation = lock.withLock {
            released = true
            defer { self.continuation = nil }
            return self.continuation
        }
        continuation?.resume()
    }
}

private actor ObservationRunner: SupervisedChildRunning {
    private var runtimeKey: String?

    func start(
        runtime: MaterializedRuntime,
        journalRoot: URL,
        port: Int,
        receiptContext: JournalRuntimeEntryReceiptContext
    ) async throws {
        runtimeKey = runtime.key
    }

    func restart() async throws {}

    func stop() async {
        runtimeKey = nil
    }

    func stopForTermination() async {
        runtimeKey = nil
    }

    func currentRuntimeKey() async -> String? {
        runtimeKey
    }

    func currentIdentity() async -> SupervisedChildIdentity? {
        nil
    }

    func terminalReason() async -> JournalDiagnostic? {
        nil
    }

    func markReady(identity: SupervisedChildIdentity) async -> Bool { true }
}

@MainActor
private final class JournalSpyUpdater: SparkleUpdating {
    var automaticallyChecksForUpdates = true
    var updateCheckInterval: TimeInterval = 86_400
    var automaticallyDownloadsUpdates = false
    var sessionInProgress = false
    var checkForUpdatesCallCount = 0
    var startCallCount = 0

    var userAgentString = "journal Sparkle"

    func checkForUpdates() {
        checkForUpdatesCallCount += 1
    }

    func checkForUpdatesInBackground() {}

    func start() throws {
        startCallCount += 1
    }
}

private struct JournalUpdateIsolatedUserDefaults {
    let suiteName: String
    let defaults: UserDefaults

    init() {
        suiteName = "app.solstone.journal.update-exclusivity.\(UUID().uuidString)"
        defaults = UserDefaults(suiteName: suiteName)!
        defaults.removePersistentDomain(forName: suiteName)
    }

    func clear() {
        defaults.removePersistentDomain(forName: suiteName)
    }
}

@MainActor
private func waitUntil(
    timeout: Duration,
    poll: Duration = .milliseconds(50),
    _ predicate: () async -> Bool
) async throws {
    let clock = ContinuousClock()
    let deadline = clock.now.advanced(by: timeout)
    while clock.now < deadline {
        if await predicate() { return }
        try await Task.sleep(for: poll)
    }
    #expect(await predicate())
}

private func makeTemporaryDirectory() throws -> URL {
    let url = FileManager.default.temporaryDirectory
        .appendingPathComponent("journal-update-observation-\(UUID().uuidString)", isDirectory: true)
    try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
    return url
}
