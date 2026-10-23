// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
import Darwin
import ExtensionFoundation
import Foundation
import XPC

@_silgen_name("ios_gpu_appraisal_initialize")
func ios_gpu_appraisal_initialize(_ budgetMs: UInt64) -> UInt16

@_silgen_name("ios_gpu_appraisal_execute")
func ios_gpu_appraisal_execute(_ input: UnsafePointer<CChar>?) -> UnsafeMutablePointer<CChar>?

@_silgen_name("ios_gpu_appraisal_free")
func ios_gpu_appraisal_free(_ ptr: UnsafeMutablePointer<CChar>?)

private let appGroup = RuntimeIdentity.group
private let attemptRecordName = "gpu-appraisal-attempt.json"
private let maximumRequestBytes = 1_114_112

struct Request: Codable, Sendable {
  var op: String = ""
  var payload: String = ""
  var bridgeAttempt: UInt64 = 0

  enum CodingKeys: String, CodingKey {
    case op
    case payload
    case bridgeAttempt = "bridge_attempt"
  }
}
struct Reply: Codable, Sendable {
  var json: String = ""
}
private struct AttemptRecord: Codable {
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
private func currentBootIdentity() -> (Int64, Int32)? {
  var boot = timeval()
  var size = MemoryLayout<timeval>.size
  guard sysctlbyname("kern.boottime", &boot, &size, nil, 0) == 0 else { return nil }
  return (Int64(boot.tv_sec), Int32(boot.tv_usec))
}
private func attemptRecordURL() -> URL? {
  FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: appGroup)?
    .appendingPathComponent(attemptRecordName, isDirectory: false)
}

private func validatedRecord(attempt: UInt64, phase: String) -> AttemptRecord? {
  guard attempt > 0,
        let url = attemptRecordURL(),
        let data = try? Data(contentsOf: url, options: [.mappedIfSafe]),
        data.count <= 4_096,
        let record = try? JSONDecoder().decode(AttemptRecord.self, from: data),
        record.schema == "gpu-appraisal-attempt-v1",
        record.bridgeAttempt == attempt,
        record.hostPid > 0,
        record.phase == phase,
        let boot = currentBootIdentity(),
        record.bootSeconds == boot.0,
        record.bootMicroseconds == boot.1,
        record.deadlineUptimeMs > UInt64(ProcessInfo.processInfo.systemUptime * 1_000)
  else {
    return nil
  }
  if phase == "launched" && record.helperPid != Int32(getpid()) {
    return nil
  }
  if phase == "reserved" && record.helperPid != nil {
    return nil
  }
  return record
}

final class AppraisalState: @unchecked Sendable {
  static let shared = AppraisalState()
  private let lock = NSLock()
  private var helloAttempt: UInt64?
  private var appraisalClaimed = false

  func claimHello(_ attempt: UInt64) -> Bool {
    lock.lock()
    defer { lock.unlock() }
    guard helloAttempt == nil, !appraisalClaimed else { return false }
    helloAttempt = attempt
    return true
  }

  func claimAppraisal(_ attempt: UInt64) -> Bool {
    lock.lock()
    defer { lock.unlock() }
    guard helloAttempt == attempt, !appraisalClaimed else { return false }
    appraisalClaimed = true
    return true
  }

  func owns(_ attempt: UInt64) -> Bool {
    lock.lock()
    defer { lock.unlock() }
    return helloAttempt == attempt
  }
}

struct AppraisalHandler: XPCPeerHandler {
  private static let appraisalQueue = DispatchQueue(
    label: "app.solstone.journal.runtime.dev.gpu-appraisal.work",
    qos: .userInitiated
  )

  func handleIncomingRequest(_ incoming: XPCReceivedMessage) -> (any Encodable)? {
    do {
      let message = try incoming.decode(as: Request.self)
      switch message.op {
      case "hello":
        return hello(message)
      case "appraise":
        return incoming.handoffReply(to: Self.appraisalQueue) {
          incoming.reply(appraise(message))
        }
      case "cancel":
        return cancel(message)
      default:
        return refusal("invalid request", attempt: message.bridgeAttempt, exitCode: 1)
      }
    } catch {
      return refusal("invalid request", attempt: 0, exitCode: 1)
    }
  }

  func handleCancellation(error _: XPCRichError) {
    _exit(125)
  }

  private func hello(_ message: Request) -> Reply {
    guard validatedRecord(attempt: message.bridgeAttempt, phase: "reserved") != nil,
          AppraisalState.shared.claimHello(message.bridgeAttempt)
    else {
      return refusal("hello authority refused", attempt: message.bridgeAttempt, exitCode: 1)
    }
    return encoded([
      "bridge_attempt": message.bridgeAttempt,
      "pid": Int(getpid()),
      "hello": true,
    ])
  }

  private func appraise(_ message: Request) -> Reply {
    guard message.payload.utf8.count <= maximumRequestBytes,
          validatedRecord(attempt: message.bridgeAttempt, phase: "launched") != nil,
          AppraisalState.shared.claimAppraisal(message.bridgeAttempt)
    else {
      return refusal("appraisal authority refused", attempt: message.bridgeAttempt, exitCode: 1)
    }

    guard let payload = message.payload.data(using: .utf8),
          let request = try? JSONSerialization.jsonObject(with: payload) as? [String: Any],
          (request["bridge_attempt"] as? NSNumber)?.uint64Value == message.bridgeAttempt,
          let budgetNumber = request["budget_ms"] as? NSNumber,
          (1...60_000).contains(budgetNumber.uint64Value),
          let record = validatedRecord(attempt: message.bridgeAttempt, phase: "launched")
    else {
      return refusal("appraisal payload refused", attempt: message.bridgeAttempt, exitCode: 1)
    }

    let budget = budgetNumber.uint64Value
    let nowMs = UInt64(ProcessInfo.processInfo.systemUptime * 1_000)
    guard record.deadlineUptimeMs > nowMs else {
      return refusal("appraisal deadline expired", attempt: message.bridgeAttempt, exitCode: 124)
    }
    let remaining = min(budget, record.deadlineUptimeMs - nowMs)
    var initResult: UInt16 = 4
    if Thread.isMainThread {
      initResult = ios_gpu_appraisal_initialize(remaining)
    } else {
      DispatchQueue.main.sync {
        initResult = ios_gpu_appraisal_initialize(remaining)
      }
    }
    guard initResult == 0 else {
      return refusal("verifier initialization failed", attempt: message.bridgeAttempt, exitCode: 1)
    }

    let result = executeHelper(message.payload, attempt: message.bridgeAttempt)
    DispatchQueue.global().asyncAfter(deadline: .now() + 0.05) { _exit(0) }
    return Reply(json: result)
  }

  private func cancel(_ message: Request) -> Reply {
    guard message.bridgeAttempt > 0, AppraisalState.shared.owns(message.bridgeAttempt) else {
      return refusal("cancel authority refused", attempt: message.bridgeAttempt, exitCode: 1)
    }
    let reply = encoded([
      "bridge_attempt": message.bridgeAttempt,
      "pid": Int(getpid()),
      "cancelled": true,
    ])
    DispatchQueue.global(qos: .userInitiated).asyncAfter(deadline: .now() + 0.01) { _exit(125) }
    return reply
  }

  private func executeHelper(_ payload: String, attempt: UInt64) -> String {
    payload.withCString { cStr in
      guard let resPtr = ios_gpu_appraisal_execute(cStr) else {
        return encodedDictionary([
          "bridge_attempt": attempt,
          "pid": Int(getpid()),
          "returncode": 1,
          "result_code": 2,
          "result_message": "helper returned null",
          "claims": [],
          "detached_eat": [],
        ])
      }
      defer { ios_gpu_appraisal_free(resPtr) }
      let raw = String(cString: resPtr)
      guard var json = (try? JSONSerialization.jsonObject(with: Data(raw.utf8))) as? [String: Any]
      else {
        return encodedDictionary([
          "bridge_attempt": attempt,
          "pid": Int(getpid()),
          "returncode": 1,
          "result_code": 2,
          "result_message": "helper returned malformed JSON",
          "claims": [],
          "detached_eat": [],
        ])
      }
      json["bridge_attempt"] = attempt
      json["pid"] = Int(getpid())
      json["main_thread_init"] = true
      return encodedDictionary(json)
    }
  }

  private func refusal(_ message: String, attempt: UInt64, exitCode: Int32) -> Reply {
    let reply = encoded([
      "bridge_attempt": attempt,
      "pid": Int(getpid()),
      "returncode": 1,
      "result_code": 4,
      "result_message": message,
      "claims": [],
      "detached_eat": [],
    ])
    DispatchQueue.global().asyncAfter(deadline: .now() + 0.05) { _exit(exitCode) }
    return reply
  }

  private func encoded(_ value: [String: Any]) -> Reply {
    Reply(json: encodedDictionary(value))
  }

  private func encodedDictionary(_ value: [String: Any]) -> String {
    guard JSONSerialization.isValidJSONObject(value),
          let data = try? JSONSerialization.data(withJSONObject: value, options: [.sortedKeys])
    else {
      return "{\"returncode\":1,\"result_code\":2,\"result_message\":\"serialization failed\",\"claims\":[],\"detached_eat\":[]}"
    }
    return String(data: data, encoding: .utf8) ?? "{}"
  }
}

@main struct Appraisal: AppExtension {
  @AppExtensionPoint.Bind var boundExtensionPoint: AppExtensionPoint {
    #if JOURNAL_OWNER_APP
    AppExtensionPoint.Identifier(host: "app.solstone.journal.owner.dev", name: "gpu-appraisal")
    #else
    AppExtensionPoint.Identifier(host: "app.solstone.journal.runtime.dev", name: "gpu-appraisal")
    #endif
  }
  var configuration: some AppExtensionConfiguration {
    ConnectionHandler(onSessionRequest: { request in request.accept { _ in AppraisalHandler() } })
  }
}
