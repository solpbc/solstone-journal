// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
#if JOURNAL_OWNER_APP
import Darwin
import SafariServices
import SwiftUI
@preconcurrency import WebKit

enum OwnerOrigin {
  static let scheme = "journal-owner"
  static let host = "local"
  static func admit(_ url: URL) throws -> String {
    guard let parts = URLComponents(url: url, resolvingAgainstBaseURL: false),
          parts.scheme == scheme, parts.host == host, parts.port == nil,
          parts.user == nil, parts.password == nil else { throw URLError(.unsupportedURL) }
    let path = parts.percentEncodedPath.isEmpty ? "/" : parts.percentEncodedPath
    guard path.hasPrefix("/"), !path.hasPrefix("//") else { throw URLError(.unsupportedURL) }
    return path + (parts.percentEncodedQuery.map { "?" + $0 } ?? "")
  }
  static func url(_ path: String) -> URL { URL(string: "\(scheme)://\(host)\(path)")! }
}

/// One process-wide identity for an ordinary read or a streamed upload.
/// Folder copies do not take an id from here.
enum OwnerRequestIdentity {
  static func next() -> UInt64 { journal_ios_owner_next_id() }
}
final class OwnerRead: @unchecked Sendable {
  private let lock = NSLock()
  private var stopped = false
  private var activeID: UInt64?
  var cancelled: Bool { lock.lock(); defer { lock.unlock() }; return stopped }
  func cancel() {
    lock.lock(); stopped = true; let id = activeID; lock.unlock()
    if let id { DispatchQueue.global().async { journal_ios_owner_cancel(id) } }
  }
  struct Head { let url: URL; let status: Int; let headers: [String: String] }
  struct Redirect: Error { let request: URLRequest }
  func run(
    _ request: URLRequest,
    followRedirects: Bool,
    head: @escaping (Head) throws -> Void,
    bytes: @escaping (Data) throws -> Void
  ) throws {
    var url = request.url!
    var method = request.httpMethod ?? "GET"
    guard request.httpBodyStream == nil else { throw URLError(.dataLengthExceedsMaximum) }
    var body = request.httpBody ?? Data()
    guard body.count <= 256 * 1024, let text = String(data: body, encoding: .utf8) else {
      throw URLError(.dataLengthExceedsMaximum)
    }
    var bodyText = text
    let allowed = Set(["accept", "content-type", "range", "if-none-match", "if-modified-since", "last-event-id"])
    var headers = request.allHTTPHeaderFields?.compactMap { key, value -> [String]? in
      allowed.contains(key.lowercased()) ? [key.lowercased(), value] : nil
    } ?? []
    for hop in 0..<8 {
      if cancelled { throw URLError(.cancelled) }
      let id = OwnerRequestIdentity.next()
      lock.lock(); activeID = id; lock.unlock()
      let path = try OwnerOrigin.admit(url)
      let (reader, writer) = try OwnerPipe.make()
      let encoded: Data
      do {
        encoded = try JSONSerialization.data(withJSONObject: [
          "id": id, "admissionGeneration": journal_ios_processing_generation(),
          "method": method, "uri": path, "headers": headers, "body": bodyText,
        ])
      } catch { close(reader); close(writer); throw error }
      // Ownership of writer transfers even if the native entry refuses.
      let pointer = encoded.withUnsafeBytes {
        journal_ios_owner_open($0.bindMemory(to: UInt8.self).baseAddress, UInt($0.count), writer)
      }
      defer { close(reader) }
      guard let pointer else { throw URLError(.cannotParseResponse) }
      defer { journal_runtime_free(pointer) }
      if cancelled { journal_ios_owner_cancel(id); throw URLError(.cancelled) }
      let data = Data(String(cString: pointer).utf8)
      guard let reply = try JSONSerialization.jsonObject(with: data) as? [String: Any],
            reply["ok"] as? Bool == true,
            let value = reply["value"] as? [String: Any],
            let status = value["status"] as? Int,
            let pairs = value["headers"] as? [[String]] else { throw URLError(.badServerResponse) }
      var responseHeaders: [String: String] = [:]
      for pair in pairs where pair.count == 2 { responseHeaders[pair[0].lowercased()] = pair[1] }
      if [301, 302, 303, 307, 308].contains(status) {
        journal_ios_owner_cancel(id)
        guard hop < 7, let location = responseHeaders["location"],
              let next = URL(string: location, relativeTo: url)?.absoluteURL else { throw URLError(.redirectToNonExistentLocation) }
        _ = try OwnerOrigin.admit(next)
        url = next
        if status == 303 || [301, 302].contains(status) && method == "POST" {
          method = "GET"; body = Data(); bodyText = ""
          headers.removeAll { $0[0] == "content-type" }
        }
        if !followRedirects {
          var nextRequest = request
          nextRequest.url = next
          nextRequest.httpMethod = method
          nextRequest.httpBody = body
          if body.isEmpty { nextRequest.setValue(nil, forHTTPHeaderField: "Content-Type") }
          throw Redirect(request: nextRequest)
        }
        continue
      }
      try head(Head(url: url, status: status, headers: responseHeaders))
      var received = 0
      var buffer = [UInt8](repeating: 0, count: 64 * 1024)
      while !cancelled {
        var descriptor = pollfd(fd: reader, events: Int16(POLLIN), revents: 0)
        let ready = poll(&descriptor, 1, 200)
        if ready < 0 {
          if errno == EINTR { continue }
          throw URLError(.networkConnectionLost)
        }
        if ready == 0 { continue }
        let count = read(reader, &buffer, buffer.count)
        if count < 0 {
          if errno == EINTR || errno == EAGAIN { continue }
          throw URLError(.networkConnectionLost)
        }
        if count == 0 {
          if method != "HEAD", let expected = responseHeaders["content-length"].flatMap(Int.init), expected != received {
            throw URLError(.networkConnectionLost)
          }
          return
        }
        received += count
        try bytes(Data(buffer.prefix(count)))
      }
      throw URLError(.cancelled)
    }
  }
}

@MainActor final class OwnerBrowserCoordinator: NSObject, WKURLSchemeHandler, WKNavigationDelegate, WKUIDelegate {
  private struct Slot { let read: OwnerRead; let task: WKURLSchemeTask }
  private let owner: OwnerJournal
  let uploads: OwnerUploadBridge
  private var requests: [ObjectIdentifier: Slot] = [:]
  private weak var web: WKWebView?
  private var mainNavigation: URL
  private var redirects = 0
  private let workers: OperationQueue = {
    let queue = OperationQueue()
    queue.maxConcurrentOperationCount = 8
    queue.qualityOfService = .userInitiated
    return queue
  }()
  init(_ owner: OwnerJournal) {
    self.owner = owner
    uploads = OwnerUploadBridge(owner)
    mainNavigation = OwnerOrigin.url(owner.route)
  }
  func close() {
    uploads.close()
    for request in requests.values { request.read.cancel() }
    requests.removeAll()
    workers.cancelAllOperations()
  }
  func webView(_ webView: WKWebView, start task: WKURLSchemeTask) {
    web = webView
    let key = ObjectIdentifier(task)
    guard requests.count < 32, let url = task.request.url, (try? OwnerOrigin.admit(url)) != nil else {
      task.didFailWithError(URLError(.unsupportedURL)); return
    }
    let type = task.request.value(forHTTPHeaderField: "Content-Type")?.lowercased() ?? ""
    if task.request.httpBodyStream != nil || type.hasPrefix("multipart/")
      || type.hasPrefix("application/octet-stream") || (task.request.httpBody?.count ?? 0) > 256 * 1024 {
      owner.uploadUnavailable()
      task.didFailWithError(URLError(.dataLengthExceedsMaximum))
      return
    }
    let read = OwnerRead()
    requests[key] = Slot(read: read, task: task)
    let request = task.request
    let mainDocument = url == mainNavigation
    workers.addOperation { [weak self] in
      func deliver(_ block: @escaping @MainActor (WKURLSchemeTask, OwnerBrowserCoordinator) -> Void) {
        DispatchQueue.main.sync {
          guard let self, let slot = self.requests[key], slot.read === read, !read.cancelled else { return }
          block(slot.task, self)
        }
      }
      do {
        try read.run(request, followRedirects: !mainDocument, head: { head in
          var headers = head.headers
          let policy = "default-src 'self' journal-owner: data: blob:; script-src 'self' journal-owner: 'unsafe-inline'; style-src 'self' journal-owner: 'unsafe-inline'; connect-src journal-owner:; frame-src 'none'; object-src 'none'; base-uri journal-owner:; form-action journal-owner:"
          if let existing = headers["content-security-policy"] { headers["content-security-policy"] = existing + ", " + policy }
          else { headers["content-security-policy"] = policy }
          guard let response = HTTPURLResponse(url: head.url, statusCode: head.status, httpVersion: "HTTP/1.1", headerFields: headers) else {
            throw URLError(.cannotParseResponse)
          }
          deliver { task, coordinator in task.didReceive(response); if mainDocument { coordinator.redirects = 0 } }
        }, bytes: { bytes in deliver { task, _ in task.didReceive(bytes) } })
        deliver { task, coordinator in task.didFinish(); coordinator.requests.removeValue(forKey: key) }
      } catch let redirect as OwnerRead.Redirect {
        deliver { task, coordinator in
          coordinator.requests.removeValue(forKey: key)
          if coordinator.redirects < 8 {
            coordinator.redirects += 1
            task.didFailWithError(URLError(.cancelled))
            coordinator.web?.load(redirect.request)
          } else { task.didFailWithError(URLError(.httpTooManyRedirects)); coordinator.owner.pageFailed() }
        }
      } catch {
        deliver { task, coordinator in
          task.didFailWithError(error)
          coordinator.requests.removeValue(forKey: key)
          if mainDocument { coordinator.owner.pageFailed() }
        }
      }
    }
  }
  func webView(_ webView: WKWebView, stop task: WKURLSchemeTask) {
    requests.removeValue(forKey: ObjectIdentifier(task))?.read.cancel()
  }
  // Leaving the app pauses the journal, which would drop a service turn-on that is
  // waiting for its consent page. Show outside pages over the journal instead.
  private func external(_ url: URL) {
    guard ["https", "http"].contains(url.scheme?.lowercased() ?? ""),
          url.host != nil, url.user == nil, url.password == nil else { return }
    guard let top = topController() else { UIApplication.shared.open(url); return }
    top.present(SFSafariViewController(url: url), animated: true)
  }
  private func topController() -> UIViewController? {
    guard let scene = UIApplication.shared.connectedScenes.first as? UIWindowScene,
          var top = scene.windows.first(where: { $0.isKeyWindow })?.rootViewController else { return nil }
    while let presented = top.presentedViewController { top = presented }
    return top
  }
  func webView(_ webView: WKWebView, decidePolicyFor action: WKNavigationAction,
               decisionHandler: @escaping (WKNavigationActionPolicy) -> Void) {
    guard let url = action.request.url else { decisionHandler(.cancel); return }
    if (try? OwnerOrigin.admit(url)) != nil {
      if action.targetFrame?.isMainFrame == true {
        mainNavigation = url
        decisionHandler(.allow)
      } else {
        decisionHandler(.cancel)
        if action.targetFrame == nil { mainNavigation = url; webView.load(action.request) }
      }
    } else {
      decisionHandler(.cancel)
      if action.targetFrame == nil || action.targetFrame?.isMainFrame == true { external(url) }
    }
  }
  func webView(_ webView: WKWebView, createWebViewWith configuration: WKWebViewConfiguration,
               for action: WKNavigationAction, windowFeatures: WKWindowFeatures) -> WKWebView? {
    if let url = action.request.url { external(url) }
    return nil
  }
  func webView(_: WKWebView, didStartProvisionalNavigation _: WKNavigation!) {
    uploads.rotateDocument()
  }
  func webView(_: WKWebView, didCommit _: WKNavigation!) {
    uploads.commitDocument()
  }
  func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) {
    if (error as NSError).code != NSURLErrorCancelled {
      uploads.abandonProvisionalDocument()
      owner.pageFailed()
    }
  }
  func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
    if (error as NSError).code != NSURLErrorCancelled { owner.pageFailed() }
  }
  func webViewWebContentProcessDidTerminate(_ webView: WKWebView) { close(); owner.pageFailed() }
  private func alert(_ message: String, confirm: Bool, done: @escaping (Bool) -> Void) {
    guard let root = topController() else { done(false); return }
    let alert = UIAlertController(title: nil, message: message, preferredStyle: .alert)
    alert.addAction(UIAlertAction(title: "ok", style: .default) { _ in done(true) })
    if confirm { alert.addAction(UIAlertAction(title: "cancel", style: .cancel) { _ in done(false) }) }
    root.present(alert, animated: true)
  }
  func webView(_ webView: WKWebView, runJavaScriptAlertPanelWithMessage message: String,
               initiatedByFrame frame: WKFrameInfo, completionHandler: @escaping () -> Void) {
    alert(message, confirm: false) { _ in completionHandler() }
  }
  func webView(_ webView: WKWebView, runJavaScriptConfirmPanelWithMessage message: String,
               initiatedByFrame frame: WKFrameInfo, completionHandler: @escaping (Bool) -> Void) {
    alert(message, confirm: true, done: completionHandler)
  }
}

struct OwnerBrowser: UIViewRepresentable {
  let owner: OwnerJournal
  func makeCoordinator() -> OwnerBrowserCoordinator { OwnerBrowserCoordinator(owner) }
  func makeUIView(context: Context) -> WKWebView {
    let configuration = WKWebViewConfiguration()
    configuration.websiteDataStore = .nonPersistent()
    configuration.setURLSchemeHandler(context.coordinator, forURLScheme: OwnerOrigin.scheme)
    context.coordinator.uploads.install(on: configuration)
    let contract = Bundle.main.url(forResource: "JournalWebHost", withExtension: "json")
      .flatMap { try? Data(contentsOf: $0) }
      .flatMap { try? JSONSerialization.jsonObject(with: $0) as? [String: Any] }
    if let product = contract?["user_agent_product"] as? String,
       let capability = contract?["javascript_capability"] as? String,
       let encoded = try? JSONSerialization.data(withJSONObject: capability, options: [.fragmentsAllowed]),
       let key = String(data: encoded, encoding: .utf8) {
      configuration.applicationNameForUserAgent = product
      configuration.userContentController.addUserScript(WKUserScript(
        source: "window[\(key)] = 1;", injectionTime: .atDocumentStart, forMainFrameOnly: true))
    }
    let web = WKWebView(frame: .zero, configuration: configuration)
    context.coordinator.uploads.attach(web)
    web.navigationDelegate = context.coordinator
    web.uiDelegate = context.coordinator
    web.allowsBackForwardNavigationGestures = true
    web.load(URLRequest(url: OwnerOrigin.url(owner.route)))
    return web
  }
  func updateUIView(_ web: WKWebView, context: Context) {}
  static func dismantleUIView(_ web: WKWebView, coordinator: OwnerBrowserCoordinator) {
    web.stopLoading(); coordinator.close(); web.navigationDelegate = nil; web.uiDelegate = nil
  }
}
#endif
