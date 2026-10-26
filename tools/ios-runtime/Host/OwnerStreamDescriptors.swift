// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
#if JOURNAL_OWNER_APP
import Darwin
import Foundation

/// Body writer and response reader for one owner stream.
///
/// `retireIfIdle` closes the numbers only when `perform` has not claimed them.
/// After a claim, the worker's `perform` return is the only closer, so a cancel
/// on the UI thread cannot free a number the worker is about to pass to the kernel.
final class OwnerStreamDescriptors: @unchecked Sendable {
  let writer: Int32
  let reader: Int32
  private let lock = NSLock()
  private var claimed = false
  private var cancelled = false
  private var writerOpen = true
  private var readerOpen = true

  init(writer: Int32, reader: Int32) {
    self.writer = writer
    self.reader = reader
  }

  /// Runs `work` only after this call owns the numbers. A cancelled or
  /// already-claimed pair returns without touching them.
  func perform(_ work: () -> Void) {
    lock.lock()
    if cancelled || claimed {
      lock.unlock()
      return
    }
    claimed = true
    lock.unlock()
    defer { release() }
    work()
  }

  /// Closes the pair when the worker has not claimed it. A claimed pair stays
  /// open; closing it here would let the kernel reuse the number under `work`.
  func retireIfIdle() -> Bool {
    lock.lock()
    cancelled = true
    if claimed {
      lock.unlock()
      return false
    }
    let writer = takeWriter()
    let reader = takeReader()
    lock.unlock()
    close(writer)
    close(reader)
    return true
  }

  /// Closes the body writer once so the request can finish. The response reader stays open.
  func closeWriter() -> Bool {
    lock.lock()
    let fd = takeWriter()
    lock.unlock()
    if fd < 0 { return false }
    close(fd)
    return true
  }

  private func release() {
    lock.lock()
    let writer = takeWriter()
    let reader = takeReader()
    lock.unlock()
    close(writer)
    close(reader)
  }

  deinit { release() }

  private func takeWriter() -> Int32 {
    guard writerOpen else { return -1 }
    writerOpen = false
    return writer
  }

  private func takeReader() -> Int32 {
    guard readerOpen else { return -1 }
    readerOpen = false
    return reader
  }

  private func close(_ fd: Int32) {
    if fd >= 0 { Darwin.close(fd) }
  }
}
#endif
