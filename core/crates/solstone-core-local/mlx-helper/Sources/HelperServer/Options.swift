// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
import Foundation

public struct Options: Sendable {
    public var model = ""
    public var host = "127.0.0.1"
    public var port = 8080
    public var alias = "local/qwen3.5-4b"
    public var context = 16384
    public var cacheLimitMB = 512

    public init(arguments: [String]) throws {
        var i = 0
        while i < arguments.count {
            let key = arguments[i]
            guard i + 1 < arguments.count else { throw RequestError("missing value for \(key)") }
            let value = arguments[i + 1]
            switch key {
            case "--model": model = value
            case "--host": host = value
            case "--port":
                guard let n = Int(value), (1...65535).contains(n) else { throw RequestError("invalid port") }
                port = n
            case "--alias": alias = value
            case "--ctx":
                guard let n = Int(value), n > 0 else { throw RequestError("ctx must be positive") }
                context = n
            case "--cache-limit-mb":
                guard let n = Int(value), n > 0, n <= Int.max / 1_048_576 else {
                    throw RequestError("cache-limit-mb must be positive and fit in bytes")
                }
                cacheLimitMB = n
            default: throw RequestError("unknown argument: \(key)")
            }
            i += 2
        }
        // Accept literal loopback addresses only; no DNS resolution can widen
        // the bind address between validation and server startup.
        guard host == "127.0.0.1" || host == "::1" else {
            throw RequestError("host must be 127.0.0.1 or ::1")
        }
        guard !model.isEmpty else { throw RequestError("--model <local-directory> is required") }
    }
}
