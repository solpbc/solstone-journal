// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
#if JOURNAL_OWNER_APP
import Darwin
import Foundation
import UIKit
import UniformTypeIdentifiers
import WebKit

/// Page-bound chunk bridge for a file the scheme handler cannot see.
/// Ordinary fetches stay on the scheme handler. File bytes stay out of the control JSON.
final class OwnerUploadBridge: NSObject, WKScriptMessageHandlerWithReply, UIDocumentPickerDelegate {
  private let owner: OwnerJournal
  private let lock = NSLock()
  private weak var web: WKWebView?
  /// Authority the visible document bound. Zero means this bridge has no live document.
  private var authority: UInt64 = 0
  /// Authority of the last committed main document. A failed provisional load restores it.
  private var committedAuthority: UInt64 = 0
  /// Authority issued for a main-frame load that has not committed yet.
  private var provisionalAuthority: UInt64 = 0
  private var installed = false
  private var presenting = false
  private var folderAuthority: UInt64 = 0
  private var pumps: [PumpKey: OwnerStreamPump] = [:]
  private let streams: OperationQueue = {
    let queue = OperationQueue()
    queue.maxConcurrentOperationCount = 32
    queue.qualityOfService = .userInitiated
    return queue
  }()
  private let copies: OperationQueue = {
    let queue = OperationQueue()
    queue.maxConcurrentOperationCount = 1
    queue.qualityOfService = .userInitiated
    return queue
  }()

  init(_ owner: OwnerJournal) { self.owner = owner }

  func install(on configuration: WKWebViewConfiguration) {
    configuration.userContentController.addScriptMessageHandler(self, contentWorld: .page, name: "journalOwner")
    installed = true
    if let url = Bundle.main.url(forResource: "owner-stream", withExtension: "js"),
       let source = try? String(contentsOf: url, encoding: .utf8) {
      configuration.userContentController.addUserScript(WKUserScript(
        source: source, injectionTime: .atDocumentStart, forMainFrameOnly: true))
    }
  }

  func attach(_ web: WKWebView) { self.web = web }

  /// A main-frame load gets a new native authority before its document script runs.
  /// The previous document stays live until that load commits.
  func rotateDocument() {
    lock.lock()
    let issued = journal_ios_owner_authority_issue()
    let abandoned = provisionalAuthority
    provisionalAuthority = issued
    let drop = abandoned != 0 && abandoned != committedAuthority && abandoned != authority
    lock.unlock()
    if drop { OwnerOutcome.retire(abandoned) }
  }

  /// The main document changed. Retire the document that was showing and stop its pumps.
  func commitDocument() {
    lock.lock()
    let retiring = committedAuthority
    let next = provisionalAuthority != 0 ? provisionalAuthority : authority
    guard next != 0 else { lock.unlock(); return }
    committedAuthority = next
    authority = next
    provisionalAuthority = 0
    var stale: [OwnerStreamPump] = []
    var clearPicker = false
    if retiring != 0 && retiring != next {
      let keys = pumps.keys.filter { $0.authority == retiring }
      for key in keys {
        if let pump = pumps.removeValue(forKey: key) { stale.append(pump) }
      }
      if folderAuthority == retiring {
        folderAuthority = 0
        clearPicker = true
      }
    }
    lock.unlock()
    if clearPicker { presenting = false }
    if retiring != 0 && retiring != next { OwnerOutcome.retire(retiring) }
    for pump in stale { pump.cancel() }
  }

  /// A provisional load failed and the committed document is still showing.
  func abandonProvisionalDocument() {
    lock.lock()
    let abandoned = provisionalAuthority
    provisionalAuthority = 0
    let drop = abandoned != 0 && abandoned != committedAuthority && abandoned != authority
    lock.unlock()
    if drop { OwnerOutcome.retire(abandoned) }
  }

  func close() {
    lock.lock()
    var retired: [UInt64] = []
    for id in [authority, committedAuthority, provisionalAuthority] where id != 0 && !retired.contains(id) {
      retired.append(id)
    }
    authority = 0
    committedAuthority = 0
    provisionalAuthority = 0
    let current = pumps
    pumps.removeAll()
    folderAuthority = 0
    lock.unlock()
    for id in retired { OwnerOutcome.retire(id) }
    for pump in current.values { pump.cancel() }
    streams.cancelAllOperations()
    presenting = false
    if installed {
      web?.configuration.userContentController.removeScriptMessageHandler(forName: "journalOwner", contentWorld: .page)
      installed = false
    }
  }

  /// The attached owner page's main frame is the only sender. The URL inside
  /// the message is a request target, not this boundary.
  private func senderAllowed(_ message: WKScriptMessage) -> Bool {
    guard let attached = web, message.webView === attached else { return false }
    let origin = message.frameInfo.securityOrigin
    let port = origin.port
    guard (0...Int(UInt16.max)).contains(port) else { return false }
    let scheme = origin.`protocol`
    let host = origin.host
    return scheme.withCString { scheme in
      host.withCString { host in
        journal_ios_owner_sender_admit(message.frameInfo.isMainFrame, scheme, host, UInt16(port))
      }
    }
  }

  func userContentController(
    _ controller: WKUserContentController,
    didReceive message: WKScriptMessage,
    replyHandler: @escaping (Any?, String?) -> Void
  ) {
    guard senderAllowed(message) else {
      replyHandler(["ok": false, "error": OwnerUploadCopy.lost], nil)
      return
    }
    guard let body = message.body as? [String: Any], let op = body["op"] as? String else {
      replyHandler(["ok": false, "error": OwnerUploadCopy.lost], nil)
      return
    }
    switch op {
    case "bind": replyHandler(bind(), nil)
    case "open": replyHandler(open(body), nil)
    case "chunk": replyHandler(chunk(body), nil)
    case "finish": replyHandler(finish(body), nil)
    case "cancel":
      cancel(body)
      replyHandler(["ok": true], nil)
    case "chooseFolder":
      chooseFolder(requestedAuthority(body))
      replyHandler(["ok": true], nil)
    default:
      replyHandler(["ok": false, "error": OwnerUploadCopy.lost], nil)
    }
  }

  private struct PumpKey: Hashable {
    let authority: UInt64
    let pageID: String
  }

  /// The id the document script should bind. A provisional load is visible to the
  /// new document before it commits; the committed document keeps its own id until then.
  private func bindingAuthorityLocked() -> UInt64 {
    if provisionalAuthority != 0 { return provisionalAuthority }
    return authority
  }

  private func authorityIsLiveLocked(_ id: UInt64) -> Bool {
    id != 0 && (id == authority || id == committedAuthority || id == provisionalAuthority)
  }

  private func currentAuthority() -> UInt64 {
    lock.lock(); defer { lock.unlock() }
    return bindingAuthorityLocked()
  }

  /// The document script asks once, at start. A reload asks again and receives
  /// the authority `rotateDocument` issued for that load.
  private func bind() -> [String: Any] {
    lock.lock()
    var current = bindingAuthorityLocked()
    if current == 0 {
      current = journal_ios_owner_authority_issue()
      authority = current
      committedAuthority = current
    }
    lock.unlock()
    guard current != 0 else { return ["ok": false, "error": OwnerUploadCopy.lost] }
    return ["ok": true, "authority": NSNumber(value: current)]
  }

  private func requestedAuthority(_ body: [String: Any]) -> UInt64? {
    let number: NSNumber
    if let value = body["authority"] as? NSNumber {
      number = value
    } else if let value = body["authority"] as? UInt64 {
      number = NSNumber(value: value)
    } else if let value = body["authority"] as? Int {
      number = NSNumber(value: value)
    } else {
      return nil
    }
    let value = number.uint64Value
    if value == 0 || number.doubleValue != Double(value) { return nil }
    return value
  }

  private func open(_ body: [String: Any]) -> [String: Any] {
    guard let id = body["id"] as? String, id.count < 64,
          let rawURL = body["url"] as? String, let url = URL(string: rawURL),
          let method = body["method"] as? String,
          let requested = requestedAuthority(body) else {
      return ["ok": false, "error": OwnerUploadCopy.lost]
    }
    let path: String
    // The sender frame already admitted this call. The body URL is only the target.
    do { path = try OwnerOrigin.admit(url) } catch {
      return ["ok": false, "error": "private journal address required"]
    }
    lock.lock()
    let authority = bindingAuthorityLocked()
    let key = PumpKey(authority: authority, pageID: id)
    if requested != authority || authority == 0 {
      lock.unlock()
      return ["ok": false, "error": OwnerUploadCopy.aborted]
    }
    if pumps.count >= 32 || pumps[key] != nil {
      lock.unlock()
      return ["ok": false, "error": OwnerUploadCopy.busy]
    }
    lock.unlock()
    guard OwnerOutcome.admit(id, authority: authority) else {
      if authority != currentAuthority() { return ["ok": false, "error": OwnerUploadCopy.aborted] }
      return ["ok": false, "error": OwnerUploadCopy.busy]
    }
    var published = false
    defer { if !published { OwnerOutcome.cancel(id, authority: authority) } }
    let nativeID = OwnerRequestIdentity.next()
    let headers = OwnerUploadCopy.headerPairs(body["headers"])
    let bodyPipe: (reader: Int32, writer: Int32)
    do { bodyPipe = try OwnerPipe.make() } catch {
      return ["ok": false, "error": OwnerUploadCopy.lost]
    }
    let responsePipe: (reader: Int32, writer: Int32)
    do { responsePipe = try OwnerPipe.make() } catch {
      Darwin.close(bodyPipe.reader)
      Darwin.close(bodyPipe.writer)
      return ["ok": false, "error": OwnerUploadCopy.lost]
    }
    let request: Data
    do {
      request = try JSONSerialization.data(withJSONObject: [
        "id": NSNumber(value: nativeID),
        "admissionGeneration": NSNumber(value: journal_ios_processing_generation()),
        "method": method,
        "uri": path,
        "headers": headers,
        "body": "",
        "stream": true,
      ])
    } catch {
      Darwin.close(bodyPipe.reader); Darwin.close(bodyPipe.writer)
      Darwin.close(responsePipe.reader); Darwin.close(responsePipe.writer)
      return ["ok": false, "error": OwnerUploadCopy.lost]
    }
    let pointer = request.withUnsafeBytes {
      journal_ios_owner_open_stream($0.bindMemory(to: UInt8.self).baseAddress, UInt($0.count), responsePipe.writer, bodyPipe.reader)
    }
    guard let pointer else {
      Darwin.close(bodyPipe.writer); Darwin.close(responsePipe.reader)
      return ["ok": false, "error": OwnerUploadCopy.lost]
    }
    defer { journal_runtime_free(pointer) }
    let parsed = (try? JSONSerialization.jsonObject(with: Data(String(cString: pointer).utf8))) as? [String: Any]
    guard parsed?["ok"] as? Bool == true,
          let value = parsed?["value"] as? [String: Any], value["streaming"] as? Bool == true else {
      Darwin.close(bodyPipe.writer); Darwin.close(responsePipe.reader)
      let error = parsed?["error"] as? String ?? OwnerUploadCopy.lost
      return ["ok": false, "error": OwnerUploadCopy.message(error)]
    }
    if authority != currentAuthority() {
      journal_ios_owner_cancel(nativeID)
      Darwin.close(bodyPipe.writer); Darwin.close(responsePipe.reader)
      return ["ok": false, "error": OwnerUploadCopy.aborted]
    }
    let pump = OwnerStreamPump(
      pageID: id, nativeID: nativeID, authority: authority, url: rawURL,
      bodyWriter: bodyPipe.writer, responseReader: responsePipe.reader, bridge: self)
    lock.lock()
    if bindingAuthorityLocked() != authority {
      lock.unlock()
      pump.cancel()
      return ["ok": false, "error": OwnerUploadCopy.aborted]
    }
    pumps[key] = pump
    published = true
    lock.unlock()
    streams.addOperation { [weak pump] in pump?.run() }
    return ["ok": true]
  }

  private func chunk(_ body: [String: Any]) -> [String: Any] {
    guard let id = body["id"] as? String, let text = body["chunk"] as? String,
          let authority = requestedAuthority(body) else {
      return ["error": OwnerUploadCopy.lost]
    }
    lock.lock()
    let pump = pumps[PumpKey(authority: authority, pageID: id)]
    lock.unlock()
    guard let pump, pump.authority == authority else { return ["error": OwnerUploadCopy.aborted] }
    do {
      let bytes = try OwnerUploadCopy.latin1(text)
      switch pump.offer(bytes) {
      case .accepted: return ["accepted": true]
      case .busy: return ["busy": true]
      case .closed: return ["error": OwnerUploadCopy.aborted]
      }
    } catch {
      return ["error": OwnerUploadCopy.unsent]
    }
  }

  private func finish(_ body: [String: Any]) -> [String: Any] {
    guard let id = body["id"] as? String, let authority = requestedAuthority(body) else {
      return ["error": OwnerUploadCopy.lost]
    }
    let key = PumpKey(authority: authority, pageID: id)
    switch OwnerOutcome.take(id, authority: authority) {
    case .ready(let reply):
      lock.lock()
      let pump = pumps.removeValue(forKey: key)
      lock.unlock()
      pump?.markFinished()
      return reply
    case .pending:
      lock.lock()
      let pump = pumps[key]
      lock.unlock()
      guard let pump else { return ["error": OwnerUploadCopy.lost] }
      pump.markFinished()
      return ["pending": true]
    case .absent:
      return ["error": OwnerUploadCopy.lost]
    }
  }

  private func cancel(_ body: [String: Any]) {
    guard let id = body["id"] as? String, let authority = requestedAuthority(body) else { return }
    lock.lock()
    let pump = pumps.removeValue(forKey: PumpKey(authority: authority, pageID: id))
    lock.unlock()
    OwnerOutcome.cancel(id, authority: authority)
    pump?.cancel()
  }

  fileprivate func authorityMatches(_ authority: UInt64) -> Bool {
    lock.lock(); defer { lock.unlock() }
    return authorityIsLiveLocked(authority)
  }

  private func chooseFolder(_ requested: UInt64?) {
    guard let requested else { return }
    lock.lock()
    let current = bindingAuthorityLocked()
    if presenting || requested != current || current == 0 {
      lock.unlock()
      return
    }
    presenting = true
    folderAuthority = requested
    lock.unlock()
    guard let top = topController() else {
      presenting = false
      folderAuthority = 0
      return
    }
    let picker = UIDocumentPickerViewController(forOpeningContentTypes: [.folder])
    picker.delegate = self
    picker.allowsMultipleSelection = false
    top.present(picker, animated: true)
  }

  func documentPickerWasCancelled(_ controller: UIDocumentPickerViewController) {
    presenting = false
    folderAuthority = 0
  }

  func documentPicker(_ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]) {
    presenting = false
    let authority = folderAuthority
    folderAuthority = 0
    guard authority != 0, let url = urls.first else { return }
    guard url.startAccessingSecurityScopedResource() else {
      notice(OwnerUploadCopy.unreadable)
      return
    }
    copies.addOperation { [weak self] in
      defer { url.stopAccessingSecurityScopedResource() }
      self?.copyFolder(url, authority: authority)
    }
  }

  private struct OwnedBridge {
    let id: UInt64
    let token: String
    var receiptCreated = false
    var directory: URL {
      FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0]
        .appendingPathComponent("owner-bridge", isDirectory: true)
        .appendingPathComponent(String(id), isDirectory: true)
    }
  }

  private static func bridgeValue(_ invoke: () -> UnsafeMutablePointer<CChar>?) throws -> [String: Any] {
    guard let pointer = invoke() else { throw CocoaError(.fileReadUnknown) }
    defer { journal_runtime_free(pointer) }
    let data = Data(String(cString: pointer).utf8)
    guard let object = try JSONSerialization.jsonObject(with: data) as? [String: Any] else {
      throw CocoaError(.fileReadCorruptFile)
    }
    if object["ok"] as? Bool == true, let value = object["value"] as? [String: Any] { return value }
    let message = object["error"] as? String ?? ""
    if message.contains("No space") || message.contains("ENOSPC") {
      throw NSError(domain: NSPOSIXErrorDomain, code: Int(ENOSPC), userInfo: nil)
    }
    throw CocoaError(.fileReadUnknown)
  }

  private static func allocateBridge(_ authority: UInt64) throws -> OwnedBridge {
    let cache = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0]
    let value = try cache.path.withCString { cachePath in
      try bridgeValue { journal_ios_owner_bridge_allocate(cachePath, authority) }
    }
    guard let number = value["id"] as? NSNumber, let token = value["token"] as? String, !token.isEmpty else {
      throw CocoaError(.fileReadUnknown)
    }
    return OwnedBridge(id: number.uint64Value, token: token)
  }

  private static func commitBridge(_ owned: inout OwnedBridge, authority: UInt64) throws {
    let cache = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0]
    let id = owned.id
    let token = owned.token
    try cache.path.withCString { cachePath in
      try token.withCString { tokenPath in
        _ = try bridgeValue { journal_ios_owner_bridge_commit(cachePath, authority, id, tokenPath) }
      }
    }
    owned.receiptCreated = true
  }

  private static func releaseBridge(_ owned: OwnedBridge, authority: UInt64) {
    let cache = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0]
    let created: Int32 = owned.receiptCreated ? 1 : 0
    try? cache.path.withCString { cachePath in
      try owned.token.withCString { tokenPath in
        _ = try bridgeValue { journal_ios_owner_bridge_release(cachePath, authority, owned.id, tokenPath, created) }
      }
    }
  }

  private static func disarmBridge(_ owned: OwnedBridge, authority: UInt64) {
    let cache = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0]
    try? cache.path.withCString { cachePath in
      try owned.token.withCString { tokenPath in
        _ = try bridgeValue { journal_ios_owner_bridge_disarm(cachePath, authority, owned.id, tokenPath) }
      }
    }
  }

  private func copyFolder(_ source: URL, authority: UInt64) {
    var owned: OwnedBridge?
    do {
      guard authorityMatches(authority) else { return }
      // Folder identity is the exclusive claim, not a native request id.
      let copy = try Self.allocateBridge(authority)
      owned = copy
      if try copyChildren(source, to: copy.directory, authority: authority) || !authorityMatches(authority) {
        Self.releaseBridge(copy, authority: authority)
        owned = nil
        return
      }
      var published = copy
      try Self.commitBridge(&published, authority: authority)
      owned = nil
      let path = published.directory.path
      DispatchQueue.main.async { [weak self] in
        guard let self, self.authorityMatches(authority), let web = self.web else {
          Self.releaseBridge(published, authority: authority)
          return
        }
        guard let quoted = try? JSONSerialization.data(withJSONObject: path, options: [.fragmentsAllowed]),
              let literal = String(data: quoted, encoding: .utf8) else {
          Self.releaseBridge(published, authority: authority)
          return
        }
        web.evaluateJavaScript(
          "(function(){var input=document.getElementById('guidedPathInput');if(!input)return;input.value=\(literal);input.dispatchEvent(new Event('input',{bubbles:true}));})()",
          completionHandler: nil)
        Self.disarmBridge(published, authority: authority)
      }
    } catch {
      if let owned { Self.releaseBridge(owned, authority: authority) }
      if authorityMatches(authority) {
        notice(OwnerUploadCopy.disk(error) ? OwnerUploadCopy.diskMessage : OwnerUploadCopy.unreadable)
      }
    }
  }

  private func copyChildren(_ source: URL, to dest: URL, authority: UInt64) throws -> Bool {
    let children = try FileManager.default.contentsOfDirectory(
      at: source, includingPropertiesForKeys: [.isDirectoryKey, .isSymbolicLinkKey])
    for child in children {
      if !authorityMatches(authority) { return true }
      let name = child.lastPathComponent
      if name.isEmpty || name == "." || name == ".." || name.contains("/") { continue }
      let values = try child.resourceValues(forKeys: [.isDirectoryKey, .isSymbolicLinkKey])
      if values.isSymbolicLink == true { continue }
      let target = dest.appendingPathComponent(name, isDirectory: values.isDirectory == true)
      if values.isDirectory == true {
        try FileManager.default.createDirectory(at: target, withIntermediateDirectories: true)
        if try copyChildren(child, to: target, authority: authority) { return true }
      } else {
        try OwnerUploadCopy.copyFile(child, to: target)
      }
    }
    return false
  }

  private func notice(_ message: String) {
    DispatchQueue.main.async { [owner] in owner.detail = message }
  }

  private func topController() -> UIViewController? {
    guard let scene = UIApplication.shared.connectedScenes.first as? UIWindowScene,
          var top = scene.windows.first(where: { $0.isKeyWindow })?.rootViewController else { return nil }
    while let presented = top.presentedViewController { top = presented }
    return top
  }
}

private enum OwnerTake {
  case ready([String: Any])
  case pending
  case absent
}

/// Completed replies outlive the pump's read loop. Finish consumes one,
/// cancel drops one, and retiring that document's authority drops the rest.
private enum OwnerOutcome {
  static func admit(_ pageID: String, authority: UInt64) -> Bool {
    let pointer = pageID.withCString { journal_ios_owner_outcome_admit(authority, $0) }
    guard let pointer else { return false }
    defer { journal_runtime_free(pointer) }
    guard let object = try? JSONSerialization.jsonObject(with: Data(String(cString: pointer).utf8)) as? [String: Any] else {
      return false
    }
    return object["ok"] as? Bool == true
  }

  static func keep(_ pageID: String, authority: UInt64, reply: [String: Any]) -> Bool {
    guard JSONSerialization.isValidJSONObject(reply),
          let data = try? JSONSerialization.data(withJSONObject: reply),
          let text = String(data: data, encoding: .utf8) else { return false }
    let pointer = pageID.withCString { page in
      text.withCString { body in journal_ios_owner_outcome_keep(authority, page, body) }
    }
    guard let pointer else { return false }
    defer { journal_runtime_free(pointer) }
    guard let object = try? JSONSerialization.jsonObject(with: Data(String(cString: pointer).utf8)) as? [String: Any] else {
      return false
    }
    return object["ok"] as? Bool == true
  }

  static func take(_ pageID: String, authority: UInt64) -> OwnerTake {
    let pointer = pageID.withCString { journal_ios_owner_outcome_take(authority, $0) }
    guard let pointer else { return .absent }
    defer { journal_runtime_free(pointer) }
    guard let object = try? JSONSerialization.jsonObject(with: Data(String(cString: pointer).utf8)) as? [String: Any],
          object["ok"] as? Bool == true,
          let value = object["value"] as? [String: Any],
          let state = value["state"] as? String else { return .absent }
    switch state {
    case "ready":
      guard let reply = value["reply"] as? [String: Any] else { return .absent }
      return .ready(reply)
    case "pending":
      return .pending
    default:
      return .absent
    }
  }

  static func cancel(_ pageID: String, authority: UInt64) {
    pageID.withCString { journal_ios_owner_outcome_cancel(authority, $0) }
  }

  static func retire(_ authority: UInt64) {
    journal_ios_owner_authority_retire(authority)
  }
}

private enum OwnerUploadCopy {
  static let stall = "the transfer stopped before it finished. check the result before trying again."
  static let lost = "journal response unavailable; check the result before trying again"
  static let busy = "journal controls busy; check health before retrying"
  static let aborted = "aborted; check the result before retrying"
  static let unreadable = "this file couldn't be read. choose it again."
  static let unsent = "this file couldn't be sent. choose it again, and check the result before retrying."
  static let diskMessage = "your journal couldn't store this file. free some space and try again."

  static func message(_ text: String) -> String {
    if text.contains("transfer stopped") { return stall }
    if text.contains("No space") || text.contains("ENOSPC") { return diskMessage }
    if text.contains("couldn't be read") { return unreadable }
    if text.contains("busy") { return busy }
    if text.contains("cancelled") || text.contains("aborted") { return aborted }
    if text == stall || text == lost || text == busy || text == aborted || text == unreadable || text == unsent || text == diskMessage {
      return text
    }
    return lost
  }

  static func disk(_ error: Error) -> Bool {
    let ns = error as NSError
    return ns.code == 640 || (ns.domain == NSPOSIXErrorDomain && ns.code == Int(ENOSPC))
  }

  static func headerPairs(_ raw: Any?) -> [[String]] {
    let allowed = Set(["accept", "content-type", "range", "if-none-match", "if-modified-since", "last-event-id"])
    guard let rows = raw as? [Any] else { return [] }
    var pairs: [[String]] = []
    for row in rows {
      let name: String
      let value: String
      if let typed = row as? [String], typed.count == 2 {
        name = typed[0]
        value = typed[1]
      } else if let untyped = row as? [Any], untyped.count == 2, let parsedName = untyped[0] as? String, let parsedValue = untyped[1] as? String {
        name = parsedName
        value = parsedValue
      } else {
        continue
      }
      if allowed.contains(name.lowercased()) { pairs.append([name.lowercased(), value]) }
    }
    return pairs
  }

  static func latin1(_ text: String) throws -> Data {
    var bytes = Data()
    bytes.reserveCapacity(text.utf16.count)
    for unit in text.utf16 {
      if bytes.count >= 65536 || unit > 255 { throw URLError(.dataLengthExceedsMaximum) }
      bytes.append(UInt8(unit))
    }
    return bytes
  }

  static func copyFile(_ source: URL, to dest: URL) throws {
    let input = try FileHandle(forReadingFrom: source)
    defer { try? input.close() }
    FileManager.default.createFile(atPath: dest.path, contents: nil)
    let output = try FileHandle(forWritingTo: dest)
    defer { try? output.close() }
    while true {
      let chunk = try input.read(upToCount: 64 * 1024) ?? Data()
      if chunk.isEmpty { return }
      try output.write(contentsOf: chunk)
    }
  }
}

private final class OwnerStreamPump: @unchecked Sendable {
  let pageID: String
  let nativeID: UInt64
  let authority: UInt64
  let url: String
  private let bodyWriter: Int32
  private let responseReader: Int32
  private weak var bridge: OwnerUploadBridge?
  private let lock = NSLock()
  private var queued: Data?
  private var finished = false
  private var cancelled = false
  private var bodyOpen = true
  private var responseOpen = true

  init(pageID: String, nativeID: UInt64, authority: UInt64, url: String, bodyWriter: Int32, responseReader: Int32, bridge: OwnerUploadBridge) {
    self.pageID = pageID
    self.nativeID = nativeID
    self.authority = authority
    self.url = url
    self.bodyWriter = bodyWriter
    self.responseReader = responseReader
    self.bridge = bridge
  }

  enum Offer { case accepted, busy, closed }
  func offer(_ data: Data) -> Offer {
    lock.lock(); defer { lock.unlock() }
    if cancelled { return .closed }
    if queued != nil { return .busy }
    queued = data
    return .accepted
  }

  func markFinished() {
    lock.lock(); finished = true; lock.unlock()
  }

  func cancel() {
    lock.lock()
    if cancelled { lock.unlock(); return }
    cancelled = true
    lock.unlock()
    OwnerOutcome.cancel(pageID, authority: authority)
    journal_ios_owner_cancel(nativeID)
    releaseDescriptors()
  }

  func run() {
    var quiet = Date()
    while bridge?.authorityMatches(authority) == true && !isCancelled {
      if let chunk = take() {
        if !write(chunk, quiet: &quiet) { return }
        continue
      }
      if isFinished {
        if bodyOpen { closeBody() ; quiet = Date() }
        switch pollHead(quiet: &quiet) {
        case .pending:
          if Date().timeIntervalSince(quiet) > 60 { fail(OwnerUploadCopy.stall); return }
          usleep(50_000)
        case .done: return
        }
        continue
      }
      if Date().timeIntervalSince(quiet) > 60 { fail(OwnerUploadCopy.stall); return }
      usleep(50_000)
    }
    cancel()
  }

  private var isCancelled: Bool { lock.lock(); defer { lock.unlock() }; return cancelled }
  private var isFinished: Bool { lock.lock(); defer { lock.unlock() }; return finished }

  private func take() -> Data? {
    lock.lock(); defer { lock.unlock() }
    let chunk = queued
    queued = nil
    return chunk
  }

  private func write(_ chunk: Data, quiet: inout Date) -> Bool {
    var offset = 0
    let bytes = [UInt8](chunk)
    while offset < bytes.count {
      if isCancelled || bridge?.authorityMatches(authority) != true { cancel(); return false }
      let wrote = bytes.withUnsafeBytes { raw -> ssize_t in
        guard let base = raw.bindMemory(to: UInt8.self).baseAddress else { return -1 }
        return Darwin.write(bodyWriter, base.advanced(by: offset), bytes.count - offset)
      }
      if wrote > 0 { offset += Int(wrote); quiet = Date(); continue }
      if wrote < 0 && (errno == EAGAIN || errno == EINTR) {
        if Date().timeIntervalSince(quiet) > 60 { fail(OwnerUploadCopy.stall); return false }
        usleep(50_000)
        continue
      }
      fail(OwnerUploadCopy.unreadable)
      return false
    }
    return true
  }

  private enum Head { case pending, done }

  private func pollHead(quiet: inout Date) -> Head {
    guard let pointer = journal_ios_owner_poll(nativeID) else { fail(OwnerUploadCopy.lost); return .done }
    defer { journal_runtime_free(pointer) }
    let parsed = (try? JSONSerialization.jsonObject(with: Data(String(cString: pointer).utf8))) as? [String: Any]
    guard parsed?["ok"] as? Bool == true, let value = parsed?["value"] as? [String: Any] else {
      fail(OwnerUploadCopy.message(parsed?["error"] as? String ?? ""))
      return .done
    }
    if value["pending"] as? Bool == true { return .pending }
    let status = value["status"] as? Int ?? 0
    var headers: [String: String] = [:]
    for pair in value["headers"] as? [[String]] ?? [] where pair.count == 2 { headers[pair[0]] = pair[1] }
    let body: Data
    do { body = try readResponse(quiet: &quiet) } catch let error as URLError where error.code == .timedOut {
      fail(OwnerUploadCopy.stall)
      return .done
    } catch {
      fail(OwnerUploadCopy.lost)
      return .done
    }
    if isCancelled || bridge?.authorityMatches(authority) != true {
      releaseDescriptors()
      return .done
    }
    let stored = OwnerOutcome.keep(pageID, authority: authority, reply: [
      "status": status, "headers": headers, "url": url,
      "body": body.base64EncodedString(),
    ])
    releaseDescriptors()
    if !stored { fail(OwnerUploadCopy.lost) }
    return .done
  }

  private func readResponse(quiet: inout Date) throws -> Data {
    var data = Data()
    var buffer = [UInt8](repeating: 0, count: 64 * 1024)
    while data.count <= 8 * 1024 * 1024 {
      var descriptor = pollfd(fd: responseReader, events: Int16(POLLIN), revents: 0)
      let ready = poll(&descriptor, 1, 200)
      if ready < 0 { if errno == EINTR { continue }; throw URLError(.networkConnectionLost) }
      if ready == 0 {
        if Date().timeIntervalSince(quiet) > 60 { throw URLError(.timedOut) }
        continue
      }
      let count = Darwin.read(responseReader, &buffer, buffer.count)
      if count == 0 { return data }
      if count < 0 { if errno == EINTR || errno == EAGAIN { continue }; throw URLError(.networkConnectionLost) }
      if data.count + count > 8 * 1024 * 1024 { throw URLError(.dataLengthExceedsMaximum) }
      data.append(buffer, count: count)
      quiet = Date()
    }
    throw URLError(.dataLengthExceedsMaximum)
  }

  private func fail(_ message: String) {
    _ = OwnerOutcome.keep(pageID, authority: authority, reply: ["error": message])
    journal_ios_owner_cancel(nativeID)
    releaseDescriptors()
  }

  private func closeBody() {
    lock.lock()
    let open = bodyOpen
    bodyOpen = false
    lock.unlock()
    if open { Darwin.close(bodyWriter) }
  }

  private func releaseDescriptors() {
    lock.lock()
    let writer = bodyOpen ? bodyWriter : -1
    let reader = responseOpen ? responseReader : -1
    bodyOpen = false
    responseOpen = false
    lock.unlock()
    if writer >= 0 { Darwin.close(writer) }
    if reader >= 0 { Darwin.close(reader) }
  }
}
#endif
