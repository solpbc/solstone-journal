// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
import Darwin
import Foundation

/// Copies a picked folder. Bookmark bytes are not a field of the public result.
enum OwnerFolderCopy {
    static let bufferBytes = 65_536
    static let maxDescriptors = 8

    static let ready = "Folder ready to preview."
    static let cancelled = "Folder selection cancelled."
    static let denied = "The folder could not be read."
    static let unavailable = "The selected folder is no longer available."
    static let storageFull = "Copy stopped because storage is full."
    static let truncated = "Copy stopped because the folder changed during the copy."
    static let noMtime = "A file in this folder has no modified time."
    static let special = "import sources must contain only regular files and folders"
    static let escape = "That entry is outside the selected folder."
    static let duplicate = "That folder contains two entries with the same name."

    struct Authority: Equatable {
        var token: String
        var generation: Int
    }

    /// Page authority re-read before each file and before ready.
    final class LiveAuthority {
        let requested: Authority
        private let lock = NSLock()
        private var current: Authority

        init(requested: Authority, live: Authority) {
            self.requested = requested
            self.current = live
        }

        var matches: Bool {
            lock.lock()
            defer { lock.unlock() }
            return requested == current
        }

        func setLive(_ authority: Authority) {
            lock.lock()
            defer { lock.unlock() }
            current = authority
        }
    }

    struct CopyResult {
        let authorityMatches: Bool
        let ownerMessage: String
    }

    protocol FolderSource {
        func children(_ relative: String) throws -> [String]
        func open(_ relative: String) throws -> InputStream
        func size(_ relative: String) throws -> UInt64
        func directory(_ relative: String) -> Bool
        func symlink(_ relative: String) -> Bool
        func modifiedMillis(_ relative: String) -> Int64?
    }

    static func copy(
        requested: Authority,
        live: Authority,
        source: FolderSource,
        destination: URL,
        cancel: AtomicFlag
    ) -> CopyResult {
        copy(
            requested: requested,
            live: LiveAuthority(requested: requested, live: live),
            source: source,
            destination: destination,
            cancel: cancel
        )
    }

    static func copy(
        requested: Authority,
        live: LiveAuthority,
        source: FolderSource,
        destination: URL,
        cancel: AtomicFlag
    ) -> CopyResult {
        guard requested == live.requested, live.matches else {
            return CopyResult(authorityMatches: false, ownerMessage: denied)
        }
        let custody = destination.appendingPathComponent("folder-custody.json")
        if FileManager.default.fileExists(atPath: custody.path) {
            return CopyResult(authorityMatches: true, ownerMessage: denied)
        }
        if cancel.value {
            return CopyResult(authorityMatches: true, ownerMessage: cancelled)
        }
        let createdRoot = !FileManager.default.fileExists(atPath: destination.path)
        let budget = DescriptorBudget(limit: maxDescriptors)
        let buffer = UnsafeMutablePointer<UInt8>.allocate(capacity: bufferBytes)
        defer { buffer.deallocate() }
        var created: [URL] = []
        do {
            try FileManager.default.createDirectory(at: destination, withIntermediateDirectories: true)
            if createdRoot { created.append(destination) }
            var seen = Set<String>()
            try copyTree(
                authority: live,
                source: source,
                relative: "",
                destination: destination,
                buffer: buffer,
                budget: budget,
                cancel: cancel,
                seen: &seen,
                created: &created
            )
            if cancel.value {
                deleteCreated(created)
                return CopyResult(authorityMatches: true, ownerMessage: cancelled)
            }
            if !live.matches {
                deleteCreated(created)
                return CopyResult(authorityMatches: false, ownerMessage: denied)
            }
            return CopyResult(authorityMatches: true, ownerMessage: ready)
        } catch is AuthorityStop {
            deleteCreated(created)
            return CopyResult(authorityMatches: false, ownerMessage: denied)
        } catch let stop as CopyStop {
            deleteCreated(created)
            return CopyResult(authorityMatches: true, ownerMessage: stop.message)
        } catch {
            deleteCreated(created)
            return CopyResult(authorityMatches: true, ownerMessage: denied)
        }
    }

    /// A detached path or bookmark is not a folder page. The denial does not echo either value.
    static func refuseDetached(path: String, bookmark: Data) -> CopyResult {
        if path.isEmpty && bookmark.isEmpty {
            return CopyResult(authorityMatches: false, ownerMessage: denied)
        }
        return CopyResult(authorityMatches: false, ownerMessage: denied)
    }

    /// Resolves the stored bookmark and compares standardized URLs. Bookmark blobs are not compared.
    static func sameGrant(picker: URL, storedBookmark: Data) -> Bool {
        var stale = false
        guard let resolved = try? URL(
            resolvingBookmarkData: storedBookmark,
            options: [.withoutUI],
            relativeTo: nil,
            bookmarkDataIsStale: &stale
        ) else {
            return false
        }
        guard resolved.startAccessingSecurityScopedResource() else {
            return false
        }
        defer { resolved.stopAccessingSecurityScopedResource() }
        return picker.standardizedFileURL == resolved.standardizedFileURL
    }

    /// Bookmark data is created only when the owner asked to remember the folder.
    static func rememberedMaterial(grant: URL, remembered: Bool) -> Data? {
        guard remembered else { return nil }
        guard grant.startAccessingSecurityScopedResource() else {
            return nil
        }
        defer { grant.stopAccessingSecurityScopedResource() }
        return try? grant.bookmarkData(
            options: [],
            includingResourceValuesForKeys: nil,
            relativeTo: nil
        )
    }

    private final class DescriptorBudget {
        private var live = 0
        let limit: Int

        init(limit: Int) { self.limit = limit }

        func acquire() throws {
            if live >= limit { throw CopyStop(denied) }
            live += 1
        }

        func release() {
            if live > 0 { live -= 1 }
        }
    }

    private struct CopyStop: Error {
        let message: String
        init(_ message: String) { self.message = message }
    }

    private struct AuthorityStop: Error {}

    private static func copyTree(
        authority: LiveAuthority,
        source: FolderSource,
        relative: String,
        destination: URL,
        buffer: UnsafeMutablePointer<UInt8>,
        budget: DescriptorBudget,
        cancel: AtomicFlag,
        seen: inout Set<String>,
        created: inout [URL]
    ) throws {
        if cancel.value { throw CopyStop(cancelled) }
        if !authority.matches { throw AuthorityStop() }
        if source.symlink(relative) { throw CopyStop(special) }
        if source.directory(relative) {
            if !relative.isEmpty {
                try createDirectory(source: source, relative: relative, destination: destination, created: &created)
            }
            try budget.acquire()
            let names: [String]
            do {
                names = try source.children(relative)
                budget.release()
            } catch {
                budget.release()
                throw error
            }
            for name in names {
                guard oneComponent(name) else { throw CopyStop(escape) }
                let child = relative.isEmpty ? name : relative + "/" + name
                guard seen.insert(child).inserted else { throw CopyStop(duplicate) }
                try copyTree(
                    authority: authority,
                    source: source,
                    relative: child,
                    destination: destination,
                    buffer: buffer,
                    budget: budget,
                    cancel: cancel,
                    seen: &seen,
                    created: &created
                )
            }
            return
        }
        if cancel.value { throw CopyStop(cancelled) }
        if !authority.matches { throw AuthorityStop() }
        guard let modified = source.modifiedMillis(relative) else { throw CopyStop(noMtime) }
        let expected = try source.size(relative)
        try budget.acquire()
        let input: InputStream
        do {
            input = try source.open(relative)
        } catch {
            budget.release()
            throw error
        }
        try budget.acquire()
        defer {
            budget.release()
            budget.release()
            input.close()
        }
        let outputURL = destination.appendingPathComponent(relative)
        let parent = outputURL.deletingLastPathComponent()
        noteMissing(root: destination, path: parent, created: &created)
        try FileManager.default.createDirectory(
            at: parent,
            withIntermediateDirectories: true
        )
        if isOccupied(outputURL) { throw CopyStop(denied) }
        let fd = Darwin.open(outputURL.path, O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW, 0o600)
        if fd < 0 { throw CopyStop(denied) }
        created.append(outputURL)
        let handle = FileHandle(fileDescriptor: fd, closeOnDealloc: true)
        defer { try? handle.close() }
        input.open()
        var copied: UInt64 = 0
        while copied < expected {
            if cancel.value { throw CopyStop(cancelled) }
            if !authority.matches { throw AuthorityStop() }
            let asked = Int(min(UInt64(bufferBytes), expected - copied))
            let read = input.read(buffer, maxLength: asked)
            if read < 0 { throw CopyStop(denied) }
            if read == 0 { throw CopyStop(truncated) }
            try handle.write(contentsOf: Data(bytes: buffer, count: read))
            copied += UInt64(read)
        }
        if copied != expected { throw CopyStop(truncated) }
        let extra = input.read(buffer, maxLength: 1)
        if extra > 0 { throw CopyStop(truncated) }
        if extra < 0 { throw CopyStop(denied) }
        try FileManager.default.setAttributes(
            [.modificationDate: Date(timeIntervalSince1970: Double(modified) / 1000)],
            ofItemAtPath: outputURL.path
        )
    }

    private static func createDirectory(
        source: FolderSource,
        relative: String,
        destination: URL,
        created: inout [URL]
    ) throws {
        let dir = destination.appendingPathComponent(relative, isDirectory: true)
        if isSymbolicLink(dir) { throw CopyStop(denied) }
        var isDir: ObjCBool = false
        let exists = FileManager.default.fileExists(atPath: dir.path, isDirectory: &isDir)
        if exists && !isDir.boolValue { throw CopyStop(denied) }
        var createdDir = false
        if !exists {
            noteMissing(root: destination, path: dir.deletingLastPathComponent(), created: &created)
            try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
            created.append(dir)
            createdDir = true
        }
        if createdDir, let modified = source.modifiedMillis(relative) {
            try FileManager.default.setAttributes(
                [.modificationDate: Date(timeIntervalSince1970: Double(modified) / 1000)],
                ofItemAtPath: dir.path
            )
        }
    }

    private static func isOccupied(_ url: URL) -> Bool {
        if isSymbolicLink(url) { return true }
        return FileManager.default.fileExists(atPath: url.path)
    }

    private static func isSymbolicLink(_ url: URL) -> Bool {
        (try? url.resourceValues(forKeys: [.isSymbolicLinkKey]))?.isSymbolicLink == true
    }

    private static func oneComponent(_ name: String) -> Bool {
        !name.isEmpty && name != "." && name != ".." && !name.contains("/") && !name.contains("\\")
    }

    private static func noteMissing(root: URL, path: URL, created: inout [URL]) {
        var missing: [URL] = []
        var cursor = path.standardizedFileURL
        let rootURL = root.standardizedFileURL
        while cursor != rootURL && !FileManager.default.fileExists(atPath: cursor.path) {
            missing.append(cursor)
            let parent = cursor.deletingLastPathComponent().standardizedFileURL
            if parent == cursor { break }
            cursor = parent
        }
        for item in missing.reversed() {
            created.append(item)
        }
    }

    private static func deleteCreated(_ created: [URL]) {
        for url in created.reversed() {
            try? FileManager.default.removeItem(at: url)
        }
    }
}

/// A cancellation flag the copy checks between reads. It is not a page path.
final class AtomicFlag {
    private let lock = NSLock()
    private var stored: Bool

    init(_ stored: Bool) { self.stored = stored }

    func set(_ value: Bool) {
        lock.lock()
        defer { lock.unlock() }
        stored = value
    }

    var value: Bool {
        lock.lock()
        defer { lock.unlock() }
        return stored
    }
}
