// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

import Foundation

// Compile-only consumer of the generated API under a MainActor-default target.
nonisolated func verifySynchronousCatalogAdapter() -> String {
    let key = CatalogSelect.resolveKey(messageId: "compile-only", enumArg: nil)
    return key
        + CatalogSelect.formatIntegerUngrouped(10000)
        + CatalogSelect.formatIntegerLocale(10000, locale: "en-US")
}
