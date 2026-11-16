// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use solstone_core_entity_matching::normalize_resolution_query;
use solstone_core_journal_io::paths::{PathOrDay, iter_segments_skipping_unreadable_streams};
use solstone_core_journal_link::Place;
use unicode_general_category::{GeneralCategory, get_general_category};
use unicode_normalization::UnicodeNormalization;

use crate::dispatch::{DispatchError, DispatchPrincipal, Prepared, reference_fingerprint};
use crate::permissions::ConnectionReadSnapshot;
use crate::references::{ReferenceKind, ReferenceTarget};
use crate::tools::{MAX_OPAQUE_REFERENCE_BYTES, ToolError};

use super::store::{
    SuggestionRecord, hold_suggestions_lock, list_records, read_settings, write_new_record,
    write_settings,
};
use super::{DAILY_AGENT_CAP, PENDING_AGENT_CAP, SUGGESTION_RESULT_SCHEMA, SUGGESTION_SCHEMA};

#[derive(Debug, Clone)]
pub struct ValidatedSuggestCorrection {
    pub kind: String,
    pub target: ValidatedTarget,
    pub value: ValidatedValue,
    pub basis: String,
    pub reason: String,
    pub operation_id: String,
    pub raw_target: Value,
    pub raw_value: Value,
}

#[derive(Debug, Clone)]
pub enum ValidatedTarget {
    Activity {
        reference: String,
    },
    Lines {
        segment: String,
        lines: Vec<LineSelector>,
    },
    Entity {
        reference: String,
    },
}

#[derive(Debug, Clone)]
pub struct LineSelector {
    pub time: String,
    pub starts_with: String,
}

#[derive(Debug, Clone)]
pub enum ValidatedValue {
    Recording,
    WhatHappened { text: String },
    WhoSaidIt { speaker: ValidatedSpeaker },
    Name { text: String, mode: String },
}

#[derive(Debug, Clone)]
pub enum ValidatedSpeaker {
    Owner,
    SomeoneElse,
    Person { reference: String },
}

pub fn normalize_suggestion_text(text: &str) -> String {
    let nfkc = text.nfkc().collect::<String>();
    let stripped: String = nfkc
        .chars()
        .filter(|&c| {
            if c.is_ascii_punctuation() {
                return false;
            }
            let cat = get_general_category(c);
            !matches!(
                cat,
                GeneralCategory::ConnectorPunctuation
                    | GeneralCategory::DashPunctuation
                    | GeneralCategory::OpenPunctuation
                    | GeneralCategory::ClosePunctuation
                    | GeneralCategory::InitialPunctuation
                    | GeneralCategory::FinalPunctuation
                    | GeneralCategory::OtherPunctuation
            )
        })
        .collect();
    let collapsed = stripped.split_whitespace().collect::<Vec<_>>().join(" ");
    caseless::default_case_fold_str(&collapsed)
}

pub fn near_match(a: &str, b: &str) -> bool {
    let na = normalize_suggestion_text(a);
    let nb = normalize_suggestion_text(b);
    if na == nb {
        return true;
    }
    if na.chars().count() >= 4 && nb.chars().count() >= 4 {
        solstone_core_entity_matching::token_sort_ratio(&na, &nb) >= 90.0
    } else {
        false
    }
}

pub fn sanitize_label(label: &str) -> String {
    let mut safe = String::new();
    for character in label.trim().chars() {
        if character.is_control() || matches!(character, '<' | '>') {
            continue;
        }
        if safe.len() + character.len_utf8() > 256 {
            break;
        }
        safe.push(character);
    }
    if safe.trim().is_empty() {
        "connected agent".into()
    } else {
        safe
    }
}

pub(crate) fn validate(arguments: Option<&Value>) -> Result<ValidatedSuggestCorrection, ToolError> {
    let Some(Value::Object(map)) = arguments else {
        return Err(ToolError::InvalidInput);
    };

    let allowed_keys = ["basis", "kind", "operation_id", "reason", "target", "value"];
    if map.len() != allowed_keys.len() || !allowed_keys.iter().all(|k| map.contains_key(*k)) {
        return Err(ToolError::InvalidInput);
    }

    let operation_id = map
        .get("operation_id")
        .and_then(Value::as_str)
        .ok_or(ToolError::InvalidInput)?;
    solstone_core_format::agent_memory::validate_operation_id(operation_id)
        .map_err(|_| ToolError::InvalidInput)?;

    let basis = map
        .get("basis")
        .and_then(Value::as_str)
        .ok_or(ToolError::InvalidInput)?;
    if !matches!(
        basis,
        "owner_said" | "my_own_work" | "journal_shows" | "outside_source"
    ) {
        return Err(ToolError::InvalidInput);
    }

    let reason = map
        .get("reason")
        .and_then(Value::as_str)
        .ok_or(ToolError::InvalidInput)?;
    let reason_len = reason.chars().count();
    if !(1..=200).contains(&reason_len) || reason.trim().is_empty() {
        return Err(ToolError::InvalidInput);
    }

    let kind = map
        .get("kind")
        .and_then(Value::as_str)
        .ok_or(ToolError::InvalidInput)?;
    let raw_target = map.get("target").ok_or(ToolError::InvalidInput)?;
    let raw_value = map.get("value").ok_or(ToolError::InvalidInput)?;

    let target_obj = raw_target.as_object().ok_or(ToolError::InvalidInput)?;
    let value_obj = raw_value.as_object().ok_or(ToolError::InvalidInput)?;

    let (validated_target, validated_value) = match kind {
        "recording" => {
            if target_obj.len() != 1 || !target_obj.contains_key("reference") {
                return Err(ToolError::InvalidInput);
            }
            let activity = target_obj
                .get("reference")
                .and_then(Value::as_str)
                .ok_or(ToolError::InvalidInput)?;
            if activity.is_empty() || activity.len() > MAX_OPAQUE_REFERENCE_BYTES {
                return Err(ToolError::InvalidInput);
            }
            if !value_obj.is_empty() {
                return Err(ToolError::InvalidInput);
            }
            (
                ValidatedTarget::Activity {
                    reference: activity.to_owned(),
                },
                ValidatedValue::Recording,
            )
        }
        "what_happened" => {
            if target_obj.len() != 1 || !target_obj.contains_key("reference") {
                return Err(ToolError::InvalidInput);
            }
            let activity = target_obj
                .get("reference")
                .and_then(Value::as_str)
                .ok_or(ToolError::InvalidInput)?;
            if activity.is_empty() || activity.len() > MAX_OPAQUE_REFERENCE_BYTES {
                return Err(ToolError::InvalidInput);
            }
            if value_obj.len() != 1 || !value_obj.contains_key("text") {
                return Err(ToolError::InvalidInput);
            }
            let text = value_obj
                .get("text")
                .and_then(Value::as_str)
                .ok_or(ToolError::InvalidInput)?;
            let text_len = text.chars().count();
            if !(1..=600).contains(&text_len) || text.trim().is_empty() {
                return Err(ToolError::InvalidInput);
            }
            (
                ValidatedTarget::Activity {
                    reference: activity.to_owned(),
                },
                ValidatedValue::WhatHappened {
                    text: text.to_owned(),
                },
            )
        }
        "who_said_it" => {
            if target_obj.len() != 2
                || !target_obj.contains_key("reference")
                || !target_obj.contains_key("lines")
            {
                return Err(ToolError::InvalidInput);
            }
            let segment = target_obj
                .get("reference")
                .and_then(Value::as_str)
                .ok_or(ToolError::InvalidInput)?;
            if segment.is_empty() || segment.len() > MAX_OPAQUE_REFERENCE_BYTES {
                return Err(ToolError::InvalidInput);
            }
            let lines_arr = target_obj
                .get("lines")
                .and_then(Value::as_array)
                .ok_or(ToolError::InvalidInput)?;
            if lines_arr.is_empty() || lines_arr.len() > 20 {
                return Err(ToolError::InvalidInput);
            }
            let mut lines = Vec::new();
            for item in lines_arr {
                let item_obj = item.as_object().ok_or(ToolError::InvalidInput)?;
                if item_obj.len() != 2
                    || !item_obj.contains_key("time")
                    || !item_obj.contains_key("starts_with")
                {
                    return Err(ToolError::InvalidInput);
                }
                let time = item_obj
                    .get("time")
                    .and_then(Value::as_str)
                    .ok_or(ToolError::InvalidInput)?;
                if !is_valid_time_str(time) {
                    return Err(ToolError::InvalidInput);
                }
                let starts_with = item_obj
                    .get("starts_with")
                    .and_then(Value::as_str)
                    .ok_or(ToolError::InvalidInput)?;
                if starts_with.is_empty() {
                    return Err(ToolError::InvalidInput);
                }
                lines.push(LineSelector {
                    time: time.to_owned(),
                    starts_with: starts_with.to_owned(),
                });
            }

            if value_obj.len() != 1 || !value_obj.contains_key("speaker") {
                return Err(ToolError::InvalidInput);
            }
            let speaker_val = value_obj.get("speaker").ok_or(ToolError::InvalidInput)?;
            let speaker = match speaker_val {
                Value::String(s) if s == "owner" => ValidatedSpeaker::Owner,
                Value::String(s) if s == "someone_else" => ValidatedSpeaker::SomeoneElse,
                Value::String(person_ref) => {
                    if person_ref.is_empty() || person_ref.len() > MAX_OPAQUE_REFERENCE_BYTES {
                        return Err(ToolError::InvalidInput);
                    }
                    ValidatedSpeaker::Person {
                        reference: person_ref.to_owned(),
                    }
                }
                _ => return Err(ToolError::InvalidInput),
            };

            (
                ValidatedTarget::Lines {
                    segment: segment.to_owned(),
                    lines,
                },
                ValidatedValue::WhoSaidIt { speaker },
            )
        }
        "name" => {
            if target_obj.len() != 1 || !target_obj.contains_key("reference") {
                return Err(ToolError::InvalidInput);
            }
            let entity = target_obj
                .get("reference")
                .and_then(Value::as_str)
                .ok_or(ToolError::InvalidInput)?;
            if entity.is_empty() || entity.len() > MAX_OPAQUE_REFERENCE_BYTES {
                return Err(ToolError::InvalidInput);
            }
            if value_obj.len() != 2
                || !value_obj.contains_key("text")
                || !value_obj.contains_key("mode")
            {
                return Err(ToolError::InvalidInput);
            }
            let text = value_obj
                .get("text")
                .and_then(Value::as_str)
                .ok_or(ToolError::InvalidInput)?;
            let text_len = text.chars().count();
            if !(1..=200).contains(&text_len) || text.trim().is_empty() {
                return Err(ToolError::InvalidInput);
            }
            let mode = value_obj
                .get("mode")
                .and_then(Value::as_str)
                .ok_or(ToolError::InvalidInput)?;
            if !matches!(mode, "replace" | "also_known_as") {
                return Err(ToolError::InvalidInput);
            }
            (
                ValidatedTarget::Entity {
                    reference: entity.to_owned(),
                },
                ValidatedValue::Name {
                    text: text.to_owned(),
                    mode: mode.to_owned(),
                },
            )
        }
        _ => return Err(ToolError::InvalidInput),
    };

    Ok(ValidatedSuggestCorrection {
        kind: kind.to_owned(),
        target: validated_target,
        value: validated_value,
        basis: basis.to_owned(),
        reason: reason.to_owned(),
        operation_id: operation_id.to_owned(),
        raw_target: raw_target.clone(),
        raw_value: raw_value.clone(),
    })
}

fn is_valid_time_str(time: &str) -> bool {
    let bytes = time.as_bytes();
    if bytes.len() != 8 || bytes[2] != b':' || bytes[5] != b':' {
        return false;
    }
    let h = (bytes[0] as char)
        .to_digit(10)
        .and_then(|h1| (bytes[1] as char).to_digit(10).map(|h2| h1 * 10 + h2));
    let m = (bytes[3] as char)
        .to_digit(10)
        .and_then(|m1| (bytes[4] as char).to_digit(10).map(|m2| m1 * 10 + m2));
    let s = (bytes[6] as char)
        .to_digit(10)
        .and_then(|s1| (bytes[7] as char).to_digit(10).map(|s2| s1 * 10 + s2));
    matches!((h,m,s),(Some(h),Some(m),Some(s)) if h<24 && m<60 && s<60)
}

pub(crate) fn recorded_arguments(request: &ValidatedSuggestCorrection) -> Map<String, Value> {
    let mut arguments = Map::new();
    arguments.insert("kind".to_owned(), Value::String(request.kind.clone()));
    arguments.insert("basis".to_owned(), Value::String(request.basis.clone()));
    arguments.insert("reason".to_owned(), Value::String(request.reason.clone()));
    arguments.insert(
        "operation_id".to_owned(),
        Value::String(request.operation_id.clone()),
    );
    match &request.target {
        ValidatedTarget::Activity { reference } => {
            arguments.insert(
                "activity".to_owned(),
                reference_fingerprint(reference).into(),
            );
        }
        ValidatedTarget::Lines { segment, lines } => {
            arguments.insert("segment".to_owned(), reference_fingerprint(segment).into());
            arguments.insert("lines_count".to_owned(), lines.len().into());
        }
        ValidatedTarget::Entity { reference } => {
            arguments.insert("entity".to_owned(), reference_fingerprint(reference).into());
        }
    }
    arguments
}

fn existing_result(
    journal_root: &Path,
    principal: DispatchPrincipal<'_>,
    existing: &SuggestionRecord,
    operation_id: &str,
    exact_retry: bool,
) -> Prepared {
    let state = super::owner::effective_status(journal_root, existing);
    if exact_retry && state == "pending" {
        return Prepared::new(existing.tool_result.clone(), 1, Vec::new());
    }
    let undone = state == "undone";
    let mut result = json!({
        "schema": SUGGESTION_RESULT_SCHEMA,
        "operation_id": operation_id,
        "status": if undone { "owner_answered" } else { "already_exists" },
        "existing_status": state,
        "message": if undone { "the owner accepted this correction and then undid it" }
            else { "a similar suggestion already exists" },
    });
    if existing.authority_id == principal.connection {
        result["suggestion_id"] = json!(existing.id);
        if let Some(link) = existing.tool_result.get("link") {
            result["link"] = link.clone();
        }
    }
    Prepared::new(result, 1, Vec::new())
}

pub(crate) fn execute(
    root: &Path,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    request: ValidatedSuggestCorrection,
    now: DateTime<Utc>,
) -> Result<Prepared, DispatchError> {
    let line = if let ValidatedTarget::Lines { segment, .. } = &request.target {
        if crate::names::is_name(segment) {
            crate::names::resolve_segment(root, segment)
                .map(|address| (address.day, address.stream, address.segment))
        } else {
            match crate::dispatch::codec().resolve(
                segment,
                ReferenceKind::Segment,
                principal.connection,
                snapshot.generation,
            ) {
                Ok(ReferenceTarget::Segment(reference)) => {
                    Some((reference.day, reference.stream, reference.segment))
                }
                _ => None,
            }
        }
    } else {
        None
    };
    let operation = request.operation_id.clone();
    let released = || {
        line.as_ref().is_none_or(|(day, stream, segment)| {
            crate::source_release::check_segment_transcript_release(root, day, stream, segment)
        })
    };
    let refused = || {
        Ok(Prepared::new(
            json!({"schema":SUGGESTION_RESULT_SCHEMA,"status":"refused",
        "operation_id":operation,"message":"the target is no longer available to this connection"}),
            1,
            Vec::new(),
        ))
    };
    if !released() {
        return refused();
    }
    let result = execute_admitted(root, principal, snapshot, request, now);
    if !released() {
        return refused();
    }
    result
}

fn execute_admitted(
    journal_root: &Path,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    request: ValidatedSuggestCorrection,
    now: DateTime<Utc>,
) -> Result<Prepared, DispatchError> {
    // 1. Canonical request hash
    let mut canonical_map = Map::new();
    canonical_map.insert("kind".to_owned(), Value::String(request.kind.clone()));
    canonical_map.insert("target".to_owned(), request.raw_target.clone());
    canonical_map.insert("value".to_owned(), request.raw_value.clone());
    canonical_map.insert("basis".to_owned(), Value::String(request.basis.clone()));
    canonical_map.insert("reason".to_owned(), Value::String(request.reason.clone()));
    canonical_map.insert(
        "operation_id".to_owned(),
        Value::String(request.operation_id.clone()),
    );
    let canonical_bytes = serde_json::to_vec(&Value::Object(canonical_map))
        .map_err(|_| DispatchError::Tool(ToolError::InvalidInput))?;
    let mut hasher = Sha256::new();
    hasher.update(&canonical_bytes);
    let request_sha256 = format!("{:x}", hasher.finalize());

    // 2. Lock suggestions store
    let _lock = hold_suggestions_lock(journal_root)
        .map_err(|_| DispatchError::Tool(ToolError::FileUnreadable))?;
    let records =
        list_records(journal_root).map_err(|_| DispatchError::Tool(ToolError::FileUnreadable))?;

    // Replay & Conflict check
    let replay = records
        .iter()
        .find(|r| r.authority_id == principal.connection && r.operation_id == request.operation_id);
    if replay.is_some_and(|existing| existing.request_sha256 != request_sha256) {
        return Err(DispatchError::Tool(ToolError::SuggestionOperationConflict));
    }

    // 3. Enabled check
    let mut settings =
        read_settings(journal_root).map_err(|_| DispatchError::Tool(ToolError::FileUnreadable))?;
    let is_enabled = settings
        .enabled
        .get(principal.connection)
        .copied()
        .unwrap_or(true);
    if !is_enabled && replay.is_none() {
        return Ok(Prepared::new(
            json!({
                "schema": SUGGESTION_RESULT_SCHEMA,
                "status": "suggestions_off",
                "operation_id": request.operation_id,
                "message": "suggestions are disabled for this connection",
            }),
            1,
            Vec::new(),
        ));
    }

    // 4. Target & Speaker Admission
    let codec = crate::dispatch::codec();
    let refusal = Ok(Prepared::new(
        json!({
            "schema": SUGGESTION_RESULT_SCHEMA,
            "status": "refused",
            "operation_id": request.operation_id,
            "message": "suggestion refused",
        }),
        1,
        Vec::new(),
    ));

    match &request.kind.as_str() {
        &"recording" | &"what_happened" => {
            let ValidatedTarget::Activity { reference } = &request.target else {
                return refusal;
            };
            let (target_facet, target_day, target_identity) = if crate::names::is_name(reference) {
                let Some(addr) = crate::names::parse_activity(reference) else {
                    return refusal;
                };
                (addr.facet, addr.day, addr.record_identity)
            } else {
                let Ok(ReferenceTarget::Activity(act_ref)) = codec.resolve(
                    reference,
                    ReferenceKind::Activity,
                    principal.connection,
                    snapshot.generation,
                ) else {
                    return refusal;
                };
                (act_ref.facet_id, act_ref.day, act_ref.record_identity)
            };

            let Some(seen) = crate::may_see::activity_at(
                journal_root,
                snapshot,
                &target_facet,
                &target_day,
                &target_identity,
                None,
            ) else {
                return refusal;
            };

            let Ok(Some(act_record)) = solstone_core_facets::get_activity_record(
                journal_root,
                &seen.place.facet_dir,
                &seen.place.day,
                &target_identity,
            ) else {
                return refusal;
            };

            if let Some(existing) = replay {
                return Ok(existing_result(
                    journal_root,
                    principal,
                    existing,
                    &request.operation_id,
                    true,
                ));
            }

            // 6. already_true check
            if request.kind == "recording" {
                if act_record.get("playback").and_then(Value::as_bool) == Some(true) {
                    return Ok(Prepared::new(
                        json!({
                            "schema": SUGGESTION_RESULT_SCHEMA,
                            "status": "already_true",
                            "operation_id": request.operation_id,
                            "message": "the suggested change is already in place",
                        }),
                        1,
                        Vec::new(),
                    ));
                }
            } else if let ValidatedValue::WhatHappened { text } = &request.value {
                let live_details = super::owner::activity_account(&act_record);
                if normalize_suggestion_text(live_details) == normalize_suggestion_text(text) {
                    return Ok(Prepared::new(
                        json!({
                            "schema": SUGGESTION_RESULT_SCHEMA,
                            "status": "already_true",
                            "operation_id": request.operation_id,
                            "message": "the suggested change is already in place",
                        }),
                        1,
                        Vec::new(),
                    ));
                }
            }

            // 7. owner_answered check
            if let Some(edits) = act_record.get("edits").and_then(Value::as_array) {
                for edit in edits {
                    if edit
                        .as_object()
                        .is_some_and(solstone_core_facets::edit_removed)
                    {
                        continue;
                    }
                    if edit.get("actor").and_then(Value::as_str) == Some("owner:update")
                        && let Some(set_obj) = edit.get("set").and_then(Value::as_object)
                    {
                        if request.kind == "recording" && set_obj.get("playback").is_some() {
                            return Ok(Prepared::new(
                                json!({
                                    "schema": SUGGESTION_RESULT_SCHEMA,
                                    "status": "owner_answered",
                                    "operation_id": request.operation_id,
                                    "message": "the owner has already answered this question",
                                }),
                                1,
                                Vec::new(),
                            ));
                        }
                        if request.kind == "what_happened" && set_obj.get("details").is_some() {
                            return Ok(Prepared::new(
                                json!({
                                    "schema": SUGGESTION_RESULT_SCHEMA,
                                    "status": "owner_answered",
                                    "operation_id": request.operation_id,
                                    "message": "the owner has already answered this question",
                                }),
                                1,
                                Vec::new(),
                            ));
                        }
                    }
                }
            }

            // Deduplication & Similarity key
            let stream = act_record
                .get("stream")
                .and_then(Value::as_str)
                .unwrap_or("");
            let mut segs = act_record
                .get("segments")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(Value::as_str)
                        .map(String::from)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            segs.sort();
            segs.dedup();
            let seg_basenames: Vec<String> = segs
                .iter()
                .map(|s| s.rsplit('/').next().unwrap_or(s.as_str()).to_string())
                .collect();

            let identity = if seg_basenames.is_empty() {
                format!("{}/{}", seen.place.facet_dir, target_identity)
            } else {
                seg_basenames.join(",")
            };
            let similarity_key = format!(
                "{}:{}:{}:{}",
                request.kind, seen.place.day, stream, identity
            );
            let same_target = |record: &SuggestionRecord| {
                if record.kind != request.kind
                    || record.target.get("day").and_then(Value::as_str)
                        != Some(seen.place.day.as_str())
                {
                    return false;
                }
                let existing: Vec<&str> = record
                    .target
                    .get("segments")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect();
                if segs.is_empty() || existing.is_empty() {
                    record.target.get("facet").and_then(Value::as_str)
                        == Some(seen.place.facet_dir.as_str())
                        && record.target.get("activity_id").and_then(Value::as_str)
                            == Some(target_identity.as_str())
                } else {
                    record.baseline.get("stream").and_then(Value::as_str) == Some(stream)
                        && existing.iter().any(|segment| {
                            segs.iter()
                                .any(|other| segment.rsplit('/').next() == other.rsplit('/').next())
                        })
                }
            };

            // Undone similarity check
            let is_undone = records.iter().any(|r| {
                if r.status != "undone" || !same_target(r) {
                    return false;
                }
                if let ValidatedValue::WhatHappened { text } = &request.value {
                    let ex_text = r.value.get("text").and_then(Value::as_str).unwrap_or("");
                    near_match(ex_text, text)
                } else {
                    true
                }
            });
            if is_undone {
                return Ok(Prepared::new(
                    json!({
                        "schema": SUGGESTION_RESULT_SCHEMA,
                        "status": "owner_answered",
                        "operation_id": request.operation_id,
                        "message": "the owner has already answered this question",
                    }),
                    1,
                    Vec::new(),
                ));
            }

            // 8. Caps
            let pending_count = records
                .iter()
                .filter(|r| {
                    r.authority_id == principal.connection
                        && super::owner::effective_status(journal_root, r) == "pending"
                })
                .count();
            if pending_count >= PENDING_AGENT_CAP {
                return Ok(Prepared::new(
                    json!({
                        "schema": SUGGESTION_RESULT_SCHEMA,
                        "status": "rate_limited",
                        "next": "wait_for_review",
                        "operation_id": request.operation_id,
                        "message": "pending suggestions cap reached",
                    }),
                    1,
                    Vec::new(),
                ));
            }

            let daily_count = records
                .iter()
                .filter(|r| {
                    r.authority_id == principal.connection
                        && chrono::DateTime::parse_from_rfc3339(&r.created_at)
                            .map(|dt| {
                                dt.with_timezone(&chrono::Utc).date_naive() == now.date_naive()
                            })
                            .unwrap_or(false)
                })
                .count();
            if daily_count >= DAILY_AGENT_CAP {
                return Ok(Prepared::new(
                    json!({
                        "schema": SUGGESTION_RESULT_SCHEMA,
                        "status": "rate_limited",
                        "next": "retry_tomorrow",
                        "operation_id": request.operation_id,
                        "message": "daily suggestions cap reached",
                    }),
                    1,
                    Vec::new(),
                ));
            }

            // Dedup scan
            let existing_match = records.iter().find(|r| {
                if !same_target(r) {
                    return false;
                }
                if let ValidatedValue::WhatHappened { text } = &request.value {
                    let ex_text = r.value.get("text").and_then(Value::as_str).unwrap_or("");
                    near_match(ex_text, text)
                } else {
                    true
                }
            });
            if let Some(existing) = existing_match {
                return Ok(existing_result(
                    journal_root,
                    principal,
                    existing,
                    &request.operation_id,
                    false,
                ));
            }

            // 10. Persist Record
            let id = format!("sc_{}", random_32_lower_hex());
            let place = Place::Activity {
                facet: target_facet.clone(),
                day: target_day.clone(),
                id: target_identity.clone(),
            };
            let owner_link = solstone_core_journal_link::seal(journal_root, &place)
                .map(|s| format!("https://go.solstone.app/j#{s}"));

            let mut tool_result = json!({
                "schema": SUGGESTION_RESULT_SCHEMA,
                "status": "pending",
                "operation_id": request.operation_id,
                "suggestion_id": id,
                "message": "suggestion queued for owner review",
            });
            if let Some(link) = &owner_link {
                tool_result["link"] = Value::String(link.clone());
            }

            let target = json!({
                "facet": seen.place.facet_dir,
                "day": seen.place.day,
                "activity_id": target_identity,
                "segments": segs,
            });

            let baseline = super::owner::activity_baseline(&request.kind, &act_record);

            let label = capture_label(journal_root, principal.connection, &mut settings);
            let memory_source_key =
                crate::agent_state::memory_source(journal_root, principal.connection)
                    .map(|s| s.component().to_owned())
                    .unwrap_or_default();

            let rec = SuggestionRecord {
                id,
                schema: SUGGESTION_SCHEMA,
                status: "pending".to_string(),
                kind: request.kind,
                basis: request.basis,
                reason: request.reason,
                value: request.raw_value,
                operation_id: request.operation_id,
                request_sha256,
                similarity_key,
                authority_id: principal.connection.to_owned(),
                source: memory_source_key,
                label,
                created_at: now.to_rfc3339(),
                updated_at: now.to_rfc3339(),
                target,
                baseline,
                applied_stretch_id: None,
                receipt: None,
                asked: None,
                ask_reason: None,
                tool_result: tool_result.clone(),
            };

            write_new_record(journal_root, &rec)
                .map_err(|_| DispatchError::Tool(ToolError::FileUnreadable))?;

            Ok(Prepared::new(tool_result, 1, Vec::new()))
        }
        &"who_said_it" => {
            let ValidatedTarget::Lines { segment, lines } = &request.target else {
                return refusal;
            };
            let (seg_day, seg_stream, seg_segment) = if crate::names::is_name(segment) {
                let Some(addr) = crate::names::resolve_segment(journal_root, segment) else {
                    return refusal;
                };
                (addr.day, addr.stream, addr.segment)
            } else {
                let Ok(ReferenceTarget::Segment(seg_ref)) = codec.resolve(
                    segment,
                    ReferenceKind::Segment,
                    principal.connection,
                    snapshot.generation,
                ) else {
                    return refusal;
                };
                (seg_ref.day, seg_ref.stream, seg_ref.segment)
            };

            // Locate segment directory
            let day_segments =
                iter_segments_skipping_unreadable_streams(journal_root, PathOrDay::Day(&seg_day))
                    .map_err(|_| DispatchError::Tool(ToolError::ReferenceNotFound))?;
            let Some(seg_entry) = day_segments.into_iter().find(|s| {
                s.record_identity()
                    .is_ok_and(|id| id.stream == seg_stream && id.name == seg_segment)
            }) else {
                return refusal;
            };

            if crate::may_see::segment_shown(
                journal_root,
                snapshot,
                &crate::body_source::BodyHides::read(journal_root),
                None,
                &seg_day,
                &seg_stream,
                &seg_segment,
                &seg_entry,
            )
            .is_none()
            {
                return refusal;
            }

            // Person speaker admission
            let mut speaker_token = "someone_else".to_string();
            let mut person_entity_id = None;
            if let ValidatedValue::WhoSaidIt { speaker } = &request.value {
                match speaker {
                    ValidatedSpeaker::Owner => {
                        speaker_token = "owner".to_string();
                    }
                    ValidatedSpeaker::SomeoneElse => {
                        speaker_token = "someone_else".to_string();
                    }
                    ValidatedSpeaker::Person { reference } => {
                        let (facet, entity_id) = if crate::names::is_name(reference) {
                            let Some((entity_id, facet)) = crate::names::parse_entity(reference)
                            else {
                                return refusal;
                            };
                            (facet, entity_id)
                        } else {
                            let Ok(ReferenceTarget::Entity(ent_ref)) = codec.resolve(
                                reference,
                                ReferenceKind::Entity,
                                principal.connection,
                                snapshot.generation,
                            ) else {
                                return refusal;
                            };
                            (ent_ref.facet_id, ent_ref.entity_id)
                        };
                        if crate::dispatch::read_entity(journal_root, snapshot, &facet, &entity_id)
                            .is_err()
                        {
                            return refusal;
                        }
                        if !solstone_core_entity::read_entity_identity(journal_root, &entity_id)
                            .ok()
                            .flatten()
                            .is_some_and(|entity| {
                                entity.value().get("type").and_then(Value::as_str) == Some("Person")
                            })
                        {
                            return refusal;
                        }
                        person_entity_id = Some(entity_id.clone());
                        speaker_token = format!("person:{}", entity_id);
                    }
                }
            }

            if let Some(existing) = replay {
                return Ok(existing_result(
                    journal_root,
                    principal,
                    existing,
                    &request.operation_id,
                    true,
                ));
            }

            // Line selector resolution
            let segment_dir = seg_entry.path();
            let Ok((statements, is_untimed)) = load_segment_statements(segment_dir) else {
                return refusal;
            };
            if is_untimed {
                return refusal;
            }

            let mut matched_sentences: Vec<(i64, String, String)> = Vec::new();
            for selector in lines {
                let matches: Vec<_> = statements
                    .iter()
                    .filter(|s| {
                        s.time == selector.time && s.text.starts_with(&selector.starts_with)
                    })
                    .collect();
                if matches.is_empty() {
                    return refusal;
                }
                if matches.len() > 1 {
                    return Ok(Prepared::new(
                        json!({
                            "schema": SUGGESTION_RESULT_SCHEMA,
                            "status": "refused",
                            "next": "narrow_lines",
                            "operation_id": request.operation_id,
                            "message": "multiple lines matched the selector",
                        }),
                        1,
                        Vec::new(),
                    ));
                }
                let m = matches[0];
                matched_sentences.push((m.sentence_id, m.time.clone(), m.text.clone()));
            }

            let source =
                solstone_core_speaker_resolve::owner_line_labels::agreed_source(segment_dir).ok();
            let who =
                solstone_core_transcripts::load_who_spoke(segment_dir, source.as_deref(), None);
            if who.owner_unreadable.is_some() {
                return refusal;
            }
            let desired = if speaker_token == "owner" {
                solstone_core_entity::read_journal_principal(journal_root)
                    .ok()
                    .flatten()
                    .and_then(|identity| {
                        identity
                            .get("id")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                    })
            } else {
                person_entity_id.clone()
            };
            if (speaker_token != "owner" || desired.is_some())
                && matched_sentences.iter().all(|(id, _, _)| {
                    who.rows.iter().any(|row| {
                        row.get("sentence_id").and_then(Value::as_i64) == Some(*id)
                            && row.get("speaker").and_then(Value::as_str) == desired.as_deref()
                    })
                })
            {
                return Ok(Prepared::new(
                    json!({"schema":SUGGESTION_RESULT_SCHEMA,"status":"already_true",
                    "operation_id":request.operation_id,"message":"the suggested change is already in place"}),
                    1,
                    Vec::new(),
                ));
            }
            matched_sentences.sort_by_key(|(id, _, _)| *id);
            matched_sentences.dedup_by_key(|(id, _, _)| *id);
            // Owner answered check
            let owner_entries =
                solstone_core_speaker_resolve::owner_line_labels::current_owner_line_entries(
                    segment_dir,
                )
                .map_err(|_| DispatchError::Tool(ToolError::FileUnreadable))?;
            for (sent_id, _, _) in &matched_sentences {
                if owner_entries.iter().any(|e| e.sentence_id == *sent_id) {
                    return Ok(Prepared::new(
                        json!({
                            "schema": SUGGESTION_RESULT_SCHEMA,
                            "status": "owner_answered",
                            "operation_id": request.operation_id,
                            "message": "the owner has already answered this question",
                        }),
                        1,
                        Vec::new(),
                    ));
                }
            }

            let segment_origin = format!("{}/{}/{}", seg_day, seg_stream, seg_segment);
            let mut sorted_ids: Vec<i64> = matched_sentences.iter().map(|(id, _, _)| *id).collect();
            sorted_ids.sort();
            sorted_ids.dedup();
            let sorted_ids_str = sorted_ids
                .iter()
                .map(|id| id.to_string())
                .collect::<Vec<_>>()
                .join(",");

            let similarity_key = format!(
                "who_said_it:{}:{}:{}",
                segment_origin, sorted_ids_str, speaker_token
            );

            if records
                .iter()
                .any(|r| r.similarity_key == similarity_key && r.status == "undone")
            {
                return Ok(Prepared::new(
                    json!({
                        "schema": SUGGESTION_RESULT_SCHEMA,
                        "status": "owner_answered",
                        "operation_id": request.operation_id,
                        "message": "the owner has already answered this question",
                    }),
                    1,
                    Vec::new(),
                ));
            }

            // Caps
            let pending_count = records
                .iter()
                .filter(|r| {
                    r.authority_id == principal.connection
                        && super::owner::effective_status(journal_root, r) == "pending"
                })
                .count();
            if pending_count >= PENDING_AGENT_CAP {
                return Ok(Prepared::new(
                    json!({
                        "schema": SUGGESTION_RESULT_SCHEMA,
                        "status": "rate_limited",
                        "next": "wait_for_review",
                        "operation_id": request.operation_id,
                        "message": "pending suggestions cap reached",
                    }),
                    1,
                    Vec::new(),
                ));
            }

            let daily_count = records
                .iter()
                .filter(|r| {
                    r.authority_id == principal.connection
                        && chrono::DateTime::parse_from_rfc3339(&r.created_at)
                            .map(|dt| {
                                dt.with_timezone(&chrono::Utc).date_naive() == now.date_naive()
                            })
                            .unwrap_or(false)
                })
                .count();
            if daily_count >= DAILY_AGENT_CAP {
                return Ok(Prepared::new(
                    json!({
                        "schema": SUGGESTION_RESULT_SCHEMA,
                        "status": "rate_limited",
                        "next": "retry_tomorrow",
                        "operation_id": request.operation_id,
                        "message": "daily suggestions cap reached",
                    }),
                    1,
                    Vec::new(),
                ));
            }

            // Dedup
            if let Some(existing) = records.iter().find(|r| r.similarity_key == similarity_key) {
                return Ok(existing_result(
                    journal_root,
                    principal,
                    existing,
                    &request.operation_id,
                    false,
                ));
            }

            // Persist
            let id = format!("sc_{}", random_32_lower_hex());
            let place = Place::Recording {
                day: seg_day.clone(),
                stream: seg_stream.clone(),
                segment: seg_segment.clone(),
            };
            let owner_link = solstone_core_journal_link::seal(journal_root, &place)
                .map(|s| format!("https://go.solstone.app/j#{s}"));

            let mut tool_result = json!({
                "schema": SUGGESTION_RESULT_SCHEMA,
                "status": "pending",
                "operation_id": request.operation_id,
                "suggestion_id": id,
                "message": "suggestion queued for owner review",
            });
            if let Some(link) = &owner_link {
                tool_result["link"] = Value::String(link.clone());
            }

            let target = json!({
                "day": seg_day,
                "stream": seg_stream,
                "segment": seg_segment,
                "source": solstone_core_speaker_resolve::owner_line_labels::agreed_source(segment_dir).ok(),
                "sentences": matched_sentences.iter().map(|(id, time, _)| {
                    json!({ "sentence_id": id, "time": time })
                }).collect::<Vec<_>>(),
            });

            let label = capture_label(journal_root, principal.connection, &mut settings);
            let memory_source_key =
                crate::agent_state::memory_source(journal_root, principal.connection)
                    .map(|s| s.component().to_owned())
                    .unwrap_or_default();

            let mut value_to_store = request.raw_value.clone();
            if let Some(person_id) = person_entity_id {
                value_to_store = json!({
                    "speaker": { "person": person_id }
                });
            }

            let Some(baseline) =
                super::owner::line_baseline(journal_root, &target, &value_to_store)
            else {
                return refusal;
            };
            if baseline["lines"]
                != json!(
                    matched_sentences
                        .iter()
                        .map(|(id, time, text)| json!({"sentence_id":id,"time":time,"text":text}))
                        .collect::<Vec<_>>()
                )
            {
                return refusal;
            }
            let rec = SuggestionRecord {
                id,
                schema: SUGGESTION_SCHEMA,
                status: "pending".to_string(),
                kind: request.kind,
                basis: request.basis,
                reason: request.reason,
                value: value_to_store,
                operation_id: request.operation_id,
                request_sha256,
                similarity_key,
                authority_id: principal.connection.to_owned(),
                source: memory_source_key,
                label,
                created_at: now.to_rfc3339(),
                updated_at: now.to_rfc3339(),
                target,
                baseline,
                applied_stretch_id: None,
                receipt: None,
                asked: None,
                ask_reason: None,
                tool_result: tool_result.clone(),
            };

            write_new_record(journal_root, &rec)
                .map_err(|_| DispatchError::Tool(ToolError::FileUnreadable))?;

            Ok(Prepared::new(tool_result, 1, Vec::new()))
        }
        &"name" => {
            let ValidatedTarget::Entity { reference } = &request.target else {
                return refusal;
            };
            let (target_facet_id, target_entity_id) = if crate::names::is_name(reference) {
                let Some((entity_id, facet)) = crate::names::parse_entity(reference) else {
                    return refusal;
                };
                (facet, entity_id)
            } else {
                let Ok(ReferenceTarget::Entity(ent_ref)) = codec.resolve(
                    reference,
                    ReferenceKind::Entity,
                    principal.connection,
                    snapshot.generation,
                ) else {
                    return refusal;
                };
                (ent_ref.facet_id, ent_ref.entity_id)
            };

            if crate::dispatch::read_entity(
                journal_root,
                snapshot,
                &target_facet_id,
                &target_entity_id,
            )
            .is_err()
            {
                return refusal;
            }

            if let Some(existing) = replay {
                return Ok(existing_result(
                    journal_root,
                    principal,
                    existing,
                    &request.operation_id,
                    true,
                ));
            }

            let Ok(Some(live_snapshot)) =
                solstone_core_entity::read_entity_identity(journal_root, &target_entity_id)
            else {
                return refusal;
            };
            let live_entity_val = live_snapshot.value();
            if live_entity_val.get("type").and_then(Value::as_str) != Some("Person") {
                return refusal;
            }

            let live_name = live_entity_val
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("");
            let live_aka: Vec<String> = live_entity_val
                .get("aka")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(Value::as_str)
                        .map(String::from)
                        .collect()
                })
                .unwrap_or_default();

            let ValidatedValue::Name { text, mode } = &request.value else {
                return refusal;
            };

            // 6. already_true check
            let query_norm = normalize_resolution_query(text);
            let live_name_norm = normalize_resolution_query(live_name);
            let name_matches = live_name_norm == query_norm;
            let aka_matches = live_aka
                .iter()
                .any(|a| normalize_resolution_query(a) == query_norm);
            if name_matches || aka_matches {
                return Ok(Prepared::new(
                    json!({
                        "schema": SUGGESTION_RESULT_SCHEMA,
                        "status": "already_true",
                        "operation_id": request.operation_id,
                        "message": "the suggested change is already in place",
                    }),
                    1,
                    Vec::new(),
                ));
            }

            // Owner acceptance is the owner's answer, including an unchanged name or alias.
            let history =
                solstone_core_entity::read_visible_history(journal_root, &target_entity_id)
                    .map_err(|_| DispatchError::Tool(ToolError::FileUnreadable))?;
            if history.iter().any(|event| {
                let value = event.value();
                value.get("kind").and_then(Value::as_str) != Some("create")
                    && (value
                        .get("operation")
                        .and_then(|operation| operation.get("owner_answer"))
                        == Some(&Value::Bool(true))
                        || value
                            .get("identity_before")
                            .zip(value.get("identity_after"))
                            .is_some_and(|(before, after)| before.get("name") != after.get("name")))
            }) {
                return Ok(Prepared::new(
                    json!({"schema":SUGGESTION_RESULT_SCHEMA,"status":"owner_answered",
                    "operation_id":request.operation_id,"message":"the owner has already answered this question"}),
                    1,
                    Vec::new(),
                ));
            }

            let similarity_key = format!("name:{}:{}", target_entity_id, mode);

            let is_undone = records.iter().any(|r| {
                if r.status != "undone" || r.similarity_key != similarity_key {
                    return false;
                }
                let ex_text = r.value.get("text").and_then(Value::as_str).unwrap_or("");
                near_match(ex_text, text)
            });
            if is_undone {
                return Ok(Prepared::new(
                    json!({
                        "schema": SUGGESTION_RESULT_SCHEMA,
                        "status": "owner_answered",
                        "operation_id": request.operation_id,
                        "message": "the owner has already answered this question",
                    }),
                    1,
                    Vec::new(),
                ));
            }

            // Caps
            let pending_count = records
                .iter()
                .filter(|r| {
                    r.authority_id == principal.connection
                        && super::owner::effective_status(journal_root, r) == "pending"
                })
                .count();
            if pending_count >= PENDING_AGENT_CAP {
                return Ok(Prepared::new(
                    json!({
                        "schema": SUGGESTION_RESULT_SCHEMA,
                        "status": "rate_limited",
                        "next": "wait_for_review",
                        "operation_id": request.operation_id,
                        "message": "pending suggestions cap reached",
                    }),
                    1,
                    Vec::new(),
                ));
            }

            let daily_count = records
                .iter()
                .filter(|r| {
                    r.authority_id == principal.connection
                        && chrono::DateTime::parse_from_rfc3339(&r.created_at)
                            .map(|dt| {
                                dt.with_timezone(&chrono::Utc).date_naive() == now.date_naive()
                            })
                            .unwrap_or(false)
                })
                .count();
            if daily_count >= DAILY_AGENT_CAP {
                return Ok(Prepared::new(
                    json!({
                        "schema": SUGGESTION_RESULT_SCHEMA,
                        "status": "rate_limited",
                        "next": "retry_tomorrow",
                        "operation_id": request.operation_id,
                        "message": "daily suggestions cap reached",
                    }),
                    1,
                    Vec::new(),
                ));
            }

            // Dedup
            let existing_match = records.iter().find(|r| {
                if r.similarity_key != similarity_key {
                    return false;
                }
                let ex_text = r.value.get("text").and_then(Value::as_str).unwrap_or("");
                near_match(ex_text, text)
            });
            if let Some(existing) = existing_match {
                return Ok(existing_result(
                    journal_root,
                    principal,
                    existing,
                    &request.operation_id,
                    false,
                ));
            }

            // Persist
            let id = format!("sc_{}", random_32_lower_hex());
            let place = Place::Person {
                id: target_entity_id.clone(),
            };
            let owner_link = solstone_core_journal_link::seal(journal_root, &place)
                .map(|s| format!("https://go.solstone.app/j#{s}"));

            let mut tool_result = json!({
                "schema": SUGGESTION_RESULT_SCHEMA,
                "status": "pending",
                "operation_id": request.operation_id,
                "suggestion_id": id,
                "message": "suggestion queued for owner review",
            });
            if let Some(link) = &owner_link {
                tool_result["link"] = Value::String(link.clone());
            }

            let target = json!({
                "facet": target_facet_id,
                "entity_id": target_entity_id,
            });

            let baseline =
                super::owner::name_baseline(journal_root, &target_entity_id, live_entity_val);

            let label = capture_label(journal_root, principal.connection, &mut settings);
            let memory_source_key =
                crate::agent_state::memory_source(journal_root, principal.connection)
                    .map(|s| s.component().to_owned())
                    .unwrap_or_default();

            let rec = SuggestionRecord {
                id,
                schema: SUGGESTION_SCHEMA,
                status: "pending".to_string(),
                kind: request.kind,
                basis: request.basis,
                reason: request.reason,
                value: request.raw_value,
                operation_id: request.operation_id,
                request_sha256,
                similarity_key,
                authority_id: principal.connection.to_owned(),
                source: memory_source_key,
                label,
                created_at: now.to_rfc3339(),
                updated_at: now.to_rfc3339(),
                target,
                baseline,
                applied_stretch_id: None,
                receipt: None,
                asked: None,
                ask_reason: None,
                tool_result: tool_result.clone(),
            };

            write_new_record(journal_root, &rec)
                .map_err(|_| DispatchError::Tool(ToolError::FileUnreadable))?;

            Ok(Prepared::new(tool_result, 1, Vec::new()))
        }
        _ => refusal,
    }
}

pub fn random_32_lower_hex() -> String {
    use ring::rand::SecureRandom;
    let mut bytes = [0u8; 16];
    let _ = ring::rand::SystemRandom::new().fill(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn capture_label(
    journal_root: &Path,
    authority_id: &str,
    settings: &mut super::store::SettingsRecord,
) -> String {
    let raw_label = if authority_id.starts_with("oauth:") {
        crate::oauth::store::OAuthStore::open(journal_root)
            .list_grants()
            .ok()
            .and_then(|clients| {
                clients
                    .into_iter()
                    .find(|grant| format!("oauth:{}", grant.id) == authority_id)
                    .map(|grant| grant.client_name.unwrap_or(grant.client_id))
            })
    } else {
        crate::tokens::TokenStore::open(journal_root)
            .list()
            .ok()
            .and_then(|tokens| {
                tokens
                    .into_iter()
                    .find(|t| format!("bearer:{}", t.id) == authority_id)
                    .map(|t| t.label)
            })
    };
    let label = sanitize_label(raw_label.as_deref().unwrap_or("connected agent"));
    if let Ok(source) = crate::agent_state::memory_source(journal_root, authority_id) {
        let hex = source.component().to_owned();
        if !hex.is_empty() && settings.labels.get(&hex) != Some(&label) {
            settings.labels.insert(hex, label.clone());
            let _ = write_settings(journal_root, settings);
        }
    }
    label
}

#[derive(Debug, Clone)]
pub(crate) struct ParsedStatement {
    pub sentence_id: i64,
    pub time: String,
    pub text: String,
}

pub(crate) fn load_segment_statements(
    segment_dir: &Path,
) -> Result<(Vec<ParsedStatement>, bool), DispatchError> {
    let source = solstone_core_speaker_resolve::owner_line_labels::agreed_source(segment_dir)
        .map_err(|_| DispatchError::Tool(ToolError::ReferenceNotFound))?;
    let bytes = fs::read_to_string(segment_dir.join(format!("{source}.jsonl")))
        .map_err(|_| DispatchError::Tool(ToolError::FileUnreadable))?;
    let mut ordinal = 0;
    let mut statements = Vec::new();
    let mut untimed = false;
    for line in bytes.lines() {
        let Ok(Value::Object(record)) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        untimed |= record.get("untimed").and_then(Value::as_bool) == Some(true);
        let Some(sentence_id) =
            solstone_core_transcripts::next_statement_ordinal(line, &mut ordinal)
        else {
            continue;
        };
        if solstone_core_transcripts::line_coordinate(&record, Some(sentence_id))
            != Some(sentence_id)
        {
            return Err(DispatchError::Tool(ToolError::ReferenceNotFound));
        }
        let time = record.get("start").and_then(Value::as_str).unwrap_or("");
        untimed |= time.is_empty();
        let text = record
            .get("corrected")
            .and_then(Value::as_str)
            .or_else(|| record.get("text").and_then(Value::as_str))
            .unwrap_or("");
        statements.push(ParsedStatement {
            sentence_id,
            time: time.to_owned(),
            text: text.to_owned(),
        });
    }
    Ok((statements, untimed))
}
