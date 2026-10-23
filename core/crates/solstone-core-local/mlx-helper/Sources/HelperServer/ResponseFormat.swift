// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
import Foundation

enum ResponseFormat {
    private struct Header: Decodable {
        let type: String
        let json_schema: Schema?
        struct Schema: Decodable { let strict: Bool? }
    }

    static func schemaText(
        decoder: any Decoder, container: KeyedDecodingContainer<ChatRequest.CodingKeys>
    ) throws -> String? {
        guard container.contains(.response_format) else { return nil }
        let header = try container.decode(Header.self, forKey: .response_format)
        switch header.type {
        case "json_object": return #"{"type":"object"}"#
        case "json_schema":
            guard header.json_schema?.strict == true,
                  let body = decoder.userInfo[ChatRequest.requestBodyKey] as? Data else {
                throw RequestError("json_schema requires strict: true and an original request body")
            }
            var raw = RawJSON(bytes: Array(body))
            let schema = try raw.member(["response_format", "json_schema", "schema"])
            guard let schema, schema.first == 123 else {
                throw RequestError("json_schema.schema must be an object")
            }
            return String(decoding: schema, as: UTF8.self)
        default: throw RequestError("unsupported response_format type")
        }
    }
}

/// JSONDecoder validates the document first. This scanner only finds the byte
/// range of a member; it never reconstructs a schema from unordered dictionaries.
private struct RawJSON {
    let bytes: [UInt8]
    var index = 0

    mutating func whitespace() {
        while index < bytes.count && [9, 10, 13, 32].contains(bytes[index]) { index += 1 }
    }

    mutating func value() {
        whitespace()
        if bytes[index] == 34 {
            index += 1
            while bytes[index] != 34 {
                if bytes[index] == 92 { index += 1 }
                index += 1
            }
            index += 1
        } else if bytes[index] == 123 || bytes[index] == 91 {
            let end: UInt8 = bytes[index] == 123 ? 125 : 93
            index += 1
            whitespace()
            while bytes[index] != end {
                if bytes[index] == 44 || bytes[index] == 58 { index += 1 }
                else { value() }
                whitespace()
            }
            index += 1
        } else {
            while index < bytes.count && ![9, 10, 13, 32, 44, 93, 125].contains(bytes[index]) {
                index += 1
            }
        }
    }

    mutating func member(_ keys: [String]) throws -> ArraySlice<UInt8>? {
        whitespace()
        guard bytes[index] == 123, let wanted = keys.first else { return nil }
        index += 1
        var found: ArraySlice<UInt8>?
        var seen = false
        whitespace()
        while bytes[index] != 125 {
            let keyStart = index
            value()
            let key = try JSONDecoder().decode(String.self, from: Data(bytes[keyStart..<index]))
            whitespace()
            index += 1 // colon
            whitespace()
            let start = index
            if key == wanted {
                guard !seen else { throw RequestError("duplicate response_format member") }
                seen = true
                if keys.count == 1 {
                    value()
                    found = bytes[start..<index]
                } else {
                    found = try member(Array(keys.dropFirst()))
                    index = start
                    value()
                }
            } else { value() }
            whitespace()
            if bytes[index] == 44 { index += 1; whitespace() }
        }
        index += 1
        return found
    }
}
