// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
import Darwin
import Foundation

/// Provider reads descend from the picked directory handle. Each observed entry
/// must still name that same regular file or directory when it is opened.
final class OwnerFolderSource: OwnerFolderCopy.FolderSource {
    private let root: Int32
    private var observed: [String: stat] = [:]

    init(_ url: URL) throws {
        let descriptor = Darwin.open(url.path, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC)
        guard descriptor >= 0 else { throw CocoaError(.fileReadUnknown) }
        var value = stat()
        guard fstat(descriptor, &value) == 0 else {
            Darwin.close(descriptor)
            throw CocoaError(.fileReadUnknown)
        }
        root = descriptor
        observed[""] = value
    }

    deinit { Darwin.close(root) }

    private func check(_ relative: String, _ value: stat) throws {
        if let original = observed[relative] {
            guard original.st_dev == value.st_dev, original.st_ino == value.st_ino,
                  original.st_mode & S_IFMT == value.st_mode & S_IFMT else {
                throw CocoaError(.fileReadUnknown)
            }
        } else {
            observed[relative] = value
        }
    }

    private func directoryHandle(_ relative: String) throws -> Int32 {
        var current = dup(root)
        guard current >= 0 else { throw CocoaError(.fileReadUnknown) }
        do {
            var prefix = ""
            var value = stat()
            guard fstat(current, &value) == 0 else { throw CocoaError(.fileReadUnknown) }
            try check("", value)
            for component in relative.split(separator: "/", omittingEmptySubsequences: false) where !relative.isEmpty {
                guard !component.isEmpty, component != ".", component != "..", !component.contains("\\") else {
                    throw CocoaError(.fileReadUnknown)
                }
                let next = openat(current, String(component), O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC)
                guard next >= 0 else { throw CocoaError(.fileReadUnknown) }
                Darwin.close(current)
                current = next
                prefix = prefix.isEmpty ? String(component) : prefix + "/" + component
                guard fstat(current, &value) == 0 else { throw CocoaError(.fileReadUnknown) }
                try check(prefix, value)
            }
            return current
        } catch {
            Darwin.close(current)
            throw error
        }
    }

    private func parent(_ relative: String) throws -> (Int32, String) {
        let parts = relative.split(separator: "/", omittingEmptySubsequences: false)
        guard let name = parts.last, !name.isEmpty, name != ".", name != "..", !name.contains("\\") else {
            throw CocoaError(.fileReadUnknown)
        }
        return (try directoryHandle(parts.dropLast().joined(separator: "/")), String(name))
    }

    private func observation(_ relative: String) throws -> stat {
        var value = stat()
        if relative.isEmpty {
            guard fstat(root, &value) == 0 else { throw CocoaError(.fileReadUnknown) }
        } else {
            let (directory, name) = try parent(relative)
            defer { Darwin.close(directory) }
            guard fstatat(directory, name, &value, AT_SYMLINK_NOFOLLOW) == 0 else { throw CocoaError(.fileReadUnknown) }
        }
        try check(relative, value)
        return value
    }

    func children(_ relative: String) throws -> [String] {
        let directory = try directoryHandle(relative)
        guard let stream = fdopendir(directory) else {
            Darwin.close(directory)
            throw CocoaError(.fileReadUnknown)
        }
        defer { closedir(stream) }
        var result: [String] = []
        while true {
            errno = 0
            guard let entry = readdir(stream) else {
                if errno != 0 { throw CocoaError(.fileReadUnknown) }
                return result
            }
            let name = withUnsafePointer(to: &entry.pointee.d_name) {
                $0.withMemoryRebound(to: CChar.self, capacity: MemoryLayout.size(ofValue: entry.pointee.d_name)) {
                    String(validatingCString: $0)
                }
            }
            guard let name else { throw CocoaError(.fileReadUnknown) }
            if name != "." && name != ".." { result.append(name) }
        }
    }

    func open(_ relative: String) throws -> InputStream {
        let expected = try observation(relative)
        guard expected.st_mode & S_IFMT == S_IFREG else { throw CocoaError(.fileReadUnknown) }
        let (directory, name) = try parent(relative)
        defer { Darwin.close(directory) }
        let descriptor = openat(directory, name, O_RDONLY | O_NONBLOCK | O_NOFOLLOW | O_CLOEXEC)
        guard descriptor >= 0 else { throw CocoaError(.fileReadUnknown) }
        do {
            var actual = stat()
            guard fstat(descriptor, &actual) == 0, actual.st_mode & S_IFMT == S_IFREG else { throw CocoaError(.fileReadUnknown) }
            try check(relative, actual)
            return DescriptorInput(descriptor)
        } catch {
            Darwin.close(descriptor)
            throw error
        }
    }

    func size(_ relative: String) throws -> UInt64 {
        let value = try observation(relative)
        guard value.st_size >= 0 else { throw CocoaError(.fileReadUnknown) }
        return UInt64(value.st_size)
    }

    func directory(_ relative: String) -> Bool {
        guard let value = try? observation(relative) else { return false }
        return value.st_mode & S_IFMT == S_IFDIR
    }

    func symlink(_ relative: String) -> Bool {
        guard let value = try? observation(relative) else { return true }
        let type = value.st_mode & S_IFMT
        return type != S_IFDIR && type != S_IFREG
    }

    func modifiedMillis(_ relative: String) -> Int64? {
        guard let value = try? observation(relative) else { return nil }
        let seconds = Int64(value.st_mtimespec.tv_sec)
        let (millis, overflow) = seconds.multipliedReportingOverflow(by: 1000)
        guard !overflow else { return nil }
        let (result, sumOverflow) = millis.addingReportingOverflow(Int64(value.st_mtimespec.tv_nsec / 1_000_000))
        return sumOverflow ? nil : result
    }

    private final class DescriptorInput: InputStream {
        private var descriptor: Int32
        init(_ descriptor: Int32) { self.descriptor = descriptor; super.init(data: Data()) }
        override func open() {}
        override func close() {
            if descriptor >= 0 { Darwin.close(descriptor); descriptor = -1 }
        }
        override func read(_ buffer: UnsafeMutablePointer<UInt8>, maxLength len: Int) -> Int {
            guard descriptor >= 0 else { return -1 }
            return Darwin.read(descriptor, buffer, len)
        }
        deinit { close() }
    }
}
