// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
import Foundation
import Darwin

/// Host receiving records only. Rust re-proves every local effect; saved data
/// never grants filesystem authority or authorizes replay of an Apply.
struct OwnerCreatorRecovery: Codable {
  struct Attempt: Codable {
    let command: Data
    var result: Data?
    var reconciliation: Data?

    func held() throws -> Bool {
      let request = try OwnerCreatorRecovery.object(command)
      guard request["op"] as? String == "creator-recovery",
        request["action"] as? String == "apply",
        let id = request["request_id"] as? String, !id.isEmpty,
        let root = request["root_observation"] as? String, !root.isEmpty,
        let epoch = request["admission_generation"] as? UInt64, epoch & 1 == 1
      else { throw Refusal.invalidRecord }
      if let reconciliation {
        let report = try OwnerCreatorRecovery.object(reconciliation)
        try OwnerCreatorRecovery.validate(report, root: root)
        if report["current_local_state_reconciled"] as? Bool == true,
          report["refusal"] is NSNull,
          let currentEpoch = report["native_epoch"] as? UInt64, currentEpoch & 1 == 1,
          let currentId = report["request_id"] as? String, !currentId.isEmpty { return false }
      }
      guard let result else { return true }
      let report = try OwnerCreatorRecovery.object(result)
      try OwnerCreatorRecovery.validate(report, root: root)
      guard report["request_id"] as? String == id,
        report["native_epoch"] as? UInt64 == epoch,
        let attempted = report["attempted"] as? Bool
      else { throw Refusal.invalidRecord }
      return attempted && !(report["local_complete"] as? Bool == true && report["refusal"] is NSNull)
    }
  }

  struct Review {
    let id: String
    let epoch: UInt64
    let root: String
    let confirmation: [String: Any]
    let revision: String
    let count: Int
  }

  enum Refusal: Error { case invalidRecord, reviewUnavailable }
  var version = 1
  var attempts: [Attempt] = []
  static let maxBytes = 8 * 1024 * 1024

  static func object(_ data: Data) throws -> [String: Any] {
    guard data.count <= 256 * 1024,
      let object = try JSONSerialization.jsonObject(with: data) as? [String: Any]
    else { throw Refusal.invalidRecord }
    return object
  }
  static func encode(_ object: [String: Any]) throws -> Data {
    let data = try JSONSerialization.data(withJSONObject: object, options: [.sortedKeys])
    guard data.count <= 256 * 1024 else { throw Refusal.invalidRecord }
    return data
  }
  static func validate(_ report: [String: Any], root: String) throws {
    guard report["scope"] as? String == "local_redundant_creator_copies",
      report["root_observation"] as? String == root,
      report["ordinary_work_started"] as? Bool == false,
      report["replay_authorized"] as? Bool == false,
      report["remote_outcome"] as? String == "unknown"
    else { throw Refusal.invalidRecord }
  }
  static func review(_ report: [String: Any], id: String, epoch: UInt64) throws -> Review {
    guard let root = report["root_observation"] as? String,
      report["request_id"] as? String == id,
      report["native_epoch"] as? UInt64 == epoch, epoch & 1 == 1,
      report["refusal"] is NSNull,
      let confirmation = report["confirmation"] as? [String: Any],
      confirmation["root_observation"] as? String == root,
      let stages = confirmation["stages"] as? [[String: Any]], stages.count <= 64,
      let revision = report["confirmation_revision"] as? String
    else { throw Refusal.reviewUnavailable }
    try validate(report, root: root)
    return Review(id: id, epoch: epoch, root: root, confirmation: confirmation,
      revision: revision, count: stages.count)
  }
  static func command(_ action: String, epoch: UInt64, review: Review? = nil) -> [String: Any] {
    var command: [String: Any] = ["op": "creator-recovery", "action": action,
      "admission_generation": epoch, "request_id": UUID().uuidString.lowercased()]
    if let review {
      command["root_observation"] = review.root
      command["observation_request_id"] = review.id
      command["observation_epoch"] = review.epoch
      command["confirmation"] = review.confirmation
      command["confirmation_revision"] = review.revision
    }
    return command
  }
  static func reconcile(_ attempt: Attempt, epoch: UInt64) throws -> [String: Any] {
    var command = try object(attempt.command)
    command["action"] = "reconcile"
    command["request_id"] = UUID().uuidString.lowercased()
    command["admission_generation"] = epoch
    return command
  }
  func heldIndex() throws -> Int? {
    for index in attempts.indices where try attempts[index].held() { return index }
    return nil
  }
  static func load(_ url: URL) throws -> OwnerCreatorRecovery {
    let file = Darwin.open(url.path, O_RDONLY | O_CLOEXEC | O_NOFOLLOW | O_NONBLOCK)
    guard file >= 0 else {
      if errno == ENOENT { return OwnerCreatorRecovery() }
      throw Refusal.invalidRecord
    }
    var metadata = stat()
    guard fstat(file, &metadata) == 0, metadata.st_mode & S_IFMT == S_IFREG else {
      close(file)
      throw Refusal.invalidRecord
    }
    let handle = FileHandle(fileDescriptor: file, closeOnDealloc: true)
    defer { try? handle.close() }
    let data = try handle.read(upToCount: maxBytes + 1) ?? Data()
    guard data.count <= maxBytes else { throw Refusal.invalidRecord }
    let state = try JSONDecoder().decode(Self.self, from: data)
    guard state.version == 1, state.attempts.count <= 64 else { throw Refusal.invalidRecord }
    for attempt in state.attempts { _ = try attempt.held() }
    return state
  }
  func save(_ url: URL) throws {
    guard attempts.count <= 64 else { throw Refusal.invalidRecord }
    for attempt in attempts { _ = try attempt.held() }
    let data = try JSONEncoder().encode(self)
    guard data.count <= Self.maxBytes else { throw Refusal.invalidRecord }
    try data.write(to: url, options: [.atomic])
    let file = Darwin.open(url.path, O_RDONLY | O_CLOEXEC)
    guard file >= 0 else { throw CocoaError(.fileWriteUnknown) }
    defer { close(file) }
    guard fsync(file) == 0 else { throw CocoaError(.fileWriteUnknown) }
    let directory = Darwin.open(url.deletingLastPathComponent().path, O_RDONLY | O_CLOEXEC)
    guard directory >= 0 else { throw CocoaError(.fileWriteUnknown) }
    defer { close(directory) }
    guard fsync(directory) == 0 else { throw CocoaError(.fileWriteUnknown) }
  }
}
