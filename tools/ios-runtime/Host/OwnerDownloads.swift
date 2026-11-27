// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
#if JOURNAL_OWNER_APP
import UIKit
@preconcurrency import WebKit

/// Temporary export copies live until the system destination choice finishes.
/// Prepared journal exports are handed straight to Files without staging a second copy.
@MainActor final class OwnerDownloads: NSObject, WKDownloadDelegate, UIDocumentPickerDelegate {
  private struct Transfer { let download: WKDownload; let directory: URL; var file: URL? }
  private var transfers: [ObjectIdentifier: Transfer] = [:]
  private var shares: [UUID: (UIActivityViewController, URL)] = [:]
  private var activeExportPicker: (UIDocumentPickerViewController, String, URL?)?
  private var nativeRead: OwnerRead?
  private var progress: UIAlertController?
  private let presenter: () -> UIViewController?
  private let failed: () -> Void
  private let handedOff: ((String) -> Void)?
  private var closed = false

  init(
    presenter: @escaping () -> UIViewController?,
    failed: @escaping () -> Void,
    handedOff: ((String) -> Void)? = nil
  ) {
    self.presenter = presenter
    self.failed = failed
    self.handedOff = handedOff
    super.init()
    Self.sweepStaleTemporaryDirectories()
  }

  /// Removes orphaned `journal-export-*` directories (including hard-links to
  /// prepared exports) left behind if the host process exited while a document
  /// picker or share sheet was open.
  nonisolated static func sweepStaleTemporaryDirectories(
    in tempRoot: URL = FileManager.default.temporaryDirectory
  ) {
    guard let entries = try? FileManager.default.contentsOfDirectory(
      at: tempRoot,
      includingPropertiesForKeys: nil,
      options: []
    ) else { return }
    for entry in entries where entry.lastPathComponent.hasPrefix("journal-export-") {
      try? FileManager.default.removeItem(at: entry)
    }
  }

  private var available: Bool {
    !closed && nativeRead == nil && transfers.isEmpty && shares.isEmpty && activeExportPicker == nil
  }

  private static func preparedExportID(from url: URL) -> String? {
    let parts = url.pathComponents
    // Matches /app/export/api/ready/<export_id>/download
    guard parts.count == 7,
          parts[0] == "/", parts[1] == "app", parts[2] == "export",
          parts[3] == "api", parts[4] == "ready", parts[6] == "download",
          !parts[5].isEmpty else { return nil }
    return parts[5]
  }

  private static func resolvePreparedExport(_ exportID: String) -> (URL, String)? {
    let pointer = exportID.withCString { journal_ios_owner_prepared_export_path($0) }
    guard let pointer else { return nil }
    defer { journal_runtime_free(pointer) }
    let data = Data(String(cString: pointer).utf8)
    guard let reply = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
          reply["ok"] as? Bool == true,
          let value = reply["value"] as? [String: Any],
          let path = value["path"] as? String,
          FileManager.default.fileExists(atPath: path) else { return nil }
    let name = Self.filename((value["filename"] as? String) ?? "solstone-journal.zip")
    return (URL(fileURLWithPath: path), name)
  }

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
  /// Prepared journal exports bypass temporary re-copying and hand directly to Files.
  func receive(_ request: URLRequest) {
    guard available, (request.httpMethod ?? "GET") == "GET", let url = request.url,
          (try? OwnerOrigin.admit(url)) != nil, let top = presenter() else {
      if !closed { failed() }
      return
    }
    if let exportID = Self.preparedExportID(from: url),
       let (preparedURL, suggestedName) = Self.resolvePreparedExport(exportID) {
      // Hard-link the immutable prepared ZIP under its owner-facing filename in a
      // private temporary directory so Files exports zero extra staging bytes.
      var exportURL = preparedURL
      var linkDir: URL?
      if let dir = try? self.directory() {
        let linked = dir.appendingPathComponent(suggestedName)
        if (try? FileManager.default.linkItem(at: preparedURL, to: linked)) != nil {
          exportURL = linked
          linkDir = dir
        } else {
          try? FileManager.default.removeItem(at: dir)
        }
      }
      let picker = UIDocumentPickerViewController(forExporting: [exportURL], asCopy: true)
      picker.delegate = self
      activeExportPicker = (picker, exportID, linkDir)
      top.present(picker, animated: true)
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

  func documentPicker(_ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]) {
    guard let (picker, exportID, linkDir) = activeExportPicker, picker === controller else { return }
    activeExportPicker = nil
    if let linkDir { try? FileManager.default.removeItem(at: linkDir) }
    if !closed { handedOff?(exportID) }
  }

  func documentPickerWasCancelled(_ controller: UIDocumentPickerViewController) {
    guard let (picker, _, linkDir) = activeExportPicker, picker === controller else { return }
    activeExportPicker = nil
    if let linkDir { try? FileManager.default.removeItem(at: linkDir) }
  }

  func close() {
    closed = true
    nativeRead?.cancel()
    progress?.dismiss(animated: false)
    progress = nil
    if let (picker, _, linkDir) = activeExportPicker {
      picker.dismiss(animated: false)
      if let linkDir { try? FileManager.default.removeItem(at: linkDir) }
      activeExportPicker = nil
    }
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
