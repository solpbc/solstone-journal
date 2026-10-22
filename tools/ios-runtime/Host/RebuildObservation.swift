// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
import CryptoKit
import Darwin
import Foundation

/// Optional development evidence. Notifications wake a reader; they are not publication receipts.
@MainActor final class RebuildObservation {
  private struct UnstableRead: Error { let reason: String }
  private struct Directory {
    let components: [String]
    let device: dev_t
    let inode: ino_t
    let source: DispatchSourceFileSystemObject
  }
  private let root: String
  private let identity: String
  private let deadline: TimeInterval
  private let emit: ([String: Any]) -> Void
  private var directories: [Directory] = []
  private let closed = DispatchGroup()
  private var active = false
  private var priorDigest: String?
  private var observations = 0
  private var wakeups = 0
  private var totalBytes = 0
  private let fileLimit = 1024 * 1024
  private let snapshotLimit = 2 * 1024 * 1024
  private let totalLimit = 16 * 1024 * 1024

  init(root: String, identity: String, seconds: Int, emit: @escaping ([String: Any]) -> Void) throws {
    guard root.hasPrefix("/"), !root.contains("\0"), (1...1700).contains(seconds) else {
      throw Self.refused("invalid observer binding or duration")
    }
    self.root = root
    self.identity = identity
    self.deadline = ProcessInfo.processInfo.systemUptime + Double(seconds)
    self.emit = emit
  }

  private static func refused(_ reason: String) -> NSError {
    NSError(domain: "rebuild-observation-refused", code: 1,
            userInfo: [NSLocalizedDescriptionKey: reason])
  }
  private func rootDescriptor() throws -> Int32 {
    let fd = open(root, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC)
    guard fd >= 0 else { throw Self.refused("root open failed: \(errno)") }
    var metadata = stat()
    guard fstat(fd, &metadata) == 0, metadata.st_dev >= 0,
      String(format: "native-root-v1:unix:%016llx:%016llx",
             UInt64(metadata.st_dev), UInt64(metadata.st_ino)) == identity else {
      close(fd)
      throw Self.refused("native root identity changed")
    }
    return fd
  }
  private func openDirectory(_ components: [String], rootFD: Int32) throws -> Int32 {
    var fd = dup(rootFD)
    guard fd >= 0 else { throw Self.refused("root descriptor duplication failed") }
    do {
      for component in components {
        guard !component.isEmpty, component != ".", component != "..",
          !component.contains("/"), !component.contains("\0") else {
          throw Self.refused("invalid directory component")
        }
        let next = openat(fd, component, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC)
        guard next >= 0 else { throw Self.refused("directory open failed: \(errno)") }
        close(fd)
        fd = next
      }
      return fd
    } catch {
      close(fd)
      throw error
    }
  }

  func start() throws {
    guard !active, directories.isEmpty else { throw Self.refused("observer already started") }
    active = true
    do {
      let rootFD = try rootDescriptor()
      defer { close(rootFD) }
      // The baseline run must already have created this maintained Cortex directory.
      for components in [[], ["health"], ["talents", "entities--entity_suggest"]] {
        let fd = try openDirectory(components, rootFD: rootFD)
        var metadata = stat()
        guard fstat(fd, &metadata) == 0 else {
          close(fd)
          throw Self.refused("directory identity unavailable")
        }
        let source = DispatchSource.makeFileSystemObjectSource(
          fileDescriptor: fd, eventMask: [.write, .rename, .delete, .revoke], queue: .main)
        source.setEventHandler { [weak self] in
          MainActor.assumeIsolated { self?.observe() }
        }
        closed.enter()
        let group = closed
        source.setCancelHandler { close(fd); group.leave() }
        directories.append(Directory(components: components, device: metadata.st_dev,
                                     inode: metadata.st_ino, source: source))
        source.resume()
      }
      try snapshot()
      emit(["kind": "ready", "root": root, "identity": identity,
            "directory_events_are_wakeups_only": true])
    } catch {
      cancel()
      throw error
    }
  }

  private func names(_ fd: Int32) throws -> [String] {
    let copied = dup(fd)
    guard copied >= 0 else { throw Self.refused("directory duplication failed") }
    guard let directory = fdopendir(copied) else {
      close(copied)
      throw Self.refused("directory enumeration failed")
    }
    defer { closedir(directory) }
    var result: [String] = []
    errno = 0
    while let entry = readdir(directory) {
      let name = withUnsafePointer(to: &entry.pointee.d_name) {
        $0.withMemoryRebound(to: CChar.self, capacity: Int(entry.pointee.d_namlen) + 1) {
          String(cString: $0)
        }
      }
      if name != "." && name != ".." {
        guard result.count < 256, !name.contains("/"), !name.contains("\0") else {
          throw Self.refused("Cortex directory exceeds observation bounds")
        }
        result.append(name)
      }
      errno = 0
    }
    guard errno == 0, Set(result).count == result.count else {
      throw Self.refused("Cortex enumeration failed or duplicated a name")
    }
    return result.sorted()
  }

  private func read(_ leaf: String, in directory: Int32) throws -> Data? {
    guard !leaf.isEmpty, leaf != ".", leaf != "..", !leaf.contains("/"), !leaf.contains("\0") else {
      throw Self.refused("invalid file coordinate")
    }
    let fd = openat(directory, leaf, O_RDONLY | O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC)
    if fd < 0 && errno == ENOENT { return nil }
    guard fd >= 0 else { throw Self.refused("file open failed: \(errno)") }
    defer { close(fd) }
    var before = stat()
    guard fstat(fd, &before) == 0, before.st_mode & S_IFMT == S_IFREG,
      before.st_size >= 0, before.st_size <= fileLimit else {
      throw Self.refused("file type, links or size refused")
    }
    guard before.st_nlink == 1 else {
      throw UnstableRead(reason: "file link count is not stable")
    }
    var data = Data()
    var buffer = [UInt8](repeating: 0, count: 16384)
    while true {
      guard active, ProcessInfo.processInfo.systemUptime < deadline else {
        throw Self.refused("observer stopped or expired")
      }
      let count = Darwin.read(fd, &buffer, buffer.count)
      if count < 0 && errno == EINTR { continue }
      guard count >= 0 else { throw Self.refused("file read failed") }
      if count == 0 { break }
      guard data.count + count <= fileLimit else { throw Self.refused("file grew past bound") }
      data.append(contentsOf: buffer.prefix(count))
    }
    var after = stat()
    var named = stat()
    guard fstat(fd, &after) == 0,
      fstatat(directory, leaf, &named, AT_SYMLINK_NOFOLLOW) == 0,
      before.st_dev == after.st_dev, before.st_ino == after.st_ino,
      before.st_size == after.st_size, data.count == after.st_size,
      before.st_mtimespec.tv_sec == after.st_mtimespec.tv_sec,
      before.st_mtimespec.tv_nsec == after.st_mtimespec.tv_nsec,
      after.st_dev == named.st_dev, after.st_ino == named.st_ino,
      named.st_mode & S_IFMT == S_IFREG, named.st_nlink == 1 else {
      throw UnstableRead(reason: "file changed during observation")
    }
    return data
  }

  private func snapshot() throws {
    guard active, observations < 1024, ProcessInfo.processInfo.systemUptime < deadline else {
      throw Self.refused("observer stopped or exhausted")
    }
    let rootFD = try rootDescriptor()
    defer { close(rootFD) }
    var files: [[String: Any]] = []
    var digest = SHA256()
    var bytes = 0
    for directory in directories {
      let fd = try openDirectory(directory.components, rootFD: rootFD)
      defer { close(fd) }
      var metadata = stat()
      guard fstat(fd, &metadata) == 0, metadata.st_dev == directory.device,
        metadata.st_ino == directory.inode else { throw Self.refused("watched directory replaced") }
      if directory.components.isEmpty { continue }
      let leaves = directory.components == ["health"] ? ["work-intent.json"] :
        try names(fd).filter { $0.hasSuffix("_active.jsonl") }
      guard leaves.count <= 32 else { throw Self.refused("too many active uses") }
      for leaf in leaves {
        let coordinate = (directory.components + [leaf]).joined(separator: "/")
        let data = try read(leaf, in: fd)
        digest.update(data: Data(coordinate.utf8))
        guard let data else {
          files.append(["path": coordinate, "absent": true])
          continue
        }
        if leaf == "work-intent.json" {
          guard let value = try JSONSerialization.jsonObject(with: data) as? [String: Any],
            value["version"] as? Int == 1, value["days"] is [String: Any] else {
            throw Self.refused("malformed work intent")
          }
        } else {
          guard data.last == 10, !data.isEmpty else { throw Self.refused("incomplete active use") }
          for line in data.split(separator: 10) {
            guard try JSONSerialization.jsonObject(with: Data(line)) is [String: Any] else {
              throw Self.refused("malformed active use")
            }
          }
        }
        bytes += data.count
        guard bytes <= snapshotLimit else { throw Self.refused("snapshot exceeds byte bound") }
        digest.update(data: data)
        files.append(["path": coordinate, "bytes": data.count,
                      "sha256": SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined(),
                      "base64": data.base64EncodedString()])
      }
    }
    let validationFD = try rootDescriptor()
    defer { close(validationFD) }
    for directory in directories {
      let fd = try openDirectory(directory.components, rootFD: validationFD)
      defer { close(fd) }
      var metadata = stat()
      guard fstat(fd, &metadata) == 0, metadata.st_dev == directory.device,
        metadata.st_ino == directory.inode else { throw Self.refused("directory changed during observation") }
    }
    let value = digest.finalize().map { String(format: "%02x", $0) }.joined()
    if value == priorDigest { return }
    guard totalBytes + bytes <= totalLimit else { throw Self.refused("total byte bound reached") }
    totalBytes += bytes
    observations += 1
    priorDigest = value
    emit(["kind": "snapshot", "root": root, "identity": identity,
          "uptime": ProcessInfo.processInfo.systemUptime, "sequence": observations,
          "files": files, "atomic_multi_file_snapshot": false,
          "eligibility_or_signal_authority": false])
  }

  private func observe() {
    guard active else { return }
    wakeups += 1
    do {
      guard wakeups <= 1024 else { throw Self.refused("notification bound reached") }
      try snapshot()
    } catch let error as UnstableRead {
      emit(["kind": "sample.refused", "error": error.reason,
            "eligibility_or_signal_authority": false])
      // Only another directory notification can trigger another read.
    } catch {
      emit(["kind": "refused", "error": String(describing: error),
            "eligibility_or_signal_authority": false])
      cancel()
    }
  }
  func cancel() {
    active = false
    for directory in directories { directory.source.cancel() }
  }
  func stop() async {
    cancel()
    await withCheckedContinuation { continuation in
      closed.notify(queue: .main) { continuation.resume() }
    }
  }
}
