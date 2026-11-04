// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import Foundation
import Testing
@testable import journal

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

    @Test(arguments: [
        "HTTP/1.1 200 OK\r\nContent-Length: -1\r\n\r\n",
        "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nContent-Length: 1\r\n\r\n",
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
