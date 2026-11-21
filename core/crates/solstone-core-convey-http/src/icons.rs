// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Shared Lucide icon SVG lookup for Convey web surfaces.

use std::collections::HashMap;
use std::sync::LazyLock;

static LUCIDE: LazyLock<HashMap<String, String>> = LazyLock::new(|| {
    serde_json::from_str(include_str!(
        "../../solstone-core-convey-shell/assets/static/icons/lucide.json"
    ))
    .expect("embedded Lucide catalogue")
});

/// The embedded Lucide icon map (`name -> svg`).
pub fn catalogue() -> &'static HashMap<String, String> {
    &LUCIDE
}

/// The named Lucide icon's SVG, or `None` when unset or unknown.
/// An emoji is never turned into an icon: it is shown as the owner chose it.
pub fn svg(name: Option<&str>) -> Option<String> {
    name.filter(|name| !name.is_empty())
        .and_then(|name| LUCIDE.get(name))
        .cloned()
}
