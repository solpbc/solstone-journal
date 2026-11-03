// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import Foundation
import JournalMarkKit

protocol JournalSetupClienting: Sendable {
    func state() async throws -> JournalSetupState
    func perform(_ action: JournalSetupAction) async throws -> JournalSetupState
    func networkStatus() async throws -> JournalNetworkStatus
}

protocol JournalIdentityResolving: Sendable {
    func resolveIdentity(baseURL: String) async -> JournalIdentityResolution
}

struct JournalIdentityResolver: JournalIdentityResolving {
    private let fetcher: JournalIdentityFetcher

    init(fetcher: JournalIdentityFetcher = JournalIdentityFetcher()) {
        self.fetcher = fetcher
    }

    func resolveIdentity(baseURL: String) async -> JournalIdentityResolution {
        await fetcher.resolveIdentity(
            baseURL: baseURL,
            expectedInstanceID: nil,
            timeoutInterval: 2
        )
    }
}

enum JournalSetupAction: Equatable, Sendable {
    case regenerateMark
    case commitMark(proposalID: String)
    case saveThinking(choice: String)
    case startThinkingDownload
    case cancelThinkingDownload(attemptID: String)
    case retrySpeechDownload
    case finalize(timezone: String)

    fileprivate var body: ActionBody {
        switch self {
        case .regenerateMark:
            return ActionBody(action: "regenerate_mark")
        case .commitMark(let proposalID):
            return ActionBody(action: "commit_mark", proposalID: proposalID)
        case .saveThinking(let choice):
            return ActionBody(action: "save_thinking", choice: choice)
        case .startThinkingDownload:
            return ActionBody(action: "start_thinking_download")
        case .cancelThinkingDownload(let attemptID):
            return ActionBody(action: "cancel_thinking_download", attemptID: attemptID)
        case .retrySpeechDownload:
            return ActionBody(action: "retry_speech_download")
        case .finalize(let timezone):
            return ActionBody(action: "finalize", timezone: timezone)
        }
    }
}

enum JournalSetupClientError: Error, Equatable {
    case invalidURL
    case redirect
    case unexpectedResponseURL
    case server(JournalDevicesErrorEnvelope)
    case transport(String)
    case decoding
    case wrongSchema
}

struct JournalNetworkStatus: Decodable, Equatable, Sendable {
    var caFingerprint: String?
    var instanceID: String?

    enum CodingKeys: String, CodingKey {
        case caFingerprint = "ca_fingerprint"
        case instanceID = "instance_id"
    }
}

enum JournalMarkWireState: String, Codable, Equatable, Sendable {
    case locked
    case proposed
    case unavailable
}

enum JournalThinkingChoice: String, Codable, Equatable, Sendable {
    case local
    case byo
    case spp
    case later
}

enum JournalThinkingCapability: String, Codable, Equatable, Sendable {
    case ok
    case blocked
    case unknown
}

enum JournalDownloadJobState: String, Codable, Equatable, Sendable {
    case idle
    case downloading
    case complete
    case cancelled
    case error
}

struct JournalSetupState: Decodable, Equatable, Sendable {
    var schema: String
    var journalPath: String
    var done: Bool
    var mark: Mark
    var thinking: Thinking
    var downloads: Downloads

    struct Mark: Decodable, Equatable, Sendable {
        var render: JournalMark?
        var state: JournalMarkWireState
        var proposalID: String?

        enum CodingKeys: String, CodingKey {
            case render
            case state
            case proposalID = "proposal_id"
        }

        init(render: JournalMark?, state: JournalMarkWireState, proposalID: String?) {
            self.render = render
            self.state = state
            self.proposalID = proposalID
        }
    }

    struct Thinking: Decodable, Equatable, Sendable {
        var choice: JournalThinkingChoice?
        var saved: Bool
        var capability: JournalThinkingCapability
        var downloadBytes: UInt64?

        enum CodingKeys: String, CodingKey {
            case choice
            case saved
            case capability
            case downloadBytes = "download_bytes"
        }

        init(
            choice: JournalThinkingChoice?,
            saved: Bool,
            capability: JournalThinkingCapability,
            downloadBytes: UInt64?
        ) {
            self.choice = choice
            self.saved = saved
            self.capability = capability
            self.downloadBytes = downloadBytes
        }
    }

    struct JobError: Decodable, Equatable, Sendable {
        var code: String?
        var detail: String?
    }

    struct Job: Decodable, Equatable, Sendable {
        var state: JournalDownloadJobState
        var attemptID: String?
        var bytesDone: UInt64
        var bytesTotal: UInt64?
        var error: JobError?

        enum CodingKeys: String, CodingKey {
            case state
            case attemptID = "attempt_id"
            case bytesDone = "bytes_done"
            case bytesTotal = "bytes_total"
            case error
        }

        init(
            state: JournalDownloadJobState,
            attemptID: String?,
            bytesDone: UInt64,
            bytesTotal: UInt64?,
            error: JobError? = nil
        ) {
            self.state = state
            self.attemptID = attemptID
            self.bytesDone = bytesDone
            self.bytesTotal = bytesTotal
            self.error = error
        }
    }

    struct Downloads: Decodable, Equatable, Sendable {
        var speech: Job
        var thinking: Job

        init(speech: Job, thinking: Job) {
            self.speech = speech
            self.thinking = thinking
        }
    }

    enum CodingKeys: String, CodingKey {
        case schema
        case journalPath = "journal_path"
        case done
        case mark
        case thinking
        case downloads
    }

    init(
        schema: String = "solstone.setup-state.v1",
        journalPath: String,
        done: Bool,
        mark: Mark,
        thinking: Thinking,
        downloads: Downloads
    ) {
        self.schema = schema
        self.journalPath = journalPath
        self.done = done
        self.mark = mark
        self.thinking = thinking
        self.downloads = downloads
    }
}

struct JournalSetupClient: JournalSetupClienting, Sendable {
    static let successSchema = "solstone.setup-state.v1"
    static let timeout: TimeInterval = 5

    private let baseURL: String
    private let dataLoader: (@Sendable (URLRequest) async throws -> (Data, URLResponse))?
    private let session: URLSession
    private let redirectDelegate: SetupNoRedirectDelegate?

    init(baseURL: String = "http://127.0.0.1:5015", session: URLSession? = nil,
         dataLoader: (@Sendable (URLRequest) async throws -> (Data, URLResponse))? = nil) {
        self.dataLoader = dataLoader
        self.baseURL = baseURL.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        if let session {
            self.session = session
            self.redirectDelegate = nil
        } else {
            let delegate = SetupNoRedirectDelegate()
            let configuration = URLSessionConfiguration.ephemeral
            configuration.timeoutIntervalForRequest = Self.timeout
            configuration.timeoutIntervalForResource = Self.timeout
            self.redirectDelegate = delegate
            self.session = URLSession(configuration: configuration, delegate: delegate, delegateQueue: nil)
        }
    }

    func state() async throws -> JournalSetupState {
        let request = try makeRequest(path: "/init/api/state", method: "GET", body: nil)
        return try decodeState(try await send(request))
    }

    func perform(_ action: JournalSetupAction) async throws -> JournalSetupState {
        let body = try JSONEncoder().encode(action.body)
        let request = try makeRequest(path: "/init/api/action", method: "POST", body: body)
        return try decodeState(try await send(request))
    }

    func networkStatus() async throws -> JournalNetworkStatus {
        let request = try makeRequest(path: "/app/network/api/status", method: "GET", body: nil)
        let data = try await send(request)
        do {
            return try JSONDecoder().decode(JournalNetworkStatus.self, from: data)
        } catch {
            throw JournalSetupClientError.decoding
        }
    }

    private func makeRequest(path: String, method: String, body: Data?) throws -> URLRequest {
        guard let url = URL(string: baseURL + path) else {
            throw JournalSetupClientError.invalidURL
        }
        var request = URLRequest(url: url)
        request.httpMethod = method
        request.timeoutInterval = Self.timeout
        if let body {
            request.httpBody = body
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        }
        return request
    }

    private func send(_ request: URLRequest) async throws -> Data {
        let data: Data
        let response: URLResponse
        do {
            if let dataLoader { (data, response) = try await dataLoader(request) }
            else { (data, response) = try await session.data(for: request) }
        } catch let error as LocalHTTPError {
            throw error
        } catch {
            throw JournalSetupClientError.transport(error.localizedDescription)
        }
        guard let http = response as? HTTPURLResponse else {
            throw JournalSetupClientError.transport("not http")
        }
        if (300..<400).contains(http.statusCode) {
            throw JournalSetupClientError.redirect
        }
        guard http.url == request.url else {
            throw JournalSetupClientError.unexpectedResponseURL
        }
        guard (200..<300).contains(http.statusCode) else {
            if let envelope = try? JSONDecoder().decode(JournalDevicesErrorEnvelope.self, from: data) {
                throw JournalSetupClientError.server(envelope)
            }
            throw JournalSetupClientError.decoding
        }
        return data
    }

    private func decodeState(_ data: Data) throws -> JournalSetupState {
        let state: JournalSetupState
        do {
            state = try JSONDecoder().decode(JournalSetupState.self, from: data)
        } catch {
            throw JournalSetupClientError.decoding
        }
        guard state.schema == Self.successSchema else {
            throw JournalSetupClientError.wrongSchema
        }
        return state
    }
}

private struct ActionBody: Encodable {
    var action: String
    var proposalID: String?
    var choice: String?
    var attemptID: String?
    var timezone: String?

    enum CodingKeys: String, CodingKey {
        case action
        case proposalID = "proposal_id"
        case choice
        case attemptID = "attempt_id"
        case timezone
    }
}

private final class SetupNoRedirectDelegate: NSObject, URLSessionTaskDelegate, @unchecked Sendable {
    func urlSession(
        _ session: URLSession,
        task: URLSessionTask,
        willPerformHTTPRedirection response: HTTPURLResponse,
        newRequest request: URLRequest
    ) async -> URLRequest? {
        nil
    }
}
