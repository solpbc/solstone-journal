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
    DispatchError, DispatchPrincipal, Prepared, available_facets, boundary, codec, entity_name,
    fold_literal, live_allows, read_scoped_entities, segment_assignment_probe_path,
};
use crate::permissions::ConnectionReadSnapshot;
use crate::references::{
    ActivityListCursor, ActivityReference, ReferenceKind, ReferenceTarget, is_framed_entity_token,
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
    #[serde(default)]
    from: Option<String>,
    #[serde(default)]
    to: Option<String>,
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
        || !optional_string_within_limit(&params.from, MAX_DAY_BYTES)
        || !optional_string_within_limit(&params.to, MAX_DAY_BYTES)
        || !optional_string_within_limit(&params.facet, MAX_FACET_BYTES)
        || !optional_string_within_limit(&params.cursor, MAX_OPAQUE_REFERENCE_BYTES)
    {
        return Err(ToolError::InvalidInput);
    }

    if params.day.is_some() && (params.from.is_some() || params.to.is_some()) {
        return Err(ToolError::InvalidInput);
    }

    let normalized_day = super::normalize_day(params.day)?;
    let normalized_from = super::normalize_day(params.from)?;
    let normalized_to = super::normalize_day(params.to)?;

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
        return Err(ToolError::InvalidInput);
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

pub(crate) fn activity_participants(record: &Map<String, Value>) -> Vec<String> {
    record
        .get("participation")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_object)
                .filter_map(|p| p.get("name").and_then(Value::as_str))
                .map(str::trim)
                .filter(|n| !n.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
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
    }

    let mut collected = Vec::new();
    let mut targets = Vec::new();

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

            for ((record, trimmed), normalized_row) in raw_rows.into_iter().zip(normalized) {
                // Same predicate as load_activity_records(..., include_hidden: false).
                // Identity stays the raw line, not the normalized object.
                let hidden = normalized_row
                    .get("hidden")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if hidden || solstone_core_facets::is_retired_plan(&record) {
                    continue;
                }
                let entities = activity_entities(&record);
                if let Some(ref filter) = entity_filter {
                    let matched = match filter {
                        EntityFilter::Token {
                            entity_id,
                            folded_name,
                        } => entities
                            .iter()
                            .any(|e| e == entity_id || fold_literal(e) == *folded_name),
                        EntityFilter::Literal(target) => entities
                            .iter()
                            .any(|entity| fold_literal(entity).contains(target)),
                    };
                    if !matched {
                        continue;
                    }
                }
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
                });
            }
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

    // The calendar belongs to no facet: only a connection that already reads
    // the whole journal's transcripts, where calendar notes are, sees what it
    // had scheduled during an activity.
    let sees_calendar = matches!(snapshot.scope, ConnectionScope::WholeJournal)
        && snapshot.categories.contains(&AdmittedCategory::Transcripts);
    let mut calendars = BTreeMap::new();

    let mut activities = Vec::new();
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

        let (start, end, duration) = activity_times_and_duration(&item.record);
        let participants = activity_participants(&item.record);
        let entities = activity_entities(&item.record);
        let title = activity_title(&item.record);

        let mut activity = json!({
            "title": title,
            "facet": item.facet_title,
            "day": item.day,
            "start": start,
            "end": end,
            "duration": duration,
            "participants": participants,
            "entities": entities,
            "reference": reference,
            "zone": zone_str,
        });
        if sees_calendar && let Some(span) = solstone_core_facets::activity_span(&item.record) {
            let calendar = calendars
                .entry(item.day.clone())
                .or_insert_with(|| solstone_core_facets::calendar_on_day(journal_root, &item.day));
            let scheduled = solstone_core_facets::calendar_beside(calendar, &item.day, span);
            if !scheduled.is_empty() {
                activity["scheduled"] = Value::Array(scheduled);
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

    let rel_path = activity_ref.relative_path().map_err(DispatchError::Tool)?;
    let facet_boundary = boundary(snapshot, None, None);
    if !live_allows(journal_root, &facet_boundary, &rel_path, None) {
        return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
    }

    let Some(bytes) = crate::activity_admit::read_contained_source(journal_root, &rel_path) else {
        return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
    };
    let file_digest = crate::activity_admit::hex_encode(&Sha256::digest(&bytes));
    if file_digest != activity_ref.file_digest {
        return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
    }
    let Ok(file_text) = String::from_utf8(bytes) else {
        return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
    };
    let Some((record, _)) =
        crate::activity_admit::record_by_identity(&file_text, &activity_ref.record_identity)
    else {
        return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
    };

    let segments = record.get("segments").and_then(Value::as_array);
    let Some(segment_vals) = segments else {
        return Ok(Prepared::new(json!({ "units": [] }), 0, vec![rel_path])
            .when_empty("This activity has no transcript sources."));
    };
    if segment_vals.is_empty() {
        return Ok(Prepared::new(json!({ "units": [] }), 0, vec![rel_path])
            .when_empty("This activity has no transcript sources."));
    }

    let activity_kind = record.get("activity").and_then(Value::as_str).unwrap_or("");

    let transcript_boundary = boundary(snapshot, Some(AdmittedCategory::Transcripts), None);
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
    for segment in ordered {
        let Ok(identity) = segment.record_identity() else {
            any_denied_or_unreadable = true;
            continue;
        };
        let stream = identity.stream;
        let seg_key = identity.name;
        let probe_path = segment_assignment_probe_path(&activity_ref.day, stream, seg_key);
        if !live_allows(
            journal_root,
            &transcript_boundary,
            &probe_path,
            Some(stream),
        ) {
            any_denied_or_unreadable = true;
            continue;
        }

        #[cfg(feature = "test-hooks")]
        crate::source_freeze_barrier::checkpoint(crate::source_freeze_barrier::FreezePoint::Before);
        let allowed_before = crate::source_release::check_segment_transcript_release(
            journal_root,
            &activity_ref.day,
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
        let mut cursor = None;
        let mut read_failed = false;
        let mut segment_untimed = false;
        let mut segment_version = None;

        loop {
            match solstone_core_transcripts::read_segment_transcript_page(
                &segment,
                cursor.as_ref(),
                speakers.as_ref(),
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
                        cursor = page.next;
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
            &activity_ref.day,
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
            match format_segment_heading(&activity_ref.day, seg_key) {
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
        let mut checker = LiveTranscriptPages::with_speakers(speakers.clone());
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
                activity_ref.facet_id, activity_ref.record_identity
            );
            return Ok(Prepared::new(json!({ "units": [] }), 0, vec![target])
                .when_empty("This activity has no transcript sources."));
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

    let start_unit = if let Some(ref cursor_token) = request.cursor {
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
        if c.facet_id != activity_ref.facet_id
            || c.facet_dir != activity_ref.facet_dir
            || c.day != activity_ref.day
            || c.record_identity != activity_ref.record_identity
            || c.file_digest != activity_ref.file_digest
            || c.source_binding.as_deref() != Some(source_binding.as_str())
        {
            return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
        }
        c.unit_index.unwrap_or(0)
    } else if activity_ref.unit_index.is_some()
        && activity_ref.source_binding.as_deref() != Some(source_binding.as_str())
    {
        return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
    } else {
        activity_ref.unit_index.unwrap_or(0)
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
                        facet_id: activity_ref.facet_id.clone(),
                        facet_dir: activity_ref.facet_dir.clone(),
                        day: activity_ref.day.clone(),
                        record_identity: activity_ref.record_identity.clone(),
                        file_digest: activity_ref.file_digest.clone(),
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
        activity_ref.facet_id, activity_ref.record_identity
    );
    Ok(Prepared::new(response_val, count, vec![target])
        .when_empty("This activity has no transcript sources."))
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
