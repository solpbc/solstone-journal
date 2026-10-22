// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import AVFoundation
import Darwin
import FluidAudio
import InstalledModels
import Foundation

private let fluidAudioVersion = "0.17.7+sol.1"
private let defaultModelVersion = "v3"

private struct JSONTokenTiming: Encodable {
    let token: String
    let token_id: Int
    let start: Double
    let end: Double
    let confidence: Float
}

private struct JSONOutput: Encodable {
    let path: String
    let transcript: String
    let confidence: Float
    let audio_sec: Double
    let load_ms: Int
    let transcribe_ms: Int
    let rtfx: Double
    let token_timings: [JSONTokenTiming]
    let model_version: String
    let fluidaudio_version: String
    let hardware: String
    let macos_version: String
    let swift_version: String
}

private struct VersionOutput: Encodable {
    let fluidaudio_version: String
    let model_version_default: String
    let swift_version: String
    let hardware: String
    let macos_version: String
}

private struct ErrorOutput: Encodable {
    let category: String
    let message: String
    let detail: String?
}

private enum HelperModel: String {
    case v2
    case v3

    var asrModelVersion: AsrModelVersion {
        switch self {
        case .v2:
            return .v2
        case .v3:
            return .v3
        }
    }

    var fullModelVersion: String {
        "parakeet-tdt-0.6b-\(rawValue)"
    }
}

private enum ParsedCommand {
    case version
    case transcribe(audioPath: String, cacheDir: URL, model: HelperModel)
}

private func writeJSONLine<T: Encodable>(_ value: T, to handle: FileHandle) {
    let encoder = JSONEncoder()
    encoder.outputFormatting = [.withoutEscapingSlashes]
    do {
        let data = try encoder.encode(value)
        handle.write(data)
        handle.write("\n".data(using: .utf8)!)
    } catch {
        handle.write(
            #"{"category":"transcribe","message":"failed to encode JSON output"}"#.data(
                using: .utf8
            )!
        )
        handle.write("\n".data(using: .utf8)!)
    }
}

private func fail(
    code: Int32,
    category: String,
    message: String,
    detail: String? = nil
) -> Never {
    writeJSONLine(
        ErrorOutput(category: category, message: message, detail: detail),
        to: FileHandle.standardError
    )
    exit(code)
}

private func sysctlString(_ name: String) -> String {
    var size = 0
    guard sysctlbyname(name, nil, &size, nil, 0) == 0, size > 0 else {
        return "unknown"
    }
    var buffer = [CChar](repeating: 0, count: size)
    guard sysctlbyname(name, &buffer, &size, nil, 0) == 0 else {
        return "unknown"
    }
    return String(cString: buffer)
}

private func hardwareString() -> String {
    let model = sysctlString("hw.model")
    let brand = sysctlString("machdep.cpu.brand_string")
    return "\(model) / \(brand)"
}

private func macosVersionString() -> String {
    let version = ProcessInfo.processInfo.operatingSystemVersion
    return "\(version.majorVersion).\(version.minorVersion).\(version.patchVersion)"
}

private func swiftVersionString() -> String {
#if os(macOS)
    let process = Process()
    let pipe = Pipe()
    process.executableURL = URL(fileURLWithPath: "/usr/bin/swift")
    process.arguments = ["--version"]
    process.standardOutput = pipe
    process.standardError = Pipe()
    do {
        try process.run()
        process.waitUntilExit()
        guard process.terminationStatus == 0 else {
            return "unknown"
        }
        let data = pipe.fileHandleForReading.readDataToEndOfFile()
        guard
            let output = String(data: data, encoding: .utf8)?
                .split(separator: "\n")
                .first
        else {
            return "unknown"
        }
        return String(output)
    } catch {
        return "unknown"
    }
#else
    return "unknown"
#endif
}

private func expandedURL(path: String) -> URL {
    URL(fileURLWithPath: NSString(string: path).expandingTildeInPath)
}

private func defaultCacheDir() -> URL {
    expandedURL(path: "~/Library/Application Support/solstone/parakeet/models")
}

private func parseCommand() -> ParsedCommand {
    let args = Array(CommandLine.arguments.dropFirst())
    if args.contains("--version") {
        guard args.count == 1, args.first == "--version" else {
            fail(
                code: 2,
                category: "argv",
                message: "--version must be used without other arguments"
            )
        }
        return .version
    }

    var cacheDir = defaultCacheDir()
    var model = HelperModel.v3
    var audioPath: String?
    var index = 0

    while index < args.count {
        let arg = args[index]
        switch arg {
        case "--cache-dir":
            guard index + 1 < args.count else {
                fail(
                    code: 2,
                    category: "argv",
                    message: "--cache-dir requires a path argument"
                )
            }
            cacheDir = expandedURL(path: args[index + 1])
            index += 2
        case "--model":
            guard index + 1 < args.count else {
                fail(
                    code: 2,
                    category: "argv",
                    message: "--model requires one of: v2, v3"
                )
            }
            guard let parsedModel = HelperModel(rawValue: args[index + 1]) else {
                fail(
                    code: 2,
                    category: "argv",
                    message: "unknown --model value '\(args[index + 1])'; valid values: v2, v3"
                )
            }
            model = parsedModel
            index += 2
        default:
            if arg.hasPrefix("--") {
                fail(code: 2, category: "argv", message: "unknown flag: \(arg)")
            }
            guard audioPath == nil else {
                fail(
                    code: 2,
                    category: "argv",
                    message: "expected exactly one positional audio path"
                )
            }
            audioPath = arg
            index += 1
        }
    }

    guard let audioPath else {
        fail(code: 2, category: "argv", message: "missing required positional audio path")
    }

    return .transcribe(audioPath: audioPath, cacheDir: cacheDir, model: model)
}

private func createCacheDir(_ cacheDir: URL) {
    do {
        try FileManager.default.createDirectory(
            at: cacheDir,
            withIntermediateDirectories: true,
            attributes: nil
        )
    } catch {
        fail(
            code: 3,
            category: "cache",
            message: "failed to create cache dir",
            detail: String(describing: error)
        )
    }
}

private func audioDurationSeconds(url: URL) throws -> Double {
    let file = try AVAudioFile(forReading: url)
    let format = file.processingFormat
    return Double(file.length) / format.sampleRate
}


@main
struct Main {
    static func main() async {
        // FluidAudio 0.17 reports token times one encoder frame (80 ms) earlier by
        // default, to line up with a rescorer this helper does not use. Pin the
        // timing the journal's other speech engine reports. Re-measure on any
        // FluidAudio bump: this is the library's own override, read per call.
        setenv("TDT_EMISSION_DELAY_FRAMES", "0", 1)
        let hardware = hardwareString()
        let macosVersion = macosVersionString()
        let swiftVersion = swiftVersionString()

        switch parseCommand() {
        case .version:
            writeJSONLine(
                VersionOutput(
                    fluidaudio_version: fluidAudioVersion,
                    model_version_default: defaultModelVersion,
                    swift_version: swiftVersion,
                    hardware: hardware,
                    macos_version: macosVersion
                ),
                to: FileHandle.standardOutput
            )
        case let .transcribe(audioPath, cacheDir, model):
            createCacheDir(cacheDir)

            let loadStart = DispatchTime.now().uptimeNanoseconds
            let manager = AsrManager()
            let loadMs: Int
            do {
                let modelDirectory = cacheDir.deletingLastPathComponent()
                    .appendingPathComponent(model.fullModelVersion)
                let models = try loadInstalledModels(from: modelDirectory, version: model.asrModelVersion)
                try await manager.loadModels(models)
                loadMs = Int(
                    (DispatchTime.now().uptimeNanoseconds - loadStart) / 1_000_000
                )
            } catch {
                fail(
                    code: 4,
                    category: "model_download",
                    message: "the speech model could not be loaded; run solstone journal install-models --force --variant coreml to repair it",
                    detail: String(describing: error)
                )
            }

            do {
                let audioURL = URL(fileURLWithPath: audioPath)
                let audioSec = try audioDurationSeconds(url: audioURL)
                var decoderState = try TdtDecoderState()

                let txStart = DispatchTime.now().uptimeNanoseconds
                let result = try await manager.transcribe(
                    audioURL,
                    decoderState: &decoderState
                )
                let transcribeMs =
                    Int((DispatchTime.now().uptimeNanoseconds - txStart) / 1_000_000)
                let txSeconds = max(Double(transcribeMs) / 1000.0, 1e-6)

                let timings = (result.tokenTimings ?? []).map {
                    JSONTokenTiming(
                        token: $0.token,
                        token_id: $0.tokenId,
                        start: $0.startTime,
                        end: $0.endTime,
                        confidence: $0.confidence
                    )
                }

                writeJSONLine(
                    JSONOutput(
                        path: audioPath,
                        transcript: result.text,
                        confidence: result.confidence,
                        audio_sec: audioSec,
                        load_ms: loadMs,
                        transcribe_ms: transcribeMs,
                        rtfx: audioSec / txSeconds,
                        token_timings: timings,
                        model_version: model.fullModelVersion,
                        fluidaudio_version: fluidAudioVersion,
                        hardware: hardware,
                        macos_version: macosVersion,
                        swift_version: swiftVersion
                    ),
                    to: FileHandle.standardOutput
                )
            } catch {
                fail(
                    code: 5,
                    category: "transcribe",
                    message: "failed to transcribe audio",
                    detail: String(describing: error)
                )
            }
        }
    }
}
