// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
import Foundation

@main struct OwnerCreatorRecoveryTests {
  static func main() throws {
    func check(_ condition: @autoclosure () throws -> Bool) throws {
      let passed = try condition()
      precondition(passed)
    }
    typealias Recovery = OwnerCreatorRecovery
    let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
    try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: false)
    defer { try? FileManager.default.removeItem(at: directory) }
    let file = directory.appendingPathComponent("receiving.json")
    let fresh = try Recovery.load(file)
    try check(try fresh.heldIndex() == nil)
    try check(!FileManager.default.fileExists(atPath: file.path))

    let report: [String: Any] = ["scope": "local_redundant_creator_copies",
      "root_observation": "original-root", "ordinary_work_started": false,
      "replay_authorized": false, "remote_outcome": "unknown", "request_id": "observe-1",
      "native_epoch": UInt64(3), "refusal": NSNull(), "attempted": false,
      "confirmation_revision": "revision", "confirmation": ["root_observation": "original-root",
        "stages": [["stage": "copy"]]]]
    let review = try Recovery.review(report, id: "observe-1", epoch: 3)
    try check(review.count == 1)
    var changed = report
    changed["replay_authorized"] = true
    do { _ = try Recovery.review(changed, id: "observe-1", epoch: 3); assertionFailure() }
    catch { }
    do { _ = try Recovery.review(report, id: "observe-1", epoch: 5); assertionFailure() }
    catch { }

    let command = Recovery.command("apply", epoch: 3, review: review)
    var state = fresh
    state.attempts.append(.init(command: try Recovery.encode(command)))
    try state.save(file)
    state = try Recovery.load(file)
    try check(try state.heldIndex() == 0) // Relaunch cannot replay or Start.

    var result = report
    result["request_id"] = command["request_id"]
    result["attempted"] = true
    result["local_complete"] = false
    result["refusal"] = "creator_recovery_partial"
    state.attempts[0].result = try Recovery.encode(result)
    try state.save(file)
    state = try Recovery.load(file)
    try check(try state.heldIndex() == 0)

    let reconcile = try Recovery.reconcile(state.attempts[0], epoch: 7)
    try check(reconcile["action"] as? String == "reconcile")
    try check(reconcile["observation_epoch"] as? UInt64 == 3)
    try check(reconcile["admission_generation"] as? UInt64 == 7)
    try check(reconcile["request_id"] as? String != command["request_id"] as? String)
    result["request_id"] = reconcile["request_id"]
    result["native_epoch"] = UInt64(7)
    result["refusal"] = NSNull()
    result["current_local_state_reconciled"] = true
    state.attempts[0].reconciliation = try Recovery.encode(result)
    try state.save(file)
    state = try Recovery.load(file)
    try check(try state.heldIndex() == nil)
    // A current local result never rewrites the old partial outcome.
    try check(try Recovery.object(state.attempts[0].result!)["local_complete"] as? Bool == false)

    var malformed = state
    result["root_observation"] = "replacement-root"
    malformed.attempts[0].reconciliation = try Recovery.encode(result)
    do { try malformed.save(file); assertionFailure() } catch { }
    try check(try Recovery.load(file).heldIndex() == nil)
    try Data("not a receipt".utf8).write(to: file)
    do { _ = try Recovery.load(file); assertionFailure() } catch { }
    try FileManager.default.removeItem(at: file)
    try FileManager.default.createSymbolicLink(at: file,
      withDestinationURL: directory.appendingPathComponent("absent"))
    do { _ = try Recovery.load(file); assertionFailure() } catch { }
    print("OwnerCreatorRecovery host receiving regression PASS")
  }
}
