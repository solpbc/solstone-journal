// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import AppKit
import Foundation
import JournalMarkKit
import JournalRuntime
import ServiceManagement
import SolstoneCore
import SwiftUI
import Testing
@testable import journal

@MainActor
extension JournalFirstRunModelTests {
    @Test func unansweredInitialDeliveryStaysVisibleUntilExplicitRetry() async throws {
        let world = try makeSetupWorld(configureRoot: true, done: true, supervisorLive: true, probes: [.complete])
        defer { world.refresh.release() }
        try seedContinuation(world.fixture.continuation, root: world.root)
        await world.fixture.model.reconcileActivation()
        let pending = try #require(try world.fixture.continuation.loadActive()?.pending)
        #expect(world.peer.launched.count == 1)
        #expect(world.fixture.stepAsides.count == 1)
        await world.fixture.model.reconcileForReopen()
        await world.fixture.model.reconcileForReopen()
        #expect(world.peer.launched.count == 1)
        #expect(world.fixture.stepAsides.count == 1)
        #expect(world.fixture.model.handoffError != nil)
        #expect(world.fixture.model.route == .ritual(.setupProgress))
        await world.fixture.model.retryHandoff()
        guard case .ready(let sent) = world.fixture.mailbox.loadStep5() else { Issue.record("retry must deliver"); return }
        #expect(sent.handoffID == pending)
        #expect(world.peer.launched.count == 2)
        #expect(world.fixture.stepAsides.count == 2)
    }

    @Test(arguments: [false, true])
    func matchingRefusalSurvivesRestartAndOwnerAcknowledgement(local: Bool) async throws {
        let world = try makeSetupWorld(configureRoot: true, done: true, supervisorLive: true, probes: [.complete])
        defer { world.refresh.release() }
        let owner: UUID
        if local { owner = try world.fixture.continuation.storeLocal(root: world.root) }
        else { owner = try seedContinuation(world.fixture.continuation, root: world.root) }
        await world.fixture.model.reconcileActivation()
        let pending = try #require(try world.fixture.continuation.loadActive()?.pending)
        try JSONEncoder().encode(SetupHandoffRefusal(handoffID: pending, reason: .heldPairing))
            .write(to: world.fixture.mailbox.directory.appendingPathComponent("solstone-setup-handoff-refused.json"))
        await world.fixture.model.reconcileForReopen()
        #expect(try world.fixture.continuation.loadActive() == nil)
        #expect(world.fixture.model.handoffRefusal?.handoffID == pending)
        #expect(world.peer.launched.count == 1)
        #expect(world.fixture.stepAsides.count == 1)
        let restarted = makeModel(config: world.fixture.config, startResults: [false, true], probeResults: [.complete],
            localSetupCapability: { true }, loadSetupCopy: { try? loadSetupFixtureCopy() }, peer: world.peer,
            setupClient: world.client, identity: world.identity, mailbox: world.fixture.mailbox,
            continuation: JournalSetupContinuationStore(directory: world.fixture.continuation.directory),
            supervisorIsLive: { _ in false })
        await restarted.model.reconcileActivation()
        #expect(restarted.model.handoffRefusal?.handoffID == pending)
        await restarted.model.openJournalAfterRefusal()
        #expect(restarted.browser.opened.isEmpty)
        #expect(restarted.model.handoffRefusal?.handoffID == pending)
        #expect(restarted.model.pendingOperation == .none)
        #expect(!restarted.model.userOperationInFlight)
        await restarted.model.openJournalAfterRefusal()
        #expect(restarted.browser.opened == [URL(string: JournalFirstRunModel.setupBaseURL + "/")!])
        #expect(restarted.model.route == .home)
        await restarted.model.reconcileForReopen()
        #expect(restarted.model.handoffRefusal == nil)
        #expect(try world.fixture.continuation.loadActive() == nil)
        if !local {
            try writeStep1(world.fixture.mailbox, id: owner)
            await restarted.model.reconcileActivation()
            #expect(try world.fixture.continuation.loadActive() == nil)
        }
        let fresh = try writeStep1(world.fixture.mailbox)
        await restarted.model.reconcileActivation()
        #expect(world.fixture.continuation.acceptance(of: fresh) == .accepted)
        #expect(try world.fixture.continuation.loadActive()?.pending != pending)
        #expect(world.peer.launched.count == 2)
    }

    @Test(arguments: [false, true])
    func staleRefusalDoesNotResolveAndAcceptanceWins(matchingAcceptance: Bool) async throws {
        let world = try makeSetupWorld(configureRoot: true, done: true, supervisorLive: true, probes: [.complete])
        defer { world.refresh.release() }
        let pending = UUID()
        try seedContinuation(world.fixture.continuation, root: world.root, pending: pending)
        try JSONEncoder().encode(SetupHandoffRefusal(handoffID: matchingAcceptance ? pending : UUID(), reason: .heldPairing))
            .write(to: world.fixture.mailbox.directory.appendingPathComponent("solstone-setup-handoff-refused.json"))
        if matchingAcceptance { try writeAcceptedReceipt(world.fixture.mailbox, id: pending, step: 5) }
        await world.fixture.model.reconcileActivation()
        #expect(world.fixture.model.handoffRefusal == nil)
        #expect((try world.fixture.continuation.loadActive() == nil) == matchingAcceptance)
        #expect(world.peer.launched.isEmpty)
    }

    @Test func newExplicitIntentAfterRefusalGetsItsOwnDelivery() async throws {
        let world = try makeSetupWorld(configureRoot: true, done: true, supervisorLive: true, probes: [.complete])
        defer { world.refresh.release() }
        try seedContinuation(world.fixture.continuation, root: world.root)
        await world.fixture.model.reconcileActivation()
        let first = try #require(try world.fixture.continuation.loadActive()?.pending)
        try JSONEncoder().encode(SetupHandoffRefusal(handoffID: first, reason: .heldPairing))
            .write(to: world.fixture.mailbox.directory.appendingPathComponent("solstone-setup-handoff-refused.json"))
        let fresh = try writeStep1(world.fixture.mailbox)
        await world.fixture.model.reconcileActivation()
        let active = try #require(try world.fixture.continuation.loadActive())
        #expect(active.owner == fresh)
        #expect(active.pending != first)
        #expect(world.peer.launched.count == 2)
        #expect(world.fixture.model.handoffRefusal == nil)
    }

    @Test func warmActivationWaitsForTheWholeMarkAction() async throws {
        let world = try makeSetupWorld(configureRoot: true, supervisorLive: true, probes: [.incomplete])
        defer { world.refresh.release() }
        try seedContinuation(world.fixture.continuation, root: world.root)
        await world.fixture.model.reconcileActivation()
        let gate = PerformGate()
        defer { gate.release() }
        world.client.gate = gate
        world.client.setHold(true)
        let locked = setupState(root: world.root, markState: "locked", proposalID: "committed")
        world.client.setPerforms([locked])
        world.client.setGets([locked])
        let before = world.client.stateCalls
        let action = Task { await world.fixture.model.commitDisplayedMark() }
        await gate.waitUntilEntered()
        let activation = Task { await world.fixture.model.reconcileActivation() }
        await Task.yield()
        #expect(world.client.stateCalls == before)
        #expect(world.fixture.model.displayedProposalID == "proposal-1")
        #expect(world.fixture.model.pendingOperation == .mark)
        gate.release()
        await action.value
        await activation.value
        #expect(world.client.actions == [.commitMark(proposalID: "proposal-1")])
        #expect(world.fixture.model.setupState?.mark.state == .locked)
        #expect(world.fixture.model.pendingOperation == .none)
    }

    @Test func localFinalizeAcceptsConcurrentCaptureBeforeMatchingReceiptRetiresIt() async throws {
        let world = try makeSetupWorld(configureRoot: true, markState: "locked", saved: true,
                                      choice: "later", probes: [.incomplete, .incomplete])
        defer { world.refresh.release() }
        await world.fixture.model.reconcileActivation()
        await world.fixture.model.continueFromFolder()
        let gate = PerformGate()
        defer { gate.release() }
        world.client.gate = gate
        world.client.setHold(true)
        let done = setupState(root: world.root, done: true, markState: "locked", saved: true, choice: "later")
        world.client.setPerforms([done])
        world.client.setGets([done])
        world.peer.onLaunch = {
            guard case .ready(let sent) = world.fixture.mailbox.loadStep5() else {
                Issue.record("delivery must precede peer launch")
                return
            }
            try writeAcceptedReceipt(world.fixture.mailbox, id: sent.handoffID, step: 5)
        }
        let action = Task { await world.fixture.model.finalizeThinking() }
        await gate.waitUntilEntered()
        let pending = try #require(try world.fixture.continuation.loadActive()?.pending)
        let captureID = try writeStep1(world.fixture.mailbox)
        let activation = Task { await world.fixture.model.reconcileActivation() }
        await Task.yield()
        #expect(world.fixture.continuation.acceptance(of: captureID) == .notAccepted)
        gate.release()
        await action.value
        await activation.value
        #expect(world.peer.launched == [setupPeerURL])
        #expect(try world.fixture.mailbox.readStep5Receipt()?.handoffID == pending)
        #expect(try world.fixture.mailbox.readStep1Receipt()?.handoffID == captureID)
        #expect(world.fixture.continuation.acceptance(of: captureID) == .accepted)
        #expect(try world.fixture.continuation.loadActive() == nil)
        await world.fixture.model.reconcileActivation()
        #expect(world.peer.launched.count == 1)
    }

    @Test func retiredReplayDoesNotSuppressANewerActiveObligation() async throws {
        let world = try makeSetupWorld(configureRoot: true, done: true, probes: [.complete])
        defer { world.refresh.release() }
        let retired = UUID(), current = UUID()
        try seedContinuation(world.fixture.continuation, root: world.root, step1: retired)
        let retiredPending = try world.fixture.continuation.mintPending(step1: retired)
        try world.fixture.continuation.retire(matchingPendingStep5: retiredPending)
        try seedContinuation(world.fixture.continuation, root: world.root, step1: current)
        let pending = try world.fixture.continuation.mintPending(step1: current)
        try writeStep1(world.fixture.mailbox, id: retired)
        await world.fixture.model.reconcileActivation()
        #expect(try world.fixture.mailbox.readStep1Receipt()?.handoffID == retired)
        #expect(try world.fixture.continuation.loadActive()?.owner == current)
        #expect(try world.fixture.continuation.loadActive()?.pending == pending)
        #expect(world.peer.launched.isEmpty)
        #expect(world.fixture.model.handoffError != nil)
        await world.fixture.model.retryHandoff()
        guard case .ready(let delivered) = world.fixture.mailbox.loadStep5() else {
            Issue.record("replay must not hide current delivery")
            return
        }
        #expect(delivered.handoffID == pending)
        #expect(world.peer.launched.count == 1)
    }

    @Test func newerReceiptAfterSnapshotWaitsForTheNextReconciliation() async throws {
        let world = try makeSetupWorld(configureRoot: true, done: true, probes: [.complete])
        defer { world.refresh.release() }
        let owner = UUID()
        try seedContinuation(world.fixture.continuation, root: world.root, step1: owner)
        let pending = try world.fixture.continuation.mintPending(step1: owner)
        let incoming = try writeStep1(world.fixture.mailbox)
        world.fixture.model.onBeforeStep1Commit = {
            try? writeAcceptedReceipt(world.fixture.mailbox, id: pending, step: 5)
        }
        await world.fixture.model.reconcileActivation()
        #expect(world.fixture.continuation.acceptance(of: incoming) == .accepted)
        #expect(try world.fixture.continuation.loadActive()?.pending == pending)
        #expect(world.peer.launched.isEmpty)
        world.fixture.model.onBeforeStep1Commit = nil
        await world.fixture.model.reconcileActivation()
        #expect(try world.fixture.continuation.loadActive() == nil)
        #expect(world.peer.launched.isEmpty)
    }

    @Test func localSupervisorFailureRetryDeliversTheSamePendingIdentity() async throws {
        let world = try makeSetupWorld(configureRoot: true, done: true, startResults: [false, true], probes: [.complete])
        defer { world.refresh.release() }
        let owner = try world.fixture.continuation.storeLocal(root: world.root)
        let pending = try world.fixture.continuation.mintPending(step1: owner)
        await world.fixture.model.reconcileActivation()
        #expect(world.fixture.model.route == .ritual(.setupProgress))
        #expect(world.fixture.model.errorMessage?.isEmpty == false)
        await world.fixture.model.retrySetup()
        guard case .ready(let delivered) = world.fixture.mailbox.loadStep5() else {
            Issue.record("successful retry must resume the local obligation")
            return
        }
        #expect(delivered.handoffID == pending)
        #expect(try world.fixture.continuation.loadActive()?.owner == owner)
    }

    @Test func draftChoiceHasNoSideEffectAndOneContinueSavesBeforeFinalizing() async throws {
        let world = try makeSetupWorld(configureRoot: true, markState: "locked", saved: true, choice: "local",
                                      probes: [.incomplete, .complete])
        defer { world.refresh.release() }
        try seedContinuation(world.fixture.continuation, root: world.root)
        await world.fixture.model.reconcileActivation()
        world.fixture.model.selectThinking("later")
        #expect(world.client.actions.isEmpty)
        #expect(world.fixture.model.displayedThinkingChoice == .later)
        let saved = setupState(root: world.root, markState: "locked", saved: true, choice: "later")
        let done = setupState(root: world.root, done: true, markState: "locked", saved: true, choice: "later")
        world.client.setPerforms([saved, done])
        await world.fixture.model.continueThinking()
        #expect(world.client.actions == [.saveThinking(choice: "later"),
                                        .finalize(timezone: TimeZone.autoupdatingCurrent.identifier)])
        #expect(world.peer.launched.count == 1)
        #expect(world.fixture.stepAsides.count == 1)
    }

    @Test func mismatchedChoiceSaveNeverFinalizesTheOlderChoice() async throws {
        let world = try makeSetupWorld(configureRoot: true, markState: "locked", saved: true, choice: "local")
        defer { world.refresh.release() }
        try seedContinuation(world.fixture.continuation, root: world.root)
        await world.fixture.model.reconcileActivation()
        world.fixture.model.selectThinking("later")
        world.client.setPerforms([setupState(root: world.root, markState: "locked", saved: true, choice: "local")])
        await world.fixture.model.continueThinking()
        #expect(world.client.actions == [.saveThinking(choice: "later")])
        #expect(world.fixture.model.thinkingFailure?.isEmpty == false)
        #expect(world.peer.launched.isEmpty)
        #expect(world.fixture.stepAsides.count == 0)
    }

    @Test func activationWaitsAcrossChoiceSaveCompletionProbeAndOutboundVerification() async throws {
        let completionGate = PerformGate(), outboundGate = PerformGate(), saveGate = PerformGate()
        defer { completionGate.release(); outboundGate.release(); saveGate.release() }
        let probe = HeldCompletionProbe(gate: completionGate)
        let world = try makeSetupWorld(configureRoot: true, markState: "locked", saved: false,
                                      initClient: probe)
        defer { world.refresh.release() }
        try seedContinuation(world.fixture.continuation, root: world.root)
        await world.fixture.model.reconcileActivation()
        world.fixture.model.selectThinking("later")
        let saved = setupState(root: world.root, markState: "locked", saved: true, choice: "later")
        let done = setupState(root: world.root, done: true, markState: "locked", saved: true, choice: "later")
        world.client.setPerforms([saved, done])
        world.client.setGets([done])
        world.client.gate = saveGate
        world.client.setHold(true)
        world.peer.onVerify = { count in if count == 2 { await outboundGate.enterAndWait() } }
        world.peer.onLaunch = {
            if case .ready(let sent) = world.fixture.mailbox.loadStep5() {
                try writeAcceptedReceipt(world.fixture.mailbox, id: sent.handoffID, step: 5)
            }
        }
        let before = world.client.stateCalls
        let action = Task { await world.fixture.model.continueThinking() }
        await saveGate.waitUntilEntered()
        let activation = Task { await world.fixture.model.reconcileActivation() }
        await Task.yield()
        #expect(world.client.stateCalls == before)
        world.client.setHold(false)
        saveGate.release()
        await completionGate.waitUntilEntered()
        #expect(world.fixture.model.userOperationInFlight)
        world.fixture.model.selectThinking("byo")
        #expect(world.fixture.model.displayedThinkingChoice == .later)
        #expect(world.client.stateCalls == before)
        completionGate.release()
        await outboundGate.waitUntilEntered()
        #expect(world.fixture.model.userOperationInFlight)
        #expect(world.client.stateCalls == before)
        #expect(world.fixture.mailbox.loadStep5() == .missing)
        outboundGate.release()
        await action.value
        await activation.value
        #expect(!world.fixture.model.userOperationInFlight)
        #expect(world.client.actions == [.saveThinking(choice: "later"),
                                        .finalize(timezone: TimeZone.autoupdatingCurrent.identifier)])
        #expect(world.peer.launched.count == 1)
        #expect(try world.fixture.continuation.loadActive() == nil)
    }

    @Test(arguments: ["cancel", "failure", "termination"])
    func deferredActivationSettlesAfterAnInterruptedAction(interruption: String) async throws {
        let world = try makeSetupWorld(configureRoot: true, supervisorLive: true, probes: [.incomplete, .incomplete])
        defer { world.refresh.release() }
        try seedContinuation(world.fixture.continuation, root: world.root)
        await world.fixture.model.reconcileActivation()
        let gate = PerformGate()
        defer { gate.release() }
        world.client.gate = gate
        world.client.setHold(true)
        let action = Task { await world.fixture.model.commitDisplayedMark() }
        await gate.waitUntilEntered()
        var settled = false
        let activation = Task { await world.fixture.model.reconcileActivation(); settled = true }
        await Task.yield()
        switch interruption {
        case "cancel": action.cancel()
        case "failure": world.client.setError(.transport("fixture unavailable"))
        default:
            await world.fixture.model.prepareForTermination()
            await activation.value
            #expect(settled)
        }
        gate.release()
        await action.value
        await activation.value
        #expect(settled)
        #expect(!world.fixture.model.userOperationInFlight)
        #expect(world.fixture.model.pendingOperation == .none)
        #expect(world.fixture.model.displayedProposalID == "proposal-1")
        #expect(world.peer.launched.isEmpty)
    }

    @Test func pendingReadFailureIsVisibleAndRetryUsesThePersistedIdentity() async throws {
        let reads = FailWrites()
        let world = try makeSetupWorld(configureRoot: true, markState: "locked", saved: true, choice: "later",
                                      probes: [.incomplete, .incomplete], continuationRead: { url in
            if reads.fail && url.lastPathComponent == JournalSetupContinuationStore.localFileName {
                throw CocoaError(.fileReadNoPermission)
            }
            return try Data(contentsOf: url)
        })
        defer { world.refresh.release() }
        await world.fixture.model.reconcileActivation()
        await world.fixture.model.continueFromFolder()
        world.client.setHook { _ in reads.fail = true }
        let done = setupState(root: world.root, done: true, markState: "locked", saved: true, choice: "later")
        world.client.setPerforms([done])
        await world.fixture.model.finalizeThinking()
        #expect(world.fixture.model.handoffError?.isEmpty == false)
        #expect(world.fixture.mailbox.loadStep5() == .missing)
        #expect(world.peer.launched.isEmpty)
        reads.fail = false
        let pending = try #require(try world.fixture.continuation.loadActive()?.pending)
        await world.fixture.model.retryHandoff()
        guard case .ready(let delivered) = world.fixture.mailbox.loadStep5() else {
            Issue.record("retry must deliver the retained pending identity")
            return
        }
        #expect(delivered.handoffID == pending)
    }

    @Test func conflictingIntentFilesDoNotChangeSelectedOrConfiguredRoots() async throws {
        let world = try makeSetupWorld(configureRoot: true)
        defer { world.refresh.release() }
        try seedContinuation(world.fixture.continuation, root: world.root)
        let selected = world.root.deletingLastPathComponent().appendingPathComponent("selected-other")
        world.fixture.model.journalRoot = selected
        let localURL = world.fixture.continuation.directory.appendingPathComponent(JournalSetupContinuationStore.localFileName)
        let local = try JSONSerialization.data(withJSONObject: [
            "schema": JournalSetupContinuationStore.localSchema,
            "localHandoffID": UUID().uuidString.lowercased(),
            "journalRootPath": world.root.path,
            "pendingStep5HandoffID": NSNull()
        ])
        try local.write(to: localURL, options: .atomic)
        let incomingBefore = continuationBytes(world.fixture.continuation)
        await world.fixture.model.reconcileActivation()
        #expect(world.fixture.model.journalRoot == selected)
        #expect(world.fixture.config.journalRoot == world.root)
        await world.fixture.model.continueFromFolder()
        #expect(world.fixture.model.journalRoot == selected)
        #expect(world.fixture.config.journalRoot == world.root)
        #expect(continuationBytes(world.fixture.continuation) == incomingBefore)
        #expect(try Data(contentsOf: localURL) == local)
        #expect(world.fixture.setupRunner.calls == 0)
        #expect(world.peer.launched.isEmpty)
    }

    @Test func failedRetirementIsVisibleAndRetryDoesNotRedeliver() async throws {
        let deletion = FailWrites()
        let world = try makeSetupWorld(configureRoot: true, done: true, probes: [.complete], continuationRemove: { url in
            if deletion.fail { throw CocoaError(.fileWriteNoPermission) }
            try FileManager.default.removeItem(at: url)
        })
        defer { world.refresh.release() }
        let owner = UUID()
        try seedContinuation(world.fixture.continuation, root: world.root, step1: owner)
        let pending = try world.fixture.continuation.mintPending(step1: owner)
        try writeAcceptedReceipt(world.fixture.mailbox, id: pending, step: 5)
        deletion.fail = true
        await world.fixture.model.reconcileActivation()
        #expect(world.fixture.model.handoffError?.isEmpty == false)
        #expect(try world.fixture.continuation.loadActive()?.pending == pending)
        #expect(world.peer.launched.isEmpty)
        deletion.fail = false
        await world.fixture.model.retryHandoff()
        #expect(try world.fixture.continuation.loadActive() == nil)
        #expect(world.peer.launched.isEmpty)
        #expect(world.fixture.model.route == .home)
    }

    @Test func aProposalWithoutARenderCannotBeCommittedAndCanRecover() async throws {
        let world = try makeSetupWorld(configureRoot: true, probes: [.incomplete])
        defer { world.refresh.release() }
        try seedContinuation(world.fixture.continuation, root: world.root)
        var incomplete = setupState(root: world.root)
        incomplete.mark.render = nil
        world.client.setGets([incomplete])
        await world.fixture.model.reconcileActivation()
        #expect(world.fixture.model.route == .ritual(.mark))
        #expect(!world.fixture.model.canCommitDisplayedMark)
        await world.fixture.model.commitDisplayedMark()
        #expect(world.client.actions.isEmpty)
        #expect(world.fixture.model.showsMarkRegenerate)
        let repaired = setupState(root: world.root, proposalID: "repaired-proposal")
        world.client.setPerforms([repaired])
        await world.fixture.model.regenerateDisplayedMark()
        #expect(world.fixture.model.canCommitDisplayedMark)
        let locked = setupState(root: world.root, markState: "locked", proposalID: "repaired-proposal")
        world.client.setPerforms([locked])
        await world.fixture.model.commitDisplayedMark()
        #expect(world.client.actions == [.regenerateMark, .commitMark(proposalID: "repaired-proposal")])
        #expect(world.fixture.model.setupState?.mark.state == .locked)
    }

    @Test func unqualifiedLaunchStillOpensInit() async throws {
        let world = try makeSetupWorld(capability: false, configureRoot: true)
        defer { world.refresh.release() }
        try writeStep1(world.fixture.mailbox)

        await world.fixture.model.reconcileActivation()

        #expect(world.fixture.browser.opened == [JournalFirstRunModel.defaultInitPageURL])
        #expect(world.fixture.model.reopenCount == 0)
        #expect(world.fixture.openJournalCalls == 0)
        #expect(world.fixture.backupOpens == 0)
        #expect(world.loads.calls == 0)
        #expect(world.peer.resolveCount == 0)
        #expect(world.peer.launched.isEmpty)
        #expect(world.fixture.continuation.loadContinuation() == .missing)
    }

    @Test func qualifiedFreshLaunchShowsWelcomeThenFolderWithoutBrowser() async throws {
        let world = try makeSetupWorld()
        defer { world.refresh.release() }

        await world.fixture.model.reconcileActivation()
        #expect(world.fixture.model.route == .ritual(.welcome))
        world.fixture.model.continueFromWelcome()

        #expect(world.fixture.model.route == .ritual(.folder))
        #expect(world.fixture.config.journalRoot == nil)
        #expect(world.peer.launched.isEmpty)
        expectQuietLaunch(world)
    }

    @Test func loginFailureStaysOnScreenAndIsNotStored() async throws {
        let suite = "app.solstone.journal.first-run.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defaults.removePersistentDomain(forName: suite)
        defaults.set(false, forKey: "launchAtLoginEnabled")
        let config = JournalAppConfig(defaults: defaults, loginItemManager: ThrowingLoginItems())
        let world = try makeSetupWorld(config: config)
        defer { world.refresh.release() }

        await world.fixture.model.reconcileActivation()
        world.fixture.model.setWelcomeLaunchAtLogin(true)

        #expect(world.fixture.model.route == .ritual(.welcome))
        #expect(world.fixture.model.loginFailure?.isEmpty == false)
        #expect(defaults.bool(forKey: "launchAtLoginEnabled") == false)
        #expect(!world.fixture.model.launchAtLoginEnabled)
        expectQuietLaunch(world)
    }

    @Test func folderContinueIsTheOnlyRootWriteAndSkipsModelInstall() async throws {
        let world = try makeSetupWorld()
        defer { world.refresh.release() }
        await world.fixture.model.reconcileActivation()
        world.fixture.model.continueFromWelcome()
        #expect(world.fixture.config.journalRoot == nil)
        world.fixture.model.journalRoot = world.root

        await world.fixture.model.continueFromFolder()

        #expect(world.fixture.config.journalRoot?.standardizedFileURL.path == world.root.standardizedFileURL.path)
        #expect(world.fixture.setupRunner.skipModelInstallFlags == [true])
        #expect(world.fixture.setupRunner.calls == 1)
        #expect(world.peer.launched.isEmpty)
        expectQuietLaunch(world)
    }

    @Test func malformedHandoffStoresNoRoot() async throws {
        let world = try makeSetupWorld()
        defer { world.refresh.release() }
        try Data("not-json".utf8).write(
            to: world.fixture.mailbox.directory.appendingPathComponent("journal-setup-handoff.json")
        )

        await world.fixture.model.reconcileActivation()

        #expect(world.fixture.model.route == .ritual(.welcome))
        #expect(world.fixture.config.journalRoot == nil)
        #expect(world.fixture.continuation.loadContinuation() == .missing)
        expectQuietLaunch(world)
    }

    @Test func existingRootIgnoresHandoffRootHint() async throws {
        let world = try makeSetupWorld(configureRoot: true)
        defer { world.refresh.release() }
        let hinted = world.root.deletingLastPathComponent().appendingPathComponent("hinted", isDirectory: true)
        try writeStep1(world.fixture.mailbox, rootHint: hinted)

        await world.fixture.model.reconcileActivation()

        #expect(world.fixture.config.journalRoot?.standardizedFileURL.path == world.root.standardizedFileURL.path)
        #expect(world.fixture.model.journalRoot.standardizedFileURL.path == world.root.standardizedFileURL.path)
        #expect(world.fixture.continuation.ready(matching: world.root) != nil)
        #expect(world.fixture.continuation.ready(matching: hinted) == nil)
        #expect(world.peer.launched.isEmpty)
        expectQuietLaunch(world)
    }

    @Test func completedOwnerWithoutIntentOrStep1LandsHome() async throws {
        let world = try makeSetupWorld(configureRoot: true, done: true, probes: [.complete])
        defer { world.refresh.release() }

        await world.fixture.model.reconcileActivation()

        #expect(world.fixture.model.route == .home)
        #expect(world.fixture.continuation.loadContinuation() == .missing)
        #expect(world.peer.launched.isEmpty)
        expectQuietLaunch(world)
    }

    @Test func validStep1OnCompletedJournalLaunchesVerifiedPeer() async throws {
        let world = try makeSetupWorld(configureRoot: true, done: true, probes: [.complete])
        defer { world.refresh.release() }
        try writeStep1(world.fixture.mailbox)

        await world.fixture.model.reconcileActivation()

        guard case .ready(let handoff) = world.fixture.mailbox.loadStep5() else {
            Issue.record("expected step 5")
            return
        }
        #expect(handoff.stepIndex == 5)
        #expect(handoff.journalRootPath == world.root.standardizedFileURL.path)
        #expect(handoff.expectedInstanceID == setupInstanceID)
        #expect(handoff.expectedCAFingerprint == setupFingerprint)
        #expect(world.peer.launched == [setupPeerURL])
        #expect(world.peer.resolveCount == 3)
        #expect(world.fixture.model.route != .ritual(.mark))
        expectQuietLaunch(world)
    }

    @Test func replacedStep1LeavesTheNewerHandoff() async throws {
        let world = try makeSetupWorld(configureRoot: true)
        defer { world.refresh.release() }
        let first = UUID()
        let second = UUID()
        try writeStep1(world.fixture.mailbox, id: first)
        world.fixture.model.onBeforeStep1Commit = {
            _ = try? writeStep1(world.fixture.mailbox, id: second)
        }

        await world.fixture.model.reconcileActivation()

        guard case .ready(let handoff) = world.fixture.mailbox.loadStep1() else {
            Issue.record("expected the replacement handoff")
            return
        }
        #expect(handoff.handoffID == second)
        #expect(try world.fixture.mailbox.readStep1Receipt() == nil)
        #expect(world.fixture.model.handoffError == nil)
        #expect(world.fixture.model.route == .ritual(.folder))
        #expect(world.peer.launched.isEmpty)
        #expect(world.client.actions.isEmpty)
        expectQuietLaunch(world)
    }

    @Test func acceptedReplayRepublishesOnlyAndNewHandoffGetsAFreshContinuation() async throws {
        let world = try makeSetupWorld(configureRoot: true, probes: [.complete])
        defer { world.refresh.release() }
        let first = UUID()
        try seedContinuation(world.fixture.continuation, root: world.root, step1: first)
        try removeContinuationFile(world.fixture.continuation)
        try writeStep1(world.fixture.mailbox, id: first)

        await world.fixture.model.reconcileActivation()

        #expect(world.fixture.continuation.loadContinuation() == .missing)
        #expect(world.fixture.continuation.acceptance(of: first) == .accepted)
        #expect(world.peer.launched.isEmpty)
        #expect(world.fixture.mailbox.loadStep5() == .missing)
        #expect(try world.fixture.mailbox.readStep1Receipt()?.handoffID == first)
        #expect(world.fixture.config.journalRoot?.standardizedFileURL.path == world.root.standardizedFileURL.path)

        let second = UUID()
        let oldPending = UUID()
        try writeAcceptedReceipt(world.fixture.mailbox, id: oldPending, step: 5)
        try writeStep1(world.fixture.mailbox, id: second)
        await world.fixture.model.reconcileActivation()

        guard case .ready(let record) = world.fixture.continuation.loadContinuation() else {
            Issue.record("expected a fresh continuation")
            return
        }
        #expect(record.step1HandoffID == second)
        #expect(record.pendingStep5HandoffID != oldPending)
        #expect(world.fixture.continuation.acceptance(of: first) == .accepted)
        #expect(world.peer.launched == [setupPeerURL])
        expectQuietLaunch(world)
    }

    @Test func onlyAcceptedAndRepublishedAdvance() async throws {
        let fresh = try makeSetupWorld()
        defer { fresh.refresh.release() }
        let freshID = try writeStep1(fresh.fixture.mailbox)
        await fresh.fixture.model.reconcileActivation()
        #expect(fresh.fixture.model.route == .ritual(.welcome))
        #expect(fresh.fixture.config.journalRoot == nil)
        guard case .ready(let rootless) = fresh.fixture.continuation.loadContinuation() else {
            Issue.record("expected a rootless continuation")
            return
        }
        #expect(rootless.journalRootPath == nil)
        #expect(rootless.step1HandoffID == freshID)
        #expect(fresh.peer.launched.isEmpty)
        expectQuietLaunch(fresh)

        let republished = try makeSetupWorld(configureRoot: true)
        defer { republished.refresh.release() }
        let id = UUID()
        let pending = UUID()
        try seedContinuation(republished.fixture.continuation, root: republished.root, pending: pending, step1: id)
        try writeStep1(republished.fixture.mailbox, id: id)
        try writeAcceptedReceipt(republished.fixture.mailbox, id: id, step: 1)
        let before = continuationBytes(republished.fixture.continuation)
        await republished.fixture.model.reconcileActivation()
        #expect(continuationBytes(republished.fixture.continuation) == before)
        #expect(republished.fixture.model.route == .ritual(.mark))
        #expect(republished.fixture.model.handoffError == nil)
        #expect(republished.peer.launched.isEmpty)
        expectQuietLaunch(republished)
    }

    @Test func intentIsSavedBeforeFinalizeAndFailedWriteSkipsPost() async throws {
        let world = try makeSetupWorld(
            configureRoot: true,
            markState: "locked",
            saved: true,
            choice: "local",
            probes: [.incomplete, .complete]
        )
        defer { world.refresh.release() }
        let seen = Flag()
        let store = world.fixture.continuation
        let root = world.root
        try seedContinuation(store, root: root)
        world.client.setHook { action in
            if case .finalize = action {
                seen.set(store.ready(matching: root)?.pendingStep5HandoffID != nil)
            }
        }
        await world.fixture.model.reconcileActivation()
        let done = setupState(root: world.root, done: true, markState: "locked", saved: true, choice: "local")
        world.client.setGets([done])
        world.client.setPerforms([done])

        await world.fixture.model.finalizeThinking()

        #expect(seen.value)
        #expect(world.client.actions == [.finalize(timezone: TimeZone.autoupdatingCurrent.identifier)])
        #expect(world.peer.launched == [setupPeerURL])
        guard case .ready = world.fixture.mailbox.loadStep5() else {
            Issue.record("expected step 5 after finalize")
            return
        }
        expectQuietLaunch(world)

        let gate = FailWrites()
        let failing = try makeSetupWorld(
            configureRoot: true,
            markState: "locked",
            saved: true,
            choice: "local",
            continuationReplace: { data, url in
                if gate.fail { throw CocoaError(.fileWriteUnknown) }
                try data.write(to: url, options: .atomic)
            }
        )
        defer { failing.refresh.release() }
        try seedContinuation(failing.fixture.continuation, root: failing.root)
        await failing.fixture.model.reconcileActivation()
        #expect(failing.fixture.model.canFinalizeThinking)
        gate.fail = true
        await failing.fixture.model.finalizeThinking()
        #expect(failing.client.actions.isEmpty)
        #expect(failing.peer.launched.isEmpty)
        #expect(failing.fixture.model.thinkingFailure?.isEmpty == false)
        expectQuietLaunch(failing)
    }

    @Test func pendingStep5IdentifierIsReused() async throws {
        let world = try makeSetupWorld(configureRoot: true, done: true, probes: [.complete])
        defer { world.refresh.release() }
        let pending = UUID()
        try seedContinuation(world.fixture.continuation, root: world.root, pending: pending)

        await world.fixture.model.reconcileActivation()
        #expect(world.peer.launched.isEmpty)
        #expect(world.fixture.model.handoffError != nil)
        await world.fixture.model.retryHandoff()

        guard case .ready(let handoff) = world.fixture.mailbox.loadStep5() else {
            Issue.record("expected step 5")
            return
        }
        #expect(handoff.handoffID == pending)
        #expect(world.peer.launched == [setupPeerURL])
        expectQuietLaunch(world)
    }

    @Test func matchingReceiptRemovesIntentAndReopenDoesNotLaunch() async throws {
        let world = try makeSetupWorld(configureRoot: true, done: true, supervisorLive: true, probes: [.complete])
        defer { world.refresh.release() }
        let pending = UUID()
        let step1 = try seedContinuation(world.fixture.continuation, root: world.root, pending: pending)
        try writeAcceptedReceipt(world.fixture.mailbox, id: pending, step: 5)

        await world.fixture.model.reconcileActivation()
        #expect(world.fixture.continuation.loadContinuation() == .missing)
        #expect(world.fixture.continuation.acceptance(of: step1) == .accepted)
        #expect(world.fixture.model.route == .home)
        #expect(world.peer.launched.isEmpty)
        #expect(await supervisorCount(world.trace) == 0)

        await world.fixture.model.reconcileActivation()
        #expect(world.peer.launched.isEmpty)
        #expect(world.fixture.model.route == .home)
        expectQuietLaunch(world)
    }

    @Test func retryBeforeReceiptDoesNotShowMarkOrRestartLiveSupervisor() async throws {
        let world = try makeSetupWorld(configureRoot: true, done: true, startResults: [false], probes: [.complete])
        defer { world.refresh.release() }
        let pending = UUID()
        try seedContinuation(world.fixture.continuation, root: world.root, pending: pending)

        await world.fixture.model.reconcileActivation()
        #expect(world.fixture.model.route != .ritual(.mark))
        #expect(world.fixture.model.handoffError == nil)
        #expect(world.fixture.model.errorMessage?.isEmpty == false)
        #expect(world.client.actions.isEmpty)
        #expect(await supervisorCount(world.trace) == 1)

        world.live.value = true
        await world.fixture.model.retryHandoff()

        #expect(world.fixture.model.route != .ritual(.mark))
        #expect(world.peer.launched == [setupPeerURL])
        guard case .ready(let handoff) = world.fixture.mailbox.loadStep5() else {
            Issue.record("expected the stored pending id")
            return
        }
        #expect(handoff.handoffID == pending)
        #expect(await supervisorCount(world.trace) == 1)
        #expect(!world.client.actions.contains { action in
            if case .regenerateMark = action { return true }
            if case .commitMark = action { return true }
            return false
        })
        expectQuietLaunch(world)
    }

    @Test func lostFinalizeDoesNotDeliverWithoutPendingOrCommitment() async throws {
        let missingPending = try makeSetupWorld(configureRoot: true, probes: [.complete])
        defer { missingPending.refresh.release() }
        try seedContinuation(missingPending.fixture.continuation, root: missingPending.root)
        await missingPending.fixture.model.reconcileActivation()
        guard case .ready(let minted) = missingPending.fixture.mailbox.loadStep5() else {
            Issue.record("a committed continuation mints and writes step 5")
            return
        }
        #expect(missingPending.peer.launched.count == 1)
        guard case .ready(let record) = missingPending.fixture.continuation.loadContinuation() else {
            Issue.record("expected the continuation to keep the minted id")
            return
        }
        #expect(record.pendingStep5HandoffID == minted.handoffID)

        let uncommitted = try makeSetupWorld(configureRoot: true, probes: [.complete])
        defer { uncommitted.refresh.release() }
        try seedContinuation(uncommitted.fixture.continuation, root: uncommitted.root, pending: UUID())
        uncommitted.identity.resolution = .uncommitted(instanceID: setupInstanceID)
        await uncommitted.fixture.model.reconcileActivation()
        await uncommitted.fixture.model.retryHandoff()
        #expect(uncommitted.fixture.mailbox.loadStep5() == .missing)
        #expect(uncommitted.peer.launched.isEmpty)

        let absent = try makeSetupWorld(configureRoot: true, probes: [.complete])
        defer { absent.refresh.release() }
        try seedContinuation(absent.fixture.continuation, root: absent.root, pending: UUID())
        absent.identity.resolution = .absent
        await absent.fixture.model.reconcileActivation()
        await absent.fixture.model.retryHandoff()
        #expect(absent.fixture.mailbox.loadStep5() == .missing)

        let failed = try makeSetupWorld(configureRoot: true, probes: [.complete])
        defer { failed.refresh.release() }
        try seedContinuation(failed.fixture.continuation, root: failed.root, pending: UUID())
        failed.identity.resolution = .failed(.timeout)
        await failed.fixture.model.reconcileActivation()
        await failed.fixture.model.retryHandoff()
        #expect(failed.fixture.mailbox.loadStep5() == .missing)

        let badFingerprint = try makeSetupWorld(configureRoot: true, probes: [.complete])
        defer { badFingerprint.refresh.release() }
        try seedContinuation(badFingerprint.fixture.continuation, root: badFingerprint.root, pending: UUID())
        badFingerprint.client.setNetwork(JournalNetworkStatus(caFingerprint: "zz", instanceID: setupInstanceID))
        await badFingerprint.fixture.model.reconcileActivation()
        await badFingerprint.fixture.model.retryHandoff()
        #expect(badFingerprint.fixture.mailbox.loadStep5() == .missing)
        #expect(badFingerprint.peer.launched.isEmpty)
    }

    @Test func freshModelMintsThePendingLeftByARefusedPeer() async throws {
        let peer = ScriptedPeer()
        peer.queue = [setupPeerURL]
        peer.verifyQueue = [true, false]
        let world = try makeSetupWorld(configureRoot: true, probes: [.complete], peer: peer)
        defer { world.refresh.release() }
        let step1 = try writeStep1(world.fixture.mailbox)

        await world.fixture.model.reconcileActivation()

        #expect(try world.fixture.mailbox.readStep1Receipt()?.handoffID == step1)
        #expect(world.fixture.mailbox.loadStep1() == .missing)
        guard case .ready(let stored) = world.fixture.continuation.loadContinuation() else {
            Issue.record("expected the accepted continuation")
            return
        }
        #expect(stored.step1HandoffID == step1)
        #expect(stored.pendingStep5HandoffID == nil)
        #expect(world.fixture.mailbox.loadStep5() == .missing)
        #expect(world.peer.launched.isEmpty)

        let recoveryPeer = ScriptedPeer()
        recoveryPeer.queue = [setupPeerURL]
        let client = ScriptedSetupClient()
        client.setNetwork(JournalNetworkStatus(caFingerprint: setupFingerprint, instanceID: setupInstanceID))
        let identity = ScriptedIdentity()
        identity.resolution = .mark(.uiTestSample, instanceID: setupInstanceID)
        let copy = try loadSetupFixtureCopy()
        let recoveryRefresh = RefreshGate()
        defer { recoveryRefresh.release() }
        let recovery = makeModel(
            config: world.fixture.config,
            startResults: [],
            probeResults: [.complete],
            localSetupCapability: { true },
            loadSetupCopy: { copy },
            peer: recoveryPeer,
            setupClient: client,
            identity: identity,
            mailbox: world.fixture.mailbox,
            continuation: world.fixture.continuation,
            supervisorIsLive: { _ in true },
            refreshSleep: { await recoveryRefresh.sleep() }
        )
        await recovery.model.reconcileActivation()

        guard case .ready(let minted) = world.fixture.continuation.loadContinuation() else {
            Issue.record("expected the recovered continuation")
            return
        }
        let pending = try #require(minted.pendingStep5HandoffID)
        guard case .ready(let handoff) = world.fixture.mailbox.loadStep5() else {
            Issue.record("expected one step 5")
            return
        }
        #expect(handoff.handoffID == pending)
        #expect(recoveryPeer.launched.count == 1)

        await recovery.model.reconcileActivation()
        guard case .ready(let retried) = world.fixture.mailbox.loadStep5() else {
            Issue.record("expected the same step 5")
            return
        }
        #expect(retried.handoffID == pending)
        guard case .ready(let reused) = world.fixture.continuation.loadContinuation() else {
            Issue.record("retry keeps the continuation")
            return
        }
        #expect(reused.pendingStep5HandoffID == pending)

        try writeAcceptedReceipt(world.fixture.mailbox, id: pending, step: 5)
        await recovery.model.reconcileActivation()
        #expect(world.fixture.continuation.loadContinuation() == .missing)
        #expect(world.fixture.continuation.acceptance(of: step1) == .accepted)
        let launched = recoveryPeer.launched.count

        try writeStep1(world.fixture.mailbox, id: step1)
        await recovery.model.reconcileActivation()
        #expect(world.fixture.continuation.loadContinuation() == .missing)
        #expect(try world.fixture.mailbox.readStep1Receipt()?.handoffID == step1)
        #expect(recoveryPeer.launched.count == launched)
        #expect(recovery.browser.opened.isEmpty)
        await recovery.model.prepareForTermination()
        expectQuietLaunch(world)
    }

    @Test func savedIntentWithIncompleteSetupDoesNotLaunch() async throws {
        let world = try makeSetupWorld(configureRoot: true, done: false)
        defer { world.refresh.release() }
        try seedContinuation(world.fixture.continuation, root: world.root, pending: UUID())

        await world.fixture.model.reconcileActivation()

        #expect(world.fixture.model.route == .ritual(.mark))
        #expect(world.peer.launched.isEmpty)
        #expect(world.fixture.mailbox.loadStep5() == .missing)
        expectQuietLaunch(world)
    }

    @Test func peerURLReplacedBeforeLaunchDoesNotLaunch() async throws {
        let peer = ScriptedPeer()
        peer.repeatLast = false
        peer.queue = [
            URL(fileURLWithPath: "/Applications/Qualification.app"),
            URL(fileURLWithPath: "/Applications/Selection.app"),
            URL(fileURLWithPath: "/Applications/Launch.app"),
        ]
        let world = try makeSetupWorld(configureRoot: true, done: true, probes: [.complete], peer: peer)
        defer { world.refresh.release() }
        try writeStep1(world.fixture.mailbox)

        await world.fixture.model.reconcileActivation()

        #expect(world.peer.launched.isEmpty)
        #expect(world.peer.resolveCount == 3)
        #expect(world.fixture.continuation.ready(matching: world.root) != nil)
        #expect(world.fixture.model.handoffError?.isEmpty == false)
        #expect(world.fixture.browser.opened.isEmpty)
        expectQuietLaunch(world)
    }

    @Test func absentUnverifiedAndIncapablePeersUseTheLegacyRoute() async throws {
        let incapable = try makeSetupWorld(capability: false, configureRoot: true)
        defer { incapable.refresh.release() }
        try writeStep1(incapable.fixture.mailbox)
        await incapable.fixture.model.reconcileActivation()
        #expect(incapable.fixture.browser.opened == [JournalFirstRunModel.defaultInitPageURL])
        #expect(incapable.loads.calls == 0)
        #expect(incapable.peer.resolveCount == 0)

        let missingCopy = try makeSetupWorld(configureRoot: true, provideCopy: false)
        defer { missingCopy.refresh.release() }
        await missingCopy.fixture.model.reconcileActivation()
        #expect(missingCopy.loads.calls == 1)
        #expect(missingCopy.peer.resolveCount == 0)
        #expect(missingCopy.fixture.browser.opened == [JournalFirstRunModel.defaultInitPageURL])

        let absent = ScriptedPeer()
        absent.repeatLast = false
        absent.queue = [nil]
        let missingPeer = try makeSetupWorld(configureRoot: true, peer: absent)
        defer { missingPeer.refresh.release() }
        await missingPeer.fixture.model.reconcileActivation()
        #expect(missingPeer.peer.resolveCount == 1)
        #expect(missingPeer.peer.verifyCount == 0)
        #expect(missingPeer.fixture.browser.opened == [JournalFirstRunModel.defaultInitPageURL])

        let unverified = ScriptedPeer()
        unverified.queue = [setupPeerURL]
        unverified.verifySticky = false
        let rejectedPeer = try makeSetupWorld(configureRoot: true, peer: unverified)
        defer { rejectedPeer.refresh.release() }
        await rejectedPeer.fixture.model.reconcileActivation()
        #expect(rejectedPeer.peer.verifyCount == 1)
        #expect(rejectedPeer.fixture.browser.opened == [JournalFirstRunModel.defaultInitPageURL])
        #expect(rejectedPeer.peer.launched.isEmpty)
        #expect(rejectedPeer.fixture.continuation.loadContinuation() == .missing)
        #expect(incapable.fixture.openJournalCalls == 0)
        #expect(incapable.fixture.backupOpens == 0)
        #expect(incapable.fixture.model.reopenCount == 0)
        #expect(missingCopy.fixture.openJournalCalls == 0)
        #expect(missingPeer.fixture.openJournalCalls == 0)
        #expect(rejectedPeer.fixture.openJournalCalls == 0)
    }

    @Test func lateResponseDoesNotChangeRootStepOrProposal() async throws {
        let world = try makeSetupWorld(configureRoot: true)
        defer { world.refresh.release() }
        try seedContinuation(world.fixture.continuation, root: world.root)
        await world.fixture.model.reconcileActivation()
        let gate = PerformGate()
        world.client.gate = gate
        world.client.setHold(true)
        let task = Task { await world.fixture.model.commitDisplayedMark() }
        await gate.waitUntilEntered()
        let root = world.fixture.model.journalRoot
        let route = world.fixture.model.route
        let proposal = world.fixture.model.displayedProposalID
        world.fixture.model.advanceEpoch()
        world.client.setPerforms([
            setupState(root: world.root.deletingLastPathComponent(), proposalID: "proposal-2")
        ])
        gate.release()
        await task.value

        #expect(world.fixture.model.journalRoot == root)
        #expect(world.fixture.model.route == route)
        #expect(world.fixture.model.displayedProposalID == proposal)
        #expect(world.client.actions == [.commitMark(proposalID: "proposal-1")])
        expectQuietLaunch(world)
    }

    @Test func proposalConflictWaitsForStateAndDoesNotCommitTheRefresh() async throws {
        let world = try makeSetupWorld(configureRoot: true)
        defer { world.refresh.release() }
        try seedContinuation(world.fixture.continuation, root: world.root)
        await world.fixture.model.reconcileActivation()
        world.client.setGets([setupState(root: world.root, proposalID: "proposal-2")])
        world.client.setError(.server(.init(
            error: "conflict",
            reasonCode: "proposal_conflict",
            detail: "stale"
        )))

        await world.fixture.model.commitDisplayedMark()

        #expect(world.fixture.model.displayedProposalID == "proposal-2")
        #expect(world.fixture.model.route == .ritual(.mark))
        #expect(world.fixture.model.handoffError == nil)
        #expect(world.fixture.model.markFailure == nil)
        #expect(world.client.actions == [.commitMark(proposalID: "proposal-1")])
        expectQuietLaunch(world)
    }

    @Test func lockedMarkHasNoRegenerateAndProposedMarkValidates() async throws {
        let locked = try makeSetupWorld(configureRoot: true, markState: "locked", saved: false)
        defer { locked.refresh.release() }
        try seedContinuation(locked.fixture.continuation, root: locked.root)
        await locked.fixture.model.reconcileActivation()

        #expect(locked.fixture.model.route == .ritual(.thinking))
        #expect(!locked.fixture.model.showsMarkRegenerate)
        await locked.fixture.model.regenerateDisplayedMark()
        #expect(locked.client.actions.isEmpty)

        let proposed = try makeSetupWorld(configureRoot: true)
        defer { proposed.refresh.release() }
        try seedContinuation(proposed.fixture.continuation, root: proposed.root)
        await proposed.fixture.model.reconcileActivation()
        #expect(proposed.fixture.model.route == .ritual(.mark))
        #expect(proposed.fixture.model.validatedMark != nil)
        #expect(proposed.fixture.model.showsMarkRegenerate)
        expectQuietLaunch(proposed)
    }

    @Test func doubleContinueCannotCommitASecondIdentifier() async throws {
        let world = try makeSetupWorld(configureRoot: true)
        defer { world.refresh.release() }
        try seedContinuation(world.fixture.continuation, root: world.root)
        await world.fixture.model.reconcileActivation()
        let gate = PerformGate()
        world.client.gate = gate
        world.client.setHold(true)
        let task = Task { await world.fixture.model.commitDisplayedMark() }
        await gate.waitUntilEntered()
        let epoch = world.fixture.model.epoch

        await world.fixture.model.commitDisplayedMark()

        #expect(world.fixture.model.epoch == epoch)
        #expect(world.client.actions.count == 1)
        world.client.setHold(false)
        gate.release()
        await task.value
        #expect(world.client.actions == [.commitMark(proposalID: "proposal-1")])
        expectQuietLaunch(world)
    }

    @Test func localIsNotOfferedReadyUnlessCapabilityIsOk() async throws {
        for capability in ["blocked", "unknown"] {
            let world = try makeSetupWorld(
                configureRoot: true,
                markState: "locked",
                saved: false,
                capabilityName: capability
            )
            defer { world.refresh.release() }
            try seedContinuation(world.fixture.continuation, root: world.root)
            await world.fixture.model.reconcileActivation()
            #expect(!world.fixture.model.offersLocalReady)
            await world.fixture.model.saveThinking("local")
            #expect(world.client.actions.isEmpty)
            expectQuietLaunch(world)
        }
    }

    @Test(arguments: ["blocked", "unknown"])
    func thinkingThisComputerCannotRunDefaultsToLaterAndContinueFinishes(capability: String) async throws {
        let world = try makeSetupWorld(configureRoot: true, markState: "locked", saved: false,
                                      capabilityName: capability)
        defer { world.refresh.release() }
        try seedContinuation(world.fixture.continuation, root: world.root)
        await world.fixture.model.reconcileActivation()
        #expect(world.fixture.model.route == .ritual(.thinking))
        #expect(!world.fixture.model.offersLocalThinking)
        #expect(world.fixture.model.displayedThinkingChoice == .later)
        #expect(world.fixture.model.canContinueThinking)
        world.fixture.model.selectThinking("local")
        #expect(world.fixture.model.displayedThinkingChoice == .later)
        #expect(world.client.actions.isEmpty)
        let saved = setupState(root: world.root, markState: "locked", saved: true, choice: "later",
                               capability: capability)
        let done = setupState(root: world.root, done: true, markState: "locked", saved: true, choice: "later",
                              capability: capability)
        world.client.setPerforms([saved, done])
        await world.fixture.model.continueThinking()
        #expect(world.client.actions == [.saveThinking(choice: "later"),
                                        .finalize(timezone: TimeZone.autoupdatingCurrent.identifier)])
        #expect(world.peer.launched.count == 1)
        #expect(world.fixture.stepAsides.count == 1)
    }

    @Test func thinkingThisComputerCanRunStillDefaultsToLocal() async throws {
        let world = try makeSetupWorld(configureRoot: true, markState: "locked", saved: false, capabilityName: "ok")
        defer { world.refresh.release() }
        try seedContinuation(world.fixture.continuation, root: world.root)
        await world.fixture.model.reconcileActivation()
        #expect(world.fixture.model.offersLocalThinking)
        #expect(world.fixture.model.displayedThinkingChoice == .local)
        #expect(world.fixture.model.canContinueThinking)
        let saved = setupState(root: world.root, markState: "locked", saved: true, choice: "local")
        let done = setupState(root: world.root, done: true, markState: "locked", saved: true, choice: "local")
        world.client.setPerforms([saved, done])
        await world.fixture.model.continueThinking()
        #expect(world.client.actions == [.saveThinking(choice: "local"),
                                        .finalize(timezone: TimeZone.autoupdatingCurrent.identifier)])
        #expect(world.peer.launched.count == 1)
    }

    @Test func saveThinkingLocalIsTheLocalSave() async throws {
        let world = try makeSetupWorld(configureRoot: true, markState: "locked", saved: false, capabilityName: "ok")
        defer { world.refresh.release() }
        try seedContinuation(world.fixture.continuation, root: world.root)
        await world.fixture.model.reconcileActivation()
        #expect(world.fixture.model.offersLocalReady)
        world.client.setPerforms([
            setupState(root: world.root, markState: "locked", saved: true, choice: "local")
        ])

        await world.fixture.model.saveThinking("local")

        #expect(world.client.actions == [.saveThinking(choice: "local")])
        #expect(world.peer.launched.isEmpty)
        expectQuietLaunch(world)
    }

    @Test func finalizeSendsTimezoneWithoutWaitingForDownloads() async throws {
        let world = try makeSetupWorld(
            configureRoot: true,
            markState: "locked",
            saved: true,
            choice: "later",
            speech: "idle",
            thinkingDownload: "idle",
            probes: [.incomplete, .incomplete]
        )
        defer { world.refresh.release() }
        await world.fixture.model.reconcileActivation()
        await world.fixture.model.continueFromFolder()
        #expect(world.fixture.model.canFinalizeThinking)
        #expect(world.fixture.model.setupState?.downloads.speech.state == .idle)
        let done = setupState(
            root: world.root,
            done: true,
            markState: "locked",
            saved: true,
            choice: "later",
            speech: "idle",
            thinkingDownload: "idle"
        )
        world.client.setGets([done])
        world.client.setPerforms([done])

        await world.fixture.model.finalizeThinking()

        #expect(world.client.actions == [.finalize(timezone: TimeZone.autoupdatingCurrent.identifier)])
        #expect(world.fixture.setupRunner.skipModelInstallFlags == [true])
        #expect(world.fixture.model.setupState?.downloads.speech.state == .idle)
        #expect(world.fixture.model.setupState?.downloads.thinking.state == .idle)
        guard case .ready(let local) = world.fixture.continuation.loadLocal() else {
            Issue.record("expected a local continuation")
            return
        }
        #expect(local.journalRootPath == world.root.standardizedFileURL.path)
        guard case .ready(let handoff) = world.fixture.mailbox.loadStep5() else {
            Issue.record("expected one step 5")
            return
        }
        #expect(handoff.handoffID == local.pendingStep5HandoffID)
        #expect(world.peer.launched.count == 1)
        #expect(world.fixture.continuation.loadContinuation() == .missing)
        #expect(world.fixture.continuation.loadLedger() == .missing)
        expectQuietLaunch(world)
    }

    @Test func localReplaceRefusalSkipsFinalize() async throws {
        let gate = FailWrites()
        let world = try makeSetupWorld(
            configureRoot: true,
            markState: "locked",
            saved: true,
            choice: "later",
            probes: [.incomplete, .incomplete],
            continuationReplace: { data, url in
                if gate.fail { throw CocoaError(.fileWriteUnknown) }
                try data.write(to: url, options: .atomic)
            }
        )
        defer { world.refresh.release() }
        await world.fixture.model.reconcileActivation()
        await world.fixture.model.continueFromFolder()
        gate.fail = true

        await world.fixture.model.finalizeThinking()

        #expect(world.client.actions.isEmpty)
        #expect(world.fixture.mailbox.loadStep5() == .missing)
        #expect(world.peer.launched.isEmpty)
        #expect(world.fixture.model.thinkingFailure?.isEmpty == false)
        let localURL = world.fixture.continuation.directory
            .appendingPathComponent(JournalSetupContinuationStore.localFileName)
        #expect(!FileManager.default.fileExists(atPath: localURL.path))
        expectQuietLaunch(world)
    }

    @Test func speechAdmissionPendingRefreshesWithoutFailing() async throws {
        let world = try makeSetupWorld(configureRoot: true, speech: "error")
        defer { world.refresh.release() }
        try seedContinuation(world.fixture.continuation, root: world.root)
        await world.fixture.model.reconcileActivation()
        #expect(world.fixture.model.showsSpeechRetry)
        world.client.setGets([setupState(root: world.root, speech: "idle")])
        world.client.setError(.server(.init(
            error: "pending",
            reasonCode: "speech_admission_pending",
            detail: "wait"
        )))

        await world.fixture.model.retrySpeechDownload()

        #expect(world.fixture.model.handoffError == nil)
        #expect(world.fixture.model.markFailure == nil)
        #expect(world.fixture.model.errorMessage == nil)
        #expect(world.fixture.model.operationError == .none)
        #expect(world.fixture.model.setupState?.downloads.speech.state == .idle)
        #expect(world.fixture.setupRunner.calls == 0)
        #expect(world.client.actions == [.retrySpeechDownload])
        expectQuietLaunch(world)
    }

    @Test func refreshDoesNotRetryIdleSpeech() async throws {
        let world = try makeSetupWorld(configureRoot: true, speech: "idle", supervisorLive: true)
        defer { world.refresh.release() }
        try seedContinuation(world.fixture.continuation, root: world.root)
        await world.fixture.model.reconcileActivation()
        #expect(world.fixture.model.isRefreshLoopRunning)
        let before = world.client.stateCalls
        world.refresh.release()
        let deadline = ContinuousClock.now.advanced(by: .seconds(2))
        while world.client.stateCalls == before, ContinuousClock.now < deadline {
            try await Task.sleep(for: .milliseconds(10))
        }

        #expect(world.client.stateCalls > before)
        #expect(world.client.actions.isEmpty)
        #expect(!world.fixture.model.showsSpeechRetry)
        let epoch = world.fixture.model.epoch
        await world.fixture.model.prepareForTermination()
        #expect(world.fixture.model.epoch > epoch)
        #expect(!world.fixture.model.isRefreshLoopRunning)
    }

    @Test func speechRetryIsNotOfferedForIdle() async throws {
        let world = try makeSetupWorld(configureRoot: true, speech: "idle")
        defer { world.refresh.release() }
        try seedContinuation(world.fixture.continuation, root: world.root)
        await world.fixture.model.reconcileActivation()

        #expect(!world.fixture.model.showsSpeechRetry)
        await world.fixture.model.retrySpeechDownload()
        #expect(world.client.actions.isEmpty)
        expectQuietLaunch(world)
    }

    @Test func thinkingCancelCarriesRenderedAttemptAndSpeechHasNoCancel() async throws {
        let world = try makeSetupWorld(
            configureRoot: true,
            thinkingDownload: "downloading",
            thinkingAttempt: "attempt-9"
        )
        defer { world.refresh.release() }
        try seedContinuation(world.fixture.continuation, root: world.root)
        await world.fixture.model.reconcileActivation()
        #expect(world.fixture.model.showsThinkingCancel)

        await world.fixture.model.cancelThinkingDownload()

        #expect(world.client.actions == [.cancelThinkingDownload(attemptID: "attempt-9")])
        expectQuietLaunch(world)
    }

    @Test func windowFreeTerminationCancelsRefreshAndClosingDoesNot() async throws {
        let world = try makeSetupWorld(configureRoot: true, supervisorLive: true, attachWindow: false)
        defer { world.refresh.release() }
        try seedContinuation(world.fixture.continuation, root: world.root)
        await world.fixture.model.reconcileActivation()

        #expect(world.fixture.model.isRefreshLoopRunning)
        let epoch = world.fixture.model.epoch
        await world.fixture.model.prepareForTermination()
        #expect(world.fixture.model.epoch > epoch)
        #expect(!world.fixture.model.isRefreshLoopRunning)
    }

    @Test func liveSupervisorKeepsRootAndLaunchesWithoutRestart() async throws {
        let world = try makeSetupWorld(
            configureRoot: true,
            done: true,
            supervisorLive: true,
            startResults: [false],
            probes: [.complete]
        )
        defer { world.refresh.release() }
        let root = world.fixture.config.journalRoot
        try writeStep1(world.fixture.mailbox)

        await world.fixture.model.reconcileActivation()

        #expect(world.fixture.config.journalRoot?.standardizedFileURL.path == root?.standardizedFileURL.path)
        #expect(world.peer.launched == [setupPeerURL])
        #expect(world.fixture.mailbox.loadStep5() != .missing)
        #expect(await supervisorCount(world.trace) == 0)
        #expect(world.fixture.model.route == .home)
        expectQuietLaunch(world)
    }

    @Test func rootlessContinuationBindsTheSelectedRootBeforeSetup() async throws {
        let world = try makeSetupWorld(probes: [.incomplete])
        defer { world.refresh.release() }
        let id = try writeStep1(world.fixture.mailbox)
        await world.fixture.model.reconcileActivation()
        #expect(world.fixture.model.route == .ritual(.welcome))
        guard case .ready(let before) = world.fixture.continuation.loadContinuation() else {
            Issue.record("expected a rootless continuation")
            return
        }
        #expect(before.journalRootPath == nil)
        #expect(before.step1HandoffID == id)
        world.fixture.model.continueFromWelcome()
        #expect(world.fixture.model.route == .ritual(.folder))

        world.fixture.model.journalRoot = world.root
        await world.fixture.model.continueFromFolder()

        #expect(world.fixture.continuation.ready(matching: world.root)?.step1HandoffID == id)
        #expect(world.fixture.setupRunner.skipModelInstallFlags == [true])
        #expect(world.peer.launched.isEmpty)
        expectQuietLaunch(world)
    }

    @Test func orphanReceiptIsAFreshAccept() async throws {
        let world = try makeSetupWorld(configureRoot: true, probes: [.incomplete])
        defer { world.refresh.release() }
        let id = try writeStep1(world.fixture.mailbox)
        try writeAcceptedReceipt(world.fixture.mailbox, id: id, step: 1)
        #expect(world.fixture.continuation.acceptance(of: id) == .notAccepted)

        await world.fixture.model.reconcileActivation()

        #expect(world.fixture.continuation.acceptance(of: id) == .accepted)
        #expect(world.fixture.mailbox.loadStep1() == .missing)
        #expect(world.fixture.model.route == .ritual(.mark))
        #expect(world.peer.launched.isEmpty)
        expectQuietLaunch(world)
    }

    @Test func progressWriteFailureLeavesThePayloadAndWritesNeitherRecord() async throws {
        let gate = FailWrites()
        let world = try makeSetupWorld(
            configureRoot: true,
            continuationReplace: { data, url in
                if gate.fail { throw CocoaError(.fileWriteUnknown) }
                try data.write(to: url, options: .atomic)
            }
        )
        defer { world.refresh.release() }
        let id = try writeStep1(world.fixture.mailbox)
        gate.fail = true

        await world.fixture.model.reconcileActivation()

        guard case .ready(let handoff) = world.fixture.mailbox.loadStep1() else {
            Issue.record("expected the payload to remain")
            return
        }
        #expect(handoff.handoffID == id)
        #expect(world.fixture.continuation.loadContinuation() == .missing)
        #expect(world.fixture.continuation.loadLedger() == .missing)
        #expect(try world.fixture.mailbox.readStep1Receipt() == nil)
        #expect(world.peer.launched.isEmpty)
        #expect(world.fixture.model.handoffError?.isEmpty == false)
        expectQuietLaunch(world)
    }

    @Test func sameURLFinalVerifyFailureDoesNotWriteOrLaunch() async throws {
        let peer = ScriptedPeer()
        peer.queue = [setupPeerURL]
        peer.verifyQueue = [true, true, false]
        let world = try makeSetupWorld(configureRoot: true, done: true, probes: [.complete], peer: peer)
        defer { world.refresh.release() }
        try writeStep1(world.fixture.mailbox)

        await world.fixture.model.reconcileActivation()

        #expect(world.peer.launched.isEmpty)
        #expect(world.peer.resolveCount == 3)
        #expect(world.fixture.mailbox.loadStep5() == .missing)
        #expect(world.fixture.model.handoffError?.isEmpty == false)
        expectQuietLaunch(world)
    }

    @Test func refreshDuringAnActionDoesNotApplyAfterSuccess() async throws {
        let world = try makeSetupWorld(configureRoot: true, supervisorLive: true, probes: [.incomplete])
        defer { world.refresh.release() }
        try seedContinuation(world.fixture.continuation, root: world.root)
        await world.fixture.model.reconcileActivation()
        #expect(world.fixture.model.displayedProposalID == "proposal-1")
        let stateGate = PerformGate()
        world.client.setStateGate(stateGate)
        world.client.setHoldState(true)
        world.client.setGets([setupState(root: world.root, proposalID: "proposal-3")])
        world.client.setPerforms([setupState(root: world.root, proposalID: "proposal-2")])
        world.refresh.release()
        await stateGate.waitUntilEntered()

        await world.fixture.model.commitDisplayedMark()
        stateGate.release()

        #expect(world.fixture.model.displayedProposalID == "proposal-2")
        expectQuietLaunch(world)
    }

    @Test func refreshStartedDuringAnActionDoesNotApplyUntilTheActionFinishes() async throws {
        let world = try makeSetupWorld(configureRoot: true, supervisorLive: true, probes: [.incomplete])
        defer { world.refresh.release() }
        try seedContinuation(world.fixture.continuation, root: world.root)
        await world.fixture.model.reconcileActivation()
        #expect(world.fixture.model.displayedProposalID == "proposal-1")

        let performGate = PerformGate()
        let stateGate = PerformGate()
        world.client.gate = performGate
        world.client.setHold(true)
        world.client.setStateGate(stateGate)
        world.client.setHoldState(true)
        world.client.setGets([setupState(root: world.root, proposalID: "proposal-3")])
        world.client.setPerforms([setupState(root: world.root, proposalID: "proposal-2")])
        let refresh = world.refresh
        world.client.setHook { _ in
            refresh.release()
        }
        let seen = world.client.stateCalls
        let task = Task { await world.fixture.model.commitDisplayedMark() }
        await performGate.waitUntilEntered()
        await stateGate.waitUntilEntered()
        stateGate.release()
        let deadline = ContinuousClock.now.advanced(by: .seconds(2))
        while world.client.stateCalls == seen, ContinuousClock.now < deadline {
            try await Task.sleep(for: .milliseconds(10))
        }

        #expect(world.client.stateCalls > seen)
        #expect(world.fixture.model.displayedProposalID == "proposal-1")
        performGate.release()
        await task.value
        #expect(world.fixture.model.displayedProposalID == "proposal-2")
        expectQuietLaunch(world)
    }

    @Test func thinkingSaveTransportFailureLeavesTheProposal() async throws {
        let world = try makeSetupWorld(configureRoot: true, probes: [.incomplete])
        defer { world.refresh.release() }
        try seedContinuation(world.fixture.continuation, root: world.root)
        await world.fixture.model.reconcileActivation()
        #expect(world.fixture.model.displayedProposalID == "proposal-1")
        world.client.setError(.transport("unavailable"))

        await world.fixture.model.saveThinking("local")

        #expect(world.fixture.model.thinkingFailure?.isEmpty == false)
        #expect(world.fixture.model.markFailure == nil)
        #expect(world.fixture.model.displayedProposalID == "proposal-1")
        expectQuietLaunch(world)
    }

    @Test func speechRetryTransportFailureIsVisibleWithoutANewSentence() async throws {
        let world = try makeSetupWorld(configureRoot: true, speech: "error")
        defer { world.refresh.release() }
        try seedContinuation(world.fixture.continuation, root: world.root)
        await world.fixture.model.reconcileActivation()
        #expect(world.fixture.model.showsSpeechRetry)
        world.client.setError(.transport("unavailable"))

        await world.fixture.model.retrySpeechDownload()

        let failure = try #require(world.fixture.model.speechFailure)
        #expect(!failure.isEmpty)
        if let shared = world.fixture.model.setupCopy?.downloads.error, !shared.isEmpty {
            #expect(failure == shared)
        }
        #expect(!JournalFirstRunCopy.all.contains(failure))
        #expect(world.fixture.model.markFailure == nil)
        #expect(world.fixture.model.thinkingFailure == nil)
        expectQuietLaunch(world)
    }

    @Test func oldRootPayloadIsDropped() async throws {
        let world = try makeSetupWorld(configureRoot: true, supervisorLive: true, probes: [.incomplete])
        defer { world.refresh.release() }
        try seedContinuation(world.fixture.continuation, root: world.root)
        await world.fixture.model.reconcileActivation()
        let other = world.root.deletingLastPathComponent().appendingPathComponent("other-root", isDirectory: true)
        world.client.setGets([setupState(root: other, proposalID: "proposal-9")])
        let before = world.client.stateCalls
        world.refresh.release()
        let deadline = ContinuousClock.now.advanced(by: .seconds(2))
        while world.client.stateCalls == before, ContinuousClock.now < deadline {
            try await Task.sleep(for: .milliseconds(10))
        }

        #expect(world.client.stateCalls > before)
        #expect(world.fixture.model.displayedProposalID == "proposal-1")
        #expect(world.fixture.model.journalRoot.standardizedFileURL.path == world.root.standardizedFileURL.path)
    }

    @Test func cancellationDuringFinalizeDoesNotWriteOrLaunch() async throws {
        let world = try makeSetupWorld(
            configureRoot: true,
            markState: "locked",
            saved: true,
            choice: "local",
            probes: [.incomplete]
        )
        defer { world.refresh.release() }
        try seedContinuation(world.fixture.continuation, root: world.root)
        await world.fixture.model.reconcileActivation()
        let holder = TaskHolder()
        world.client.setHook { _ in holder.task?.cancel() }
        let task = Task { await world.fixture.model.finalizeThinking() }
        holder.task = task
        await task.value

        #expect(world.fixture.mailbox.loadStep5() == .missing)
        #expect(world.peer.launched.isEmpty)
    }

    @Test func cancellationAfterFinalVerifyDoesNotWriteOrLaunch() async throws {
        let peer = ScriptedPeer()
        peer.queue = [setupPeerURL]
        peer.verifyQueue = [true, true, false]
        let world = try makeSetupWorld(configureRoot: true, probes: [.complete], peer: peer)
        defer { world.refresh.release() }
        let holder = TaskHolder()
        peer.onVerify = { count in
            if count == 3 { holder.task?.cancel() }
        }
        try writeStep1(world.fixture.mailbox)
        let task = Task { await world.fixture.model.reconcileActivation() }
        holder.task = task
        await task.value

        #expect(world.peer.verifyCount == 3)
        #expect(world.fixture.mailbox.loadStep5() == .missing)
        #expect(world.peer.launched.isEmpty)
        #expect(world.fixture.model.handoffError == nil)
        #expect(world.fixture.model.outbound != .failed)
        #expect(world.fixture.model.operationError != .outbound)
    }

    @Test func terminationDuringFinalVerifyDoesNotSetOutboundFailure() async throws {
        let peer = ScriptedPeer()
        peer.queue = [setupPeerURL]
        peer.verifyQueue = [true, true, false]
        let world = try makeSetupWorld(configureRoot: true, probes: [.complete], peer: peer)
        defer { world.refresh.release() }
        peer.onVerify = { count in
            if count == 3 {
                await world.fixture.model.prepareForTermination()
            }
        }
        try writeStep1(world.fixture.mailbox)

        await world.fixture.model.reconcileActivation()

        #expect(world.peer.verifyCount == 3)
        #expect(world.fixture.mailbox.loadStep5() == .missing)
        #expect(world.peer.launched.isEmpty)
        #expect(world.fixture.model.handoffError == nil)
        #expect(world.fixture.model.outbound != .failed)
        #expect(world.fixture.model.operationError != .outbound)
    }

    @Test func refreshBeforeAutomaticRegenerateDoesNotReplaceProposal() async throws {
        let world = try makeSetupWorld(configureRoot: true, supervisorLive: true, probes: [.incomplete])
        defer { world.refresh.release() }
        try seedContinuation(world.fixture.continuation, root: world.root)
        await world.fixture.model.reconcileActivation()
        #expect(world.fixture.model.displayedProposalID == "proposal-1")

        let commitGate = PerformGate()
        let regenerateGate = PerformGate()
        world.client.gate = commitGate
        world.client.setHold(true)
        world.client.setPerforms([
            marklessUnavailableState(root: world.root),
            setupState(root: world.root, proposalID: "proposal-regenerated"),
        ])
        let client = world.client
        client.setHook { action in
            if case .regenerateMark = action {
                client.gate = regenerateGate
            }
        }
        let commit = Task { await world.fixture.model.commitDisplayedMark() }
        await commitGate.waitUntilEntered()
        commitGate.release()
        await regenerateGate.waitUntilEntered()

        let stateGate = PerformGate()
        world.client.setStateGate(stateGate)
        world.client.setHoldState(true)
        world.client.setGets([setupState(root: world.root, proposalID: "proposal-stale")])
        world.refresh.release()
        await stateGate.waitUntilEntered()
        await world.fixture.model.regenerateDisplayedMark()
        #expect(world.client.actions.filter {
            if case .regenerateMark = $0 { return true }
            return false
        }.count == 1)

        regenerateGate.release()
        let deadline = ContinuousClock.now.advanced(by: .seconds(2))
        while world.fixture.model.displayedProposalID != "proposal-regenerated", ContinuousClock.now < deadline {
            try await Task.sleep(for: .milliseconds(10))
        }
        let calls = world.client.stateCalls
        stateGate.release()
        while world.client.stateCalls == calls, ContinuousClock.now < deadline {
            try await Task.sleep(for: .milliseconds(10))
        }

        #expect(world.client.stateCalls > calls)
        #expect(world.fixture.model.displayedProposalID == "proposal-regenerated")
        await commit.value
        expectQuietLaunch(world)
    }

    @Test func overlappingReconcileRunsOneBodyThenOneFollowUp() async throws {
        let world = try makeSetupWorld()
        defer { world.refresh.release() }
        let gate = PerformGate()
        world.loads.armHold(gate)
        let first = Task { await world.fixture.model.reconcileActivation() }
        await gate.waitUntilEntered()
        let second = Task { await world.fixture.model.reconcileActivation() }
        await Task.yield()
        #expect(world.loads.maxConcurrent == 1)
        gate.release()
        let deadline = ContinuousClock.now.advanced(by: .seconds(2))
        while world.loads.calls < 2, ContinuousClock.now < deadline {
            try await Task.sleep(for: .milliseconds(10))
        }
        #expect(world.loads.calls == 2)
        #expect(world.loads.maxConcurrent == 1)
        gate.release()
        await first.value
        await second.value
        #expect(world.loads.calls == 2)
        #expect(world.loads.maxConcurrent == 1)
    }

    @Test func unknownSizeKeepsTheCallerTemplate() async throws {
        var copy = try loadSetupFixtureCopy()
        copy.thinking.defaultLine = "keep {size} end"
        copy.thinking.sizeUnknown = "size-token"
        let world = try makeSetupWorld(copy: copy)
        defer { world.refresh.release() }

        await world.fixture.model.reconcileActivation()

        #expect(world.fixture.model.thinkingSizeLine == "keep size-token end")
    }

    @Test func selectedChoiceDescriptionIsAvailable() async throws {
        let copy = try loadSetupFixtureCopy()
        let world = try makeSetupWorld(
            configureRoot: true,
            markState: "locked",
            saved: true,
            choice: "local",
            copy: copy
        )
        defer { world.refresh.release() }
        try seedContinuation(world.fixture.continuation, root: world.root)
        await world.fixture.model.reconcileActivation()

        let expected = copy.thinking.choices.first { $0.id == "local" }?.description
        #expect(expected?.isEmpty == false)
        #expect(world.fixture.model.selectedThinkingDescription == expected)
    }

    @Test func downloadProgressUsesFileCountsOnlyForActiveTotals() async throws {
        let world = try makeSetupWorld()
        defer { world.refresh.release() }
        func formatted(_ bytes: UInt64) -> String {
            ByteCountFormatter.string(fromByteCount: Int64(bytes), countStyle: .file)
        }
        let downloading = JournalSetupState.Job(
            state: .downloading, attemptID: "attempt", bytesDone: 10, bytesTotal: 40
        )
        #expect(world.fixture.model.downloadProgressText(for: downloading) == "\(formatted(10)) / \(formatted(40))")
        let complete = JournalSetupState.Job(
            state: .complete, attemptID: nil, bytesDone: 40, bytesTotal: 40
        )
        #expect(world.fixture.model.downloadProgressText(for: complete) == "\(formatted(40)) / \(formatted(40))")
        let idle = JournalSetupState.Job(state: .idle, attemptID: nil, bytesDone: 0, bytesTotal: 40)
        #expect(world.fixture.model.downloadProgressText(for: idle) == nil)
        let stale = JournalSetupState.Job(state: .downloading, attemptID: "", bytesDone: 10, bytesTotal: 40)
        #expect(world.fixture.model.downloadProgressText(for: stale) == nil)
        let missingTotal = JournalSetupState.Job(state: .complete, attemptID: nil, bytesDone: 1, bytesTotal: nil)
        #expect(world.fixture.model.downloadProgressText(for: missingTotal) == formatted(1))
        for total in [nil, 0] as [UInt64?] {
            let unknown = JournalSetupState.Job(state: .downloading, attemptID: "active", bytesDone: 10, bytesTotal: total)
            #expect(world.fixture.model.downloadProgressText(for: unknown) == formatted(10))
        }
    }

    @Test func downloadCountsAboveSignedRangeRemainRenderable() throws {
        let world = try makeSetupWorld()
        defer { world.refresh.release() }
        let bounded = ByteCountFormatter.string(fromByteCount: Int64.max, countStyle: .file)
        let known = JournalSetupState.Job(
            state: .downloading, attemptID: "active", bytesDone: UInt64.max, bytesTotal: UInt64.max
        )
        #expect(world.fixture.model.downloadProgressText(for: known) == "\(bounded) / \(bounded)")
        let unknown = JournalSetupState.Job(
            state: .downloading, attemptID: "active", bytesDone: UInt64.max, bytesTotal: nil
        )
        #expect(world.fixture.model.downloadProgressText(for: unknown) == bounded)
    }
}

@MainActor
private struct SetupWorld {
    var fixture: FirstRunModelFixture
    var peer: ScriptedPeer
    var client: ScriptedSetupClient
    var refresh: RefreshGate
    var root: URL
    var loads: CopyLoads
    var live: LiveSwitch
    var trace: FirstRunTrace
    var identity: ScriptedIdentity
}

@MainActor
private func makeSetupWorld(
    capability: Bool = true,
    configureRoot: Bool = false,
    provideCopy: Bool = true,
    done: Bool = false,
    markState: String = "proposed",
    saved: Bool = false,
    choice: String? = nil,
    capabilityName: String = "ok",
    speech: String = "idle",
    thinkingDownload: String = "idle",
    thinkingAttempt: String? = nil,
    proposalID: String = "proposal-1",
    supervisorLive: Bool = false,
    startResults: [Bool] = [],
    probes: [JournalInitSetupProbe] = [.incomplete, .complete],
    attachWindow: Bool = true,
    peer suppliedPeer: ScriptedPeer? = nil,
    config suppliedConfig: JournalAppConfig? = nil,
    copy suppliedCopy: JournalSetupCopy? = nil,
    downloadBytes: UInt64? = nil,
    continuationReplace: (@Sendable (Data, URL) throws -> Void)? = nil,
    continuationRead: (@Sendable (URL) throws -> Data)? = nil,
    continuationRemove: (@Sendable (URL) throws -> Void)? = nil,
    initClient: (any JournalInitClienting)? = nil
) throws -> SetupWorld {
    let root = FileManager.default.temporaryDirectory
        .appendingPathComponent("journal-setup-root-\(UUID().uuidString)", isDirectory: true)
    try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    let config = suppliedConfig ?? makeConfig()
    if configureRoot {
        config.journalRoot = root
    }
    let copy = try suppliedCopy ?? loadSetupFixtureCopy()
    let loads = CopyLoads()
    let peer = suppliedPeer ?? {
        let created = ScriptedPeer()
        created.queue = [setupPeerURL]
        return created
    }()
    let client = ScriptedSetupClient()
    client.setNetwork(JournalNetworkStatus(caFingerprint: setupFingerprint, instanceID: setupInstanceID))
    client.setGets([
        setupState(
            root: root,
            done: done,
            markState: markState,
            proposalID: proposalID,
            saved: saved,
            choice: choice,
            capability: capabilityName,
            speech: speech,
            thinkingDownload: thinkingDownload,
            thinkingAttempt: thinkingAttempt,
            downloadBytes: downloadBytes
        )
    ])
    let identity = ScriptedIdentity()
    identity.resolution = .mark(.uiTestSample, instanceID: setupInstanceID)
    let refresh = RefreshGate()
    let live = LiveSwitch()
    live.value = supervisorLive
    let trace = FirstRunTrace()
    var continuation: JournalSetupContinuationStore?
    if continuationReplace != nil || continuationRead != nil || continuationRemove != nil {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("journal-setup-continuation-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        continuation = JournalSetupContinuationStore(directory: directory,
            replace: continuationReplace ?? journalSetupContinuationReplace,
            read: continuationRead ?? { try Data(contentsOf: $0) },
            remove: continuationRemove ?? { try FileManager.default.removeItem(at: $0) })
    }
    let fixture = makeModel(
        trace: trace,
        config: config,
        startResults: startResults,
        probeResults: probes,
        initClient: initClient,
        localSetupCapability: { capability },
        loadSetupCopy: {
            loads.enter()
            if let gate = loads.holdGate() {
                await gate.enterAndWait()
            }
            loads.leave()
            return provideCopy ? copy : nil
        },
        peer: peer,
        setupClient: client,
        identity: identity,
        continuation: continuation,
        supervisorIsLive: { _ in live.value },
        externalSupervisorIsLive: { _ in false },
        refreshSleep: { await refresh.sleep() },
        attachWindow: attachWindow
    )
    return SetupWorld(
        fixture: fixture,
        peer: peer,
        client: client,
        refresh: refresh,
        root: root,
        loads: loads,
        live: live,
        trace: trace,
        identity: identity
    )
}

private func marklessUnavailableState(root: URL) -> JournalSetupState {
    JournalSetupState(
        journalPath: root.standardizedFileURL.path,
        done: false,
        mark: .init(render: nil, state: .unavailable, proposalID: nil),
        thinking: .init(choice: nil, saved: false, capability: .unknown, downloadBytes: nil),
        downloads: .init(
            speech: .init(state: .idle, attemptID: nil, bytesDone: 0, bytesTotal: nil),
            thinking: .init(state: .idle, attemptID: nil, bytesDone: 0, bytesTotal: nil)
        )
    )
}

private func setupState(
    root: URL,
    done: Bool = false,
    markState: String = "proposed",
    proposalID: String? = "proposal-1",
    saved: Bool = false,
    choice: String? = nil,
    capability: String = "ok",
    speech: String = "idle",
    thinkingDownload: String = "idle",
    thinkingAttempt: String? = nil,
    downloadBytes: UInt64? = nil,
    speechTotal: UInt64? = nil,
    thinkingTotal: UInt64? = nil
) -> JournalSetupState {
    JournalSetupState(
        journalPath: root.standardizedFileURL.path,
        done: done,
        mark: .init(
            render: .uiTestSample,
            state: JournalMarkWireState(rawValue: markState) ?? .proposed,
            proposalID: proposalID
        ),
        thinking: .init(
            choice: choice.flatMap(JournalThinkingChoice.init(rawValue:)),
            saved: saved,
            capability: JournalThinkingCapability(rawValue: capability) ?? .unknown,
            downloadBytes: downloadBytes
        ),
        downloads: .init(
            speech: .init(
                state: JournalDownloadJobState(rawValue: speech) ?? .idle,
                attemptID: nil,
                bytesDone: 0,
                bytesTotal: speechTotal
            ),
            thinking: .init(
                state: JournalDownloadJobState(rawValue: thinkingDownload) ?? .idle,
                attemptID: thinkingAttempt,
                bytesDone: 0,
                bytesTotal: thinkingTotal
            )
        )
    )
}

func loadSetupFixtureCopy() throws -> JournalSetupCopy {
    let url = URL(filePath: #filePath)
        .deletingLastPathComponent()
        .deletingLastPathComponent()
        .appendingPathComponent("JournalRuntimeTests/Fixtures/setup-copy.json")
    return try JournalSetupCopy.decodeQualified(Data(contentsOf: url))
}

@discardableResult
private func writeStep1(_ mailbox: SetupMailboxStore, id: UUID = UUID(), rootHint: URL? = nil) throws -> UUID {
    if let rootHint {
        let handoff = SetupHandoff(handoffID: id, stepIndex: 1, journalRootPath: rootHint.path)
        let data = try JSONEncoder().encode(handoff)
        try data.write(to: mailbox.directory.appendingPathComponent("journal-setup-handoff.json"))
        return id
    }
    try mailbox.writeStep1(handoffID: id)
    return id
}

private func writeAcceptedReceipt(_ mailbox: SetupMailboxStore, id: UUID, step: Int) throws {
    let data = try JSONEncoder().encode(SetupHandoffReceipt(handoffID: id, stepIndex: step))
    let name = step == 5 ? "solstone-setup-handoff-accepted.json" : "journal-setup-handoff-accepted.json"
    try data.write(to: mailbox.directory.appendingPathComponent(name))
}

private func continuationBytes(_ store: JournalSetupContinuationStore) -> Data? {
    let url = store.directory.appendingPathComponent(JournalSetupContinuationStore.fileName)
    return try? Data(contentsOf: url)
}

@discardableResult
private func seedContinuation(
    _ store: JournalSetupContinuationStore,
    root: URL?,
    pending: UUID? = nil,
    step1: UUID = UUID()
) throws -> UUID {
    try store.storeProgress(step1: step1, journalRootPath: root?.standardizedFileURL.path)
    if let pending {
        let object: [String: Any] = [
            "journalRootPath": root?.standardizedFileURL.path as Any? ?? NSNull(),
            "pendingStep5HandoffID": pending.uuidString.lowercased(),
            "schema": JournalSetupContinuationStore.schema,
            "step1HandoffID": step1.uuidString.lowercased(),
        ]
        let data = try JSONSerialization.data(withJSONObject: object, options: [.sortedKeys])
        try data.write(
            to: store.directory.appendingPathComponent(JournalSetupContinuationStore.fileName),
            options: .atomic
        )
    }
    return step1
}

private func removeContinuationFile(_ store: JournalSetupContinuationStore) throws {
    let url = store.directory.appendingPathComponent(JournalSetupContinuationStore.fileName)
    if FileManager.default.fileExists(atPath: url.path) {
        try FileManager.default.removeItem(at: url)
    }
}

private final class FailWrites: @unchecked Sendable {
    var fail = false
}

private final class TaskHolder: @unchecked Sendable {
    private let lock = NSLock()
    private var stored: Task<Void, Never>?
    var task: Task<Void, Never>? {
        get { lock.withLock { stored } }
        set { lock.withLock { stored = newValue } }
    }
}

private func supervisorCount(_ trace: FirstRunTrace) async -> Int {
    await trace.snapshot().filter { $0 == "supervisor" }.count
}

@MainActor
private func expectQuietLaunch(_ world: SetupWorld) {
    #expect(world.fixture.browser.opened.isEmpty)
    #expect(world.fixture.model.reopenCount == 0)
    #expect(world.fixture.openJournalCalls == 0)
    #expect(world.fixture.backupOpens == 0)
}

private let setupFingerprint = String(repeating: "0123456789abcdef", count: 4)
private let setupInstanceID = "instance-1"
private let setupPeerURL = URL(fileURLWithPath: "/Applications/Solstone.app")

@MainActor
private final class ScriptedPeer: JournalSetupPeerResolving {
    var queue: [URL?] = []
    var sticky: URL?
    var repeatLast = true
    var verifyQueue: [Bool] = []
    var verifySticky = true
    private(set) var launched: [URL] = []
    private(set) var resolveCount = 0
    private(set) var verifyCount = 0
    var onVerify: (@MainActor (Int) async -> Void)?
    var onLaunch: (@MainActor () throws -> Void)?

    func resolve() -> URL? {
        resolveCount += 1
        if !queue.isEmpty {
            sticky = queue.removeFirst()
            return sticky
        }
        return repeatLast ? sticky : nil
    }

    func verify(_ url: URL) async -> Bool {
        _ = url
        verifyCount += 1
        await onVerify?(verifyCount)
        if !verifyQueue.isEmpty {
            return verifyQueue.removeFirst()
        }
        return verifySticky
    }

    func launch(_ url: URL) async throws {
        launched.append(url)
        try onLaunch?()
    }
}

private final class ScriptedSetupClient: JournalSetupClienting, @unchecked Sendable {
    private let lock = NSLock()
    private var gets: [JournalSetupState] = []
    private var performs: [JournalSetupState] = []
    private var recorded: [JournalSetupAction] = []
    private var getsCount = 0
    private var thrown: JournalSetupClientError?
    private var hold = false
    private var holdState = false
    private var network = JournalNetworkStatus(caFingerprint: nil, instanceID: nil)
    private var hook: (@Sendable (JournalSetupAction) -> Void)?
    private var stateGateStorage: PerformGate?
    var gate: PerformGate?

    var actions: [JournalSetupAction] { lock.withLock { recorded } }
    var stateCalls: Int { lock.withLock { getsCount } }

    func setGets(_ values: [JournalSetupState]) { lock.withLock { gets = values } }
    func setPerforms(_ values: [JournalSetupState]) { lock.withLock { performs = values } }
    func setError(_ error: JournalSetupClientError?) { lock.withLock { thrown = error } }
    func setHold(_ value: Bool) { lock.withLock { hold = value } }
    func setHoldState(_ value: Bool) { lock.withLock { holdState = value } }
    func setStateGate(_ gate: PerformGate?) { lock.withLock { stateGateStorage = gate } }
    func setNetwork(_ value: JournalNetworkStatus) { lock.withLock { network = value } }
    func setHook(_ hook: (@Sendable (JournalSetupAction) -> Void)?) { lock.withLock { self.hook = hook } }

    func state() async throws -> JournalSetupState {
        let gate: PerformGate? = lock.withLock { holdState ? stateGateStorage : nil }
        if gate != nil {
            await gate?.enterAndWait()
        }
        let value: JournalSetupState? = lock.withLock {
            getsCount += 1
            guard !gets.isEmpty else { return nil }
            if gets.count == 1 { return gets[0] }
            return gets.removeFirst()
        }
        guard let value else { throw JournalSetupClientError.decoding }
        return value
    }

    func perform(_ action: JournalSetupAction) async throws -> JournalSetupState {
        let callback: (@Sendable (JournalSetupAction) -> Void)? = lock.withLock { hook }
        callback?(action)
        let shouldHold: Bool = lock.withLock {
            recorded.append(action)
            return hold
        }
        if shouldHold {
            await gate?.enterAndWait()
        }
        let error: JournalSetupClientError? = lock.withLock {
            let value = thrown
            thrown = nil
            return value
        }
        if let error { throw error }
        let value: JournalSetupState? = lock.withLock {
            if !performs.isEmpty {
                if performs.count == 1 { return performs[0] }
                return performs.removeFirst()
            }
            return gets.last
        }
        guard let value else { throw JournalSetupClientError.decoding }
        return value
    }

    func networkStatus() async throws -> JournalNetworkStatus {
        lock.withLock { network }
    }
}

private final class ScriptedIdentity: JournalIdentityResolving, @unchecked Sendable {
    private let lock = NSLock()
    private var stored: JournalIdentityResolution = .absent

    var resolution: JournalIdentityResolution {
        get { lock.withLock { stored } }
        set { lock.withLock { stored = newValue } }
    }

    func resolveIdentity(baseURL: String) async -> JournalIdentityResolution {
        _ = baseURL
        return resolution
    }
}

private final class RefreshGate: @unchecked Sendable {
    private let lock = NSLock()
    private var open = false
    private var waiters: [CheckedContinuation<Void, Never>] = []

    func sleep() async {
        await withCheckedContinuation { (cont: CheckedContinuation<Void, Never>) in
            lock.lock()
            if open {
                open = false
                lock.unlock()
                cont.resume()
                return
            }
            waiters.append(cont)
            lock.unlock()
        }
    }

    func release() {
        let waiter: CheckedContinuation<Void, Never>? = lock.withLock {
            if waiters.isEmpty {
                open = true
                return nil
            }
            return waiters.removeFirst()
        }
        waiter?.resume()
    }
}

private final class PerformGate: @unchecked Sendable {
    private let lock = NSLock()
    private var open = false
    private var waiters: [CheckedContinuation<Void, Never>] = []
    private var entered = 0
    private var enteredWaiters: [CheckedContinuation<Void, Never>] = []

    func enterAndWait() async {
        await withCheckedContinuation { (cont: CheckedContinuation<Void, Never>) in
            lock.lock()
            entered += 1
            let notify = enteredWaiters
            enteredWaiters.removeAll()
            if open {
                open = false
                lock.unlock()
                notify.forEach { $0.resume() }
                cont.resume()
                return
            }
            waiters.append(cont)
            lock.unlock()
            notify.forEach { $0.resume() }
        }
    }

    func waitUntilEntered() async {
        await withCheckedContinuation { (cont: CheckedContinuation<Void, Never>) in
            lock.lock()
            if entered > 0 {
                lock.unlock()
                cont.resume()
                return
            }
            enteredWaiters.append(cont)
            lock.unlock()
        }
    }

    func release() {
        let waiter: CheckedContinuation<Void, Never>? = lock.withLock {
            if waiters.isEmpty {
                open = true
                return nil
            }
            return waiters.removeFirst()
        }
        waiter?.resume()
    }
}

private final class CopyLoads: @unchecked Sendable {
    private let lock = NSLock()
    private var count = 0
    private var current = 0
    private var maxSeen = 0
    private var hold = false
    private var gate: PerformGate?
    var calls: Int { lock.withLock { count } }
    var maxConcurrent: Int { lock.withLock { maxSeen } }
    func armHold(_ gate: PerformGate) {
        lock.withLock {
            hold = true
            self.gate = gate
        }
    }
    func holdGate() -> PerformGate? {
        lock.withLock { hold ? gate : nil }
    }
    func enter() {
        lock.withLock {
            current += 1
            count += 1
            maxSeen = max(maxSeen, current)
        }
    }
    func leave() {
        lock.withLock {
            current = max(0, current - 1)
        }
    }
}

private final class LiveSwitch: @unchecked Sendable {
    var value = false
}

private final class Flag: @unchecked Sendable {
    private let lock = NSLock()
    private var stored = false
    var value: Bool { lock.withLock { stored } }
    func set(_ value: Bool) { lock.withLock { stored = value } }
}

@MainActor
private final class ThrowingLoginItems: LoginItemManaging {
    var status: SMAppService.Status = .notRegistered

    func register() throws { throw CocoaError(.fileWriteUnknown) }
    func unregister() throws { throw CocoaError(.fileWriteUnknown) }
    func unregisterAwaitingCompletion() async throws {}
}

private actor HeldCompletionProbe: JournalInitClienting {
    let gate: PerformGate
    private var calls = 0
    init(gate: PerformGate) { self.gate = gate }
    func probeSetupComplete() async throws -> JournalInitSetupProbe {
        calls += 1
        if calls == 1 { return .incomplete }
        if calls == 2 { await gate.enterAndWait() }
        return .complete
    }
}

@MainActor
extension JournalSetupChromeTests {
    @Test(arguments: [JournalFirstRunStep.setupProgress, JournalFirstRunStep.folder])
    func refusalRendersOpenYourJournalAndOmitsStepControls(step: JournalFirstRunStep) async throws {
        let world = try makeSetupWorld(configureRoot: true, done: true, supervisorLive: true, probes: [.complete])
        defer { world.refresh.release() }
        _ = try world.fixture.continuation.storeLocal(root: world.root)
        await world.fixture.model.reconcileActivation()
        let pending = try #require(try world.fixture.continuation.loadActive()?.pending)
        try JSONEncoder().encode(SetupHandoffRefusal(handoffID: pending, reason: .heldPairing))
            .write(to: world.fixture.mailbox.directory.appendingPathComponent("solstone-setup-handoff-refused.json"))
        await world.fixture.model.reconcileForReopen()
        #expect(world.fixture.model.handoffRefusal?.handoffID == pending)

        world.fixture.model.route = .ritual(step)

        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 880, height: 580),
            styleMask: [.titled, .closable, .resizable],
            backing: .buffered,
            defer: false
        )
        let hostingView = NSHostingView(rootView: JournalFirstRunView(model: world.fixture.model))
        window.contentView = hostingView
        hostingView.layoutSubtreeIfNeeded()

        let elements = collectAccessibilityElements(root: hostingView)
        let tree = String(reflecting: hostingView.rootView.body)

        let hasOpenYourJournal = tree.contains(JournalFirstRunCopy.openYourJournal) || elements.contains { elem in
            getAXTitleOrLabel(elem) == JournalFirstRunCopy.openYourJournal
        }
        #expect(hasOpenYourJournal)

        let stepPrimaryID: String = (step == .setupProgress)
            ? AXID.Journal.Ritual.setupRetry
            : AXID.Journal.Ritual.folderContinue

        let hasBack = elements.contains { elem in
            getAXIdentifier(elem) == AXID.Journal.Ritual.back
        }
        #expect(!hasBack)

        let hasStepPrimary = elements.contains { elem in
            getAXIdentifier(elem) == stepPrimaryID
        }
        #expect(!hasStepPrimary)
    }
}

@MainActor
private func getAXChildren(_ obj: AnyObject) -> [AnyObject] {
    if let v = obj as? NSView {
        return (v.accessibilityChildren() ?? []) as [AnyObject]
    }
    if let el = obj as? NSAccessibilityElement {
        return (el.accessibilityChildren() ?? []) as [AnyObject]
    }
    return []
}

@MainActor
private func getAXIdentifier(_ obj: AnyObject) -> String? {
    if let v = obj as? NSView {
        let id = v.accessibilityIdentifier()
        return id.isEmpty ? nil : id
    }
    if let el = obj as? NSAccessibilityElement {
        return el.accessibilityIdentifier()
    }
    return nil
}

@MainActor
private func getAXTitleOrLabel(_ obj: AnyObject) -> String? {
    if let v = obj as? NSView {
        if let t = v.accessibilityTitle(), !t.isEmpty { return t }
        if let l = v.accessibilityLabel(), !l.isEmpty { return l }
        if let b = v as? NSButton, !b.title.isEmpty { return b.title }
    }
    if let el = obj as? NSAccessibilityElement {
        if let t = el.accessibilityTitle(), !t.isEmpty { return t }
        if let l = el.accessibilityLabel(), !l.isEmpty { return l }
    }
    return nil
}

@MainActor
private func collectAccessibilityElements(root: NSView) -> [AnyObject] {
    var visited: [ObjectIdentifier] = []
    var results: [AnyObject] = []

    func walk(_ node: AnyObject) {
        let id = ObjectIdentifier(node)
        if visited.contains(id) { return }
        visited.append(id)
        results.append(node)

        for child in getAXChildren(node) {
            walk(child)
        }
        if let view = node as? NSView {
            for sub in view.subviews {
                walk(sub)
            }
        }
    }

    walk(root)
    return results
}
