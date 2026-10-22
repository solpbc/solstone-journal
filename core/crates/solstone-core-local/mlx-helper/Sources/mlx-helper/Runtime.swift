// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
import CoreImage
import Foundation
import HelperServer
import MLX
import MLXHuggingFace
import MLXLMCommon
import MLXVLM
@preconcurrency import Tokenizers

private enum GenerationFailure: Error {
    case missingCompletionMetadata
}

struct MLXBackend: InferenceBackend {
    let container: ModelContainer
    let options: Options

    static func load(options: Options) async throws -> Self {
        let url = URL(fileURLWithPath: options.model, isDirectory: true)
        var isDirectory: ObjCBool = false
        guard FileManager.default.fileExists(atPath: url.path, isDirectory: &isDirectory), isDirectory.boolValue else {
            throw RequestError("model must be an existing local directory")
        }
        Memory.cacheLimit = options.cacheLimitMB * 1_048_576
        let container = try await loadModelContainer(from: url, using: #huggingFaceTokenizerLoader())
        return Self(container: container, options: options)
    }

    func tokenize(_ content: String) async throws -> [Int] {
        await container.perform { context in
            context.tokenizer.encode(text: content, addSpecialTokens: false)
        }
    }

    func complete(_ request: ChatRequest) async throws -> Completion {
        let prepared = PreparedMessages(request.messages)
        let options = self.options
        return try await container.perform { context in
            let images: [UserInput.Image] = try prepared.images.map { data in
                guard let image = CIImage(data: data) else { throw RequestError("invalid image data") }
                return .ciImage(image)
            }
            var input = UserInput(messages: prepared.messages, images: images,
                                  additionalContext: ["enable_thinking": false])
            input.processing.maxPixels = 3_686_400
            input.processing.minPixels = nil
            let lmInput = try await context.processor.prepare(input: input)
            let limit = try request.generationBudget(promptTokens: lmInput.text.tokens.size, context: options.context)
            var parameters = GenerateParameters(maxTokens: limit, temperature: request.temperature ?? 0.7)
            parameters.topP = request.top_p ?? 0.8
            parameters.topK = request.top_k ?? 20
            parameters.minP = request.min_p ?? 0
            parameters.presencePenalty = request.presence_penalty ?? 1.5
            parameters.presenceContextSize = 64
            // TokenIterator seeds the processor with the complete prepared prompt.
            let iterator = try TokenIterator(input: lmInput, model: context.model, parameters: parameters)
            let (stream, generationTask) = MLXLMCommon.generateTask(
                promptTokenCount: lmInput.text.tokens.size,
                modelConfiguration: context.configuration, tokenizer: context.tokenizer,
                iterator: iterator, toolCallPolicy: parameters.toolCallPolicy)
            var text = ""
            var info: GenerateCompletionInfo?
            await withTaskCancellationHandler {
                for await item in stream {
                    switch item {
                    case .chunk(let chunk): text += chunk
                    case .info(let completion): info = completion
                    default: break
                    }
                }
                // Stream termination precedes GPU cleanup on cancellation. Keep
                // the slot until the producer has synchronized and returned.
                await generationTask.value
            } onCancel: {
                generationTask.cancel()
            }
            try Task.checkCancellation()
            guard let info else { throw GenerationFailure.missingCompletionMetadata }
            let finish: FinishReason
            switch info.stopReason {
            case .stop: finish = try .map(stopped: true, reachedLimit: false)
            case .length: finish = try .map(stopped: false, reachedLimit: true)
            case .cancelled: throw CancellationError()
            }
            return Completion(model: options.alias, text: text, finish: finish,
                              promptTokens: info.promptTokenCount, completionTokens: info.generationTokenCount,
                              promptSeconds: info.promptTime, decodeSeconds: info.generateTime)
        }
    }
}
