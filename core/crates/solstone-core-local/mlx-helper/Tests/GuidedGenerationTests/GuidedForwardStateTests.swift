// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
import Foundation
import HelperServer
import HummingbirdTesting
import MLX
import MLXNN
import MLXLMCommon
@testable import GuidedGeneration
import Testing

private struct TraceByteTokenizer: MLXLMCommon.Tokenizer {
    func encode(text: String, addSpecialTokens: Bool) -> [Int] { text.utf8.map(Int.init) }
    func decode(tokenIds: [Int], skipSpecialTokens: Bool) -> String {
        String(decoding: tokenIds.filter { $0 != 255 }.map(UInt8.init), as: UTF8.self)
    }
    func convertTokenToId(_ token: String) -> Int? {
        if token == "<eos>" { return 255 }
        return token.utf8.count == 1 ? token.utf8.first.map(Int.init) : nil
    }
    func convertIdToToken(_ id: Int) -> String? {
        if id == 255 { return "<eos>" }
        return id >= 0 && id < 255 ? String(format: "<0x%02X>", id) : nil
    }
    var bosToken: String? { nil }
    var eosToken: String? { "<eos>" }
    var unknownToken: String? { nil }
    func applyChatTemplate(
        messages: [[String: any Sendable]], tools: [[String: any Sendable]]?,
        additionalContext: [String: any Sendable]?
    ) throws -> [Int] { [] }
}

private struct UnusedTraceProcessor: UserInputProcessor {
    func prepare(input: UserInput) async throws -> LMInput {
        fatalError("test supplies the prepared input directly")
    }
}

private let traceStateKey = LMOutput.Key<Int>("test.trace.hash")

// The state depends on every prior token in order. Losing even a syntactic byte
// changes subsequent state and logits, despite the grammar keeping JSON valid.
private final class TraceLanguageModel: Module, LanguageModel {
    var forwardPasses = 0
    var forwarded: [Int] = []
    var hashes: [Int] = []
    var perTokenLogits: [[Float]] = []
    var preparedPrefixLength = 0
    var caches: [KVCache] = []

    func prepare(
        _ input: LMInput, cache: [KVCache], state: LMOutput.State?, prefill: PrefillParameters
    ) throws -> PrepareResult {
        if preparedPrefixLength > 0 {
            _ = callAsFunction(input.text[text: 0..<preparedPrefixLength][text: .newAxis], cache: cache, state: state)
            return .tokens(input.text[text: preparedPrefixLength...])
        }
        return .tokens(input.text)
    }

    func newCache(parameters: GenerateParameters?) throws -> [KVCache] {
        caches = [KVCacheSimple()]
        return caches
    }

    func callAsFunction(
        _ input: LMInput.Text, cache: [KVCache]?, state: LMOutput.State?
    ) -> LMOutput {
        forwardPasses += 1
        var hash = state?[traceStateKey] ?? hashes.last ?? 0
        let positions = input.tokens.size
        let dummy = MLXArray.zeros([1, 1, positions, 1])
        if let cache { _ = cache[0].update(keys: dummy, values: dummy) }
        var rows: [Float] = []
        for id in input.tokens.flattened().asArray(Int32.self).map(Int.init) {
            hash = (hash * 257 + id + 1) % 1_000_003
            forwarded.append(id)
            hashes.append(hash)
            var row = (0..<256).map { -Float($0) / 1000 - 100 }
            row[65] = Float(hash % 97) / 100 + 10
            row[66] = Float(hash % 89) / 100 + 9
            row[255] = 20
            perTokenLogits.append(row)
            rows += row
        }
        var nextState = LMOutput.State()
        nextState[traceStateKey] = hash
        return LMOutput(
            logits: MLXArray(rows).reshaped(1, rows.count / 256, 256), state: nextState)
    }
}

private final class TokenBox {
    var prompt: [Int] = []
    var emitted: [Int] = []
}
private struct TraceTokenProcessor: LogitProcessor {
    let box: TokenBox
    mutating func prompt(_ prompt: MLXArray) {
        box.prompt = prompt.asArray(Int32.self).map(Int.init)
    }
    func process(logits: MLXArray) -> MLXArray { logits }
    mutating func didSample(token: MLXArray) { box.emitted.append(token.item(Int.self)) }
}

private final class ReplayTraceSampler: LogitSampler {
    let tokens: [Int]
    var index = 0
    init(_ tokens: [Int]) { self.tokens = tokens }
    func sample(logits: MLXArray) -> MLXArray {
        precondition(index < tokens.count, "replay consumed beyond supplied sequence")
        defer { index += 1 }
        return MLXArray([Int32(tokens[index])])
    }
}


/// Picks the best token other than `avoid`, so an injected sampler is visible
/// in the output.
private struct AvoidingSampler: LogitSampler {
    let avoid: Int
    func sample(logits: MLXArray) -> MLXArray {
        let positions = MLXArray(Array(Int32(0) ..< Int32(logits.dim(-1))))
        return argMax(which(positions .== MLXArray(Int32(avoid)), MLXArray(-Float.infinity), logits), axis: -1)
    }
}

/// Greedy over non-whitespace bytes. The trace model prefers low byte values,
/// and JSON grammars allow whitespace between tokens, so plain argmax would
/// emit tabs until the token budget runs out.
private struct NoWhitespaceSampler: LogitSampler {
    func sample(logits: MLXArray) -> MLXArray {
        let positions = MLXArray(Array(Int32(0) ..< Int32(logits.dim(-1))))
        var blocked = positions .== MLXArray(Int32(9))
        for byte in [10, 13, 32] { blocked = logicalOr(blocked, positions .== MLXArray(Int32(byte))) }
        return argMax(which(blocked, MLXArray(-Float.infinity), logits), axis: -1)
    }
}

/// Pushes one token far down, so an injected processor is visible.
private struct SuppressingProcessor: LogitProcessor {
    let token: Int
    mutating func prompt(_ prompt: MLXArray) {}
    func process(logits: MLXArray) -> MLXArray {
        let positions = MLXArray(Array(Int32(0) ..< Int32(logits.dim(-1))))
        return logits + which(positions .== MLXArray(Int32(token)), MLXArray(Float32(-1000)), MLXArray(Float32(0)))
    }
    mutating func didSample(token: MLXArray) {}
}

private struct Trace {
    var output = ""
    var forwarded: [Int] = []
    var hashes: [Int] = []
    var perTokenLogits: [[Float]] = []
    var emitted: [Int] = []
    var passes = 0
    var count = 0
}

/// Run the loop on the trace model over `grammar` and return what it saw.
private func traceRun(
    grammar: String? = nil, jsonSchema: String? = nil,
    feed: GuidedGenerationLoop.ForcedTokenFeed, pipeline: Bool = false,
    sampler: any LogitSampler = ArgMaxSampler(), processor: (any LogitProcessor)? = nil
) throws -> Trace {
    let tokenizer = TraceByteTokenizer()
    let vocabulary = (0..<256).map { String(format: "<0x%02X>", $0) }
    let grammarTokenizer = try GrammarTokenizer(
        vocab: vocabulary, vocabType: .byteFallback, eosTokenId: 255)
    let constraint: GrammarConstraint
    if let jsonSchema {
        constraint = try GrammarConstraint(
            tokenizer: grammarTokenizer, jsonSchema: jsonSchema, fastForward: true,
            hostTokenizer: tokenizer)
    } else {
        constraint = try GrammarConstraint(
            tokenizer: grammarTokenizer, grammar: grammar!, fastForward: true,
            hostTokenizer: tokenizer)
    }
    let model = TraceLanguageModel()
    let context = ModelContext(
        configuration: ModelConfiguration(id: "trace-model"), model: model,
        processor: UnusedTraceProcessor(), tokenizer: tokenizer)
    let box = TokenBox()
    let history = TraceTokenProcessor(box: box)
    var trace = Trace()
    var output = ""
    // The trace processor records history; an extra processor, if any, runs
    // through a composite so both see every token.
    var composite: any LogitProcessor = history
    if let processor { composite = PairProcessor(first: history, second: processor) }
    trace.count = try GuidedGenerationLoop.run(
        input: LMInput(tokens: MLXArray([Int32(1), 2, 3])), context: context,
        constraint: constraint, maxTokens: 128, vocabSize: 256,
        logitProcessor: composite, logitSampler: sampler,
        forcedTokenFeed: feed, pipelineDispatch: pipeline
    ) { output += $0; return true }
    trace.output = output
    trace.forwarded = model.forwarded
    trace.hashes = model.hashes
    trace.perTokenLogits = model.perTokenLogits
    trace.emitted = box.emitted
    trace.passes = model.forwardPasses
    return trace
}

private struct PairProcessor: LogitProcessor {
    var first: any LogitProcessor
    var second: any LogitProcessor
    mutating func prompt(_ prompt: MLXArray) { first.prompt(prompt); second.prompt(prompt) }
    func process(logits: MLXArray) -> MLXArray { second.process(logits: first.process(logits: logits)) }
    mutating func didSample(token: MLXArray) { first.didSample(token: token); second.didSample(token: token) }
}

private let forcedGrammar = #"root ::= "{\"abcdefghijklmnopqrstuv\":\"" ("A" | "B") "\"}""#

@Suite(.serialized)
struct GuidedForwardStateTests {
    @Test(arguments: [false, true], [0, 2])
    func emittedPrefixEqualsForwardedPrefix(fastForward: Bool, preparedPrefixLength: Int) throws {
        try Stream.withNewDefaultStream(device: .cpu) {
            let tokenizer = TraceByteTokenizer()
            let vocabulary = (0..<256).map { String(format: "<0x%02X>", $0) }
            let grammarTokenizer = try GrammarTokenizer(
                vocab: vocabulary, vocabType: .byteFallback, eosTokenId: 255)
            let grammar = #"root ::= "{\"abcdefghijklmnopqrstuv\":\"" ("A" | "B") "\"}""#
            let constraint = try GrammarConstraint(
                tokenizer: grammarTokenizer, grammar: grammar, fastForward: fastForward,
                hostTokenizer: tokenizer)
            let model = TraceLanguageModel()
            model.preparedPrefixLength = preparedPrefixLength
            let context = ModelContext(
                configuration: ModelConfiguration(id: "trace-model"), model: model,
                processor: UnusedTraceProcessor(), tokenizer: tokenizer)
            let prompt = [Int32(1), 2, 3]
            let input = LMInput(tokens: MLXArray(prompt))
            let box = TokenBox()
            let sink = GuidedGenerationDiagnosticSink()
            var output = ""
            let count = try GuidedGenerationDiagnosticSink.$current.withValue(sink) {
                try GuidedGenerationLoop.run(
                    input: input, context: context, constraint: constraint,
                    maxTokens: 128, vocabSize: 256,
                    logitProcessor: TraceTokenProcessor(box: box), logitSampler: ArgMaxSampler()
                ) { output += $0; return true }
            }
            #expect(sink.grammarTerminated)
            #expect(count == box.emitted.count)
            #expect(box.prompt == prompt.map(Int.init))
            #expect(output == tokenizer.decode(tokenIds: box.emitted, skipSpecialTokens: false))
            if fastForward { #expect(!sink.fastForwardTokenIDs.isEmpty) }
            else { #expect(sink.fastForwardTokenIDs.isEmpty) }

            // This pins the original defect directly: previously every sampled
            // byte just before a nonempty FF suffix was absent from forwarded.
            #expect(model.forwarded == prompt.map(Int.init) + box.emitted)
            #expect(model.caches.map(\.offset) == [prompt.count + box.emitted.count])

            // Ordinary TokenIterator gets the SAME tokenization by replaying
            // the observed IDs; comparing only decoded strings would miss BPE
            // segmentation differences. Its one-step lookahead forwards each
            // returned token before sampling the following one.
            let replayModel = TraceLanguageModel()
            var iterator = try TokenIterator(
                input: input, model: replayModel, processor: nil,
                sampler: ReplayTraceSampler(box.emitted + [255]), maxTokens: count)
            var replayed: [Int] = []
            for _ in 0..<count {
                if let token = iterator.next() { replayed.append(token) }
            }
            #expect(replayed == box.emitted)
            #expect(replayModel.forwarded == model.forwarded)
            #expect(replayModel.hashes == model.hashes)
            #expect(replayModel.perTokenLogits == model.perTokenLogits)
        }
    }

    /// Batched, sequential and pipelined feeding must give the model the
    /// same token history, the same state, the same logits at every
    /// position, and the processor the same penalty history.
    @Test func batchedSequentialAndPipelinedFeedsAgree() throws {
        try Stream.withNewDefaultStream(device: .cpu) {
            let sequential = try traceRun(grammar: forcedGrammar, feed: .sequential)
            let batched = try traceRun(grammar: forcedGrammar, feed: .batched)
            let pipelined = try traceRun(grammar: forcedGrammar, feed: .batched, pipeline: true)
            #expect(batched.output == sequential.output)
            #expect(batched.forwarded == sequential.forwarded)
            #expect(batched.hashes == sequential.hashes)
            #expect(batched.perTokenLogits == sequential.perTokenLogits)
            #expect(batched.emitted == sequential.emitted)
            // Pipelining dispatches each sampled token's pass before reading it
            // back, so the final stop token is forwarded too, and nothing else.
            let n = sequential.forwarded.count
            #expect(pipelined.output == sequential.output)
            #expect(pipelined.emitted == sequential.emitted)
            #expect(pipelined.forwarded.count <= n + 1)
            #expect(Array(pipelined.forwarded.prefix(n)) == sequential.forwarded)
            #expect(Array(pipelined.hashes.prefix(n)) == sequential.hashes)
            #expect(Array(pipelined.perTokenLogits.prefix(n)) == sequential.perTokenLogits)
            // Batching is the point: fewer forward passes than tokens fed.
            #expect(batched.passes < sequential.passes)
            #expect(sequential.forwarded == [1, 2, 3] + sequential.emitted)
        }
    }

    /// The injected sampler and processor decide the token; the loop does not
    /// silently fall back to argmax.
    @Test func injectedSamplerAndProcessorAreHonoured() throws {
        try Stream.withNewDefaultStream(device: .cpu) {
            let greedy = try traceRun(grammar: forcedGrammar, feed: .batched)
            #expect(greedy.output.hasSuffix(#""A"}"#))
            let sampled = try traceRun(
                grammar: forcedGrammar, feed: .batched, sampler: AvoidingSampler(avoid: 65))
            #expect(sampled.output.hasSuffix(#""B"}"#))
            let processed = try traceRun(
                grammar: forcedGrammar, feed: .batched, processor: SuppressingProcessor(token: 65))
            #expect(processed.output.hasSuffix(#""B"}"#))
        }
    }

    /// Fields are generated in the schema's own property order. A runner that
    /// re-serialises the schema with sorted keys breaks this.
    @Test func schemaPropertyOrderIsGenerationOrder() throws {
        try Stream.withNewDefaultStream(device: .cpu) {
            let schema = #"{"type":"object","properties":{"z":{"type":"boolean"},"a":{"type":"boolean"},"m":{"type":"boolean"}},"required":["z","a","m"],"additionalProperties":false}"#
            let trace = try traceRun(jsonSchema: schema, feed: .batched, sampler: NoWhitespaceSampler())
            let compact = trace.output.filter { !$0.isWhitespace }
            let z = compact.range(of: #""z":"#), a = compact.range(of: #""a":"#), m = compact.range(of: #""m":"#)
            #expect(z != nil && a != nil && m != nil)
            if let z, let a, let m {
                #expect(z.lowerBound < a.lowerBound && a.lowerBound < m.lowerBound)
            }
        }
    }

    /// The GPU expansion of the packed mask equals the CPU float expansion,
    /// including logit positions beyond the grammar vocabulary.
    @Test(arguments: [(256, 256), (250, 256), (256, 240), (70, 64)])
    func packedMaskMatchesFloatExpansion(vocabSize: Int, logitDim: Int) throws {
        try Stream.withNewDefaultStream(device: .cpu) {
            var generator = SystemRandomNumberGenerator()
            let words = (vocabSize + 31) / 32
            let raw = (0 ..< words).map { _ in Int32(bitPattern: UInt32.random(in: 0 ... UInt32.max, using: &generator)) }
            let mask = MaskResult(mask: raw, isTerminated: false, needsApply: true)
            let floats = GuidedGenerationLoop.buildMaskArray(for: mask, vocabSize: vocabSize, logitDim: logitDim)!
            let allowed = PackedTokenMask(vocabSize: vocabSize, logitDim: logitDim).allowed(mask)!
            #expect(allowed.asArray(Bool.self) == (floats .== 0).asArray(Bool.self))
        }
    }
}

private actor OrderedBackend: InferenceBackend {
    let generator = StructuredGenerator()
    func tokenize(_ content: String) async throws -> [Int] { [] }
    func complete(_ request: ChatRequest) async throws -> Completion {
        let schema = try #require(request.schemaText)
        let result = try Stream.withNewDefaultStream(device: .cpu) {
            let context = ModelContext(
                configuration: ModelConfiguration(id: "trace-model"), model: TraceLanguageModel(),
                processor: UnusedTraceProcessor(), tokenizer: TraceByteTokenizer())
            return try generator.complete(
                input: LMInput(tokens: MLXArray([Int32(1), 2, 3])), context: context,
                schema: schema, maxTokens: request.max_tokens ?? 128,
                // Plain argmax: the trace model prefers whitespace bytes, and the
                // helper's grammar admits none outside its fixed separators.
                processor: nil, sampler: ArgMaxSampler())
        }
        return Completion(model: "test", text: result.text, finish: result.reachedLimit ? .length : .stop,
                          promptTokens: 3, completionTokens: result.tokens,
                          promptSeconds: result.promptSeconds, decodeSeconds: result.decodeSeconds)
    }
}

@Suite(.serialized)
struct StructuredHTTPTests {
    @Test func schemaOrderAndFreshMatcherSurviveHTTP() async throws {
        let state = ModelState()
        await state.ready(OrderedBackend())
        let app = makeApplication(options: try Options(arguments: ["--model", "/model"]), state: state)
        let first = #"{"type":"object","properties":{"z":{"type":"boolean"},"a":{"type":"boolean"},"m":{"type":"object","properties":{"y":{"type":"boolean"},"b":{"type":"boolean"}},"required":["y","b"],"additionalProperties":false}},"required":["z","a","m"],"additionalProperties":false}"#
        let second = #"{"type":"object","properties":{"a":{"type":"boolean"},"z":{"type":"boolean"},"m":{"type":"object","properties":{"b":{"type":"boolean"},"y":{"type":"boolean"}},"required":["b","y"],"additionalProperties":false}},"required":["a","z","m"],"additionalProperties":false}"#
        try await app.test(.router) { client in
            // Exhaust inside a forced span, then reuse its compiled grammar.
            for (schema, limit, keys) in [(first, 2, [String]()), (first, 128, ["z", "a", "m", "y", "b"]),
                                          (second, 128, ["a", "z", "m", "b", "y"]),
                                          (first, 128, ["z", "a", "m", "y", "b"])] {
                let body = #"{"messages":[{"role":"user","content":"test"}],"max_tokens":\#(limit),"response_format":{"type":"json_schema","json_schema":{"strict":true,"schema":\#(schema)}}}"#
                try await client.execute(uri: "/v1/chat/completions", method: .post,
                                         headers: [.contentType: "application/json"], body: .init(string: body)) { response in
                    #expect(response.status == .ok)
                    let root = try #require(JSONSerialization.jsonObject(with: Data(response.body.readableBytesView)) as? [String: Any])
                    let choices = try #require(root["choices"] as? [[String: Any]])
                    let message = try #require(choices[0]["message"] as? [String: String])
                    let text = try #require(message["content"])
                    let usage = try #require(root["usage"] as? [String: Int])
                    #expect(usage["prompt_tokens"] == 3)
                    // Byte tokenizer: every emitted byte is one completion token,
                    // including tokens supplied by grammar fast-forwarding.
                    #expect(usage["completion_tokens"] == text.utf8.count)
                    if limit == 2 {
                        #expect(choices[0]["finish_reason"] as? String == "length")
                        #expect(text == "{\"")
                        #expect(usage["completion_tokens"] == 2)
                    } else {
                        #expect(choices[0]["finish_reason"] as? String == "stop")
                        #expect(!text.contains("\n") && !text.contains("\t"))
                        var positions: [String.Index] = []
                        for key in keys {
                            positions.append(try #require(text.range(of: "\"" + key + "\":" )).lowerBound)
                        }
                        #expect(positions == positions.sorted())
                    }
                }
            }
        }
    }
    @Test func unicodeSchemaCacheIdentityUsesOriginalBytes() async throws {
        let composed = "\u{00e9}"
        let decomposed = "e\u{0301}"
        #expect(composed == decomposed)
        #expect(Data(composed.utf8) != Data(decomposed.utf8))
        let state = ModelState()
        await state.ready(OrderedBackend())
        let app = makeApplication(options: try Options(arguments: ["--model", "/model"]), state: state)
        try await app.test(.router) { client in
            for key in [composed, decomposed, composed] {
                let schema = #"{"type":"object","properties":{"\#(key)":{"type":"boolean"}},"required":["\#(key)"],"additionalProperties":false}"#
                let body = #"{"messages":[{"role":"user","content":"test"}],"response_format":{"type":"json_schema","json_schema":{"strict":true,"schema":\#(schema)}}}"#
                try await client.execute(uri: "/v1/chat/completions", method: .post,
                                         headers: [.contentType: "application/json"], body: .init(string: body)) { response in
                    #expect(response.status == .ok)
                    let root = try #require(JSONSerialization.jsonObject(with: Data(response.body.readableBytesView)) as? [String: Any])
                    let choices = try #require(root["choices"] as? [[String: Any]])
                    let message = try #require(choices[0]["message"] as? [String: String])
                    let text = try #require(message["content"])
                    let object = try #require(JSONSerialization.jsonObject(with: Data(text.utf8)) as? [String: Bool])
                    let actual = try #require(object.keys.first)
                    #expect(Data(actual.utf8) == Data(key.utf8))
                }
            }
        }
    }

}

private final class WeakCachedGrammar {
    weak var value: GrammarConstraint?
    init(_ value: GrammarConstraint) { self.value = value }
}

@inline(never)
private func weakCachedGrammars(_ generator: StructuredGenerator) throws -> [Data: WeakCachedGrammar] {
    let storage = try #require(Mirror(reflecting: generator).children.first { $0.label == "compiled" })
    let roots = try #require(storage.value as? [Data: GrammarConstraint])
    return roots.mapValues(WeakCachedGrammar.init)
}

@Test func grammarCacheEvictsAtEight() throws {
    try Stream.withNewDefaultStream(device: .cpu) {
        let generator = StructuredGenerator()
        let tokenizer = TraceByteTokenizer()
        let context = ModelContext(
            configuration: ModelConfiguration(id: "trace-model"), model: TraceLanguageModel(),
            processor: UnusedTraceProcessor(), tokenizer: tokenizer)
        var first: WeakCachedGrammar?
        for i in 0..<9 {
            let schema = "{\"title\":\"cache-\(i)\",\"type\":\"boolean\"}"
            _ = try generator.complete(
                input: LMInput(tokens: MLXArray([Int32(1), 2, 3])), context: context,
                schema: schema, maxTokens: 128, processor: nil, sampler: NoWhitespaceSampler())
            if i == 0 {
                first = try #require(weakCachedGrammars(generator)[Data(schema.utf8)])
                #expect(first?.value != nil)
            }
        }
        #expect(first?.value == nil)
        let remaining = try weakCachedGrammars(generator)
        #expect(remaining.count == 8)
        #expect(remaining.values.allSatisfy { $0.value != nil })
    }
}
