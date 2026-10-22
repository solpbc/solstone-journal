// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
import Foundation

public struct RequestError: Error, Sendable {
    public let message: String
    public init(_ message: String) { self.message = message }
}

public struct ChatRequest: Decodable, Sendable {
    public let model: String?
    public let messages: [ChatMessage]
    public let temperature: Float?
    public let max_tokens: Int?
    public let top_p: Float?
    public let top_k: Int?
    public let min_p: Float?
    public let presence_penalty: Float?
    public let stream: Bool?
    public let chat_template_kwargs: TemplateOptions?

    enum CodingKeys: String, CodingKey {
        case model, messages, temperature, max_tokens, top_p, top_k, min_p
        case presence_penalty, stream, chat_template_kwargs, response_format
    }

    public init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        guard !c.contains(.response_format) else {
            throw RequestError("response_format is not supported by this helper")
        }
        model = try c.decodeIfPresent(String.self, forKey: .model)
        messages = try c.decode([ChatMessage].self, forKey: .messages)
        temperature = try c.decodeIfPresent(Float.self, forKey: .temperature)
        max_tokens = try c.decodeIfPresent(Int.self, forKey: .max_tokens)
        top_p = try c.decodeIfPresent(Float.self, forKey: .top_p)
        top_k = try c.decodeIfPresent(Int.self, forKey: .top_k)
        min_p = try c.decodeIfPresent(Float.self, forKey: .min_p)
        presence_penalty = try c.decodeIfPresent(Float.self, forKey: .presence_penalty)
        stream = try c.decodeIfPresent(Bool.self, forKey: .stream)
        chat_template_kwargs = try c.decodeIfPresent(TemplateOptions.self, forKey: .chat_template_kwargs)
        guard !messages.isEmpty else { throw RequestError("messages must not be empty") }
        guard stream != true else { throw RequestError("streaming is not supported") }
        guard chat_template_kwargs?.enable_thinking != true else {
            throw RequestError("enable_thinking must be false")
        }
        guard (temperature ?? 0.7).isFinite, (temperature ?? 0.7) >= 0,
              (0...1).contains(top_p ?? 0.8), (0...1).contains(min_p ?? 0),
              (top_k ?? 20) >= 0, (max_tokens ?? 2048) > 0,
              (presence_penalty ?? 1.5).isFinite else {
            throw RequestError("invalid sampling parameters or max_tokens")
        }
    }

    public func generationBudget(promptTokens: Int, context: Int) throws -> Int {
        guard promptTokens < context else {
            throw RequestError("prepared prompt exceeds the context capacity")
        }
        return min(max_tokens ?? 2048, context - promptTokens)
    }
}

public struct TemplateOptions: Decodable, Sendable {
    public let enable_thinking: Bool?
}

public struct ChatMessage: Decodable, Sendable {
    public let role: String
    public let content: MessageContent

    public init(from decoder: any Decoder) throws {
        enum Keys: String, CodingKey { case role, content }
        let c = try decoder.container(keyedBy: Keys.self)
        role = try c.decode(String.self, forKey: .role)
        guard role == "system" || role == "user" else {
            throw RequestError("only system and user messages are supported")
        }
        content = try c.decode(MessageContent.self, forKey: .content)
    }
}

public enum MessageContent: Decodable, Sendable {
    case text(String)
    case parts([ContentPart])

    public init(from decoder: any Decoder) throws {
        let c = try decoder.singleValueContainer()
        if let text = try? c.decode(String.self) { self = .text(text) }
        else { self = .parts(try c.decode([ContentPart].self)) }
    }
}

public enum ContentPart: Decodable, Sendable {
    case text(String)
    case image(Data)

    public init(from decoder: any Decoder) throws {
        enum Keys: String, CodingKey { case type, text, image_url }
        struct ImageURL: Decodable { let url: String }
        let c = try decoder.container(keyedBy: Keys.self)
        switch try c.decode(String.self, forKey: .type) {
        case "text": self = .text(try c.decode(String.self, forKey: .text))
        case "image_url":
            self = .image(try Self.decodeDataURL(c.decode(ImageURL.self, forKey: .image_url).url))
        default: throw RequestError("unsupported message content type")
        }
    }

    public static func decodeDataURL(_ url: String) throws -> Data {
        guard let comma = url.firstIndex(of: ","),
              url[..<comma].hasPrefix("data:image/"),
              url[..<comma].hasSuffix(";base64"),
              let data = Data(base64Encoded: String(url[url.index(after: comma)...])),
              !data.isEmpty else {
            throw RequestError("image_url must contain a base64 image data URL")
        }
        return data
    }
}

/// This conversion preserves part positions and image association. The runtime
/// decodes each image in the same traversal and never fetches a URL.
public struct PreparedMessages: Sendable {
    public let messages: [[String: any Sendable]]
    public let images: [Data]

    public init(_ messages: [ChatMessage]) {
        var images: [Data] = []
        self.messages = messages.map { message in
            switch message.content {
            case .text(let text): return ["role": message.role, "content": text]
            case .parts(let parts):
                let converted: [[String: any Sendable]] = parts.map { part in
                    switch part {
                    case .text(let text): return ["type": "text", "text": text]
                    case .image(let data):
                        images.append(data)
                        return ["type": "image"]
                    }
                }
                return ["role": message.role, "content": converted]
            }
        }
        self.images = images
    }
}

public enum FinishReason: String, Encodable, Sendable {
    case stop, length
    public static func map(stopped: Bool, reachedLimit: Bool) throws -> Self {
        if stopped { return .stop }
        if reachedLimit { return .length }
        throw RequestError("generation ended without a completion")
    }
}

public struct Completion: Encodable, Sendable {
    public struct Choice: Encodable, Sendable {
        public struct Message: Encodable, Sendable {
            let role = "assistant"
            let content: String
        }
        let index = 0
        let message: Message
        let finish_reason: FinishReason
    }
    public struct Usage: Encodable, Sendable {
        let prompt_tokens: Int
        let completion_tokens: Int
        let total_tokens: Int
    }
    public struct Timings: Encodable, Sendable {
        let prompt_n: Int
        let prompt_ms: Double
        let predicted_n: Int
        let predicted_ms: Double
        let cache_n = 0
    }
    let id = "chatcmpl-" + UUID().uuidString
    let object = "chat.completion"
    let created = Int(Date().timeIntervalSince1970)
    let model: String
    let choices: [Choice]
    let usage: Usage
    let timings: Timings

    public init(model: String, text: String, finish: FinishReason,
                promptTokens: Int, completionTokens: Int, promptSeconds: Double, decodeSeconds: Double) {
        self.model = model
        choices = [.init(message: .init(content: text), finish_reason: finish)]
        usage = .init(prompt_tokens: promptTokens, completion_tokens: completionTokens,
                      total_tokens: promptTokens + completionTokens)
        timings = .init(prompt_n: promptTokens, prompt_ms: promptSeconds * 1000,
                        predicted_n: completionTokens, predicted_ms: decodeSeconds * 1000)
    }
}
