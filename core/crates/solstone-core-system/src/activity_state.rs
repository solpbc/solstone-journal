// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Deterministic, read-only activity state machine.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use solstone_core_format::segment::segment_start_and_end_seconds;

pub const GAP_THRESHOLD_SECONDS: i64 = 600;
pub const END_HYSTERESIS_SEGMENTS: usize = 2;
/// The segments an open activity holds while its facet is missing from them.
const PENDING_MISS_SEGMENTS: &str = "_pending_miss_segments";
/// The segments buffered while waiting to see whether an activity-type switch holds.
const PENDING_TYPE_SEGMENTS: &str = "_pending_type_segments";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct PendingTypeSegment {
    segment: String,
    activity: String,
    description: String,
    level: String,
    #[serde(default)]
    active_entities: Vec<String>,
}

fn is_idle_sense(sense: &Value) -> bool {
    sense.get("density").and_then(Value::as_str) == Some("idle")
        || sense.get("content_type").and_then(Value::as_str) == Some("idle")
}

fn is_storyless_activity(kind: &str) -> bool {
    matches!(kind, "video" | "media" | "music" | "gaming")
}

fn is_story_bearing_activity(kind: &str) -> bool {
    matches!(
        kind,
        "meeting"
            | "messaging"
            | "email"
            | "social"
            | "coding"
            | "browsing"
            | "reading"
            | "terminal"
            | "ai_conversation"
            | "writing"
            | "planning"
            | "design"
            | "productivity"
    )
}

/// The snapshot every stream shared before activity state was kept per stream.
/// Only read, once, to carry the activities it holds over to one stream.
pub const SHARED_ACTIVITY_STATE: &str = "awareness/activity_state.json";

/// Where one stream's live activity state is kept.
///
/// Each stream has its own machine: a segment from one stream (a phone's
/// location segment, a watch's audio) must never end, extend or measure a gap
/// against another stream's activities. A separate file per stream also keeps
/// a flush on one stream from overwriting a segment run on another. Segments
/// filed directly under the day use the journal's name for that layout.
pub fn activity_state_path(journal: &Path, stream: Option<&str>) -> PathBuf {
    let stream = stream.unwrap_or(solstone_core_journal_io::paths::DEFAULT_STREAM);
    journal
        .join("awareness/activity_state")
        .join(format!("{stream}.json"))
}

/// Read facets recorded by the facets classifier for one day.
///
/// This is deliberately independent of `activity_state.json`: a day's active
/// facets come only from that day's segments' `talents/facets.json`, so a
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
            let stream = entry.file_name();
            for segment in stream_entries.flatten() {
                if segment.path().is_dir()
                    && !stream.to_str().is_some_and(|stream| {
                        solstone_core_segment::is_body_only_dir(stream, &segment.path())
                    })
                    && !solstone_core_segment::is_activity_only_folder_dir(&segment.path())
                {
                    collect_segment_facets(&segment.path(), &mut active);
                }
            }
        }
    }
    with_live_facets(journal, active)
}

/// Add, for every recorded facet name, the live facet it now belongs to: a
/// facet merged or renamed away makes the facet it became active. Names
/// already present stay, so nothing that matched before stops matching, and
/// a journal whose facets can't be read keeps exactly the recorded names.
fn with_live_facets(journal: &Path, mut active: BTreeSet<String>) -> BTreeSet<String> {
    let Ok(facets) =
        solstone_core_indexer_store::classification::FacetDeclarationSet::from_journal(journal)
    else {
        return active;
    };
    let live: Vec<String> = active
        .iter()
        .filter_map(|name| facets.live_name_for_reference(name))
        .collect();
    active.extend(live);
    active
}

/// Coverage admission must distinguish absent classification from unreadable classification.
pub fn active_facets_checked(journal: &Path, day: &str) -> Result<BTreeSet<String>, String> {
    active_facets_checked_in(journal, day, solstone_core_cli_copy::ENGLISH)
}
pub(crate) fn active_facets_checked_in(
    journal: &Path,
    day: &str,
    locale: &str,
) -> Result<BTreeSet<String>, String> {
    active_facets_with_non_historical_checked_in(journal, day, locale).map(|(all, _)| all)
}

/// Returns `(all_active_facets, non_historical_active_facets)` for `day`.
/// A facet in `all_active_facets` but not `non_historical_active_facets` was
/// activated on `day` exclusively by segments that arrived as historical
/// backfill.
pub(crate) fn active_facets_with_non_historical_checked_in(
    journal: &Path,
    day: &str,
    locale: &str,
) -> Result<(BTreeSet<String>, BTreeSet<String>), String> {
    use solstone_core_journal_io::durability::{ArtifactId, DurableRead, read_json_durable};
    let segments = solstone_core_journal_io::iter_segments(
        journal,
        solstone_core_journal_io::PathOrDay::Day(day),
    )
    .map_err(|e| e.to_string())?;
    let mut all_facets = BTreeSet::new();
    let mut non_historical_facets = BTreeSet::new();
    for segment in segments {
        // An import's chosen facet files body records, and a folder's chosen
        // facet scopes activity-only folder segments; neither owes a facet's
        // daily talent work.
        if solstone_core_segment::is_body_only_segment(&segment)
            || solstone_core_segment::is_activity_only_folder_segment(&segment)
        {
            continue;
        }
        let is_historical = segment
            .stream()
            .directory()
            .and_then(|s| s.to_str())
            .is_some_and(|stream| {
                solstone_core_segment::stream_arrived_as_history_on_day(journal, stream, day)
            });
        let path = segment.path().join("talents/facets.json");
        let rows: Vec<Value> =
            match read_json_durable::<Vec<Value>>(ArtifactId::SegmentFacets, &path) {
                Ok(DurableRead::Present(rows)) => rows,
                Ok(DurableRead::Absent | DurableRead::SetAside(_)) => continue,
                Ok(DurableRead::Unreadable { path: p, error }) => {
                    return Err(format!("{}: {error}", p.display()));
                }
                Err(e) => return Err(format!("{}: {e}", path.display())),
            };
        for row in rows {
            let facet = row
                .get("facet")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    solstone_core_cli_copy::display_for(
                        locale,
                        "journal.cli.reprocess.facet.classification",
                        [("path", &path.display())],
                    )
                })?;
            all_facets.insert(facet.to_owned());
            if !is_historical {
                non_historical_facets.insert(facet.to_owned());
            }
        }
    }
    Ok((
        with_live_facets(journal, all_facets),
        with_live_facets(journal, non_historical_facets),
    ))
}

/// Whether `facet` (or all active facets when `None`) on `day` came solely
/// from segments that arrived as historical backfill.
pub(crate) fn facet_is_historical_backfill_only(
    journal: &Path,
    day: &str,
    facet: Option<&str>,
) -> bool {
    let Ok((all, non_historical)) =
        active_facets_with_non_historical_checked_in(journal, day, solstone_core_cli_copy::ENGLISH)
    else {
        return false;
    };
    match facet {
        Some(f) => all.contains(f) && !non_historical.contains(f),
        None => !all.is_empty() && non_historical.is_empty(),
    }
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

/// Auxiliary verbs the activity talent leaves between the subject and the verb.
///
/// The talent writes the present progressive often enough that stripping the
/// subject alone leaves "is performing technical maintenance…" — a fragment,
/// not a statement. Two-word forms are listed first so a one-word entry never
/// shadows them.
const LEADING_AUXILIARIES: [&str; 6] = ["has been", "have been", "is", "are", "was", "were"];

/// Remove a leading subject phrase from a talent-written activity description.
///
/// `sense.md` asks for a plain statement of what was done that opens with a
/// past-tense verb and no subject, because the string is rendered verbatim in
/// the owner's recent activity list. A small local model slips back into "The
/// user navigated between…" often enough that the prompt alone does not hold,
/// so this is the deterministic guard at the one boundary where talent output
/// becomes an activity record.
///
/// It strips a leading subject, drops an auxiliary verb ("is", "are", "was",
/// "were", "has been", "have been") left stranded in front of the real verb,
/// and capitalises what remains. Everything
/// else is returned exactly as written: a description that does not open with a
/// bare subject phrase is never rewritten, a contraction ("I'm still waiting")
/// is not a subject phrase, and stored records are never revisited — only the
/// records written after this point are normalised.
pub fn normalize_activity_description(description: &str) -> String {
    for subject in LEADING_SUBJECTS {
        let Some(rest) = strip_leading_phrase(description, subject) else {
            continue;
        };
        let rest = strip_leading_auxiliary(rest).unwrap_or(rest);
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

/// Return the text after a leading `phrase`, or `None` when `text` does not
/// open with that phrase followed by whitespace.
fn strip_leading_phrase<'a>(text: &'a str, phrase: &str) -> Option<&'a str> {
    if !text.get(..phrase.len())?.eq_ignore_ascii_case(phrase) {
        return None;
    }
    let rest = text.get(phrase.len()..)?;
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    // A title keeps its capitals: "I Am Legend finished downloading." opens with
    // a real subject phrase but the words after it are part of the name, not a
    // predicate. Every phrase this normaliser is meant to strip is followed by a
    // lowercase word, so requiring one leaves titles alone.
    Some(rest.trim_start()).filter(|rest| rest.starts_with(char::is_lowercase))
}

/// Return the text after a leading auxiliary verb, or `None` when the text
/// does not open with one that leaves a word behind it. "The user island" is
/// not an auxiliary, and dropping one that leaves nothing readable would turn
/// a description into an empty string.
fn strip_leading_auxiliary(text: &str) -> Option<&str> {
    LEADING_AUXILIARIES
        .iter()
        .find_map(|auxiliary| strip_leading_phrase(text, auxiliary))
        .filter(|rest| rest.starts_with(char::is_alphabetic))
}

pub fn level_value(level: &str) -> f64 {
    match level {
        "high" => 1.0,
        "low" => 0.25,
        _ => 0.5,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoalesceScope {
    /// Continuous live or spoken-audio stream: coalesces across segments within
    /// `GAP_THRESHOLD_SECONDS` with `END_HYSTERESIS_SEGMENTS`.
    ContinuousStream,
    /// Standalone point-in-time item (single email, photo, screenshot, vault
    /// note, or document): never merges into a preceding or following item.
    DiscreteItem,
}

#[derive(Debug, Default)]
pub struct ActivityStateMachine {
    state: BTreeMap<String, Map<String, Value>>,
    last_segment_key: Option<String>,
    last_segment_day: Option<String>,
    last_scope: Option<CoalesceScope>,
    completed: Vec<Value>,
    adopted_shared: bool,
}

impl ActivityStateMachine {
    /// Hydrate one stream's machine. This never writes a snapshot.
    ///
    /// A stream with no snapshot of its own takes over the shared snapshot
    /// an earlier version kept, so the activities open when the journal was
    /// updated are still ended and recorded; [`Self::adopted_shared`] tells
    /// the writer to remove it, so only one stream takes it over.
    pub fn hydrate(journal: Option<&Path>, stream: Option<&str>) -> Self {
        use solstone_core_journal_io::durability::{ArtifactId, DurableRead, read_json_durable};
        let Some(journal) = journal else {
            return Self::default();
        };
        let own = activity_state_path(journal, stream);
        match read_json_durable::<Value>(ArtifactId::StreamActivityState, &own) {
            Ok(DurableRead::Present(value)) => return Self::from_snapshot(&value),
            Ok(DurableRead::Absent) => {}
            _ => return Self::default(),
        }
        let shared = journal.join(SHARED_ACTIVITY_STATE);
        match read_json_durable::<Value>(ArtifactId::ActivityState, &shared) {
            Ok(DurableRead::Present(value)) => Self {
                adopted_shared: true,
                ..Self::from_snapshot(&value)
            },
            _ => Self::default(),
        }
    }

    /// Whether this machine took over the shared snapshot, which its writer
    /// removes once this stream's own snapshot is written.
    pub fn adopted_shared(&self) -> bool {
        self.adopted_shared
    }

    /// Read either historical snapshot shape.
    fn from_snapshot(value: &Value) -> Self {
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
        self.update_with_scope(
            sense,
            segment,
            day,
            previous,
            created_at,
            &CoalesceScope::ContinuousStream,
        )
    }

    pub fn update_with_scope(
        &mut self,
        sense: &Value,
        segment: &str,
        day: &str,
        previous: Option<&str>,
        created_at: i64,
        scope: &CoalesceScope,
    ) -> Vec<Value> {
        let mut changes = Vec::new();
        // A standalone item never shares an activity with what came before
        // it, nor with what follows.
        let scope_boundary = self.last_scope.as_ref().is_some_and(|prev| {
            *prev == CoalesceScope::DiscreteItem || *scope == CoalesceScope::DiscreteItem
        });
        if scope_boundary {
            changes.extend(self.end_all(segment, "ended_item_boundary", created_at));
        } else if self.should_reset(segment, day, previous) {
            changes.extend(self.end_all(segment, "ended_gap", created_at));
        }
        self.last_scope = Some(scope.clone());
        let content = sense
            .get("content_type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if is_idle_sense(sense) {
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
                self.finish_active_entry(
                    &facet,
                    prior,
                    segment,
                    "ended_facet_gone",
                    created_at,
                    &mut changes,
                );
            } else {
                prior.insert("_pending_facet_misses".to_owned(), json!(misses));
                prior.insert("_change".to_owned(), json!("facet_gone_pending"));
                append_segment(prior, segment);
                note_pending_miss(prior, segment);
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
                // The facet came back, so the segments it missed bridge one
                // activity and stay in it (or move into the new activity if a
                // pending type switch confirms right after the miss).
                let pending_misses = take_pending_miss_segments(prior);
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
                        let mut old = self.state.remove(&facet).expect("entry remains");
                        let pending = take_pending_type_segments(&mut old);
                        let bridged_misses = if let Some(first_pending) = pending.first() {
                            let moved = pending_misses
                                .iter()
                                .filter(|key| key.as_str() > first_pending.segment.as_str())
                                .cloned()
                                .collect::<BTreeSet<_>>();
                            if !moved.is_empty()
                                && let Some(values) =
                                    old.get_mut("segments").and_then(Value::as_array_mut)
                            {
                                values.retain(|value| {
                                    value.as_str().is_none_or(|key| !moved.contains(key))
                                });
                            }
                            moved.into_iter().collect::<Vec<_>>()
                        } else {
                            Vec::new()
                        };
                        changes.push(ended(&old, &facet, segment, "ended_type_change"));
                        self.completed.push(completed(&old, created_at));
                        let entry = active_from_pending(
                            &pending,
                            &bridged_misses,
                            Some((content, segment, &description, level, &entities)),
                            &facet,
                        );
                        self.state.insert(facet, entry.clone());
                        changes.push(Value::Object(entry));
                    } else {
                        let mut pending = if same {
                            read_pending_type_segments(prior)
                        } else {
                            let abandoned = take_pending_type_segments(prior);
                            fold_pending_type_segments(prior, &abandoned);
                            Vec::new()
                        };
                        pending.push(PendingTypeSegment {
                            segment: segment.to_owned(),
                            activity: content.to_owned(),
                            description,
                            level: level.to_owned(),
                            active_entities: entities.clone(),
                        });
                        store_pending_type_segments(prior, &pending);
                        prior.insert("_pending_type".to_owned(), json!(content));
                        prior.insert("_pending_type_count".to_owned(), json!(count));
                        prior.insert("_change".to_owned(), json!("type_change_pending"));
                        prior.insert("segment".to_owned(), json!(segment));
                        changes.push(Value::Object(prior.clone()));
                    }
                } else {
                    let pending = take_pending_type_segments(prior);
                    fold_pending_type_segments(prior, &pending);
                    prior.insert("description".to_owned(), json!(description));
                    prior.insert("level".to_owned(), json!(level));
                    merge_entities(prior, &entities);
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

    /// Add a segment that arrived after newer ones to the open activities it
    /// belongs to: same day, started before it, and named by its Sense facets.
    /// Nothing ends and the stream's last segment stays where it is. Returns
    /// whether any open activity took it.
    pub fn join_late(&mut self, sense: &Value, segment: &str, day: &str) -> bool {
        if self.last_segment_day.as_deref() != Some(day) || is_idle_sense(sense) {
            return false;
        }
        let facets = sense
            .get("facets")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_object)
            .filter(|entry| entry.get("level").and_then(Value::as_str) != Some("low"))
            .filter_map(|entry| entry.get("facet").and_then(Value::as_str))
            .collect::<BTreeSet<_>>();
        let mut joined = false;
        for (facet, entry) in &mut self.state {
            let started_before = entry
                .get("since")
                .and_then(Value::as_str)
                .is_some_and(|since| since < segment);
            if !started_before || !facets.contains(facet.as_str()) {
                continue;
            }
            let pending = read_pending_type_segments(entry);
            if pending.iter().any(|item| item.segment == segment) {
                joined = true;
                continue;
            }
            if pending
                .first()
                .is_some_and(|first| segment >= first.segment.as_str())
            {
                continue;
            }
            insert_segment_sorted(entry, segment);
            joined = true;
        }
        joined
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
        let mut changes = Vec::new();
        for (facet, entry) in active {
            self.finish_active_entry(&facet, entry, segment, change, created_at, &mut changes);
        }
        changes
    }

    fn finish_active_entry(
        &mut self,
        facet: &str,
        mut entry: Map<String, Value>,
        closing_segment: &str,
        change: &str,
        created_at: i64,
        changes: &mut Vec<Value>,
    ) {
        let pending = take_pending_type_segments(&mut entry);
        let parent_activity = entry
            .get("activity")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if let Some(first) = pending.first()
            && is_storyless_activity(parent_activity)
            && is_story_bearing_activity(&first.activity)
        {
            changes.push(ended(&entry, facet, closing_segment, change));
            self.completed.push(completed(&entry, created_at));
            let rescued = active_from_pending(&pending, &[], None, facet);
            changes.push(ended(&rescued, facet, closing_segment, change));
            self.completed.push(completed(&rescued, created_at));
            return;
        }
        if !pending.is_empty() {
            fold_pending_type_segments(&mut entry, &pending);
        }
        changes.push(ended(&entry, facet, closing_segment, change));
        self.completed.push(completed(&entry, created_at));
    }
}

fn read_pending_type_segments(entry: &Map<String, Value>) -> Vec<PendingTypeSegment> {
    entry
        .get(PENDING_TYPE_SEGMENTS)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| serde_json::from_value::<PendingTypeSegment>(item.clone()).ok())
                .filter(|item| !item.segment.is_empty() && !item.activity.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn take_pending_type_segments(entry: &mut Map<String, Value>) -> Vec<PendingTypeSegment> {
    let pending = read_pending_type_segments(entry);
    entry.remove(PENDING_TYPE_SEGMENTS);
    entry.insert("_pending_type".to_owned(), Value::Null);
    entry.insert("_pending_type_count".to_owned(), json!(0));
    pending
}

fn store_pending_type_segments(entry: &mut Map<String, Value>, pending: &[PendingTypeSegment]) {
    if pending.is_empty() {
        entry.remove(PENDING_TYPE_SEGMENTS);
    } else if let Ok(value) = serde_json::to_value(pending) {
        entry.insert(PENDING_TYPE_SEGMENTS.to_owned(), value);
    }
}

fn fold_pending_type_segments(entry: &mut Map<String, Value>, pending: &[PendingTypeSegment]) {
    for item in pending {
        insert_segment_sorted(entry, &item.segment);
        merge_entities(entry, &item.active_entities);
        entry.insert("level".to_owned(), json!(item.level));
    }
}

fn take_pending_miss_segments(entry: &mut Map<String, Value>) -> Vec<String> {
    entry
        .remove(PENDING_MISS_SEGMENTS)
        .and_then(|value| match value {
            Value::Array(values) => Some(
                values
                    .into_iter()
                    .filter_map(|value| value.as_str().map(str::to_owned))
                    .collect(),
            ),
            _ => None,
        })
        .unwrap_or_default()
}

fn active_from_pending(
    pending: &[PendingTypeSegment],
    bridged_misses: &[String],
    current: Option<(&str, &str, &str, &str, &[String])>,
    facet: &str,
) -> Map<String, Value> {
    let Some(first) = pending.first() else {
        let (activity, segment, description, level, entities) =
            current.expect("confirmed switch without pending segments carries current segment");
        return active(activity, segment, description, level, entities, facet);
    };
    let activity = current.map_or(first.activity.as_str(), |(activity, ..)| activity);
    let mut entry = active(
        activity,
        &first.segment,
        &first.description,
        &first.level,
        &first.active_entities,
        facet,
    );
    for item in &pending[1..] {
        insert_segment_sorted(&mut entry, &item.segment);
        merge_entities(&mut entry, &item.active_entities);
        entry.insert("description".to_owned(), json!(item.description));
        entry.insert("level".to_owned(), json!(item.level));
        entry.insert("segment".to_owned(), json!(item.segment));
    }
    for missed in bridged_misses {
        insert_segment_sorted(&mut entry, missed);
    }
    if let Some((_, segment, description, level, entities)) = current {
        insert_segment_sorted(&mut entry, segment);
        merge_entities(&mut entry, entities);
        entry.insert("description".to_owned(), json!(description));
        entry.insert("level".to_owned(), json!(level));
        entry.insert("segment".to_owned(), json!(segment));
    }
    entry
}

fn insert_segment_sorted(entry: &mut Map<String, Value>, segment: &str) {
    let since = entry.get("since").cloned().unwrap_or(Value::Null);
    let segments = entry
        .entry("segments".to_owned())
        .or_insert_with(|| json!([since]));
    if let Some(values) = segments.as_array_mut()
        && !values.iter().any(|value| value.as_str() == Some(segment))
    {
        let at = values
            .iter()
            .position(|value| value.as_str().is_some_and(|key| key > segment))
            .unwrap_or(values.len());
        values.insert(at, json!(segment));
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
    // A segment the facet was missing from is held in case the facet comes
    // back. If the activity ends first, that segment was never part of it.
    let unresolved = entry
        .get(PENDING_MISS_SEGMENTS)
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();
    let segments = match entry.get("segments").and_then(Value::as_array) {
        Some(values) => Value::Array(
            values
                .iter()
                .filter(|value| value.as_str().is_none_or(|key| !unresolved.contains(key)))
                .cloned()
                .collect(),
        ),
        None => json!([entry.get("since")]),
    };
    // Keep the stored key for existing journals: this is the last tracked
    // segment's level, not an average over the activity's segments.
    json!({"id": entry.get("id"), "facet": entry.get("facet"), "activity": entry.get("activity"), "segments": segments, "level_avg": level_value(entry.get("level").and_then(Value::as_str).unwrap_or("medium")), "description": entry.get("description"), "active_entities": entry.get("active_entities").cloned().unwrap_or_else(|| json!([])), "created_at": created_at})
}
fn note_pending_miss(entry: &mut Map<String, Value>, segment: &str) {
    let missed = entry
        .entry(PENDING_MISS_SEGMENTS.to_owned())
        .or_insert_with(|| json!([]));
    if let Some(values) = missed.as_array_mut()
        && !values.iter().any(|value| value.as_str() == Some(segment))
    {
        values.push(json!(segment));
    }
}
/// An activity's entities are every name its segments' Sense listed, once
/// each (ignoring case), in the order they first came up.
fn merge_entities(entry: &mut Map<String, Value>, entities: &[String]) {
    let merged = entry
        .entry("active_entities".to_owned())
        .or_insert_with(|| json!([]));
    if !merged.is_array() {
        *merged = json!([]);
    }
    let Some(values) = merged.as_array_mut() else {
        return;
    };
    let mut seen = values
        .iter()
        .filter_map(Value::as_str)
        .map(|name| name.trim().to_lowercase())
        .collect::<BTreeSet<_>>();
    for name in entities {
        if seen.insert(name.trim().to_lowercase()) {
            values.push(json!(name));
        }
    }
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

pub use solstone_core_segment::{
    effective_activity_stream, live_segment_dir, segment_live, stream_segment_dir as segment_dir,
};

/// Fields a talent writes onto an activity record from every segment it read.
/// When a segment leaves, they no longer hold for what remains, so they are
/// dropped until the record's talents run again.
pub const TALENT_WRITTEN_FIELDS: [&str; 7] = [
    "title",
    "story",
    "relations",
    "participation",
    "participation_confidence",
    "participation_partial_input",
    "participation_said",
];

/// What [`rederive`] made of a record.
#[derive(Debug, Clone, PartialEq)]
pub enum Rederived {
    /// Every segment it names is in the journal, or it isn't the journal's to
    /// re-make (body data, a streamless record outside the default stream):
    /// leave it as it is.
    Unchanged,
    /// Re-made from the segments still in the journal.
    Changed(Map<String, Value>),
    /// Nothing it was made from is left.
    Removed,
}

/// Re-make an activity record from the segments it names that are still in
/// the journal, with no model, as the state machine builds one: a segment
/// continues the record when its Sense names the record's facet above `low`
/// with the record's activity type; the description and level come from the
/// last such segment and the entities from all of them. What talents wrote
/// from every segment is dropped (see [`TALENT_WRITTEN_FIELDS`]); identity,
/// placement, the owner's edits and any field the owner holds are kept.
///
/// A record the owner made keeps its words and loses only the gone segments.
/// Body data is never touched here: its own trash owns it.
pub fn rederive(journal: &Path, day: &str, record: &Map<String, Value>) -> Rederived {
    rederive_with(journal, day, record, false)
}

/// [`rederive`], re-making the record even when every segment it names is in
/// the journal (`force`): a recover gives a narrowed record its segments back,
/// and what was derived from fewer of them must be made again.
pub fn rederive_with(
    journal: &Path,
    day: &str,
    record: &Map<String, Value>,
    force: bool,
) -> Rederived {
    use solstone_core_format::content::{activity_field_held, is_body_record, is_strava_reading};
    if is_body_record(record) || is_strava_reading(record) {
        return Rederived::Unchanged;
    }
    let Some(stream) = effective_activity_stream(journal, day, record) else {
        return Rederived::Unchanged;
    };
    let segments = record
        .get("segments")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let live = segments
        .iter()
        .filter(|key| segment_live(journal, day, stream, key))
        .cloned()
        .collect::<Vec<_>>();
    if live.len() == segments.len() && !force {
        return Rederived::Unchanged;
    }
    let mut next = record.clone();
    next.insert("segments".to_owned(), json!(live));
    if record.get("source").and_then(Value::as_str) == Some("user") {
        return if live.len() == segments.len() {
            Rederived::Unchanged
        } else {
            Rederived::Changed(next)
        };
    }
    let facet = record
        .get("facet")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let activity = record
        .get("activity")
        .and_then(Value::as_str)
        .unwrap_or_default();
    // The owner said what kind of activity it is: no segment's Sense decides.
    let type_held = activity_field_held(record, "activity");
    // Sense names facets as they were when it ran; a facet merged or renamed
    // since is the record's under its live name.
    let declarations =
        solstone_core_indexer_store::classification::FacetDeclarationSet::from_journal(journal)
            .ok();
    let is_record_facet = |name: &str| {
        name == facet
            || declarations
                .as_ref()
                .and_then(|facets| facets.live_name_for_reference(name))
                .is_some_and(|live| live == facet)
    };
    let mut last: Option<(String, String)> = None;
    let mut unread = false;
    let mut names = Vec::<String>::new();
    let mut seen = BTreeSet::<String>::new();
    for key in &live {
        let dir = live_segment_dir(journal, day, stream, key)
            .unwrap_or_else(|| segment_dir(journal, day, stream, key));
        let sense = match solstone_core_journal_io::durability::read_json_durable::<Value>(
            solstone_core_journal_io::durability::ArtifactId::SegmentSense,
            &dir.join("talents").join("sense.json"),
        ) {
            Ok(solstone_core_journal_io::durability::DurableRead::Present(sense)) => sense,
            _ => {
                unread = true;
                continue;
            }
        };
        if is_idle_sense(&sense)
            || (!type_held && sense.get("content_type").and_then(Value::as_str) != Some(activity))
        {
            continue;
        }
        let Some(entry) = sense
            .get("facets")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_object)
            .find(|entry| {
                entry
                    .get("facet")
                    .and_then(Value::as_str)
                    .is_some_and(is_record_facet)
                    && entry.get("level").and_then(Value::as_str) != Some("low")
            })
        else {
            continue;
        };
        let summary = sense
            .get("activity_summary")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let description = normalize_activity_description(
            entry
                .get("activity")
                .and_then(Value::as_str)
                .unwrap_or(summary),
        );
        let level = entry
            .get("level")
            .and_then(Value::as_str)
            .filter(|level| *level == "high")
            .unwrap_or("medium");
        last = Some((description, level.to_owned()));
        for name in sense
            .get("entities")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.get("name").and_then(Value::as_str))
        {
            if seen.insert(name.trim().to_lowercase()) {
                names.push(name.to_owned());
            }
        }
    }
    for field in TALENT_WRITTEN_FIELDS {
        if !activity_field_held(record, field) {
            next.remove(field);
        }
    }
    let Some((description, level)) = last else {
        // A Sense it couldn't read might have continued it: it stays, with
        // only what its talents wrote dropped, never removed on a failed read.
        return if unread && !live.is_empty() {
            Rederived::Changed(next)
        } else {
            Rederived::Removed
        };
    };
    if !activity_field_held(record, "description") {
        next.insert("description".to_owned(), json!(description));
    }
    next.insert("level_avg".to_owned(), json!(level_value(&level)));
    next.insert("active_entities".to_owned(), json!(names));
    Rederived::Changed(next)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;

    use super::{
        ActivityStateMachine, END_HYSTERESIS_SEGMENTS, GAP_THRESHOLD_SECONDS, active_facets,
        active_facets_checked, level_value, make_activity_id, normalize_activity_description,
    };

    fn active(facet: &str) -> serde_json::Value {
        json!({"facet": facet, "id": "work_090000_60", "activity": "work", "since": "090000_60", "description": "work"})
    }

    fn sense(facets: serde_json::Value) -> serde_json::Value {
        json!({"density":"active","content_type":"work","activity_summary":"work","facets":facets})
    }

    #[test]
    fn a_chosen_facet_on_body_records_activates_nothing() {
        let root = tempfile::tempdir().unwrap();
        let body = root
            .path()
            .join("chronicle/20261010/import.apple_health/063000_300");
        fs::create_dir_all(body.join("talents")).unwrap();
        fs::write(body.join("body.jsonl"), "{}\n").unwrap();
        fs::write(body.join("talents/facets.json"), r#"[{"facet":"health"}]"#).unwrap();
        assert!(active_facets(root.path(), "20261010").is_empty());
        assert!(
            active_facets_checked(root.path(), "20261010")
                .unwrap()
                .is_empty()
        );

        let recording = root.path().join("chronicle/20261010/phone/090000_300");
        fs::create_dir_all(recording.join("talents")).unwrap();
        fs::write(recording.join("audio.jsonl"), "{}\n").unwrap();
        fs::write(
            recording.join("talents/facets.json"),
            r#"[{"facet":"work"}]"#,
        )
        .unwrap();
        assert_eq!(
            active_facets(root.path(), "20261010"),
            std::collections::BTreeSet::from(["work".to_owned()])
        );
        assert_eq!(
            active_facets_checked(root.path(), "20261010").unwrap(),
            std::collections::BTreeSet::from(["work".to_owned()])
        );
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
        let legacy = ActivityStateMachine::hydrate(Some(root.path()), None);
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
        let current = ActivityStateMachine::hydrate(Some(root.path()), None);
        assert_eq!(current.state.len(), 1);
        assert_eq!(current.last_segment_key.as_deref(), Some("100000_60"));
        assert_eq!(current.last_segment_day.as_deref(), Some("20260101"));
        assert!(current.adopted_shared());
    }

    #[test]
    fn a_streams_own_snapshot_wins_over_the_shared_one() {
        let root = tempfile::tempdir().unwrap();
        let shared = root.path().join("awareness/activity_state.json");
        fs::create_dir_all(shared.parent().unwrap()).unwrap();
        fs::write(
            &shared,
            serde_json::to_vec(&json!({"active":{"shared":active("shared")}})).unwrap(),
        )
        .unwrap();
        let own = super::activity_state_path(root.path(), Some("desktop"));
        fs::create_dir_all(own.parent().unwrap()).unwrap();
        fs::write(
            &own,
            serde_json::to_vec(&json!({"active":{"own":active("own")}})).unwrap(),
        )
        .unwrap();

        let desktop = ActivityStateMachine::hydrate(Some(root.path()), Some("desktop"));
        assert_eq!(desktop.state.keys().collect::<Vec<_>>(), ["own"]);
        assert!(!desktop.adopted_shared());
        assert_eq!(
            super::activity_state_path(root.path(), None),
            root.path().join("awareness/activity_state/_default.json")
        );
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

    const TEAM: &str = "11111111-1111-4111-8111-111111111111";
    const PROJECT: &str = "22222222-2222-4222-8222-222222222222";
    const OLD: &str = "33333333-3333-4333-8333-333333333333";
    const GONE: &str = "44444444-4444-4444-8444-444444444444";
    const HOME: &str = "55555555-5555-4555-8555-555555555555";

    /// Live `team` and `home`; `project` merged into `team`; `old` renamed
    /// to `project` before that; `gone` deleted. One segment names the
    /// retired and deleted names and `home`.
    fn merged_facet_journal(retired: &str) -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        for (name, id) in [("team", TEAM), ("home", HOME)] {
            let dir = root.path().join("facets").join(name);
            fs::create_dir_all(&dir).unwrap();
            fs::write(
                dir.join("facet.json"),
                serde_json::to_vec(&json!({"id": id, "title": name})).unwrap(),
            )
            .unwrap();
        }
        fs::write(root.path().join("facets/retired.json"), retired).unwrap();
        let talents = root.path().join("chronicle/20260101/090000_60/talents");
        fs::create_dir_all(&talents).unwrap();
        fs::write(
            talents.join("facets.json"),
            serde_json::to_vec(
                &json!([{"facet":"project"},{"facet":"old"},{"facet":"gone"},{"facet":"home"}]),
            )
            .unwrap(),
        )
        .unwrap();
        root
    }

    fn names(values: &[&str]) -> std::collections::BTreeSet<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn a_merged_or_renamed_facet_makes_the_facet_it_became_active() {
        let retired = json!({"names": {
            "project": {"state": "merged", "id": PROJECT, "successor": TEAM},
            "old": {"state": "renamed", "id": OLD, "successor": PROJECT},
            "gone": {"state": "deleted", "id": GONE},
        }})
        .to_string();
        let root = merged_facet_journal(&retired);
        let expected = names(&["gone", "home", "old", "project", "team"]);
        assert_eq!(active_facets(root.path(), "20260101"), expected);
        assert_eq!(
            active_facets_checked(root.path(), "20260101").unwrap(),
            expected
        );
    }

    #[test]
    fn a_damaged_facet_record_keeps_exactly_the_recorded_names() {
        let root = merged_facet_journal("{not json");
        let recorded = names(&["gone", "home", "old", "project"]);
        assert_eq!(active_facets(root.path(), "20260101"), recorded);
        assert_eq!(
            active_facets_checked(root.path(), "20260101").unwrap(),
            recorded
        );
    }

    #[test]
    fn close_active_completes_every_entry_and_never_persists_state() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("awareness/activity_state.json");
        let mut machine = ActivityStateMachine::hydrate(Some(root.path()), None);
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
    fn an_activity_keeps_every_name_its_segments_listed() {
        let with = |names: serde_json::Value| {
            let mut s = sense(json!([{"facet":"work"}]));
            s["entities"] = names;
            s
        };
        let mut machine = ActivityStateMachine::default();
        machine.update(
            &with(json!([{"name":"Mina"},{"name":"Orchard"}])),
            "090000_60",
            "20260101",
            None,
            0,
        );
        machine.update(
            &with(json!([{"name":"mina"},{"name":"Ravi"}])),
            "090100_60",
            "20260101",
            None,
            0,
        );
        machine.update(&with(json!([])), "090200_60", "20260101", None, 0);
        machine.close_active("090300_60", 0);
        let record = machine
            .completed_activities()
            .into_iter()
            .find(|record| record["facet"] == "work")
            .expect("completed record");
        assert_eq!(
            record["active_entities"],
            json!(["Mina", "Orchard", "Ravi"])
        );
    }

    fn completed_segments(machine: &ActivityStateMachine, facet: &str) -> Vec<String> {
        let record = machine
            .completed_activities()
            .into_iter()
            .find(|record| record["facet"] == facet)
            .expect("completed record");
        serde_json::from_value(record["segments"].clone()).expect("segment keys")
    }

    #[test]
    fn a_segment_the_facet_missed_leaves_the_activity_when_it_never_returns() {
        // A phone's airport announcement (home), then its screen recording of
        // a work call: the call segment must not join the announcement.
        let mut machine = ActivityStateMachine::default();
        machine.update(
            &sense(json!([{"facet":"home"}])),
            "170012_80",
            "20261026",
            None,
            0,
        );
        machine.update(
            &sense(json!([{"facet":"work"}])),
            "170248_300",
            "20261026",
            None,
            0,
        );
        machine.update(
            &sense(json!([{"facet":"work"}])),
            "170748_300",
            "20261026",
            None,
            0,
        );
        assert_eq!(completed_segments(&machine, "home"), ["170012_80"]);

        // Ending any other way (here the day's end) drops it too.
        let mut machine = ActivityStateMachine::default();
        machine.update(
            &sense(json!([{"facet":"home"}])),
            "170012_80",
            "20261026",
            None,
            0,
        );
        machine.update(&sense(json!([])), "170248_300", "20261026", None, 0);
        machine.close_active("170248_300", 0);
        assert_eq!(completed_segments(&machine, "home"), ["170012_80"]);
    }

    #[test]
    fn a_segment_the_facet_missed_stays_when_the_facet_returns() {
        let mut machine = ActivityStateMachine::default();
        for (facets, segment) in [
            (json!([{"facet":"work"}]), "090000_60"),
            (json!([]), "090100_60"),
            (json!([{"facet":"work"}]), "090200_60"),
        ] {
            machine.update(&sense(facets), segment, "20260101", None, 0);
        }
        // The pending miss survives a snapshot round trip and then resolves.
        let mut machine = ActivityStateMachine::from_snapshot(&machine.snapshot());
        machine.close_active("090200_60", 0);
        assert_eq!(
            completed_segments(&machine, "work"),
            ["090000_60", "090100_60", "090200_60"]
        );

        let mut machine = ActivityStateMachine::default();
        machine.update(
            &sense(json!([{"facet":"work"}])),
            "090000_60",
            "20260101",
            None,
            0,
        );
        machine.update(&sense(json!([])), "090100_60", "20260101", None, 0);
        let mut machine = ActivityStateMachine::from_snapshot(&machine.snapshot());
        machine.close_active("090100_60", 0);
        assert_eq!(completed_segments(&machine, "work"), ["090000_60"]);
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
            // X-21, verbatim from the same capture: the talent writes the
            // present progressive, so the subject strip alone left a fragment.
            (
                "The user is performing technical maintenance and data synchronization within the Solstone system.",
                "Performing technical maintenance and data synchronization within the Solstone system.",
            ),
            (
                "The user is actively debugging a build submission pipeline.",
                "Actively debugging a build submission pipeline.",
            ),
            (
                "You are reviewing the launch checklist.",
                "Reviewing the launch checklist.",
            ),
            (
                "The owner was drafting the release notes.",
                "Drafting the release notes.",
            ),
            (
                "You were reading the incident report.",
                "Reading the incident report.",
            ),
            (
                "The person has been waiting on the build.",
                "Waiting on the build.",
            ),
            (
                "You have been pairing on the migration.",
                "Pairing on the migration.",
            ),
            // Known limitation: the word after the subject phrase is lowercase,
            // so the deterministic guard cannot tell this noun phrase from a
            // predicate. Distinguishing them needs a verb check this normaliser
            // does not do.
            (
                "The user manual was open on the second monitor.",
                "Manual was open on the second monitor.",
            ),
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
            // A capitalised word after the subject phrase is a title, not a
            // predicate, so the phrase is left in place.
            "I Was Here on the shelf.",
            "I Am Legend finished downloading.",
            "You Only Live Twice starts at 8.",
            "Is the build green?",
            "Was on the launch call at two.",
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
    fn a_late_segment_joins_the_open_activity_in_capture_order_without_moving_back() {
        let mut machine = ActivityStateMachine::default();
        let work = sense(json!([{"facet": "work", "level": "high"}]));
        for key in ["090000_300", "091000_300"] {
            machine.update(&work, key, "20260101", None, 1);
        }
        assert!(machine.join_late(&work, "090500_300", "20260101"));
        let snapshot = machine.snapshot();
        assert_eq!(
            snapshot["active"]["work"]["segments"],
            json!(["090000_300", "090500_300", "091000_300"])
        );
        assert_eq!(snapshot["active"]["work"]["id"], "work_090000_300");
        assert_eq!(snapshot["last_segment_key"], "091000_300");

        // Older than the activity, idle, another facet or another day: not taken.
        assert!(!machine.join_late(&work, "085500_300", "20260101"));
        assert!(!machine.join_late(
            &json!({"density": "idle", "content_type": "idle", "facets": []}),
            "090200_300",
            "20260101"
        ));
        assert!(!machine.join_late(
            &sense(json!([{"facet": "home", "level": "high"}])),
            "090200_300",
            "20260101"
        ));
        assert!(!machine.join_late(&work, "090200_300", "20251231"));
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

    fn typed_sense(
        content_type: &str,
        facet: &str,
        level: &str,
        activity: &str,
        entities: &[&str],
    ) -> serde_json::Value {
        json!({
            "density": "active",
            "content_type": content_type,
            "activity_summary": activity,
            "facets": [{"facet": facet, "level": level, "activity": activity}],
            "entities": entities.iter().map(|name| json!({"name": name})).collect::<Vec<_>>(),
        })
    }

    #[test]
    fn low_change_with_idle_content_type_ends_active_spans_and_emits_no_idle_row() {
        let mut machine = ActivityStateMachine::default();
        machine.update(
            &typed_sense("coding", "work", "high", "Edited parser.", &["Rust"]),
            "090000_300",
            "20261124",
            None,
            1,
        );
        let low_change_idle = json!({
            "density": "low_change",
            "content_type": "idle",
            "activity_summary": "Lock screen displayed unchanged.",
            "facets": [{"facet": "work", "level": "high", "activity": "Lock screen"}],
            "entities": [{"name": "Alice"}],
        });
        let ended = machine.update(&low_change_idle, "090500_300", "20261124", None, 2);
        assert_eq!(ended.len(), 1);
        assert_eq!(ended[0]["_change"], "ended_idle");
        assert!(machine.state.is_empty());

        // A second low_change + idle segment opens nothing.
        let again = machine.update(&low_change_idle, "091000_300", "20261124", None, 3);
        assert!(again.is_empty());
        assert!(machine.state.is_empty());
        assert!(
            machine
                .completed_activities()
                .iter()
                .all(|record| record["activity"] != "idle")
        );

        // join_late also rejects low_change + content_type: idle.
        let mut live = ActivityStateMachine::default();
        live.update(
            &typed_sense("coding", "work", "high", "Coded.", &[]),
            "090000_300",
            "20261124",
            None,
            1,
        );
        live.update(
            &typed_sense("coding", "work", "high", "Coded more.", &[]),
            "091000_300",
            "20261124",
            None,
            2,
        );
        assert!(!live.join_late(&low_change_idle, "090500_300", "20261124"));
    }

    #[test]
    fn confirmed_type_switch_peels_first_segment_into_new_activity_across_snapshot() {
        let mut machine = ActivityStateMachine::default();
        machine.update(
            &typed_sense("coding", "work", "high", "Wrote parser.", &["Cargo"]),
            "090000_300",
            "20261124",
            None,
            1,
        );
        // Segment t switches to meeting: buffered without contaminating coding.
        let pending = machine.update(
            &typed_sense("meeting", "work", "medium", "Joined sync.", &["Alice"]),
            "090500_300",
            "20261124",
            None,
            2,
        );
        assert_eq!(pending[0]["_change"], "type_change_pending");
        assert_eq!(pending[0]["segments"], json!(["090000_300"]));
        assert_eq!(pending[0]["active_entities"], json!(["Cargo"]));

        // Snapshot round-trip preserves buffered pending segment.
        let mut machine = ActivityStateMachine::from_snapshot(&machine.snapshot());

        // Segment t+1 confirms meeting: coding closes with [0900], meeting opens at [0905, 0910].
        let switched = machine.update(
            &typed_sense(
                "meeting",
                "work",
                "high",
                "Reviewed launch checklist.",
                &["Bob"],
            ),
            "091000_300",
            "20261124",
            None,
            3,
        );
        assert_eq!(switched.len(), 2);
        assert_eq!(switched[0]["_change"], "ended_type_change");
        assert_eq!(switched[0]["id"], "coding_090000_300");
        assert_eq!(switched[1]["_change"], "new");
        assert_eq!(switched[1]["id"], "meeting_090500_300");
        assert_eq!(switched[1]["since"], "090500_300");
        assert_eq!(switched[1]["segments"], json!(["090500_300", "091000_300"]));
        assert_eq!(switched[1]["active_entities"], json!(["Alice", "Bob"]));

        machine.close_active("091000_300", 4);
        let completed = machine.completed_activities();
        assert_eq!(completed.len(), 2);
        assert_eq!(completed[0]["id"], "coding_090000_300");
        assert_eq!(completed[0]["segments"], json!(["090000_300"]));
        assert_eq!(completed[0]["description"], "Wrote parser.");
        assert_eq!(completed[0]["level_avg"], 1.0);
        assert_eq!(completed[0]["active_entities"], json!(["Cargo"]));

        assert_eq!(completed[1]["id"], "meeting_090500_300");
        assert_eq!(
            completed[1]["segments"],
            json!(["090500_300", "091000_300"])
        );
        assert_eq!(completed[1]["description"], "Reviewed launch checklist.");
        assert_eq!(completed[1]["level_avg"], 1.0);
        assert_eq!(completed[1]["active_entities"], json!(["Alice", "Bob"]));
    }

    #[test]
    fn unconfirmed_one_segment_bounce_folds_back_without_fragmenting() {
        // A -> B -> A bounce folds B back into A.
        let mut machine = ActivityStateMachine::default();
        machine.update(
            &typed_sense("coding", "work", "high", "Edited code.", &["Rust"]),
            "090000_300",
            "20261124",
            None,
            1,
        );
        machine.update(
            &typed_sense("browsing", "work", "medium", "Checked docs.", &["MDN"]),
            "090500_300",
            "20261124",
            None,
            2,
        );
        let resumed = machine.update(
            &typed_sense("coding", "work", "high", "Finished tests.", &["Cargo"]),
            "091000_300",
            "20261124",
            None,
            3,
        );
        assert_eq!(resumed.len(), 1);
        assert_eq!(resumed[0]["_change"], "continuing");
        machine.close_active("091000_300", 4);
        let completed = machine.completed_activities();
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0]["id"], "coding_090000_300");
        assert_eq!(
            completed[0]["segments"],
            json!(["090000_300", "090500_300", "091000_300"])
        );
        assert_eq!(
            completed[0]["active_entities"],
            json!(["Rust", "MDN", "Cargo"])
        );
        assert_eq!(completed[0]["description"], "Finished tests.");

        // Storied A -> 1-seg storied B -> idle also folds B back into A (no micro-row spam).
        let mut storied = ActivityStateMachine::default();
        storied.update(
            &typed_sense("coding", "work", "high", "Edited code.", &["Rust"]),
            "090000_300",
            "20261124",
            None,
            1,
        );
        storied.update(
            &typed_sense("browsing", "work", "medium", "Checked docs.", &["MDN"]),
            "090500_300",
            "20261124",
            None,
            2,
        );
        storied.update(
            &json!({"density": "idle", "content_type": "idle", "facets": []}),
            "091000_300",
            "20261124",
            None,
            3,
        );
        let completed = storied.completed_activities();
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0]["id"], "coding_090000_300");
        assert_eq!(
            completed[0]["segments"],
            json!(["090000_300", "090500_300"])
        );
        assert_eq!(completed[0]["active_entities"], json!(["Rust", "MDN"]));
    }

    #[test]
    fn story_bearing_tail_is_rescued_when_storyless_span_closes() {
        let idle = json!({"density": "idle", "content_type": "idle", "facets": []});

        // 1. Closing on idle rescues social from media.
        let mut machine = ActivityStateMachine::default();
        machine.update(
            &typed_sense(
                "media",
                "personal",
                "medium",
                "Listened to sports broadcast.",
                &["Radio"],
            ),
            "181000_300",
            "20261124",
            None,
            1,
        );
        machine.update(
            &typed_sense(
                "media",
                "personal",
                "medium",
                "Continued sports broadcast.",
                &[],
            ),
            "181500_300",
            "20261124",
            None,
            2,
        );
        machine.update(
            &typed_sense(
                "social",
                "personal",
                "high",
                "Arranged airport pickup with Sam.",
                &["Sam"],
            ),
            "182000_300",
            "20261124",
            None,
            3,
        );
        let changes = machine.update(&idle, "182500_300", "20261124", None, 4);
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0]["id"], "media_181000_300");
        assert_eq!(changes[0]["state"], "ended");
        assert_eq!(changes[1]["id"], "social_182000_300");
        assert_eq!(changes[1]["state"], "ended");
        let completed = machine.completed_activities();
        assert_eq!(completed.len(), 2);
        assert_eq!(completed[0]["id"], "media_181000_300");
        assert_eq!(
            completed[0]["segments"],
            json!(["181000_300", "181500_300"])
        );
        assert_eq!(completed[0]["active_entities"], json!(["Radio"]));
        assert_eq!(completed[1]["id"], "social_182000_300");
        assert_eq!(completed[1]["activity"], "social");
        assert_eq!(completed[1]["segments"], json!(["182000_300"]));
        assert_eq!(completed[1]["level_avg"], 1.0);
        assert_eq!(
            completed[1]["description"],
            "Arranged airport pickup with Sam."
        );
        assert_eq!(completed[1]["active_entities"], json!(["Sam"]));

        // 2. Closing on gap, flush (close_active), and facet-drop also rescue storied tails.
        for close_mode in ["gap", "flush", "facet_drop"] {
            let mut m = ActivityStateMachine::default();
            m.update(
                &typed_sense("video", "personal", "medium", "Watched clip.", &[]),
                "100000_300",
                "20261124",
                None,
                1,
            );
            m.update(
                &typed_sense("browsing", "personal", "high", "Read article.", &["Docs"]),
                "100500_300",
                "20261124",
                None,
                2,
            );
            match close_mode {
                "gap" => {
                    m.update(
                        &typed_sense("coding", "work", "high", "Coded.", &[]),
                        "110000_300",
                        "20261124",
                        None,
                        3,
                    );
                }
                "flush" => {
                    m.close_active("100500_300", 3);
                }
                "facet_drop" => {
                    m.update(&sense(json!([])), "101000_300", "20261124", None, 3);
                    m.update(&sense(json!([])), "101500_300", "20261124", None, 4);
                }
                _ => unreachable!(),
            }
            let personal: Vec<_> = m
                .completed_activities()
                .into_iter()
                .filter(|record| record["facet"] == "personal")
                .collect();
            assert_eq!(personal.len(), 2, "{close_mode}");
            assert_eq!(personal[0]["id"], "video_100000_300", "{close_mode}");
            assert_eq!(
                personal[0]["segments"],
                json!(["100000_300"]),
                "{close_mode}"
            );
            assert_eq!(personal[1]["id"], "browsing_100500_300", "{close_mode}");
            assert_eq!(
                personal[1]["segments"],
                json!(["100500_300"]),
                "{close_mode}"
            );
        }
    }

    #[test]
    fn join_late_and_legacy_snapshots_behave_cleanly_with_pending_type_segments() {
        // join_late with pending type segment:
        // - segment before pending boundary joins parent cleanly
        // - duplicate of pending segment does not duplicate into parent
        // - segment after pending boundary returns false for capture-order replay
        let mut machine = ActivityStateMachine::default();
        let coding = typed_sense("coding", "work", "high", "Coded.", &[]);
        let meeting = typed_sense("meeting", "work", "high", "Synced.", &["Maya"]);
        machine.update(&coding, "090000_300", "20261124", None, 1);
        machine.update(&coding, "091000_300", "20261124", None, 2);
        machine.update(&meeting, "091500_300", "20261124", None, 3);
        machine.update(&sense(json!([])), "092500_300", "20261124", None, 4);

        assert!(machine.join_late(&coding, "090500_300", "20261124"));
        assert!(machine.join_late(&meeting, "091500_300", "20261124"));
        assert!(!machine.join_late(&coding, "092000_300", "20261124"));

        // Confirm meeting at 0930: parent has [0900, 0905, 0910], meeting gets [0915, 0925, 0930].
        machine.update(&meeting, "093000_300", "20261124", None, 5);
        machine.close_active("093000_300", 6);
        let completed = machine.completed_activities();
        assert_eq!(completed.len(), 2);
        assert_eq!(
            completed[0]["segments"],
            json!(["090000_300", "090500_300", "091000_300"])
        );
        assert_eq!(
            completed[1]["segments"],
            json!(["091500_300", "092500_300", "093000_300"])
        );

        // Legacy snapshot without _pending_type_segments still confirms cleanly.
        let legacy_snapshot = json!({
            "last_segment_key": "090500_300",
            "last_segment_day": "20261124",
            "active": {
                "work": {
                    "id": "coding_090000_300",
                    "activity": "coding",
                    "state": "active",
                    "since": "090000_300",
                    "description": "Coded.",
                    "level": "high",
                    "active_entities": [],
                    "facet": "work",
                    "segment": "090500_300",
                    "segments": ["090000_300", "090500_300"],
                    "_pending_type": "meeting",
                    "_pending_type_count": 1
                }
            }
        });
        let mut restored = ActivityStateMachine::from_snapshot(&legacy_snapshot);
        restored.update(&meeting, "091000_300", "20261124", None, 3);
        restored.close_active("091000_300", 4);
        let completed = restored.completed_activities();
        assert_eq!(completed.len(), 2);
        assert_eq!(
            completed[0]["segments"],
            json!(["090000_300", "090500_300"])
        );
        assert_eq!(completed[1]["id"], "meeting_091000_300");
        assert_eq!(completed[1]["segments"], json!(["091000_300"]));
    }

    mod rederive {
        use std::fs;
        use std::path::Path;

        use serde_json::{Map, Value, json};

        use super::super::{Rederived, rederive, rederive_with};

        const DAY: &str = "20261110";

        fn segment(root: &Path, key: &str, sense: Option<Value>) {
            let dir = root.join("chronicle").join(DAY).join("laptop").join(key);
            fs::create_dir_all(dir.join("talents")).unwrap();
            fs::write(dir.join("audio.flac"), b"x").unwrap();
            match sense {
                Some(sense) => fs::write(
                    dir.join("talents/sense.json"),
                    serde_json::to_vec(&sense).unwrap(),
                )
                .unwrap(),
                None => fs::write(dir.join("talents/sense.json"), b"{").unwrap(),
            }
        }

        fn sense(said: &str, who: &str) -> Value {
            json!({"density": "active", "content_type": "meeting", "activity_summary": said,
                "facets": [{"facet": "work", "level": "high", "activity": said}],
                "entities": [{"name": who}]})
        }

        /// The trash took it: its folder is the tombstone the removal left.
        fn trashed(root: &Path, key: &str) {
            let dir = root.join("chronicle").join(DAY).join("laptop").join(key);
            fs::remove_dir_all(&dir).unwrap();
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("tombstone.json"), b"{}").unwrap();
        }

        fn record(segments: &[&str]) -> Map<String, Value> {
            json!({"id": "meeting_090000_300", "facet": "work", "activity": "meeting",
                "stream": "laptop", "segments": segments,
                "title": "the secret plan", "description": "the secret plan",
                "level_avg": 1.0, "active_entities": ["Ann", "Bob", "Cat"],
                "story": {"body": "they spoke of the secret plan"},
                "relations": [{"from": "Bob", "to": "Ann", "kind": "other", "note": "secret"}],
                "participation": [{"name": "Bob"}], "participation_confidence": 0.9,
                "created_at": 1})
            .as_object()
            .unwrap()
            .clone()
        }

        fn journal() -> tempfile::TempDir {
            let root = tempfile::tempdir().unwrap();
            segment(
                root.path(),
                "090000_300",
                Some(sense("planning the launch", "Ann")),
            );
            segment(
                root.path(),
                "090500_300",
                Some(sense("the secret plan", "Bob")),
            );
            segment(
                root.path(),
                "091000_300",
                Some(sense("reviewing the launch", "Cat")),
            );
            root
        }

        const ALL: [&str; 3] = ["090000_300", "090500_300", "091000_300"];

        #[test]
        fn a_record_whose_segments_are_all_there_is_left_as_it_is() {
            let root = journal();
            assert_eq!(
                rederive(root.path(), DAY, &record(&ALL)),
                Rederived::Unchanged
            );
        }

        #[test]
        fn a_record_is_made_again_from_what_remains_with_none_of_the_gone_words() {
            let root = journal();
            trashed(root.path(), "090500_300");
            let Rederived::Changed(next) = rederive(root.path(), DAY, &record(&ALL)) else {
                panic!("re-made");
            };
            assert_eq!(next["segments"], json!(["090000_300", "091000_300"]));
            assert_eq!(next["description"], json!("reviewing the launch"));
            assert_eq!(next["active_entities"], json!(["Ann", "Cat"]));
            for field in [
                "title",
                "story",
                "relations",
                "participation",
                "participation_confidence",
            ] {
                assert!(!next.contains_key(field), "{field} stays");
            }
            assert!(!serde_json::to_string(&next).unwrap().contains("secret"));
            assert!(!serde_json::to_string(&next).unwrap().contains("Bob"));
            // Identity and placement stay.
            assert_eq!(next["id"], json!("meeting_090000_300"));
            assert_eq!(next["created_at"], json!(1));
        }

        #[test]
        fn a_record_with_nothing_left_goes() {
            let root = journal();
            trashed(root.path(), "090500_300");
            assert_eq!(
                rederive(root.path(), DAY, &record(&["090500_300"])),
                Rederived::Removed
            );
        }

        #[test]
        fn an_unreadable_sense_never_removes_a_record() {
            let root = journal();
            segment(root.path(), "091000_300", None);
            trashed(root.path(), "090500_300");
            let Rederived::Changed(next) =
                rederive(root.path(), DAY, &record(&["090500_300", "091000_300"]))
            else {
                panic!("kept");
            };
            assert_eq!(next["segments"], json!(["091000_300"]));
            assert!(!next.contains_key("story"));
        }

        #[test]
        fn the_owners_own_record_keeps_its_words() {
            let root = journal();
            trashed(root.path(), "090500_300");
            let mut mine = record(&ALL);
            mine.insert("source".into(), json!("user"));
            let Rederived::Changed(next) = rederive(root.path(), DAY, &mine) else {
                panic!("narrowed");
            };
            assert_eq!(next["segments"], json!(["090000_300", "091000_300"]));
            assert_eq!(next["title"], json!("the secret plan"));
            assert_eq!(
                rederive(root.path(), DAY, &record(&["090000_300"])),
                Rederived::Unchanged
            );
        }

        #[test]
        fn a_key_is_found_in_a_folder_whose_name_carries_more() {
            let root = journal();
            let base = root.path().join("chronicle").join(DAY).join("laptop");
            fs::rename(base.join("091000_300"), base.join("091000_300_summary")).unwrap();
            assert_eq!(
                rederive(root.path(), DAY, &record(&ALL)),
                Rederived::Unchanged
            );
        }

        #[test]
        fn a_recover_makes_a_narrowed_record_again_from_all_of_it() {
            let root = journal();
            let mut narrowed = record(&ALL);
            narrowed.insert("description".into(), json!("reviewing the launch"));
            let Rederived::Changed(next) = rederive_with(root.path(), DAY, &narrowed, true) else {
                panic!("re-made");
            };
            assert_eq!(next["active_entities"], json!(["Ann", "Bob", "Cat"]));
            assert_eq!(next["description"], json!("reviewing the launch"));
            assert!(!next.contains_key("story"));
        }

        #[test]
        fn discrete_items_never_merge_with_their_neighbours() {
            use crate::activity_state::{ActivityStateMachine, CoalesceScope};
            let mut machine = ActivityStateMachine::default();
            let sense_a = json!({
                "density": "active",
                "content_type": "communication",
                "activity_summary": "Read email from Elena.",
                "facets": [{"facet": "work", "level": "high", "activity": "Read email from Elena."}],
                "entities": [{"name": "Elena"}]
            });
            let sense_b = json!({
                "density": "active",
                "content_type": "communication",
                "activity_summary": "Read email from Marcus.",
                "facets": [{"facet": "work", "level": "high", "activity": "Read email from Marcus."}],
                "entities": [{"name": "Marcus"}]
            });
            // Two unrelated discrete items 4 minutes apart must stay separate activities.
            machine.update_with_scope(
                &sense_a,
                "100000_0",
                "20210514",
                None,
                1,
                &CoalesceScope::DiscreteItem,
            );
            machine.update_with_scope(
                &sense_b,
                "100400_0",
                "20210514",
                None,
                2,
                &CoalesceScope::DiscreteItem,
            );
            machine.close_active("100400_0", 3);
            let done = machine.completed_activities();
            assert_eq!(done.len(), 2);
            assert_eq!(done[0]["segments"], json!(["100000_0"]));
            assert_eq!(done[1]["segments"], json!(["100400_0"]));
        }

        #[test]
        fn low_change_with_idle_content_type_is_skipped_on_rederive() {
            let root = journal();
            segment(
                root.path(),
                "091000_300",
                Some(json!({
                    "density": "low_change",
                    "content_type": "idle",
                    "activity_summary": "Lock screen",
                    "facets": [{"facet": "work", "level": "high", "activity": "Lock screen"}],
                    "entities": [{"name": "Ghost"}]
                })),
            );
            let mut held = record(&ALL);
            held.insert(
                "edits".into(),
                json!([{"actor": "owner:update", "fields": ["activity"], "timestamp": "2026-11-10T10:00:00Z", "note": ""}]),
            );
            let Rederived::Changed(next) = rederive_with(root.path(), DAY, &held, true) else {
                panic!("re-made");
            };
            assert_eq!(next["description"], json!("the secret plan"));
            assert_eq!(next["active_entities"], json!(["Ann", "Bob"]));
        }

        #[test]
        fn default_stream_suffixed_segments_participation_said_and_user_rows_rederive_cleanly() {
            let root = tempfile::tempdir().unwrap();
            let day_dir = root.path().join("chronicle").join(DAY);
            let write_default_seg = |dir_name: &str, s: Value| {
                let dir = day_dir.join(dir_name);
                fs::create_dir_all(dir.join("talents")).unwrap();
                fs::write(dir.join("audio.flac"), b"x").unwrap();
                fs::write(
                    dir.join("talents/sense.json"),
                    serde_json::to_vec(&s).unwrap(),
                )
                .unwrap();
            };
            write_default_seg("090000_300", sense("planning the launch", "Ann"));
            write_default_seg("090500_300", sense("the secret plan", "Bob"));
            write_default_seg("091000_300_summary", sense("reviewing the launch", "Cat"));

            // Trash middle segment under _default.
            let mid = day_dir.join("090500_300");
            fs::remove_dir_all(&mid).unwrap();
            fs::create_dir_all(&mid).unwrap();
            fs::write(mid.join("tombstone.json"), b"{}").unwrap();

            // Streamless _default record carrying participation_said quote from trashed segment.
            let mut rec = record(&ALL);
            rec.remove("stream");
            rec.insert(
                "participation_said".into(),
                json!([{"name": "Bob", "quote": "the secret plan"}]),
            );
            let Rederived::Changed(next) = rederive(root.path(), DAY, &rec) else {
                panic!("_default record should narrow");
            };
            assert_eq!(next["segments"], json!(["090000_300", "091000_300"]));
            // Sense was actually read from suffixed 091000_300_summary.
            assert_eq!(next["description"], json!("reviewing the launch"));
            assert_eq!(next["active_entities"], json!(["Ann", "Cat"]));
            assert!(!next.contains_key("participation_said"));
            assert!(!serde_json::to_string(&next).unwrap().contains("secret"));

            // Single-segment _default record whose only segment was trashed is removed.
            let mut single = record(&["090500_300"]);
            single.remove("stream");
            assert_eq!(rederive(root.path(), DAY, &single), Rederived::Removed);

            // Anchored source: "user" _default record whose only linked segment is trashed
            // narrows segments to [] while keeping owner words intact.
            let mut user_rec = record(&["090500_300"]);
            user_rec.remove("stream");
            user_rec.insert("source".into(), json!("user"));
            user_rec.insert("title".into(), json!("My manual meeting note"));
            let Rederived::Changed(narrowed_user) = rederive(root.path(), DAY, &user_rec) else {
                panic!("user record with all segments trashed should narrow to []");
            };
            assert_eq!(narrowed_user["segments"], json!([]));
            assert_eq!(narrowed_user["title"], json!("My manual meeting note"));
            assert_eq!(narrowed_user["source"], json!("user"));

            // Legacy streamless record whose keys do not exist under chronicle/<day>/ stays untouched.
            let mut legacy = record(&["140000_300"]);
            legacy.remove("stream");
            assert_eq!(rederive(root.path(), DAY, &legacy), Rederived::Unchanged);
        }
    }
}
