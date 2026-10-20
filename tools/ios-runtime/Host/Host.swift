// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
import ExtensionFoundation
import SwiftUI
import UIKit
import XPC

extension AppExtensionPoint {
  @Definition static var journal: AppExtensionPoint { Name("journal") }
  @Definition static var gpuAppraisal: AppExtensionPoint { Name("gpu-appraisal") }
}

struct AppraisalWireRequest: Codable, Sendable {
  var op: String
  var payload: String = ""
  var bridgeAttempt: UInt64

  enum CodingKeys: String, CodingKey {
    case op
    case payload
    case bridgeAttempt = "bridge_attempt"
  }
}

private struct HostAttemptRecord: Codable {
  let schema: String
  let bootSeconds: Int64
  let bootMicroseconds: Int32
  let bridgeAttempt: UInt64
  let hostPid: Int32
  let helperPid: Int32?
  let deadlineUptimeMs: UInt64
  let phase: String

  enum CodingKeys: String, CodingKey {
    case schema
    case bootSeconds = "boot_seconds"
    case bootMicroseconds = "boot_microseconds"
    case bridgeAttempt = "bridge_attempt"
    case hostPid = "host_pid"
    case helperPid = "helper_pid"
    case deadlineUptimeMs = "deadline_uptime_ms"
    case phase
  }
}

final class AppraisalHostBridge: @unchecked Sendable {
  static let shared = AppraisalHostBridge()
  private let appGroup = "group.app.solstone.journal.runtime.dev"
  private let recordName = "gpu-appraisal-attempt.json"
  let lock = NSLock()
  private var currentSession: XPCSession?
  private var activePid: pid_t?
  private var activeAttempt: UInt64?
  private var ownershipUnknown = false
  private var isCancelled = false

  private func bootIdentity() -> (Int64, Int32)? {
    var boot = timeval()
    var size = MemoryLayout<timeval>.size
    guard sysctlbyname("kern.boottime", &boot, &size, nil, 0) == 0 else { return nil }
    return (Int64(boot.tv_sec), Int32(boot.tv_usec))
  }

  private func recordURL() -> URL? {
    FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: appGroup)?
      .appendingPathComponent(recordName, isDirectory: false)
  }

  private func readRecord(_ url: URL) -> HostAttemptRecord? {
    guard let data = try? Data(contentsOf: url, options: [.mappedIfSafe]),
          data.count <= 4_096
    else {
      return nil
    }
    return try? JSONDecoder().decode(HostAttemptRecord.self, from: data)
  }

  private func recordPredatesCurrentBoot(_ url: URL, bootSeconds: Int64) -> Bool {
    var info = stat()
    let status: Int32 = url.withUnsafeFileSystemRepresentation { path in
      guard let path else { return Int32(-1) }
      return stat(path, &info)
    }
    return status == 0 && Int64(info.st_mtimespec.tv_sec) < bootSeconds
  }

  private func writeRecord(_ record: HostAttemptRecord, to url: URL) -> Bool {
    let encoder = JSONEncoder()
    encoder.outputFormatting = [.sortedKeys]
    guard let data = try? encoder.encode(record),
          data.count <= 4_096
    else {
      return false
    }
    do {
      try data.write(to: url, options: [.atomic])
      let fileDescriptor = open(url.path, O_RDONLY | O_CLOEXEC)
      guard fileDescriptor >= 0 else { return false }
      let fileSynced = fsync(fileDescriptor) == 0
      close(fileDescriptor)
      let directoryDescriptor = open(url.deletingLastPathComponent().path, O_RDONLY | O_CLOEXEC)
      guard directoryDescriptor >= 0 else { return false }
      let directorySynced = fsync(directoryDescriptor) == 0
      close(directoryDescriptor)
      return fileSynced && directorySynced
    } catch {
      return false
    }
  }

  private func makeRecord(
    attempt: UInt64, deadlineMs: UInt64, helperPid: pid_t?, phase: String
  ) -> HostAttemptRecord? {
    guard let boot = bootIdentity(), attempt > 0 else { return nil }
    return HostAttemptRecord(
      schema: "gpu-appraisal-attempt-v1",
      bootSeconds: boot.0,
      bootMicroseconds: boot.1,
      bridgeAttempt: attempt,
      hostPid: Int32(getpid()),
      helperPid: helperPid,
      deadlineUptimeMs: deadlineMs,
      phase: phase)
  }

  private func reserve(attempt: UInt64, deadlineMs: UInt64) -> Bool {
    guard let url = recordURL(), let boot = bootIdentity() else { return false }
    if FileManager.default.fileExists(atPath: url.path) {
      if let prior = readRecord(url) {
        guard prior.schema == "gpu-appraisal-attempt-v1" else { return false }
        if prior.bootSeconds == boot.0 && prior.bootMicroseconds == boot.1 {
          guard prior.phase == "retired" else { return false }
        }
      } else if !recordPredatesCurrentBoot(url, bootSeconds: boot.0) {
        return false
      }
    }
    guard let record = makeRecord(
      attempt: attempt, deadlineMs: deadlineMs, helperPid: nil, phase: "reserved")
    else {
      return false
    }
    return writeRecord(record, to: url)
  }

  private func updateRecord(
    attempt: UInt64, deadlineMs: UInt64, helperPid: pid_t?, phase: String
  ) -> Bool {
    guard let url = recordURL(),
          let current = readRecord(url),
          let boot = bootIdentity(),
          current.schema == "gpu-appraisal-attempt-v1",
          current.bootSeconds == boot.0,
          current.bootMicroseconds == boot.1,
          current.bridgeAttempt == attempt,
          current.hostPid == Int32(getpid()),
          let next = makeRecord(
            attempt: attempt, deadlineMs: deadlineMs, helperPid: helperPid, phase: phase)
    else {
      return false
    }
    return writeRecord(next, to: url)
  }

  func cancel() {
    journal_ios_verifier_cancel()
    lock.lock()
    isCancelled = true
    let session = currentSession
    let pid = activePid
    let attempt = activeAttempt
    lock.unlock()

    if let session, let attempt {
      let request = AppraisalWireRequest(op: "cancel", bridgeAttempt: attempt)
      try? session.send(request) { (_: Result<Reply, Error>) in }
    }
    if let pid, pid > 0 {
      kill(pid, SIGKILL)
    }
  }

  func clearCancellationIfDrained() {
    lock.lock()
    if activePid == nil && !ownershipUnknown {
      isCancelled = false
      activeAttempt = nil
    }
    lock.unlock()
  }

  private func isPidDead(_ pid: pid_t) -> Bool {
    guard pid > 0 else { return false }
    return kill(pid, 0) == -1 && errno == ESRCH
  }

  private func waitForDeath(pid: pid_t, deadline: Double) async -> Bool {
    guard pid > 0 else { return false }
    while ProcessInfo.processInfo.systemUptime < deadline {
      if isPidDead(pid) { return true }
      try? await Task.sleep(for: .milliseconds(20))
    }
    kill(pid, SIGKILL)
    let killDeadline = ProcessInfo.processInfo.systemUptime + 2.0
    while ProcessInfo.processInfo.systemUptime < killDeadline {
      if isPidDead(pid) { return true }
      try? await Task.sleep(for: .milliseconds(20))
    }
    return isPidDead(pid)
  }

  private func json(_ value: [String: Any]) -> String {
    guard JSONSerialization.isValidJSONObject(value),
          let data = try? JSONSerialization.data(withJSONObject: value, options: [.sortedKeys])
    else {
      return "{}"
    }
    return String(data: data, encoding: .utf8) ?? "{}"
  }

  private func logTransportResult(_ value: String) {
    guard let data = value.data(using: .utf8),
          let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
    else {
      print("event=ios_verifier_transport malformed_summary=true")
      return
    }
    var summary: [String: Any] = [:]
    for key in ["bridge_attempt", "pid", "exited", "cancelled", "malformed"] {
      if let item = object[key] { summary[key] = item }
    }
    if let reply = object["reply"] as? [String: Any] {
      for key in ["bridge_attempt", "pid", "returncode", "result_code", "result_message", "main_thread_init"] {
        if let item = reply[key] { summary["reply_\(key)"] = item }
      }
      summary["reply_claims_count"] = (reply["claims"] as? [Any])?.count ?? -1
      summary["reply_detached_eat_count"] = (reply["detached_eat"] as? [Any])?.count ?? -1
    }
    print("event=ios_verifier_transport summary=\(json(summary))")
  }

  private func send(
    _ request: AppraisalWireRequest, through session: XPCSession, timeout: Double
  ) async throws -> Reply {
    try await withCheckedThrowingContinuation { continuation in
      let pending = PendingReply(continuation)
      let timer = DispatchWorkItem {
        if pending.finish(.failure(NSError(domain: "appraisal-timeout", code: 151))) {
          session.cancel(reason: "appraisal timeout")
        }
      }
      pending.watchdog = timer
      DispatchQueue.global().asyncAfter(deadline: .now() + max(0.001, timeout), execute: timer)
      do {
        try session.send(request) { (result: Result<Reply, Error>) in pending.finish(result) }
      } catch {
        pending.finish(.failure(error))
      }
    }
  }

  private func finishKnownProcess(
    attempt: UInt64, pid: pid_t, deadlineMs: UInt64, deadline: Double
  ) async -> Bool {
    let dead = await waitForDeath(pid: pid, deadline: deadline)
    lock.lock()
    if dead {
      activePid = nil
      currentSession = nil
    } else {
      ownershipUnknown = true
    }
    lock.unlock()
    if dead {
      let recorded = updateRecord(
        attempt: attempt, deadlineMs: deadlineMs, helperPid: pid, phase: "retired")
      if recorded {
        lock.lock()
        activeAttempt = nil
        lock.unlock()
      }
      return recorded
    }
    return false
  }

  func transport(_ cRequestStr: UnsafePointer<CChar>?) -> UnsafeMutablePointer<CChar>? {
    guard let cRequestStr else {
      return strdup(json([
        "bridge_attempt": 0,
        "malformed": "null request pointer",
        "exited": false,
        "cancelled": false,
      ]))
    }
    let requestString = String(cString: cRequestStr)
    guard requestString.utf8.count <= 1_114_112,
          let data = requestString.data(using: .utf8),
          let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
          let attemptNumber = object["bridge_attempt"] as? NSNumber,
          let budgetNumber = object["budget_ms"] as? NSNumber
    else {
      return strdup(json([
        "bridge_attempt": 0,
        "malformed": "invalid bounded appraisal request",
        "exited": false,
        "cancelled": false,
      ]))
    }
    let attempt = attemptNumber.uint64Value
    let budgetMs = min(budgetNumber.uint64Value, 60_000)
    guard attempt > 0, budgetMs > 0 else {
      return strdup(json([
        "bridge_attempt": attempt,
        "malformed": "invalid appraisal attempt or budget",
        "retired_without_launch": true,
        "cancelled": false,
      ]))
    }

    let start = ProcessInfo.processInfo.systemUptime
    let deadline = start + Double(budgetMs) / 1_000.0
    let deadlineMs = UInt64(deadline * 1_000)

    lock.lock()
    if ownershipUnknown || activePid != nil || activeAttempt != nil {
      lock.unlock()
      return strdup(json([
        "bridge_attempt": attempt,
        "malformed": "previous helper ownership has not retired",
        "exited": false,
        "cancelled": false,
      ]))
    }
    if isCancelled {
      lock.unlock()
      return strdup(json([
        "bridge_attempt": attempt,
        "malformed": "appraisal cancelled before reservation",
        "retired_without_launch": true,
        "cancelled": true,
      ]))
    }
    activeAttempt = attempt
    lock.unlock()

    guard reserve(attempt: attempt, deadlineMs: deadlineMs) else {
      lock.lock()
      ownershipUnknown = true
      lock.unlock()
      return strdup(json([
        "bridge_attempt": attempt,
        "malformed": "appraisal ownership record refused",
        "exited": false,
        "cancelled": false,
      ]))
    }

    let result = AppraisalResultBox()
    let semaphore = DispatchSemaphore(value: 0)
    Task {
      do {
        let monitor = try await AppExtensionPoint.Monitor(appExtensionPoint: .gpuAppraisal)
        let matches = monitor.identities.filter {
          $0.bundleIdentifier == "app.solstone.journal.runtime.dev.gpu-appraisal"
        }
        guard matches.count == 1, let identity = matches.first else {
          guard self.updateRecord(
            attempt: attempt, deadlineMs: deadlineMs, helperPid: nil, phase: "retired")
          else {
            self.lock.lock()
            self.ownershipUnknown = true
            self.lock.unlock()
            throw NSError(domain: "appraisal-record", code: 1)
          }
          self.lock.lock()
          self.activeAttempt = nil
          let cancelled = self.isCancelled
          self.lock.unlock()
          result.finish(self.json([
            "bridge_attempt": attempt,
            "malformed": "extension discovery failed",
            "retired_without_launch": true,
            "cancelled": cancelled,
          ]))
          semaphore.signal()
          return
        }

        self.lock.lock()
        let cancelledBeforeLaunch = self.isCancelled
        self.lock.unlock()
        if cancelledBeforeLaunch {
          guard self.updateRecord(
            attempt: attempt, deadlineMs: deadlineMs, helperPid: nil, phase: "retired")
          else {
            throw NSError(domain: "appraisal-record", code: 2)
          }
          self.lock.lock()
          self.activeAttempt = nil
          self.lock.unlock()
          result.finish(self.json([
            "bridge_attempt": attempt,
            "malformed": "appraisal cancelled before launch",
            "retired_without_launch": true,
            "cancelled": true,
          ]))
          semaphore.signal()
          return
        }

        let process = try await AppExtensionProcess(
          configuration: .init(appExtensionIdentity: identity))
        let session = try process.makeXPCSession()
        try session.activate()

        self.lock.lock()
        self.currentSession = session
        self.lock.unlock()

        let helloTimeout = max(0.001, deadline - ProcessInfo.processInfo.systemUptime)
        let hello = try await self.send(
          AppraisalWireRequest(op: "hello", bridgeAttempt: attempt),
          through: session,
          timeout: helloTimeout)
        guard let helloObject =
          (try? JSONSerialization.jsonObject(with: Data(hello.json.utf8))) as? [String: Any],
          (helloObject["bridge_attempt"] as? NSNumber)?.uint64Value == attempt,
          (helloObject["hello"] as? Bool) == true,
          let pidNumber = helloObject["pid"] as? NSNumber,
          pidNumber.int64Value > 0,
          pidNumber.int64Value <= Int64(Int32.max)
        else {
          self.lock.lock()
          self.ownershipUnknown = true
          self.lock.unlock()
          throw NSError(domain: "appraisal-hello", code: 1)
        }
        let pid = pid_t(pidNumber.int32Value)
        let aliveProbe = kill(pid, 0)
        let aliveProbeErrno = aliveProbe == -1 ? errno : 0
        print("event=ios_verifier_alive attempt=\(attempt) pid=\(pid) kill_zero_result=\(aliveProbe) errno=\(aliveProbeErrno)")
        self.lock.lock()
        self.activePid = pid
        let cancelledAfterHello = self.isCancelled
        self.lock.unlock()

        guard self.updateRecord(
          attempt: attempt, deadlineMs: deadlineMs, helperPid: pid, phase: "launched")
        else {
          kill(pid, SIGKILL)
          _ = await self.finishKnownProcess(
            attempt: attempt, pid: pid, deadlineMs: deadlineMs,
            deadline: ProcessInfo.processInfo.systemUptime + 2.0)
          throw NSError(domain: "appraisal-record", code: 3)
        }

        if cancelledAfterHello {
          kill(pid, SIGKILL)
          let dead = await self.finishKnownProcess(
            attempt: attempt, pid: pid, deadlineMs: deadlineMs,
            deadline: ProcessInfo.processInfo.systemUptime + 2.0)
          result.finish(self.json([
            "bridge_attempt": attempt,
            "pid": Int(pid),
            "cancelled": true,
            "exited": dead,
          ]))
          semaphore.signal()
          return
        }

        let replyTimeout = max(0.001, deadline - ProcessInfo.processInfo.systemUptime)
        let reply = try await self.send(
          AppraisalWireRequest(
            op: "appraise", payload: requestString, bridgeAttempt: attempt),
          through: session,
          timeout: replyTimeout)
        let replyObject =
          (try? JSONSerialization.jsonObject(with: Data(reply.json.utf8))) as? [String: Any]
        guard let replyObject,
              (replyObject["bridge_attempt"] as? NSNumber)?.uint64Value == attempt,
              (replyObject["pid"] as? NSNumber)?.int32Value == pid
        else {
          kill(pid, SIGKILL)
          let dead = await self.finishKnownProcess(
            attempt: attempt, pid: pid, deadlineMs: deadlineMs,
            deadline: ProcessInfo.processInfo.systemUptime + 2.0)
          result.finish(self.json([
            "bridge_attempt": attempt,
            "pid": Int(pid),
            "malformed": "helper reply identity mismatch",
            "cancelled": false,
            "exited": dead,
          ]))
          semaphore.signal()
          return
        }

        let dead = await self.finishKnownProcess(
          attempt: attempt, pid: pid, deadlineMs: deadlineMs, deadline: deadline)
        self.lock.lock()
        let cancelled = self.isCancelled
        self.lock.unlock()
        let deadlineExpired = ProcessInfo.processInfo.systemUptime >= deadline
        if deadlineExpired {
          result.finish(self.json([
            "bridge_attempt": attempt,
            "pid": Int(pid),
            "malformed": "appraisal deadline expired",
            "deadline": true,
            "exited": dead,
            "cancelled": cancelled,
          ]))
        } else {
          result.finish(self.json([
            "bridge_attempt": attempt,
            "pid": Int(pid),
            "reply": replyObject,
            "exited": dead,
            "cancelled": cancelled,
          ]))
        }
      } catch {
        self.lock.lock()
        let pid = self.activePid
        let cancelled = self.isCancelled
        self.lock.unlock()
        let deadlineExpired = ProcessInfo.processInfo.systemUptime >= deadline
        if let pid {
          kill(pid, SIGKILL)
          let dead = await self.finishKnownProcess(
            attempt: attempt, pid: pid, deadlineMs: deadlineMs,
            deadline: ProcessInfo.processInfo.systemUptime + 2.0)
          result.finish(self.json([
            "bridge_attempt": attempt,
            "pid": Int(pid),
            "malformed": deadlineExpired ? "appraisal deadline expired" : cancelled ? "appraisal cancelled" : "helper communication failure",
            "deadline": deadlineExpired,
            "cancelled": cancelled,
            "exited": dead,
          ]))
        } else {
          self.lock.lock()
          self.ownershipUnknown = true
          self.lock.unlock()
          result.finish(self.json([
            "bridge_attempt": attempt,
            "malformed": deadlineExpired ? "appraisal deadline expired before PID proof" : cancelled ? "appraisal cancelled before PID proof" : "helper ownership unknown",
            "deadline": deadlineExpired,
            "cancelled": cancelled,
            "exited": false,
          ]))
        }
      }
      semaphore.signal()
    }

    if semaphore.wait(timeout: .now() + Double(budgetMs) / 1_000.0 + 3.0) == .timedOut {
      lock.lock()
      ownershipUnknown = true
      let pid = activePid
      lock.unlock()
      if let pid { kill(pid, SIGKILL) }
      var report: [String: Any] = [
        "bridge_attempt": attempt,
        "malformed": "host transport deadline expired without retirement proof",
        "deadline": true,
        "cancelled": false,
        "exited": false,
      ]
      if let pid { report["pid"] = Int(pid) }
      return strdup(json(report))
    }
    let response = result.value() ?? json([
      "bridge_attempt": attempt,
      "malformed": "host transport returned no result",
      "cancelled": false,
      "exited": false,
    ])
    logTransportResult(response)
    return strdup(response)
  }
}

final class AppraisalResultBox: @unchecked Sendable {
  private let lock = NSLock()
  private var result: String?

  func finish(_ value: String) {
    lock.lock()
    if result == nil { result = value }
    lock.unlock()
  }

  func value() -> String? {
    lock.lock()
    defer { lock.unlock() }
    return result
  }
}


let appraisalTransportCallback: journal_ios_transport_fn = { cRequestStr in
  AppraisalHostBridge.shared.transport(cRequestStr)
}

func cancelAndDrain() async {
  AppraisalHostBridge.shared.cancel()
  let deadline = ProcessInfo.processInfo.systemUptime + 3.0
  while !journal_ios_verifier_drain_ready()
    && ProcessInfo.processInfo.systemUptime < deadline
  {
    try? await Task.sleep(for: .milliseconds(50))
  }
  if journal_ios_verifier_drain_ready() {
    AppraisalHostBridge.shared.clearCancellationIfDrained()
  }
}
final class PendingReply: @unchecked Sendable {
  let lock = NSLock()
  var continuation: CheckedContinuation<Reply, Error>?
  var watchdog: DispatchWorkItem?
  init(_ c: CheckedContinuation<Reply, Error>) { continuation = c }
  @discardableResult func finish(_ result: Result<Reply, Error>) -> Bool {
    lock.lock()
    let c = continuation
    continuation = nil
    watchdog?.cancel()
    watchdog = nil
    lock.unlock()
    guard let c else { return false }
    c.resume(with: result)
    return true
  }
}

@MainActor final class Runner: ObservableObject {
  @Published var status = "journal closed"
  @Published var busy = false
  @Published var result = ""
  var process: AppExtensionProcess?
  var session: XPCSession?
  var started = false
  var finished = false
  var servingActive = false
  let output = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
  let args = ProcessInfo.processInfo.arguments
  func argument(_ name: String, _ fallback: String) -> String {
    guard let i = args.firstIndex(of: name), i + 1 < args.count else { return fallback }
    return args[i + 1]
  }
  var journal: String { argument("--suite", "development") }
  func request(
    _ op: String, count: Int = 128, seconds: Int = 0, revision: Int = 0, mode: String = "reopen",
    root: String = "", payload: String = ""
  ) -> Request {
    Request(
      op: op, journal: journal, root: root, count: count, seconds: seconds, revision: revision,
      mode: mode, payload: payload, storage: args.contains("--serve") ? "serving" : "private")
  }
  func log(_ event: String, _ value: Any) {
    do {
      let row: [String: Any] = [
        "event": event, "time": Date().timeIntervalSince1970,
        "uptime": ProcessInfo.processInfo.systemUptime, "host_pid": getpid(), "value": value,
      ]
      let data = try JSONSerialization.data(withJSONObject: row, options: [.sortedKeys])
      let file = output.appendingPathComponent("evidence.jsonl")
      if !FileManager.default.fileExists(atPath: file.path) {
        FileManager.default.createFile(atPath: file.path, contents: nil)
      }
      let handle = try FileHandle(forWritingTo: file)
      try handle.seekToEnd()
      let line = data + Data([10])
      try handle.write(contentsOf: line)
      try handle.close()
      try FileHandle.standardOutput.write(contentsOf: line)
      status = event
    } catch {
      status = "evidence error: \(error)"
      print(status)
    }
  }
  func reopen() async throws {
    session?.cancel(reason: "explicit reconnect")
    session = nil
    process = nil
    let monitor = try await AppExtensionPoint.Monitor(appExtensionPoint: .journal)
    guard let identity = monitor.identities.first else {
      throw NSError(domain: "helper-not-discovered", code: 1)
    }
    let p = try await AppExtensionProcess(
      configuration: .init(
        appExtensionIdentity: identity,
        onInterruption: { [weak self] in
          Task { @MainActor in self?.log("helper.interruption", ["observed": true]) }
        }))
    let s = try p.makeXPCSession()
    try s.activate()
    process = p
    session = s
    _ = try await call(s, request("open"), "open")
  }
  func call(_ s: XPCSession, _ request: Request, _ label: String) async throws -> [String: Any] {
    let start = ProcessInfo.processInfo.systemUptime
    let reply: Reply = try await withCheckedThrowingContinuation { continuation in
      let pending = PendingReply(continuation)
      let timer = DispatchWorkItem {
        if pending.finish(.failure(NSError(domain: "reply-timeout", code: 150))) {
          s.cancel(reason: "reply timeout; outcome may be committed")
        }
      }
      pending.watchdog = timer
      DispatchQueue.global().asyncAfter(deadline: .now() + 150, execute: timer)
      do { try s.send(request) { (r: Result<Reply, Error>) in pending.finish(r) } } catch {
        pending.finish(.failure(error))
      }
    }
    let obj = try JSONSerialization.jsonObject(with: Data(reply.json.utf8)) as! [String: Any]
    log(
      label, ["roundtrip_ms": (ProcessInfo.processInfo.systemUptime - start) * 1000, "reply": obj])
    if let e = obj["error"] { throw NSError(domain: String(describing: e), code: 1) }
    if let j = obj["journal"] as? [String: Any], let r = j["result"] as? [String: Any],
      r["ok"] as? Bool == false
    {
      throw NSError(domain: String(describing: r["error"]), code: 1)
    }
    return obj
  }
  func invoke(_ request: Request, _ label: String) async throws -> [String: Any] {
    guard let s = session else { throw NSError(domain: "journal-closed", code: 1) }
    return try await call(s, request, label)
  }
  func servingCall(_ request: Request, _ label: String) async throws -> [String: Any] {
    var admitted = request
    admitted.admissionGeneration = request.admissionGeneration ?? journal_ios_processing_generation()
    let queuedRequest = admitted
    if request.op == "serve-stop" {
      journal_ios_processing_request_stop()
      await cancelAndDrain()
    }
    let start = ProcessInfo.processInfo.systemUptime
    let reply: Reply = await withCheckedContinuation { continuation in
      Engine.queue.async { continuation.resume(returning: Engine.execute(queuedRequest)) }
    }
    let obj = try JSONSerialization.jsonObject(with: Data(reply.json.utf8)) as! [String: Any]
    log(label, ["roundtrip_ms": (ProcessInfo.processInfo.systemUptime - start) * 1000, "reply": obj])
    if let error = obj["error"] { throw NSError(domain: String(describing: error), code: 1) }
    if let native = obj["journal"] as? [String: Any], let result = native["result"] as? [String: Any],
      result["ok"] as? Bool == false {
      throw NSError(domain: String(describing: result["error"]), code: 1)
    }
    if request.op == "serve-start" { servingActive = true }
    if request.op == "serve-stop" { servingActive = false }
    return obj
  }
  func command(_ op: String) {
    Task {
      busy = true
      defer { busy = false }
      do {
        if op == "open" {
          try await reopen()
          return
        }
        let reply = try await invoke(request(op), op)
        result = String(describing: reply)
      } catch {
        status = "operation failed: \(error)"
        result = status
      }
    }
  }
  func stopServing() {
    // This signal bypasses the queue a provider call may currently occupy.
    journal_ios_processing_request_stop()
    log("serve.stop-requested", ["generation": journal_ios_processing_generation()])
    Task {
      do {
        result = String(describing: try await servingCall(request("serve-stop"), "serve.stopped"))
      } catch {
        status = "operation failed: \(error)"
        result = status
      }
    }
  }
  func export() async throws {
    guard !servingActive else { throw NSError(domain: "stop-serving-before-export", code: 1) }
    if args.contains("--serve") {
      _ = try await servingCall(request("serve-export-admit"), "export.host-admission")
    }
    let exported = try await invoke(request("export"), "export")
    for file in [
      "indexer/journal.sqlite", "indexer/journal.sqlite-wal", "checkpoint.bin", "committed.bin",
    ] {
      if let encoded = exported[file] as? String, let data = Data(base64Encoded: encoded) {
        try data.write(
          to: output.appendingPathComponent(
            journal + "-" + file.replacingOccurrences(of: "/", with: "-")), options: .atomic)
      }
    }
  }
  func serve(_ duration: Int, admissionGeneration: UInt64) async throws {
    let port = Int(argument("--port", "37657")) ?? 37657
    var startRequest = request("serve-start", count: port)
    startRequest.admissionGeneration = admissionGeneration
    _ = try await servingCall(request("open"), "serve.open")
    _ = try await servingCall(startRequest, "serve.start")
    if args.contains("--audio") {
      _ = try await servingCall(
        request("audio-enable", mode: argument("--audio-mode", "reopen")), "audio.enabled")
    }
    let deadline = ProcessInfo.processInfo.systemUptime + Double(duration)
    let commandFile = output.appendingPathComponent("owner-command.json")
    let replyFile = output.appendingPathComponent("owner-reply.json")
    let seenFile = output.appendingPathComponent("owner-command-seen.txt")
    var lastID = (try? String(contentsOf: seenFile, encoding: .utf8)) ?? ""
    var done = false
    while !done && ProcessInfo.processInfo.systemUptime < deadline {
      if FileManager.default.fileExists(atPath: commandFile.path) {
        let data = try Data(contentsOf: commandFile)
        // USB transfer may still be writing the file. Consume only complete JSON.
        guard let command = try? JSONDecoder().decode(ServingCommand.self, from: data) else {
          try await Task.sleep(for: .milliseconds(500))
          continue
        }
        guard command.id != lastID else {
          try await Task.sleep(for: .milliseconds(500))
          continue
        }
        // Retain the transfer destination until devicectl finishes its file-node check.
        // Persist consumption before execution; host death leaves an uncertain reply,
        // never an automatic replay of an owner operation.
        try command.id.write(to: seenFile, atomically: true, encoding: .utf8)
        lastID = command.id
        var reply: [String: Any] = [:]
        do {
          reply["id"] = command.id
          guard command.request.journal == journal, command.request.root.isEmpty,
            command.request.storage == "serving" else {
            throw NSError(domain: "command-journal-mismatch", code: 1)
          }
          if command.request.op == "serve-restart-worker" {
            do { _ = try await invoke(request("serve-crash"), "serve.crash") } catch {
              log("serve.crash.reply-loss", ["error": String(describing: error)])
            }
            try await reopen()
            reply["reply"] = try await invoke(request("serve-index"), "serve.restarted")
          } else if command.request.op == "serve-finish" {
            reply["reply"] = try await servingCall(request("serve-stop"), "serve.stopped")
            done = true
          } else if command.request.op == "export" {
            try await export()
            reply["reply"] = ["exported": true]
          } else if command.request.op == "serve-index" {
            reply["reply"] = try await invoke(command.request, "owner.command")
          } else {
            reply["reply"] = try await servingCall(command.request, "owner.command")
          }
          reply["ok"] = true
        } catch {
          reply["ok"] = false
          reply["error"] = String(describing: error)
          log("owner.command.error", reply)
        }
        try JSONSerialization.data(withJSONObject: reply, options: [.sortedKeys])
          .write(to: replyFile, options: .atomic)
        log("owner.reply", reply)
      }
      try await Task.sleep(for: .milliseconds(500))
    }
    _ = try await servingCall(request("serve-stop"), "serve.stopped")
    log("serve.complete", ["duration": duration, "explicit_finish": done])
  }
  func start() async {
    guard !started else { return }
    started = true
    let startupGeneration = journal_ios_processing_generation()
    let backupAdmitted = journal_ios_backup_admit()
    let backupDiagnosticPointer = journal_ios_backup_admission_diagnostic()
    let backupDiagnostic = backupDiagnosticPointer.map { String(cString: $0) } ?? "unavailable"
    journal_runtime_free(backupDiagnosticPointer)
    log("backup.admission", ["admitted": backupAdmitted, "diagnostic": backupDiagnostic])
    UIApplication.shared.isIdleTimerDisabled = true
    UIDevice.current.isBatteryMonitoringEnabled = true
    guard args.contains("--qualify") || args.contains("--serve") else { return }
    let scenario = args.contains("--serve") ? "serving" : argument("--scenario", "acceptance")
    let count = Int(argument("--count", "128")) ?? 128
    let duration = Int(argument("--duration", "1200")) ?? 1200
    let watchdog = Task {
      try await Task.sleep(for: .seconds(1800))
      log("host.watchdog", [:])
      exit(78)
    }
    defer {
      watchdog.cancel()
      finished = true
    }
    for _ in 0..<50 {
      if UIApplication.shared.applicationState == .active { break }
      try? await Task.sleep(for: .milliseconds(100))
    }
    Task {
      while !finished {
        try? await Task.sleep(for: .seconds(1))
        if !finished {
          if args.contains("--audio") && servingActive {
            _ = try? await servingCall(request("serve-status"), "audio.status")
          }
          log(
            "host.heartbeat",
            [
              "state": UIApplication.shared.applicationState.rawValue, "resources": resources(),
              "thermal": ProcessInfo.processInfo.thermalState.rawValue,
              "battery": UIDevice.current.batteryState.rawValue,
            ])
        }
      }
    }
    log(
      "host.start",
      [
        "state": UIApplication.shared.applicationState.rawValue, "scenario": scenario,
        "journal": journal, "count": count, "duration": duration, "documents": output.path,
        "resources": resources(),
      ])
    do {
      let verifierAdmitted = journal_ios_verifier_admit()
      let admissionDiagnosticPointer = journal_ios_verifier_admission_diagnostic()
      let admissionDiagnostic = admissionDiagnosticPointer.map { String(cString: $0) } ?? "unavailable"
      journal_runtime_free(admissionDiagnosticPointer)
      let verifierCallbackRegistered =
        verifierAdmitted
        && journal_ios_verifier_register_callback(appraisalTransportCallback)
      log(
        "verifier.admission",
        [
          "admitted": verifierAdmitted,
          "callback_registered": verifierCallbackRegistered,
          "diagnostic": admissionDiagnostic,
        ])
      if scenario == "verifier-admission" {
        guard verifierAdmitted && verifierCallbackRegistered else {
          throw NSError(domain: "verifier-admission-refused", code: 1)
        }
        return
      }
      try Data("host-owned authority control".utf8).write(
        to: output.appendingPathComponent("host.txt"))
      try await reopen()
      if scenario == "serving" {
        try await serve(duration, admissionGeneration: startupGeneration)
        return
      }
      _ = try await invoke(
        request("probe", root: output.appendingPathComponent("host.txt").path), "authority")
      if scenario != "reopen" {
        _ = try await invoke(request("seed", count: count), "seed")
        _ = try await invoke(request("index"), "index")
      }
      if scenario == "acceptance" {
        _ = try await invoke(request("create"), "create.first")
        do {
          _ = try await invoke(request("create"), "create.unexpected")
          throw NSError(domain: "existing-destination-accepted", code: 2)
        } catch {
          guard (error as NSError).domain != "existing-destination-accepted" else { throw error }
          log("create.refused", String(describing: error))
        }
        _ = try await invoke(request("binding-control"), "binding.control")
        for fault in ["crash-write", "commit-disconnect"] {
          do {
            _ = try await invoke(request(fault), fault + ".unexpected")
            throw NSError(domain: "fault-did-not-fire", code: 2)
          } catch {
            guard (error as NSError).domain != "fault-did-not-fire" else { throw error }
            log(fault + ".lost-reply", String(describing: error))
          }
          try await Task.sleep(for: .seconds(1))
          try await reopen()
          _ = try await invoke(
            request(fault == "crash-write" ? "recover" : "inventory"), fault + ".reopened")
        }
        // A new process object may resolve to the same operating-system process.
        try await reopen()
        _ = try await invoke(request("probe"), "handle.reopened")
        let interruptedSession = session!
        let interrupted = Task {
          try await self.call(
            interruptedSession, self.request("active", seconds: 30), "connection.work")
        }
        try await Task.sleep(for: .seconds(1))
        interruptedSession.cancel(reason: "qualification connection interruption")
        do {
          _ = try await interrupted.value
          log("connection.reply-after-cancel", [:])
        } catch { log("connection.lost-reply", String(describing: error)) }
        try await reopen()
        _ = try await invoke(request("cancel"), "connection.cancel")
        _ = try await invoke(request("query"), "connection.recovered")
        let s = session!
        let active = Task {
          try await self.call(s, self.request("active", seconds: 30), "cancel.work")
        }
        try await Task.sleep(for: .seconds(2))
        _ = try await invoke(request("cancel"), "cancel.control")
        _ = try await active.value
        _ = try await invoke(request("probe"), "idle.before")
        try await Task.sleep(for: .seconds(30))
        _ = try await invoke(request("query"), "idle.after")
      } else if scenario == "hostloss" {
        let work = Task {
          try await self.invoke(self.request("active", seconds: 120), "hostloss.work")
        }
        try await Task.sleep(for: .seconds(1))
        let observed = try await invoke(request("activity"), "hostloss.activity")
        guard let native = observed["journal"] as? [String: Any],
          let result = native["result"] as? [String: Any],
          let value = result["value"] as? [String: Any], value["active"] as? Bool == true
        else { throw NSError(domain: "worker-not-active", code: 1) }
        log("host.kill-ready", [:])
        _ = try await work.value
        throw NSError(domain: "host-was-not-killed", code: 2)
      } else if scenario == "controls" {
        for mode in ["reopen", "retained", "reopen", "retained", "session"] {
          log("control.begin", ["mode": mode])
          _ = try await invoke(request("control", seconds: 10, mode: mode), "control." + mode)
        }
      } else if scenario == "mixed" {
        let start = ProcessInfo.processInfo.systemUptime
        var n = 0
        while ProcessInfo.processInfo.systemUptime - start < Double(max(1, min(duration, 1500))) {
          n += 1
          _ = try await invoke(request("mixed", count: n, revision: n), "mixed.\(n)")
          try await Task.sleep(for: .seconds(1))
        }
        log(
          "mixed.completed", ["units": n, "seconds": ProcessInfo.processInfo.systemUptime - start])
      }
      _ = try await invoke(request("query"), "final.query")
      _ = try await invoke(request("inventory"), "final.inventory")
      try await export()
      log(
        "host.finished",
        [
          "passed": true, "state": UIApplication.shared.applicationState.rawValue,
          "resources": resources(),
        ])
      if !args.contains("--hold") {
        try await Task.sleep(for: .seconds(1))
        exit(0)
      }
    } catch {
      log("host.failed", String(describing: error))
      if !args.contains("--hold") { exit(1) }
    }
  }
}
@main struct RuntimeDev: App {
  @Environment(\.scenePhase) var scenePhase
  @StateObject var runner = Runner()
  var body: some Scene {
    WindowGroup {
      VStack(alignment: .leading, spacing: 16) {
        Text("journal development").font(.title)
        Text(runner.status).accessibilityIdentifier("runtime-status")
        HStack {
          Button("open / reconnect") { runner.command("open") }
          Button("cancel") { runner.command("cancel") }
          Button("stop journal") { runner.stopServing() }
            .accessibilityIdentifier("runtime-stop")
        }
        HStack {
          Button("add sample memories") { runner.command("seed") }
          Button("index") { runner.command("index") }
          Button("search") { runner.command("query") }
        }
        ScrollView { Text(runner.result).font(.caption) }
      }
      .padding()
      .task { await runner.start() }
      .onChange(of: scenePhase) { newPhase in
        if newPhase != .active {
          journal_ios_processing_request_stop()
          Task {
            await cancelAndDrain()
          }
        }
      }
    }
  }
}
