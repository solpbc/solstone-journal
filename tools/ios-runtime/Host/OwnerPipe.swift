// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
#if JOURNAL_OWNER_APP
import Darwin
import Foundation

enum OwnerPipe {
  /// Transfers both descriptors to the caller. A closed reader must produce
  /// EPIPE, not terminate a Swift host that has the default SIGPIPE handler.
  static func make() throws -> (reader: Int32, writer: Int32) {
    var descriptors = [Int32](repeating: -1, count: 2)
    guard pipe(&descriptors) == 0 else { throw URLError(.cannotOpenFile) }
    let reader = descriptors[0], writer = descriptors[1]
    guard fcntl(writer, F_SETFL, fcntl(writer, F_GETFL) | O_NONBLOCK) == 0,
          fcntl(writer, F_SETNOSIGPIPE, 1) == 0,
          fcntl(writer, F_SETFD, FD_CLOEXEC) == 0,
          fcntl(reader, F_SETFD, FD_CLOEXEC) == 0 else {
      close(reader); close(writer); throw URLError(.cannotOpenFile)
    }
    return (reader, writer)
  }
}
#endif
