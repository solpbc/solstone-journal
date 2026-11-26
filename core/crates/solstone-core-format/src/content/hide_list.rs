// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The shape the owner's hide stores share: one json object holding one list
//! of strings, and how this version reads one a later version wrote.
//!
//! A later version may add a field beside the list or a kind of entry in it.
//! Either could hide more than this version can work out, so a store holding
//! one is never read as if it weren't there, and never refused whole either:
//! the entries this version reads are honoured for every reader, the owner's
//! own pages included, while search and connections treat everything the
//! store decides as hidden. What isn't read is kept as [`Unread`] and written
//! back with the store.

use std::collections::BTreeSet;
use std::fmt;

use serde_json::{Map, Value};

/// What a later version wrote in a hide store that this version can't read.
/// Never empty when a reader returns one.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Unread {
    entries: Vec<Value>,
    fields: Map<String, Value>,
}

impl Unread {
    fn is_empty(&self) -> bool {
        self.entries.is_empty() && self.fields.is_empty()
    }
}

/// Counts only: what the owner hid is never printed.
impl fmt::Debug for Unread {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Unread")
            .field("entries", &self.entries.len())
            .field("fields", &self.fields.len())
            .finish()
    }
}

/// The entries of `list` in a store's bytes that `known` reads, and whatever
/// else the store holds. `None` when the bytes aren't a json object holding
/// `list` as a list: that is damage, not a later version.
pub(crate) fn parse_hide_list(
    bytes: &[u8],
    list: &str,
    known: impl Fn(&str) -> bool,
) -> Option<(BTreeSet<String>, Option<Unread>)> {
    let Ok(Value::Object(mut fields)) = serde_json::from_slice::<Value>(bytes) else {
        return None;
    };
    let Some(Value::Array(entries)) = fields.remove(list) else {
        return None;
    };
    let mut read = BTreeSet::new();
    let mut unread = Unread {
        entries: Vec::new(),
        fields,
    };
    for entry in entries {
        match entry {
            Value::String(entry) if known(&entry) => {
                read.insert(entry);
            }
            other => unread.entries.push(other),
        }
    }
    Some((read, (!unread.is_empty()).then_some(unread)))
}

/// The store's json for `entries` under `list`, in sorted order, with what a
/// later version wrote put back after them.
pub(crate) fn hide_list_bytes(
    list: &str,
    entries: &BTreeSet<String>,
    unread: Option<&Unread>,
) -> Vec<u8> {
    let mut all: Vec<Value> = entries.iter().cloned().map(Value::String).collect();
    let mut store = Map::new();
    if let Some(unread) = unread {
        all.extend(unread.entries.iter().cloned());
    }
    store.insert(list.to_owned(), Value::Array(all));
    if let Some(unread) = unread {
        store.extend(unread.fields.clone());
    }
    serde_json::to_vec(&Value::Object(store)).expect("a hide store serializes")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn what_a_later_version_wrote_is_kept_and_written_back() {
        let later = json!({
            "keys": ["k2", {"from": "k5"}, "k1", 7],
            "until": "20270101",
        });
        let (read, unread) =
            parse_hide_list(later.to_string().as_bytes(), "keys", |_| true).unwrap();
        assert_eq!(read, BTreeSet::from(["k1".to_owned(), "k2".to_owned()]));
        let unread = unread.expect("the store holds more than this version reads");
        let written: Value =
            serde_json::from_slice(&hide_list_bytes("keys", &read, Some(&unread))).unwrap();
        assert_eq!(
            written,
            json!({"keys": ["k1", "k2", {"from": "k5"}, 7], "until": "20270101"})
        );
    }

    #[test]
    fn a_store_this_version_wrote_has_nothing_unread() {
        let bytes = hide_list_bytes("keys", &BTreeSet::from(["k1".to_owned()]), None);
        assert_eq!(bytes, br#"{"keys":["k1"]}"#);
        let (read, unread) = parse_hide_list(&bytes, "keys", |_| true).unwrap();
        assert_eq!(read, BTreeSet::from(["k1".to_owned()]));
        assert_eq!(unread, None);
    }

    #[test]
    fn damage_is_not_a_later_version() {
        for damaged in [&b"garbage"[..], br#"{"keys":"k1"}"#, br#"{}"#, br#"["k1"]"#] {
            assert_eq!(parse_hide_list(damaged, "keys", |_| true), None);
        }
    }

    #[test]
    fn what_is_unread_prints_as_counts_only() {
        let later = br#"{"keys":["k1",{"secret":"name"}],"private":"word"}"#;
        let (_, unread) = parse_hide_list(later, "keys", |_| true).unwrap();
        let printed = format!("{:?}", unread.unwrap());
        assert_eq!(printed, "Unread { entries: 1, fields: 1 }");
    }
}
