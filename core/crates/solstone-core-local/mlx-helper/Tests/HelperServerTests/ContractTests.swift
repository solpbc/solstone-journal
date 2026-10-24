// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
import Foundation
import Hummingbird
import HummingbirdTesting
import Testing
import HelperServer

private func decode(_ body: String) throws -> ChatRequest {
    try ChatRequest.decode(Data(body.utf8))
}

private let productionBody = #"""
{"model":"local/qwen3.5-4b","messages":[{"role":"system","content":"Describe the screen."},{"role":"user","content":[{"type":"text","text":"Read it."},{"type":"image_url","image_url":{"url":"data:image/png;base64,AQID"}}]}],"temperature":0.2,"max_tokens":512,"top_p":0.8,"top_k":20,"min_p":0,"presence_penalty":1.5,"stream":false,"chat_template_kwargs":{"enable_thinking":false}}
"""#

private actor StubBackend: InferenceBackend {
    var calls = 0
    func tokenize(_ content: String) -> [Int] { [content.count, 17] }
    func complete(_ request: ChatRequest) -> Completion {
        calls += 1
        return Completion(model: "local/qwen3.5-4b", text: "result", finish: .stop,
                          promptTokens: 8, completionTokens: 2, promptSeconds: 0.12, decodeSeconds: 0.04)
    }
}

private actor ConcurrencyProbe {
    var active = 0
    var peak = 0
    func enter() { active += 1; peak = max(peak, active) }
    func leave() { active -= 1 }
}

struct ContractTests {
    @Test func productionRequest() throws {
        let request = try decode(productionBody)
        #expect(request.messages.map(\.role) == ["system", "user"])
        #expect(request.temperature == 0.2)
        #expect(request.max_tokens == 512)
        #expect(request.top_p == 0.8)
        #expect(request.top_k == 20)
        #expect(request.min_p == 0)
        #expect(request.presence_penalty == 1.5)
        #expect(request.chat_template_kwargs?.enable_thinking == false)
        let parts = PreparedMessages(request.messages)
        let content = try #require(parts.messages[1]["content"] as? [[String: any Sendable]])
        #expect(content.compactMap { $0["type"] as? String } == ["text", "image"])
        #expect(parts.images == [Data([1, 2, 3])])
    }

    @Test func interleavedContentKeepsPositionsAndImages() throws {
        let r = try decode(#"{"messages":[{"role":"user","content":[{"type":"image_url","image_url":{"url":"data:image/png;base64,AQ=="}},{"type":"text","text":"between"},{"type":"image_url","image_url":{"url":"data:image/jpeg;base64,Ag=="}},{"type":"text","text":"last"}]}]}"#)
        let p = PreparedMessages(r.messages)
        let content = try #require(p.messages[0]["content"] as? [[String: any Sendable]])
        #expect(content.compactMap { $0["type"] as? String } == ["image", "text", "image", "text"])
        #expect(content.compactMap { $0["text"] as? String } == ["between", "last"])
        #expect(p.images == [Data([1]), Data([2])])
    }

    @Test func dataURLsOnly() throws {
        #expect(try ContentPart.decodeDataURL("data:image/png;base64,AQID") == Data([1, 2, 3]))
        for url in ["https://example.org/a.png", "file:///a.png", "data:text/plain;base64,AQID", "data:image/png,raw", "data:image/png;base64,!!"] {
            #expect(throws: RequestError.self) { try ContentPart.decodeDataURL(url) }
        }
    }

    @Test func bindAndCacheValidation() throws {
        for host in ["0.0.0.0", "::", "192.0.2.1", "example.org", "localhost"] {
            #expect(throws: RequestError.self) { try Options(arguments: ["--model", "/model", "--host", host]) }
        }
        for host in ["127.0.0.1", "::1"] {
            #expect(try Options(arguments: ["--model", "/model", "--host", host]).host == host)
        }
        for value in ["0", "-1", "oops", String(Int.max)] {
            #expect(throws: RequestError.self) { try Options(arguments: ["--model", "/model", "--cache-limit-mb", value]) }
        }
    }

    @Test func contextBudgetUsesPreparedLength() throws {
        let r = try decode(productionBody)
        #expect(try r.generationBudget(promptTokens: 2100, context: 2200) == 100)
        #expect(try r.generationBudget(promptTokens: 2100, context: 16384) == 512)
        #expect(throws: RequestError.self) { try r.generationBudget(promptTokens: 2200, context: 2200) }
    }

    @Test func finishMappingAndResponse() throws {
        #expect(try FinishReason.map(stopped: true, reachedLimit: false) == .stop)
        #expect(try FinishReason.map(stopped: false, reachedLimit: true) == .length)
        #expect(throws: RequestError.self) { try FinishReason.map(stopped: false, reachedLimit: false) }
        let data = try JSONEncoder().encode(Completion(model: "test", text: "quoted \"value\"", finish: .length,
            promptTokens: 100, completionTokens: 20, promptSeconds: 0.2, decodeSeconds: 0.3))
        let root = try #require(JSONSerialization.jsonObject(with: data) as? [String: Any])
        #expect(root["object"] as? String == "chat.completion")
        let choices = try #require(root["choices"] as? [[String: Any]])
        #expect(choices[0]["finish_reason"] as? String == "length")
        #expect((choices[0]["message"] as? [String: String])?["content"] == "quoted \"value\"")
        #expect((root["usage"] as? [String: Int])?["total_tokens"] == 120)
        #expect((root["timings"] as? [String: Double])?["predicted_ms"] == 300)
        #expect((root["timings"] as? [String: Double])?["cache_n"] == 0)
    }

    @Test func healthPropsTokenizeAndCompletionRoutes() async throws {
        let options = try Options(arguments: ["--model", "/model", "--ctx", "4096"])
        let state = ModelState()
        let backend = StubBackend()
        let app = makeApplication(options: options, state: state)
        try await app.test(.router) { client in
            try await client.execute(uri: "/health", method: .get) { response in
                #expect(response.status == .serviceUnavailable)
                #expect(String(buffer: response.body).contains("loading model"))
            }
            await state.ready(backend)
            try await client.execute(uri: "/health", method: .get) { response in
                #expect(response.status == .ok)
            }
            try await client.execute(uri: "/props", method: .get) { response in
                let root = try #require(JSONSerialization.jsonObject(with: Data(response.body.readableBytesView)) as? [String: Any])
                #expect(root["total_slots"] as? Int == 1)
                #expect(root["n_ctx"] as? Int == 4096)
                #expect((root["default_generation_settings"] as? [String: Int])?["n_ctx"] == 4096)
            }
            try await client.execute(uri: "/tokenize", method: .post, headers: [.contentType: "application/json"], body: .init(string: #"{"content":"hello"}"#)) { response in
                #expect(response.status == .ok)
                let root = try #require(JSONSerialization.jsonObject(with: Data(response.body.readableBytesView)) as? [String: [Int]])
                #expect(root["tokens"] == [5, 17])
            }
            try await client.execute(uri: "/v1/chat/completions", method: .post, headers: [.contentType: "application/json"], body: .init(string: productionBody)) { response in
                #expect(response.status == .ok)
                let root = try #require(JSONSerialization.jsonObject(with: Data(response.body.readableBytesView)) as? [String: Any])
                #expect((root["usage"] as? [String: Int])?["total_tokens"] == 10)
            }
        }
    }

    @Test func invalidResponseFormatRefusedBeforeInference() async throws {
        let state = ModelState()
        let backend = StubBackend()
        await state.ready(backend)
        let app = makeApplication(options: try Options(arguments: ["--model", "/model"]), state: state)
        try await app.test(.router) { client in
            for format in ["null", #"{"type":"text"}"#, #"{"type":"json_schema","json_schema":{"name":"example","schema":{"type":"object"}}}"#] {
                let body = #"{"messages":[{"role":"user","content":"test"}],"response_format":\#(format)}"#
                try await client.execute(uri: "/v1/chat/completions", method: .post, headers: [.contentType: "application/json"], body: .init(string: body)) { response in
                    #expect(response.status == .badRequest)
                    let root = try #require(JSONSerialization.jsonObject(with: Data(response.body.readableBytesView)) as? [String: [String: String]])
                    #expect(root["error"]?["type"] == "invalid_request_error")
                }
            }
        }
        #expect(await backend.calls == 0)
    }

    @Test func exactSchemaBytesAndObjectFormat() throws {
        let schema = #"{ "type": "object", "properties": {"z":{"type":"boolean"},"a":{"type":"boolean"}}, "required":["z","a"], "additionalProperties":false }"#
        let body = #"{"messages":[{"role":"user","content":"a string with \"schema\":{} and } characters"}],"response_format":{"type":"json_schema","json_schema":{"strict":true,"schema":\#(schema)}}}"#
        #expect(try decode(body).schemaText == schema)
        #expect(try decode(#"{"messages":[{"role":"user","content":"test"}],"response_format":{"type":"json_object"}}"#).schemaText == #"{"type":"object"}"#)
        #expect(throws: RequestError.self) {
            try decode(#"{"messages":[{"role":"user","content":"test"}],"response_format":{"type":"json_schema","json_schema":{"strict":true,"schema":{},"schema":{}}}}"#)
        }
    }

    @Test func slotReleasedAfterErrorAndCancellation() async throws {
        let slot = FIFOSlot()
        do {
            let _: Int = try await slot.run { throw RequestError("failure") }
            Issue.record("expected failure")
        } catch is RequestError {}
        #expect(try await slot.run { 42 } == 42)
        let cancelled = Task {
            withUnsafeCurrentTask { $0?.cancel() }
            return try await slot.run { return 0 }
        }
        do {
            _ = try await cancelled.value
            Issue.record("expected cancellation")
        } catch is CancellationError {}
        #expect(try await slot.run { 43 } == 43)
    }

    @Test func slotSpansSuspensionPoints() async throws {
        let slot = FIFOSlot()
        let probe = ConcurrencyProbe()
        let results = try await withThrowingTaskGroup(of: Int.self) { group in
            for value in 0..<20 {
                group.addTask {
                    try await slot.run {
                        await probe.enter()
                        await Task.yield()
                        await probe.leave()
                        return value
                    }
                }
            }
            var values: [Int] = []
            for try await value in group { values.append(value) }
            return values
        }
        #expect(await probe.peak == 1)
        #expect(results.sorted() == Array(0..<20))
    }
}

struct MaintenanceContractTests {
    @Test func activityObservationIsReadOnlyAndSeesBusySlot() async throws {
        let slot = FIFOSlot()
        let initial = await slot.activity()
        #expect(initial.generation == 0 && initial.idle)
        let repeated = await slot.activity()
        #expect(repeated.generation == 0 && repeated.idle)
        try await slot.run {
            let busy = await slot.activity()
            #expect(busy.generation == 1 && !busy.idle)
        }
        let finished = await slot.activity()
        #expect(finished.generation == 1 && finished.idle)
        enum Failure: Error { case expected }
        do { try await slot.run { throw Failure.expected } }
        catch Failure.expected {}
        let failed = await slot.activity()
        #expect(failed.generation == 2 && failed.idle)
    }

    @Test func missingFootprintDoesNotHideRequestActivity() async throws {
        enum Failure: Error { case unavailable }
        let state = ModelState()
        await state.ready(StubBackend())
        let app = makeApplication(options: try Options(arguments: ["--model", "/model"]), state: state,
                                  footprint: { throw Failure.unavailable })
        try await app.test(.router) { client in
            try await client.execute(uri: "/maintenance", method: .get) { response in
                #expect(response.status == .ok)
                let root = try #require(JSONSerialization.jsonObject(with: Data(response.body.readableBytesView)) as? [String: Any])
                #expect(root["footprint_bytes"] == nil)
                #expect(root["activity_generation"] as? Int == 0)
                #expect(root["idle"] as? Bool == true)
            }
        }
    }

    @Test func maintenanceObservationDoesNotPreventIdle() async throws {
        let state = ModelState()
        let backend = StubBackend()
        let app = makeApplication(options: try Options(arguments: ["--model", "/model"]), state: state,
                                  footprint: { 4242 })
        try await app.test(.router) { client in
            try await client.execute(uri: "/maintenance", method: .get) { response in
                #expect(response.status == .serviceUnavailable)
            }
            await state.ready(backend)
            for _ in 0..<2 {
                try await client.execute(uri: "/maintenance", method: .get) { response in
                    #expect(response.status == .ok)
                    let root = try #require(JSONSerialization.jsonObject(with: Data(response.body.readableBytesView)) as? [String: Any])
                    #expect(root["footprint_bytes"] as? Int == 4242)
                    #expect(root["activity_generation"] as? Int == 0)
                    #expect(root["idle"] as? Bool == true)
                }
            }
            try await client.execute(uri: "/tokenize", method: .post, headers: [.contentType: "application/json"], body: .init(string: #"{"content":"hello"}"#)) { response in
                #expect(response.status == .ok)
            }
            try await client.execute(uri: "/maintenance", method: .get) { response in
                let root = try #require(JSONSerialization.jsonObject(with: Data(response.body.readableBytesView)) as? [String: Any])
                #expect(root["activity_generation"] as? Int == 1)
                #expect(root["idle"] as? Bool == true)
            }
        }
    }
}
