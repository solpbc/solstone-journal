// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
import Foundation
import Hummingbird

public protocol InferenceBackend: Sendable {
    func tokenize(_ content: String) async throws -> [Int]
    func complete(_ request: ChatRequest) async throws -> Completion
}

/// Actor reentrancy alone does not serialize inference across suspension points.
public actor FIFOSlot {
    private var busy = false
    private var waiting: [CheckedContinuation<Void, Never>] = []

    private func acquire() async {
        if !busy { busy = true; return }
        await withCheckedContinuation { waiting.append($0) }
    }

    private func release() {
        if waiting.isEmpty { busy = false }
        else { waiting.removeFirst().resume() }
    }

    public init() {}
    public func run<T: Sendable>(_ work: @Sendable () async throws -> T) async throws -> T {
        await acquire()
        defer { release() }
        try Task.checkCancellation()
        return try await work()
    }
}

public actor ModelState {
    private var backend: (any InferenceBackend)?
    public init() {}
    public func ready(_ backend: any InferenceBackend) { self.backend = backend }
    public func loaded() -> Bool { backend != nil }
    public func requireBackend() throws -> any InferenceBackend {
        guard let backend else { throw HTTPError(.serviceUnavailable, message: "loading model") }
        return backend
    }
}

private struct Props: Encodable {
    let total_slots = 1
    let n_ctx: Int
    let default_generation_settings: Settings
    struct Settings: Encodable { let n_ctx: Int }
}

private struct ErrorEnvelope: Encodable {
    let error: Detail
    struct Detail: Encodable {
        let message: String
        let type: String
        let code: String
    }
}

private func json<T: Encodable>(_ value: T, status: HTTPResponse.Status = .ok) throws -> Response {
    let data = try JSONEncoder().encode(value)
    return Response(status: status, headers: [.contentType: "application/json"], body: .init(byteBuffer: .init(bytes: data)))
}

private func failure(_ message: String, status: HTTPResponse.Status = .badRequest) throws -> Response {
    let kind = status == .badRequest ? "invalid_request_error" : "server_error"
    return try json(ErrorEnvelope(error: .init(message: message, type: kind, code: kind)), status: status)
}

private func decodeBody<T: Decodable>(_ request: Request, as: T.Type) async throws -> T {
    let buffer = try await request.body.collect(upTo: 32 * 1024 * 1024)
    let data = Data(buffer.readableBytesView)
    let decoder = JSONDecoder()
    decoder.userInfo[ChatRequest.requestBodyKey] = data
    return try decoder.decode(T.self, from: data)
}

public func makeApplication(
    options: Options, state: ModelState,
    onRunning: @escaping @Sendable () async -> Void = {}
) -> some ApplicationProtocol {
    let router = Router()
    let slot = FIFOSlot()
    router.get("health") { _, _ -> Response in
        if await state.loaded() {
            return Response(status: .ok, body: .init(byteBuffer: .init(string: "{\"status\":\"ok\"}")))
        }
        return Response(status: .serviceUnavailable, body: .init(byteBuffer: .init(string: "loading model")))
    }
    router.get("props") { _, _ -> Response in
        try json(Props(n_ctx: options.context, default_generation_settings: .init(n_ctx: options.context)))
    }
    router.post("tokenize") { request, context -> Response in
        struct Input: Decodable { let content: String }
        struct Output: Encodable { let tokens: [Int] }
        do {
            let input = try await decodeBody(request, as: Input.self)
            let backend = try await state.requireBackend()
            let tokens = try await slot.run { try await backend.tokenize(input.content) }
            return try json(Output(tokens: tokens))
        } catch let e as RequestError { return try failure(e.message) }
        catch let e as HTTPError { return try failure(e.body ?? "request refused", status: e.status) }
        catch is DecodingError { return try failure("invalid tokenize body") }
        catch { context.logger.error("tokenization failed: \(error)"); return try failure("tokenization failed", status: .internalServerError) }
    }
    router.post("v1/chat/completions") { request, context -> Response in
        do {
            let body = try await decodeBody(request, as: ChatRequest.self)
            guard body.model == nil || body.model == options.alias else {
                throw RequestError("model does not match the loaded alias")
            }
            let backend = try await state.requireBackend()
            let result = try await slot.run { try await backend.complete(body) }
            return try json(result)
        } catch let e as RequestError { return try failure(e.message) }
        catch let e as HTTPError { return try failure(e.body ?? "request refused", status: e.status) }
        catch is DecodingError { return try failure("invalid chat completion body") }
        catch { context.logger.error("generation failed: \(error)"); return try failure("generation failed", status: .internalServerError) }
    }
    return Application(
        responder: router.buildResponder(),
        configuration: .init(address: .hostname(options.host, port: options.port)),
        onServerRunning: { _ in await onRunning() }
    )
}
