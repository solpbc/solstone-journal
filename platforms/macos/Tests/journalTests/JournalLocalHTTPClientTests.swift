// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import Darwin
import Foundation
import Network
import Testing
@testable import journal

/// Delivers `.ready`, takes the request, then replays the events a test names, in that order.
private final class ScriptedConnection: UnixExchangeConnection, @unchecked Sendable {
    enum Event { case failed, bytes(String), closed }
    var stateUpdateHandler: (@Sendable (NWConnection.State) -> Void)?
    private let script: [Event]
    private let sendCompletes: Bool
    private var queue: DispatchQueue?
    private var pending: (@Sendable (Data?, Bool, NWError?) -> Void)?
    private var next = 0

    init(_ script: [Event], sendCompletes: Bool = true) {
        self.script = script
        self.sendCompletes = sendCompletes
    }
    func start(queue: DispatchQueue) {
        self.queue = queue
        queue.async { self.stateUpdateHandler?(.ready) }
    }
    func send(_ content: Data, completion: @escaping @Sendable (NWError?) -> Void) {
        if sendCompletes { completion(nil) } else { queue?.async { self.stateUpdateHandler?(.failed(.posix(.EPIPE))) } }
    }
    func receive(_ completion: @escaping @Sendable (Data?, Bool, NWError?) -> Void) {
        pending = completion
        queue?.async { self.play() }
    }
    func cancel() {}
    private func play() {
        while next < script.count {
            let event = script[next]
            next += 1
            switch event {
            case .failed:
                stateUpdateHandler?(.failed(.posix(.ENETDOWN)))
            case .bytes(let text):
                let done = pending
                pending = nil
                done?(Data(text.utf8), false, nil)
                return
            case .closed:
                let done = pending
                pending = nil
                done?(nil, true, .posix(.ENETDOWN))
                return
            }
        }
    }
}

/// A real socket that answers once per connection and closes, the way the journal does on `Connection: close`.
private final class ClosingSocketServer: @unchecked Sendable {
    let path: String
    private let fd: Int32

    init(reply: String?) throws {
        path = "/tmp/jla-\(UUID().uuidString.prefix(8)).sock"
        fd = Darwin.socket(AF_UNIX, SOCK_STREAM, 0)
        var address = sockaddr_un()
        address.sun_family = sa_family_t(AF_UNIX)
        _ = withUnsafeMutablePointer(to: &address.sun_path) {
            $0.withMemoryRebound(to: CChar.self, capacity: 104) { strncpy($0, path, 103) }
        }
        let bound = withUnsafePointer(to: &address) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { bind(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size)) }
        }
        try #require(bound == 0 && listen(fd, 64) == 0)
        let listener = fd
        Thread.detachNewThread {
            while true {
                let peer = accept(listener, nil, nil)
                guard peer >= 0 else { return }
                var noSigpipe: Int32 = 1
                setsockopt(peer, SOL_SOCKET, SO_NOSIGPIPE, &noSigpipe, socklen_t(MemoryLayout<Int32>.size))
                var buffer = [UInt8](repeating: 0, count: 1024)
                _ = read(peer, &buffer, buffer.count)
                if let reply { _ = reply.withCString { write(peer, $0, strlen($0)) } }
                close(peer)
            }
        }
    }

    deinit {
        shutdown(fd, SHUT_RDWR)
        close(fd)
        unlink(path)
    }
}

struct UnixHTTPExchangeTests {
    private let answer = "HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello"
    private var request: URLRequest { URLRequest(url: URL(string: "http://localhost/init")!, timeoutInterval: 5) }

    private func scripted(_ script: [ScriptedConnection.Event], sendCompletes: Bool = true) async throws -> (Data, URLResponse) {
        try await UnixHTTPExchange(connection: ScriptedConnection(script, sendCompletes: sendCompletes), request: request).run()
    }

    private func exchange(_ server: ClosingSocketServer) async throws -> (Data, URLResponse) {
        try await UnixHTTPExchange(socket: server.path, request: request).run()
    }

    @Test func aCompleteAnswerBehindTheFailedStateIsDeliveredAsTheAnswer() async throws {
        let result = try await scripted([.failed, .bytes(answer)])
        #expect(result.0 == Data("hello".utf8))
        #expect((result.1 as? HTTPURLResponse)?.statusCode == 200)
    }

    @Test func aFailedStateBeforeTheSendCompletesStillReadsTheAnswer() async throws {
        let result = try await scripted([.bytes(answer)], sendCompletes: false)
        #expect(result.0 == Data("hello".utf8))
    }

    @Test func aFailedStateBeforeTheSendCompletesWithNothingBehindItDoesNotWaitForTheDeadline() async throws {
        let started = Date()
        do {
            _ = try await scripted([.closed], sendCompletes: false)
            Issue.record("expected the exchange to fail")
        } catch let error as LocalHTTPError {
            guard case .unavailable = error else { Issue.record("got \(error)"); return }
        }
        #expect(Date().timeIntervalSince(started) < 4)
    }

    @Test func aFailedStateWithNothingBehindItIsStillUnavailable() async throws {
        do {
            _ = try await scripted([.failed, .closed])
            Issue.record("expected the exchange to fail")
        } catch let error as LocalHTTPError {
            guard case .unavailable = error else { Issue.record("got \(error)"); return }
        }
    }

    @Test func anAnswerFollowedByACloseIsAlwaysRead() async throws {
        let server = try ClosingSocketServer(reply: answer)
        for _ in 0..<300 {
            let result = try await exchange(server)
            #expect(result.0 == Data("hello".utf8))
        }
    }

    @Test func aCloseWithNoAnswerIsUnavailableAndDoesNotWaitForTheDeadline() async throws {
        let server = try ClosingSocketServer(reply: nil)
        let started = Date()
        do {
            _ = try await exchange(server)
            Issue.record("expected the exchange to fail")
        } catch let error as LocalHTTPError {
            guard case .unavailable = error else { Issue.record("got \(error)"); return }
        }
        #expect(Date().timeIntervalSince(started) < 4)
    }

    @Test func aPartialAnswerBeforeACloseIsInvalid() async throws {
        let server = try ClosingSocketServer(reply: "HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhe")
        do {
            _ = try await exchange(server)
            Issue.record("expected the exchange to fail")
        } catch let error as LocalHTTPError {
            guard case .invalidResponse = error else { Issue.record("got \(error)"); return }
        }
    }
}

struct JournalLocalHTTPClientTests {
    let url = URL(string: "http://localhost/init")!

    @Test func fragmentedContentLengthCompletesWithoutEOF() throws {
        let message = Data("HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello".utf8)
        for count in 0..<message.count {
            #expect(try JournalHTTPFraming.parse(Data(message.prefix(count)), url: url) == nil)
        }
        let result = try #require(try JournalHTTPFraming.parse(message, url: url))
        #expect(result.0 == Data("hello".utf8))
        #expect((result.1 as? HTTPURLResponse)?.statusCode == 200)
    }

    @Test func chunkExtensionsAndTrailersAreFramedAcrossFragments() throws {
        let message = Data("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3;test=yes\r\nabc\r\n2\r\nde\r\n0\r\nX-Finished: yes\r\n\r\n".utf8)
        for count in 0..<message.count {
            #expect(try JournalHTTPFraming.parse(Data(message.prefix(count)), url: url) == nil)
        }
        #expect(try JournalHTTPFraming.parse(message, url: url)?.0 == Data("abcde".utf8))
        let empty = Data("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n".utf8)
        #expect(try JournalHTTPFraming.parse(empty, url: url)?.0.isEmpty == true)
    }

    @Test func redirectDoesNotRequireAnotherConnection() throws {
        let message = Data("HTTP/1.1 302 Found\r\nLocation: /\r\nContent-Length: 0\r\n\r\n".utf8)
        let result = try #require(try JournalHTTPFraming.parse(message, url: url))
        #expect((result.1 as? HTTPURLResponse)?.statusCode == 302)
        #expect(result.1.url == url)
    }

    // The head Journal 2.0.69 sent over its local socket, captured from a released runtime.
    @Test func repeatedListHeadersReadAsOneValue() throws {
        let head = "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\nserver-timing: gate;dur=3.151\r\nserver-timing: app;dur=4.704\r\ncontent-security-policy: default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; media-src 'self' blob:; form-action 'self'; frame-ancestors 'none'; object-src 'none'; base-uri 'self'\r\nx-frame-options: DENY\r\ncontent-length: 687\r\ndate: Fri, 06 Nov 2026 11:03:57 GMT\r\n\r\n"
        let body = Data(repeating: 0x20, count: 687)
        let result = try #require(try JournalHTTPFraming.parse(Data(head.utf8) + body, url: url))
        #expect(result.0 == body)
        let http = try #require(result.1 as? HTTPURLResponse)
        #expect(http.statusCode == 200)
        #expect(http.value(forHTTPHeaderField: "server-timing") == "gate;dur=3.151, app;dur=4.704")
    }

    @Test(arguments: [
        "HTTP/1.1 200 OK\r\nContent-Length: -1\r\n\r\n",
        "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nContent-Length: 1\r\n\r\n",
        "HTTP/1.1 200 OK\r\nContent-Length: 0\r\ncontent-length: 0\r\n\r\n",
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n",
        "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nTransfer-Encoding: chunked\r\n\r\n",
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n\r\n",
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n",
    ])
    func invalidFramingIsRefused(_ message: String) {
        #expect(throws: (any Error).self) { try JournalHTTPFraming.parse(Data(message.utf8), url: url) }
    }

    @Test func aLinkIsOpenedOnlyWhenItIsAOneTimeLinkToTheCheckedPort() {
        let link = Data(#"{"url":"http://localhost:15415/?auth=abc","expires_in":60}"#.utf8)
        #expect(JournalLocalHTTPClient.linkAnswer(status: 200, body: link, port: 15415)
            == .link(URL(string: "http://localhost:15415/?auth=abc")!))
        for url in ["http://localhost:15416/?auth=abc", "https://localhost:15415/?auth=abc", "http://example.com:15415/?auth=abc",
                    "http://localhost:15415/app?auth=abc", "http://localhost:15415/?auth=", "http://localhost:15415/?auth=a&next=b",
                    "http://localhost:15415/?auth=abc#x", "http://localhost:15415/"] {
            let body = try! JSONSerialization.data(withJSONObject: ["url": url])
            #expect(JournalLocalHTTPClient.linkAnswer(status: 200, body: body, port: 15415) == .failed("the answer held no usable link"))
        }
        #expect(JournalLocalHTTPClient.linkAnswer(status: 200, body: Data("not json".utf8), port: 15415)
            == .failed("the answer held no usable link"))
    }

    @Test func onlyA404MeansAJournalWithoutLinks() {
        #expect(JournalLocalHTTPClient.linkAnswer(status: 404, body: Data(), port: 5015) == .noLinks)
        let busy = Data(#"{"error":"too many unused links; try again in a minute"}"#.utf8)
        #expect(JournalLocalHTTPClient.linkAnswer(status: 400, body: busy, port: 5015)
            == .failed("status 400: too many unused links; try again in a minute"))
        for status in [0, 401, 409, 500, 503] {
            guard case .failed = JournalLocalHTTPClient.linkAnswer(status: status, body: Data(), port: 5015) else {
                Issue.record("status \(status) must fail"); continue
            }
        }
    }
}
