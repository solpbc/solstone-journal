// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import Darwin
import Foundation

public enum JournalLocalEndpointError: LocalizedError {
    case unavailable
    public var errorDescription: String? { "your journal isn't ready for the journal app yet. try again shortly." }
}

public enum JournalDriveCheck {
    /// Whether this drive lacks the two operations journal saves are built on: an exclusive rename and a
    /// hard link. It asks the drive by trying both in `root`, or in the closest folder that exists yet.
    /// The link only counts on a local drive: that is where it has been measured to mean the journal
    /// can't run. Anything but a refusal of the operation leaves the answer to the journal's own start.
    public static func cannotRunJournal(root: URL) -> Bool {
        var directory = root.standardizedFileURL
        var isDirectory: ObjCBool = false
        while !(FileManager.default.fileExists(atPath: directory.path, isDirectory: &isDirectory) && isDirectory.boolValue) {
            guard directory.path != "/" else { return false }
            directory.deleteLastPathComponent()
        }
        let unique = ".solstone-drive-check-\(getpid())"
        let from = directory.appendingPathComponent(unique + "-a")
        let to = directory.appendingPathComponent(unique + "-b")
        let linked = directory.appendingPathComponent(unique + "-c")
        let fd = open(from.path, O_CREAT | O_EXCL | O_WRONLY | O_NOFOLLOW, S_IRUSR | S_IWUSR)
        guard fd >= 0 else { return false }
        close(fd)
        defer {
            unlink(from.path)
            unlink(to.path)
            unlink(linked.path)
        }
        if renamex_np(from.path, to.path, UInt32(RENAME_EXCL)) == -1 {
            return errno == ENOTSUP
        }
        var volume = statfs()
        guard statfs(directory.path, &volume) == 0, volume.f_flags & UInt32(MNT_LOCAL) != 0 else { return false }
        return link(to.path, linked.path) == -1 && errno == ENOTSUP
    }
}

public enum JournalLocalEndpoint {
    public static func socketPath(environment: [String: String] = ProcessInfo.processInfo.environment) throws -> String? {
        if let override = environment["SOLSTONE_JOURNAL_SOCKET"] {
            if override.isEmpty { return nil }
            return try checkedPath(override)
        }
        let count = confstr(_CS_DARWIN_USER_TEMP_DIR, nil, 0)
        guard count > 0 else { throw JournalLocalEndpointError.unavailable }
        var bytes = [CChar](repeating: 0, count: count)
        guard confstr(_CS_DARWIN_USER_TEMP_DIR, &bytes, count) == count else { throw JournalLocalEndpointError.unavailable }
        let directory = String(decoding: bytes.dropLast().map { UInt8(bitPattern: $0) }, as: UTF8.self)
        return try checkedPath(URL(fileURLWithPath: directory).appendingPathComponent("solstone/journal.sock").path)
    }

    private static func checkedPath(_ path: String) throws -> String {
        guard path.hasPrefix("/"), !path.utf8.contains(0), path.utf8.count < 104 else {
            throw JournalLocalEndpointError.unavailable
        }
        return path
    }

}
