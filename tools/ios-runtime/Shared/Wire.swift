// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
import Foundation

struct Request: Codable, Sendable {
  var op: String
  var journal: String = "development"
  var root: String = ""
  var count: Int = 128
  var seconds: Int = 0
  var revision: Int = 0
  var mode: String = "reopen"
  var payload: String = ""
  var storage: String = "private"
  var admissionGeneration: UInt64? = nil
}
struct ServingCommand: Codable { var id: String; var request: Request }
struct Reply: Codable, Sendable { var json: String }
func resources() -> [String: Any] {
  guard let p = runtime_resources() else { return ["error": "resource allocation failed"] }
  defer { runtime_resources_free(p) }
  return (try? JSONSerialization.jsonObject(with: Data(String(cString: p).utf8))) as? [String: Any]
    ?? ["error": "resource decoding failed"]
}
