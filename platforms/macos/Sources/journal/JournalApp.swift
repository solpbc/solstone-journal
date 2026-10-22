// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import AppKit
import JournalMarkKit
import JournalRuntime
import os
import SetupChrome
import SwiftUI
import UpdateKit

@MainActor
final class JournalAppModel {
    static var shared: JournalAppModel?

    let config: JournalAppConfig
    let supervisor: JournalSupervisor
    let iconManager: JournalIconManager
    let windowModel: JournalWindowModel
    let firstRunModel: JournalFirstRunModel
    private var startupTask: Task<Void, Never>?
    private var activationTasks: [Task<Void, Never>] = []
    private(set) var terminationPrepared = false
    private(set) var appKitTerminationBegan = false

    lazy var updateController: UpdateController = makeUpdateController()

    init(
        config: JournalAppConfig = JournalAppConfig(),
        supervisor: JournalSupervisor = JournalSupervisor()
    ) {
        self.config = config
        self.supervisor = supervisor
        let iconManager = JournalIconManager(config: config, bundlePath: Bundle.main.bundlePath)
        self.iconManager = iconManager
        let windowModel = JournalWindowModel(config: config, supervisor: supervisor)
        windowModel.onIdentityMark = { [weak iconManager] mark in
            iconManager?.handleIdentityMark(mark)
        }
        self.windowModel = windowModel
        self.firstRunModel = JournalFirstRunModel(
            config: config,
            startSupervisor: { [supervisor] root in
                await supervisor.start(journalRoot: root)
            },
            windowModel: windowModel,
            supervisorIsLive: { [supervisor] root in
                supervisor.state == .running
                    && supervisor.activeJournalRoot?.standardizedFileURL.path == root.standardizedFileURL.path
            }
        )
    }

    func launch(receiptContext: JournalRuntimeEntryReceiptContext? = nil) {
        if let receiptContext {
            supervisor.configureReceiptContext(receiptContext)
        }
        JournalMarkFont.register()
        iconManager.start()
        Task { @MainActor in
            await config.applyLaunchAtLoginPreference()
            windowModel.noteLaunchAtLoginApplied()
            firstRunModel.loginFailure = config.launchAtLoginFailureSentence
        }
        windowModel.prepareForWindowOpen()
        startActivation()
    }

    func reconcileForReopen() {
        startActivation(reopen: true)
    }

    private func startActivation(reopen: Bool = false) {
        let task = Task { @MainActor [firstRunModel] in
            if reopen {
                await firstRunModel.reconcileForReopen()
            } else {
                await firstRunModel.reconcileActivation()
            }
        }
        activationTasks.append(task)
        startupTask = task
    }

    func prepareWindowOpen(load: Bool = true) {
        windowModel.prepareForWindowOpen()
        if load, firstRunModel.route == .home {
            Task { @MainActor [windowModel] in
                await windowModel.loadForWindowOpen()
            }
        }
    }

    func prepareForTermination() async {
        noteAppKitTerminationBegan()
        guard !terminationPrepared else { return }
        terminationPrepared = true
        startupTask?.cancel()
        activationTasks.forEach { $0.cancel() }
        await firstRunModel.prepareForTermination()
        await supervisor.terminate(reason: "ordinary-quit")
    }

    func noteAppKitTerminationBegan() {
        appKitTerminationBegan = true
    }

    func resetAfterFailedUpdaterInstall() {
        firstRunModel.resetAfterFailedUpdaterInstall()
        terminationPrepared = false
        appKitTerminationBegan = false
    }

    func reestablishSupervisionAfterFailedUpdate() async {
        guard let root = config.journalRoot else { return }
        _ = await supervisor.start(journalRoot: root)
    }

    private func makeUpdateController() -> UpdateController {
        UpdateController(
            log: Logger.updates,
            errorDomain: "app.solstone.journal.updates",
            exclusivity: { [weak supervisor] in
                guard let supervisor else { return false }
                switch supervisor.state {
                case .materializing, .starting, .waitingForReadiness:
                    return true
                default:
                    return false
                }
            },
            preInstallFinalizer: { [weak supervisor] in
                await supervisor?.terminate(reason: "updater-install")
            },
            installFailureRecovery: { [weak self] in
                self?.resetAfterFailedUpdaterInstall()
                await self?.reestablishSupervisionAfterFailedUpdate()
            },
            terminationBegan: { [weak self] in
                self?.appKitTerminationBegan ?? false
            }
        )
    }
}

@MainActor
final class JournalAppDelegate: NSObject, NSApplicationDelegate {
    private let receiptContextFactory: () -> JournalRuntimeEntryReceiptContext
    private let modelLauncher: (JournalRuntimeEntryReceiptContext) -> Void

    override init() {
        self.receiptContextFactory = {
            JournalRuntimeEntryReceiptLaunch.begin(provenanceBundle: .module)
        }
        self.modelLauncher = { context in
            JournalAppModel.shared?.launch(receiptContext: context)
        }
        super.init()
    }

    init(
        receiptContextFactory: @escaping () -> JournalRuntimeEntryReceiptContext,
        modelLauncher: @escaping (JournalRuntimeEntryReceiptContext) -> Void
    ) {
        self.receiptContextFactory = receiptContextFactory
        self.modelLauncher = modelLauncher
        super.init()
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        let receiptContext = receiptContextFactory()
        modelLauncher(receiptContext)
    }

    static func isJournalWindow(_ window: NSWindow) -> Bool {
        window.identifier?.rawValue.contains("journal") == true || window.title == "journal"
    }

    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        guard let model = JournalAppModel.shared else {
            Logger.journalApp.error("JournalAppModel.shared nil in applicationShouldHandleReopen")
            return true
        }
        if let journalWindow = NSApp.windows.first(where: Self.isJournalWindow) {
            model.prepareWindowOpen()
            journalWindow.makeKeyAndOrderFront(nil)
            NSApp.activate(ignoringOtherApps: true)
        } else {
            model.prepareWindowOpen(load: false)
            Logger.journalApp.info("applicationShouldHandleReopen: no journal NSWindow found; posting open journal notification")
            NotificationCenter.default.post(name: .openJournalWindow, object: nil)
            NSApp.activate(ignoringOtherApps: true)
        }
        model.reconcileForReopen()
        return true
    }

    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        guard let model = JournalAppModel.shared else {
            return .terminateNow
        }
        model.noteAppKitTerminationBegan()
        if model.terminationPrepared {
            return .terminateNow
        }
        Task { @MainActor in
            await model.prepareForTermination()
            NSApp.reply(toApplicationShouldTerminate: true)
        }
        return .terminateLater
    }
}

struct JournalApp: App {
    @NSApplicationDelegateAdaptor(JournalAppDelegate.self) private var appDelegate
    @State private var model: JournalAppModel

    init() {
        let model = JournalAppModel()
        JournalAppModel.shared = model
        _model = State(initialValue: model)
    }

    var body: some Scene {
        Window("journal", id: "journal") {
            JournalWindowSceneRoot(
                model: model.windowModel,
                firstRunModel: model.firstRunModel,
                updateController: model.updateController
            )
        }
        .windowResizability(.contentMinSize)
        .defaultPosition(.center)
        .commands {
            CommandGroup(after: .appSettings) {
                Button(JournalAdminTerminal.menuItemTitle) {
                    JournalAdminTerminal.openFromMenu()
                }
            }
        }
    }
}

private struct JournalWindowSceneRoot: View {
    let model: JournalWindowModel
    @Bindable var firstRunModel: JournalFirstRunModel
    @Bindable var updateController: UpdateController
    @Environment(\.openWindow) private var openWindow

    // The window takes the owner's system appearance; nothing here sets it (the sun arc's
    // 2026-09-23 amendment retired the clock-driven `.preferredColorScheme`).
    var body: some View {
        Group {
            if firstRunModel.route == .home {
                ZStack(alignment: .top) {
                    JournalSettingsWindow(model: model, updateController: updateController)
                        .task(id: model.supervisor.state == .running) {
                            await model.loadForWindowOpen()
                        }

                    if firstRunModel.adoptMessage == JournalFirstRunCopy.adoptLandingLine {
                        Text(JournalFirstRunCopy.adoptLandingLine)
                            .font(.callout)
                            .padding(.horizontal, 14)
                            .padding(.vertical, 8)
                            .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                            .padding(.top, 12)
                    }
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .background(SunArcBackgroundView())
            } else {
                JournalFirstRunView(model: firstRunModel)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        // Behind the content, never over it: the installer is a plain view and would take every click.
        .background {
            JournalSetupWindowInstaller(setupShowing: firstRunModel.route.showsSetupWindow)
        }
        .onReceive(NotificationCenter.default.publisher(for: .openJournalWindow)) { _ in
            model.prepareForWindowOpen()
            openWindow(id: "journal")
            NSApp.activate(ignoringOtherApps: true)
            if firstRunModel.route == .home {
                Task {
                    await model.loadForWindowOpen()
                }
            }
        }
    }
}

private extension Notification.Name {
    static let openJournalWindow = Notification.Name("openJournalWindow")
}

extension JournalFirstRunRoute {
    /// The setup window's look applies while setup is on screen: not while the app is still
    /// deciding what to show, and not on home.
    var showsSetupWindow: Bool {
        switch self {
        case .ritual, .adopting:
            return true
        case .deciding, .home:
            return false
        }
    }
}
