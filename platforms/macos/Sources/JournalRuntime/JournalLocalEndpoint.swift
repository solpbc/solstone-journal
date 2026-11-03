// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import Darwin
import Foundation

public enum JournalLocalEndpointError: LocalizedError {
    case unavailable
    public var errorDescription: String? { "your journal isn't ready for the journal app yet. try again shortly." }
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
