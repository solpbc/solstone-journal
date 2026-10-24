// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
import Foundation
import Darwin
import HelperServer
import Hummingbird
import OSLog

@main
struct Main {
    static func main() async {
        do { try await run() }
        catch {
            Logger(subsystem: "org.solpbc.mlx-helper", category: "runtime").error("helper failed: \(String(describing: error), privacy: .public)")
            exit(1)
        }
    }

    static func run() async throws {
        let arguments = Array(CommandLine.arguments.dropFirst())
        if arguments == ["--help"] {
            print("""
            mlx-helper --model <local-directory> [--host 127.0.0.1|::1] [--port 8080]
                       [--alias local/qwen3.5-4b] [--ctx 16384] [--cache-limit-mb 512]
            Non-streaming, loopback-only MLX inference.
            The positive allocator cache limit is in MiB. Completion tokens are capped at
            the context space remaining after the prepared prompt.
            """)
            return
        }
        let options = try Options(arguments: arguments)
        let state = ModelState()
        let started = AsyncStream<Void>.makeStream(bufferingPolicy: .bufferingNewest(1))
        let app = makeApplication(options: options, state: state, onRunning: {
            started.continuation.yield(())
            started.continuation.finish()
        })
        // A load failure cancels the listener and exits nonzero. It cannot leave
        // an apparently live server stuck in the loading state.
        try await withThrowingTaskGroup(of: Void.self) { group in
            group.addTask { try await app.runService() }
            group.addTask {
                for await _ in started.stream {
                    try Task.checkCancellation()
                    let backend = try await MLXBackend.load(options: options)
                    try Task.checkCancellation()
                    await state.ready(backend)
                }
            }
            for try await _ in group {}
        }
    }
}
