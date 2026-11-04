// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import Foundation

protocol JournalDevicesClientProtocol: Sendable {
    func listDevices() async throws -> [DeviceRow]
    func startPairing() async throws -> PairStartResponse
    func nonceStatus(nonce: String) async throws -> NonceStatusResponse
    func unpairDevice(fingerprint: String) async throws -> UnpairResponse
}

struct DeviceRow: Decodable, Equatable, Identifiable, Sendable {
    var displayLabel: String?
    var deviceLabel: String?
    var kind: String?
    var role: String?
    var network: String?
    var pairedAt: String?
    var lastSeenAt: String?
    var fingerprint: String
    var observerHandle: String?

    var id: String { fingerprint }

    enum CodingKeys: String, CodingKey {
        case displayLabel = "display_label"
        case deviceLabel = "device_label"
        case kind
        case role
        case network
        case pairedAt = "paired_at"
        case lastSeenAt = "last_seen_at"
        case fingerprint
        case observerHandle = "observer_handle"
    }

    init(
        displayLabel: String? = nil,
        deviceLabel: String? = nil,
        kind: String? = nil,
        role: String? = nil,
        network: String? = nil,
        pairedAt: String? = nil,
        lastSeenAt: String? = nil,
        fingerprint: String,
        observerHandle: String? = nil
    ) {
        self.displayLabel = displayLabel
        self.deviceLabel = deviceLabel
        self.kind = kind
        self.role = role
        self.network = network
        self.pairedAt = pairedAt
        self.lastSeenAt = lastSeenAt
        self.fingerprint = fingerprint
        self.observerHandle = observerHandle
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        displayLabel = try container.decodeIfPresent(String.self, forKey: .displayLabel)
        deviceLabel = try container.decodeIfPresent(String.self, forKey: .deviceLabel)
        kind = try container.decodeIfPresent(String.self, forKey: .kind)
        role = try container.decodeIfPresent(String.self, forKey: .role)
        network = try container.decodeIfPresent(String.self, forKey: .network)
        pairedAt = try container.decodeLossyStringIfPresent(forKey: .pairedAt)
        lastSeenAt = try container.decodeLossyStringIfPresent(forKey: .lastSeenAt)
        fingerprint = try container.decode(String.self, forKey: .fingerprint)
        observerHandle = try container.decodeIfPresent(String.self, forKey: .observerHandle)
    }
}

struct DevicesListResponse: Decodable, Equatable, Sendable {
    var devices: [DeviceRow]
}

struct PairStartResponse: Decodable, Equatable, Sendable {
    var nonce: String
    var pairLink: String
    var expiresIn: Int
    var deviceLabel: String
    var caFingerprint: String
    var relay: Bool?

    enum CodingKeys: String, CodingKey {
        case nonce
        case pairLink = "pair_link"
        case expiresIn = "expires_in"
        case deviceLabel = "device_label"
        case caFingerprint = "ca_fingerprint"
        case relay
    }
}

struct NonceStatusResponse: Decodable, Equatable, Sendable {
    /// The device a used link added, when the journal still holds it.
    struct Device: Decodable, Equatable, Sendable {
        var cid: String
        var displayLabel: String?

        enum CodingKeys: String, CodingKey {
            case cid
            case displayLabel = "display_label"
        }
    }

    var present: Bool
    var used: Bool
    var device: Device?
}

struct UnpairResponse: Decodable, Equatable, Sendable {
    var unpaired: String
}

struct JournalDevicesErrorEnvelope: Decodable, Equatable, Sendable {
    static let pairedDeviceNotFound = "PAIRED_DEVICE_NOT_FOUND"

    var error: String?
    var reasonCode: String?
    var detail: String?

    enum CodingKeys: String, CodingKey {
        case error
        case reasonCode = "reason_code"
        case detail
    }

    init(error: String? = nil, reasonCode: String? = nil, detail: String? = nil) {
        self.error = error
        self.reasonCode = reasonCode
        self.detail = detail
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        error = try container.decodeIfPresent(String.self, forKey: .error)
        reasonCode = try container.decodeIfPresent(String.self, forKey: .reasonCode)
        detail = try container.decodeIfPresent(String.self, forKey: .detail)
        guard error != nil || reasonCode != nil || detail != nil else {
            throw DecodingError.dataCorrupted(.init(
                codingPath: decoder.codingPath,
                debugDescription: "missing error envelope fields"
            ))
        }
    }
}

enum JournalDevicesClientError: Error, Equatable {
    case invalidURL
    case notReady
    case server(JournalDevicesErrorEnvelope)
    case serverStatus(Int)
    case transport(String)
    case decoding
}

struct JournalDevicesClient: Sendable, JournalDevicesClientProtocol {
    private enum Route {
        static let devices = "/app/network/api/devices"
        static let pairStart = "/app/network/pair-start"
        static let nonceStatus = "/app/network/api/pair/nonce-status"
        static let unpair = "/app/network/unpair"
    }

    private struct EmptyRequest: Encodable, Sendable {}
    private struct UnpairRequest: Encodable, Sendable {
        var fingerprint: String
    }

    private let baseURL: String
    private let dataLoader: (@Sendable (URLRequest) async throws -> (Data, URLResponse))?
    private let session: URLSession

    init(
        baseURL: String = "http://127.0.0.1:5015",
        sessionConfiguration: URLSessionConfiguration? = nil,
        dataLoader: (@Sendable (URLRequest) async throws -> (Data, URLResponse))? = nil
    ) {
        self.dataLoader = dataLoader
        self.baseURL = baseURL.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        let config = sessionConfiguration ?? .default
        self.session = URLSession(configuration: config)
    }

    func listDevices() async throws -> [DeviceRow] {
        let request = try buildRequest(path: Route.devices, method: "GET", timeout: 5)
        let response = try await perform(request, expectedPath: Route.devices, as: DevicesListResponse.self)
        return response.devices
    }

    func startPairing() async throws -> PairStartResponse {
        let request = try buildJSONRequest(
            path: Route.pairStart,
            body: EmptyRequest(),
            timeout: 20
        )
        return try await perform(request, expectedPath: Route.pairStart, as: PairStartResponse.self)
    }

    func nonceStatus(nonce: String) async throws -> NonceStatusResponse {
        guard var components = URLComponents(string: "\(baseURL)\(Route.nonceStatus)") else {
            throw JournalDevicesClientError.invalidURL
        }
        components.queryItems = [URLQueryItem(name: "nonce", value: nonce)]
        guard let url = components.url else {
            throw JournalDevicesClientError.invalidURL
        }
        var request = URLRequest(url: url)
        request.httpMethod = "GET"
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        request.timeoutInterval = 5
        return try await perform(request, expectedPath: Route.nonceStatus, as: NonceStatusResponse.self)
    }

    func unpairDevice(fingerprint: String) async throws -> UnpairResponse {
        let request = try buildJSONRequest(
            path: Route.unpair,
            body: UnpairRequest(fingerprint: fingerprint),
            timeout: 5
        )
        return try await perform(request, expectedPath: Route.unpair, as: UnpairResponse.self)
    }

    private func buildRequest(path: String, method: String, timeout: TimeInterval) throws -> URLRequest {
        guard let url = URL(string: "\(baseURL)\(path)") else {
            throw JournalDevicesClientError.invalidURL
        }
        var request = URLRequest(url: url)
        request.httpMethod = method
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        request.timeoutInterval = timeout
        return request
    }

    private func buildJSONRequest<Body: Encodable>(
        path: String,
        body: Body,
        timeout: TimeInterval
    ) throws -> URLRequest {
        var request = try buildRequest(path: path, method: "POST", timeout: timeout)
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys]
        request.httpBody = try encoder.encode(body)
        return request
    }

    private func perform<Response: Decodable>(
        _ request: URLRequest,
        expectedPath: String,
        as responseType: Response.Type
    ) async throws -> Response {
        do {
            let data: Data
            let response: URLResponse
            if let dataLoader { (data, response) = try await dataLoader(request) }
            else { (data, response) = try await session.data(for: request) }
            return try decodeResponse(data: data, response: response, expectedPath: expectedPath, as: responseType)
        } catch is CancellationError {
            throw CancellationError()
        } catch let error as JournalDevicesClientError {
            throw error
        } catch {
            throw JournalDevicesClientError.transport(Self.transportDescription(error))
        }
    }

    private func decodeResponse<Response: Decodable>(
        data: Data,
        response: URLResponse,
        expectedPath: String,
        as responseType: Response.Type
    ) throws -> Response {
        guard let http = response as? HTTPURLResponse else {
            throw JournalDevicesClientError.notReady
        }
        if (300...399).contains(http.statusCode) {
            throw JournalDevicesClientError.notReady
        }
        if response.url?.path != expectedPath {
            throw JournalDevicesClientError.notReady
        }
        guard Self.hasJSONBody(data) else {
            throw JournalDevicesClientError.notReady
        }

        let decoder = JSONDecoder()
        guard (200...299).contains(http.statusCode) else {
            if let envelope = try? decoder.decode(JournalDevicesErrorEnvelope.self, from: data) {
                throw JournalDevicesClientError.server(envelope)
            }
            throw JournalDevicesClientError.serverStatus(http.statusCode)
        }

        do {
            return try decoder.decode(responseType, from: data)
        } catch {
            throw JournalDevicesClientError.decoding
        }
    }

    private static func hasJSONBody(_ data: Data) -> Bool {
        guard !data.isEmpty else { return false }
        return (try? JSONSerialization.jsonObject(with: data)) != nil
    }

    private static func transportDescription(_ error: Error) -> String {
        let nsError = error as NSError
        return "\(nsError.domain):\(nsError.code)"
    }
}

private extension KeyedDecodingContainer {
    func decodeLossyStringIfPresent(forKey key: Key) throws -> String? {
        if let value = try? decodeIfPresent(String.self, forKey: key) {
            return value
        }
        if let value = try? decodeIfPresent(Int64.self, forKey: key) {
            return String(value)
        }
        if let value = try? decodeIfPresent(Double.self, forKey: key) {
            return String(value)
        }
        if let value = try? decodeIfPresent(Bool.self, forKey: key) {
            return String(value)
        }
        return nil
    }
}
