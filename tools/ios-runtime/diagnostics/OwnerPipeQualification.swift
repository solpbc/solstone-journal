// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
import Darwin
import Foundation

@main enum OwnerPipeQualification {
  static func main() throws {
    signal(SIGPIPE, SIG_DFL)
    let (reader, writer) = try OwnerPipe.make()
    defer { close(writer) }
    precondition(fcntl(writer, F_GETFL) & O_NONBLOCK != 0)
    precondition(fcntl(writer, F_GETFD) & FD_CLOEXEC != 0)
    precondition(fcntl(reader, F_GETFD) & FD_CLOEXEC != 0)
    close(reader)
    var byte: UInt8 = 42
    let written = withUnsafePointer(to: &byte) { Darwin.write(writer, $0, 1) }
    precondition(written == -1 && errno == EPIPE)
    print("closed response reader returns EPIPE; Swift host survives with SIG_DFL")
  }
}
