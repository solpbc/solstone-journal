// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
import Foundation

@main enum OwnerFixtureQualification {
  static func main() throws {
    let original = try OwnerFixture.parse(["app"])
    precondition(original.suite == "owner")
    precondition(original.choiceFileName == "owner-run-choice.json")
    let source = try OwnerFixture.parse(["app", "--owner-fixture", "storage-source"])
    let restore = try OwnerFixture.parse(["app", "--owner-fixture", "storage-restore"])
    precondition(source.suite != restore.suite && source.suite != original.suite)
    precondition(source.choiceFileName != restore.choiceFileName)
    precondition(source.choiceFileName != original.choiceFileName)
    let reopened = try OwnerFixture.parse(["app", "--owner-fixture", "storage-source"])
    precondition(reopened.suite == source.suite && reopened.choiceFileName == source.choiceFileName)
    for arguments in [
      ["--owner-fixture"], ["--owner-fixture="], ["--owner-fixtures", "other"],
      ["--owner-fixture", "a", "--owner-fixture", "b"],
    ] {
      do {
        _ = try OwnerFixture.parse(["app"] + arguments)
        preconditionFailure("malformed fixture arguments were accepted")
      } catch OwnerFixture.Refusal.invalidArguments {}
    }
    for name in ["", "owner", "OWNER", "Owner", "development", "Storage-source", ".", "..",
      "../other", "a/b", "a\\b", "a b", "é", "--serve", "-source", "source-",
      "a\nother", String(repeating: "a", count: 49)] {
      do {
        _ = try OwnerFixture.parse(["app", "--owner-fixture", name])
        preconditionFailure("invalid fixture name was accepted")
      } catch OwnerFixture.Refusal.invalidArguments {}
    }
    print("owner fixture selection preserves default custody and isolates valid suites")
  }
}
