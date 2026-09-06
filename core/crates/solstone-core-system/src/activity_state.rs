// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Deterministic, read-only activity state machine.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use serde_json::{Map, Value, json};
use solstone_core_format::segment::segment_start_and_end_seconds;

pub const GAP_THRESHOLD_SECONDS: i64 = 600;
pub const END_HYSTERESIS_SEGMENTS: usize = 2;

/// Read facets recorded by the facets classifier for one day.
///
/// This is deliberately independent of `activity_state.json`: Python's
/// `get_active_facets(day)` scans each segment's `talents/facets.json`, so a
/// previous day's durable activity state must not make a facet active today.
pub fn active_facets(journal: &Path, day: &str) -> BTreeSet<String> {
    let day_dir = journal.join("chronicle").join(day);
    let Ok(entries) = fs::read_dir(day_dir) else {
        return BTreeSet::new();
    };
    let mut active = BTreeSet::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        collect_segment_facets(&path, &mut active);
        if path.join("talents").is_dir() {
            continue;
        }
        if let Ok(stream_entries) = fs::read_dir(path) {
            for segment in stream_entries.flatten() {
                if segment.path().is_dir() {
                    collect_segment_facets(&segment.path(), &mut active);
                }
            }
        }
    }
    active
}

fn collect_segment_facets(segment: &Path, active: &mut BTreeSet<String>) {
    let path = segment.join("talents/facets.json");
    let Ok(bytes) = fs::read(path) else {
        return;
    };
    let Ok(Value::Array(rows)) = serde_json::from_slice::<Value>(&bytes) else {
        return;
    };
    active.extend(rows.into_iter().filter_map(|row| {
        row.get("facet")
            .and_then(Value::as_str)
            .filter(|facet| !facet.is_empty())
            .map(ToOwned::to_owned)
    }));
}

pub fn make_activity_id(activity_type: &str, since_segment: &str) -> String {
    format!("{activity_type}_{since_segment}")
}

/// Subject phrases the activity talent may still prefix a description with.
///
/// Longest first; none is a prefix of another, so the order is documentation
/// rather than a correctness requirement.
const LEADING_SUBJECTS: [&str; 6] = [
    "This person",
    "The person",
    "The owner",
    "The user",
    "You",
    "I",
];

/// Remove a leading subject phrase from a talent-written activity description.
///
/// `sense.md` asks for a plain statement of what was done with no subject
/// ("Debugged the retry handling"), because the string is rendered verbatim in
/// the owner's recent activity list. A small local model slips back into "The
/// user navigated between…" often enough that the prompt alone does not hold,
/// so this is the deterministic guard at the one boundary where talent output
/// becomes an activity record.
///
/// It strips a leading subject and capitalises the verb behind it. Everything
/// else is returned exactly as written: a description that does not open with a
/// bare subject phrase is never rewritten, a contraction ("I'm still waiting")
/// is not a subject phrase, and stored records are never revisited — only the
/// records written after this point are normalised.
pub fn normalize_activity_description(description: &str) -> String {
    for subject in LEADING_SUBJECTS {
        let Some(rest) = strip_subject_phrase(description, subject) else {
            continue;
        };
        let mut characters = rest.chars();
        let Some(first) = characters.next() else {
            continue;
        };
        if !first.is_alphabetic() {
            continue;
        }
        return first.to_uppercase().collect::<String>() + characters.as_str();
    }
    description.to_owned()
}

/// Return the text after a leading `subject` word, or `None` when `text` does
/// not open with that subject followed by whitespace.
fn strip_subject_phrase<'a>(text: &'a str, subject: &str) -> Option<&'a str> {
    if !text.get(..subject.len())?.eq_ignore_ascii_case(subject) {
        return None;
    }
    let rest = text.get(subject.len()..)?;
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    Some(rest.trim_start())
}

pub fn level_value(level: &str) -> f64 {
    match level {
        "high" => 1.0,
        "low" => 0.25,
        _ => 0.5,
    }
}

#[derive(Debug, Default)]
pub struct ActivityStateMachine {
    state: BTreeMap<String, Map<String, Value>>,
    last_segment_key: Option<String>,
    last_segment_day: Option<String>,
    completed: Vec<Value>,
}

impl ActivityStateMachine {
    /// Hydrate either historical shape. This never writes activity_state.json.
    pub fn hydrate(journal: Option<&Path>) -> Self {
        let Some(journal) = journal else {
            return Self::default();
        };
        let Ok(bytes) = fs::read(journal.join("awareness/activity_state.json")) else {
            return Self::default();
        };
        let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
            return Self::default();
        };
        let mut machine = Self::default();
        let active = if let Some(list) = value.as_array() {
            list.iter()
                .filter_map(|entry| {
                    entry.as_object().and_then(|entry| {
                        entry
                            .get("facet")
                            .and_then(Value::as_str)
                            .map(|facet| (facet.to_owned(), entry.clone()))
                    })
                })
                .collect()
        } else if let Some(object) = value.as_object() {
            machine.last_segment_key = object
                .get("last_segment_key")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
            machine.last_segment_day = object
                .get("last_segment_day")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
            object
                .get("active")
                .and_then(Value::as_object)
                .map(|active| {
                    active
                        .iter()
                        .filter_map(|(facet, value)| {
                            value
                                .as_object()
                                .map(|entry| (facet.clone(), entry.clone()))
                        })
                        .collect()
                })
                .unwrap_or_default()
        } else {
            BTreeMap::new()
        };
        for (facet, mut entry) in active {
            if ["id", "activity", "since", "description"]
                .iter()
                .all(|key| entry.contains_key(*key))
            {
                entry
                    .entry("facet".to_owned())
                    .or_insert_with(|| json!(facet));
                let since = entry.get("since").cloned().unwrap_or(Value::Null);
                entry
                    .entry("segment".to_owned())
                    .or_insert_with(|| since.clone());
                entry
                    .entry("segments".to_owned())
                    .or_insert_with(|| json!([since]));
                machine.state.insert(facet, entry);
            }
        }
        machine
    }

    pub fn should_reset(&self, segment: &str, day: &str, previous: Option<&str>) -> bool {
        if self.last_segment_day.as_deref().is_none() {
            return false;
        }
        if self.last_segment_day.as_deref() != Some(day) {
            return true;
        }
        let Some(previous) = previous.or(self.last_segment_key.as_deref()) else {
            return false;
        };
        let Some((_previous_start, previous_end)) = segment_start_and_end_seconds(previous) else {
            return false;
        };
        let Some((current_start, _)) = segment_start_and_end_seconds(segment) else {
            return false;
        };
        let current = i64::from(current_start.hour) * 3600
            + i64::from(current_start.minute) * 60
            + i64::from(current_start.second);
        current - i64::try_from(previous_end).unwrap_or(i64::MAX) > GAP_THRESHOLD_SECONDS
    }

    pub fn update(
        &mut self,
        sense: &Value,
        segment: &str,
        day: &str,
        previous: Option<&str>,
        created_at: i64,
    ) -> Vec<Value> {
        let mut changes = Vec::new();
        if self.should_reset(segment, day, previous) {
            changes.extend(self.end_all(segment, "ended_gap", created_at));
        }
        let density = sense
            .get("density")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let content = sense
            .get("content_type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if density == "idle" {
            changes.extend(self.end_all(segment, "ended_idle", created_at));
            self.last_segment_key = Some(segment.to_owned());
            self.last_segment_day = Some(day.to_owned());
            return changes;
        }
        let summary = sense
            .get("activity_summary")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let entities = sense
            .get("entities")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(|entry| {
                        entry.get("name").and_then(Value::as_str).map(str::to_owned)
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let facets = sense
            .get("facets")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(|entry| {
                        let object = entry.as_object()?;
                        let facet = object.get("facet")?.as_str()?;
                        (object.get("level").and_then(Value::as_str) != Some("low"))
                            .then_some((facet.to_owned(), object.clone()))
                    })
                    .collect::<BTreeMap<_, _>>()
            })
            .unwrap_or_default();
        let inactive = self
            .state
            .keys()
            .filter(|facet| !facets.contains_key(*facet))
            .cloned()
            .collect::<Vec<_>>();
        for facet in inactive {
            let prior = self.state.get_mut(&facet).expect("collected key remains");
            let misses = prior
                .get("_pending_facet_misses")
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize
                + 1;
            if misses >= END_HYSTERESIS_SEGMENTS {
                let prior = self.state.remove(&facet).expect("entry remains");
                changes.push(ended(&prior, &facet, segment, "ended_facet_gone"));
                self.completed.push(completed(&prior, created_at));
            } else {
                prior.insert("_pending_facet_misses".to_owned(), json!(misses));
                prior.insert("_change".to_owned(), json!("facet_gone_pending"));
                append_segment(prior, segment);
                changes.push(Value::Object(prior.clone()));
            }
        }
        for (facet, data) in facets {
            let level = match data.get("level").and_then(Value::as_str) {
                Some("high" | "medium" | "low") => {
                    data.get("level").and_then(Value::as_str).unwrap()
                }
                _ => "medium",
            };
            let description = normalize_activity_description(
                data.get("activity")
                    .and_then(Value::as_str)
                    .unwrap_or(summary),
            );
            if let Some(prior) = self.state.get_mut(&facet) {
                prior.insert("_pending_facet_misses".to_owned(), json!(0));
                if prior.get("activity").and_then(Value::as_str) != Some(content) {
                    let same = prior.get("_pending_type").and_then(Value::as_str) == Some(content);
                    let count = if same {
                        prior
                            .get("_pending_type_count")
                            .and_then(Value::as_u64)
                            .unwrap_or(0) as usize
                            + 1
                    } else {
                        1
                    };
                    if count >= END_HYSTERESIS_SEGMENTS {
                        let old = self.state.remove(&facet).expect("entry remains");
                        changes.push(ended(&old, &facet, segment, "ended_type_change"));
                        self.completed.push(completed(&old, created_at));
                        let entry =
                            active(content, segment, &description, level, &entities, &facet);
                        self.state.insert(facet, entry.clone());
                        changes.push(Value::Object(entry));
                    } else {
                        prior.insert("_pending_type".to_owned(), json!(content));
                        prior.insert("_pending_type_count".to_owned(), json!(count));
                        prior.insert("_change".to_owned(), json!("type_change_pending"));
                        prior.insert("level".to_owned(), json!(level));
                        prior.insert("active_entities".to_owned(), json!(entities));
                        append_segment(prior, segment);
                        changes.push(Value::Object(prior.clone()));
                    }
                } else {
                    prior.insert("description".to_owned(), json!(description));
                    prior.insert("level".to_owned(), json!(level));
                    prior.insert("active_entities".to_owned(), json!(entities));
                    prior.insert("_pending_type".to_owned(), Value::Null);
                    prior.insert("_pending_type_count".to_owned(), json!(0));
                    prior.insert("_change".to_owned(), json!("continuing"));
                    append_segment(prior, segment);
                    changes.push(Value::Object(prior.clone()));
                }
            } else {
                let entry = active(content, segment, &description, level, &entities, &facet);
                self.state.insert(facet, entry.clone());
                changes.push(Value::Object(entry));
            }
        }
        self.last_segment_key = Some(segment.to_owned());
        self.last_segment_day = Some(day.to_owned());
        changes
    }

    pub fn close_active(&mut self, segment: &str, created_at: i64) -> Vec<Value> {
        self.end_all(segment, "ended_day_end", created_at)
    }
    pub fn completed_activities(&self) -> Vec<Value> {
        self.completed.clone()
    }

    /// The day associated with the last processed segment, if any.
    ///
    /// The think orchestrator reads this before calling [`Self::update`] so a
    /// completion caused by the first segment of a new day remains routed to
    /// the day where its activity began.
    pub fn last_segment_day(&self) -> Option<&str> {
        self.last_segment_day.as_deref()
    }

    /// The last processed segment key, used to close a finite replay stream.
    pub fn last_segment_key(&self) -> Option<&str> {
        self.last_segment_key.as_deref()
    }

    /// Return the on-disk representation; the think orchestrator owns writing
    /// this domain state, while this system helper remains deterministic.
    pub fn snapshot(&self) -> Value {
        Value::Object(Map::from_iter([
            (
                "last_segment_key".to_owned(),
                self.last_segment_key
                    .clone()
                    .map_or(Value::Null, Value::String),
            ),
            (
                "last_segment_day".to_owned(),
                self.last_segment_day
                    .clone()
                    .map_or(Value::Null, Value::String),
            ),
            (
                "active".to_owned(),
                Value::Object(
                    self.state
                        .iter()
                        .map(|(facet, entry)| {
                            let mut entry = entry.clone();
                            entry.remove("_change");
                            (facet.clone(), Value::Object(entry))
                        })
                        .collect(),
                ),
            ),
        ]))
    }
    fn end_all(&mut self, segment: &str, change: &str, created_at: i64) -> Vec<Value> {
        let active = std::mem::take(&mut self.state);
        active
            .into_iter()
            .map(|(facet, entry)| {
                self.completed.push(completed(&entry, created_at));
                ended(&entry, &facet, segment, change)
            })
            .collect()
    }
}

fn active(
    activity: &str,
    segment: &str,
    description: &str,
    level: &str,
    entities: &[String],
    facet: &str,
) -> Map<String, Value> {
    Map::from_iter([
        ("id".to_owned(), json!(make_activity_id(activity, segment))),
        ("activity".to_owned(), json!(activity)),
        ("state".to_owned(), json!("active")),
        ("since".to_owned(), json!(segment)),
        ("description".to_owned(), json!(description)),
        ("level".to_owned(), json!(level)),
        ("active_entities".to_owned(), json!(entities)),
        ("_change".to_owned(), json!("new")),
        ("facet".to_owned(), json!(facet)),
        ("segment".to_owned(), json!(segment)),
        ("segments".to_owned(), json!([segment])),
    ])
}
fn ended(prior: &Map<String, Value>, facet: &str, segment: &str, change: &str) -> Value {
    json!({"id": prior.get("id"), "activity": prior.get("activity"), "state": "ended", "since": prior.get("since"), "description": prior.get("description"), "_change": change, "facet": facet, "segment": segment})
}
fn completed(entry: &Map<String, Value>, created_at: i64) -> Value {
    json!({"id": entry.get("id"), "facet": entry.get("facet"), "activity": entry.get("activity"), "segments": entry.get("segments").cloned().unwrap_or_else(|| json!([entry.get("since")])), "level_avg": level_value(entry.get("level").and_then(Value::as_str).unwrap_or("medium")), "description": entry.get("description"), "active_entities": entry.get("active_entities").cloned().unwrap_or_else(|| json!([])), "created_at": created_at})
}
fn append_segment(entry: &mut Map<String, Value>, segment: &str) {
    entry.insert("segment".to_owned(), json!(segment));
    let since = entry.get("since").cloned().unwrap_or(Value::Null);
    let segments = entry
        .entry("segments".to_owned())
        .or_insert_with(|| json!([since]));
    if let Some(values) = segments.as_array_mut()
        && !values.iter().any(|value| value.as_str() == Some(segment))
    {
        values.push(json!(segment));
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;

    use super::{
        ActivityStateMachine, END_HYSTERESIS_SEGMENTS, GAP_THRESHOLD_SECONDS, active_facets,
        level_value, make_activity_id, normalize_activity_description,
    };

    fn active(facet: &str) -> serde_json::Value {
        json!({"facet": facet, "id": "work_090000_60", "activity": "work", "since": "090000_60", "description": "work"})
    }

    fn sense(facets: serde_json::Value) -> serde_json::Value {
        json!({"density":"active","content_type":"work","activity_summary":"work","facets":facets})
    }

    #[test]
    fn hydrates_legacy_and_current_shapes_and_drops_every_incomplete_entry() {
        let root = tempfile::tempdir().unwrap();
        let awareness = root.path().join("awareness");
        fs::create_dir_all(&awareness).unwrap();
        fs::write(
            awareness.join("activity_state.json"),
            serde_json::to_vec(&json!([active("legacy")])).unwrap(),
        )
        .unwrap();
        let legacy = ActivityStateMachine::hydrate(Some(root.path()));
        assert_eq!(legacy.state.len(), 1);
        assert_eq!(legacy.state["legacy"]["segment"], "090000_60");

        let mut entries = serde_json::Map::new();
        entries.insert("complete".to_owned(), active("complete"));
        for missing in ["id", "activity", "since", "description"] {
            let mut malformed = active(missing).as_object().unwrap().clone();
            malformed.remove(missing);
            entries.insert(
                format!("missing-{missing}"),
                serde_json::Value::Object(malformed),
            );
        }
        fs::write(awareness.join("activity_state.json"), serde_json::to_vec(&json!({"active":entries,"last_segment_key":"100000_60","last_segment_day":"20260101"})).unwrap()).unwrap();
        let current = ActivityStateMachine::hydrate(Some(root.path()));
        assert_eq!(current.state.len(), 1);
        assert_eq!(current.last_segment_key.as_deref(), Some("100000_60"));
        assert_eq!(current.last_segment_day.as_deref(), Some("20260101"));
    }

    #[test]
    fn active_facets_is_day_specific_and_reads_segment_facet_outputs() {
        // Source-derived, not measured: thinking.py:614-631 scans the requested
        // day's `talents/facets.json`, never durable activity-state keys.
        let root = tempfile::tempdir().unwrap();
        let first = root.path().join("chronicle/20260101/090000_60/talents");
        let second = root
            .path()
            .join("chronicle/20260102/default/090000_60/talents");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();
        fs::write(
            first.join("facets.json"),
            serde_json::to_vec(&json!([{"facet":"work"}])).unwrap(),
        )
        .unwrap();
        fs::write(
            second.join("facets.json"),
            serde_json::to_vec(&json!([{"facet":"home"}])).unwrap(),
        )
        .unwrap();
        assert_eq!(
            active_facets(root.path(), "20260101"),
            std::collections::BTreeSet::from(["work".to_owned()])
        );
        assert_eq!(
            active_facets(root.path(), "20260102"),
            std::collections::BTreeSet::from(["home".to_owned()])
        );
    }

    #[test]
    fn close_active_completes_every_entry_and_never_persists_state() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("awareness/activity_state.json");
        let mut machine = ActivityStateMachine::hydrate(Some(root.path()));
        machine.update(
            &sense(json!([{"facet":"work"},{"facet":"home"}])),
            "090000_60",
            "20260101",
            None,
            1,
        );
        let ended = machine.close_active("090100_60", 2);
        assert_eq!(ended.len(), 2);
        assert!(
            ended
                .iter()
                .all(|entry| entry["_change"] == "ended_day_end" && entry["state"] == "ended")
        );
        assert!(machine.state.is_empty());
        assert_eq!(machine.completed_activities().len(), 2);
        assert!(!path.exists());
        let _ = machine.update(&sense(json!([])), "090200_60", "20260101", None, 3);
        assert!(!path.exists());
    }

    #[test]
    fn hysteresis_gap_and_clamped_end_boundaries_match_python() {
        assert_eq!(END_HYSTERESIS_SEGMENTS, 2);
        assert_eq!(GAP_THRESHOLD_SECONDS, 600);
        let mut machine = ActivityStateMachine::default();
        machine.update(
            &sense(json!([{"facet":"work"}])),
            "090000_60",
            "20260101",
            None,
            0,
        );
        let pending = machine.update(&sense(json!([])), "090100_60", "20260101", None, 0);
        assert_eq!(pending[0]["_change"], "facet_gone_pending");
        let ended = machine.update(&sense(json!([])), "090200_60", "20260101", None, 0);
        assert_eq!(ended[0]["_change"], "ended_facet_gone");

        machine.last_segment_day = Some("20260101".to_owned());
        assert!(!machine.should_reset("091100_60", "20260101", Some("090000_60")));
        assert!(machine.should_reset("091101_60", "20260101", Some("090000_60")));
        assert!(
            !machine.should_reset("235959_60", "20260101", Some("235000_7200")),
            "the reference clamps an overrun to 23:59:59"
        );
    }

    #[test]
    fn a_leading_subject_phrase_is_stripped_and_the_verb_capitalised() {
        for (written, shown) in [
            // X-21, verbatim from the 2026-09-06 review capture of the live
            // journal's home pulse.
            (
                "The user navigated between the personal foundation hub and a specific project workspace to manage a UX verification task.",
                "Navigated between the personal foundation hub and a specific project workspace to manage a UX verification task.",
            ),
            (
                "You reviewed personal project management tasks related to timeline history and wave burn-in issues within your own workspace.",
                "Reviewed personal project management tasks related to timeline history and wave burn-in issues within your own workspace.",
            ),
            ("I sent the invoice to Sam.", "Sent the invoice to Sam."),
            (
                "The owner debugged the retry handling.",
                "Debugged the retry handling.",
            ),
            (
                "The person opened the launch checklist.",
                "Opened the launch checklist.",
            ),
            (
                "This person wrote the release notes.",
                "Wrote the release notes.",
            ),
            ("the user  typed a status report.", "Typed a status report."),
        ] {
            assert_eq!(normalize_activity_description(written), shown, "{written}");
        }
    }

    #[test]
    fn every_other_description_is_returned_untouched() {
        for description in [
            "Resolved a technical incident involving hub daemons and updated system tracking documents.",
            "Debugged the retry handling in the ingest worker.",
            "Investigated the request timeout.",
            "The users of the beta build reported a crash.",
            "The owner's manual was open on the second monitor.",
            "I'm still waiting on the build.",
            "You're on the launch call at two.",
            "The user",
            "The user, who had opened the checklist, switched panes.",
            "",
        ] {
            assert_eq!(
                normalize_activity_description(description),
                description,
                "{description}"
            );
        }
    }

    #[test]
    fn update_writes_a_subjectless_description_onto_new_and_continuing_records() {
        let mut machine = ActivityStateMachine::default();
        let opened = machine.update(
            &json!({"density":"active","content_type":"terminal","activity_summary":"The user typed a status report.","facets":[{"facet":"work","level":"high","activity":"The user navigated between the personal foundation hub and a specific project workspace to manage a UX verification task."}]}),
            "100558_300",
            "20260906",
            None,
            1,
        );
        assert_eq!(opened.len(), 1);
        assert_eq!(opened[0]["_change"], "new");
        assert_eq!(
            opened[0]["description"],
            "Navigated between the personal foundation hub and a specific project workspace to manage a UX verification task."
        );

        let continued = machine.update(
            &json!({"density":"active","content_type":"terminal","activity_summary":"The user typed a status report.","facets":[{"facet":"work","level":"high","activity":"I refined the sense prompt."}]}),
            "101058_300",
            "20260906",
            None,
            1,
        );
        assert_eq!(continued[0]["_change"], "continuing");
        assert_eq!(continued[0]["description"], "Refined the sense prompt.");

        let fallback = machine.update(
            &json!({"density":"active","content_type":"terminal","activity_summary":"The user typed a status report.","facets":[{"facet":"personal","level":"high"}]}),
            "101558_304",
            "20260906",
            None,
            1,
        );
        let opened_personal = fallback
            .iter()
            .find(|change| change["facet"] == "personal")
            .expect("the personal facet opened an activity");
        assert_eq!(opened_personal["description"], "Typed a status report.");
    }

    #[test]
    fn ids_and_levels_match_activity_helpers() {
        assert_eq!(
            make_activity_id("meeting", "090000_60"),
            "meeting_090000_60"
        );
        assert_eq!(level_value("high"), 1.0);
        assert_eq!(level_value("medium"), 0.5);
        assert_eq!(level_value("low"), 0.25);
    }
}
