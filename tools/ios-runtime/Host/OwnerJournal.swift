// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
#if JOURNAL_OWNER_APP
import SwiftUI
import UIKit

/// App lifecycle only. Journal setup, domain writes and recovery stay in Rust.
@MainActor final class OwnerJournal: ObservableObject {
  enum Phase { case closed, starting, running, stopping, stopped, suspended, unavailable }
  @Published var phase = Phase.closed
  @Published var detail = ""
  @Published var page = UUID()
  @Published var route = "/init"
  @Published var recoveryBusy = false
  @Published var recoveryReview: OwnerCreatorRecovery.Review?
  @Published var showRecoveryReview = false
  @Published var recoveryDetail = ""
  @Published private(set) var recoveryHeld = true
  private var recoveryInvalid = false
  private var recoveryState = OwnerCreatorRecovery()
  private let runner = Runner()
  private var pending: Task<Void, Never>?
  private var revision: UInt64 = 0
  private var retained = false
  private var loaded = false
  @Published private var foreground = true
  private var ownerStopped = true
  private var choiceSaved = true
  private var servicesPrepared = false
  private var choice: URL? {
    guard let fixture = try? runner.ownerFixture.get() else { return nil }
    return runner.output.appendingPathComponent(fixture.choiceFileName)
  }
  private var recoveryFile: URL? {
    choice?.appendingPathExtension("creator-recovery.json")
  }

  private func admitFixture() -> Bool {
    guard case .success = runner.ownerFixture else {
      ownerStopped = true
      choiceSaved = false
      phase = .unavailable
      detail = "the test fixture name isn't valid. close the app and check the launch arguments."
      return false
    }
    return true
  }

  var label: String {
    switch phase {
    case .closed, .starting: return "opening your journal"
    case .running: return "your journal is open"
    case .stopping: return "stopping your journal"
    case .stopped: return "your journal is stopped"
    case .suspended: return "your journal is paused"
    case .unavailable: return "your journal needs attention"
    }
  }
  var canStart: Bool { loaded && choiceSaved && !retained && !recoveryBusy && !recoveryHeld && !recoveryInvalid && phase != .closed && phase != .starting && phase != .stopping }
  var canReviewRecovery: Bool {
    loaded && choiceSaved && ownerStopped && !retained && foreground && !recoveryBusy
      && !recoveryInvalid && (phase == .stopped || phase == .unavailable)
  }
  var hasOwnerPage: Bool { retained && (phase == .running || phase == .unavailable) }

  private struct Choice: Codable { let stopped: Bool }
  private func saveChoice(_ stopped: Bool) throws {
    guard let choice else { throw OwnerFixture.Refusal.invalidArguments }
    let bytes = try JSONEncoder().encode(Choice(stopped: stopped))
    try bytes.write(to: choice, options: [.atomic])
    let file = Darwin.open(choice.path, O_RDONLY | O_CLOEXEC)
    guard file >= 0 else { throw CocoaError(.fileWriteUnknown) }
    defer { close(file) }
    guard fsync(file) == 0 else { throw CocoaError(.fileWriteUnknown) }
    let directory = Darwin.open(choice.deletingLastPathComponent().path, O_RDONLY | O_CLOEXEC)
    guard directory >= 0 else { throw CocoaError(.fileWriteUnknown) }
    defer { close(directory) }
    guard fsync(directory) == 0 else { throw CocoaError(.fileWriteUnknown) }
    ownerStopped = stopped
    choiceSaved = true
  }
  private func enqueue(_ action: @escaping @MainActor () async -> Void) {
    let preceding = pending
    pending = Task { await preceding?.value; await action() }
  }
  func open() {
    guard admitFixture(), let choice else { return }
    guard !loaded else { return }
    loaded = true
    UIDevice.current.isBatteryMonitoringEnabled = true
    do {
      guard let recoveryFile else { throw OwnerCreatorRecovery.Refusal.invalidRecord }
      do { recoveryState = try OwnerCreatorRecovery.load(recoveryFile) }
      catch { recoveryInvalid = true; throw error }
      recoveryHeld = try recoveryState.heldIndex() != nil
      if FileManager.default.fileExists(atPath: choice.path) {
        ownerStopped = try JSONDecoder().decode(Choice.self, from: Data(contentsOf: choice)).stopped
      } else {
        try saveChoice(false)
      }
      if ownerStopped { phase = .stopped }
      else if recoveryHeld {
        ownerStopped = true
        choiceSaved = false
        phase = .unavailable
        detail = "a storage recovery result is unconfirmed. use stop before checking it."
      }
      else { start(explicit: false) }
      if recoveryHeld { recoveryDetail = "an earlier cleanup result is unconfirmed. check current storage before starting." }
    } catch {
      ownerStopped = true
      choiceSaved = false
      phase = .unavailable
      detail = "couldn't read the saved start or recovery state. your journal is still stopped."
    }
  }
  func start(explicit: Bool = true) {
    guard admitFixture() else { return }
    if explicit && !canStart { return }
    guard !retained, !recoveryBusy, !recoveryHeld, !recoveryInvalid,
      phase != .starting, phase != .stopping else { return }
    recoveryReview = nil
    showRecoveryReview = false
    if explicit {
      do { try saveChoice(false) }
      catch { phase = .unavailable; detail = "couldn't save start. free some space, then try again."; return }
    }
    guard !ownerStopped, foreground else { return }
    recoveryDetail = ""
    revision &+= 1
    let ticket = revision
    let admission = journal_ios_processing_generation()
    phase = .starting
    detail = "keep this app open and this device plugged in."
    enqueue { [self] in
      guard ticket == revision, !ownerStopped, foreground else { return }
      do {
        _ = try await runner.servingCall(runner.request("open"), "owner.open")
        guard ticket == revision, !ownerStopped, foreground else { return }
        if !servicesPrepared {
          // Both services admit only the app identity selected at build time.
          let backup = journal_ios_backup_admit()
          let verifier = journal_ios_verifier_admit()
          let callback = verifier && journal_ios_verifier_register_callback(appraisalTransportCallback)
          runner.log("owner.service-admission", ["backup": backup, "verifier": verifier, "callback": callback])
          servicesPrepared = true
        }
        var request = runner.request("serve-start", count: 37657)
        request.admissionGeneration = admission
        retained = true // Even failed startup can retain workers needing a checked drain.
        _ = try await runner.servingCall(request, "owner.started")
        guard ticket == revision, !ownerStopped, foreground else { return }
        _ = try await runner.servingCall(runner.request("audio-enable"), "owner.audio-ready")
        guard ticket == revision, !ownerStopped, foreground else { return }
        route = "/init"
        page = UUID()
        // Sweep only here, before this start admits the owner page.
        reclaimOwnerBridge()
        phase = .running
        detail = "keep this app open and this device plugged in."
        UIApplication.shared.isIdleTimerDisabled = true
      } catch {
        runner.log("owner.start-failed", ["error": String(describing: error)])
        if ticket == revision {
          journal_ios_processing_request_stop()
          await drain(explicit: false)
          phase = .unavailable
          detail = retained
            ? "couldn't finish stopping. use stop to check again."
            : "couldn't open your journal. check Wi-Fi and available storage, then use start."
        }
      }
    }
  }
  func stop(restart: Bool = false) {
    guard admitFixture() else { return }
    recoveryReview = nil
    showRecoveryReview = false
    phase = .stopping
    detail = "finishing current work. keep this app open."
    UIApplication.shared.isIdleTimerDisabled = foreground && retained
    do { try saveChoice(true) }
    catch { ownerStopped = true; choiceSaved = false }
    revision &+= 1
    let ticket = revision
    journal_ios_processing_request_stop()
    // Stop reaches the journal before waiting behind any lifecycle operation.
    if retained { _ = journal_ios_processing_request_owner_stop() }
    enqueue { [self] in
      await drain(explicit: true)
      guard ticket == revision else { return }
      if retained { phase = .unavailable; detail = "couldn't finish stopping. use stop to check again." }
      else if !choiceSaved {
        phase = .unavailable
        detail = "stopped, but couldn't save that choice. free some space, then use stop again."
      } else {
        phase = .stopped
        detail = ""
        if restart { start() }
      }
    }
  }
  private func drain(explicit: Bool) async {
    UIApplication.shared.isIdleTimerDisabled = foreground && retained
    defer { UIApplication.shared.isIdleTimerDisabled = foreground && retained }
    guard retained else { return }
    do {
      await cancelAndDrain()
      _ = try await runner.servingCall(runner.request(explicit ? "serve-stop" : "serve-suspend"), "owner.drained")
      retained = false
    } catch { runner.log("owner.drain-failed", ["error": String(describing: error)]) }
  }
  func scene(_ active: Bool) {
    guard admitFixture() else { return }
    foreground = active
    UIApplication.shared.isIdleTimerDisabled = active && retained
    guard loaded else { return }
    if active {
      enqueue { [self] in
        if !ownerStopped && !retained { phase = .suspended; start(explicit: false) }
      }
    } else {
      recoveryReview = nil
      showRecoveryReview = false
      revision &+= 1
      journal_ios_processing_request_stop()
      UIApplication.shared.isIdleTimerDisabled = false
      phase = ownerStopped ? .stopping : .suspended
      enqueue { [self] in
        await drain(explicit: false)
        if retained { phase = .unavailable; detail = "couldn't finish pausing. use stop to check again." }
        else if ownerStopped { phase = choiceSaved ? .stopped : .unavailable }
        else { phase = .suspended }
      }
    }
  }
  func status() async {
    guard phase == .running, foreground else { return }
    do {
      let reply = try await runner.servingCall(runner.request("serve-status"), "owner.status")
      if let native = reply["journal"] as? [String: Any],
         let result = native["result"] as? [String: Any],
         let value = result["value"] as? [String: Any], value["failed"] as? Bool == true {
        phase = .unavailable
        detail = "processing stopped unexpectedly. open health to check what needs attention."
      }
    } catch {
      if phase == .running { phase = .unavailable; detail = "journal status unavailable. use stop to check before restarting." }
    }
  }
  func show(_ path: String) { route = path; page = UUID() }
  func pageFailed() { detail = "couldn't open this page. open home or health to try again." }
  func uploadUnavailable() { detail = "this file couldn't be sent. choose it again, and check the result before retrying." }

  private func recoveryCall(_ command: [String: Any]) async throws -> [String: Any] {
    let bytes = try OwnerCreatorRecovery.encode(command)
    var request = runner.request("creator-recovery", payload: String(decoding: bytes, as: UTF8.self))
    request.admissionGeneration = command["admission_generation"] as? UInt64
    let reply = try await runner.servingCall(request, "owner.creator-recovery")
    guard let native = reply["journal"] as? [String: Any],
      let result = native["result"] as? [String: Any],
      let report = result["value"] as? [String: Any],
      report["request_id"] as? String == command["request_id"] as? String,
      report["native_epoch"] as? UInt64 == command["admission_generation"] as? UInt64,
      let root = report["root_observation"] as? String
    else { throw OwnerCreatorRecovery.Refusal.invalidRecord }
    try OwnerCreatorRecovery.validate(report, root: root)
    if let expected = command["root_observation"] as? String, expected != root {
      throw OwnerCreatorRecovery.Refusal.invalidRecord
    }
    return report
  }
  private func saveRecovery(_ state: OwnerCreatorRecovery) throws {
    guard let recoveryFile else { throw OwnerCreatorRecovery.Refusal.invalidRecord }
    // Fail closed if publication/fsync fails, including before first dispatch.
    recoveryHeld = true
    try state.save(recoveryFile)
    recoveryState = state
    recoveryHeld = try state.heldIndex() != nil
  }
  func reviewRecovery() {
    guard canReviewRecovery else { return }
    recoveryBusy = true
    recoveryReview = nil
    recoveryDetail = "checking temporary recovery items. keep this app open."
    let ticket = revision
    let epoch = journal_ios_processing_generation()
    let command = OwnerCreatorRecovery.command("observe", epoch: epoch)
    enqueue { [self] in
      defer { recoveryBusy = false }
      guard ticket == revision, ownerStopped, foreground else { return }
      do {
        let report = try await recoveryCall(command)
        guard ticket == revision, foreground else { return }
        recoveryReview = try OwnerCreatorRecovery.review(report,
          id: command["request_id"] as! String, epoch: epoch)
        showRecoveryReview = true
        recoveryDetail = ""
      } catch {
        recoveryDetail = "couldn't complete the storage review. your journal is still stopped."
      }
    }
  }
  func applyRecovery() {
    guard canReviewRecovery, let review = recoveryReview,
      review.count > 0, review.epoch == journal_ios_processing_generation() else { return }
    recoveryReview = nil
    showRecoveryReview = false
    recoveryBusy = true
    recoveryDetail = "removing temporary recovery items. keep this app open."
    let command = OwnerCreatorRecovery.command("apply", epoch: review.epoch, review: review)
    let ticket = revision
    enqueue { [self] in
      defer { recoveryBusy = false }
      guard ticket == revision, ownerStopped, foreground else { return }
      do {
        guard let recoveryFile else { throw OwnerCreatorRecovery.Refusal.invalidRecord }
        var sent = try OwnerCreatorRecovery.load(recoveryFile)
        let index = sent.attempts.count
        sent.attempts.append(.init(command: try OwnerCreatorRecovery.encode(command)))
        try saveRecovery(sent) // Durable unknown outcome before native ingress.
        let report = try await recoveryCall(command)
        var received = recoveryState
        received.attempts[index].result = try OwnerCreatorRecovery.encode(report)
        try saveRecovery(received)
        recoveryDetail = report["local_complete"] as? Bool == true
          ? (recoveryHeld
            ? "temporary recovery items removed. an earlier local result is still unconfirmed. check current storage before starting."
            : "temporary recovery items removed. your journal is still stopped. start it separately when you are ready.")
          : "cleanup is incomplete. keep stopped and review current storage."
      } catch {
        recoveryDetail = "cleanup is incomplete or unconfirmed. keep stopped and check current storage."
      }
    }
  }
  func reconcileRecovery() {
    guard canReviewRecovery, recoveryHeld else { return }
    recoveryBusy = true
    recoveryReview = nil
    recoveryDetail = "checking current local storage. no cleanup request is being repeated."
    let ticket = revision
    let epoch = journal_ios_processing_generation()
    enqueue { [self] in
      defer { recoveryBusy = false }
      guard ticket == revision, ownerStopped, foreground else { return }
      do {
        // Re-read after a failed receipt save; memory is not receiving evidence.
        guard let recoveryFile else { throw OwnerCreatorRecovery.Refusal.invalidRecord }
        recoveryState = try OwnerCreatorRecovery.load(recoveryFile)
        guard let index = try recoveryState.heldIndex() else {
          recoveryHeld = false
          return
        }
        let command = try OwnerCreatorRecovery.reconcile(recoveryState.attempts[index], epoch: epoch)
        let report = try await recoveryCall(command)
        var received = recoveryState
        received.attempts[index].reconciliation = try OwnerCreatorRecovery.encode(report)
        try saveRecovery(received)
        recoveryDetail = report["current_local_state_reconciled"] as? Bool == true
          ? "current local storage checked. the earlier request and remote outcome remain unknown. your journal is still stopped."
          : "current local storage could not be confirmed. keep stopped and review storage."
      } catch {
        recoveryHeld = true
        recoveryDetail = "current local storage is unconfirmed. your journal is still stopped."
      }
    }
  }

  private func reclaimOwnerBridge() {
    let cache = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0]
    cache.path.withCString { cachePath in
      "[]".withCString { live in
        if let result = journal_ios_owner_reclaim(cachePath, live) { journal_runtime_free(result) }
      }
    }
  }
}

struct OwnerJournalView: View {
  @Environment(\.scenePhase) private var scenePhase
  @ObservedObject var owner: OwnerJournal
  var body: some View {
    VStack(spacing: 0) {
      VStack(alignment: .leading, spacing: 8) {
        HStack {
          Text(owner.label).accessibilityIdentifier("owner-run-state")
          Spacer()
          if owner.canStart { Button("start") { owner.start() }.accessibilityIdentifier("owner-start") }
          else if owner.phase != .stopping {
            Button("stop") { owner.stop() }.accessibilityIdentifier("owner-stop")
            if owner.phase == .running { Button("restart") { owner.stop(restart: true) } }
          }
        }
        if !owner.detail.isEmpty { Text(owner.detail).font(.caption).foregroundStyle(.secondary) }
        if owner.canReviewRecovery || owner.recoveryBusy {
          HStack {
            Button("review storage recovery") { owner.reviewRecovery() }
              .disabled(!owner.canReviewRecovery).accessibilityIdentifier("owner-recovery-review")
            if owner.recoveryHeld {
              Button("check current storage") { owner.reconcileRecovery() }
                .disabled(!owner.canReviewRecovery).accessibilityIdentifier("owner-recovery-check")
            }
          }
        }
        if !owner.recoveryDetail.isEmpty { Text(owner.recoveryDetail).font(.caption).foregroundStyle(.secondary) }
        if owner.hasOwnerPage {
          HStack {
            Button("home") { owner.show("/") }
            Button("health") { owner.show("/app/health/") }
          }
        }
      }.padding(12).background(.bar)
      if owner.hasOwnerPage {
        OwnerBrowser(owner: owner).id(owner.page)
      } else { Spacer() }
    }
    .alert("review storage recovery", isPresented: $owner.showRecoveryReview) {
      Button("keep stopped", role: .cancel) { owner.recoveryReview = nil }
      if let review = owner.recoveryReview, review.count > 0 {
        Button("remove temporary items", role: .destructive) { owner.applyRecovery() }
          .accessibilityIdentifier("owner-recovery-apply")
      }
    } message: {
      if let review = owner.recoveryReview {
        Text(review.count == 0
          ? "no temporary recovery items remain. this check does not tell us whether an earlier archive request completed."
          : "temporary recovery items: \(review.count). copied files have matching originals. empty temporary folders may also be removed. this does not tell us whether an earlier archive request completed.")
      }
    }
    .task {
      owner.scene(scenePhase == .active)
      owner.open()
      while !Task.isCancelled {
        try? await Task.sleep(for: .seconds(10))
        if !Task.isCancelled { await owner.status() }
      }
    }
    .onChange(of: scenePhase) { _, value in owner.scene(value == .active) }
  }
}
#endif
