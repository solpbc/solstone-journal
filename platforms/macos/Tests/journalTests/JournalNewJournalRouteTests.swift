// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import Foundation
import JournalMarkKit
import JournalRuntime
import Testing
@testable import journal

@MainActor
@Suite("JournalNewJournalRoute")
struct JournalNewJournalRouteTests {
    @Test func startingANewJournalStopsTheOldOneLeavesItsFolderAndStartsSetupElsewhere() async throws {
        let trace = FirstRunTrace()
        let old = try makeRouteDirectory()
        let marker = old.appendingPathComponent("kept.txt")
        try Data("kept".utf8).write(to: marker)
        let fixture = makeModel(trace: trace, startResults: [true], probeResults: [.complete])
        await fixture.model.resumeConfiguredRoot(old)
        #expect(fixture.model.route == .home)

        await fixture.model.startNewJournal()

        #expect(await trace.snapshot() == ["supervisor", "probe", "stop"])
        #expect(fixture.config.journalRoot == nil)
        #expect(fixture.config.knownJournalRoots == [old.standardizedFileURL])
        #expect(fixture.model.route == .ritual(.nameLocation))
        #expect(!JournalFirstRunModel.journalFolders(fixture.model.journalRoot, overlap: old))
        #expect(fixture.model.otherJournalRoots == [old.standardizedFileURL])
        #expect(fixture.model.returnJournalRoot == old.standardizedFileURL)
        #expect(try Data(contentsOf: marker) == Data("kept".utf8))
        #expect(try FileManager.default.contentsOfDirectory(atPath: old.path) == ["kept.txt"])
    }

    @Test(arguments: ["same", "inside", "holding"])
    func setupRefusesAFolderThatOverlapsAnotherJournal(_ shape: String) async throws {
        let trace = FirstRunTrace()
        let parent = try makeRouteDirectory()
        let old = parent.appendingPathComponent("journal", isDirectory: true)
        try FileManager.default.createDirectory(at: old, withIntermediateDirectories: true)
        let fixture = makeModel(trace: trace, startResults: [true], probeResults: [.complete])
        await fixture.model.resumeConfiguredRoot(old)
        await fixture.model.startNewJournal()

        switch shape {
        case "same": fixture.model.journalRoot = old
        case "inside": fixture.model.journalRoot = old.appendingPathComponent("new", isDirectory: true)
        default: fixture.model.journalRoot = parent
        }
        await fixture.model.continueFromNameLocation()

        #expect(fixture.model.errorMessage == JournalFirstRunCopy.folderHoldsAnotherJournal)
        #expect(fixture.config.journalRoot == nil)
        #expect(await trace.snapshot() == ["supervisor", "probe", "stop"])
    }

    @Test func goingBackFromSetupKeepsTheOldJournalAgainAndRemembersTheNewFolder() async throws {
        let trace = FirstRunTrace()
        let old = try makeRouteDirectory()
        let new = try makeRouteDirectory()
        var started: [URL] = []
        let probe = RootProbe()
        let fixture = makeModel(
            trace: trace, startResults: [true, true, true], probeResults: [],
            initClient: probe,
            willStartSupervisor: { root in
                started.append(root.standardizedFileURL)
                probe.set(root.standardizedFileURL == new ? .incomplete : .complete)
            }
        )
        await fixture.model.resumeConfiguredRoot(old)
        await fixture.model.startNewJournal()
        fixture.model.journalRoot = new
        // The new journal's setup opens its page in the browser and waits there.
        let setup = Task { await fixture.model.continueFromNameLocation() }
        while fixture.model.route != .ritual(.finishInBrowser) { await Task.yield() }

        await fixture.model.switchToJournal(old)
        await setup.value

        #expect(fixture.config.journalRoot == old.standardizedFileURL)
        // The switch binds the installation to the journal before starting it.
        let events = await trace.snapshot()
        let bind = try #require(events.lastIndex(of: "bind:" + old.lastPathComponent))
        #expect(events[(bind + 1)...].first == "supervisor")
        #expect(fixture.config.knownJournalRoots == [old.standardizedFileURL, new.standardizedFileURL])
        #expect(started == [old, new, old].map(\.standardizedFileURL))
        #expect(fixture.model.route == .home)
        #expect(fixture.model.otherJournalRoots == [new.standardizedFileURL])
    }

    @Test func aJournalWhoseSetupContinuesInTheSolstoneAppCannotBeLeft() async throws {
        let trace = FirstRunTrace()
        let fixture = makeModel(trace: trace, startResults: [true], probeResults: [.complete])
        await fixture.model.resumeConfiguredRoot(try makeRouteDirectory())
        _ = try fixture.continuation.storeLocal(root: try makeRouteDirectory())
        #expect(!fixture.model.canChangeJournal)
        await fixture.model.startNewJournal()
        #expect(fixture.model.route == .home)
        #expect(await trace.snapshot() == ["supervisor", "probe"])
    }

    @Test func theNewJournalConfirmationNamesWhatARestoreDoesToThisJournalsBackup() async throws {
        let trace = FirstRunTrace()
        let root = try makeRouteDirectory()
        let fixture = makeModel(trace: trace, startResults: [true], probeResults: [.complete])
        await fixture.model.resumeConfiguredRoot(root)
        #expect(fixture.model.newJournalConfirmMessage() == JournalLocationCopy.startNewJournalConfirm(current: root))
        try writeBackup(root, mode: "byo", repository: "s3:https://example.invalid/r")
        // A restore keeps the backups made before it on the owner's own storage: nothing to say.
        #expect(fixture.model.newJournalConfirmMessage() == JournalLocationCopy.startNewJournalConfirm(current: root))
        try writeBackup(root, mode: "operated", prefix: "acct/inst")
        #expect(fixture.model.newJournalConfirmMessage().hasSuffix(JournalLocationCopy.newJournalHostedShare))
    }

    @Test func foldersOverlapWhenEqualOrNestedIgnoringCase() {
        let a = URL(fileURLWithPath: "/Users/o/journal", isDirectory: true)
        #expect(JournalFirstRunModel.journalFolders(a, overlap: URL(fileURLWithPath: "/Users/o/Journal/")))
        #expect(JournalFirstRunModel.journalFolders(a, overlap: URL(fileURLWithPath: "/Users/o")))
        #expect(JournalFirstRunModel.journalFolders(a, overlap: URL(fileURLWithPath: "/Users/o/journal/x")))
        #expect(!JournalFirstRunModel.journalFolders(a, overlap: URL(fileURLWithPath: "/Users/o/journal-2")))
    }

    @Test func aSharedBackupIsFoundPerStorageAndOnlyWhenOn() throws {
        let a = try makeRouteDirectory(), b = try makeRouteDirectory(), c = try makeRouteDirectory()
        try writeBackup(a, mode: "operated", prefix: "acct/inst")
        try writeBackup(b, mode: "operated", prefix: "acct/inst")
        try writeBackup(c, mode: "operated", prefix: "acct/other")
        #expect(JournalBackupShare.find(target: a, among: [c, b]) == JournalBackupShare(place: .hosted(prefix: "acct/inst"), other: b))
        #expect(JournalBackupShare.find(target: c, among: [a, b]) == nil)

        try writeBackup(a, mode: "byo", repository: "s3:https://example.invalid/r")
        try writeBackup(b, mode: "byo", repository: "s3:https://example.invalid/r")
        #expect(JournalBackupShare.find(target: b, among: [a])?.place == .own(repository: "s3:https://example.invalid/r"))
        try writeBackup(a, mode: "byo", repository: "s3:https://example.invalid/r", enabled: false)
        #expect(JournalBackupShare.find(target: b, among: [a]) == nil)
    }

    @Test func theSwitchConfirmationNamesTheSharedBackupAndTheSharedMark() async throws {
        let trace = FirstRunTrace()
        let old = try makeRouteDirectory(), clone = try makeRouteDirectory()
        for root in [old, clone] {
            try writeBackup(root, mode: "operated", prefix: "acct/inst")
            try writeInstance(root, "inst")
        }
        let fixture = makeModel(trace: trace, startResults: [true], probeResults: [.complete])
        await fixture.model.resumeConfiguredRoot(clone)
        fixture.config.rememberJournalRoot(old)

        let message = fixture.model.switchConfirmMessage(target: old)
        #expect(message.contains(JournalLocationCopy.switchConfirm(current: clone)))
        #expect(message.contains(try #require(JournalLocationCopy.shared(JournalBackupShare(place: .hosted(prefix: "acct/inst"), other: clone)))))
        #expect(JournalLocationCopy.shared(JournalBackupShare(place: .own(repository: "r"), other: clone)) == nil)
        #expect(message.contains(JournalLocationCopy.sameMarkTemplate))

        try writeInstance(clone, "fresh")
        try writeBackup(clone, mode: "byo", repository: "elsewhere")
        #expect(fixture.model.switchConfirmMessage(target: old) == JournalLocationCopy.switchConfirm(current: clone))
    }
}

private final class RootProbe: JournalInitClienting, @unchecked Sendable {
    private let lock = NSLock()
    private var result: JournalInitSetupProbe = .complete

    func set(_ value: JournalInitSetupProbe) { lock.withLock { result = value } }

    func probeSetupComplete() async throws -> JournalInitSetupProbe { lock.withLock { result } }
}

private func makeRouteDirectory() throws -> URL {
    let url = FileManager.default.temporaryDirectory
        .appendingPathComponent("journal-route-\(UUID().uuidString)", isDirectory: true)
    try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
    return url.standardizedFileURL
}

private func writeBackup(_ root: URL, mode: String, prefix: String? = nil, repository: String? = nil, enabled: Bool = true) throws {
    var backup: [String: Any] = ["enabled": enabled, "mode": mode]
    if let repository { backup["destination"] = ["repository": repository] }
    let config = root.appendingPathComponent("config", isDirectory: true)
    try FileManager.default.createDirectory(at: config, withIntermediateDirectories: true)
    try JSONSerialization.data(withJSONObject: ["backup": backup]).write(to: config.appendingPathComponent("journal.json"))
    if let prefix {
        let hosted = root.appendingPathComponent("backup/hosted", isDirectory: true)
        try FileManager.default.createDirectory(at: hosted, withIntermediateDirectories: true)
        try JSONSerialization.data(withJSONObject: ["prefix": prefix]).write(to: hosted.appendingPathComponent("binding.json"))
    }
}

private func writeInstance(_ root: URL, _ id: String) throws {
    let link = root.appendingPathComponent("link/ca", isDirectory: true)
    try FileManager.default.createDirectory(at: link, withIntermediateDirectories: true)
    try JSONSerialization.data(withJSONObject: ["instance_id": id]).write(to: link.appendingPathComponent("state.json"))
}
