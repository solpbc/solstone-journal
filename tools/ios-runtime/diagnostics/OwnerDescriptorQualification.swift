// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
// Host control for owner stream descriptor lifetime. From tools/ios-runtime:
// swiftc -D JOURNAL_OWNER_APP Host/OwnerPipe.swift Host/OwnerStreamDescriptors.swift diagnostics/OwnerDescriptorQualification.swift
#if JOURNAL_OWNER_APP
import Darwin
import Foundation

/// Proves cancel closes idle descriptor numbers, a running worker keeps those
/// numbers through its next read and write, and a queued operation cannot write
/// a number the kernel has already given to a new pipe.
@main enum OwnerDescriptorQualification {
  static func main() throws {
    signal(SIGPIPE, SIG_IGN)
    try closeWriterThenReleaseDoesNotCloseTheReusedNumber()
    try inFlightCancelLeavesTheWorkerPipeIntact()
    try queuedStartCannotWriteTheReusedNumber()
    try cancelledQueueDropsTheOperationWithoutALeak()
    print("owner descriptors: in-flight reuse, queued start, and never-started close passed")
  }

  private static func closeWriterThenReleaseDoesNotCloseTheReusedNumber() throws {
    let body = try OwnerPipe.make()
    let response = try OwnerPipe.make()
    defer { close(body.reader); close(response.writer) }
    let descriptors = OwnerStreamDescriptors(writer: body.writer, reader: response.reader)
    var reused: Reused?
    descriptors.perform {
      precondition(descriptors.closeWriter(), "body writer stayed open")
      precondition(closed(body.writer))
      precondition(live(response.reader))
      precondition(!descriptors.closeWriter(), "body writer closed twice")
      reused = occupy(body.writer)
      reused?.write(0x5A)
    }
    precondition(!descriptors.retireIfIdle(), "finished worker was treated as idle")
    precondition(closed(response.reader), "response reader leaked after perform")
    guard let reused else { preconditionFailure("reused pipe missing") }
    reused.expect(0x5A)
    reused.closeEnds()
  }

  private static func inFlightCancelLeavesTheWorkerPipeIntact() throws {
    let body = try OwnerPipe.make()
    let response = try OwnerPipe.make()
    defer { close(body.reader); close(response.writer) }
    let descriptors = OwnerStreamDescriptors(writer: body.writer, reader: response.reader)
    let writerIdentity = identity(body.writer)
    let readerIdentity = identity(response.reader)
    let started = DispatchSemaphore(value: 0)
    let proceed = DispatchSemaphore(value: 0)
    let finished = DispatchSemaphore(value: 0)
    DispatchQueue.global(qos: .userInitiated).async {
      descriptors.perform {
        started.signal()
        proceed.wait()
        var poller = pollfd(fd: descriptors.reader, events: Int16(POLLIN), revents: 0)
        let ready = poll(&poller, 1, 0)
        precondition(ready == 1, "response descriptor was not readable after cancel")
        precondition(poller.revents & Int16(POLLNVAL) == 0, "response descriptor was invalid")
        var seen: UInt8 = 0
        precondition(Darwin.read(descriptors.reader, &seen, 1) == 1)
        precondition(seen == 0x11, "worker read \(seen) from the response pipe")
        var byte: UInt8 = 0xA5
        precondition(Darwin.write(descriptors.writer, &byte, 1) == 1, "worker write missed the original body pipe")
      }
      finished.signal()
    }
    precondition(started.wait(timeout: .now() + 5) == .success, "worker did not claim the descriptors")
    precondition(!descriptors.retireIfIdle(), "cancel retired descriptors the worker had claimed")
    precondition(identity(body.writer) == writerIdentity, "body writer number was replaced during cancel")
    precondition(identity(response.reader) == readerIdentity, "response reader number was replaced during cancel")
    var other = [Int32](repeating: -1, count: 2)
    precondition(pipe(&other) == 0)
    defer { close(other[0]); close(other[1]) }
    precondition(![body.writer, response.reader].contains(other[0]))
    precondition(![body.writer, response.reader].contains(other[1]))
    var wake: UInt8 = 0x11
    precondition(Darwin.write(response.writer, &wake, 1) == 1)
    proceed.signal()
    precondition(finished.wait(timeout: .now() + 5) == .success, "worker did not finish after cancel")
    var echoed: UInt8 = 0
    precondition(Darwin.read(body.reader, &echoed, 1) == 1)
    precondition(echoed == 0xA5, "worker write did not arrive on the original body pipe")
    expectQuiet(other[0])
    precondition(closed(body.writer) && closed(response.reader))
    let reusedWriter = occupy(body.writer)
    reusedWriter.write(0x5A)
    reusedWriter.expect(0x5A)
    reusedWriter.closeEnds()
    let reusedReader = occupy(response.reader)
    reusedReader.write(0x5A)
    reusedReader.expect(0x5A)
    reusedReader.closeEnds()
  }

  private static func queuedStartCannotWriteTheReusedNumber() throws {
    let body = try OwnerPipe.make()
    let response = try OwnerPipe.make()
    defer { close(body.reader); close(response.writer) }
    let descriptors = OwnerStreamDescriptors(writer: body.writer, reader: response.reader)
    let started = Started()
    let queue = OperationQueue()
    queue.maxConcurrentOperationCount = 1
    queue.isSuspended = true
    queue.addOperation { [descriptors, started] in
      started.mark()
      descriptors.perform {
        var byte: UInt8 = 0xA5
        precondition(Darwin.write(descriptors.writer, &byte, 1) == 1)
      }
    }
    precondition(descriptors.retireIfIdle(), "queued work had already claimed the descriptors")
    precondition(closed(body.writer) && closed(response.reader))
    let reused = occupy(body.writer)
    reused.write(0x5A)
    queue.isSuspended = false
    queue.waitUntilAllOperationsAreFinished()
    precondition(started.read(), "queued operation never started")
    reused.expect(0x5A)
    expectQuiet(body.reader)
    reused.closeEnds()
  }

  private static func cancelledQueueDropsTheOperationWithoutALeak() throws {
    let body = try OwnerPipe.make()
    let response = try OwnerPipe.make()
    defer { close(body.reader); close(response.writer) }
    var held: OwnerStreamDescriptors? = OwnerStreamDescriptors(writer: body.writer, reader: response.reader)
    let started = Started()
    let queue = OperationQueue()
    queue.maxConcurrentOperationCount = 1
    queue.isSuspended = true
    queue.addOperation { [descriptors = held!, started] in
      started.mark()
      descriptors.perform {
        var byte: UInt8 = 0xA5
        precondition(Darwin.write(descriptors.writer, &byte, 1) == 1)
      }
    }
    precondition(held?.retireIfIdle() == true)
    let reused = occupy(body.writer)
    reused.write(0x5A)
    precondition(held?.retireIfIdle() == true, "second idle retire skipped a pair the worker never claimed")
    held = nil
    queue.cancelAllOperations()
    queue.isSuspended = false
    queue.waitUntilAllOperationsAreFinished()
    precondition(!started.read(), "cancelled operation still ran")
    reused.expect(0x5A)
    expectQuiet(body.reader)
    precondition(closed(response.reader), "response reader leaked after the queue dropped it")
    reused.closeEnds()
  }

  private static func live(_ fd: Int32) -> Bool {
    fcntl(fd, F_GETFD) >= 0
  }

  private static func closed(_ fd: Int32) -> Bool {
    errno = 0
    return fcntl(fd, F_GETFD) < 0 && errno == EBADF
  }

  private static func identity(_ fd: Int32) -> (dev: dev_t, ino: ino_t) {
    var info = stat()
    precondition(fstat(fd, &info) == 0, "descriptor \(fd) was not live")
    return (info.st_dev, info.st_ino)
  }

  private static func expectQuiet(_ fd: Int32) {
    let flags = fcntl(fd, F_GETFL)
    precondition(flags >= 0)
    precondition(fcntl(fd, F_SETFL, flags | O_NONBLOCK) == 0)
    var byte: UInt8 = 0
    errno = 0
    let count = Darwin.read(fd, &byte, 1)
    precondition(count < 0 && errno == EAGAIN, "unexpected byte \(byte) on \(fd)")
  }

  /// Puts a new pipe writer on `retired`, which must already be closed.
  private static func occupy(_ retired: Int32) -> Reused {
    errno = 0
    precondition(fcntl(retired, F_GETFD) < 0 && errno == EBADF, "number \(retired) was still open")
    var pair = [Int32](repeating: -1, count: 2)
    precondition(pipe(&pair) == 0)
    if pair[1] == retired { return Reused(reader: pair[0], writer: pair[1]) }
    if pair[0] == retired {
      let reader = dup(pair[0])
      precondition(reader >= 0)
      close(pair[0])
      precondition(dup2(pair[1], retired) == retired)
      close(pair[1])
      return Reused(reader: reader, writer: retired)
    }
    precondition(dup2(pair[1], retired) == retired)
    close(pair[1])
    return Reused(reader: pair[0], writer: retired)
  }

  private struct Reused {
    let reader: Int32
    let writer: Int32

    func write(_ byte: UInt8) {
      var value = byte
      precondition(Darwin.write(writer, &value, 1) == 1, "write to reused descriptor \(writer) failed")
    }

    func expect(_ byte: UInt8) {
      var got: UInt8 = 0
      precondition(Darwin.read(reader, &got, 1) == 1, "reused descriptor produced no byte")
      precondition(got == byte, "reused descriptor returned \(got) instead of \(byte)")
      let flags = fcntl(reader, F_GETFL)
      precondition(flags >= 0)
      precondition(fcntl(reader, F_SETFL, flags | O_NONBLOCK) == 0)
      var extra: UInt8 = 0
      errno = 0
      let more = Darwin.read(reader, &extra, 1)
      precondition(more < 0 && errno == EAGAIN, "retired worker wrote \(extra) on the reused descriptor")
    }

    func closeEnds() {
      close(reader)
      close(writer)
    }
  }

  private final class Started: @unchecked Sendable {
    private let lock = NSLock()
    private var value = false
    func mark() { lock.lock(); value = true; lock.unlock() }
    func read() -> Bool { lock.lock(); defer { lock.unlock() }; return value }
  }
}
#endif
