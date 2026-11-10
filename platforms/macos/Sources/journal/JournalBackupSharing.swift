// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import Foundation

/// Where a journal backs up, read from its own settings. A journal restored from another one's
/// backup starts with that journal's backup settings, so two journals on this mac can share one
/// backup. This only reads; the journal writes its settings.
enum JournalBackupPlace: Equatable, Sendable {
    /// sol pbc's storage, named by the storage prefix the journal is bound to.
    case hosted(prefix: String)
    /// The owner's own storage, named by its repository.
    case own(repository: String)

    static func read(journalRoot: URL) -> JournalBackupPlace? {
        let configURL = journalRoot.appendingPathComponent("config/journal.json")
        guard let backup = jsonObject(at: configURL)?["backup"] as? [String: Any],
              backup["enabled"] as? Bool == true else { return nil }
        if backup["mode"] as? String == "operated" {
            let bindingURL = journalRoot.appendingPathComponent("backup/hosted/binding.json")
            guard let prefix = jsonObject(at: bindingURL)?["prefix"] as? String,
                  !prefix.isEmpty else { return nil }
            return .hosted(prefix: prefix)
        }
        guard let destination = backup["destination"] as? [String: Any],
              let repository = destination["repository"] as? String,
              !repository.isEmpty else { return nil }
        return .own(repository: repository)
    }

    private static func jsonObject(at url: URL) -> [String: Any]? {
        guard let data = try? Data(contentsOf: url, options: [.uncached]),
              data.count <= 1 << 20 else { return nil }
        return (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
    }
}

/// The instance a journal's link identity names. Two journals with one instance are known by the
/// same mark, and this mac's capture reaches whichever of them is running.
enum JournalInstance {
    static func read(journalRoot: URL) -> String? {
        let url = journalRoot.appendingPathComponent("link/ca/state.json")
        guard let data = try? Data(contentsOf: url, options: [.uncached]), data.count <= 1 << 20,
              let object = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
              let id = object["instance_id"] as? String, !id.isEmpty else { return nil }
        return id
    }

    /// Another journal here with the same instance as `target`.
    static func sameMark(
        target: URL,
        among others: [URL],
        read: (URL) -> String? = { JournalInstance.read(journalRoot: $0) }
    ) -> URL? {
        guard let id = read(target) else { return nil }
        let targetPath = target.standardizedFileURL.path
        return others.first { $0.standardizedFileURL.path != targetPath && read($0) == id }
    }
}

/// A journal this app knows that backs up to the same place as the one being switched to.
struct JournalBackupShare: Equatable, Sendable {
    var place: JournalBackupPlace
    var other: URL

    static func find(
        target: URL,
        among others: [URL],
        read: (URL) -> JournalBackupPlace? = { JournalBackupPlace.read(journalRoot: $0) }
    ) -> JournalBackupShare? {
        guard let place = read(target) else { return nil }
        let targetPath = target.standardizedFileURL.path
        for other in others where other.standardizedFileURL.path != targetPath {
            if read(other) == place { return JournalBackupShare(place: place, other: other) }
        }
        return nil
    }
}
