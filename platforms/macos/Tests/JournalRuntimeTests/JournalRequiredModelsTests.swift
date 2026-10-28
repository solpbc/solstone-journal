// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import Foundation
import JournalRuntimeTestSupport
import MacTestSupport
import SolstoneCore
import Testing
@testable import JournalRuntime

@Suite("Journal required model reconciliation")
struct JournalRequiredModelsTests {
    @Test func repairsPreviousRuntimeCacheInSelectedJournalWithBundledCommand() async throws {
        let runtime = try makeRuntime()
        defer { try? FileManager.default.removeItem(at: runtime.layout.rootURL) }
        let journal = runtime.layout.rootURL.appendingPathComponent("selected journal", isDirectory: true)
        try FileManager.default.createDirectory(at: journal, withIntermediateDirectories: true)
        let sidecar = journal.appendingPathComponent("engine-sha")
        try "previous-signed-archive\n".write(to: sidecar, atomically: true, encoding: .utf8)
        let script = """
        #!/bin/sh
        set -eu
        [ "$#" = 3 ] && [ "$1" = journal ] && [ "$2" = install-models ] && [ "$3" = --required-only ] || exit 41
        [ -z "${SOLSTONE_JOURNAL+x}" ] || exit 42
        [ "$0" = "\(runtime.layout.solstoneBinary.path)" ] || exit 43
        [ "$(command -v solstone)" = "\(runtime.layout.solstoneBinary.path)" ] || exit 44
        [ "$(cat engine-sha)" = previous-signed-archive ] || exit 45
        printf 'current-signed-archive\\n' > engine-sha
        """
        try script.write(to: runtime.layout.solstoneBinary, atomically: true, encoding: .utf8)
        try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: runtime.layout.solstoneBinary.path)
        let scoped = MaterializedRuntime(key: runtime.key, layout: runtime.layout,
            environment: ["PATH": "/usr/bin:/bin", "SOLSTONE_JOURNAL": "/wrong/inherited/journal"])
        try await JournalRequiredModelsReconciler().reconcile(runtime: scoped, journalRoot: journal)
        #expect(try String(contentsOf: sidecar, encoding: .utf8) == "current-signed-archive\n")
    }

    @Test func nativeNonzeroExitPropagates() async throws {
        let runtime = try makeRuntime()
        defer { try? FileManager.default.removeItem(at: runtime.layout.rootURL) }
        let subprocess = FakeSubprocessRunner()
        subprocess.enqueue("install-models", .success(exitCode: 65))
        let reconciler = JournalRequiredModelsReconciler(makeRunner: { _ in subprocess })
        await #expect(throws: SupervisedJournalRunnerError.self) {
            try await reconciler.reconcile(runtime: runtime, journalRoot: runtime.layout.rootURL)
        }
        #expect(subprocess.invocations.count == 1)
        #expect(subprocess.invocations.first?.executable == runtime.layout.solstoneBinary)
        #expect(subprocess.invocations.first?.arguments == ["journal", "install-models", "--required-only"])
        #expect(subprocess.invocations.first?.timeout == .seconds(120))
        #expect(subprocess.invocations.first?.environment?["SOLSTONE_JOURNAL"] == nil)
        #expect(subprocess.invocations.first?.environment?["PATH"]?.hasPrefix(runtime.layout.binDir.path + ":") == true)
    }

    @Test func timeoutCannotBecomeSuccessfulReconciliation() async throws {
        let runtime = try makeRuntime()
        defer { try? FileManager.default.removeItem(at: runtime.layout.rootURL) }
        let subprocess = FakeSubprocessRunner()
        subprocess.enqueue("install-models", .success(delay: .seconds(1)))
        let reconciler = JournalRequiredModelsReconciler(timeout: .milliseconds(1), makeRunner: { _ in subprocess })
        await #expect(throws: SupervisedJournalRunnerError.self) {
            try await reconciler.reconcile(runtime: runtime, journalRoot: runtime.layout.rootURL)
        }
    }

    @Test func reconcileSummaryOmitsChildPayloadAndKeepsByteCount() async throws {
        let runtime = try makeRuntime()
        defer { try? FileManager.default.removeItem(at: runtime.layout.rootURL) }
        let marker = "AC8-REQUIRED-MODELS-MARKER-9c2e"
        let subprocess = FakeSubprocessRunner()
        subprocess.enqueue(
            "install-models",
            .success(stdout: Data("installed \(marker)\n".utf8), stderr: Data("warn \(marker)\n".utf8))
        )
        let log = RecordingClassifiedLogSink()
        let reconciler = JournalRequiredModelsReconciler(makeRunner: { _ in subprocess }, log: log)

        try await reconciler.reconcile(runtime: runtime, journalRoot: runtime.layout.rootURL)

        let emission = try #require(log.emissions.first)
        #expect(emission.classification == "journal-lifecycle: required-models")
        #expect(emission.level == .notice)
        let concatenated = emission.classification + emission.publicFields.values.joined()
        #expect(!concatenated.contains(marker))
        #expect(emission.publicFields["stdoutEmpty"] == "false")
        #expect(emission.publicFields["stderrEmpty"] == "false")
        #expect(emission.publicFields["exit"] == "0")
        #expect(Int(emission.publicFields["stdoutBytes"] ?? "0") ?? 0 > 0)
    }
}
