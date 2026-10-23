// SPDX-License-Identifier: MIT
// Copyright © 2026 Apple Inc.

import MLX
import os
import MLXLMCommon

/// Runs grammar-constrained generation with fast-forward token support.
///
/// When the grammar forces deterministic tokens (e.g. JSON structural
/// characters `{`, `}`, `,`, `:`), they're fed through the model together
/// with the sampled token that preceded them, in one forward pass.
///
/// The loop overlaps grammar mask computation (CPU) with the model forward
/// pass (GPU). After committing a token, the grammar state is ready for the
/// next mask computation. We compute it while the GPU processes the forward
/// pass, hiding the ~50us CPU cost behind the 10-100ms GPU latency.
///
/// This is a self-contained loop that has direct access to the grammar state
/// and can inject fast-forwarded tokens into both the output stream and the
/// KV cache.
public enum GuidedGenerationLoop {

    private static let logger = Logger(
        subsystem: "com.apple.FoundationModels-MLX",
        category: "GuidedGenerationLoop"
    )

    /// Result of a single generation step.
    enum StepResult {
        /// A sampled token (normal generation).
        case token(Int)
        /// A batch of tokens: the sampled token followed by fast-forward tokens.
        case tokenBatch([Int])
        /// Generation should stop (grammar accepted or error).
        case stop
    }

    /// How the sampled token and the grammar-forced tokens that follow it
    /// reach the model.
    public enum ForcedTokenFeed: Sendable {
        /// One forward pass for `[sampled] + forced`. The default.
        case batched
        /// One T_q = 1 forward pass per token, in order. Same cache history and
        /// next-token logits as `batched`, more passes. Kept as the reference
        /// for tests and for models whose attention cannot take T_q > 1 against
        /// a populated cache.
        case sequential
    }

    /// Optional per-run accounting of where a guided generation spends time.
    ///
    /// All spans are host wall-clock seconds. `prefillSeconds` runs from the
    /// start of the run until the prompt's logits are evaluated.
    /// `sampleWaitSeconds` is time blocked reading a sampled token back from
    /// the GPU, which includes any forward pass still in flight. `maskSeconds`
    /// is grammar mask computation plus its upload; `commitSeconds` is
    /// grammar commit including jump-forward tokenization; `graphSeconds` is
    /// building forward-pass graphs on the host.
    public final class GuidedGenerationTimings: @unchecked Sendable {
        public var prefillSeconds: Double = 0
        public var sampleWaitSeconds: Double = 0
        public var maskSeconds: Double = 0
        public var commitSeconds: Double = 0
        public var graphSeconds: Double = 0
        public var forwardPasses = 0
        public var sampledTokens = 0
        public var forcedTokens = 0
        /// When non-nil, receives each forward group in order: the sampled
        /// token followed by the forced tokens that were fed with it.
        public var forwardGroups: [[Int]]?

        public init(recordForwardGroups: Bool = false) {
            forwardGroups = recordForwardGroups ? [] : nil
        }
    }

    /// Runs the guided generation loop, yielding text deltas through `emit`.
    ///
    /// Overlaps grammar mask computation with GPU forward passes: after
    /// committing a token, the next mask is computed on the CPU while the
    /// model forward pass runs on the GPU.
    ///
    /// Every generated token reaches the model exactly once and in order:
    /// the KV cache after a step holds the prompt, every sampled token and
    /// every forced token. When the grammar forces tokens after a sampled
    /// one, `[sampled] + forced` go through a single forward pass.
    ///
    /// - Parameters:
    ///   - input: Prepared model input (prompt already tokenized)
    ///   - context: Model context (model, tokenizer, configuration)
    ///   - constraint: The xgrammar constraint (must have `fastForward: true`)
    ///   - maxTokens: Maximum tokens to generate
    ///   - completionReserve: Number of tokens before maxTokens at which closing
    ///     bias activates. When `tokenCount >= maxTokens - completionReserve`,
    ///     the bias nudges sampling toward JSON-closing tokens.
    ///   - hardReserve: Number of tokens before maxTokens at which a *hard*
    ///     closing zone activates: closing tokens are forced and all others
    ///     suppressed (plus an EOS penalty), more aggressively than
    ///     `completionReserve`'s soft nudge. Defaults to 0, which disables it.
    ///   - vocabSize: Number of tokens in the grammar's vocabulary. May differ
    ///     from the model's logit dimension (e.g. added special tokens beyond
    ///     the embedding size). Used to correctly interpret the grammar bitmask.
    ///   - closingBias: Pre-computed logit bias array favoring closing tokens
    ///     (from `ClosingTokenBias.compute`). Nil disables forced completion.
    ///   - whitespaceBias: Pre-computed negative logit bias array penalizing
    ///     whitespace-only tokens (from `WhitespaceTokenBias.compute`). Nil
    ///     disables whitespace suppression.
    ///   - whitespaceTokenIDs: Set of token IDs classified as whitespace-only.
    ///     Used by the run tracker to detect consecutive whitespace runs.
    ///   - diagnosticLog: When true, flush the grammar constraint's diagnostic
    ///     logs after the run completes. Defaults to false.
    ///   - logitProcessor: Optional processor (e.g. `GenerateParameters.processor()`
    ///     for repetition, presence and frequency penalties). It sees the prompt,
    ///     visits the raw logits before the grammar mask, and is told about every
    ///     sampled and every forced token, in order.
    ///   - logitSampler: Sampler applied to the masked logits, e.g.
    ///     `GenerateParameters.sampler()` for temperature, top-p, top-k and
    ///     min-p. Nil samples greedily (argmax).
    ///   - forcedTokenFeed: How sampled and forced tokens reach the model.
    ///   - pipelineDispatch: When true, the forward pass for a sampled token is
    ///     dispatched before the token is read back, as `TokenIterator` does,
    ///     so graph construction overlaps GPU sampling. Forced tokens that
    ///     follow then take a second pass, and a final stop token is forwarded
    ///     too (harmless: generation ends). Off by default.
    ///   - timings: Optional accounting of where the run spends time.
    ///   - prefill: Prompt prefill parameters (step size, chunking strategy,
    ///     progress callback). Defaults to a 512-token step with balanced
    ///     chunking.
    ///   - emit: Callback for each text delta. Return `false` to stop.
    /// - Returns: Total number of tokens generated (including FF tokens).
    /// - Throws: `GuidedGenerationError.incompleteOutput` if maxTokens is
    ///   exhausted before the grammar reaches a stop state.
    ///   `GuidedGenerationError.prematureEOS` if the model emits EOS
    ///   before the grammar accepts.
    @discardableResult
    public static func run(
        input: LMInput,
        context: ModelContext,
        constraint: GrammarConstraint,
        maxTokens: Int,
        vocabSize: Int,
        completionReserve: Int = 64,
        hardReserve: Int = 0,
        closingBias: MLXArray? = nil,
        whitespaceBias: MLXArray? = nil,
        whitespaceTokenIDs: Set<Int> = [],
        diagnosticLog: Bool = false,
        logitProcessor: (any LogitProcessor)? = nil,
        logitSampler: (any LogitSampler)? = nil,
        forcedTokenFeed: ForcedTokenFeed = .batched,
        pipelineDispatch: Bool = false,
        timings: GuidedGenerationTimings? = nil,
        prefill: PrefillParameters = .init(stepSize: PrefillParameters.defaultStepSize),
        emit: (String) -> Bool
    ) throws -> Int {
        let model = context.model
        let diagnosticSink = GuidedGenerationDiagnosticSink.current
        // This helper uses the model's ordinary, unquantized per-request caches.
        // Quantized-cache planning is package-private in the upstream library.
        let cache = try model.newCache(parameters: nil)
        var modelState: LMOutput.State?
        var processor = logitProcessor
        let sampler: any LogitSampler = logitSampler ?? ArgMaxSampler()
        let runStart = ContinuousClock.now

        // Build EOS token set
        let stopTokenIDs = Self.buildStopTokenIDs(
            tokenizer: context.tokenizer,
            configuration: context.configuration
        )

        processor?.prompt(input.text.tokens)

        // Prefill prompt and get first set of logits
        var logits: MLXArray
        switch try model.prepare(
            input, cache: cache, state: nil, prefill: prefill)
        {
        case .tokens(let tokens):
            let result = model(
                tokens[text: .newAxis], cache: cache, state: nil)
            modelState = result.state
            logits = result.logits

        case .logits(let result):
            modelState = result.state
            logits = result.logits
        }


        /// Feed `tokens` (1-D) through the model; returns the logits after the
        /// last one. Advances the cache and model state by `tokens.size`.
        func feed(_ tokens: MLXArray) -> MLXArray {
            let graphStart = ContinuousClock.now
            defer { timings?.graphSeconds += seconds(since: graphStart) }
            let pieces: [MLXArray] =
                switch forcedTokenFeed {
                case .batched: [tokens]
                case .sequential: (0 ..< tokens.size).map { tokens[$0 ..< ($0 + 1)] }
                }
            var last = logits
            for piece in pieces {
                let tokenInput = LMInput.Text(tokens: piece)
                let result = model(
                    tokenInput[text: .newAxis],
                    cache: cache.isEmpty ? nil : cache,
                    state: modelState
                )
                modelState = result.state
                last = result.logits
                timings?.forwardPasses += 1
            }
            return last
        }

        /// Start evaluating `logits` together with the cache state, as
        /// `TokenIterator` does, so cache updates never chain across steps.
        func dispatch(_ logits: MLXArray) {
            asyncEval([logits] + cache.flatMap { $0.state })
        }

        var detokenizer = context.tokenizer.makeStreamingDetokenizer()
        var tokenCount = 0
        var grammarStopped = false
        var consumerStopped = false
        var whitespaceTracker = WhitespaceRunTracker(whitespaceTokenIDs: whitespaceTokenIDs)

        // Pre-compute bias arrays used in the zone policy.
        //
        // eosPenalty: -10000 at each EOS/stop position. Used in the normal
        // zone to prevent premature EOS at structurally incomplete states,
        // and in the hard zone alongside closing token penalties.
        //
        // The EOS penalty is NOT applied in the soft zone. The grammar mask
        // ensures structural validity (EOS only appears when JSON is
        // structurally complete). Removing the penalty lets the model stop
        // when output is structurally valid but semantically short, which
        // is acceptable near the budget limit.
        let eosPenalty: MLXArray? =
            if let bias = closingBias {
                {
                    let biasLen = bias.shape[0]
                    var penalty = [Float32](repeating: 0.0, count: biasLen)
                    for eos in stopTokenIDs where eos >= 0 && eos < biasLen {
                        penalty[eos] = -10000.0
                    }
                    return MLXArray(penalty)
                }()
            } else {
                nil
            }

        let clock = ContinuousClock()
        let startInstant = clock.now
        var accumulatedText = ""

        // Logit dimension is constant across the generation; capture once so the
        // grammar mask can be expanded on the GPU against it.
        let logitDim = logits.shape[logits.ndim - 1]
        let packedMask = PackedTokenMask(vocabSize: vocabSize, logitDim: logitDim)

        /// Compute the grammar's next mask on the CPU and stage its packed
        /// words for the GPU. Runs in the shadow of a forward pass.
        func nextMask() throws -> (MaskResult, MLXArray?) {
            let maskStart = ContinuousClock.now
            defer { timings?.maskSeconds += seconds(since: maskStart) }
            let mask = try constraint.computeMask()
            return (mask, packedMask.allowed(mask))
        }

        // The first mask overlaps the prompt's forward pass.
        dispatch(logits)
        var (mask, allowed) = try nextMask()
        eval(logits)
        timings?.prefillSeconds = seconds(since: runStart)

        while tokenCount < maxTokens {
            // Cooperative cancellation: exit promptly when the enclosing Task
            // is cancelled (e.g. test timeout or user-initiated cancellation).
            try Task.checkCancellation()

            // Diagnostic: capture mask state before sampling
            if diagnosticLog {
                let snapshot = mask.mask.withUnsafeBufferPointer { buffer -> MaskSnapshot in
                    let ptr: UnsafePointer<UInt32>? =
                        mask.needsApply
                        ? UnsafeRawPointer(buffer.baseAddress!).assumingMemoryBound(
                            to: UInt32.self)
                        : nil
                    return MaskSnapshot.capture(
                        sampleMask: ptr,
                        vocabSize: vocabSize,
                        tokenIndex: tokenCount,
                        isStop: mask.isTerminated
                    )
                }
                logger.info("\(snapshot.summary())")
            }

            // Check stop from the pre-computed mask
            if mask.isTerminated {
                if diagnosticLog {
                    logger.info(
                        "[GuidedGen] Stop reason: mask.isTerminated at token \(tokenCount)")
                }
                grammarStopped = true
                break
            }

            // Zone policy for budget management:
            //
            //   Normal zone (tokenCount < maxTokens - completionReserve):
            //     No bias. The grammar mask already gates EOS on
            //     structural validity, so primitive schemas (e.g.
            //     `{"type": "integer"}`, where the grammar allows EOS
            //     after one digit) can stop naturally after one token,
            //     without a bias layer on top.
            //
            //   Soft zone (completionReserve .. hardReserve tokens left):
            //     Closing bias only (+200 EOS, +100 closing tokens). No EOS
            //     penalty. The grammar mask ensures EOS only appears when JSON
            //     is structurally valid, so removing the penalty lets the model
            //     stop naturally. May produce shorter output for unbounded
            //     schemas, which is acceptable this close to the budget.
            //
            //   Hard zone (hardReserve tokens left):
            //     Penalize all non-closing tokens (-10000) AND EOS (-10000).
            //     Forces the model to select closing tokens (}, ], ", digits)
            //     that build up JSON structure. The grammar reaches a natural
            //     stop state when JSON is complete. EOS is penalized because
            //     the grammar may allow it at intermediate valid states before
            //     all required fields are present.
            //
            // Only applied when the grammar's mask carries exclusions
            // (`needsApply == true`). When false, the grammar is in an
            // unconditional splice (all tokens forced by FF). Applying
            // bias without a grammar mask can cause EOS selection before
            // the grammar has accepted the output.
            var activeBias: MLXArray? = nil
            if mask.needsApply {
                if let bias = closingBias {
                    if hardReserve > 0 && tokenCount >= maxTokens - hardReserve {
                        // Hard zone: force closing tokens, suppress everything else.
                        var hardBias = which(bias .> 0, Float32(0.0), Float32(-10000.0))
                        if let eosPenalty {
                            hardBias = hardBias + eosPenalty
                        }
                        activeBias = hardBias
                    } else if tokenCount >= maxTokens - completionReserve {
                        // Soft zone: nudge toward closing tokens, no EOS penalty.
                        activeBias = bias
                    }
                    // Normal zone: no bias. Grammar mask + natural EOS handle
                    // termination. Intentionally leaves `activeBias == nil`.
                }
                if let wsBias = whitespaceBias, whitespaceTracker.isActive {
                    activeBias = activeBias.map { $0 + wsBias } ?? wsBias
                }
            }

            let sampled = sampleToken(
                logits: logits, allowed: allowed, closingBias: activeBias,
                logitProcessor: processor, logitSampler: sampler)

            // Pipelined dispatch: the sampled token must reach the model
            // whatever the grammar does next, so its forward pass can be
            // queued behind the sample before the token is read back.
            var pipelinedLogits: MLXArray? = nil
            if pipelineDispatch {
                asyncEval(sampled)
                let next = feed(sampled.reshaped(-1).asType(.int32))
                dispatch(next)
                pipelinedLogits = next
            }

            let waitStart = ContinuousClock.now
            let tokenId = sampled.item(Int.self)
            timings?.sampleWaitSeconds += seconds(since: waitStart)
            timings?.sampledTokens += 1

            // Track the sampled token for whitespace run detection.
            // Fast-forward tokens are NOT tracked (they are grammar-forced).
            if whitespaceBias != nil {
                _ = whitespaceTracker.record(tokenID: tokenId)
            }

            // Check EOS only when the grammar exposed a real mask
            // (`needsApply == true`). When `false` the grammar is in an
            // unconditional splice: the sampled value is irrelevant
            // because commitToken will surface the forced tokens.
            // Checking for EOS here would cause a spurious stop -- the
            // model's raw logits might have EOS as the highest value
            // even though the grammar has NOT accepted the output.
            //
            // When `needsApply` IS true: if the grammar mask allowed
            // EOS (bit = 1), the grammar considers the output
            // acceptable. If the mask did NOT allow EOS, `sampleToken`
            // set it to -inf, so no sampler can select it.
            if mask.needsApply {
                if tokenId == context.tokenizer.unknownTokenId || stopTokenIDs.contains(tokenId) {
                    if diagnosticLog {
                        logger.info(
                            "[GuidedGen] Stop reason: EOS/unk tokenId=\(tokenId) at token \(tokenCount)"
                        )
                    }
                    grammarStopped = true
                    break
                }
            }

            // Penalty history holds exactly the generated tokens; a stop
            // token never reaches it.
            processor?.didSample(token: sampled)

            // Commit to grammar
            let commitStart = ContinuousClock.now
            let commitResult = try constraint.commitToken(Int32(tokenId))
            timings?.commitSeconds += seconds(since: commitStart)

            diagnosticSink?.recordSampledToken(tokenId)
            // Yield the sampled token
            detokenizer.append(token: tokenId)
            if let text = detokenizer.next() {
                accumulatedText += text
                if !emit(text) {
                    consumerStopped = true
                    break
                }
            }
            tokenCount += 1

            if diagnosticLog {
                // Periodic progress logging (once per main loop iteration, not per FF token)
                if tokenCount % 50 == 0 {
                    let elapsed = clock.now - startInstant
                    let ms =
                        elapsed.components.seconds * 1000 + elapsed.components.attoseconds
                        / 1_000_000_000_000_000
                    let prefix = String(accumulatedText.prefix(200))
                    logger.info("[GuidedGen] token=\(tokenCount) elapsed=\(ms)ms text=\(prefix)")
                }
            }

            if commitResult.isTerminated {
                if diagnosticLog {
                    logger.info(
                        "[GuidedGen] Stop reason: commitResult.isTerminated at token \(tokenCount)"
                    )
                }
                grammarStopped = true
                break
            }

            // Fast-forward tokens. CommitResult.tokens carries ONLY the
            // jump-forward ids (the sampled token is not echoed back by
            // xgrammar). Yield them to output. The caller's `emit` stop
            // signal (`emit(text) == false`) must halt generation
            // immediately, just like on the sampled-token path above; a
            // bare `break` here would only exit the inner `for`, so
            // propagate through `shouldStopAfterFF`.
            let ffTokens: [Int32] = commitResult.tokens
            var shouldStopAfterFF = false
            for ffToken in ffTokens {
                if tokenCount >= maxTokens {
                    shouldStopAfterFF = true
                    break
                }
                processor?.didSample(token: MLXArray([ffToken]))
                diagnosticSink?.recordFastForwardToken(Int(ffToken))
                detokenizer.append(token: Int(ffToken))
                if let text = detokenizer.next() {
                    accumulatedText += text
                    if !emit(text) {
                        consumerStopped = true
                        shouldStopAfterFF = true
                        break
                    }
                }
                tokenCount += 1
            }
            if shouldStopAfterFF { break }
            timings?.forcedTokens += ffTokens.count
            timings?.forwardGroups?.append([tokenId] + ffTokens.map(Int.init))

            // Feed the model. The sampled token always goes in: without it
            // the cache would skip a token the output contains, and every
            // later logit would condition on a different sequence.
            if let pipelinedLogits {
                logits =
                    ffTokens.isEmpty ? pipelinedLogits : feed(MLXArray(ffTokens))
            } else {
                logits = feed(MLXArray([Int32(tokenId)] + ffTokens))
            }

            // Kick off GPU computation asynchronously, and compute the next
            // mask on the CPU while the GPU runs the forward pass.
            dispatch(logits)
            (mask, allowed) = try nextMask()

            // Periodically return freed buffers that cannot be reused, as
            // `TokenIterator` does.
            if tokenCount % 256 == 0 {
                MLX.Memory.clearCache()
            }
        }

        if !consumerStopped, let text = detokenizer.finish(), !text.isEmpty {
            accumulatedText += text
            _ = emit(text)
        }

        if diagnosticLog {
            // Log final generation stats
            let totalElapsed = clock.now - startInstant
            let totalMs =
                totalElapsed.components.seconds * 1000 + totalElapsed.components.attoseconds
                / 1_000_000_000_000_000
            logger.info("[GuidedGen] done tokens=\(tokenCount) elapsed=\(totalMs)ms")
        }
        // Flush any xgrammar warnings (limit exceedances, parser state)
        if diagnosticLog, let logs = constraint.flushLogs() {
            logger.warning("[GuidedGen] xgrammar logs:\n\(logs)")
        }

        diagnosticSink?.recordTermination(
            grammarTerminated: grammarStopped, generatedTokenCount: tokenCount)

        // If we exhausted maxTokens without the grammar reaching a stop state,
        // the output is structurally incomplete (e.g., truncated JSON).
        if !grammarStopped && tokenCount >= maxTokens {
            throw GuidedGenerationError.incompleteOutput
        }

        return tokenCount
    }

    // MARK: - Internal (visible for testing)

    /// Monotonic seconds elapsed since `start`.
    static func seconds(since start: ContinuousClock.Instant) -> Double {
        let d = ContinuousClock.now - start
        return Double(d.components.seconds) + Double(d.components.attoseconds) * 1e-18
    }

    /// Build the set of token ids that terminate generation.
    ///
    /// Pulls from three sources, all carried by the `ModelConfiguration` and
    /// tokenizer (all required for chat-tuned models to stop correctly):
    ///
    /// 1. `configuration.eosTokenIds` — loaded from `config.json` /
    ///    `generation_config.json` at model-load time. Chat models like
    ///    Gemma 3 ship `eos_token_id` as an array (e.g. `[1, 106]` for
    ///    `<eos>` + `<end_of_turn>`); this source is the only way to pick
    ///    up the turn-ender when the tokenizer's primary EOS is the
    ///    completion EOS.
    /// 2. `tokenizer.eosTokenId` — the tokenizer's single primary EOS.
    /// 3. `configuration.extraEOSTokens` — hardcoded-by-token-string
    ///    additions from registry entries (e.g. `["<end_of_turn>"]` on
    ///    some Gemma variants in `LLMModelFactory`). Callers needing extra
    ///    stop tokens add them here (via the model configuration), not as a
    ///    per-call argument.
    static func buildStopTokenIDs(
        tokenizer: any Tokenizer,
        configuration: ModelConfiguration
    ) -> Set<Int> {
        var stopTokenIDs = Set(configuration.eosTokenIds)
        if let eos = tokenizer.eosTokenId {
            stopTokenIDs.insert(eos)
        }
        for token in configuration.extraEOSTokens {
            if let id = tokenizer.convertTokenToId(token) {
                stopTokenIDs.insert(id)
            }
        }
        return stopTokenIDs
    }

    /// Apply a *prebuilt* grammar mask and optional bias to logits, then argmax.
    ///
    /// The mask is built once per token in the eval loop's overlap window (see
    /// `buildMaskArray`) and passed in here, so this runs no `bitmaskToMLXArray`
    /// work on the sampling critical path.
    ///
    /// - Parameters:
    ///   - logits: Raw model output logits (shape: [batch, seq, vocab]).
    ///   - maskArray: Prebuilt additive grammar mask (length == logit dim;
    ///     0.0 allowed, -inf disallowed), or nil when the grammar forces all
    ///     tokens (no mask to apply).
    ///   - closingBias: Optional logit bias favoring closing tokens. Applied
    ///     after the grammar mask so masked-out tokens remain at -inf.
    /// - Returns: The sampled token ID.
    static func applyMaskAndSample(
        logits rawLogits: MLXArray,
        maskArray: MLXArray?,
        closingBias: MLXArray? = nil
    ) -> UInt32 {
        // Extract last-position logits: [batch, seq, vocab] -> [vocab]
        var logits = rawLogits[0..., -1, 0...]

        if let maskArray {
            logits = logits + maskArray
        }

        if let bias = closingBias {
            let logitDim = logits.shape[logits.ndim - 1]
            let biasDim = bias.shape[0]
            if biasDim < logitDim {
                // Model logit dimension can exceed tokenizer vocab (padding/special tokens).
                // Pad with zeros so the bias has no effect on extra positions.
                let padding = MLXArray.zeros([logitDim - biasDim])
                logits = logits + concatenated([bias, padding])
            } else if biasDim > logitDim {
                // Tokenizer vocab can exceed model logit dimension (added special tokens
                // beyond the embedding size). Truncate to match.
                logits = logits + bias[0 ..< logitDim]
            } else {
                logits = logits + bias
            }
        }

        // Grammar-constrained generation samples greedily by construction.
        let sampled = argMax(logits, axis: -1)
        return sampled.item(UInt32.self)
    }

    /// Apply the processor, the grammar mask and an optional bias to the
    /// last position's logits, then sample. Stays on the GPU: the returned
    /// token (shape `[1]`) is lazy.
    ///
    /// - Parameters:
    ///   - logits: Raw model output logits (shape: [batch, seq, vocab]).
    ///   - allowed: Boolean mask over the logit dimension from
    ///     `PackedTokenMask.allowed(_:)`, or nil when the grammar excludes
    ///     nothing.
    ///   - closingBias: Optional logit bias favoring closing tokens. Applied
    ///     after the grammar mask so masked-out tokens remain at -inf.
    ///   - logitProcessor: Penalties and similar, applied before the mask.
    ///   - logitSampler: The sampler; masked tokens have zero probability.
    static func sampleToken(
        logits rawLogits: MLXArray,
        allowed: MLXArray?,
        closingBias: MLXArray? = nil,
        logitProcessor: (any LogitProcessor)? = nil,
        logitSampler: any LogitSampler
    ) -> MLXArray {
        // Extract last-position logits: [batch, seq, vocab] -> [batch, vocab]
        var logits = rawLogits[0..., -1, 0...]
        logits = logitProcessor?.process(logits: logits) ?? logits

        if let allowed {
            logits = which(allowed, logits, MLXArray(-Float.infinity).asType(logits.dtype))
        }
        if let closingBias {
            logits = logits + alignedBias(closingBias, logitDim: logits.dim(-1))
        }
        return logitSampler.sample(logits: logits)
    }

    /// Pad or truncate a vocabulary-length bias to the model's logit dimension.
    static func alignedBias(_ bias: MLXArray, logitDim: Int) -> MLXArray {
        let biasDim = bias.shape[0]
        if biasDim < logitDim {
            // Model logit dimension can exceed tokenizer vocab (padding/special tokens).
            // Pad with zeros so the bias has no effect on extra positions.
            return concatenated([bias, MLXArray.zeros([logitDim - biasDim])])
        } else if biasDim > logitDim {
            // Tokenizer vocab can exceed model logit dimension (added special tokens
            // beyond the embedding size). Truncate to match.
            return bias[0 ..< logitDim]
        }
        return bias
    }

    /// Build the additive grammar-mask array for a freshly computed `MaskResult`,
    /// or nil when the grammar needs no mask applied (unconditional FF splice).
    ///
    /// Hoisted out of `applyMaskAndSample` so the loop can build it in the
    /// CPU/GPU overlap window (alongside `computeMask`) instead of on the
    /// critical path at sample time. `logitDim` is the model's logit dimension
    /// (constant across the generation); `vocabSize` is the grammar bitmask's
    /// valid-bit count.
    static func buildMaskArray(for mask: MaskResult, vocabSize: Int, logitDim: Int) -> MLXArray? {
        guard mask.needsApply else { return nil }
        return mask.mask.withUnsafeBufferPointer { buffer in
            let ptr = UnsafeRawPointer(buffer.baseAddress!).assumingMemoryBound(to: UInt32.self)
            return bitmaskToMLXArray(ptr, maskBitCount: vocabSize, totalCount: logitDim)
        }
    }

    /// Convert a packed bitmask (1 bit per token) to an MLXArray of floats.
    /// Allowed tokens get 0.0, disallowed tokens get -inf.
    ///
    /// `maskBitCount` is the number of valid bits in the mask (= tokenizer vocab
    /// size). `totalCount` is the model's logit dimension. When the tokenizer
    /// has more tokens than the model has logits (e.g. added special tokens
    /// beyond the embedding dimension), we only read `min(maskBitCount, totalCount)`
    /// bits. Positions beyond the mask are left at -inf.
    ///
    /// Internal (not private) so the mask-build microbenchmark can time it in
    /// isolation.
    static func bitmaskToMLXArray(
        _ maskPtr: UnsafePointer<UInt32>,
        maskBitCount: Int,
        totalCount: Int
    ) -> MLXArray {
        var floats = [Float](repeating: -Float.infinity, count: totalCount)
        let readCount = min(maskBitCount, totalCount)
        for i in 0 ..< readCount {
            let word = maskPtr[i / 32]
            let bit = (word >> (UInt32(i) % 32)) & 1
            if bit == 1 {
                floats[i] = 0.0
            }
        }
        return MLXArray(floats)
    }
}

/// Expands xgrammar's packed token bitmask (one bit per token, 32 per word)
/// to a boolean mask over the model's logit dimension on the GPU.
///
/// Per token the host uploads only the packed words (~31 KB for a 248K
/// vocabulary) instead of building a vocabulary-length `[Float]`. Bit `i` of
/// the mask is bit `i % 32` of word `i / 32`. Logit positions at or beyond the
/// grammar vocabulary size are never allowed, matching `bitmaskToMLXArray`.
struct PackedTokenMask {
    private let wordIndex: MLXArray
    private let bitIndex: MLXArray
    private let inVocabulary: MLXArray

    init(vocabSize: Int, logitDim: Int) {
        let positions = MLXArray(Array(Int32(0) ..< Int32(logitDim)))
        let readCount = MLXArray(Int32(min(vocabSize, logitDim)))
        // Positions beyond the vocabulary read word 0; `inVocabulary` masks them.
        inVocabulary = positions .< readCount
        let clamped = which(inVocabulary, positions, MLXArray(Int32(0)))
        wordIndex = clamped >> MLXArray(Int32(5))
        bitIndex = clamped & MLXArray(Int32(31))
    }

    /// The boolean allowed-mask for `mask`, or nil when the grammar excludes
    /// nothing. Lazy: the expansion runs on the GPU when the sample does.
    func allowed(_ mask: MaskResult) -> MLXArray? {
        guard mask.needsApply else { return nil }
        let words = MLXArray(mask.mask)
        // Arithmetic shift is fine: only the low bit survives `& 1`.
        let one = MLXArray(Int32(1))
        let bits = (take(words, wordIndex) >> bitIndex) & one
        return logicalAnd(bits .== one, inVocabulary)
    }
}
