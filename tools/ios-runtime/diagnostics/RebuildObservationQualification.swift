// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
import CryptoKit
import Darwin
import Foundation

@MainActor private final class Evidence {
  var rows: [[String: Any]] = []
  private var pending: (UUID, ([String: Any]) -> Bool,
    CheckedContinuation<[String: Any], Error>)?
  func receive(_ row: [String: Any]) {
    rows.append(row)
    if let item = pending, item.1(row) {
      pending = nil
      item.2.resume(returning: row)
    }
  }
  func next(_ predicate: @escaping ([String: Any]) -> Bool) async throws -> [String: Any] {
    if let row = rows.last(where: predicate) { return row }
    return try await withCheckedThrowingContinuation { continuation in
      precondition(pending == nil)
      let identifier = UUID()
      pending = (identifier, predicate, continuation)
      DispatchQueue.main.asyncAfter(deadline: .now() + 5) { [weak self] in
        MainActor.assumeIsolated {
          guard let item = self?.pending, item.0 == identifier else { return }
          self?.pending = nil
          item.2.resume(throwing: NSError(domain: "observation-timeout", code: 1))
        }
      }
    }
  }
}

private struct Fixture {
  let base: URL
  let root: URL
  let health: URL
  let intent: URL
  let identity: String
  init() throws {
    base = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
    root = base.appendingPathComponent("journal")
    health = root.appendingPathComponent("health")
    intent = health.appendingPathComponent("work-intent.json")
    try FileManager.default.createDirectory(at: health, withIntermediateDirectories: true)
    try FileManager.default.createDirectory(
      at: root.appendingPathComponent("talents/entities--entity_suggest"), withIntermediateDirectories: true)
    var metadata = stat()
    guard lstat(root.path, &metadata) == 0 else { throw NSError(domain: "fixture-stat", code: 1) }
    identity = String(format: "native-root-v1:unix:%016llx:%016llx",
                      UInt64(metadata.st_dev), UInt64(metadata.st_ino))
  }
  func write(_ marker: String) throws {
    let data = try JSONSerialization.data(withJSONObject: ["version": 1, "days": ["marker": marker]])
    try data.write(to: intent, options: .atomic)
  }
  func remove() { try? FileManager.default.removeItem(at: base) }
}

@main struct RebuildObservationQualification {
  @MainActor static func main() async throws {
    var passed: [String] = []
    var stopOrdering: [String] = []
    var verifiedClosedAtReturn: [String: Int] = [:]
    func directoryDescriptors(_ fixture: Fixture) throws -> Set<Int32> {
      let directories = [fixture.root, fixture.health,
        fixture.root.appendingPathComponent("talents/entities--entity_suggest")]
      let identities = try directories.map { path -> String in
        var metadata = stat()
        guard lstat(path.path, &metadata) == 0 else {
          throw NSError(domain: "qualification-directory-stat", code: 1)
        }
        return "\(metadata.st_dev):\(metadata.st_ino)"
      }
      return Set((0..<min(getdtablesize(), 4096)).filter { fd in
        var metadata = stat()
        return fstat(fd, &metadata) == 0
          && identities.contains("\(metadata.st_dev):\(metadata.st_ino)")
      })
    }
    func marker(_ row: [String: Any]) -> String? {
      guard let files = row["files"] as? [[String: Any]],
        let file = files.first(where: { $0["path"] as? String == "health/work-intent.json" }),
        let raw = file["base64"] as? String, let data = Data(base64Encoded: raw),
        let value = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
        let days = value["days"] as? [String: Any] else { return nil }
      precondition(file["bytes"] as? Int == data.count)
      precondition(file["sha256"] as? String == SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined())
      return days["marker"] as? String
    }
    func observer(_ fixture: Fixture, _ evidence: Evidence) throws -> RebuildObservation {
      try RebuildObservation(root: fixture.root.path, identity: fixture.identity, seconds: 30) {
        evidence.receive($0)
      }
    }
    func initialRefusal(_ name: String, _ prepare: (Fixture) throws -> Void) async throws {
      let fixture = try Fixture()
      defer { fixture.remove() }
      try prepare(fixture)
      let evidence = Evidence()
      let value = try observer(fixture, evidence)
      var refused = false
      do { try value.start() } catch { refused = true }
      await value.stop()
      precondition(refused && !evidence.rows.contains(where: { $0["kind"] as? String == "ready" }))
      passed.append(name)
    }

    do {
      let fixture = try Fixture()
      defer { fixture.remove() }
      try fixture.write("baseline")
      let evidence = Evidence()
      let value = try observer(fixture, evidence)
      let priorDescriptors = try directoryDescriptors(fixture)
      try value.start()
      let watchedDescriptors = try directoryDescriptors(fixture).subtracting(priorDescriptors)
      precondition(watchedDescriptors.count == 3)
      _ = try await evidence.next { $0["kind"] as? String == "ready" }
      precondition(evidence.rows.contains { marker($0) == "baseline" })
      try fixture.write("atomic")
      let changed = try await evidence.next { marker($0) == "atomic" }
      precondition(changed["atomic_multi_file_snapshot"] as? Bool == false)
      precondition(changed["eligibility_or_signal_authority"] as? Bool == false)
      try fixture.write("intermediate")
      try fixture.write("completed")
      _ = try await evidence.next { marker($0) == "completed" }
      // Notifications may coalesce. Only the validated final bytes are required.
      let active = fixture.root.appendingPathComponent("talents/entities--entity_suggest/qualification_active.jsonl")
      let activeData = Data("{\"name\":\"entities:entity_suggest\",\"day\":\"20261016\",\"facet\":\"home\",\"use_id\":\"qualification\"}\n".utf8)
      try activeData.write(to: active, options: .atomic)
      let claimed = try await evidence.next { row in
        (row["files"] as? [[String: Any]])?.contains {
          $0["path"] as? String == "talents/entities--entity_suggest/qualification_active.jsonl"
            && ($0["base64"] as? String).flatMap { Data(base64Encoded: $0) } == activeData
        } == true
      }
      let claimedSequence = claimed["sequence"] as! Int
      try FileManager.default.moveItem(at: active, to: active.deletingLastPathComponent().appendingPathComponent("qualification.jsonl"))
      _ = try await evidence.next { row in
        (row["sequence"] as? Int ?? 0) > claimedSequence &&
          !(row["files"] as? [[String: Any]] ?? []).contains {
            ($0["path"] as? String)?.hasSuffix("_active.jsonl") == true
          }
      }
      let stage = active.deletingLastPathComponent().appendingPathComponent("staged-claim")
      let linked = active.deletingLastPathComponent().appendingPathComponent("linked_active.jsonl")
      try activeData.write(to: stage)
      try FileManager.default.linkItem(at: stage, to: linked)
      _ = try await evidence.next { $0["kind"] as? String == "sample.refused" }
      try FileManager.default.removeItem(at: stage)
      _ = try await evidence.next { row in
        (row["files"] as? [[String: Any]])?.contains {
          $0["path"] as? String == "talents/entities--entity_suggest/linked_active.jsonl"
        } == true
      }
      func stopAndVerify(_ name: String) async {
        stopOrdering.append(name + ".enter")
        await value.stop()
        // No opens between return and these actual descriptor checks.
        for fd in watchedDescriptors {
          var metadata = stat()
          errno = 0
          precondition(fstat(fd, &metadata) == -1 && errno == EBADF)
        }
        verifiedClosedAtReturn[name] = watchedDescriptors.count
        stopOrdering.append(name + ".return")
      }
      let first = Task { @MainActor in await stopAndVerify("first") }
      let second = Task { @MainActor in await stopAndVerify("second") }
      await first.value
      await second.value
      let overlap = stopOrdering.prefix(2).allSatisfy { $0.hasSuffix(".enter") }
      precondition(overlap, "Both stops closed descriptors, but no concurrent overlap was observed")
      let count = evidence.rows.count
      try fixture.write("after-stop")
      try await Task.sleep(for: .milliseconds(100))
      precondition(evidence.rows.count == count)
      passed += ["bounded initial read and ready", "real atomic replacement",
                 "coalesced writes require final state only", "actual Cortex claim publication and rename events",
                 "exclusive-publish link race refuses sample until later notification",
                 "overlapping Stop callers each observe all watcher descriptors closed",
                 "Stop prevents late observations"]
    }
    do {
      let fixture = try Fixture()
      defer { fixture.remove() }
      let evidence = Evidence()
      let value = try observer(fixture, evidence)
      try value.start()
      let initial = try await evidence.next { $0["kind"] as? String == "snapshot" }
      let files = initial["files"] as! [[String: Any]]
      precondition(files.contains { $0["path"] as? String == "health/work-intent.json" && $0["absent"] as? Bool == true })
      precondition(initial["eligibility_or_signal_authority"] as? Bool == false)
      await value.stop()
      passed.append("absent file remains explicit without eligibility")
    }
    try await initialRefusal("malformed work intent") { fixture in
      try Data("{".utf8).write(to: fixture.intent)
    }
    try await initialRefusal("symlink refused without following") { fixture in
      let outside = fixture.base.appendingPathComponent("outside.json")
      try Data("{}".utf8).write(to: outside)
      try FileManager.default.createSymbolicLink(at: fixture.intent, withDestinationURL: outside)
    }
    try await initialRefusal("FIFO refuses without blocking") { fixture in
      precondition(mkfifo(fixture.intent.path, 0o600) == 0)
    }
    try await initialRefusal("multiple hard links refused") { fixture in
      try fixture.write("linked")
      try FileManager.default.linkItem(at: fixture.intent, to: fixture.base.appendingPathComponent("linked.json"))
    }
    try await initialRefusal("oversized file refused") { fixture in
      try Data(repeating: 32, count: 1024 * 1024 + 1).write(to: fixture.intent)
    }
    for replacement in ["health", "root"] {
      let fixture = try Fixture()
      defer { fixture.remove() }
      try fixture.write("before-replacement")
      let evidence = Evidence()
      let value = try observer(fixture, evidence)
      try value.start()
      let destination = fixture.base.appendingPathComponent("old-" + replacement)
      let source = replacement == "health" ? fixture.health : fixture.root
      try FileManager.default.moveItem(at: source, to: destination)
      try FileManager.default.createDirectory(at: fixture.health, withIntermediateDirectories: true)
      try fixture.write("replacement")
      _ = try await evidence.next { $0["kind"] as? String == "refused" }
      precondition(!evidence.rows.contains { marker($0) == "replacement" })
      await value.stop()
      passed.append(replacement + " identity replacement refuses")
    }
    let output: [String: Any] = ["passed": true, "controls": passed, "device_actions": 0,
      "stop_ordering": stopOrdering, "watcher_descriptors_closed_at_each_return": verifiedClosedAtReturn,
      "boundary": "Actual observer source and Darwin file events; no phone or recovery acceptance."]
    print(String(data: try JSONSerialization.data(withJSONObject: output, options: [.sortedKeys]), encoding: .utf8)!)
  }
}
