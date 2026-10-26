// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
import Foundation

// Both processes derive storage from their signed container entitlement.
// Neither accepts an external path as journal or payload authority.
enum Engine {
  static let queue = DispatchQueue(label: "journal-runtime-work")

  // PDFium is installed on the containing app. Inside the worker appex,
  // Bundle.main is <app>/Extensions/<name>.appex.
  private static func pdfiumLibraryPath() -> String {
    var container = Bundle.main.bundleURL
    if container.pathExtension == "appex" {
      container.deleteLastPathComponent()
      container.deleteLastPathComponent()
    }
    return container
      .appendingPathComponent("Frameworks", isDirectory: true)
      .appendingPathComponent("PDFium.framework", isDirectory: true)
      .appendingPathComponent("PDFium", isDirectory: false)
      .path
  }

  static func execute(_ message: Request) -> Reply {
    do {
      guard !message.journal.isEmpty, message.journal.count <= 64,
        message.journal.allSatisfy({ $0.isASCII && ($0.isLetter || $0.isNumber || $0 == "-") })
      else { throw NSError(domain: "invalid-journal-name", code: 1) }
      // This container is the journal's authority. Host-supplied paths never select storage.
      guard message.op == "probe" || message.root.isEmpty else {
        throw NSError(domain: "host-path-is-not-authority", code: 1)
      }
      let base: URL
      switch message.storage {
      case "private": base = URL(fileURLWithPath: NSHomeDirectory())
      case "serving":
        guard let group = FileManager.default.containerURL(
          forSecurityApplicationGroupIdentifier: RuntimeIdentity.group) else {
          throw NSError(domain: "serving-group-unavailable", code: 1)
        }
        base = group
      default: throw NSError(domain: "invalid-storage-authority", code: 1)
      }
      let root = base.appendingPathComponent("Documents/journal-" + message.journal).path
      var out: [String: Any] = [
        "op": message.op, "home": NSHomeDirectory(), "root": root, "before": resources(),
      ]
      if message.op == "probe" {
        out["host_file_readable"] = FileManager.default.isReadableFile(atPath: message.root)
        out["pid"] = getpid()
      } else if message.op == "export" {
        // Validate the retained authority before reading a closed-connection snapshot.
        let validation = String(
          data: try JSONEncoder().encode(
            Request(op: "prepare-export", journal: message.journal, root: root)), encoding: .utf8)!
        guard let pointer = validation.withCString({ journal_runtime($0) }) else {
          throw NSError(domain: "native-null", code: 1)
        }
        defer { journal_runtime_free(pointer) }
        let checked =
          try JSONSerialization.jsonObject(with: Data(String(cString: pointer).utf8))
          as! [String: Any]
        guard let result = checked["result"] as? [String: Any], result["ok"] as? Bool == true else {
          throw NSError(domain: "root-validation-failed", code: 1)
        }
        // Work queue serializes this snapshot with all writes.
        for file in [
          "indexer/journal.sqlite", "indexer/journal.sqlite-wal", "checkpoint.bin", "committed.bin",
        ] {
          if FileManager.default.fileExists(atPath: root + "/" + file) {
            out[file] = try Data(contentsOf: URL(fileURLWithPath: root + "/" + file))
              .base64EncodedString()
          }
        }
        // Only synthetic embedding sidecars are exported here; original media is hashed in place.
        let chronicle = URL(fileURLWithPath: root + "/chronicle")
        if let entries = FileManager.default.enumerator(
          at: chronicle, includingPropertiesForKeys: [.isRegularFileKey], options: []) {
          for case let file as URL in entries where file.pathExtension == "npz" {
            guard try file.resourceValues(forKeys: [.isRegularFileKey]).isRegularFile == true else {
              throw NSError(domain: "unexpected-sidecar-type", code: 1)
            }
            out[String(file.path.dropFirst(root.count + 1))] = try Data(contentsOf: file)
              .base64EncodedString()
          }
        }
      } else {
        guard message.root.isEmpty else {
          throw NSError(domain: "host-path-is-not-authority", code: 1)
        }
        var request = message
        if request.op == "open" || request.op == "serve-custody" { request.root = root }
        if request.op == "serve-start" {
          guard let payload = Bundle.main.url(forResource: "JournalPayload", withExtension: nil) else {
            throw NSError(domain: "packaged-payload-missing", code: 1)
          }
          request.payload = payload.path
          request.pdfiumLibrary = pdfiumLibraryPath()
        }
        if request.op == "audio-enable" {
          guard let models = Bundle.main.url(forResource: "AudioModels", withExtension: nil) else {
            throw NSError(domain: "packaged-audio-models-missing", code: 1)
          }
          request.payload = models.path
        }
        let input = String(data: try JSONEncoder().encode(request), encoding: .utf8)!
        guard let p = input.withCString({ journal_runtime($0) }) else {
          throw NSError(domain: "native-null", code: 1)
        }
        defer { journal_runtime_free(p) }
        out["journal"] = try JSONSerialization.jsonObject(with: Data(String(cString: p).utf8))
        if request.op == "serve-custody" {
          guard let journal = out["journal"] as? [String: Any],
            let result = journal["result"] as? [String: Any], result["ok"] as? Bool == true
          else { throw NSError(domain: "root-validation-failed", code: 1) }
          var files: [[String: Any]] = []
          for name in ["", "config/journal.json", "link/ca/private.pem"] {
            let url = URL(fileURLWithPath: root).appendingPathComponent(name)
            let attributes = try FileManager.default.attributesOfItem(atPath: url.path)
            let resources = try url.resourceValues(forKeys: [.isExcludedFromBackupKey])
            files.append([
              "path": name, "excluded_from_backup": resources.isExcludedFromBackup.map { $0 as Any } ?? NSNull(),
              "protection": String(describing: attributes[.protectionKey] ?? "unknown"),
              "posix_permissions": attributes[.posixPermissions] ?? NSNull(),
            ])
          }
          out["custody"] = files
        }
      }
      out["after"] = resources()
      return Reply(
        json: String(
          data: try JSONSerialization.data(withJSONObject: out, options: [.sortedKeys]),
          encoding: .utf8)!)
    } catch {
      return Reply(
        json: String(
          data: try! JSONSerialization.data(withJSONObject: ["error": String(describing: error)]),
          encoding: .utf8)!)
    }
  }
}
