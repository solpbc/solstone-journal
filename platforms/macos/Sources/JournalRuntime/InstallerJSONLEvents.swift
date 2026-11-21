// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import Foundation

internal enum JSONValue: Sendable, Equatable, Decodable {
    case string(String)
    case number(Double)
    case bool(Bool)
    case object([String: JSONValue])
    case array([JSONValue])
    case null

    internal init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        if container.decodeNil() {
            self = .null
        } else if let value = try? container.decode(Bool.self) {
            self = .bool(value)
        } else if let value = try? container.decode(Int.self) {
            self = .number(Double(value))
        } else if let value = try? container.decode(Double.self) {
            self = .number(value)
        } else if let value = try? container.decode(String.self) {
            self = .string(value)
        } else if let value = try? container.decode([String: JSONValue].self) {
            self = .object(value)
        } else if let value = try? container.decode([JSONValue].self) {
            self = .array(value)
        } else {
            throw DecodingError.dataCorruptedError(in: container, debugDescription: "unsupported JSON value")
        }
    }

    internal var stringValue: String? {
        if case .string(let value) = self { return value }
        return nil
    }

    internal var intValue: Int? {
        if case .number(let value) = self, value.rounded() == value {
            return Int(value)
        }
        return nil
    }

    internal var int32Value: Int32? {
        guard let value = intValue else { return nil }
        return Int32(exactly: value)
    }

    internal var objectValue: [String: JSONValue]? {
        if case .object(let value) = self { return value }
        return nil
    }
}

internal struct RawSetupEvent: Sendable, Equatable, Decodable {
    internal let event: String
    internal let payload: [String: JSONValue]

    internal init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: DynamicCodingKey.self)
        let eventKey = DynamicCodingKey(stringValue: "event")!
        event = try container.decode(String.self, forKey: eventKey)

        var payload: [String: JSONValue] = [:]
        for key in container.allKeys where key.stringValue != "event" {
            payload[key.stringValue] = try container.decode(JSONValue.self, forKey: key)
        }
        self.payload = payload
    }
}

internal enum SetupEvent: Sendable, Equatable {
    case setupStarted(version: String?, mode: String?)
    case setupCompleted(status: String, durationMS: Int?, failedStep: String?)
    case stepStarted(step: String, index: Int?, total: Int?)
    case stepCompleted(step: String, outcome: String, reason: String?, durationMS: Int?)
    case stepFailed(step: String?, errorCode: String?, message: String, details: String?, exitCode: Int32?)
    case stepWarning(step: String?, text: String, fixHint: String?)
    case doctorStarted(version: String?)
    case checkCompleted(name: String, severity: String?, status: String, detail: String?, fix: String?)
    case doctorCompleted(status: String, durationMS: Int?, summary: DoctorSummary?)
}

public struct DoctorSummary: Decodable, Sendable, Equatable {
    public let total: Int
    public let failed: Int
    public let warnings: Int
    public let skipped: Int

    public init(total: Int, failed: Int, warnings: Int, skipped: Int) {
        self.total = total
        self.failed = failed
        self.warnings = warnings
        self.skipped = skipped
    }
}

internal enum ParsedLine: Sendable, Equatable {
    case event(SetupEvent)
    case malformed(String)
    case unrecognized(String)
    case unparseableLine(String)
}

// Source of truth: the native setup's EventType (core/crates/solstone-core-setup,
// src/events.rs). The parser recognizes exactly these; step names, skip reasons,
// error codes and statuses pass through as text. InstallerJSONLEventsTests decodes
// the fixture that crate keeps equal to its own vocabulary.
public enum InstallerKnownValues {
    public static let eventTypes: [String] = [
        "setup.started",
        "setup.completed",
        "step.started",
        "step.completed",
        "step.failed",
        "step.warning",
        "doctor.started",
        "check.completed",
        "doctor.completed"
    ]
}

internal enum SetupEventParser {
    internal static func parse(line: String) -> ParsedLine {
        guard !line.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            return .unparseableLine(line)
        }

        guard let data = line.data(using: .utf8),
              let raw = try? JSONDecoder().decode(RawSetupEvent.self, from: data) else {
            return .malformed(line)
        }

        guard InstallerKnownValues.eventTypes.contains(raw.event) else {
            return .unrecognized(line)
        }

        guard let event = parse(raw) else {
            return .malformed(line)
        }
        return .event(event)
    }

    private static func parse(_ raw: RawSetupEvent) -> SetupEvent? {
        switch raw.event {
        case "setup.started":
            return .setupStarted(
                version: raw.payload["version"]?.stringValue,
                mode: raw.payload["mode"]?.stringValue
            )
        case "setup.completed":
            guard let status = raw.payload["status"]?.stringValue else { return nil }
            return .setupCompleted(
                status: status,
                durationMS: raw.payload["duration_ms"]?.intValue,
                failedStep: raw.payload["failed_step"]?.stringValue
            )
        case "step.started":
            guard let step = raw.payload["step"]?.stringValue else { return nil }
            return .stepStarted(
                step: step,
                index: raw.payload["index"]?.intValue,
                total: raw.payload["total"]?.intValue
            )
        case "step.completed":
            guard let step = raw.payload["step"]?.stringValue,
                  let outcome = raw.payload["outcome"]?.stringValue else { return nil }
            return .stepCompleted(
                step: step,
                outcome: outcome,
                reason: raw.payload["reason"]?.stringValue,
                durationMS: raw.payload["duration_ms"]?.intValue
            )
        case "step.failed":
            guard let error = raw.payload["error"]?.objectValue,
                  let message = error["message"]?.stringValue else { return nil }
            return .stepFailed(
                step: raw.payload["step"]?.stringValue,
                errorCode: error["code"]?.stringValue,
                message: message,
                details: error["details"]?.stringValue,
                exitCode: error["exit_code"]?.int32Value
            )
        case "step.warning":
            guard let text = raw.payload["text"]?.stringValue else { return nil }
            return .stepWarning(
                step: raw.payload["step"]?.stringValue,
                text: text,
                fixHint: raw.payload["fix_hint"]?.stringValue
            )
        case "doctor.started":
            return .doctorStarted(version: raw.payload["version"]?.stringValue)
        case "check.completed":
            guard let name = raw.payload["name"]?.stringValue,
                  let status = raw.payload["status"]?.stringValue else { return nil }
            return .checkCompleted(
                name: name,
                severity: raw.payload["severity"]?.stringValue,
                status: status,
                detail: raw.payload["detail"]?.stringValue,
                fix: raw.payload["fix"]?.stringValue
            )
        case "doctor.completed":
            guard let status = raw.payload["status"]?.stringValue else { return nil }
            return .doctorCompleted(
                status: status,
                durationMS: raw.payload["duration_ms"]?.intValue,
                summary: parseSummary(raw.payload["summary"]?.objectValue)
            )
        default:
            return nil
        }
    }

    private static func parseSummary(_ value: [String: JSONValue]?) -> DoctorSummary? {
        guard let value,
              let total = value["total"]?.intValue,
              let failed = value["failed"]?.intValue,
              let warnings = value["warnings"]?.intValue,
              let skipped = value["skipped"]?.intValue else {
            return nil
        }
        return DoctorSummary(total: total, failed: failed, warnings: warnings, skipped: skipped)
    }
}

internal struct EventRenderer: Sendable {
    internal private(set) var renderedLog = ""

    internal init() {}

    internal mutating func append(_ parsedLine: ParsedLine) {
        switch parsedLine {
        case .event(let event):
            append(event)
        case .malformed(let raw):
            appendLine(JournalCopy.eventsMalformed(sanitize(raw)))
        case .unrecognized(let raw):
            appendLine(JournalCopy.eventsUnrecognized(sanitize(raw)))
        case .unparseableLine(let raw):
            if !raw.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                appendLine(JournalCopy.eventsUnparseable(sanitize(raw)))
            }
        }
    }

    internal mutating func append(_ event: SetupEvent) {
        appendLine(render(event))
    }

    internal func render(_ event: SetupEvent) -> String {
        switch event {
        case .setupStarted(let version, let mode):
            if let version, let mode {
                return JournalCopy.eventsStartedVersion(sanitize(version), sanitize(mode))
            }
            return JournalCopy.eventsStarted()
        case .setupCompleted(let status, let durationMS, let failedStep):
            if status == "ok" {
                if let durationMS {
                    return JournalCopy.eventsCompletedDuration(String(durationMS))
                }
                return JournalCopy.eventsCompleted()
            }
            if let failedStep {
                return JournalCopy.eventsFailedStep(sanitize(failedStep))
            }
            return JournalCopy.eventsFailed()
        case .stepStarted(let step, let index, let total):
            if let index, let total {
                return JournalCopy.eventsStepIndexed(String(index), String(total), sanitize(step))
            }
            return JournalCopy.eventsStep(sanitize(step))
        case .stepCompleted(let step, let outcome, let reason, let durationMS):
            if outcome == "skipped" {
                if let reason {
                    return JournalCopy.eventsSkippedReason(sanitize(step), sanitize(reason))
                }
                return JournalCopy.eventsSkipped(sanitize(step))
            }
            if let durationMS {
                return JournalCopy.eventsDoneDuration(sanitize(step), sanitize(outcome), String(durationMS))
            }
            return JournalCopy.eventsDone(sanitize(step), sanitize(outcome))
        case .stepFailed(let step, _, let message, _, _):
            return JournalCopy.eventsStepFailed(sanitize(step ?? JournalCopy.eventsUnknown()), sanitize(message))
        case .stepWarning(_, let text, let fixHint):
            if let fixHint, !fixHint.isEmpty {
                return JournalCopy.eventsWarningFix(sanitize(text), sanitize(fixHint))
            }
            return JournalCopy.eventsWarning(sanitize(text))
        case .doctorStarted:
            return JournalCopy.eventsDoctorStarted()
        case .checkCompleted(let name, _, let status, let detail, _):
            let label = renderStatus(status)
            return JournalCopy.eventsDoctorCheck(label, sanitize(name), sanitize(detail ?? ""))
        case .doctorCompleted(let status, _, let summary):
            if let summary {
                return JournalCopy.eventsDoctorSummary(summary.total, String(summary.failed), String(summary.warnings), String(summary.skipped))
            }
            return JournalCopy.eventsDoctorStatus(sanitize(status))
        }
    }

    private mutating func appendLine(_ line: String) {
        renderedLog += line + "\n"
        renderedLog = String(renderedLog.suffix(16 * 1024))
    }

    private func renderStatus(_ status: String) -> String {
        switch status {
        case "ok":
            return JournalCopy.eventsOk()
        case "warning", "warn":
            return JournalCopy.eventsWarn()
        case "failed", "fail":
            return JournalCopy.eventsFail()
        case "skipped", "skip":
            return JournalCopy.eventsSkip()
        default:
            return sanitize(status)
        }
    }

    private func sanitize(_ value: String) -> String {
        value
            .trimmingCharacters(in: .whitespacesAndNewlines)
            .components(separatedBy: .whitespacesAndNewlines)
            .filter { !$0.isEmpty }
            .joined(separator: " ")
    }
}

private struct DynamicCodingKey: CodingKey {
    let stringValue: String
    let intValue: Int?

    init?(stringValue: String) {
        self.stringValue = stringValue
        self.intValue = nil
    }

    init?(intValue: Int) {
        self.stringValue = "\(intValue)"
        self.intValue = intValue
    }
}
