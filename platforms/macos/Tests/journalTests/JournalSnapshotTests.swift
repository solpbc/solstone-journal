// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import AppKit
import Foundation
import JournalMarkKit
import JournalRuntime
import JournalRuntimeTestSupport
import os
import ServiceManagement
import SwiftUI
import Testing
import SetupChrome
@testable import UpdateKit
@testable import journal

private func runGit(_ args: String...) throws -> String {
    let process = Process()
    process.executableURL = URL(fileURLWithPath: "/usr/bin/git")
    process.arguments = Array(args)
    let pipe = Pipe()
    process.standardOutput = pipe
    try process.run()
    process.waitUntilExit()
    return String(data: pipe.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8)!
        .trimmingCharacters(in: .whitespacesAndNewlines)
}

func makeSnapshotOutputDir() throws -> URL {
    // Renders land under this package (platforms/macos/snapshots, ignored), not the
    // repository root: the package root and the Git root are different directories.
    let repoRoot = journalPackageRoot().path
    let gitHash = try runGit("rev-parse", "--short", "HEAD")

    let dirtyCheck = Process()
    dirtyCheck.executableURL = URL(fileURLWithPath: "/usr/bin/git")
    dirtyCheck.arguments = ["diff", "--quiet", "HEAD"]
    dirtyCheck.currentDirectoryURL = URL(fileURLWithPath: repoRoot)
    try dirtyCheck.run()
    dirtyCheck.waitUntilExit()
    let isDirty = dirtyCheck.terminationStatus != 0

    let formatter = DateFormatter()
    formatter.dateFormat = "yyyy-MM-dd_HHmmss"
    let dirName = "\(formatter.string(from: Date()))_\(gitHash)\(isDirty ? "-dirty" : "")"

    let url = URL(fileURLWithPath: repoRoot).appendingPathComponent("snapshots/\(dirName)")
    try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
    return url
}

@Suite("Snapshot-journal")
@MainActor
struct JournalSnapshotTests {
    private static nonisolated(unsafe) var _outputDir: URL?
    private let size = CGSize(width: 800, height: 560)

    private var outputDir: URL {
        get throws {
            if let dir = Self._outputDir { return dir }
            let dir = try makeSnapshotOutputDir()
            Self._outputDir = dir
            return dir
        }
    }

    init() {
        _ = NSApplication.shared
        JournalMarkFont.register()
    }

    @Test func paneMatrix() async throws {
        try await renderHomeConfiguredRunning()
        try await renderHomeConfiguredStopped()
        try await renderHomeConfiguredStarting()
        try await renderHomeConfiguredBlocked()
        try await renderHomeConfiguredUnknown()
        try await renderHomeUnconfiguredInterim()
        try await renderHomeHostnameFallback()
        try await renderJournalNameAndDiskUsage()
        try await renderRunStateRunning()
        try await renderRunStateStopped()
        try await renderRunStateBlocked()
        try await renderRunStateUnknownHealth()
        try await renderBackupStatic()
        try await renderStartup(enabled: true)
        try await renderStartup(enabled: false)
        try await renderUpdates()
        try await renderFirstRunNameEntry()
        try await renderFirstRunSetupProgress()
        try await renderFirstRunFinishInBrowser()
        try await renderFirstRunLockedHome()
        try await renderFirstRunAdoptLanding()
    }

    @Test(.enabled(if: ProcessInfo.processInfo.environment["JOURNAL_UPDATE_UX_GUI"] == "1"))
    func updateAttentionMatrix() async throws {
        let model = try configuredModel(mark: .uiTestSample)
        let suite = "journal-update-snapshots-\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let controller = UpdateController(
            feedURL: "https://updates.solstone.app/solstone-journal-macos/appcast.xml",
            publicKey: Data(repeating: 0, count: 32).base64EncodedString(),
            log: Logger.updates, errorDomain: "journal.update-snapshots", defaults: defaults
        ) { _, _ in JournalUpdateWindowUpdater() }
        controller.automaticChecksEnabled = false
        let fixtures: [(String, ReconciledUpdateStatus.Outcome, Bool, Bool)] = [
            ("available", .found, true, false), ("staged", .staged, true, false),
            ("failed", .failed, false, false), ("failed-available", .failed, true, false),
            ("deferred", .found, true, true), ("current", .upToDate, false, false)
        ]
        for (name, outcome, hasVersion, deferred) in fixtures {
            controller.applyDebugFixture(
                activity: .idle,
                availableUpdate: hasVersion ? .init(version: "9.9.9", releaseNotes: nil) : nil,
                lastCheck: .init(checkedAt: Date(), outcome: outcome),
                deferredInstallIntent: deferred ? .init(version: "9.9.9", requestedAt: Date()) : nil
            )
            for pane in [JournalPane.home, .updates] {
                model.selectedPane = pane
                try await render(
                    ZStack {
                        SunArcBackgroundView()
                        JournalSettingsWindow(model: model, updateController: controller, openURL: { _ in true })
                    }.frame(width: size.width, height: size.height),
                    size: size,
                    to: "journal-update-\(pane.rawValue)-\(name).png"
                )
            }
        }
    }

    /// The sun arc in both appearances at the spec's five check times (Denver 2026-09-23:
    /// 13:00, 19:41 = dusk + 15, 22:00, 01:00 true dark, 05:30 before dawn), for review next to
    /// the sun-arc appearances board. Each frame is rendered in the
    /// window appearance the owner's system would give it (`NSAppearance` on the host, no
    /// SwiftUI override), bare and under the two surfaces the window shows.
    @Test func sunArcAppearanceMatrix() async throws {
        let denver = TimeZone(identifier: "America/Denver")!
        let times: [(label: String, epoch: TimeInterval)] = [
            ("1300", 1_790_190_000), ("1941", 1_790_214_060), ("2200", 1_790_222_400),
            ("0100", 1_790_146_800), ("0530", 1_790_163_000)
        ]
        for (name, appearance) in [("light", NSAppearance.Name.aqua), ("dark", NSAppearance.Name.darkAqua)] {
            for time in times {
                let date = Date(timeIntervalSince1970: time.epoch)
                let background = SunArcBackgroundView(fixedDate: date, timeZone: denver)
                try await render(
                    background.frame(width: size.width, height: size.height),
                    size: size,
                    appearance: appearance,
                    validateContent: false,
                    to: "journal-sunarc-\(name)-\(time.label)-bare.png"
                )

                let fixture = try makeConfiguredFixture()
                let supervisor = JournalSupervisor()
                supervisor.applyRuntimeStatus(.stoppedByUser)
                let model = configuredModel(fixture: fixture, supervisor: supervisor, mark: .uiTestSample)
                model.selectedPane = .home
                try await render(
                    ZStack {
                        background
                        JournalSettingsWindow(model: model, updateController: makeUpdateController(), openURL: { _ in true })
                    }
                    .frame(width: size.width, height: size.height),
                    size: size,
                    appearance: appearance,
                    to: "journal-sunarc-\(name)-\(time.label)-home.png"
                )

                let firstRun = makeModel(startResults: [], probeResults: [])
                firstRun.model.route = .ritual(.nameLocation)
                try await render(
                    JournalFirstRunView(model: firstRun.model, background: background)
                    .frame(width: size.width, height: size.height),
                    size: size,
                    appearance: appearance,
                    to: "journal-sunarc-\(name)-\(time.label)-first-run.png"
                )
            }
        }
    }

    private func renderHomeConfiguredRunning() async throws {
        let supervisor = try await runningSupervisor()
        let model = try configuredModel(supervisor: supervisor, mark: .uiTestSample)
        model.selectedPane = .home
        try await renderWindow(model, to: "journal-home-configured-running.png")
    }

    private func renderHomeConfiguredStopped() async throws {
        let fixture = try makeConfiguredFixture()
        let supervisor = JournalSupervisor()
        supervisor.applyRuntimeStatus(.stoppedByUser)
        let model = configuredModel(fixture: fixture, supervisor: supervisor, mark: .uiTestSample)
        model.selectedPane = .home
        try await renderWindow(model, to: "journal-home-configured-stopped.png")
    }

    private func renderHomeConfiguredStarting() async throws {
        let fixture = try makeConfiguredFixture()
        let supervisor = JournalSupervisor()
        supervisor.applyRuntimeStatus(.restarting(generation: nil))
        let model = configuredModel(fixture: fixture, supervisor: supervisor, mark: .uiTestSample)
        model.selectedPane = .home
        try await renderWindow(model, to: "journal-home-configured-starting.png")
    }

    private func renderHomeConfiguredBlocked() async throws {
        let diagnostic = JournalDiagnostic(commandLabel: "gate", outputExcerpt: "port busy")
        let blockage = SingleSupervisorGateBlockage.portConflict(diagnostic)
        let supervisor = JournalSupervisor(
            gate: MockSingleSupervisorGate(),
            materializer: MockRuntimeMaterializer(result: .success(try makeRuntime())),
            runner: MockSupervisedChildRunner(startError: SupervisedJournalRunnerError.gateBlocked(blockage)),
            readinessGate: MockJournalReadinessGate(result: .ready)
        )
        _ = configureInMemoryReceiptContext(supervisor)
        let fixture = try makeConfiguredFixture()
        _ = await supervisor.start(journalRoot: try #require(fixture.config.journalRoot))
        let model = configuredModel(fixture: fixture, supervisor: supervisor, mark: .uiTestSample)
        model.selectedPane = .home
        try await renderWindow(model, to: "journal-home-configured-blocked.png")
    }

    private func renderHomeConfiguredUnknown() async throws {
        let model = try configuredModel(mark: .uiTestSample)
        model.selectedPane = .home
        try await renderWindow(model, to: "journal-home-configured-unknown.png")
    }

    private func renderHomeUnconfiguredInterim() async throws {
        let model = unconfiguredModel()
        model.selectedPane = .home
        try await renderWindow(model, to: "journal-home-unconfigured-interim.png")
    }

    private func renderHomeHostnameFallback() async throws {
        let supervisor = try await runningSupervisor()
        let model = try configuredModel(supervisor: supervisor, mark: nil)
        model.selectedPane = .home
        try await renderWindow(model, to: "journal-home-hostname-fallback.png")
    }

    private func renderJournalNameAndDiskUsage() async throws {
        let model = try configuredModel(mark: .uiTestSample)
        model.selectedPane = .journal
        model.diskUsageBytes = 1_234_567
        try await renderWindow(model, to: "journal-journal-name-diskusage.png")
    }

    private func renderRunStateRunning() async throws {
        let supervisor = try await runningSupervisor()
        let model = try configuredModel(supervisor: supervisor, mark: .uiTestSample)
        model.selectedPane = .runState
        model.healthDisplay = .healthy
        model.journalVersion = "1.2.3"
        try await renderWindow(model, to: "journal-run-state-running.png")
    }

    private func renderRunStateStopped() async throws {
        let fixture = try makeConfiguredFixture()
        let supervisor = JournalSupervisor()
        supervisor.applyRuntimeStatus(.stoppedByUser)
        let model = configuredModel(fixture: fixture, supervisor: supervisor, mark: .uiTestSample)
        model.selectedPane = .runState
        model.healthDisplay = .stopped
        model.journalVersion = "unknown"
        try await renderWindow(model, to: "journal-run-state-stopped.png")
    }

    private func renderRunStateBlocked() async throws {
        let diagnostic = JournalDiagnostic(commandLabel: "gate", outputExcerpt: "port busy")
        let blockage = SingleSupervisorGateBlockage.portConflict(diagnostic)
        let supervisor = JournalSupervisor(
            gate: MockSingleSupervisorGate(),
            materializer: MockRuntimeMaterializer(result: .success(try makeRuntime())),
            runner: MockSupervisedChildRunner(startError: SupervisedJournalRunnerError.gateBlocked(blockage)),
            readinessGate: MockJournalReadinessGate(result: .ready)
        )
        _ = configureInMemoryReceiptContext(supervisor)
        let fixture = try makeConfiguredFixture()
        _ = await supervisor.start(journalRoot: try #require(fixture.config.journalRoot))
        let model = configuredModel(fixture: fixture, supervisor: supervisor, mark: .uiTestSample)
        model.selectedPane = .runState
        try await renderWindow(model, to: "journal-run-state-blocked.png")
    }

    private func renderRunStateUnknownHealth() async throws {
        let supervisor = try await runningSupervisor()
        let model = try configuredModel(supervisor: supervisor, mark: .uiTestSample)
        model.selectedPane = .runState
        model.healthDisplay = .unknown
        model.journalVersion = "unknown"
        try await renderWindow(model, to: "journal-run-state-unknown-health.png")
    }

    private func renderBackupStatic() async throws {
        let model = try configuredModel(mark: .uiTestSample)
        model.selectedPane = .backup
        try await renderWindow(model, to: "journal-backup-static.png")
    }

    private func renderStartup(enabled: Bool) async throws {
        let fixture = try makeConfiguredFixture()
        fixture.config.setLaunchAtLoginEnabled(enabled)
        let model = configuredModel(fixture: fixture, supervisor: JournalSupervisor(), mark: .uiTestSample)
        model.selectedPane = .startup
        try await renderWindow(model, to: enabled ? "journal-startup-enabled.png" : "journal-startup-disabled.png")
    }

    private func renderUpdates() async throws {
        let model = try configuredModel(mark: .uiTestSample)
        model.selectedPane = .updates
        try await renderWindow(model, to: "journal-updates.png")
    }

    private func renderFirstRunNameEntry() async throws {
        let fixture = makeModel(startResults: [], probeResults: [])
        fixture.model.route = .ritual(.nameLocation)
        try await renderFirstRun(fixture.model, to: "journal-first-run-name-entry.png")
    }

    private func renderFirstRunSetupProgress() async throws {
        let fixture = makeModel(startResults: [], probeResults: [])
        fixture.model.route = .ritual(.setupProgress)
        fixture.model.currentStep = "prepare"
        fixture.model.setupRenderedLog = "prepare ok\ncreate journal"
        try await renderFirstRun(fixture.model, to: "journal-first-run-setup-progress.png")
    }

    private func renderFirstRunFinishInBrowser() async throws {
        let fixture = makeModel(startResults: [], probeResults: [])
        fixture.model.route = .ritual(.finishInBrowser)
        try await renderFirstRun(fixture.model, to: "journal-first-run-finish-in-browser.png")
    }

    private func renderFirstRunLockedHome() async throws {
        let supervisor = try await runningSupervisor()
        let model = try configuredModel(supervisor: supervisor, mark: .uiTestSample)
        model.selectedPane = .home
        try await renderWindow(model, to: "journal-first-run-locked-home.png")
    }

    private func renderFirstRunAdoptLanding() async throws {
        let fixture = makeModel(startResults: [], probeResults: [])
        fixture.model.route = .adopting
        fixture.model.adoptMessage = JournalFirstRunCopy.adoptLandingLine
        try await renderFirstRun(fixture.model, to: "journal-first-run-adopt-landing.png")
    }

    private func runningSupervisor() async throws -> JournalSupervisor {
        let supervisor = JournalSupervisor(
            gate: MockSingleSupervisorGate(),
            materializer: MockRuntimeMaterializer(result: .success(try makeRuntime())),
            runner: MockSupervisedChildRunner(),
            readinessGate: MockJournalReadinessGate(result: .ready)
        )
        _ = configureInMemoryReceiptContext(supervisor)
        let root = try makeTemporaryDirectory()
        _ = await supervisor.start(journalRoot: root)
        supervisor.applyRuntimeStatus(.running)
        return supervisor
    }

    private func configuredModel(
        supervisor: JournalSupervisor = JournalSupervisor(),
        mark: JournalMark?
    ) throws -> JournalWindowModel {
        configuredModel(
            fixture: try makeConfiguredFixture(),
            supervisor: supervisor,
            mark: mark
        )
    }

    private func configuredModel(
        fixture: SnapshotFixture,
        supervisor: JournalSupervisor,
        mark: JournalMark?
    ) -> JournalWindowModel {
        let read: JournalIdentityRead = if let mark {
            .mark(mark)
        } else {
            .unavailable
        }
        let model = JournalWindowModel(
            config: fixture.config,
            supervisor: supervisor,
            fetchIdentity: { _ in read },
            fetchDiskUsage: { _ in 1_234_567 },
            fetchHealth: { _, _ in .unknown(JournalDiagnostic(commandLabel: "health")) },
            fetchVersion: { _, _ in nil },
            appBuild: "67"
        )
        model.identityRead = read
        return model
    }

    private func unconfiguredModel() -> JournalWindowModel {
        let fixture = makeUnconfiguredFixture()
        return JournalWindowModel(
            config: fixture.config,
            supervisor: JournalSupervisor(),
            fetchIdentity: { _ in .uncommitted },
            fetchDiskUsage: { _ in 0 },
            fetchHealth: { _, _ in .unknown(JournalDiagnostic(commandLabel: "health")) },
            fetchVersion: { _, _ in nil },
            appBuild: "67"
        )
    }

    private func makeConfiguredFixture() throws -> SnapshotFixture {
        let fixture = makeUnconfiguredFixture()
        let root = try makeTemporaryDirectory()
        fixture.config.journalRoot = root
        return fixture
    }

    private func makeUnconfiguredFixture() -> SnapshotFixture {
        let suiteName = "app.solstone.journal.snapshots.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suiteName)!
        defaults.removePersistentDomain(forName: suiteName)
        let config = JournalAppConfig(defaults: defaults, loginItemManager: SnapshotFakeLoginItemManager())
        return SnapshotFixture(suiteName: suiteName, defaults: defaults, config: config)
    }

    private func makeTemporaryDirectory() throws -> URL {
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("journal-snapshots-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        return url
    }

    private func renderWindow(_ model: JournalWindowModel, to filename: String) async throws {
        try await render(
            ZStack {
                SunArcBackgroundView()
                JournalSettingsWindow(model: model, updateController: makeUpdateController(), openURL: { _ in true })
            }
            .frame(width: size.width, height: size.height),
            size: size,
            to: filename
        )
    }

    private func renderFirstRun(_ model: JournalFirstRunModel, to filename: String) async throws {
        try await render(
            JournalFirstRunView(model: model)
                .frame(width: size.width, height: size.height),
            size: size,
            to: filename
        )
    }

    private func makeUpdateController() -> UpdateController {
        UpdateController(
            feedURL: nil,
            publicKey: nil,
            log: Logger.updates,
            errorDomain: "app.solstone.journal.updates"
        ) { _, _ in nil }
    }

    /// `validateContent: false` is for a frame that is legitimately uniform (the bare sun arc in
    /// true dark is one ground colour and nothing else).
    static func render<V: View>(
        _ view: V,
        size: CGSize,
        outputDir: URL,
        appearance: NSAppearance.Name? = nil,
        validateContent: Bool = true,
        to filename: String
    ) async throws {
        let hostingView = NSHostingView(rootView: view)
        if let appearance { hostingView.appearance = NSAppearance(named: appearance) }
        hostingView.frame = NSRect(origin: .zero, size: size)
        hostingView.layoutSubtreeIfNeeded()

        guard let bitmapRep = hostingView.bitmapImageRepForCachingDisplay(in: hostingView.bounds) else {
            throw RenderError.noBitmap
        }
        hostingView.cacheDisplay(in: hostingView.bounds, to: bitmapRep)

        guard let pngData = bitmapRep.representation(using: .png, properties: [:]) else {
            throw RenderError.noPNG
        }

        let url = outputDir.appendingPathComponent(filename)
        let bytesPerPixel = bitmapRep.bitsPerPixel / 8
        let bytesPerRow = bitmapRep.bytesPerRow
        let width = bitmapRep.pixelsWide
        let height = bitmapRep.pixelsHigh

        let bitmap: SnapshotBitmap?
        if validateContent, let bitmapData = bitmapRep.bitmapData {
            bitmap = SnapshotBitmap(
                bytes: Array(UnsafeBufferPointer(start: bitmapData, count: bytesPerRow * height)),
                bytesPerPixel: bytesPerPixel,
                bytesPerRow: bytesPerRow,
                width: width,
                height: height
            )
        } else {
            bitmap = nil
        }

        try await SnapshotRenderPostProcessor.writePNGAndValidateContent(
            pngData: pngData,
            outputURL: url,
            filename: filename,
            bitmap: bitmap,
            emptyContent: RenderError.emptyContent
        )
    }

    func render<V: View>(
        _ view: V,
        size: CGSize,
        appearance: NSAppearance.Name? = nil,
        validateContent: Bool = true,
        to filename: String
    ) async throws {
        try await Self.render(
            view,
            size: size,
            outputDir: try outputDir,
            appearance: appearance,
            validateContent: validateContent,
            to: filename
        )
    }

    enum RenderError: Error, Sendable, CustomStringConvertible {
        case noBitmap
        case noPNG
        case emptyContent(filename: String, contentPixelCount: Int, minimum: Int)

        var description: String {
            switch self {
            case .noBitmap: return "failed to create bitmap rep"
            case .noPNG: return "failed to encode PNG"
            case let .emptyContent(filename, contentPixelCount, minimum):
                return "\(filename): \(contentPixelCount) content pixels below \(minimum) minimum"
            }
        }
    }
}

private struct SnapshotFixture {
    let suiteName: String
    let defaults: UserDefaults
    let config: JournalAppConfig
}

@MainActor
private final class SnapshotFakeLoginItemManager: LoginItemManaging {
    var status: SMAppService.Status = .notRegistered

    func register() throws {}
    func unregister() throws {}
    func unregisterAwaitingCompletion() async throws {}
}

@MainActor
private final class JournalUpdateWindowUpdater: SparkleUpdating {
    var automaticallyChecksForUpdates = false
    var automaticallyDownloadsUpdates = false
    var updateCheckInterval: TimeInterval = 86_400
    var sessionInProgress = false
    func checkForUpdates() {}
    func start() throws {}
}
