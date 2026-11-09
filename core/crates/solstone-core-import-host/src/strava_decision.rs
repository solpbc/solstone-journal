// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! What a Strava workout's code record holds once the records an earlier
//! reading of its tiles filed are folded into it.
//!
//! The decision is written onto the code record, so it survives the Sense lines
//! being removed one at a time. Everything here is pure: the same record and
//! lines decide the same thing, however often and from whichever partial state
//! it runs.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use solstone_core_format::content::{edit_names, edit_removed, owner_edit};

/// The fields an owner writes that a code record can hold.
const CARRIED_FIELDS: [&str; 4] = ["title", "description", "details", "playback"];
/// What `carried` says on conversion's own hide or show.
const DECISION: &str = "decision";
const HIDDEN: &str = "hidden";
const EDITS: &str = "edits";
const CONVERTED: &str = "converted";
/// On `converted`: the record was one the owner could see before its first
/// decision, so it counts as visible in the hide rule.
pub(crate) const RECORD_VISIBLE: &str = "record_visible";

type Record = Map<String, Value>;

/// One record an earlier reading of a workout's tiles filed.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SenseLine {
    pub day: String,
    pub facet: String,
    pub id: String,
    pub record: Record,
}

impl SenseLine {
    /// The name a line goes by in the decision, the list, receipts and outputs.
    /// Sense gives a two-facet workout one id in both, so the id isn't enough.
    #[must_use]
    pub(crate) fn key(&self) -> String {
        format!("{}|{}|{}", self.day, self.facet, self.id)
    }

    /// A digest of the line as the store holds it. It's taken over the parsed
    /// line, because the store rewrites a whole day file when it writes one.
    #[must_use]
    pub(crate) fn digest(&self) -> String {
        record_digest(&self.record)
    }

    fn created_at(&self) -> i64 {
        self.record
            .get("created_at")
            .and_then(Value::as_i64)
            .unwrap_or(i64::MAX)
    }
}

/// What [`SenseLine::digest`] gives for a record.
#[must_use]
pub(crate) fn record_digest(record: &Record) -> String {
    let bytes = serde_json::to_vec(record).unwrap_or_default();
    Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn edits_of(record: &Record) -> Vec<Record> {
    record
        .get(EDITS)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .cloned()
        .collect()
}

fn text<'a>(edit: &'a Record, field: &str) -> &'a str {
    edit.get(field).and_then(Value::as_str).unwrap_or_default()
}

fn carried_of(edit: &Record) -> Option<&str> {
    edit.get("carried").and_then(Value::as_str)
}

/// The owner's newest live edit naming `field`, as (position, edit).
fn newest_owner_edit<'a>(edits: &'a [Record], field: &str) -> Option<(usize, &'a Record)> {
    edits
        .iter()
        .enumerate()
        .filter(|(_, edit)| owner_edit(edit) && !edit_removed(edit) && edit_names(edit, field))
        .max_by(|(left_at, left), (right_at, right)| {
            (text(left, "timestamp"), left_at).cmp(&(text(right, "timestamp"), right_at))
        })
}

/// The edit entries a line carries: one per source edit that is the newest for
/// any field, naming only the fields it's newest for. An edit saved before
/// edits recorded their values carries the field as it stands on the line.
fn carried_entries(line: &SenseLine) -> Vec<Record> {
    let edits = edits_of(&line.record);
    let mut by_edit: BTreeMap<usize, Vec<(&str, Value)>> = BTreeMap::new();
    for field in CARRIED_FIELDS {
        let Some((position, edit)) = newest_owner_edit(&edits, field) else {
            continue;
        };
        let value = edit
            .get("set")
            .and_then(|set| set.get(field))
            .or_else(|| line.record.get(field));
        if let Some(value) = value {
            by_edit
                .entry(position)
                .or_default()
                .push((field, value.clone()));
        }
    }
    let key = line.key();
    by_edit
        .into_iter()
        .map(|(position, fields)| {
            let edit = &edits[position];
            let mut entry = Record::new();
            entry.insert(
                "timestamp".to_owned(),
                edit.get("timestamp").cloned().unwrap_or(Value::Null),
            );
            entry.insert(
                "actor".to_owned(),
                edit.get("actor").cloned().unwrap_or(Value::Null),
            );
            entry.insert(
                "fields".to_owned(),
                json!(fields.iter().map(|(name, _)| *name).collect::<Vec<_>>()),
            );
            entry.insert(
                "note".to_owned(),
                edit.get("note").cloned().unwrap_or_else(|| json!("")),
            );
            entry.insert(
                "set".to_owned(),
                Value::Object(
                    fields
                        .into_iter()
                        .map(|(name, value)| (name.to_owned(), value))
                        .collect(),
                ),
            );
            entry.insert("before".to_owned(), json!({}));
            entry.insert("carried".to_owned(), Value::String(key.clone()));
            entry
        })
        .collect()
}

/// An owner or command-line hide.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct OwnerHide {
    pub timestamp: String,
    pub actor: String,
    pub note: String,
}

/// The owner or command-line hide a line stands on, when that's why it's
/// hidden. A line hidden any other way counts as visible.
pub(crate) fn owner_hide(line: &SenseLine) -> Option<OwnerHide> {
    let hidden = line.record.get(HIDDEN).is_some_and(|value| {
        value.as_bool().unwrap_or(false) || value.as_str().is_some_and(|text| !text.is_empty())
    });
    if !hidden {
        return None;
    }
    let edits = edits_of(&line.record);
    let (_, edit) = edits
        .iter()
        .enumerate()
        .filter(|(_, edit)| edit_names(edit, HIDDEN))
        .max_by_key(|(at, edit)| (text(edit, "timestamp").to_owned(), *at))?;
    owner_edit(edit).then(|| OwnerHide {
        timestamp: text(edit, "timestamp").to_owned(),
        actor: text(edit, "actor").to_owned(),
        note: text(edit, "note").to_owned(),
    })
}

/// What's left of a changed line's entries once everything the owner removed
/// from this line's earlier entries is taken out: a change can add what's new,
/// and an edit the owner removed stays removed.
fn without_removed(new: Vec<Record>, removed: &[Record]) -> Vec<Record> {
    new.into_iter()
        .filter_map(|mut entry| {
            let same_edit = |gone: &&Record| {
                text(gone, "timestamp") == text(&entry, "timestamp")
                    && text(gone, "actor") == text(&entry, "actor")
            };
            let taken = removed
                .iter()
                .filter(same_edit)
                .flat_map(|gone| {
                    CARRIED_FIELDS
                        .into_iter()
                        .filter(|field| edit_names(gone, field))
                })
                .collect::<Vec<_>>();
            let kept = entry
                .get("fields")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(|field| !field.as_str().is_some_and(|name| taken.contains(&name)))
                .cloned()
                .collect::<Vec<_>>();
            if kept.is_empty() {
                return None;
            }
            entry.insert("fields".to_owned(), Value::Array(kept));
            if let Some(Value::Object(set)) = entry.get_mut("set") {
                set.retain(|field, _| !taken.contains(&field.as_str()));
            }
            Some(entry)
        })
        .collect()
}

/// What the decision keeps about one line.
struct LineEntry {
    digest: String,
    hidden: bool,
    hide: Option<OwnerHide>,
    fields: Map<String, Value>,
}

impl LineEntry {
    fn stored(value: &Value) -> Self {
        Self {
            digest: value
                .get("sha256")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            hidden: value.get(HIDDEN).and_then(Value::as_bool).unwrap_or(false),
            hide: value
                .get("hide")
                .and_then(Value::as_object)
                .map(|hide| OwnerHide {
                    timestamp: text(hide, "timestamp").to_owned(),
                    actor: text(hide, "actor").to_owned(),
                    note: text(hide, "note").to_owned(),
                }),
            fields: value
                .get("fields")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default(),
        }
    }

    fn to_value(&self) -> Value {
        let mut value =
            json!({"sha256": self.digest, "hidden": self.hidden, "fields": self.fields});
        if let Some(hide) = &self.hide {
            value["hide"] =
                json!({"timestamp": hide.timestamp, "actor": hide.actor, "note": hide.note});
        }
        value
    }
}

/// Which line a workout's owner values come from first: the line in the target
/// facet naming tile 0's live key, else the earliest by `created_at`, ties
/// broken by the line's key.
#[must_use]
pub(crate) fn choose_primary(
    lines: &[&SenseLine],
    target_facet: &str,
    first_tile: Option<(&str, &str)>,
) -> Option<String> {
    let names_first = |line: &&&SenseLine| {
        first_tile.is_some_and(|(day, key)| {
            line.day == day
                && line
                    .record
                    .get("segments")
                    .and_then(Value::as_array)
                    .is_some_and(|segments| segments.iter().any(|s| s.as_str() == Some(key)))
        })
    };
    lines
        .iter()
        .filter(|line| line.facet == target_facet)
        .find(names_first)
        .or_else(|| {
            lines
                .iter()
                .min_by_key(|line| (line.created_at(), line.key()))
        })
        .map(|line| line.key())
}

/// A workout's code record after folding `lines` into it. `base` is the record
/// as it stands, or a fresh one. `carries_fields` is the lines whose owner
/// values go to this workout: a line spanning two gives them only to the one
/// its `since` names, and its hide to both. Lines named before and gone from
/// disk count as what the decision stored.
#[must_use]
pub(crate) fn decide(
    mut base: Record,
    lines: &[&SenseLine],
    carries_fields: &dyn Fn(&SenseLine) -> bool,
    primary: &str,
    now: &str,
) -> Record {
    let stored = base
        .get(CONVERTED)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let primary = stored
        .get("primary")
        .and_then(Value::as_str)
        .unwrap_or(primary)
        .to_owned();
    let decided_at = stored
        .get("decided_at")
        .and_then(Value::as_str)
        .unwrap_or(now)
        .to_owned();
    // A code record that was already listed before any line was folded in.
    let record_visible = stored
        .get(RECORD_VISIBLE)
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut entries = stored
        .get("lines")
        .and_then(Value::as_object)
        .map(|lines| {
            lines
                .iter()
                .map(|(key, value)| (key.clone(), LineEntry::stored(value)))
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();

    let edits = edits_of(&base);
    let own = edits
        .iter()
        .filter(|edit| carried_of(edit).is_none())
        .cloned()
        .collect::<Vec<_>>();
    // Carried entries as they stand, by the line that carried them.
    let mut carried: BTreeMap<String, Vec<Record>> = BTreeMap::new();
    for edit in &edits {
        if let Some(key) = carried_of(edit).filter(|key| *key != DECISION) {
            carried
                .entry(key.to_owned())
                .or_default()
                .push(edit.clone());
        }
    }

    for line in lines {
        let key = line.key();
        let digest = line.digest();
        if entries
            .get(&key)
            .is_some_and(|entry| entry.digest == digest)
        {
            continue;
        }
        let removed = carried
            .get(&key)
            .into_iter()
            .flatten()
            .filter(|edit| edit_removed(edit))
            .cloned()
            .collect::<Vec<_>>();
        let new = if carries_fields(line) {
            without_removed(carried_entries(line), &removed)
        } else {
            Vec::new()
        };
        let fields = new
            .iter()
            .flat_map(|entry| {
                let timestamp = entry.get("timestamp").cloned().unwrap_or(Value::Null);
                entry
                    .get("fields")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(move |field| (field.to_owned(), timestamp.clone()))
                    .collect::<Vec<_>>()
            })
            .collect();
        let hide = owner_hide(line);
        entries.insert(
            key.clone(),
            LineEntry {
                digest,
                hidden: hide.is_some(),
                hide,
                fields,
            },
        );
        let kept = removed.into_iter().chain(new).collect::<Vec<_>>();
        if kept.is_empty() {
            carried.remove(&key);
        } else {
            carried.insert(key, kept);
        }
    }

    // Lowest precedence first: other lines by time and key, the primary last,
    // so the store's undo and the value here land on the same one.
    let mut ordered = carried
        .iter()
        .filter(|(key, _)| **key != primary)
        .flat_map(|(key, group)| {
            group.iter().map(move |edit| {
                (
                    text(edit, "timestamp").to_owned(),
                    key.clone(),
                    edit.clone(),
                )
            })
        })
        .collect::<Vec<_>>();
    ordered.sort_by(|left, right| (&left.0, &left.1).cmp(&(&right.0, &right.1)));
    let mut carried_order = ordered
        .into_iter()
        .map(|(_, _, edit)| edit)
        .collect::<Vec<_>>();
    if let Some(group) = carried.get(&primary) {
        carried_order.extend(group.iter().cloned());
    }

    for field in CARRIED_FIELDS {
        let own_held = own
            .iter()
            .any(|edit| owner_edit(edit) && !edit_removed(edit) && edit_names(edit, field));
        if own_held {
            continue;
        }
        let value = carried_order
            .iter()
            .rev()
            .filter(|edit| !edit_removed(edit))
            .find_map(|edit| edit.get("set").and_then(|set| set.get(field)));
        match value {
            Some(value) => {
                base.insert(field.to_owned(), value.clone());
            }
            None => {
                if carried_order.iter().any(|edit| edit_names(edit, field)) {
                    base.remove(field);
                }
            }
        }
    }

    let own_hide_history = own.iter().any(|edit| edit_names(edit, HIDDEN));
    let decision = if own_hide_history {
        None
    } else {
        // A hide written after the decision was first made counts as visible:
        // in that window an owner is most likely hiding the duplicate.
        let counted = |entry: &LineEntry| {
            entry.hidden
                && entry
                    .hide
                    .as_ref()
                    .is_none_or(|hide| hide.timestamp <= decided_at)
        };
        let all_hidden = !record_visible && !entries.is_empty() && entries.values().all(counted);
        let deciding = entries
            .iter()
            .filter(|(_, entry)| counted(entry))
            .filter_map(|(key, entry)| entry.hide.clone().map(|hide| (hide, key.clone())))
            .max_by(|(left, left_key), (right, right_key)| {
                (&left.timestamp, left_key).cmp(&(&right.timestamp, right_key))
            })
            .map(|(hide, _)| hide);
        base.insert(HIDDEN.to_owned(), Value::Bool(all_hidden));
        let (timestamp, actor, note) = match deciding {
            Some(hide) if all_hidden => (hide.timestamp, hide.actor, hide.note),
            _ => (decided_at.clone(), "convert".to_owned(), String::new()),
        };
        Some(json!({
            "timestamp": timestamp,
            "actor": actor,
            "fields": [HIDDEN],
            "note": note,
            "carried": DECISION,
        }))
    };

    let mut rebuilt = carried_order
        .into_iter()
        .map(Value::Object)
        .collect::<Vec<_>>();
    rebuilt.extend(decision);
    rebuilt.extend(own.into_iter().map(Value::Object));
    if !rebuilt.is_empty() {
        base.insert(EDITS.to_owned(), Value::Array(rebuilt));
    }
    let mut converted = json!({
        "primary": primary,
        "decided_at": decided_at,
        "lines": entries
            .iter()
            .map(|(key, entry)| (key.clone(), entry.to_value()))
            .collect::<Map<_, _>>(),
    });
    if record_visible {
        converted[RECORD_VISIBLE] = Value::Bool(true);
    }
    base.insert(CONVERTED.to_owned(), converted);
    base
}

#[cfg(test)]
#[path = "strava_decision_tests.rs"]
mod tests;
