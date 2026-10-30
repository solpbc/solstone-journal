// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
import Foundation
import MLXGuidedGeneration
import MLX
import MLXLMCommon

/// Access is serialized by the server's inference slot and ModelContainer.
public final class StructuredGenerator: @unchecked Sendable {
    private var tokenizer: GrammarTokenizer?
    private var compiled: [Data: GrammarConstraint] = [:]
    private var order: [Data] = []

    public init() {}

    /// Initialize model-specific vocabulary before the server becomes ready.
    public func prepare(context: ModelContext) throws {
        guard tokenizer == nil else { return }
        let vocab = MLXGuidedGeneration.TokenizerVocabExtractor.extractForGrammar(from: context.tokenizer)
        let type: VocabType
        switch vocab.vocabType {
        case .raw: type = .raw
        case .byteFallback: type = .byteFallback
        case .byteLevel: type = .byteLevel
        }
        tokenizer = try GrammarTokenizer(
            vocab: vocab.vocab, vocabType: type,
            eosTokenId: Int32(context.tokenizer.eosTokenId ?? 0))
    }

    public struct Result {
        public let text: String
        public let tokens: Int
        public let reachedLimit: Bool
        public let promptSeconds: Double
        public let decodeSeconds: Double
    }

    public func complete(input: LMInput, context: ModelContext, schema: String, maxTokens: Int,
                         processor: (any LogitProcessor)?, sampler: any LogitSampler) throws -> Result {
        defer { Stream.gpu.synchronize() }
        let (constraint, vocabSize) = try matcher(schema: schema, context: context)
        let timings = GuidedGenerationLoop.GuidedGenerationTimings()
        var text = ""
        let start = ContinuousClock.now
        let count: Int
        let reachedLimit: Bool
        do {
            count = try GuidedGenerationLoop.run(
                input: input, context: context, constraint: constraint, maxTokens: maxTokens,
                vocabSize: vocabSize, logitProcessor: processor, logitSampler: sampler,
                timings: timings
            ) { text += $0; return true }
            reachedLimit = false
        } catch GuidedGenerationError.incompleteOutput {
            // The loop flushes the partial text before reporting token exhaustion.
            count = maxTokens
            reachedLimit = true
        }
        try Task.checkCancellation()
        let elapsed = ContinuousClock.now - start
        let seconds = Double(elapsed.components.seconds) + Double(elapsed.components.attoseconds) * 1e-18
        return Result(text: text, tokens: count, reachedLimit: reachedLimit,
                      promptSeconds: timings.prefillSeconds,
                      decodeSeconds: max(0, seconds - timings.prefillSeconds))
    }

    private func matcher(schema: String, context: ModelContext) throws -> (GrammarConstraint, Int) {
        try prepare(context: context)
        let tokenizer = tokenizer!
        // String equality merges canonically equivalent Unicode spellings.
        let key = Data(schema.utf8)
        if let grammar = compiled[key] {
            return (try grammar.freshMatcher(), tokenizer.vocabSize)
        }
        // Fixed separators, no free whitespace. With xgrammar's default any
        // whitespace, the model can begin to close a list with a newline and
        // then still continue it, which lengthens lists until they run to the
        // output limit. A llama.cpp grammar never admits whitespace between a
        // value and the comma after it.
        let grammar = try GrammarConstraint(
            tokenizer: tokenizer, jsonSchema: schema, anyWhitespace: false, fastForward: true,
            hostTokenizer: context.tokenizer)
        // An arbitrary client can supply distinct schemas indefinitely.
        if order.count == 8 { compiled.removeValue(forKey: order.removeFirst()) }
        order.append(key)
        compiled[key] = grammar
        return (try grammar.freshMatcher(), tokenizer.vocabSize)
    }
}
