// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
#if JOURNAL_OWNER_APP
import UIKit
@preconcurrency import WebKit

/// Temporary export copies live until the system destination choice finishes.
@MainActor final class OwnerDownloads: NSObject, WKDownloadDelegate {
  private struct Transfer { let download: WKDownload; let directory: URL; var file: URL? }
  private var transfers: [ObjectIdentifier: Transfer] = [:]
  private var shares: [UUID: (UIActivityViewController, URL)] = [:]
  private var nativeRead: OwnerRead?
  private var progress: UIAlertController?
  private let presenter: () -> UIViewController?
  private let failed: () -> Void
  private var closed = false

  init(presenter: @escaping () -> UIViewController?, failed: @escaping () -> Void) {
    self.presenter = presenter
    self.failed = failed
  }

  private var available: Bool { !closed && nativeRead == nil && transfers.isEmpty && shares.isEmpty }

  private func directory() throws -> URL {
    let directory = FileManager.default.temporaryDirectory
      .appendingPathComponent("journal-export-" + UUID().uuidString, isDirectory: true)
    try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: false,
      attributes: [.posixPermissions: 0o700, .protectionKey: FileProtectionType.complete])
    return directory
  }

  nonisolated private static func filename(_ suggested: String) -> String {
    let leaf = (suggested as NSString).lastPathComponent
    let safe = !leaf.isEmpty && !leaf.hasPrefix(".") && leaf.utf8.count <= 240
      && !leaf.unicodeScalars.contains(where: { CharacterSet.controlCharacters.contains($0) })
    return safe ? leaf : "journal-export"
  }

  func receive(_ download: WKDownload) {
    guard available, download.originalRequest?.url?.scheme == "blob" else {
      download.cancel { _ in }
      if !closed { failed() }
      return
    }
    do {
      transfers[ObjectIdentifier(download)] = Transfer(download: download, directory: try directory())
      download.delegate = self
    } catch {
      download.cancel { _ in }
      failed()
    }
  }

  /// Private scheme resources never enter WebKit's network process. Read them
  /// through the admitted owner boundary, writing bounded chunks to disk.
  func receive(_ request: URLRequest) {
    guard available, (request.httpMethod ?? "GET") == "GET", let url = request.url,
          (try? OwnerOrigin.admit(url)) != nil, let top = presenter() else {
      if !closed { failed() }
      return
    }
    let directory: URL
    do { directory = try self.directory() } catch { failed(); return }
    let read = OwnerRead()
    nativeRead = read
    let alert = UIAlertController(title: "preparing file", message: nil, preferredStyle: .alert)
    alert.addAction(UIAlertAction(title: "cancel", style: .cancel) { _ in read.cancel() })
    progress = alert
    top.present(alert, animated: true)
    DispatchQueue.global(qos: .userInitiated).async { [weak self] in
      var file: URL?
      var handle: FileHandle?
      var failure: Error?
      do {
        try read.run(request, followRedirects: true, head: { head in
          guard (200...299).contains(head.status) else { throw URLError(.badServerResponse) }
          let response = HTTPURLResponse(url: head.url, statusCode: head.status,
            httpVersion: "HTTP/1.1", headerFields: head.headers)
          let target = directory.appendingPathComponent(Self.filename(response?.suggestedFilename ?? "journal-export"))
          guard FileManager.default.createFile(atPath: target.path, contents: nil,
            attributes: [.posixPermissions: 0o600, .protectionKey: FileProtectionType.complete]) else {
            throw CocoaError(.fileWriteUnknown)
          }
          file = target
          handle = try FileHandle(forWritingTo: target)
        }, bytes: { data in
          guard let handle else { throw URLError(.cannotParseResponse) }
          try handle.write(contentsOf: data)
        })
        try handle?.close()
      } catch {
        failure = error
        try? handle?.close()
      }
      let resultFile = file
      let resultError = failure
      DispatchQueue.main.async {
        guard let self, self.nativeRead === read else {
          try? FileManager.default.removeItem(at: directory)
          return
        }
        self.nativeRead = nil
        let progress = self.progress
        self.progress = nil
        let finish = {
          guard !self.closed, !read.cancelled, resultError == nil, let file = resultFile else {
            try? FileManager.default.removeItem(at: directory)
            if !self.closed && !read.cancelled { self.failed() }
            return
          }
          self.share(file, directory: directory)
        }
        if let progress, progress.presentingViewController != nil {
          progress.dismiss(animated: true, completion: finish)
        } else { finish() }
      }
    }
  }

  func download(_ download: WKDownload, decideDestinationUsing response: URLResponse,
                suggestedFilename: String, completionHandler: @escaping (URL?) -> Void) {
    let key = ObjectIdentifier(download)
    guard !closed, var transfer = transfers[key] else { completionHandler(nil); return }
    let file = transfer.directory.appendingPathComponent(Self.filename(suggestedFilename))
    transfer.file = file
    transfers[key] = transfer
    completionHandler(file)
  }

  func downloadDidFinish(_ download: WKDownload) {
    guard let transfer = transfers.removeValue(forKey: ObjectIdentifier(download)) else { return }
    guard let file = transfer.file else {
      try? FileManager.default.removeItem(at: transfer.directory)
      if !closed { failed() }
      return
    }
    share(file, directory: transfer.directory)
  }

  private func share(_ file: URL, directory: URL) {
    guard !closed, FileManager.default.fileExists(atPath: file.path), let top = presenter() else {
      try? FileManager.default.removeItem(at: directory)
      if !closed { failed() }
      return
    }
    let id = UUID()
    let sheet = UIActivityViewController(activityItems: [file], applicationActivities: nil)
    shares[id] = (sheet, directory)
    sheet.completionWithItemsHandler = { [weak self] _, _, _, error in
      Task { @MainActor in
        guard let self, let (_, directory) = self.shares.removeValue(forKey: id) else { return }
        try? FileManager.default.removeItem(at: directory)
        if error != nil && !self.closed { self.failed() }
      }
    }
    if let popover = sheet.popoverPresentationController {
      popover.sourceView = top.view
      popover.sourceRect = CGRect(x: top.view.bounds.midX, y: top.view.bounds.maxY, width: 1, height: 1)
    }
    top.present(sheet, animated: true)
  }

  func download(_ download: WKDownload, didFailWithError error: Error, resumeData: Data?) {
    guard let transfer = transfers.removeValue(forKey: ObjectIdentifier(download)) else { return }
    try? FileManager.default.removeItem(at: transfer.directory)
    if !closed && (error as NSError).code != NSURLErrorCancelled { failed() }
  }

  func close() {
    closed = true
    nativeRead?.cancel()
    progress?.dismiss(animated: false)
    progress = nil
    let current = transfers.values
    transfers.removeAll()
    for transfer in current {
      transfer.download.cancel { _ in try? FileManager.default.removeItem(at: transfer.directory) }
    }
    for (sheet, directory) in shares.values {
      sheet.dismiss(animated: false)
      try? FileManager.default.removeItem(at: directory)
    }
    shares.removeAll()
  }
}
#endif
