import ExtensionFoundation
// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
import Foundation
import XPC

struct Handler: XPCPeerHandler {
  func handleIncomingRequest(_ incoming: XPCReceivedMessage) -> (any Encodable)? {
    do {
      let message = try incoming.decode(as: Request.self)
      // All file/SQLite work is off the connection handler. Controls remain responsive.
      if message.op == "cancel" || message.op == "probe" || message.op == "activity" {
        return Engine.execute(message)
      }
      return incoming.handoffReply(to: Engine.queue) { incoming.reply(Engine.execute(message)) }
    } catch { return Reply(json: "{\"error\":\"invalid request\"}") }
  }

}
@main struct Worker: AppExtension {
  @AppExtensionPoint.Bind var boundExtensionPoint: AppExtensionPoint {
    #if JOURNAL_OWNER_APP
    AppExtensionPoint.Identifier(host: "app.solstone.journal.owner.dev", name: "journal")
    #else
    AppExtensionPoint.Identifier(host: "app.solstone.journal.runtime.dev", name: "journal")
    #endif
  }
  var configuration: some AppExtensionConfiguration {
    ConnectionHandler(onSessionRequest: { request in request.accept { _ in Handler() } })
  }
}
