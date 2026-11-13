// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::path::Path;

use chrono::Offset;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use solstone_core_indexer_query::{AdmittedCategory, ConnectionScope};
use solstone_core_transcripts::{MAX_TRANSCRIPT_PAGE_BYTES, MAX_TRANSCRIPT_PAGE_ITEMS};

use super::{
    MAX_DAY_BYTES, MAX_FACET_BYTES, MAX_OPAQUE_REFERENCE_BYTES, ToolError,
    optional_string_within_limit, search::MAX_LIMIT,
};
use crate::dispatch::{
    DispatchError, DispatchPrincipal, Prepared, available_facets, codec, entity_name, fold_literal,
    read_scoped_entities,
};
use crate::permissions::ConnectionReadSnapshot;
use crate::references::{
    ActivityAddress, ActivityListCursor, ActivityReference, ReferenceKind, ReferenceTarget,
    is_framed_entity_token,
};
use crate::segment_links::{
    LinkSubject, LiveTranscriptPages, activity_segment_candidates, issue_segment_links,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ValidatedListActivities {
    pub(crate) day: Option<String>,
    pub(crate) from: String,
    pub(crate) to: String,
    pub(crate) facet: Option<String>,
    pub(crate) entity: Option<String>,
    pub(crate) limit: usize,
    pub(crate) cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ValidatedGetActivitySources {
    pub(crate) reference: String,
    pub(crate) cursor: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListParams {
    #[serde(default)]
    day: Option<String>,
    // `from` and `to` are the names this tool took first; they stay accepted.
    #[serde(default, alias = "from")]
    day_from: Option<String>,
    #[serde(default, alias = "to")]
    day_to: Option<String>,
    #[serde(default)]
    facet: Option<String>,
    #[serde(default)]
    entity: Option<String>,
    #[serde(default = "default_limit")]
    limit: usize,
    #[serde(default)]
    cursor: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GetSourcesParams {
    reference: String,
    #[serde(default)]
    cursor: Option<String>,
}

const fn default_limit() -> usize {
    10
}

fn validate_limit(limit: usize) -> Result<(), ToolError> {
    if limit == 0 || limit > MAX_LIMIT {
        Err(ToolError::InvalidInput)
    } else {
        Ok(())
    }
}

pub(crate) fn validate_list(
    params: Option<&Value>,
    owner_zone: &solstone_core_journal_config::Tz,
) -> Result<ValidatedListActivities, ToolError> {
    let params = params.cloned().unwrap_or_else(|| serde_json::json!({}));
    let params =
        serde_json::from_value::<ListParams>(params).map_err(|_| ToolError::InvalidInput)?;
    validate_limit(params.limit)?;

    if !optional_string_within_limit(&params.day, MAX_DAY_BYTES)
        || !optional_string_within_limit(&params.day_from, MAX_DAY_BYTES)
        || !optional_string_within_limit(&params.day_to, MAX_DAY_BYTES)
        || !optional_string_within_limit(&params.facet, MAX_FACET_BYTES)
        || !optional_string_within_limit(&params.cursor, MAX_OPAQUE_REFERENCE_BYTES)
    {
        return Err(ToolError::InvalidInput);
    }

    if params.day.is_some() && (params.day_from.is_some() || params.day_to.is_some()) {
        return Err(ToolError::ActivityDayArguments);
    }

    let normalized_day = super::normalize_day(params.day)?;
    let normalized_from = super::normalize_day(params.day_from)?;
    let normalized_to = super::normalize_day(params.day_to)?;

    let (from, to) = if let Some(day) = normalized_day.clone() {
        (day.clone(), day)
    } else {
        match (normalized_from, normalized_to) {
            (None, None) => {
                let today = chrono::Utc::now()
                    .with_timezone(owner_zone)
                    .format("%Y%m%d")
                    .to_string();
                (today.clone(), today)
            }
            (Some(f), None) => (f.clone(), f),
            (None, Some(t)) => (t.clone(), t),
            (Some(f), Some(t)) => (f, t),
        }
    };

    if to < from {
        return Err(ToolError::ActivityDayArguments);
    }

    Ok(ValidatedListActivities {
        day: normalized_day,
        from,
        to,
        facet: params.facet,
        entity: params.entity,
        limit: params.limit,
        cursor: params.cursor,
    })
}

pub(crate) fn validate_get_sources(
    params: Option<&Value>,
) -> Result<ValidatedGetActivitySources, ToolError> {
    let params = match params {
        None | Some(Value::Null) => return Err(ToolError::InvalidInput),
        Some(val) => val,
    };
    let params = serde_json::from_value::<GetSourcesParams>(params.clone())
        .map_err(|_| ToolError::InvalidInput)?;

    if params.reference.is_empty() || params.reference.len() > MAX_OPAQUE_REFERENCE_BYTES {
        return Err(ToolError::InvalidInput);
    }
    if !optional_string_within_limit(&params.cursor, MAX_OPAQUE_REFERENCE_BYTES) {
        return Err(ToolError::InvalidInput);
    }

    Ok(ValidatedGetActivitySources {
        reference: params.reference,
        cursor: params.cursor,
    })
}

pub(crate) fn activity_title(record: &Map<String, Value>) -> String {
    for key in ["title", "description", "activity", "id"] {
        if let Some(val) = record.get(key).and_then(Value::as_str) {
            let trimmed = val.trim();
            if !trimmed.is_empty() {
                if matches!(key, "activity" | "id") {
                    return trimmed
                        .replace('_', " ")
                        .split_whitespace()
                        .map(|part| {
                            let mut chars = part.chars();
                            chars
                                .next()
                                .map(|first| {
                                    first.to_uppercase().collect::<String>() + chars.as_str()
                                })
                                .unwrap_or_default()
                        })
                        .collect::<Vec<_>>()
                        .join(" ");
                } else {
                    return trimmed.to_string();
                }
            }
        }
    }
    "untitled activity".to_string()
}

pub(crate) fn parse_segment_time(segment: &str) -> Option<(u32, u32, u32, u32)> {
    let (time_part, length_part) = segment.split_once('_')?;
    if time_part.len() != 6
        || !time_part.bytes().all(|byte| byte.is_ascii_digit())
        || length_part.is_empty()
        || !length_part.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let hour = time_part[0..2].parse::<u32>().ok()?;
    let minute = time_part[2..4].parse::<u32>().ok()?;
    let second = time_part[4..6].parse::<u32>().ok()?;
    if hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    let duration = length_part.parse::<u32>().ok()?;
    Some((hour, minute, second, duration))
}

fn parse_time_string_to_seconds(s: &str) -> Option<u32> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() == 2 {
        let h = parts[0].parse::<u32>().ok()?;
        let m = parts[1].parse::<u32>().ok()?;
        if h < 24 && m < 60 {
            return Some(h * 3600 + m * 60);
        }
    } else if parts.len() == 3 {
        let h = parts[0].parse::<u32>().ok()?;
        let m = parts[1].parse::<u32>().ok()?;
        let sec = parts[2].parse::<u32>().ok()?;
        if h < 24 && m < 60 && sec < 60 {
            return Some(h * 3600 + m * 60 + sec);
        }
    }
    None
}

pub(crate) fn activity_times_and_duration(
    record: &Map<String, Value>,
) -> (Option<String>, Option<String>, Option<u64>) {
    if let Some(segments) = record.get("segments").and_then(Value::as_array)
        && !segments.is_empty()
    {
        let mut total_duration = 0u64;
        let mut first_start: Option<(u32, u32, u32)> = None;
        let mut last_end_seconds: Option<u32> = None;
        let mut parsed_any = false;
        for seg_val in segments {
            if let Some(seg_str) = seg_val.as_str()
                && let Some((h, m, s, dur)) = parse_segment_time(seg_str)
            {
                parsed_any = true;
                if first_start.is_none() {
                    first_start = Some((h, m, s));
                }
                let seg_end = h * 3600 + m * 60 + s + dur;
                last_end_seconds = Some(last_end_seconds.map_or(seg_end, |cur| cur.max(seg_end)));
                total_duration += dur as u64;
            }
        }
        let start = first_start.map(|(h, m, s)| format!("{h:02}:{m:02}:{s:02}"));
        let end = last_end_seconds.map(|sec| {
            let clamped = sec.min(23 * 3600 + 59 * 60 + 59);
            format!(
                "{:02}:{:02}:{:02}",
                clamped / 3600,
                (clamped % 3600) / 60,
                clamped % 60
            )
        });
        let duration = if parsed_any {
            Some(total_duration)
        } else {
            None
        };
        return (start, end, duration);
    }

    let start = record
        .get("start")
        .and_then(Value::as_str)
        .map(str::to_string);
    let end = record
        .get("end")
        .and_then(Value::as_str)
        .map(str::to_string);
    let duration = match (&start, &end) {
        (Some(s), Some(e)) => {
            if let (Some(sec_s), Some(sec_e)) = (
                parse_time_string_to_seconds(s),
                parse_time_string_to_seconds(e),
            ) {
                if sec_e >= sec_s {
                    Some((sec_e - sec_s) as u64)
                } else {
                    None
                }
            } else {
                None
            }
        }
        _ => None,
    };
    (start, end, duration)
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct ActivityPeople {
    pub participants: Vec<String>,
    pub heard: Vec<String>,
    pub mentioned: Vec<String>,
    pub on_screen: Vec<String>,
    /// Voices heard that the journal has no name for.
    pub unnamed_voices: usize,
}

/// A voice the journal has no name for, as participation records it:
/// `Speaker 3`. It is a voice, not a person.
fn is_unnamed_voice(name: &str) -> bool {
    let lower = name.to_lowercase();
    let Some(rest) = lower.strip_prefix("speaker") else {
        return false;
    };
    let rest = rest.trim_start();
    !rest.is_empty() && rest.bytes().all(|byte| byte.is_ascii_digit())
}

/// How strongly a role places someone in an activity; a name recorded under
/// two roles is listed under the stronger.
fn role_rank(role: &str) -> u8 {
    match role {
        "attendee" => 3,
        "heard" => 2,
        "visible" => 0,
        _ => 1,
    }
}

/// Classifies an activity's participants into participants, heard, mentioned,
/// and on-screen names.
#[cfg(test)]
pub(crate) fn activity_people(record: &Map<String, Value>) -> ActivityPeople {
    activity_people_across(std::iter::once(record))
}

/// The people of one moment, read from every facet's copy of it. Each name is
/// listed once, under the strongest role any copy gives it, in the order the
/// copies first name it. A voice with no name is counted, never listed.
pub(crate) fn activity_people_across<'a>(
    records: impl Iterator<Item = &'a Map<String, Value>>,
) -> ActivityPeople {
    // Name as first written, and its strongest role so far.
    let mut named: Vec<(String, u8)> = Vec::new();
    let mut people = ActivityPeople::default();
    for record in records {
        let Some(entries) = record.get("participation").and_then(Value::as_array) else {
            continue;
        };
        let mut voices = 0;
        for entry in entries.iter().filter_map(Value::as_object) {
            let Some(name) = entry.get("name").and_then(Value::as_str) else {
                continue;
            };
            let trimmed = name.trim();
            if trimmed.is_empty() {
                continue;
            }
            let role = entry
                .get("role")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if is_unnamed_voice(trimmed) {
                // Counted only where it was heard: an attendee or a heard voice.
                if role_rank(role) >= 2 {
                    voices += 1;
                }
                continue;
            }
            let rank = role_rank(role);
            match named
                .iter_mut()
                .find(|(known, _)| known.eq_ignore_ascii_case(trimmed))
            {
                Some((_, known_rank)) => *known_rank = (*known_rank).max(rank),
                None => named.push((trimmed.to_owned(), rank)),
            }
        }
        // Copies hear the same recording, so the most any one copy counted.
        people.unnamed_voices = people.unnamed_voices.max(voices);
    }
    for (name, rank) in named {
        match rank {
            3 => people.participants.push(name),
            2 => people.heard.push(name),
            0 => people.on_screen.push(name),
            _ => people.mentioned.push(name),
        }
    }
    people
}

/// What kind of activity the journal recorded: meeting, writing and so on,
/// in its own word.
pub(crate) fn activity_kind(record: &Map<String, Value>) -> Option<String> {
    let kind = record.get("activity").and_then(Value::as_str)?.trim();
    (!kind.is_empty()).then(|| kind.replace('_', " "))
}

pub(crate) fn activity_entities(record: &Map<String, Value>) -> Vec<String> {
    record
        .get("active_entities")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|e| !e.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// One item a beside line can name: its span as instants, and the line.
type BesideLine = ((i64, i64), Value);

/// A face's span on its first account's instants.
fn face_span(face: &Value) -> Option<(i64, i64)> {
    let first = face.get("accounts")?.get(0)?;
    let time = |field: &str| {
        solstone_core_format::content::body_records::parse_source_time(first.get(field)?.as_str()?)
            .map(|time| time.timestamp())
    };
    let (start, end) = (time("start")?, time("end")?);
    (end > start).then_some((start, end))
}

/// What a beside line says of a body face: its kind, the owner's title or the
/// source's label, and its times. No reading values.
fn body_line(face: &Value) -> Value {
    let first = face.get("accounts").and_then(|accounts| accounts.get(0));
    json!({
        "kind": face.get("activity").cloned().unwrap_or(Value::Null),
        "title": face
            .get("title")
            .cloned()
            .or_else(|| first.and_then(|first| first.get("label")).cloned())
            .unwrap_or(Value::Null),
        "start": first.and_then(|first| first.get("start")).cloned().unwrap_or(Value::Null),
        "end": first.and_then(|first| first.get("end")).cloned().unwrap_or(Value::Null),
    })
}

/// Body faces this connection reads that can sit beside a capture activity on
/// `day`: those shown under it and under the day before, for a night.
fn body_lines_near(
    journal_root: &Path,
    body: &mut [(String, solstone_core_location::body_records::Groups)],
    day: &str,
    targets: &mut Vec<String>,
) -> Vec<BesideLine> {
    let previous = chrono::NaiveDate::parse_from_str(day, "%Y%m%d")
        .ok()
        .and_then(|date| date.pred_opt())
        .map(|date| date.format("%Y%m%d").to_string());
    let mut lines = Vec::new();
    for on in std::iter::once(day.to_owned()).chain(previous) {
        for (_, groups) in body.iter_mut() {
            for group in groups.on(&on).to_vec() {
                let face = groups.compose(journal_root, &group);
                if !solstone_core_location::body_records::face_still_there(&face) {
                    continue;
                }
                crate::body_source::name_windows(targets, crate::body_source::face_windows(&face));
                let value = solstone_core_location::body_records::face_json(&face);
                if let Some(span) = face_span(&value) {
                    lines.push((span, body_line(&value)));
                }
            }
        }
    }
    lines
}

/// How many days of body groups `list_activities` reads at a time.
const BODY_STRETCH_DAYS: i64 = 31;

/// Capture activities this connection reads that can sit beside a body item
/// shown under `day`: those on it and on the next day, for a night. Beside
/// lines: title and type.
fn capture_lines_on(
    journal_root: &Path,
    facets: &[(String, String, String)],
    day: &str,
) -> Vec<BesideLine> {
    let mut lines = Vec::new();
    let next = solstone_core_location::body_records::next_day(day);
    for on in std::iter::once(day.to_owned()).chain(next) {
        lines.extend(capture_lines_of(journal_root, facets, &on));
    }
    lines
}

fn capture_lines_of(
    journal_root: &Path,
    facets: &[(String, String, String)],
    day: &str,
) -> Vec<BesideLine> {
    let mut lines = Vec::new();
    for (_, facet_dir, _) in facets {
        for record in
            solstone_core_facets::load_activity_records(journal_root, facet_dir, day, false)
                .unwrap_or_default()
        {
            if let Some(span) =
                solstone_core_location::body_records::capture_instants(journal_root, day, &record)
            {
                lines.push((
                    span,
                    json!({"title": activity_title(&record), "activity": record.get("activity").cloned().unwrap_or(Value::Null)}),
                ));
            }
        }
    }
    lines
}

/// A body face as `list_activities` gives it: the owner's title when they set
/// one, else the first account's label, its kind, and every account with its
/// own composed line. Nothing is summed or chosen.
pub(crate) fn body_activity(
    face: &Value,
    facet_title: &str,
    day: &str,
    reference: String,
    zone: &str,
) -> Value {
    let accounts = face
        .get("accounts")
        .cloned()
        .unwrap_or(Value::Array(Vec::new()));
    let first = accounts.get(0).cloned().unwrap_or(Value::Null);
    let title = face
        .get("title")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| {
            first
                .get("label")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_default();
    let mut activity = json!({
        "title": title,
        "facet": facet_title,
        "day": day,
        "source": "body",
        "kind": face.get("activity").cloned().unwrap_or(Value::Null),
        "start": first.get("start").cloned().unwrap_or(Value::Null),
        "end": first.get("end").cloned().unwrap_or(Value::Null),
        "accounts": accounts,
        "reference": reference,
        "zone": zone,
    });
    for field in ["description", "details"] {
        if let Some(value) = face.get(field) {
            activity[field] = value.clone();
        }
    }
    activity
}

pub(crate) fn sort_activity_key(
    file_day: &str,
    facet_id: &str,
    record: &Map<String, Value>,
) -> (String, String, i64, String) {
    let created_at = record
        .get("created_at")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let id_val = record
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    (
        file_day.to_string(),
        facet_id.to_string(),
        created_at,
        id_val,
    )
}

pub(crate) fn is_path_or_segment_pointer(val: &Value) -> bool {
    match val {
        Value::Object(_) => true,
        Value::String(s) => {
            s.contains('/')
                || s.contains('\\')
                || s.contains(':')
                || s.ends_with(".md")
                || s.ends_with(".json")
                || s.ends_with(".jsonl")
        }
        _ => false,
    }
}

pub(crate) fn clean_activity_record_for_wire(mut record: Map<String, Value>) -> Map<String, Value> {
    record.remove("stream");
    if let Some(src) = record.get("source")
        && is_path_or_segment_pointer(src)
    {
        record.remove("source");
    }
    // What the owner retracted, or wrote over, never reaches their agent: an
    // edit goes out without the values it recorded, and a removed one not at all.
    if let Some(Value::Array(edits)) = record.get_mut("edits") {
        edits.retain(|edit| {
            !edit
                .as_object()
                .is_some_and(solstone_core_facets::edit_removed)
        });
        for edit in edits.iter_mut().filter_map(Value::as_object_mut) {
            edit.remove(solstone_core_facets::EDIT_SET);
            edit.remove(solstone_core_facets::EDIT_BEFORE);
            edit.remove(solstone_core_facets::EDIT_LINES);
            edit.remove(solstone_core_facets::EDIT_SPEAKER);
        }
    }
    record
}

pub(crate) fn execute_list(
    journal_root: &Path,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    request: ValidatedListActivities,
) -> Result<Prepared, DispatchError> {
    let available = available_facets(journal_root, snapshot)?;
    let all_facets: Vec<(String, String, String)> = available
        .into_iter()
        .map(|(id, dir, decl)| {
            let title = decl.as_ref().map_or_else(
                || dir.clone(),
                |v| {
                    if v.title.is_empty() {
                        dir.clone()
                    } else {
                        v.title.clone()
                    }
                },
            );
            (id, dir, title)
        })
        .collect();

    let facets_to_search: Vec<(String, String, String)> = if let Some(ref facet_filter) =
        request.facet
    {
        let matching: Vec<_> = all_facets
            .into_iter()
            .filter(|(id, dir, _)| id == facet_filter || dir == facet_filter)
            .collect();
        if matching.is_empty() {
            return Ok(Prepared::new(json!({ "activities": [] }), 0, Vec::new()).when_empty(
                "No activities. This connection can see none that match. This is a complete answer, not an error.",
            ));
        }
        matching
    } else {
        all_facets
    };

    enum EntityFilter {
        Token {
            entity_id: String,
            folded_name: String,
        },
        Literal(String),
    }

    let entity_filter: Option<EntityFilter> = if let Some(ref entity_str) = request.entity {
        if is_framed_entity_token(entity_str) {
            if !snapshot.categories.contains(&AdmittedCategory::Entities) {
                return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
            }
            let ReferenceTarget::Entity(entity_ref) = codec()
                .resolve(
                    entity_str,
                    ReferenceKind::Entity,
                    principal.connection,
                    snapshot.generation,
                )
                .map_err(|_| DispatchError::Tool(ToolError::ReferenceNotFound))?
            else {
                return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
            };
            let Some((_, facet_dir, _)) = facets_to_search
                .iter()
                .find(|(id, _, _)| id == &entity_ref.facet_id)
            else {
                return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
            };
            let item = read_scoped_entities(journal_root, facet_dir)?
                .into_iter()
                .find(|item| item.entity_id == entity_ref.entity_id)
                .ok_or(DispatchError::Tool(ToolError::ReferenceNotFound))?;
            let name = entity_name(&item.identity);
            Some(EntityFilter::Token {
                entity_id: entity_ref.entity_id,
                folded_name: fold_literal(&name),
            })
        } else {
            Some(EntityFilter::Literal(fold_literal(entity_str)))
        }
    } else {
        None
    };

    let cursor_bound = if let Some(ref cursor_token) = request.cursor {
        let ReferenceTarget::ActivityList(c) = codec()
            .resolve(
                cursor_token,
                ReferenceKind::ActivityList,
                principal.connection,
                snapshot.generation,
            )
            .map_err(|_| DispatchError::Tool(ToolError::ReferenceNotFound))?
        else {
            return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
        };
        if c.day != request.day
            || c.from.as_deref() != Some(&request.from)
            || c.to.as_deref() != Some(&request.to)
            || c.facet != request.facet
            || c.entity != request.entity
        {
            return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
        }
        Some((c.last_day, c.last_facet, c.last_created_at, c.last_id))
    } else {
        None
    };

    struct CollectedItem {
        sort_key: (String, String, i64, String),
        record: Map<String, Value>,
        facet_id: String,
        facet_dir: String,
        facet_title: String,
        day: String,
        file_digest: String,
        rec_identity: String,
        /// A body record's group as this connection reads it, and the
        /// stretch it was read in; composed once the item is on the page.
        group: Option<(usize, solstone_core_location::body_records::Group)>,
        /// Whether this record passes the entity filter, when one is given.
        matched: bool,
        /// The other facets holding a copy of this moment, by title, and
        /// those copies' records.
        also_in: Vec<String>,
        copies: Vec<Map<String, Value>>,
    }

    let mut collected = Vec::new();
    let mut targets = Vec::new();
    let gate = crate::body_source::window_gate(journal_root, snapshot);
    // Each facet's body groups this connection reads, grouped once: from the
    // day before the first day this page can show (a night beside it) to the
    // range's last. Nothing anchored before a cursor's day can be on its page.
    let first_shown = match &cursor_bound {
        Some((last_day, ..)) if last_day > &request.from => last_day,
        _ => &request.from,
    };
    let before_from = chrono::NaiveDate::parse_from_str(first_shown, "%Y%m%d")
        .ok()
        .and_then(|date| date.pred_opt())
        .map_or_else(
            || first_shown.clone(),
            |date| date.format("%Y%m%d").to_string(),
        );
    // Body rows as read, by facet, day and id, until their groups are read.
    let mut body_rows: BTreeMap<(String, String, String), CollectedItem> = BTreeMap::new();
    // A body record or Strava line this connection would list with the body
    // permission, so the answer says it needs it.
    let holds_body = crate::body_source::holds_body(snapshot);
    let mut body_withheld = false;

    for (facet_id, facet_dir, facet_title) in &facets_to_search {
        let stems = solstone_core_facets::list_activity_day_stems(journal_root, facet_dir)
            .map_err(|_| DispatchError::Tool(ToolError::FileUnreadable))?;
        for day in stems {
            if day < request.from || day > request.to {
                continue;
            }
            targets.push(format!("facets/{facet_dir}/activities/{day}.jsonl"));
            let normalized =
                solstone_core_facets::read_activity_records_strict(journal_root, facet_dir, &day)
                    .map_err(|_| DispatchError::Tool(ToolError::FileUnreadable))?;

            let rel_path = format!("facets/{facet_dir}/activities/{day}.jsonl");
            let Some(bytes) = crate::activity_admit::read_contained_source(journal_root, &rel_path)
            else {
                return Err(DispatchError::Tool(ToolError::FileUnreadable));
            };
            let file_digest = crate::activity_admit::hex_encode(&Sha256::digest(&bytes));
            let Ok(text) = String::from_utf8(bytes) else {
                return Err(DispatchError::Tool(ToolError::FileUnreadable));
            };
            let mut raw_rows = Vec::new();
            for line in text.lines() {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let Ok(Value::Object(record)) = serde_json::from_str::<Value>(trimmed) else {
                    return Err(DispatchError::Tool(ToolError::FileUnreadable));
                };
                raw_rows.push((record, trimmed.to_owned()));
            }
            if raw_rows.len() != normalized.len() {
                return Err(DispatchError::Tool(ToolError::FileUnreadable));
            }
            // The same test every read of one of these records makes again.
            let judged = crate::may_see::ActivityDay::new(
                journal_root,
                snapshot,
                facet_dir,
                &day,
                &normalized,
            );
            for (record, trimmed) in raw_rows {
                // Identity stays the raw line, not the normalized object.
                match judged.shows(&record) {
                    crate::may_see::Shown::Yes => {}
                    crate::may_see::Shown::No => continue,
                    // A body record or Strava line this connection would list
                    // with the body permission, so the answer says it needs it.
                    crate::may_see::Shown::NeedsBody => {
                        body_withheld = body_withheld || entity_filter.is_none();
                        continue;
                    }
                }
                if solstone_core_facets::is_body_record(&record) {
                    // A body record shows as its face, only for windows this
                    // connection reads; it names no entity, so an entity filter
                    // leaves it out.
                    if entity_filter.is_none()
                        && let Some(id) = record.get("id").and_then(Value::as_str)
                    {
                        body_rows.insert(
                            (facet_dir.clone(), day.clone(), id.to_owned()),
                            CollectedItem {
                                sort_key: sort_activity_key(&day, facet_id, &record),
                                rec_identity: crate::activity_admit::compute_record_identity(
                                    &record, &trimmed,
                                ),
                                record,
                                facet_id: facet_id.clone(),
                                facet_dir: facet_dir.clone(),
                                facet_title: facet_title.clone(),
                                day: day.clone(),
                                file_digest: file_digest.clone(),
                                group: None,
                                matched: true,
                                also_in: Vec::new(),
                                copies: Vec::new(),
                            },
                        );
                    }
                    continue;
                }
                let entities = activity_entities(&record);
                // A copy that doesn't match is kept until the moment is
                // folded: another facet's copy of it may.
                let matched = entity_filter.as_ref().is_none_or(|filter| match filter {
                    EntityFilter::Token {
                        entity_id,
                        folded_name,
                    } => entities
                        .iter()
                        .any(|e| e == entity_id || fold_literal(e) == *folded_name),
                    EntityFilter::Literal(target) => entities
                        .iter()
                        .any(|entity| fold_literal(entity).contains(target)),
                });
                let rec_identity =
                    crate::activity_admit::compute_record_identity(&record, &trimmed);
                let sort_key = sort_activity_key(&day, facet_id, &record);
                collected.push(CollectedItem {
                    sort_key,
                    record,
                    facet_id: facet_id.clone(),
                    facet_dir: facet_dir.clone(),
                    facet_title: facet_title.clone(),
                    day: day.clone(),
                    file_digest: file_digest.clone(),
                    rec_identity,
                    group: None,
                    matched,
                    also_in: Vec::new(),
                    copies: Vec::new(),
                });
            }
        }
    }

    // A moment recorded in several facets is listed once, from the facet it
    // belongs to most, with the others named beside it. It passes an entity
    // filter when any facet's copy does.
    collected.sort_by(|a, b| a.sort_key.cmp(&b.sort_key));
    let collected: Vec<CollectedItem> = solstone_core_facets::merge_moment_copies(
        collected,
        |item| item.day.as_str(),
        |item| &item.record,
    )
    .into_iter()
    .filter(|moment| moment.primary.matched || moment.copies.iter().any(|copy| copy.matched))
    .map(|moment| {
        let mut primary = moment.primary;
        for copy in moment.copies {
            primary.also_in.push(copy.facet_title);
            primary.copies.push(copy.record);
        }
        primary
    })
    .collect();
    let mut collected = collected;

    // Body groups, read a stretch of days at a time from the day before the
    // first this page can show, until the page and the item after it are
    // settled: everything sorts by day first, so nothing on a later day can
    // come before them. Each stretch groups as its days do alone.
    let mut body: Vec<(String, solstone_core_location::body_records::Groups)> = Vec::new();
    if let (Ok(mut from), Ok(last)) = (
        chrono::NaiveDate::parse_from_str(&before_from, "%Y%m%d"),
        chrono::NaiveDate::parse_from_str(&request.to, "%Y%m%d"),
    ) {
        while from <= last {
            let to = (from + chrono::Duration::days(BODY_STRETCH_DAYS - 1)).min(last);
            let (from_day, to_day) = (
                from.format("%Y%m%d").to_string(),
                to.format("%Y%m%d").to_string(),
            );
            for (_, facet_dir, _) in &facets_to_search {
                let groups = match solstone_core_location::body_records::body_groups_range(
                    journal_root,
                    facet_dir,
                    &from_day,
                    &to_day,
                    Some(&gate),
                    false,
                ) {
                    Ok(groups) => groups,
                    // A body record shows only as its face, so a list that would
                    // hold one can't go on without it; beside lines alone can.
                    Err(_) if entity_filter.is_none() => {
                        return Err(DispatchError::Tool(ToolError::FileUnreadable));
                    }
                    Err(_) => solstone_core_location::body_records::Groups::default(),
                };
                let mut on = from;
                while on <= to {
                    let day = on.format("%Y%m%d").to_string();
                    for group in groups.on(&day) {
                        if let Some(mut item) = body_rows.remove(&(
                            facet_dir.clone(),
                            day.clone(),
                            group.anchor_id.clone(),
                        )) {
                            item.group = Some((body.len(), group.clone()));
                            collected.push(item);
                        }
                    }
                    on += chrono::Duration::days(1);
                }
                body.push((facet_dir.clone(), groups));
            }
            let settled = collected
                .iter()
                .filter(|item| item.day <= to_day)
                .filter(|item| {
                    cursor_bound
                        .as_ref()
                        .is_none_or(|cursor_key| &item.sort_key > cursor_key)
                })
                .count();
            if settled > request.limit {
                break;
            }
            from = to + chrono::Duration::days(1);
        }
    }

    collected.sort_by(|a, b| a.sort_key.cmp(&b.sort_key));

    let items_iter = collected.into_iter();
    let unskipped: Vec<CollectedItem> = if let Some(ref cursor_key) = cursor_bound {
        items_iter
            .filter(|item| &item.sort_key > cursor_key)
            .collect()
    } else {
        items_iter.collect()
    };

    let has_more = unskipped.len() > request.limit;
    let page: Vec<CollectedItem> = unskipped.into_iter().take(request.limit).collect();

    let owner_zone = solstone_core_journal_config::owner_zone(journal_root);
    let now = chrono::Utc::now().with_timezone(&owner_zone);
    let zone_str = solstone_core_journal_config::zone_label(
        Some(owner_zone),
        now.offset().fix().local_minus_utc(),
    );

    // The calendar and a day's location belong to no facet: only a connection
    // that already reads the whole journal's transcripts, where calendar notes
    // and each day's location in words are, sees what it had scheduled during
    // an activity and where a device was.
    let sees_calendar = matches!(snapshot.scope, ConnectionScope::WholeJournal)
        && snapshot.categories.contains(&AdmittedCategory::Transcripts);
    // Where a device was during a body face is body data with the face.
    let sees_body_place = matches!(snapshot.scope, ConnectionScope::WholeJournal) && holds_body;
    let mut calendars = BTreeMap::new();
    let mut locations = BTreeMap::new();

    let sources = crate::dispatch::source_labels(journal_root);
    let mut activities = Vec::new();
    // Beside lines between body faces and capture activities, both ways, only
    // between items this connection reads. Worked out when read; never stored.
    let mut body_near: BTreeMap<String, Vec<BesideLine>> = BTreeMap::new();
    let mut capture_on: BTreeMap<String, Vec<BesideLine>> = BTreeMap::new();
    for item in &page {
        let reference = codec()
            .mint(
                principal.connection,
                snapshot.generation,
                ReferenceTarget::Activity(ActivityReference {
                    facet_id: item.facet_id.clone(),
                    facet_dir: item.facet_dir.clone(),
                    day: item.day.clone(),
                    record_identity: item.rec_identity.clone(),
                    file_digest: item.file_digest.clone(),
                    unit_index: None,
                    source_binding: None,
                }),
            )
            .map_err(|_| DispatchError::Tool(ToolError::ReferenceNotFound))?;

        let face = item.group.as_ref().map(|(stretch, group)| {
            let face = body[*stretch].1.compose(journal_root, group);
            crate::body_source::name_windows(&mut targets, crate::body_source::face_windows(&face));
            (
                solstone_core_location::body_records::face_json(&face),
                solstone_core_location::body_records::face_still_there(&face),
            )
        });
        if let Some((face, there)) = &face {
            let mut activity =
                body_activity(face, &item.facet_title, &item.day, reference, &zone_str);
            if let Some(span) = face_span(face) {
                let captures = capture_on.entry(item.day.clone()).or_insert_with(|| {
                    capture_lines_on(journal_root, &facets_to_search, &item.day)
                });
                // A record with nothing still there sits beside nothing, either way.
                let beside = captures
                    .iter()
                    .filter(|_| *there)
                    .filter(|(other, _)| {
                        solstone_core_location::body_records::half_cover(span, *other)
                    })
                    .map(|(_, line)| line.clone())
                    .collect::<Vec<_>>();
                if !beside.is_empty() {
                    activity["beside"] = Value::Array(beside);
                }
                // Where a device was at its middle, as that device timed it.
                if sees_body_place
                    && let Some(beside) = solstone_core_location::place_at(
                        journal_root,
                        span.0 + (span.1 - span.0) / 2,
                        &mut locations,
                    )
                {
                    activity["place"] = json!({"device": beside.device, "words": beside.words});
                }
            }
            activities.push(activity);
            continue;
        }
        let (start, end, duration) = activity_times_and_duration(&item.record);
        let people =
            activity_people_across(std::iter::once(&item.record).chain(item.copies.iter()));
        let entities = activity_entities(&item.record);
        let title = activity_title(&item.record);

        let mut activity = json!({
            "title": title,
            "facet": item.facet_title,
            "day": item.day,
            "start": start,
            "end": end,
            "duration": duration,
            "participants": people.participants,
            "heard": people.heard,
            "mentioned": people.mentioned,
            "on_screen": people.on_screen,
            "entities": entities,
            "reference": reference,
            "zone": zone_str,
        });
        if let Some(kind) = activity_kind(&item.record) {
            activity["kind"] = json!(kind);
        }
        if let Some(stream) = item.record.get("stream").and_then(Value::as_str) {
            activity["source"] = json!(crate::dispatch::source_label(&sources, stream));
        }
        if people.unnamed_voices > 0 {
            activity["unnamed_voices"] = json!(people.unnamed_voices);
        }
        if !item.also_in.is_empty() {
            activity["also_in"] = json!(item.also_in);
        }
        if sees_calendar && let Some(span) = solstone_core_facets::activity_span(&item.record) {
            let calendar = calendars
                .entry(item.day.clone())
                .or_insert_with(|| solstone_core_facets::calendar_on_day(journal_root, &item.day));
            let mut scheduled = solstone_core_facets::calendar_beside(calendar, &item.day, span);
            // Naming the people on an invite takes the people grant too.
            if !snapshot.categories.contains(&AdmittedCategory::Entities) {
                for event in &mut scheduled {
                    if let Some(event) = event.as_object_mut() {
                        event.remove("people");
                    }
                }
            }
            if !scheduled.is_empty() {
                activity["scheduled"] = Value::Array(scheduled);
            }
            let location = locations.entry(item.day.clone()).or_insert_with(|| {
                solstone_core_location::DayBeside::read(journal_root, &item.day)
            });
            let stream = item.record.get("stream").and_then(Value::as_str);
            if let Some(beside) = location.beside(span, stream) {
                activity["place"] = json!({"device": beside.device, "words": beside.words});
            }
        }
        // A capture's span is worked out only when a body face could sit
        // beside it.
        let faces = body_near
            .entry(item.day.clone())
            .or_insert_with(|| body_lines_near(journal_root, &mut body, &item.day, &mut targets));
        if !faces.is_empty()
            && let Some(span) = solstone_core_location::body_records::capture_instants(
                journal_root,
                &item.day,
                &item.record,
            )
        {
            let beside = faces
                .iter()
                .filter(|(other, _)| solstone_core_location::body_records::half_cover(span, *other))
                .map(|(_, line)| line.clone())
                .collect::<Vec<_>>();
            if !beside.is_empty() {
                activity["beside"] = Value::Array(beside);
            }
        }
        activities.push(activity);
    }

    let next_cursor = if has_more {
        if let Some(last) = page.last() {
            Some(
                codec()
                    .mint(
                        principal.connection,
                        snapshot.generation,
                        ReferenceTarget::ActivityList(ActivityListCursor {
                            day: request.day.clone(),
                            from: Some(request.from.clone()),
                            to: Some(request.to.clone()),
                            facet: request.facet.clone(),
                            entity: request.entity.clone(),
                            last_day: last.sort_key.0.clone(),
                            last_facet: last.sort_key.1.clone(),
                            last_created_at: last.sort_key.2,
                            last_id: last.sort_key.3.clone(),
                        }),
                    )
                    .map_err(|_| DispatchError::Tool(ToolError::ReferenceNotFound))?,
            )
        } else {
            None
        }
    } else {
        None
    };

    let count = activities.len();
    let mut response_val = json!({ "activities": activities });
    if let Some(next_cursor) = next_cursor {
        response_val["next_cursor"] = json!(next_cursor);
    }
    if body_withheld {
        response_val["body_note"] = json!(crate::body_source::BODY_NOT_GRANTED);
        return Ok(Prepared::new(response_val, count, targets)
            .when_empty(crate::body_source::BODY_NOT_GRANTED));
    }

    Ok(Prepared::new(response_val, count, targets).when_empty(
        "No activities. This connection can see none that match. This is a complete answer, not an error.",
    ))
}

pub(crate) fn execute_get_sources(
    journal_root: &Path,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    request: ValidatedGetActivitySources,
) -> Result<Prepared, DispatchError> {
    let ReferenceTarget::Activity(activity_ref) = codec()
        .resolve(
            &request.reference,
            ReferenceKind::Activity,
            principal.connection,
            snapshot.generation,
        )
        .map_err(|_| DispatchError::Tool(ToolError::ReferenceNotFound))?
    else {
        return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
    };
    sources_of(
        journal_root,
        principal,
        snapshot,
        &activity_ref.into(),
        request.cursor.as_deref(),
    )
}

/// The verbatim sources of one activity, when this connection's own list
/// shows that activity now.
pub(crate) fn sources_of(
    journal_root: &Path,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    address: &ActivityAddress,
    cursor: Option<&str>,
) -> Result<Prepared, DispatchError> {
    // Hidden, superseded, out of scope and gone all read as not found.
    let seen = crate::may_see::activity_at(
        journal_root,
        snapshot,
        &address.facet,
        &address.day,
        &address.record_identity,
        address.file_digest.as_deref(),
    )
    .ok_or(DispatchError::Tool(ToolError::ReferenceNotFound))?;
    let place = seen.place;
    let file_digest = seen.file_digest;
    let rel_path = place.rel_path.clone();
    let record = match seen.record {
        // A body record's sources are its accounts, composed for this connection.
        crate::may_see::SeenRecord::Body(body) => {
            let face = crate::dispatch::body_face_for(
                journal_root,
                snapshot,
                &place.facet_dir,
                &place.day,
                &body,
            )?;
            // A hidden group is not found, as a missing one isn't: hidden by the
            // owner's hide of a member or of the day it shows under.
            if face.hidden {
                return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
            }
            let item = solstone_core_location::body_records::face_json(&face);
            let units = item
                .get("accounts")
                .cloned()
                .unwrap_or(Value::Array(Vec::new()));
            let mut targets = vec![rel_path];
            crate::body_source::name_windows(&mut targets, crate::body_source::face_windows(&face));
            return Ok(Prepared::new(json!({ "units": units }), 1, targets));
        }
        crate::may_see::SeenRecord::Activity(record) => record,
    };

    let segments = record.get("segments").and_then(Value::as_array);
    let Some(segment_vals) = segments else {
        return Ok(Prepared::new(json!({ "units": [] }), 0, vec![rel_path])
            .when_empty(crate::dispatch::ACTIVITY_SOURCES_EMPTY));
    };
    if segment_vals.is_empty() {
        return Ok(Prepared::new(json!({ "units": [] }), 0, vec![rel_path])
            .when_empty(crate::dispatch::ACTIVITY_SOURCES_EMPTY));
    }

    let activity_kind = record.get("activity").and_then(Value::as_str).unwrap_or("");

    // Each segment is read as what it is: a body window under body, any other
    // under transcripts.
    if crate::dispatch::segment_boundary(snapshot, None).is_none() {
        return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
    }
    let hides = crate::body_source::BodyHides::read(journal_root);
    let mut admitted_transcripts = Vec::new();
    let mut any_denied_or_unreadable = false;
    let mut segment_link_map = std::collections::BTreeMap::new();
    let candidates = activity_segment_candidates(journal_root, &rel_path, &record);
    let mut ordered = Vec::new();
    for seg_val in segment_vals {
        let Some(key) = seg_val.as_str() else {
            any_denied_or_unreadable = true;
            continue;
        };
        let matched: Vec<_> = candidates
            .iter()
            .filter(|segment| segment.name().to_str() == Some(key) || segment.key() == key)
            .cloned()
            .collect();
        if matched.is_empty() {
            any_denied_or_unreadable = true;
            continue;
        }
        ordered.extend(matched);
    }

    // The same voice gate the activity's Story reads names who spoke.
    let speakers = solstone_core_talent_runtime::agent_speakers(journal_root);
    // Source units are the activity's own source, word for word: no possible
    // name is inserted into them. Links to a segment still bind the version
    // this connection's `get_transcript` reads, possible names included.
    let possible_names =
        crate::possible_names::ConnectionPossibleNames::new(journal_root, snapshot);
    for segment in ordered {
        let Ok(identity) = segment.record_identity() else {
            any_denied_or_unreadable = true;
            continue;
        };
        let stream = identity.stream;
        let seg_key = identity.name;
        // A workout tile holds no transcript: nothing of it is a source.
        if solstone_core_segment::is_body_only_segment(&segment)
            && !crate::may_see::is_body_window(&segment)
        {
            continue;
        }
        // The same test a read of this segment alone makes.
        let Some(left_out) = crate::may_see::segment_shown(
            journal_root,
            snapshot,
            &hides,
            None,
            &place.day,
            stream,
            seg_key,
            &segment,
        ) else {
            any_denied_or_unreadable = true;
            continue;
        };

        #[cfg(feature = "test-hooks")]
        crate::source_freeze_barrier::checkpoint(crate::source_freeze_barrier::FreezePoint::Before);
        let allowed_before = crate::source_release::check_segment_transcript_release(
            journal_root,
            &place.day,
            stream,
            seg_key,
        );
        if !allowed_before {
            #[cfg(feature = "test-hooks")]
            crate::source_freeze_barrier::checkpoint(
                crate::source_freeze_barrier::FreezePoint::After,
            );
            any_denied_or_unreadable = true;
            continue;
        }

        let mut segment_lines = Vec::new();
        let mut page_cursor = None;
        let mut read_failed = false;
        let mut segment_untimed = false;
        let mut segment_version = None;

        loop {
            match solstone_core_transcripts::read_segment_transcript_page_leaving_out(
                &segment,
                page_cursor.as_ref(),
                speakers.as_ref(),
                None,
                &left_out,
            ) {
                Ok(page) => {
                    segment_untimed = page.untimed;
                    segment_version = Some(page.version.fingerprint());
                    for entry in page.entries {
                        segment_lines.push(SourcedLine {
                            channel: entry.channel,
                            text: entry.text,
                        });
                    }
                    if page.next.is_some() {
                        page_cursor = page.next;
                    } else {
                        break;
                    }
                }
                Err(_) => {
                    read_failed = true;
                    break;
                }
            }
        }

        let allowed_after = crate::source_release::check_segment_transcript_release(
            journal_root,
            &place.day,
            stream,
            seg_key,
        );
        #[cfg(feature = "test-hooks")]
        crate::source_freeze_barrier::checkpoint(crate::source_freeze_barrier::FreezePoint::After);

        if !allowed_after || read_failed {
            any_denied_or_unreadable = true;
            continue;
        }

        if segment_lines.is_empty() {
            continue;
        }

        let heading = if segment_untimed {
            None
        } else {
            match format_segment_heading(&place.day, seg_key) {
                Some(heading) => Some(heading),
                None => {
                    any_denied_or_unreadable = true;
                    continue;
                }
            }
        };
        let Some(version) = segment_version else {
            any_denied_or_unreadable = true;
            continue;
        };

        let formatted = compose_segment_source(heading.as_deref(), &segment_lines);
        let mut dummy_record = record.clone();
        dummy_record.insert("segments".to_string(), json!([seg_key]));
        // This candidate's identity, so a streamless record's other matches
        // do not take the link.
        dummy_record.insert("stream".to_string(), json!(stream));
        let mut single_subject = [LinkSubject {
            path: &rel_path,
            idx: 0,
            text: "",
            segments: Vec::new(),
            more_segments: false,
            activity_record: Some(&dummy_record),
        }];
        let mut checker = LiveTranscriptPages::with_speakers(
            speakers.clone(),
            std::sync::Arc::clone(&possible_names),
        );
        let mut response_checks = 0;
        issue_segment_links(
            journal_root,
            principal,
            snapshot,
            &mut single_subject,
            48,
            48,
            &mut response_checks,
            &mut checker,
        );
        if let Some(link) = single_subject[0].segments.first().cloned() {
            segment_link_map.insert(admitted_transcripts.len(), link);
        }
        admitted_transcripts.push(AdmittedTranscript {
            version,
            formatted,
            untimed: segment_untimed,
        });
    }

    if admitted_transcripts.is_empty() {
        if any_denied_or_unreadable {
            return Err(DispatchError::Tool(ToolError::FileUnreadable));
        } else {
            let target = format!(
                "facet:{}/activity:{}",
                place.facet_id, address.record_identity
            );
            return Ok(Prepared::new(json!({ "units": [] }), 0, vec![target])
                .when_empty(crate::dispatch::ACTIVITY_SOURCES_EMPTY));
        }
    }

    let mut all_units = Vec::new();
    for (index, admitted) in admitted_transcripts.iter().enumerate() {
        for mut unit in solstone_core_talent_runtime::source_passages::source_units(
            &admitted.formatted,
            activity_kind,
        ) {
            // The rendered text keeps the native projection, including a
            // synthetic [00:00:00] on an untimed chat line. The page flag is
            // the qualifier that clock is not a captured time.
            unit.untimed = admitted.untimed;
            all_units.push((index, unit));
        }
    }
    let source_binding = admitted_source_binding(&admitted_transcripts);

    let start_unit = if let Some(cursor_token) = cursor {
        let ReferenceTarget::Activity(c) = codec()
            .resolve(
                cursor_token,
                ReferenceKind::Activity,
                principal.connection,
                snapshot.generation,
            )
            .map_err(|_| DispatchError::Tool(ToolError::ReferenceNotFound))?
        else {
            return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
        };
        // A cursor keeps its pins: the same activity, in a day file and with
        // sources that haven't changed since the page before.
        if c.facet_id != place.facet_id
            || c.day != place.day
            || c.record_identity != address.record_identity
            || c.file_digest != file_digest
            || c.source_binding.as_deref() != Some(source_binding.as_str())
        {
            return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
        }
        c.unit_index.unwrap_or(0)
    } else if address.unit_index.is_some()
        && address.source_binding.as_deref() != Some(source_binding.as_str())
    {
        return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
    } else {
        address.unit_index.unwrap_or(0)
    };

    let mut page_units = Vec::new();
    let mut page_bytes = 0usize;
    let mut next_unit_index = None;

    for (idx, (segment_index, unit)) in all_units.iter().enumerate().skip(start_unit) {
        let unit_bytes = unit.text.len();
        if !page_units.is_empty()
            && (page_units.len() >= MAX_TRANSCRIPT_PAGE_ITEMS
                || page_bytes + unit_bytes > MAX_TRANSCRIPT_PAGE_BYTES)
        {
            next_unit_index = Some(idx);
            break;
        }

        let source_link = segment_link_map.get(segment_index).cloned();

        let mut unit_json = json!({
            "text": unit.text,
            "kind": unit.kind.as_str(),
        });
        if let Some(ref ts) = unit.timestamp {
            unit_json["timestamp"] = json!(ts);
        }
        if unit.untimed {
            unit_json["untimed"] = json!(true);
        }
        if let Some(link) = source_link {
            unit_json["source_link"] = link;
        }
        page_units.push(unit_json);
        page_bytes += unit_bytes;
    }

    let next_cursor = if let Some(next_idx) = next_unit_index {
        Some(
            codec()
                .mint(
                    principal.connection,
                    snapshot.generation,
                    ReferenceTarget::Activity(ActivityReference {
                        facet_id: place.facet_id.clone(),
                        facet_dir: place.facet_dir.clone(),
                        day: place.day.clone(),
                        record_identity: address.record_identity.clone(),
                        file_digest: file_digest.clone(),
                        unit_index: Some(next_idx),
                        source_binding: Some(source_binding.clone()),
                    }),
                )
                .map_err(|_| DispatchError::Tool(ToolError::ReferenceNotFound))?,
        )
    } else {
        None
    };

    let count = page_units.len();
    let mut response_val = json!({ "units": page_units });
    if any_denied_or_unreadable {
        response_val["unreadable"] = Value::Bool(true);
    }
    if let Some(next_cursor) = next_cursor {
        response_val["next_cursor"] = json!(next_cursor);
    }

    let target = format!(
        "facet:{}/activity:{}",
        place.facet_id, address.record_identity
    );
    Ok(Prepared::new(response_val, count, vec![target])
        .when_empty(crate::dispatch::ACTIVITY_SOURCES_EMPTY))
}

struct SourcedLine {
    channel: &'static str,
    text: String,
}

struct AdmittedTranscript {
    version: u64,
    formatted: String,
    untimed: bool,
}

fn admitted_source_binding(admitted: &[AdmittedTranscript]) -> String {
    let mut hasher = Sha256::new();
    for source in admitted {
        hasher.update(source.version.to_le_bytes());
        hasher.update(source.formatted.as_bytes());
    }
    crate::activity_admit::hex_encode(&hasher.finalize())
}

/// One segment's admitted lines, grouped by the channel the reader already
/// assigned. An untimed segment omits the clock heading, so its units carry no
/// timestamp, and each of those units says `untimed`. Each segment is composed
/// alone: an untimed segment must not sit inside a timed segment's clock heading.
fn compose_segment_source(heading: Option<&str>, lines: &[SourcedLine]) -> String {
    let mut out = String::new();
    if let Some(heading) = heading {
        out.push_str(heading);
        out.push_str("\n\n");
    }
    let mut index = 0;
    while index < lines.len() {
        let channel = lines[index].channel;
        if !out.is_empty() && !out.ends_with("\n\n") {
            if !out.ends_with('\n') {
                out.push('\n');
            }
            out.push('\n');
        }
        out.push_str("### ");
        out.push_str(channel);
        out.push_str("\n\n");
        let start = index;
        while index < lines.len() && lines[index].channel == channel {
            if index > start {
                out.push('\n');
            }
            out.push_str(&lines[index].text);
            index += 1;
        }
        out.push('\n');
    }
    out
}

fn format_segment_heading(day: &str, seg_key: &str) -> Option<String> {
    let times = solstone_core_format::segment::segment_parse(seg_key)?;
    let length = seg_key.split_once('_')?.1.parse::<i64>().ok()?;
    let date = chrono::NaiveDate::parse_from_str(day, "%Y%m%d")
        .or_else(|_| chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d"))
        .ok()?;
    let start = date.and_hms_opt(times.hour as u32, times.minute as u32, times.second as u32)?;
    let end = start + chrono::Duration::seconds(length);
    Some(format!(
        "## {} - {}",
        start.format("%Y-%m-%d %H:%M:%S"),
        end.format("%H:%M:%S")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_agent_never_reads_what_the_owner_retracted_or_wrote_over() {
        let record = |edits: Value| {
            serde_json::json!({"id":"a","description":"kept","edits":edits})
                .as_object()
                .unwrap()
                .clone()
        };
        let cleaned = clean_activity_record_for_wire(record(serde_json::json!([
            {"timestamp":"t1","actor":"owner:update","fields":["description"],"set":{"description":"retracted"},"before":{"description":"old"},"removed":"t3"},
            {"timestamp":"t2","actor":"owner:update","fields":["description"],"set":{"description":"kept"},"before":{"description":"old"}},
            {"timestamp":"t4","actor":"owner:lines","fields":[],"note":"","lines":[{"stream":"mic","segment":"120000_300","source":"audio","sentence_id":1,"set_at":"s","prior":null}],"speaker":{"kind":"unknown"}},
        ])));
        assert_eq!(
            Value::Object(cleaned),
            Value::Object(record(serde_json::json!([
                {"timestamp":"t2","actor":"owner:update","fields":["description"]},
                {"timestamp":"t4","actor":"owner:lines","fields":[],"note":""},
            ])))
        );
        let plain = record(serde_json::json!([
            {"timestamp":"t0","actor":"cli:create","fields":["title"],"note":"created"},
        ]));
        assert_eq!(clean_activity_record_for_wire(plain.clone()), plain);
    }

    #[test]
    fn activity_heard_and_role_classification() {
        let record = json!({
            "participation": [
                {"name": "Alice", "role": "attendee"},
                {"name": "Bob", "role": "heard"},
                {"name": "Carol", "role": "mentioned"},
                {"name": "Dave", "role": "visible"},
                {"name": "Eve", "role": "organizer"},
                {"name": "Frank"},
            ]
        });
        let people = activity_people(record.as_object().unwrap());
        assert_eq!(people.participants, vec!["Alice"]);
        assert_eq!(people.heard, vec!["Bob"]);
        assert_eq!(people.mentioned, vec!["Carol", "Eve", "Frank"]);
        assert_eq!(people.on_screen, vec!["Dave"]);
    }
}
