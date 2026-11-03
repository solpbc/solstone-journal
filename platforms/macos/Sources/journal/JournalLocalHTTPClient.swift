// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import Darwin
import Foundation
import Network
import JournalRuntime

/// The app's HTTP consumers share this boundary, including the clients in JournalMarkKit.
/// Redirects are returned to the consumer and are never followed by this transport.
struct JournalLocalHTTPClient: Sendable {
    typealias RootProvider = @Sendable () async -> URL
    static let portInUse = "your journal can't start because another app is using the connection it needs on this mac. on a shared mac, this is usually another account's journal."
    let root: RootProvider
    var environment = ProcessInfo.processInfo.environment

    static func portAvailable(_ port: UInt16) -> Bool {
        let fd = socket(AF_INET, SOCK_STREAM, 0)
        guard fd >= 0 else { return false }
        defer { close(fd) }
        var reuse: Int32 = 1
        guard setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &reuse, socklen_t(MemoryLayout<Int32>.size)) == 0 else { return false }
        var address = sockaddr_in()
        address.sin_len = UInt8(MemoryLayout<sockaddr_in>.size)
        address.sin_family = sa_family_t(AF_INET)
        address.sin_port = port.bigEndian
        address.sin_addr.s_addr = inet_addr("127.0.0.1")
        return withUnsafePointer(to: &address) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                bind(fd, $0, socklen_t(MemoryLayout<sockaddr_in>.size)) == 0
            }
        }
    }

    static func socketExists(_ path: String) throws -> Bool {
        var info = stat()
        guard lstat(path, &info) == 0 else {
            if errno == ENOENT { return false }
            throw LocalHTTPError.refused
        }
        guard info.st_mode & S_IFMT == S_IFSOCK, info.st_uid == geteuid() else {
            throw LocalHTTPError.refused
        }
        return true
    }

    func data(for request: URLRequest) async throws -> (Data, URLResponse) {
        let endpoint = try await verifiedEndpoint()
        return try await send(request, endpoint: endpoint)
    }

    func browserURL(path: String) async throws -> URL {
        let endpoint = try await verifiedEndpoint()
        return URL(string: "http://127.0.0.1:\(endpoint.port)\(path)")!
    }

    private struct Endpoint {
        var socket: String?
        var port: Int
    }

    private struct Runtime: Decodable {
        var generation: String
        var journal_root: String
        var tcp_port: Int
        var socket_path: String
    }

    private func verifiedEndpoint() async throws -> Endpoint {
        try Task.checkCancellation()
        let ownRoot = await root().resolvingSymlinksInPath().standardizedFileURL.path
        let socket = try JournalLocalEndpoint.socketPath(environment: environment)
        let recordURL = socket.map { URL(fileURLWithPath: $0).deletingLastPathComponent().appendingPathComponent("journal.json") }
        let record: Runtime?
        if let recordURL {
            do {
                let bytes = try Data(contentsOf: recordURL)
                record = try JSONDecoder().decode(Runtime.self, from: bytes)
            } catch let error as CocoaError where error.code == .fileReadNoSuchFile {
                record = nil
            } catch { throw LocalHTTPError.unavailable }
        } else { record = nil }
        if let record {
            guard Self.sameRoot(record.journal_root, ownRoot), (1...65535).contains(record.tcp_port),
                  record.socket_path == socket, !record.generation.isEmpty else { throw LocalHTTPError.wrongJournal }
        }
        let exists = try socket.map(Self.socketExists) ?? false
        let endpoint = Endpoint(socket: exists ? socket : nil, port: record?.tcp_port ?? 5015)
        if exists {
            guard let record else { throw LocalHTTPError.unavailable }
            let request = URLRequest(url: URL(string: "http://localhost/_solstone/local-runtime")!, timeoutInterval: 5)
            let (bytes, response) = try await send(request, endpoint: endpoint)
            guard (response as? HTTPURLResponse)?.statusCode == 200 else { throw LocalHTTPError.unavailable }
            let live = try JSONDecoder().decode(Runtime.self, from: bytes)
            guard Self.sameRoot(live.journal_root, ownRoot), live.generation == record.generation,
                  live.socket_path == record.socket_path, live.tcp_port == record.tcp_port else {
                throw LocalHTTPError.wrongJournal
            }
        } else {
            // Older journals have no runtime route. Their instance id or setup root still
            // proves which journal answered before any mark or action reaches a consumer.
            let ownState = URL(fileURLWithPath: ownRoot).appendingPathComponent("link/state.json")
            let nativeState = URL(fileURLWithPath: ownRoot).appendingPathComponent("link/ca/state.json")
            let stateURL = FileManager.default.fileExists(atPath: ownState.path) ? ownState : nativeState
            if let bytes = try? Data(contentsOf: stateURL),
               let state = try JSONSerialization.jsonObject(with: bytes) as? [String: Any],
               let expected = state["instance_id"] as? String, !expected.isEmpty {
                let request = URLRequest(url: URL(string: "http://localhost/app/link/api/identity")!, timeoutInterval: 5)
                let (bytes, response) = try await send(request, endpoint: endpoint)
                guard (response as? HTTPURLResponse)?.statusCode == 200,
                      let identity = try JSONSerialization.jsonObject(with: bytes) as? [String: Any],
                      identity["instance_id"] as? String == expected else { throw LocalHTTPError.wrongJournal }
            } else {
                let request = URLRequest(url: URL(string: "http://localhost/init/api/state")!, timeoutInterval: 5)
                let (bytes, response) = try await send(request, endpoint: endpoint)
                guard (response as? HTTPURLResponse)?.statusCode == 200,
                      let state = try JSONSerialization.jsonObject(with: bytes) as? [String: Any],
                      let path = state["journal_path"] as? String, Self.sameRoot(path, ownRoot) else {
                    throw LocalHTTPError.wrongJournal
                }
            }
        }
        return endpoint
    }

    static func sameRoot(_ left: String, _ right: String) -> Bool {
        URL(fileURLWithPath: left).resolvingSymlinksInPath().standardizedFileURL ==
            URL(fileURLWithPath: right).resolvingSymlinksInPath().standardizedFileURL
    }

    private func send(_ request: URLRequest, endpoint: Endpoint) async throws -> (Data, URLResponse) {
        guard let originalURL = request.url, originalURL.scheme == "http",
              ["localhost", "127.0.0.1"].contains(originalURL.host) else { throw LocalHTTPError.unavailable }
        if let socket = endpoint.socket {
            guard try Self.socketExists(socket) else { throw LocalHTTPError.refused }
            return try await UnixHTTPExchange(socket: socket, request: request).run()
        }
        var forwarded = request
        var url = URLComponents(url: originalURL, resolvingAgainstBaseURL: false)!
        url.host = "127.0.0.1"
        url.port = endpoint.port
        forwarded.url = url.url
        forwarded.setValue("solstone-journal-app", forHTTPHeaderField: "User-Agent")
        let config = URLSessionConfiguration.ephemeral
        config.connectionProxyDictionary = [:]
        let delegate = LocalNoRedirectDelegate()
        let session = URLSession(configuration: config, delegate: delegate, delegateQueue: nil)
        defer { session.invalidateAndCancel() }
        let (data, response) = try await session.data(for: forwarded)
        guard let http = response as? HTTPURLResponse, http.url == forwarded.url,
              let normalized = HTTPURLResponse(url: originalURL, statusCode: http.statusCode, httpVersion: "HTTP/1.1",
                headerFields: http.allHeaderFields.reduce(into: [String: String]()) { $0[String(describing: $1.key)] = String(describing: $1.value) })
        else { throw LocalHTTPError.unavailable }
        return (data, normalized)
    }
}

enum LocalHTTPError: LocalizedError {
    case refused, wrongJournal, unavailable, invalidResponse, timedOut
    var errorDescription: String? {
        switch self {
        case .refused: "the journal app couldn't connect to your journal on this mac."
        case .wrongJournal: JournalLocalHTTPClient.portInUse
        case .unavailable: "your journal isn't ready for the journal app yet. try again shortly."
        case .invalidResponse: "the journal app couldn't read what came back from your journal."
        case .timedOut: "the journal app waited too long for your journal. try again."
        }
    }
}

private final class LocalNoRedirectDelegate: NSObject, URLSessionTaskDelegate, @unchecked Sendable {
    func urlSession(_ session: URLSession, task: URLSessionTask, willPerformHTTPRedirection response: HTTPURLResponse,
                    newRequest request: URLRequest) async -> URLRequest? { nil }
}

/// All mutable exchange state is confined to queue. Cancellation and the deadline use it too.
private final class UnixHTTPExchange: @unchecked Sendable {
    let connection: NWConnection
    let request: URLRequest
    let queue = DispatchQueue(label: "journal.local-http")
    var continuation: CheckedContinuation<(Data, URLResponse), any Error>?
    var buffer = Data()
    var cancelled = false
    var deadline: DispatchWorkItem?

    init(socket: String, request: URLRequest) {
        self.connection = NWConnection(to: .unix(path: socket), using: .tcp)
        self.request = request
    }

    func run() async throws -> (Data, URLResponse) {
        try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { continuation in
                queue.async { self.start(continuation) }
            }
        } onCancel: {
            queue.async {
                self.cancelled = true
                self.finish(.failure(CancellationError()))
            }
        }
    }

    private func start(_ continuation: CheckedContinuation<(Data, URLResponse), any Error>) {
        self.continuation = continuation
        if cancelled { finish(.failure(CancellationError())); return }
        let timeout = DispatchWorkItem { self.finish(.failure(LocalHTTPError.timedOut)) }
        deadline = timeout
        queue.asyncAfter(deadline: .now() + min(max(request.timeoutInterval, 1), 30), execute: timeout)
        connection.stateUpdateHandler = { state in
            switch state {
            case .ready: self.send()
            case .failed: self.finish(.failure(LocalHTTPError.unavailable))
            default: break
            }
        }
        connection.start(queue: queue)
    }

    private func send() {
        guard let url = request.url else { finish(.failure(LocalHTTPError.unavailable)); return }
        let components = URLComponents(url: url, resolvingAgainstBaseURL: false)!
        let target = (components.percentEncodedPath.isEmpty ? "/" : components.percentEncodedPath) +
            (components.percentEncodedQuery.map { "?" + $0 } ?? "")
        let body = request.httpBody ?? Data()
        var head = "\(request.httpMethod ?? "GET") \(target) HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nUser-Agent: solstone-journal-app\r\nContent-Length: \(body.count)\r\n"
        for (key, value) in request.allHTTPHeaderFields ?? [:] {
            guard !["host", "connection", "content-length", "transfer-encoding", "user-agent"].contains(key.lowercased()) else { continue }
            guard !key.contains("\r"), !key.contains("\n"), !value.contains("\r"), !value.contains("\n") else {
                finish(.failure(LocalHTTPError.invalidResponse)); return
            }
            head += "\(key): \(value)\r\n"
        }
        head += "\r\n"
        connection.send(content: Data(head.utf8) + body, completion: .contentProcessed { error in
            if error != nil { self.finish(.failure(LocalHTTPError.unavailable)) }
            else { self.receive() }
        })
    }

    private func receive() {
        connection.receive(minimumIncompleteLength: 1, maximumLength: 65536) { data, _, complete, error in
            guard self.continuation != nil else { return }
            if let data { self.buffer.append(data) }
            do {
                if let parsed = try JournalHTTPFraming.parse(self.buffer, url: self.request.url!, method: self.request.httpMethod ?? "GET") {
                    self.finish(.success(parsed)); return
                }
                if complete || error != nil { self.finish(.failure(LocalHTTPError.invalidResponse)); return }
                self.receive()
            } catch { self.finish(.failure(error)) }
        }
    }

    private func finish(_ result: Result<(Data, URLResponse), any Error>) {
        guard let continuation else { return }
        self.continuation = nil
        deadline?.cancel()
        deadline = nil
        connection.stateUpdateHandler = nil
        connection.cancel()
        continuation.resume(with: result)
    }
}

/// A complete frame wins over an EOF/Network error delivered with the final bytes.
enum JournalHTTPFraming {
    static func parse(_ bytes: Data, url: URL, method: String = "GET") throws -> (Data, URLResponse)? {
        let boundary = Data("\r\n\r\n".utf8)
        guard bytes.count <= 16 * 1024 * 1024 else { throw LocalHTTPError.invalidResponse }
        guard let end = bytes.range(of: boundary) else {
            if bytes.count > 65536 { throw LocalHTTPError.invalidResponse }
            return nil
        }
        guard end.lowerBound <= 65536,
              let head = String(data: bytes[..<end.lowerBound], encoding: .utf8) else { throw LocalHTTPError.invalidResponse }
        let lines = head.components(separatedBy: "\r\n")
        let status = lines[0].split(separator: " ")
        guard status.count >= 2, ["HTTP/1.0", "HTTP/1.1"].contains(String(status[0])), let code = Int(status[1]),
              (100...599).contains(code) else { throw LocalHTTPError.invalidResponse }
        if (100..<200).contains(code), code != 101 {
            return try parse(bytes.subdata(in: end.upperBound..<bytes.count), url: url, method: method)
        }
        var headers: [String: String] = [:]
        for line in lines.dropFirst() {
            guard let colon = line.firstIndex(of: ":") else { throw LocalHTTPError.invalidResponse }
            let key = line[..<colon].lowercased()
            let value = line[line.index(after: colon)...].trimmingCharacters(in: .whitespaces)
            guard !key.isEmpty, headers[key] == nil else { throw LocalHTTPError.invalidResponse }
            headers[key] = value
        }
        guard let response = HTTPURLResponse(url: url, statusCode: code, httpVersion: "HTTP/1.1", headerFields: headers) else {
            throw LocalHTTPError.invalidResponse
        }
        if method == "HEAD" || code == 204 || code == 304 { return (Data(), response) }
        let start = end.upperBound
        if let transfer = headers["transfer-encoding"] {
            guard transfer.lowercased() == "chunked", headers["content-length"] == nil else { throw LocalHTTPError.invalidResponse }
            var offset = start
            var body = Data()
            while true {
                guard let line = bytes.range(of: Data("\r\n".utf8), in: offset..<bytes.count) else { return nil }
                let text = String(decoding: bytes[offset..<line.lowerBound], as: UTF8.self)
                guard let token = text.split(separator: ";", maxSplits: 1).first,
                      let chunk = Int(token, radix: 16), chunk >= 0,
                      chunk <= 16 * 1024 * 1024 - body.count else { throw LocalHTTPError.invalidResponse }
                offset = line.upperBound
                if chunk == 0 {
                    if bytes.count >= offset + 2, bytes[offset..<offset+2] == Data("\r\n".utf8) { return (body, response) }
                    guard bytes.range(of: boundary, in: offset..<bytes.count) != nil else { return nil }
                    return (body, response)
                }
                guard bytes.count - offset >= chunk + 2 else { return nil }
                guard bytes[offset+chunk..<offset+chunk+2] == Data("\r\n".utf8) else { throw LocalHTTPError.invalidResponse }
                body.append(bytes[offset..<offset+chunk])
                offset += chunk + 2
            }
        }
        guard let value = headers["content-length"], let length = Int(value), length >= 0,
              length <= 16 * 1024 * 1024 else { throw LocalHTTPError.invalidResponse }
        guard bytes.count - start >= length else { return nil }
        return (bytes.subdata(in: start..<start+length), response)
    }
}
