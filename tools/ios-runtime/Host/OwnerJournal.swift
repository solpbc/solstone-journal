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
  private let runner = Runner()
  private var pending: Task<Void, Never>?
  private var revision: UInt64 = 0
  private var retained = false
  private var loaded = false
  private var foreground = true
  private var ownerStopped = true
  private var choiceSaved = true
  private var servicesPrepared = false
  private let choice = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
    .appendingPathComponent("owner-run-choice.json")

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
  var canStart: Bool { loaded && choiceSaved && !retained && phase != .closed && phase != .starting && phase != .stopping }

  private struct Choice: Codable { let stopped: Bool }
  private func saveChoice(_ stopped: Bool) throws {
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
    guard !loaded else { return }
    loaded = true
    UIDevice.current.isBatteryMonitoringEnabled = true
    do {
      if FileManager.default.fileExists(atPath: choice.path) {
        ownerStopped = try JSONDecoder().decode(Choice.self, from: Data(contentsOf: choice)).stopped
      } else {
        try saveChoice(false)
      }
      if ownerStopped { phase = .stopped }
      else { start(explicit: false) }
    } catch {
      ownerStopped = true
      choiceSaved = false
      phase = .unavailable
      detail = "couldn't read the start choice. use stop to save it again."
    }
  }
  func start(explicit: Bool = true) {
    if explicit && !canStart { return }
    guard !retained, phase != .starting, phase != .stopping else { return }
    if explicit {
      do { try saveChoice(false) }
      catch { phase = .unavailable; detail = "couldn't save start. free some space, then try again."; return }
    }
    guard !ownerStopped, foreground else { return }
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
          // The verifier admits only this app's own appraisal extension. Backup
          // keeps its development-app identity, so it refuses here.
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
    foreground = active
    UIApplication.shared.isIdleTimerDisabled = active && retained
    guard loaded else { return }
    if active {
      enqueue { [self] in
        if !ownerStopped && !retained { phase = .suspended; start(explicit: false) }
      }
    } else {
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
  func uploadUnavailable() { detail = "file import from this page isn't available in this build. use a paired device to add media." }
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
        if owner.phase == .running || owner.phase == .unavailable && !owner.canStart {
          HStack {
            Button("home") { owner.show("/") }
            Button("health") { owner.show("/app/health/") }
          }
        }
      }.padding(12).background(.bar)
      if owner.phase == .running || owner.phase == .unavailable && !owner.canStart {
        OwnerBrowser(owner: owner).id(owner.page)
      } else { Spacer() }
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
