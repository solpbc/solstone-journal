// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import Foundation

public struct JournalSetupCopy: Equatable, Sendable {
    public var heading: String
    public var footer: String
    public var welcome: Welcome
    public var fits: Fits
    public var login: Login
    public var folder: Folder
    public var mark: Mark
    public var thinking: Thinking
    public var downloads: Downloads

    public struct Welcome: Equatable, Sendable {
        public var title: String
        public var body: String
        public var howItFits: [String]
    }

    public struct Fits: Equatable, Sendable {
        public var title: String
        public var journalTerm: String
        public var journal: String
        public var appTerm: String
        public var app: String
        public var thinkingTerm: String
        public var thinking: String
    }

    public struct Login: Equatable, Sendable {
        public var label: String
        public var help: String
    }

    public struct Folder: Equatable, Sendable {
        public var title: String
        public var help: String
        public var change: String
        public var continueLabel: String
    }

    public struct Mark: Equatable, Sendable {
        public var title: String
        public var help: String
        public var error: String
        public var permanence: String
        public var tryAnother: String
        public var continueLabel: String
    }

    public struct Choice: Equatable, Sendable {
        public var id: String
        public var label: String
        public var sub: String
        public var description: String
    }

    public struct Thinking: Equatable, Sendable {
        public var title: String
        public var choices: [Choice]
        public var defaultLine: String
        public var sizeUnknown: String
        public var later: String
        public var change: String
        public var continueLabel: String
        public var chooseLater: String
    }

    public struct Downloads: Equatable, Sendable {
        public var speech: String
        public var thinking: String
        public var progress: String
        public var cancel: String
        public var retry: String
        public var error: String
        public var idle: String
        public var complete: String
        public var cancelled: String
    }

    public static func decodeQualified(_ data: Data) throws -> JournalSetupCopy {
        let decoded = try JSONDecoder().decode(Document.self, from: data)
        guard decoded.schema == "solstone.setup-copy.v1" else {
            throw JournalSetupCopyLoaderError.unqualified
        }
        let copy = decoded.copy
        guard copy.isQualified else {
            throw JournalSetupCopyLoaderError.unqualified
        }
        return copy
    }

    var isQualified: Bool {
        !heading.isEmpty
            && !footer.isEmpty
            && welcome.isQualified
            && fits.isQualified
            && login.isQualified
            && folder.isQualified
            && mark.isQualified
            && thinking.isQualified
            && downloads.isQualified
    }
}

extension JournalSetupCopy.Welcome {
    var isQualified: Bool {
        !title.isEmpty && !body.isEmpty && !howItFits.isEmpty && howItFits.allSatisfy { !$0.isEmpty }
    }
}

extension JournalSetupCopy.Fits {
    var isQualified: Bool {
        !title.isEmpty && !journalTerm.isEmpty && !journal.isEmpty
            && !appTerm.isEmpty && !app.isEmpty
            && !thinkingTerm.isEmpty && !thinking.isEmpty
    }
}

extension JournalSetupCopy.Login {
    var isQualified: Bool { !label.isEmpty && !help.isEmpty }
}

extension JournalSetupCopy.Folder {
    var isQualified: Bool {
        !title.isEmpty && !help.isEmpty && !change.isEmpty && !continueLabel.isEmpty
    }
}

extension JournalSetupCopy.Mark {
    var isQualified: Bool {
        !title.isEmpty && !help.isEmpty && !error.isEmpty && !permanence.isEmpty
            && !tryAnother.isEmpty && !continueLabel.isEmpty
    }
}

extension JournalSetupCopy.Thinking {
    var isQualified: Bool {
        !choices.isEmpty
            && choices.allSatisfy(\.isQualified)
            && defaultLine.contains("{size}")
            && !sizeUnknown.isEmpty && !later.isEmpty && !change.isEmpty
            && !continueLabel.isEmpty && !chooseLater.isEmpty
    }
}

extension JournalSetupCopy.Choice {
    var isQualified: Bool {
        !id.isEmpty && !label.isEmpty && !sub.isEmpty && !description.isEmpty
    }
}

extension JournalSetupCopy.Downloads {
    var isQualified: Bool {
        !speech.isEmpty && !thinking.isEmpty && !progress.isEmpty && !cancel.isEmpty
            && !retry.isEmpty && !error.isEmpty && !idle.isEmpty && !complete.isEmpty
            && !cancelled.isEmpty
    }
}

public enum JournalSetupCopyLoaderError: Error, Equatable {
    case failed
    case unqualified
}

public struct JournalSetupCopyLoader: Sendable {
    public static let stdoutCap = 256 * 1024
    public static let timeout: Duration = .seconds(30)

    private let materializer: any RuntimeMaterializing
    private let subprocessRunner: any SubprocessRunning

    public init(
        materializer: any RuntimeMaterializing = NativeJournalRuntimeMaterializer(),
        subprocessRunner: any SubprocessRunning = SubprocessRunner()
    ) {
        self.materializer = materializer
        self.subprocessRunner = subprocessRunner
    }

    public func load() async throws -> JournalSetupCopy {
        let runtime: MaterializedRuntime
        do {
            runtime = try await materializer.materialize(excludingLiveKey: nil)
        } catch {
            throw JournalSetupCopyLoaderError.failed
        }

        var environment = runtime.environment
        environment["SOLSTONE_LOCALE"] = JournalCopy.requestedLanguage
        let inherited = environment["PATH"].flatMap { $0.isEmpty ? nil : $0 }
            ?? "/usr/bin:/bin:/usr/sbin:/sbin"
        environment["PATH"] = runtime.layout.binDir.path + ":" + inherited

        let output = CopyOutput()
        let result: SubprocessResult
        do {
            result = try await withTaskCancellationHandler {
                try Task.checkCancellation()
                return try await subprocessRunner.run(
                    executable: runtime.layout.solstoneBinary,
                    arguments: ["journal", "setup-copy", "--json"],
                    environment: environment,
                    timeout: Self.timeout,
                    stdoutHandler: { chunk in
                        output.append(chunk, cap: Self.stdoutCap, cancel: {
                            subprocessRunner.cancelAll()
                        })
                    },
                    stderrHandler: { _ in }
                )
            } onCancel: {
                subprocessRunner.cancelAll()
            }
        } catch is CancellationError {
            throw CancellationError()
        } catch {
            throw JournalSetupCopyLoaderError.failed
        }

        try Task.checkCancellation()
        guard !output.overrun, result.terminationReason == .exit, result.exitCode == 0 else {
            throw JournalSetupCopyLoaderError.failed
        }
        do {
            return try JournalSetupCopy.decodeQualified(output.data)
        } catch is CancellationError {
            throw CancellationError()
        } catch {
            throw JournalSetupCopyLoaderError.unqualified
        }
    }
}

private final class CopyOutput: @unchecked Sendable {
    private let lock = NSLock()
    private var storage = Data()
    private var didOverrun = false

    var data: Data {
        lock.lock()
        defer { lock.unlock() }
        return storage
    }

    var overrun: Bool {
        lock.lock()
        defer { lock.unlock() }
        return didOverrun
    }

    func append(_ chunk: Data, cap: Int, cancel: () -> Void) {
        let tooBig: Bool
        lock.lock()
        storage.append(chunk)
        tooBig = storage.count > cap
        if tooBig { didOverrun = true }
        lock.unlock()
        if tooBig { cancel() }
    }
}

private struct Document: Decodable {
    var schema: String
    var copy: JournalSetupCopy

    private enum CodingKeys: String, CodingKey {
        case schema, heading, footer, welcome, fits, login, folder, mark, thinking, downloads
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        schema = try container.decode(String.self, forKey: .schema)
        copy = JournalSetupCopy(
            heading: try container.decode(String.self, forKey: .heading),
            footer: try container.decode(String.self, forKey: .footer),
            welcome: try container.decode(WelcomeDocument.self, forKey: .welcome).value,
            fits: try container.decode(FitsDocument.self, forKey: .fits).value,
            login: try container.decode(LoginDocument.self, forKey: .login).value,
            folder: try container.decode(FolderDocument.self, forKey: .folder).value,
            mark: try container.decode(MarkDocument.self, forKey: .mark).value,
            thinking: try container.decode(ThinkingDocument.self, forKey: .thinking).value,
            downloads: try container.decode(DownloadsDocument.self, forKey: .downloads).value
        )
    }
}

private struct WelcomeDocument: Decodable {
    var value: JournalSetupCopy.Welcome

    private enum CodingKeys: String, CodingKey {
        case title, body
        case howItFits = "how_it_fits"
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        value = JournalSetupCopy.Welcome(
            title: try container.decode(String.self, forKey: .title),
            body: try container.decode(String.self, forKey: .body),
            howItFits: try container.decode([String].self, forKey: .howItFits)
        )
    }
}

private struct FitsDocument: Decodable {
    var value: JournalSetupCopy.Fits

    private enum CodingKeys: String, CodingKey {
        case title
        case journalTerm = "journal_term"
        case journal
        case appTerm = "app_term"
        case app
        case thinkingTerm = "thinking_term"
        case thinking
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        value = JournalSetupCopy.Fits(
            title: try container.decode(String.self, forKey: .title),
            journalTerm: try container.decode(String.self, forKey: .journalTerm),
            journal: try container.decode(String.self, forKey: .journal),
            appTerm: try container.decode(String.self, forKey: .appTerm),
            app: try container.decode(String.self, forKey: .app),
            thinkingTerm: try container.decode(String.self, forKey: .thinkingTerm),
            thinking: try container.decode(String.self, forKey: .thinking)
        )
    }
}

private struct LoginDocument: Decodable {
    var value: JournalSetupCopy.Login
    private enum CodingKeys: String, CodingKey { case label, help }
    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        value = JournalSetupCopy.Login(
            label: try container.decode(String.self, forKey: .label),
            help: try container.decode(String.self, forKey: .help)
        )
    }
}

private struct FolderDocument: Decodable {
    var value: JournalSetupCopy.Folder
    private enum CodingKeys: String, CodingKey { case title, help, change, continueLabel = "continue" }
    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        value = JournalSetupCopy.Folder(
            title: try container.decode(String.self, forKey: .title),
            help: try container.decode(String.self, forKey: .help),
            change: try container.decode(String.self, forKey: .change),
            continueLabel: try container.decode(String.self, forKey: .continueLabel)
        )
    }
}

private struct MarkDocument: Decodable {
    var value: JournalSetupCopy.Mark
    private enum CodingKeys: String, CodingKey {
        case title, help, error, permanence
        case tryAnother = "try_another"
        case continueLabel = "continue"
    }
    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        value = JournalSetupCopy.Mark(
            title: try container.decode(String.self, forKey: .title),
            help: try container.decode(String.self, forKey: .help),
            error: try container.decode(String.self, forKey: .error),
            permanence: try container.decode(String.self, forKey: .permanence),
            tryAnother: try container.decode(String.self, forKey: .tryAnother),
            continueLabel: try container.decode(String.self, forKey: .continueLabel)
        )
    }
}

private struct ThinkingDocument: Decodable {
    var value: JournalSetupCopy.Thinking
    private enum CodingKeys: String, CodingKey {
        case title, choices
        case defaultLine = "default_line"
        case sizeUnknown = "size_unknown"
        case later, change
        case continueLabel = "continue"
        case chooseLater = "choose_later"
    }
    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        let rows = try container.decode([ChoiceDocument].self, forKey: .choices)
        value = JournalSetupCopy.Thinking(
            title: try container.decode(String.self, forKey: .title),
            choices: rows.map(\.value),
            defaultLine: try container.decode(String.self, forKey: .defaultLine),
            sizeUnknown: try container.decode(String.self, forKey: .sizeUnknown),
            later: try container.decode(String.self, forKey: .later),
            change: try container.decode(String.self, forKey: .change),
            continueLabel: try container.decode(String.self, forKey: .continueLabel),
            chooseLater: try container.decode(String.self, forKey: .chooseLater)
        )
    }
}

private struct ChoiceDocument: Decodable {
    var value: JournalSetupCopy.Choice
    private enum CodingKeys: String, CodingKey { case id, label, sub, description }
    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        value = JournalSetupCopy.Choice(
            id: try container.decode(String.self, forKey: .id),
            label: try container.decode(String.self, forKey: .label),
            sub: try container.decode(String.self, forKey: .sub),
            description: try container.decode(String.self, forKey: .description)
        )
    }
}

private struct DownloadsDocument: Decodable {
    var value: JournalSetupCopy.Downloads
    private enum CodingKeys: String, CodingKey {
        case speech, thinking, progress, cancel, retry, error, idle, complete, cancelled
    }
    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        value = JournalSetupCopy.Downloads(
            speech: try container.decode(String.self, forKey: .speech),
            thinking: try container.decode(String.self, forKey: .thinking),
            progress: try container.decode(String.self, forKey: .progress),
            cancel: try container.decode(String.self, forKey: .cancel),
            retry: try container.decode(String.self, forKey: .retry),
            error: try container.decode(String.self, forKey: .error),
            idle: try container.decode(String.self, forKey: .idle),
            complete: try container.decode(String.self, forKey: .complete),
            cancelled: try container.decode(String.self, forKey: .cancelled)
        )
    }
}
