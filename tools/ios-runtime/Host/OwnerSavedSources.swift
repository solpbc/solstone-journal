// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
#if JOURNAL_OWNER_APP
import Foundation

/// Static provider code. Journal state and bookmark bytes remain Rust-owned.
enum OwnerSavedSources {
  @MainActor static func install() throws {
    let cache = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0]
    let identity = cache.path.withCString { path in
      journal_ios_owner_sources_install(path, { material, destination in
        guard let material, let destination,
              let text = String(validatingCString: material),
              let path = String(validatingCString: destination) else { return -1 }
        return copy(text, to: URL(fileURLWithPath: path, isDirectory: true))
      })
    }
    guard identity != 0 else { throw NSError(domain: "saved-source-provider-unavailable", code: 1) }
  }

  private static func copy(_ material: String, to destination: URL) -> Int32 {
    guard journal_ios_owner_folder_copy_continues() else { return -1 }
    guard let data = Data(base64Encoded: material), data.count <= 65_536 else { return 1 }
    let url: URL
    do {
      var stale = false
      url = try URL(resolvingBookmarkData: data, options: [.withoutUI], relativeTo: nil,
        bookmarkDataIsStale: &stale)
      guard !stale else { return 1 }
    } catch { return 1 }
    guard journal_ios_owner_folder_copy_continues() else { return -1 }
    guard url.startAccessingSecurityScopedResource() else { return 1 }
    defer { url.stopAccessingSecurityScopedResource() }
    let requested = OwnerFolderCopy.Authority(token: UUID().uuidString, generation: 0)
    let live = OwnerFolderCopy.LiveAuthority(requested: requested, live: requested,
      continues: { journal_ios_owner_folder_copy_continues() })
    let cancelled = AtomicFlag(false)
    var error: NSError?
    var ready = false
    NSFileCoordinator().coordinate(readingItemAt: url, options: [], error: &error) { coordinated in
      guard live.matches, let source = try? OwnerFolderSource(coordinated) else { return }
      let result = OwnerFolderCopy.copy(requested: requested, live: live,
        source: source, destination: destination, cancel: cancelled)
      ready = result.authorityMatches && result.ownerMessage == OwnerFolderCopy.ready
    }
    guard journal_ios_owner_folder_copy_continues() else { return -1 }
    return error == nil && ready ? 0 : -1
  }
}

/// One picker worker owns this context through synchronous native publication.
final class OwnerSourceBookmark {
  let url: URL
  let continues: () -> Bool
  init(_ url: URL, continues: @escaping () -> Bool) {
    self.url = url
    self.continues = continues
  }
  func write(to output: UnsafeMutablePointer<UInt8>, capacity: Int) -> Int {
    guard continues(), journal_ios_owner_folder_copy_continues(),
          url.startAccessingSecurityScopedResource() else { return -1 }
    defer { url.stopAccessingSecurityScopedResource() }
    var error: NSError?
    var bookmark: Data?
    NSFileCoordinator().coordinate(readingItemAt: url, options: [], error: &error) { coordinated in
      guard continues(), journal_ios_owner_folder_copy_continues(),
            let source = try? OwnerFolderSource(coordinated) else { return }
      // Retain the actual readable directory descriptor during bookmark creation.
      withExtendedLifetime(source) {
        bookmark = try? url.bookmarkData(options: .minimalBookmark,
          includingResourceValuesForKeys: nil, relativeTo: nil)
      }
    }
    guard error == nil, continues(), journal_ios_owner_folder_copy_continues(),
          let bookmark else { return -1 }
    let encoded = Data(bookmark.base64EncodedString().utf8)
    guard !encoded.isEmpty, encoded.count <= capacity else { return -1 }
    encoded.copyBytes(to: output, count: encoded.count)
    return continues() && journal_ios_owner_folder_copy_continues() ? encoded.count : -1
  }
}
#endif
