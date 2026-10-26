// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! One resolver for the views pinned to the side rail.
//!
//! The stored value is `config/journal.json` → `convey.pinned_views`. Reads
//! use [`read_journal_config`] and never materialize a missing file.

use std::collections::HashSet;
use std::path::Path;

use serde_json::{Map, Value};
use solstone_core_journal_config::{ConfigLoadError, read_journal_config};

use crate::registry::served_apps;

pub(crate) const PINNED_VIEWS_KEY: &str = "pinned_views";

pub const DEFAULT_PINNED_VIEWS: &[&str] = &["search", "entities", "thinking"];

pub fn is_pinnable(name: &str) -> bool {
    served_apps(true).any(|app| app.name == name && app.pinnable)
}

pub(crate) fn known_view(name: &str) -> bool {
    served_apps(true).any(|app| app.name == name)
}

fn default_pinned_views() -> Vec<String> {
    DEFAULT_PINNED_VIEWS
        .iter()
        .map(|name| (*name).to_owned())
        .collect()
}

/// Resolve a stored `pinned_views` value.
///
/// `None` is a missing key. An array of strings keeps the first pinnable
/// name of each view. Any other JSON value resolves to the defaults.
pub fn resolve(stored: Option<&Value>) -> Vec<String> {
    let Some(value) = stored else {
        return default_pinned_views();
    };
    let Some(items) = value.as_array() else {
        return default_pinned_views();
    };
    if items.iter().any(|item| !item.is_string()) {
        return default_pinned_views();
    }
    let mut seen = HashSet::new();
    let mut resolved = Vec::new();
    for item in items {
        let Some(name) = item.as_str() else {
            continue;
        };
        if is_pinnable(name) && seen.insert(name) {
            resolved.push(name.to_owned());
        }
    }
    resolved
}

/// Read the resolved pin list. A missing file, a missing `convey` object, or
/// a non-object `convey` resolves to the defaults and does not write.
pub fn read_pinned_views(journal_root: &Path) -> Result<Vec<String>, ConfigLoadError> {
    let read = read_journal_config(journal_root)?;
    Ok(pinned_from_config(read.config.as_ref()))
}

pub(crate) fn pinned_from_config(config: Option<&Map<String, Value>>) -> Vec<String> {
    let Some(config) = config else {
        return default_pinned_views();
    };
    match config.get("convey") {
        None => default_pinned_views(),
        Some(convey) if convey.is_object() => resolve(convey.get(PINNED_VIEWS_KEY)),
        Some(_) => default_pinned_views(),
    }
}

/// Apply a pin or unpin to the in-memory config map.
///
/// The caller holds the journal config lock. A non-object `convey` is refused
/// and the map is left untouched. `changed` is true only when the resolved
/// membership changes; a no-op leaves a dirty stored value in place.
pub(crate) fn toggle(
    config: &mut Map<String, Value>,
    view: &str,
    pin: bool,
) -> Result<(Vec<String>, bool), String> {
    if let Some(convey) = config.get("convey")
        && !convey.is_object()
    {
        return Err("convey must be an object".to_owned());
    }
    let stored = config
        .get("convey")
        .and_then(|convey| convey.get(PINNED_VIEWS_KEY))
        .cloned();
    let current = resolve(stored.as_ref());
    let mut next = current.clone();
    if pin {
        if !next.iter().any(|name| name == view) {
            next.push(view.to_owned());
        }
    } else {
        next.retain(|name| name != view);
    }
    let changed = next != current;
    if changed {
        let convey = config
            .entry("convey".to_owned())
            .or_insert_with(|| Value::Object(Map::new()));
        let convey = convey
            .as_object_mut()
            .expect("convey is an object when a pin changes");
        convey.insert(
            PINNED_VIEWS_KEY.to_owned(),
            Value::Array(next.iter().cloned().map(Value::String).collect()),
        );
    }
    Ok((next, changed))
}

#[cfg(test)]
mod tests {
    use super::{resolve, toggle};
    use serde_json::json;

    #[test]
    fn pinned_views_resolve_table_handles_absent_empty_unknown_dirty_and_corrupt_shapes() {
        assert_eq!(resolve(None), ["search", "entities", "thinking"]);
        assert_eq!(resolve(Some(&json!([]))), Vec::<String>::new());
        assert_eq!(resolve(Some(&json!(["news"]))), Vec::<String>::new());
        assert_eq!(
            resolve(Some(&json!([
                "news", "entities", "home", "entities", "search"
            ]))),
            ["entities", "search"]
        );
        assert_eq!(
            resolve(Some(&json!("oops"))),
            ["search", "entities", "thinking"]
        );
        assert_eq!(
            resolve(Some(&json!([1]))),
            ["search", "entities", "thinking"]
        );
        assert_eq!(
            resolve(Some(&json!(["entities", "entities", "news"]))),
            ["entities"]
        );

        let mut absent = serde_json::Map::new();
        let (list, changed) = toggle(&mut absent, "map", true).unwrap();
        assert!(changed);
        assert_eq!(list, ["search", "entities", "thinking", "map"]);

        let mut empty = json!({"convey": {"pinned_views": []}});
        let map = empty.as_object_mut().unwrap();
        let (list, changed) = toggle(map, "map", true).unwrap();
        assert!(changed);
        assert_eq!(list, ["map"]);
        assert_eq!(map["convey"]["pinned_views"], json!(["map"]));

        let mut dirty =
            json!({"convey": {"pinned_views": ["entities", "entities", "news"], "secret": "s"}});
        let before = dirty.clone();
        {
            let map = dirty.as_object_mut().unwrap();
            let (list, changed) = toggle(map, "entities", true).unwrap();
            assert!(!changed);
            assert_eq!(list, ["entities"]);
        }
        assert_eq!(dirty, before);

        let map = dirty.as_object_mut().unwrap();
        let (list, changed) = toggle(map, "entities", false).unwrap();
        assert!(changed);
        assert_eq!(list, Vec::<String>::new());
        assert_eq!(map["convey"]["pinned_views"], json!([]));
        assert_eq!(map["convey"]["secret"], "s");

        let mut blocked = json!({"convey": ["not", "an", "object"], "setup": {"completed_at": 1}});
        let before = blocked.clone();
        let error = toggle(blocked.as_object_mut().unwrap(), "map", true).unwrap_err();
        assert!(error.contains("object"));
        assert_eq!(blocked, before);
    }

    #[test]
    fn non_object_convey_reads_as_defaults() {
        let config = json!({"convey": "nope"});
        assert_eq!(
            super::pinned_from_config(config.as_object()),
            ["search", "entities", "thinking"]
        );
        assert_eq!(
            super::pinned_from_config(None),
            ["search", "entities", "thinking"]
        );
    }
}
