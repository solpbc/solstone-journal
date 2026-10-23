// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
import Foundation

enum RuntimeIdentity {
  #if JOURNAL_OWNER_APP
  static let host = "app.solstone.journal.owner.dev"
  static let group = "group.app.solstone.journal.owner.dev"
  #else
  static let host = "app.solstone.journal.runtime.dev"
  static let group = "group.app.solstone.journal.runtime.dev"
  #endif
  static let appraisal = host + ".gpu-appraisal"
}
