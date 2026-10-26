// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
import Foundation

/// A development launch selects one immutable, container-relative fixture.
struct OwnerFixture {
  let suite: String
  let choiceFileName: String

  enum Refusal: Error { case invalidArguments }

  static func parse(_ arguments: [String]) throws -> OwnerFixture {
    let options = arguments.indices.filter { arguments[$0].hasPrefix("--owner-fixture") }
    guard !options.isEmpty else {
      return OwnerFixture(suite: "owner", choiceFileName: "owner-run-choice.json")
    }
    guard options.count == 1, let index = options.first,
      arguments[index] == "--owner-fixture", index + 1 < arguments.count
    else { throw Refusal.invalidArguments }
    let name = arguments[index + 1]
    let bytes = Array(name.utf8)
    func alphanumeric(_ byte: UInt8) -> Bool {
      (97...122).contains(byte) || (48...57).contains(byte)
    }
    guard !bytes.isEmpty, bytes.count <= 48,
      bytes.first.map(alphanumeric) == true, bytes.last.map(alphanumeric) == true,
      bytes.allSatisfy({ alphanumeric($0) || $0 == 45 }),
      name != "owner", name != "development"
    else { throw Refusal.invalidArguments }
    return OwnerFixture(
      suite: "fixture-" + name, choiceFileName: "owner-fixture-" + name + "-run-choice.json")
  }
}
