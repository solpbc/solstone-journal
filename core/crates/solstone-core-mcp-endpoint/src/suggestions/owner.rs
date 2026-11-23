// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::{Path as AxumPath, Query};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use jiff::Timestamp;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use solstone_core_speaker_resolve::owner_line_labels::{
    LineRevert, LineSpeaker, OwnerLine, revert_owner_line_labels_then,
};

use crate::owner_web::refusal;
use crate::suggestions::store::{
    SuggestionReceipt, SuggestionRecord, hold_suggestions_lock, list_records, read_record,
    read_settings, save_record, update_connection_enabled,
};
use crate::suggestions::submit::sanitize_label;

use super::same_person;

#[derive(Deserialize, Default)]
pub struct ListQuery {
    pub agent: Option<String>,
    pub status: Option<String>,
    pub limit: Option<usize>,
    pub before: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct AcceptBody {
    #[serde(default)]
    pub value: Option<Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetSuggestionsBody {
    pub enabled: bool,
}

pub async fn list_suggestions(
    Extension(journal): Extension<Arc<PathBuf>>,
    Query(params): Query<ListQuery>,
) -> Response {
    let _lock = match hold_suggestions_lock(&journal) {
        Ok(lock) => lock,
        Err(error) => return refusal("storage_error", error, StatusCode::INTERNAL_SERVER_ERROR),
    };
    let records = match list_records(&journal) {
        Ok(recs) => recs,
        Err(err) => {
            return refusal("storage_error", err, StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    let status_filter: Option<BTreeSet<String>> = params.status.as_deref().and_then(|s| {
        let set: BTreeSet<String> = s
            .split(',')
            .map(str::trim)
            .filter(|x| !x.is_empty())
            .map(String::from)
            .collect();
        if set.is_empty() { None } else { Some(set) }
    });

    let before_cursor = params.before.as_deref().and_then(decode_cursor);

    let mut cards = Vec::new();
    let mut total_matching = Vec::new();

    for mut rec in records {
        if let Err(error) = reconcile_pending_effect(&journal, &mut rec) {
            return refusal("storage_error", error, StatusCode::INTERNAL_SERVER_ERROR);
        }
        if let Some(agent) = &params.agent
            && &rec.source != agent
        {
            continue;
        }

        let proj_status = effective_status(&journal, &rec);

        if let Some(filter) = &status_filter
            && !filter.contains(proj_status)
        {
            continue;
        }

        if let Some((before_created, before_id)) = &before_cursor {
            let item_order = (&rec.created_at, &rec.id);
            let cursor_order = (before_created, before_id);
            if item_order >= cursor_order {
                continue;
            }
        }

        total_matching.push(rec);
    }

    let limit = params.limit.unwrap_or(50).clamp(1, 100);
    let has_more = total_matching.len() > limit;
    let page = if has_more {
        &total_matching[..limit]
    } else {
        &total_matching[..]
    };

    for rec in page {
        cards.push(project_card(&journal, rec));
    }

    let next_cursor = if has_more {
        let last = total_matching[limit - 1].clone();
        Value::String(encode_cursor(&last.created_at, &last.id))
    } else {
        Value::Null
    };

    // Enumerate agents
    let agents = list_agents_summary(&journal);

    Json(json!({
        "items": cards,
        "agents": agents,
        "next_cursor": next_cursor,
    }))
    .into_response()
}

fn find_existing_effect(
    journal_root: &Path,
    record: &SuggestionRecord,
) -> Option<(Value, SuggestionReceipt)> {
    match record.kind.as_str() {
        "same_person" => same_person::existing_effect(journal_root, record),
        "recording" | "what_happened" => {
            let facet = record.target.get("facet").and_then(Value::as_str)?;
            let day = record.target.get("day").and_then(Value::as_str)?;
            let act_id = record.target.get("activity_id").and_then(Value::as_str)?;
            let act = solstone_core_facets::get_activity_record(journal_root, facet, day, act_id)
                .ok()??;
            let edits = act.get("edits").and_then(Value::as_array)?;
            for (idx, edit) in edits.iter().enumerate() {
                if edit
                    .as_object()
                    .is_some_and(solstone_core_facets::edit_removed)
                {
                    continue;
                }
                if edit.get("suggestion_id").and_then(Value::as_str) == Some(&record.id) {
                    let mut receipt = SuggestionReceipt {
                        facet: Some(facet.to_owned()),
                        day: Some(day.to_owned()),
                        activity_id: Some(act_id.to_owned()),
                        edit_position: Some(idx),
                        edit_timestamp: edit
                            .get("timestamp")
                            .and_then(Value::as_str)
                            .map(String::from),
                        ..Default::default()
                    };
                    if let Some(edit_obj) = edit.as_object() {
                        receipt.stretch_id =
                            solstone_core_facets::stretch::edit_stretch_id(edit_obj)
                                .map(String::from);
                    }

                    let stored_value = if record.kind == "recording" {
                        json!({})
                    } else {
                        let text = edit
                            .get("set")
                            .and_then(|s| s.get("details"))
                            .and_then(Value::as_str)
                            .unwrap_or("");
                        json!({ "text": text })
                    };
                    return Some((stored_value, receipt));
                }
            }
            None
        }
        "who_said_it" => {
            let day = record.target.get("day").and_then(Value::as_str)?;
            let stream = record.target.get("stream").and_then(Value::as_str)?;
            let segment = record.target.get("segment").and_then(Value::as_str)?;
            let segment_origin = format!("{day}/{stream}/{segment}");
            let mut activities = find_activities_spanning_segment(journal_root, &segment_origin);
            if let Some(hint) = record.target.get("activity_hint")
                && let (Some(facet), Some(id)) = (
                    hint.get("facet").and_then(Value::as_str),
                    hint.get("id").and_then(Value::as_str),
                )
                && let Ok(Some(activity)) =
                    solstone_core_facets::get_activity_record(journal_root, facet, day, id)
            {
                activities.insert(
                    0,
                    (
                        facet.to_owned(),
                        day.to_owned(),
                        id.to_owned(),
                        Value::Object(activity),
                    ),
                );
            }
            for (facet, day, act_id, act) in &activities {
                let Some(edits) = act.get("edits").and_then(Value::as_array) else {
                    continue;
                };
                for (idx, edit) in edits.iter().enumerate() {
                    if edit
                        .as_object()
                        .is_some_and(solstone_core_facets::edit_removed)
                    {
                        continue;
                    }
                    if edit.get("suggestion_id").and_then(Value::as_str) == Some(&record.id) {
                        let mut receipt = SuggestionReceipt {
                            facet: Some(facet.to_owned()),
                            day: Some(day.to_owned()),
                            activity_id: Some(act_id.to_owned()),
                            edit_position: Some(idx),
                            edit_timestamp: edit
                                .get("timestamp")
                                .and_then(Value::as_str)
                                .map(String::from),
                            ..Default::default()
                        };
                        let mut reverts = Vec::new();
                        if let Some(lines) = edit.get("lines").and_then(Value::as_array) {
                            for line in lines {
                                if let Some(sent_id) =
                                    line.get("sentence_id").and_then(Value::as_i64)
                                {
                                    reverts.push(json!({
                                        "sentence_id": sent_id,
                                        "set_at": line.get("set_at"),
                                        "prior": line.get("prior"),
                                    }));
                                }
                            }
                        }
                        receipt.line_reverts = Some(reverts);

                        let speaker_val = match edit.get("speaker") {
                            Some(spk)
                                if spk.get("kind").and_then(Value::as_str) == Some("owner") =>
                            {
                                json!({ "speaker": "owner" })
                            }
                            Some(spk)
                                if spk.get("kind").and_then(Value::as_str) == Some("person") =>
                            {
                                let pid = spk.get("id").and_then(Value::as_str).unwrap_or("");
                                json!({ "speaker": { "person": pid } })
                            }
                            _ => json!({ "speaker": "someone_else" }),
                        };
                        return Some((speaker_val, receipt));
                    }
                }
            }
            None
        }
        "name" => {
            let entity_id = record.target.get("entity_id").and_then(Value::as_str)?;
            let events =
                solstone_core_entity::read_visible_history(journal_root, entity_id).ok()?;
            for event in &events {
                let val = event.value();
                let op = val.get("operation").and_then(Value::as_object);
                if op
                    .and_then(|o| o.get("suggestion_id"))
                    .and_then(Value::as_str)
                    == Some(&record.id)
                {
                    let before = val.get("identity_before");
                    let after = val.get("identity_after")?;
                    let version_id = val.get("version_id").and_then(Value::as_str);
                    let prior_version_id = before
                        .and_then(|b| b.get("version_id"))
                        .and_then(Value::as_str);

                    let mut receipt = SuggestionReceipt {
                        entity_id: Some(entity_id.to_owned()),
                        version_id: version_id.map(String::from),
                        prior_version_id: prior_version_id.map(String::from),
                        identity_before: before.cloned(),
                        identity_after: Some(after.clone()),
                        ..Default::default()
                    };

                    if let Some(value) = op.and_then(|operation| operation.get("suggestion_value"))
                    {
                        receipt.mode = value.get("mode").and_then(Value::as_str).map(str::to_owned);
                        return Some((value.clone(), receipt));
                    }
                    let before_name = before.and_then(|b| b.get("name")).and_then(Value::as_str);
                    let after_name = after.get("name").and_then(Value::as_str).unwrap_or("");
                    if before_name != Some(after_name) {
                        receipt.mode = Some("replace".to_owned());
                        let stored_value = json!({
                            "text": after_name,
                            "mode": "replace",
                        });
                        return Some((stored_value, receipt));
                    } else {
                        receipt.mode = Some("also_known_as".to_owned());
                        let before_aka: Vec<&str> = before
                            .and_then(|b| b.get("aka"))
                            .and_then(Value::as_array)
                            .map(|arr| arr.iter().filter_map(Value::as_str).collect())
                            .unwrap_or_default();
                        let after_aka: Vec<&str> = after
                            .get("aka")
                            .and_then(Value::as_array)
                            .map(|arr| arr.iter().filter_map(Value::as_str).collect())
                            .unwrap_or_default();
                        let added_alias = after_aka
                            .iter()
                            .find(|a| !before_aka.contains(a))
                            .copied()
                            .unwrap_or("");
                        let stored_value = json!({
                            "text": added_alias,
                            "mode": "also_known_as",
                        });
                        return Some((stored_value, receipt));
                    }
                }
            }
            None
        }
        _ => None,
    }
}

pub async fn accept_suggestion(
    Extension(journal): Extension<Arc<PathBuf>>,
    AxumPath(id): AxumPath<String>,
    body: axum::body::Bytes,
) -> Response {
    let body: AcceptBody = if body.is_empty() {
        AcceptBody::default()
    } else {
        match serde_json::from_slice(&body) {
            Ok(b) => b,
            Err(err) => return refusal("invalid_body", err, StatusCode::BAD_REQUEST),
        }
    };

    let _lock = match hold_suggestions_lock(&journal) {
        Ok(l) => l,
        Err(err) => return refusal("storage_error", err, StatusCode::INTERNAL_SERVER_ERROR),
    };

    let mut record = match read_record(&journal, &id) {
        Ok(Some(r)) => r,
        Ok(None) => {
            return refusal(
                "suggestion_not_found",
                "suggestion not found",
                StatusCode::NOT_FOUND,
            );
        }
        Err(err) => return refusal("storage_error", err, StatusCode::INTERNAL_SERVER_ERROR),
    };

    let _trust = if record.kind == "same_person" {
        let trust = match solstone_core_entity::hold_entity_trust_lock(&journal) {
            Ok(trust) => trust,
            Err(error) => return refusal("entity_busy", error, StatusCode::CONFLICT),
        };
        if let Err(error) = solstone_core_entity::recover_interrupted_entity_merge(&journal) {
            return refusal("storage_error", error, StatusCode::INTERNAL_SERVER_ERROR);
        }
        Some(trust)
    } else {
        None
    };
    if let Err(error) = reconcile_pending_effect(&journal, &mut record) {
        return refusal("storage_error", error, StatusCode::INTERNAL_SERVER_ERROR);
    }
    if record.status == "accepted" {
        return Json(json!({ "suggestion": project_card(&journal, &record) })).into_response();
    }
    if record.status != "pending" {
        return refusal(
            "not_pending",
            "can only accept pending suggestions",
            StatusCode::CONFLICT,
        );
    }

    let (accepted_value, receipt, asked, ask_reason) = {
        let accepted_value = if let Some(edit_val) = body.value {
            if record.kind == "recording" || record.kind == "who_said_it" {
                return refusal(
                    "invalid_body",
                    "this suggestion kind cannot be edited",
                    StatusCode::BAD_REQUEST,
                );
            }
            if record.kind == "what_happened" {
                let Some(text) = edit_val.get("text").and_then(Value::as_str) else {
                    return refusal("invalid_body", "missing text", StatusCode::BAD_REQUEST);
                };
                if edit_val.as_object().is_none_or(|value| value.len() != 1)
                    || text.trim().is_empty()
                    || !(1..=600).contains(&text.chars().count())
                {
                    return refusal(
                        "invalid_body",
                        "text length must be 1..=600",
                        StatusCode::BAD_REQUEST,
                    );
                }
                json!({ "text": text })
            } else if record.kind == "name" {
                let Some(text) = edit_val.get("text").and_then(Value::as_str) else {
                    return refusal("invalid_body", "missing text", StatusCode::BAD_REQUEST);
                };
                if edit_val.as_object().is_none_or(|value| value.len() != 2)
                    || text.trim().is_empty()
                    || !(1..=200).contains(&text.chars().count())
                {
                    return refusal(
                        "invalid_body",
                        "text length must be 1..=200",
                        StatusCode::BAD_REQUEST,
                    );
                }
                let Some(mode) = edit_val.get("mode").and_then(Value::as_str) else {
                    return refusal("invalid_body", "missing mode", StatusCode::BAD_REQUEST);
                };
                if !matches!(mode, "replace" | "also_known_as") {
                    return refusal(
                        "invalid_body",
                        "mode must be replace or also_known_as",
                        StatusCode::BAD_REQUEST,
                    );
                }
                json!({ "text": text, "mode": mode })
            } else if record.kind == "same_person" {
                let Some(value) = same_person::accept_value(&record, &edit_val) else {
                    return refusal(
                        "invalid_body",
                        "choose one of the two people",
                        StatusCode::BAD_REQUEST,
                    );
                };
                value
            } else {
                record.value.clone()
            }
        } else {
            record.value.clone()
        };

        if is_record_stale(&journal, &record) {
            let mut card = project_card(&journal, &record);
            card["status"] = Value::String("stale".to_string());
            return (
                StatusCode::CONFLICT,
                Json(json!({
                    "code": "already_changed",
                    "error": "already_changed",
                    "reason_code": "already_changed",
                    "suggestion": card,
                })),
            )
                .into_response();
        }

        let now_dt = Timestamp::now();
        let now_str = solstone_core_time::utc_rfc3339(now_dt);

        let (receipt, asked, ask_reason) = match record.kind.as_str() {
            "recording" | "what_happened" => {
                let mut receipt = SuggestionReceipt::default();
                let facet = record
                    .target
                    .get("facet")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let day = record
                    .target
                    .get("day")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let act_id = record
                    .target
                    .get("activity_id")
                    .and_then(Value::as_str)
                    .unwrap_or("");

                let mut patch_map = Map::new();
                if record.kind == "recording" {
                    patch_map.insert("playback".to_string(), Value::Bool(true));
                } else {
                    let text = accepted_value
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    patch_map.insert("details".to_string(), Value::String(text.to_owned()));
                }

                let mut extra = Map::new();
                extra.insert(
                    "suggested_by".to_string(),
                    json!({ "source": record.source, "label": record.label }),
                );
                extra.insert(
                    "suggestion_id".to_string(),
                    Value::String(record.id.clone()),
                );

                let baseline = record.baseline.clone();
                let kind_str = record.kind.clone();
                let checkpoint = move |r: &solstone_core_facets::ActivityRecord| {
                    if activity_baseline(&kind_str, r) != baseline {
                        return Err("stale".to_owned());
                    }
                    Ok(())
                };

                let ask = |root:&Path,day:&str,activities:&[solstone_core_system::owner_ask::AskedActivity]| {
                    ask_once(root,&record.id,"accept",day,activities)
                };
                let ask_hook: Option<&solstone_core_system::owner_ask::CorrectionAsk<'_>> =
                    Some(&ask);

                #[cfg(all(test, feature = "full-tests"))]
                crate::suggestions::run_before_activity_apply();
                match solstone_core_system::owner_activity::apply_owner_activity_correction(
                    &journal,
                    facet,
                    day,
                    act_id,
                    &patch_map,
                    "owner:update",
                    &now_str,
                    "mcp_endpoint",
                    "owner:update",
                    checkpoint,
                    &extra,
                    true,
                    ask_hook,
                ) {
                    Ok(applied) => {
                        if applied.correction.record.is_none() {
                            return (
                                StatusCode::CONFLICT,
                                Json(
                                    json!({"code":"already_changed","reason_code":"already_changed",
                                "suggestion":project_card(&journal,&record)}),
                                ),
                            )
                                .into_response();
                        }
                        let stretch_id = applied
                            .correction
                            .record
                            .as_ref()
                            .and_then(|r| r.get("edits"))
                            .and_then(Value::as_array)
                            .and_then(|e| e.last())
                            .and_then(Value::as_object)
                            .and_then(solstone_core_facets::stretch::edit_stretch_id)
                            .map(str::to_owned);

                        receipt.incomplete = !applied.correction.failed.is_empty();

                        receipt.stretch_id = stretch_id;
                        receipt.facet = Some(facet.to_owned());
                        receipt.day = Some(day.to_owned());
                        receipt.activity_id = Some(act_id.to_owned());
                        receipt.edit_timestamp = Some(now_str.clone());

                        if let Some(r) = &applied.correction.record
                            && let Some(edits) = r.get("edits").and_then(Value::as_array)
                        {
                            receipt.edit_position = Some(edits.len().saturating_sub(1));
                        }

                        let incomplete = receipt.incomplete;
                        (
                            receipt,
                            if incomplete { false } else { applied.asked },
                            if incomplete {
                                Some("stretch_incomplete".to_owned())
                            } else {
                                applied.ask_reason
                            },
                        )
                    }
                    Err(err) => {
                        return refusal("mutation_failed", err, StatusCode::INTERNAL_SERVER_ERROR);
                    }
                }
            }
            "who_said_it" => {
                let mut receipt = SuggestionReceipt::default();
                let day = record
                    .target
                    .get("day")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let stream = record
                    .target
                    .get("stream")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let segment = record
                    .target
                    .get("segment")
                    .and_then(Value::as_str)
                    .unwrap_or("");

                let speaker_val = accepted_value.get("speaker").unwrap_or(&Value::Null);
                let speaker = match speaker_val {
                    Value::String(s) if s == "owner" => LineSpeaker::Owner,
                    Value::String(s) if s == "someone_else" => LineSpeaker::Unknown,
                    Value::Object(m) => {
                        let id = m.get("person").and_then(Value::as_str).unwrap_or("");
                        LineSpeaker::Person(id.to_owned())
                    }
                    _ => LineSpeaker::Unknown,
                };

                let sentences = record
                    .target
                    .get("sentences")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();

                let segment_origin = format!("{day}/{stream}/{segment}");
                let activities = find_activities_spanning_segment(&journal, &segment_origin);
                let primary_act = activities.first().cloned();

                let mut extra = Map::new();
                extra.insert(
                    "suggested_by".to_string(),
                    json!({ "source": record.source, "label": record.label }),
                );
                extra.insert(
                    "suggestion_id".to_string(),
                    Value::String(record.id.clone()),
                );

                if let Some((facet, act_day, act_id, _)) = &primary_act {
                    let mut intent = record.clone();
                    intent.target["activity_hint"] = json!({"facet":facet,"id":act_id});
                    if let Err(error) = save_record(&journal, &intent) {
                        return refusal("storage_error", error, StatusCode::INTERNAL_SERVER_ERROR);
                    }

                    let line_targets: Vec<solstone_core_speaker_resolve::owner_line_labels::LineAnswerTarget> = sentences
                        .iter()
                        .filter_map(|s| {
                            let sentence_id = s.get("sentence_id").and_then(Value::as_i64)?;
                            Some(solstone_core_speaker_resolve::owner_line_labels::LineAnswerTarget {
                                stream: stream.to_string(),
                                segment: segment.to_string(),
                                source: record.target.get("source").and_then(Value::as_str).unwrap_or("").to_owned(),
                                sentence_id,
                            })
                        })
                        .collect();

                    let journal_ref = journal.clone();
                    let rec_ref = record.clone();
                    let checkpoint = move || {
                        if is_record_stale(&journal_ref, &rec_ref) {
                            Err("stale".to_string())
                        } else {
                            Ok(())
                        }
                    };

                    let ask = |root:&Path,day:&str,activities:&[solstone_core_system::owner_ask::AskedActivity]| {
                    ask_once(root,&record.id,"accept",day,activities)
                };
                    let ask_hook: Option<&solstone_core_system::owner_ask::CorrectionAsk<'_>> =
                        Some(&ask);

                    match solstone_core_speaker_resolve::owner_line_labels::apply_owner_line_answers(
                        &journal,
                        act_day,
                        facet,
                        act_id,
                        &line_targets,
                        &speaker,
                        &now_str,
                        &extra,
                        &checkpoint,
                        || Ok(()),
                        ask_hook,
                    ) {
                        Ok(applied) => {
                            receipt.line_reverts = Some(
                                applied
                                    .lines
                                    .iter()
                                    .map(|l| {
                                        json!({
                                            "sentence_id": l.sentence_id,
                                            "set_at": l.set_at,
                                            "prior": l.prior,
                                        })
                                    })
                                    .collect(),
                            );
                            receipt.facet = Some(facet.clone());
                            receipt.day = Some(act_day.clone());
                            receipt.activity_id = Some(act_id.clone());
                            receipt.edit_timestamp = Some(now_str.clone());
                            if let Some(edits) =
                                applied.record.get("edits").and_then(Value::as_array)
                            {
                                receipt.edit_position = Some(edits.len().saturating_sub(1));
                            }
                            (receipt, applied.asked, applied.ask_reason)
                        }
                        Err(err) => {
                            return refusal(
                                "mutation_failed",
                                err,
                                StatusCode::INTERNAL_SERVER_ERROR,
                            );
                        }
                    }
                } else {
                    return refusal(
                        "already_changed",
                        "the activity for these lines is no longer available",
                        StatusCode::CONFLICT,
                    );
                }
            }

            "name" => {
                let mut receipt = SuggestionReceipt::default();
                let facet = record
                    .target
                    .get("facet")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let entity_id = record
                    .target
                    .get("entity_id")
                    .and_then(Value::as_str)
                    .unwrap_or("");

                let existing_ent =
                    match solstone_core_entity::read_entity_identity(&journal, entity_id) {
                        Ok(Some(e)) => e,
                        _ => {
                            return refusal("not_found", "entity not found", StatusCode::NOT_FOUND);
                        }
                    };

                let ent_val = existing_ent.value();
                receipt.identity_before = Some(ent_val.clone());
                let prior_version_id = ent_val
                    .get("version_id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let ent_name = ent_val
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let ent_kind = String::new();
                let ent_aka: Vec<String> = ent_val
                    .get("aka")
                    .and_then(Value::as_array)
                    .map(|arr| {
                        arr.iter()
                            .filter_map(Value::as_str)
                            .map(String::from)
                            .collect()
                    })
                    .unwrap_or_default();

                let mode = accepted_value
                    .get("mode")
                    .and_then(Value::as_str)
                    .unwrap_or("replace");
                let new_text = accepted_value
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or("");

                let metadata = json!({
                    "suggested_by": { "source": record.source, "label": record.label },
                    "suggestion_id": record.id,
                    "owner_answer": true,
                    "suggestion_value": accepted_value,
                });

                let baseline = record.baseline.clone();
                let checkpoint = |curr: &Value| {
                    if name_baseline(&journal, entity_id, curr) != baseline {
                        return Err(
                            solstone_core_facets::FacetEntityWriteError::MutationRefused(
                                "stale".into(),
                            ),
                        );
                    }
                    Ok(())
                };

                if mode == "replace" {
                    match solstone_core_facets::update_facet_entity_identity(
                        &journal,
                        facet,
                        &ent_name,
                        new_text,
                        &ent_kind,
                        &ent_aka,
                        Some(entity_id),
                        metadata,
                        checkpoint,
                    ) {
                        Ok(val) => {
                            receipt.entity_id = Some(entity_id.to_owned());
                            receipt.prior_version_id = Some(prior_version_id);
                            receipt.version_id = val
                                .get("version_id")
                                .and_then(Value::as_str)
                                .map(String::from);
                            receipt.identity_after = Some(val);
                            receipt.mode = Some("replace".to_owned());
                        }
                        Err(err) => {
                            return refusal(
                                "mutation_failed",
                                err,
                                StatusCode::INTERNAL_SERVER_ERROR,
                            );
                        }
                    }
                } else {
                    match solstone_core_facets::add_entity_aka(
                        &journal, facet, entity_id, new_text, metadata, checkpoint,
                    ) {
                        Ok(_) => {
                            receipt.entity_id = Some(entity_id.to_owned());
                            receipt.prior_version_id = Some(prior_version_id);
                            receipt.mode = Some("also_known_as".to_owned());
                            if let Ok(Some(updated)) =
                                solstone_core_entity::read_entity_identity(&journal, entity_id)
                            {
                                receipt.version_id = updated
                                    .value()
                                    .get("version_id")
                                    .and_then(Value::as_str)
                                    .map(String::from);
                                receipt.identity_after = Some(updated.value().clone());
                            }
                        }
                        Err(err) => {
                            return refusal(
                                "mutation_failed",
                                err,
                                StatusCode::INTERNAL_SERVER_ERROR,
                            );
                        }
                    }
                }
                let (_, receipt) = match find_existing_effect(&journal, &record) {
                    Some(effect) => effect,
                    None => {
                        return refusal(
                            "receipt_failed",
                            "identity history could not be read",
                            StatusCode::INTERNAL_SERVER_ERROR,
                        );
                    }
                };
                (receipt, false, None)
            }
            "same_person" => match same_person::apply(&journal, &record, &accepted_value) {
                Ok(receipt) => (receipt, false, None),
                Err(error) => return refusal("mutation_failed", error, StatusCode::CONFLICT),
            },
            _ => return refusal("invalid_kind", "unknown kind", StatusCode::BAD_REQUEST),
        };

        (accepted_value, receipt, asked, ask_reason)
    };

    let (asked, ask_reason) = match super::store::read_ask_outcome(&journal, &record.id, "accept") {
        Ok(Some(outcome)) => (outcome.asked, outcome.reason),
        Ok(None) => (asked, ask_reason),
        Err(error) => return refusal("storage_error", error, StatusCode::INTERNAL_SERVER_ERROR),
    };
    record.asked = Some(asked);
    record.ask_reason = ask_reason;
    record.value = accepted_value;
    record.receipt = Some(receipt);
    #[cfg(test)]
    if crate::suggestions::force_suggestion_receipt_failure() {
        return refusal(
            "receipt_failed",
            "forced receipt failure",
            StatusCode::INTERNAL_SERVER_ERROR,
        );
    }
    if let Err(err) = save_record(&journal, &record) {
        return refusal("storage_error", err, StatusCode::INTERNAL_SERVER_ERROR);
    }
    record.status = "accepted".to_string();
    record.updated_at = solstone_core_time::utc_rfc3339(jiff::Timestamp::now());

    if let Err(err) = save_record(&journal, &record) {
        return refusal("storage_error", err, StatusCode::INTERNAL_SERVER_ERROR);
    }

    Json(json!({ "suggestion": project_card(&journal, &record) })).into_response()
}

pub async fn dismiss_suggestion(
    Extension(journal): Extension<Arc<PathBuf>>,
    AxumPath(id): AxumPath<String>,
) -> Response {
    let _lock = match hold_suggestions_lock(&journal) {
        Ok(l) => l,
        Err(err) => return refusal("storage_error", err, StatusCode::INTERNAL_SERVER_ERROR),
    };

    let mut record = match read_record(&journal, &id) {
        Ok(Some(r)) => r,
        Ok(None) => {
            return refusal(
                "suggestion_not_found",
                "suggestion not found",
                StatusCode::NOT_FOUND,
            );
        }
        Err(err) => return refusal("storage_error", err, StatusCode::INTERNAL_SERVER_ERROR),
    };

    if let Err(error) = reconcile_pending_effect(&journal, &mut record) {
        return refusal("storage_error", error, StatusCode::INTERNAL_SERVER_ERROR);
    }
    if record.status == "dismissed" {
        return Json(json!({ "suggestion": project_card(&journal, &record) })).into_response();
    }
    if record.status != "pending" {
        return refusal(
            "not_pending",
            "can only dismiss pending suggestions",
            StatusCode::CONFLICT,
        );
    }

    record.status = "dismissed".to_string();
    record.updated_at = solstone_core_time::utc_rfc3339(jiff::Timestamp::now());

    if let Err(err) = save_record(&journal, &record) {
        return refusal("storage_error", err, StatusCode::INTERNAL_SERVER_ERROR);
    }

    Json(json!({ "suggestion": project_card(&journal, &record) })).into_response()
}

pub async fn undo_suggestion(
    Extension(journal): Extension<Arc<PathBuf>>,
    AxumPath(id): AxumPath<String>,
) -> Response {
    let _lock = match hold_suggestions_lock(&journal) {
        Ok(lock) => lock,
        Err(error) => return refusal("storage_error", error, StatusCode::INTERNAL_SERVER_ERROR),
    };
    let mut record = match read_record(&journal, &id) {
        Ok(Some(record)) => record,
        Ok(None) => {
            return refusal(
                "suggestion_not_found",
                "suggestion not found",
                StatusCode::NOT_FOUND,
            );
        }
        Err(error) => return refusal("storage_error", error, StatusCode::INTERNAL_SERVER_ERROR),
    };
    if record.status == "undone" {
        return Json(json!({"suggestion":project_card(&journal, &record)})).into_response();
    }
    if record.status != "accepted" {
        return refusal(
            "not_accepted",
            "only accepted suggestions can be undone",
            StatusCode::CONFLICT,
        );
    }
    let Some(mut receipt) = record.receipt.clone() else {
        return undo_refused(&journal, &record);
    };
    if !receipt.undo_started && !compute_can_undo(&journal, &record) {
        return undo_refused(&journal, &record);
    }
    receipt.undo_started = true;
    record.receipt = Some(receipt.clone());
    if let Err(error) = save_record(&journal, &record) {
        return refusal("storage_error", error, StatusCode::INTERNAL_SERVER_ERROR);
    }
    let now = solstone_core_time::utc_rfc3339(jiff::Timestamp::now());
    let day = receipt.day.as_deref().unwrap_or("");
    let activities = affected_activities(&journal, &record, &receipt);
    let mut owner_conflict = false;
    let result: Result<(), String> = (|| {
        match record.kind.as_str() {
            "recording" | "what_happened" => {
                let facet = receipt.facet.as_deref().ok_or("missing facet")?;
                let activity = receipt.activity_id.as_deref().ok_or("missing activity")?;
                let position = receipt.edit_position.ok_or("missing edit")?;
                let timestamp = receipt
                    .edit_timestamp
                    .as_deref()
                    .ok_or("missing timestamp")?;
                if let Some(stretch) = receipt.stretch_id.as_deref() {
                    let (_, failed) = solstone_core_facets::stretch::remove_over_stretch(
                        &journal, day, facet, activity, stretch, &now,
                    );
                    if !failed.is_empty() {
                        return Err(
                            "some stretch edits could not be removed; retry undo".to_owned()
                        );
                    }
                }
                match solstone_core_facets::remove_activity_edit_checked(
                    &journal,
                    facet,
                    day,
                    activity,
                    position,
                    timestamp,
                    "owner:update",
                    &now,
                    |current| {
                        let removed = current
                            .get("edits")
                            .and_then(Value::as_array)
                            .and_then(|edits| edits.get(position))
                            .and_then(Value::as_object)
                            .is_some_and(solstone_core_facets::edit_removed);
                        if !removed && later_field_answer(current, position, &record.kind) {
                            Err("a later owner answer blocks undo".to_owned())
                        } else {
                            Ok(())
                        }
                    },
                ) {
                    Ok(_)
                    | Err(solstone_core_facets::ActivityRecordStoreError::EditRefused(
                        solstone_core_facets::EditRefusal::AlreadyRemoved,
                    )) => {}
                    Err(error) => return Err(error.to_string()),
                }
            }
            "who_said_it" => {
                let facet = receipt.facet.as_deref().ok_or("missing facet")?;
                let activity = receipt.activity_id.as_deref().ok_or("missing activity")?;
                let position = receipt.edit_position.ok_or("missing edit")?;
                let timestamp = receipt
                    .edit_timestamp
                    .as_deref()
                    .ok_or("missing timestamp")?;
                let current =
                    solstone_core_facets::get_activity_record(&journal, facet, day, activity)
                        .map_err(|error| error.to_string())?
                        .ok_or("missing activity")?;
                let removed = current
                    .get("edits")
                    .and_then(Value::as_array)
                    .and_then(|edits| edits.get(position))
                    .and_then(Value::as_object)
                    .is_some_and(solstone_core_facets::edit_removed);
                if !removed {
                    let segment =
                        suggestion_segment(&journal, &record).ok_or("segment unavailable")?;
                    let source = record
                        .target
                        .get("source")
                        .and_then(Value::as_str)
                        .ok_or("missing source")?;
                    let reverts: Vec<LineRevert> = receipt
                        .line_reverts
                        .as_deref()
                        .unwrap_or(&[])
                        .iter()
                        .map(|item| {
                            Ok(LineRevert {
                                line: OwnerLine {
                                    segment_dir: segment.clone(),
                                    source: source.to_owned(),
                                    sentence_id: item
                                        .get("sentence_id")
                                        .and_then(Value::as_i64)
                                        .ok_or("missing sentence")?,
                                },
                                tokens: BTreeSet::from([item
                                    .get("set_at")
                                    .and_then(Value::as_str)
                                    .ok_or("missing line token")?
                                    .to_owned()]),
                                restore: None,
                            })
                        })
                        .collect::<Result<_, String>>()?;
                    revert_owner_line_labels_then(&journal, &reverts, |outcomes| {
                        if outcomes.iter().any(|outcome| !matches!(outcome, solstone_core_speaker_resolve::owner_line_labels::RevertOutcome::Reverted)) {
                            return Err("a later line answer blocks undo".to_owned());
                        }
                        solstone_core_facets::remove_activity_edit(&journal, facet, day, activity, position, timestamp, "owner:lines", &now)
                            .map_err(|error|error.to_string())
                    }).map_err(|error| error.render_in(crate::owner_web::owner_response_locale()))??;
                }
            }
            "name" => {
                let metadata = json!({"suggestion_id":record.id, "suggestion_undo":true, "owner_answer":true,
                    "suggested_by":{"source":record.source,"label":record.label}});
                if let Err(error) = solstone_core_entity::restore_identity_snapshot(
                    &journal,
                    receipt.entity_id.as_deref().ok_or("missing entity")?,
                    receipt
                        .identity_before
                        .as_ref()
                        .ok_or("missing prior names")?,
                    receipt
                        .identity_after
                        .as_ref()
                        .ok_or("missing accepted names")?,
                    receipt
                        .version_id
                        .as_deref()
                        .ok_or("missing history version")?,
                    metadata,
                ) {
                    owner_conflict = matches!(
                        error,
                        solstone_core_entity::EntityLifecycleError::RestoreNameChanged
                    );
                    return Err(error.to_string());
                }
            }
            _ => return Err("unknown suggestion kind".to_owned()),
        }
        Ok(())
    })();
    if let Err(error) = result {
        if owner_conflict {
            if let Some(receipt) = record.receipt.as_mut() {
                receipt.undo_started = false;
            }
            if let Err(error) = save_record(&journal, &record) {
                return refusal("storage_error", error, StatusCode::INTERNAL_SERVER_ERROR);
            }
            return undo_refused(&journal, &record);
        }

        return refusal("undo_incomplete", error, StatusCode::INTERNAL_SERVER_ERROR);
    }
    if receipt.undo_asked.is_none() {
        let result = if activities.is_empty() {
            Ok(false)
        } else {
            ask_once(&journal, &record.id, "undo", day, &activities).map(|()| true)
        };

        match result {
            Ok(asked) => {
                receipt.undo_asked = Some(asked);
                receipt.undo_ask_reason = None;
            }
            Err(error) => {
                receipt.undo_asked = Some(false);
                receipt.undo_ask_reason = Some(error.reason_code().to_owned());
            }
        }
    }
    if let Ok(Some(outcome)) = super::store::read_ask_outcome(&journal, &record.id, "undo") {
        receipt.undo_asked = Some(outcome.asked);
        receipt.undo_ask_reason = outcome.reason;
    }
    record.asked = receipt.undo_asked;
    record.ask_reason = receipt.undo_ask_reason.clone();
    record.receipt = Some(receipt);
    #[cfg(test)]
    if crate::suggestions::force_suggestion_receipt_failure() {
        return refusal(
            "receipt_failed",
            "forced receipt failure",
            StatusCode::INTERNAL_SERVER_ERROR,
        );
    }
    if let Err(error) = save_record(&journal, &record) {
        return refusal("storage_error", error, StatusCode::INTERNAL_SERVER_ERROR);
    }
    record.status = "undone".to_owned();
    record.updated_at = now;
    if let Err(error) = save_record(&journal, &record) {
        return refusal("storage_error", error, StatusCode::INTERNAL_SERVER_ERROR);
    }
    Json(json!({"suggestion":project_card(&journal, &record)})).into_response()
}

fn undo_refused(root: &Path, record: &SuggestionRecord) -> Response {
    let mut card = project_card(root, record);
    card["can_undo"] = Value::Bool(false);
    (
        StatusCode::CONFLICT,
        Json(json!({"code":"undo_refused", "reason_code":"undo_refused", "suggestion":card})),
    )
        .into_response()
}

fn later_field_answer(
    activity: &solstone_core_facets::ActivityRecord,
    position: usize,
    kind: &str,
) -> bool {
    let field = if kind == "recording" {
        "playback"
    } else {
        "details"
    };
    activity
        .get("edits")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .skip(position + 1)
        .any(|edit| {
            edit.get("actor").and_then(Value::as_str) == Some("owner:update")
                && edit.get("set").and_then(|set| set.get(field)).is_some()
        })
}

pub async fn set_connection_suggestions(
    Extension(journal): Extension<Arc<PathBuf>>,
    AxumPath((kind, id)): AxumPath<(String, String)>,
    Json(body): Json<SetSuggestionsBody>,
) -> Response {
    if !matches!(kind.as_str(), "bearer" | "oauth") {
        return refusal(
            "invalid_kind",
            "unknown connection kind",
            StatusCode::BAD_REQUEST,
        );
    }
    let authority_id = format!("{kind}:{id}");

    let label = if kind == "bearer" {
        let store = crate::tokens::TokenStore::open(&journal);
        match store.list() {
            Ok(tokens) => tokens.into_iter().find(|t| t.id == id).map(|t| t.label),
            Err(_) => None,
        }
    } else {
        let oauth = crate::oauth::store::OAuthStore::open(&journal);
        match oauth.list_grants() {
            Ok(grants) => grants
                .into_iter()
                .find(|grant| grant.id == id)
                .map(|grant| grant.client_name.unwrap_or(grant.client_id)),
            Err(_) => None,
        }
    };

    let Some(label) = label else {
        return refusal("not_found", "connection not found", StatusCode::NOT_FOUND);
    };

    let sanitized = sanitize_label(&label);
    if let Err(err) =
        update_connection_enabled(&journal, &authority_id, body.enabled, Some(&sanitized))
    {
        return refusal("storage_error", err, StatusCode::INTERNAL_SERVER_ERROR);
    }

    Json(json!({ "enabled": body.enabled })).into_response()
}

pub fn project_card(journal_root: &Path, record: &SuggestionRecord) -> Value {
    let status = effective_status(journal_root, record);

    let can_undo = compute_can_undo(journal_root, record);
    let editable = matches!(status, "pending" | "stale")
        && matches!(record.kind.as_str(), "what_happened" | "name");

    let agent_label = live_or_stored_label(journal_root, &record.source, &record.label);

    let (now_val, now_lines, place_val, impact_val) = compute_context(journal_root, record);

    let change_parts = match record.kind.as_str() {
        "recording" => json!({}),
        "what_happened" => json!({"text":record.value.get("text")}),
        "who_said_it" => json!({"speaker":suggested_speaker(journal_root, record),
            "line_count":record.target.get("sentences").and_then(Value::as_array).map_or(0, Vec::len)}),
        "name" => {
            json!({"from":record.baseline.get("name"), "to":record.value.get("text"), "mode":record.value.get("mode")})
        }
        "same_person" => same_person::change_parts(journal_root, record, status),
        _ => json!({}),
    };
    let part = |key| {
        record
            .target
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned()
    };
    let place = match record.kind.as_str() {
        "same_person" => solstone_core_journal_link::Place::Person {
            id: record.value["keep_id"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
        },
        "name" => solstone_core_journal_link::Place::Person {
            id: part("entity_id"),
        },
        "who_said_it" => solstone_core_journal_link::Place::Recording {
            day: part("day"),
            stream: part("stream"),
            segment: part("segment"),
        },
        _ => solstone_core_journal_link::Place::Activity {
            facet: part("facet"),
            day: part("day"),
            id: part("activity_id"),
        },
    };
    let open_url = solstone_core_journal_link::seal(journal_root, &place)
        .map(|sealed| format!("/app/open/{sealed}"));

    let mut card = json!({
        "id": record.id,
        "kind": record.kind,
        "status": status,
        "agent": {
            "source": record.source,
            "label": agent_label,
        },
        "basis": record.basis,
        "reason": record.reason,
        "created_at": record.created_at,
        "updated_at": record.updated_at,
        "target": record.target,
        "value": record.value,
        "now": now_val,
        "now_lines": now_lines,
        "change_parts": change_parts,
        "place": place_val,
        "impact": impact_val,
        "open_url": open_url,
        "editable": editable,
        "can_undo": can_undo,
        "incomplete": record.receipt.as_ref().is_some_and(|receipt|receipt.incomplete),
    });

    if record.kind == "same_person" && status == "stale" {
        card["stale_reason"] = json!(same_person::stale_reason(journal_root, record));
    }
    if let Some(asked) = record.asked {
        card["asked"] = Value::Bool(asked);
    }
    if let Some(reason) = &record.ask_reason {
        card["ask_reason"] = Value::String(reason.clone());
    }

    card
}

fn compute_can_undo(journal_root: &Path, record: &SuggestionRecord) -> bool {
    if record.status != "accepted" {
        return false;
    }
    let Some(receipt) = &record.receipt else {
        return false;
    };
    if receipt.undo_started {
        return true;
    }

    match record.kind.as_str() {
        "recording" | "what_happened" => {
            let facet = receipt.facet.as_deref().unwrap_or("");
            let day = receipt.day.as_deref().unwrap_or("");
            let act_id = receipt.activity_id.as_deref().unwrap_or("");
            let pos = receipt.edit_position.unwrap_or(0);
            let ts = receipt.edit_timestamp.as_deref().unwrap_or("");

            let Ok(Some(act)) =
                solstone_core_facets::get_activity_record(journal_root, facet, day, act_id)
            else {
                return false;
            };
            let Some(edits) = act.get("edits").and_then(Value::as_array) else {
                return false;
            };
            let Some(edit) = edits.get(pos).and_then(Value::as_object) else {
                return false;
            };
            if edit.get("timestamp").and_then(Value::as_str) != Some(ts) {
                return false;
            }
            if solstone_core_facets::edit_removed(edit) {
                return false;
            }
            if later_field_answer(&act, pos, &record.kind) {
                return false;
            }
            if let Some(stretch) = receipt.stretch_id.as_deref() {
                for (_, copy, _) in solstone_core_facets::stretch::activities_over_stretch(
                    journal_root,
                    day,
                    facet,
                    &act,
                ) {
                    if let Some((position, _)) = copy
                        .get("edits")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .enumerate()
                        .find(|(_, edit)| {
                            edit.as_object().is_some_and(|edit| {
                                solstone_core_facets::stretch::edit_stretch_id(edit)
                                    == Some(stretch)
                            })
                        })
                        && later_field_answer(&copy, position, &record.kind)
                    {
                        return false;
                    }
                }
            }
            true
        }
        "who_said_it" => {
            let facet = receipt.facet.as_deref().unwrap_or("");
            let day = receipt.day.as_deref().unwrap_or("");
            let act_id = receipt.activity_id.as_deref().unwrap_or("");
            let pos = receipt.edit_position.unwrap_or(0);

            let Ok(Some(act)) =
                solstone_core_facets::get_activity_record(journal_root, facet, day, act_id)
            else {
                return false;
            };
            let Some(edits) = act.get("edits").and_then(Value::as_array) else {
                return false;
            };
            if edits
                .get(pos)
                .and_then(Value::as_object)
                .is_none_or(solstone_core_facets::edit_removed)
            {
                return false;
            }
            let Some(segment) = suggestion_segment(journal_root, record) else {
                return false;
            };
            let Ok(entries) =
                solstone_core_speaker_resolve::owner_line_labels::current_owner_line_entries(
                    &segment,
                )
            else {
                return false;
            };
            let source = record
                .target
                .get("source")
                .and_then(Value::as_str)
                .unwrap_or("");
            if receipt
                .line_reverts
                .as_deref()
                .unwrap_or(&[])
                .iter()
                .any(|line| {
                    !entries.iter().any(|entry| {
                        entry.source == source
                            && Some(entry.sentence_id)
                                == line.get("sentence_id").and_then(Value::as_i64)
                            && Some(entry.set_at.as_str())
                                == line.get("set_at").and_then(Value::as_str)
                    })
                })
            {
                return false;
            }
            true
        }
        "name" => {
            let entity_id = receipt.entity_id.as_deref().unwrap_or("");
            let Some(identity_after) = &receipt.identity_after else {
                return false;
            };
            let Some(version) = receipt.version_id.as_deref() else {
                return false;
            };
            solstone_core_entity::identity_name_restore_allowed(
                journal_root,
                entity_id,
                identity_after,
                version,
            )
            .unwrap_or(false)
        }

        _ => false,
    }
}

pub(crate) fn effective_status<'a>(journal_root: &Path, record: &'a SuggestionRecord) -> &'a str {
    if record.status == "pending"
        && (record.receipt.is_some() || find_existing_effect(journal_root, record).is_some())
    {
        "accepted"
    } else if record.status == "pending" && is_record_stale(journal_root, record) {
        "stale"
    } else {
        &record.status
    }
}

pub fn is_record_stale(journal_root: &Path, record: &SuggestionRecord) -> bool {
    match record.kind.as_str() {
        "same_person" => same_person::is_stale(journal_root, record),
        "recording" | "what_happened" => {
            let facet = record
                .target
                .get("facet")
                .and_then(Value::as_str)
                .unwrap_or("");
            let day = record
                .target
                .get("day")
                .and_then(Value::as_str)
                .unwrap_or("");
            let id = record
                .target
                .get("activity_id")
                .and_then(Value::as_str)
                .unwrap_or("");
            let Ok(Some(act)) =
                solstone_core_facets::get_activity_record(journal_root, facet, day, id)
            else {
                return true;
            };
            activity_baseline(&record.kind, &act) != record.baseline
        }
        "who_said_it" => {
            let day = record
                .target
                .get("day")
                .and_then(Value::as_str)
                .unwrap_or("");
            let stream = record
                .target
                .get("stream")
                .and_then(Value::as_str)
                .unwrap_or("");
            let segment = record
                .target
                .get("segment")
                .and_then(Value::as_str)
                .unwrap_or("");
            if !crate::source_release::check_segment_transcript_release(
                journal_root,
                day,
                stream,
                segment,
            ) {
                return true;
            }
            line_baseline(journal_root, &record.target, &record.value).as_ref()
                != Some(&record.baseline)
                || find_activities_spanning_segment(
                    journal_root,
                    &format!("{day}/{stream}/{segment}"),
                )
                .is_empty()
        }

        "name" => {
            let entity_id = record
                .target
                .get("entity_id")
                .and_then(Value::as_str)
                .unwrap_or("");
            let Ok(Some(live_snapshot)) =
                solstone_core_entity::read_entity_identity(journal_root, entity_id)
            else {
                return true;
            };
            name_baseline(journal_root, entity_id, live_snapshot.value()) != record.baseline
        }

        _ => false,
    }
}

fn live_or_stored_label(journal_root: &Path, source_hex: &str, default_label: &str) -> String {
    if let Some(label) = active_agent_labels(journal_root).get(source_hex) {
        return label.clone();
    }
    let settings = read_settings(journal_root).unwrap_or_default();
    if let Some(l) = settings.labels.get(source_hex)
        && !l.is_empty()
    {
        return l.clone();
    }
    default_label.to_owned()
}

fn compute_context(root: &Path, record: &SuggestionRecord) -> (Value, Value, Value, Value) {
    let text = |key| record.target.get(key).and_then(Value::as_str).unwrap_or("");
    let day = text("day");
    let facet = text("facet");
    let empty = json!({"activities":[],"facets":[],"days":[]});
    match record.kind.as_str() {
        "recording" | "what_happened" => {
            let activity =
                solstone_core_facets::get_activity_record(root, facet, day, text("activity_id"))
                    .ok()
                    .flatten();
            let field = if record.kind == "recording" {
                "playback"
            } else {
                "details"
            };
            let now = if field == "playback" {
                json!(if activity
                    .as_ref()
                    .and_then(|act| act.get(field))
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    "on"
                } else {
                    "off"
                })
            } else {
                json!(activity.as_ref().map(activity_account).unwrap_or(""))
            };
            let place = json!({"kind":"activity", "title":activity.as_ref().and_then(|act|act.get("title")), "day":day,
                "start":activity.as_ref().and_then(|act|act.get("start")), "end":activity.as_ref().and_then(|act|act.get("end")), "facets":[facet]});
            let mut affected = Vec::new();
            if let Some(activity) =
                activity.filter(|act| !solstone_core_facets::is_body_record(act))
            {
                affected.push((
                    facet.to_owned(),
                    day.to_owned(),
                    text("activity_id").to_owned(),
                    Value::Object(activity.clone()),
                ));
                affected.extend(
                    solstone_core_facets::stretch::activities_over_stretch(
                        root, day, facet, &activity,
                    )
                    .into_iter()
                    .map(|(facet, act, _)| {
                        (
                            facet,
                            day.to_owned(),
                            act.get("id")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_owned(),
                            Value::Object(act),
                        )
                    }),
                );
            }
            if record
                .receipt
                .as_ref()
                .is_some_and(|receipt| receipt.incomplete)
            {
                affected.retain(|(_, _, _, activity)| {
                    activity
                        .get("edits")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .any(|edit| {
                            edit.get("suggestion_id").and_then(Value::as_str)
                                == Some(record.id.as_str())
                        })
                });
            }
            (now, json!([]), place, impact(&affected))
        }
        "who_said_it" => {
            let stream = text("stream");
            let segment = text("segment");
            let Some(directory) = suggestion_segment(root, record) else {
                return (json!(""), json!([]), json!({"kind":"segment"}), empty);
            };
            let affected =
                find_activities_spanning_segment(root, &format!("{day}/{stream}/{segment}"));
            let first = affected.first().map(|(_, _, _, act)| act);
            let place = json!({"kind":"segment", "title":first.and_then(|act|act.get("title")), "day":day,
                "start":first.and_then(|act|act.get("start")), "end":first.and_then(|act|act.get("end")),
                "facets":affected.iter().map(|(facet,_,_,_)|facet).collect::<BTreeSet<_>>()});
            let targets: BTreeSet<i64> = record
                .target
                .get("sentences")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|line| line.get("sentence_id").and_then(Value::as_i64))
                .collect();
            let parsed = super::submit::load_segment_statements(&directory).unwrap_or_default();
            let who =
                solstone_core_transcripts::load_who_spoke(&directory, Some(&parsed.source), None);
            let voices = solstone_core_talent_runtime::voice_names(root);
            let numbers = voices
                .as_ref()
                .map(|_| {
                    solstone_core_transcripts::run_numbers_by_source(
                        &directory,
                        &[directory.join(format!("{}.jsonl", parsed.source))],
                    )
                    .remove(&parsed.source)
                    .unwrap_or_default()
                })
                .unwrap_or_default();
            let rel = format!("{day}/{stream}/{segment}/{}.jsonl", parsed.source);
            let now_lines: Vec<Value> = parsed
                .statements
                .into_iter()
                .filter(|line| targets.contains(&line.sentence_id))
                .map(|line| {
                    let named = voices.as_ref().and_then(|voices| {
                        who.rows
                            .iter()
                            .find(|row| {
                                row.get("sentence_id").and_then(Value::as_i64)
                                    == Some(line.sentence_id)
                            })
                            .and_then(|row| voices.name_for(row))
                    });
                    let mut raw = Map::new();
                    if let Some(speaker) = line.speaker {
                        raw.insert("speaker".to_owned(), speaker);
                    }
                    if let Some(name) = named {
                        raw.insert("speaker".to_owned(), json!(name));
                    } else if let Some(number) = raw
                        .get("speaker")
                        .and_then(Value::as_i64)
                        .and_then(|number| numbers.get(&number))
                    {
                        raw.insert("speaker".to_owned(), json!(number));
                    }
                    let speaker = solstone_core_format::content::audio_speaker_label(
                        &rel,
                        parsed.metadata.as_ref(),
                        &raw,
                    )
                    .unwrap_or_default();
                    json!({"time":line.time,"speaker":speaker,"text":line.text})
                })
                .collect();
            let now = now_lines
                .iter()
                .filter_map(|line| line.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n");
            (json!(now), json!(now_lines), place, impact(&affected))
        }
        "name" => {
            let entity = solstone_core_entity::read_entity_identity(root, text("entity_id"))
                .ok()
                .flatten();
            let name = entity
                .as_ref()
                .and_then(|entity| entity.value().get("name"))
                .cloned()
                .unwrap_or(json!(""));
            let place = json!({"kind":"entity","name":name,"type":entity.as_ref().and_then(|entity|entity.value().get("type"))});
            (name, json!([]), place, empty)
        }
        _ => (Value::Null, json!([]), json!({}), empty),
    }
}

fn impact(activities: &[(String, String, String, Value)]) -> Value {
    json!({"activities":activities.iter().map(|(facet,day,id,act)|json!({"day":day,"facet":facet,"id":id,"title":act.get("title")})).collect::<Vec<_>>(),
        "facets":activities.iter().map(|(facet,_,_,_)|facet).collect::<BTreeSet<_>>(),
        "days":activities.iter().map(|(_,day,_,_)|day).collect::<BTreeSet<_>>()})
}

fn speaker_name(root: &Path, id: &str) -> String {
    solstone_core_entity::read_entity_identity(root, id)
        .ok()
        .flatten()
        .and_then(|entity| {
            entity
                .value()
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_default()
}

fn suggested_speaker(root: &Path, record: &SuggestionRecord) -> Value {
    let speaker = record.value.get("speaker");
    match speaker.and_then(Value::as_str) {
        Some("owner") => {
            json!({"kind":"owner","label":solstone_core_entity::read_journal_principal(root).ok().flatten().and_then(|identity|identity.get("name").cloned())})
        }
        Some("someone_else") => json!({"kind":"someone_else","label":""}),
        _ => {
            let id = speaker
                .and_then(|speaker| speaker.get("person"))
                .and_then(Value::as_str)
                .unwrap_or("");
            json!({"kind":"person","label":speaker_name(root,id)})
        }
    }
}

fn find_activities_spanning_segment(
    journal_root: &Path,
    segment_origin: &str,
) -> Vec<(String, String, String, Value)> {
    let Some((day, rest)) = segment_origin.split_once('/') else {
        return Vec::new();
    };
    let Some((stream, segment)) = rest.split_once('/') else {
        return Vec::new();
    };
    let segments = BTreeSet::from([(stream.to_owned(), segment.to_owned())]);
    solstone_core_system::owner_ask::activities_over(journal_root, day, &segments)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|activity| {
            solstone_core_facets::get_activity_record(
                journal_root,
                &activity.facet,
                day,
                &activity.activity,
            )
            .ok()
            .flatten()
            .map(|record| {
                (
                    activity.facet,
                    day.to_owned(),
                    activity.activity,
                    Value::Object(record),
                )
            })
        })
        .collect()
}

fn list_agents_summary(journal_root: &Path) -> Vec<Value> {
    let settings = read_settings(journal_root).unwrap_or_default();
    let records = list_records(journal_root).unwrap_or_default();
    let active = active_agent_labels(journal_root);
    let mut labels = active.clone();
    let mut pending_by_source: BTreeMap<String, usize> = BTreeMap::new();
    for rec in records {
        labels.entry(rec.source.clone()).or_insert_with(|| {
            settings
                .labels
                .get(&rec.source)
                .cloned()
                .unwrap_or_else(|| rec.label.clone())
        });
        if effective_status(journal_root, &rec) == "pending" {
            *pending_by_source.entry(rec.source.clone()).or_insert(0) += 1;
        }
    }
    labels
        .into_iter()
        .map(|(source, label)| {
            json!({
                "pending_count": pending_by_source.get(&source).copied().unwrap_or(0),
                "connected": active.contains_key(&source),
                "source": source, "label": sanitize_label(&label),
            })
        })
        .collect()
}

fn active_agent_labels(journal_root: &Path) -> BTreeMap<String, String> {
    let tokens = crate::tokens::TokenStore::open(journal_root)
        .list()
        .unwrap_or_default()
        .into_iter()
        .map(|token| (format!("bearer:{}", token.id), token.label));
    let grants = crate::oauth::store::OAuthStore::open(journal_root)
        .list_grants()
        .unwrap_or_default()
        .into_iter()
        .map(|grant| {
            (
                format!("oauth:{}", grant.id),
                grant.client_name.unwrap_or(grant.client_id),
            )
        });
    tokens
        .chain(grants)
        .filter_map(|(connection, label)| {
            crate::agent_state::memory_source(journal_root, &connection)
                .ok()
                .map(|source| (source.component().to_owned(), sanitize_label(&label)))
        })
        .collect()
}

fn encode_cursor(created_at: &str, id: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(format!("{created_at}\n{id}"))
}

fn decode_cursor(token: &str) -> Option<(String, String)> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(token)
        .ok()?;
    let text = String::from_utf8(bytes).ok()?;
    let (created_at, id) = text.split_once('\n')?;
    Some((created_at.to_owned(), id.to_owned()))
}

pub(crate) fn activity_account(record: &solstone_core_facets::ActivityRecord) -> &str {
    record
        .get("details")
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty())
        .or_else(|| record.get("description").and_then(Value::as_str))
        .unwrap_or("")
}

pub(crate) fn activity_baseline(
    kind: &str,
    record: &solstone_core_facets::ActivityRecord,
) -> Value {
    let field = if kind == "recording" {
        "playback"
    } else {
        "details"
    };
    let answers: Vec<Value> = record
        .get("edits")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|edit| {
            edit.get("actor").and_then(Value::as_str) == Some("owner:update")
                && edit.get("set").and_then(|set| set.get(field)).is_some()
        })
        .cloned()
        .collect();
    let value = if kind == "recording" {
        json!(record.get(field).and_then(Value::as_bool).unwrap_or(false))
    } else {
        json!(super::submit::normalize_suggestion_text(activity_account(
            record
        )))
    };
    json!({"id":record.get("id"), "hidden":record.get("hidden").is_some_and(solstone_core_facets::activity_value_truthy), "source":record.get("source"),
        "stream":record.get("stream"), "segments":record.get("segments"),
        "value":value, "owner_answers":answers})
}

pub(crate) fn name_baseline(root: &Path, entity: &str, identity: &Value) -> Value {
    let answers: Vec<Value> = solstone_core_entity::read_visible_history(root, entity)
        .unwrap_or_default()
        .into_iter()
        .filter(|event| {
            event
                .value()
                .get("operation")
                .and_then(|op| op.get("owner_answer"))
                == Some(&Value::Bool(true))
        })
        .map(|event| {
            event
                .value()
                .get("version_id")
                .cloned()
                .unwrap_or(Value::Null)
        })
        .collect();
    json!({"name":identity.get("name"), "aka":identity.get("aka"), "type":identity.get("type"),
        "blocked":identity.get("blocked"), "owner_answers":answers})
}

fn affected_activities(
    root: &Path,
    record: &SuggestionRecord,
    receipt: &SuggestionReceipt,
) -> Vec<solstone_core_system::owner_ask::AskedActivity> {
    let (Some(facet), Some(day), Some(id)) = (&receipt.facet, &receipt.day, &receipt.activity_id)
    else {
        return Vec::new();
    };
    if record.kind == "who_said_it" {
        let stream = record
            .target
            .get("stream")
            .and_then(Value::as_str)
            .unwrap_or("");
        let segment = record
            .target
            .get("segment")
            .and_then(Value::as_str)
            .unwrap_or("");
        return find_activities_spanning_segment(root, &format!("{day}/{stream}/{segment}"))
            .into_iter()
            .filter(|(_, _, _, act)| {
                !solstone_core_facets::is_body_record(act.as_object().expect("activity object"))
            })
            .map(
                |(facet, _, activity, _)| solstone_core_system::owner_ask::AskedActivity {
                    facet,
                    activity,
                },
            )
            .collect();
    }
    let Ok(Some(activity)) = solstone_core_facets::get_activity_record(root, facet, day, id) else {
        return Vec::new();
    };
    if solstone_core_facets::is_body_record(&activity) {
        return Vec::new();
    }
    let mut activities: Vec<_> = std::iter::once(solstone_core_system::owner_ask::AskedActivity {
        facet: facet.clone(),
        activity: id.clone(),
    })
    .chain(
        solstone_core_facets::stretch::activities_over_stretch(root, day, facet, &activity)
            .into_iter()
            .map(
                |(facet, act, _)| solstone_core_system::owner_ask::AskedActivity {
                    facet,
                    activity: act
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned(),
                },
            ),
    )
    .collect();
    if receipt.incomplete {
        activities.retain(|asked| {
            solstone_core_facets::get_activity_record(root, &asked.facet, day, &asked.activity)
                .ok()
                .flatten()
                .is_some_and(|activity| {
                    activity
                        .get("edits")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .any(|edit| {
                            edit.get("suggestion_id").and_then(Value::as_str)
                                == Some(record.id.as_str())
                        })
                })
        });
    }
    activities
}

fn reconcile_pending_effect(root: &Path, record: &mut SuggestionRecord) -> Result<(), String> {
    if record.status != "pending" {
        return Ok(());
    }
    let effect = record
        .receipt
        .clone()
        .map(|receipt| (record.value.clone(), receipt))
        .or_else(|| find_existing_effect(root, record));
    let Some((value, mut receipt)) = effect else {
        return Ok(());
    };
    if record.receipt.is_none() && matches!(record.kind.as_str(), "recording" | "what_happened") {
        let facet = receipt.facet.as_deref().ok_or("missing facet")?;
        let day = receipt.day.as_deref().ok_or("missing day")?;
        let activity = receipt.activity_id.as_deref().ok_or("missing activity")?;
        let timestamp = receipt
            .edit_timestamp
            .as_deref()
            .ok_or("missing timestamp")?;
        let patch = if record.kind == "recording" {
            Map::from_iter([("playback".to_owned(), json!(true))])
        } else {
            Map::from_iter([(
                "details".to_owned(),
                value.get("text").cloned().unwrap_or_default(),
            )])
        };
        let extra = Map::from_iter([
            ("suggestion_id".to_owned(), json!(record.id)),
            (
                "suggested_by".to_owned(),
                json!({"source":record.source,"label":record.label}),
            ),
        ]);
        let applied = solstone_core_facets::stretch::correct_over_stretch(
            root,
            facet,
            day,
            activity,
            &patch,
            "owner:update",
            timestamp,
            |_| Ok(()),
            &extra,
        )
        .map_err(|error| error.to_string())?;
        if !applied.failed.is_empty() {
            receipt.incomplete = true;
            record.asked = Some(false);
            record.ask_reason = Some("stretch_incomplete".to_owned());
        }
    }
    if record.asked.is_none() && !receipt.incomplete {
        let activities = affected_activities(root, record, &receipt);
        if activities.is_empty() {
            record.asked = Some(false);
            record.ask_reason = None;
        } else {
            let result = ask_once(
                root,
                &record.id,
                "accept",
                receipt.day.as_deref().unwrap_or(""),
                &activities,
            );
            let outcome = super::store::read_ask_outcome(root, &record.id, "accept")
                .map_err(|error| error.to_string())?;
            if let Some(outcome) = outcome {
                record.asked = Some(outcome.asked);
                record.ask_reason = outcome.reason;
            } else {
                record.asked = Some(false);
                record.ask_reason = result.err().map(|error| error.reason_code().to_owned());
            }
        }
    }

    record.value = value;
    record.receipt = Some(receipt);
    record.status = "accepted".to_owned();
    record.updated_at = solstone_core_time::utc_rfc3339(jiff::Timestamp::now());
    save_record(root, record).map_err(|error| error.to_string())
}

fn suggestion_segment(root: &Path, record: &SuggestionRecord) -> Option<PathBuf> {
    let day = record.target.get("day")?.as_str()?;
    let stream = record.target.get("stream")?.as_str()?;
    let segment = record.target.get("segment")?.as_str()?;
    solstone_core_segment::SegmentDir::resolve(root, day, segment, stream)
        .ok()
        .map(|segment| segment.path().to_path_buf())
}

pub(crate) fn line_baseline(root: &Path, target: &Value, value: &Value) -> Option<Value> {
    let day = target.get("day")?.as_str()?;
    let stream = target.get("stream")?.as_str()?;
    let segment = target.get("segment")?.as_str()?;
    let directory = solstone_core_segment::SegmentDir::resolve(root, day, segment, stream).ok()?;
    let source =
        solstone_core_speaker_resolve::owner_line_labels::agreed_source(directory.path()).ok()?;
    if target.get("source").and_then(Value::as_str) != Some(source.as_str()) {
        return None;
    }
    let parsed = super::submit::load_segment_statements(directory.path()).ok()?;
    if parsed.untimed {
        return None;
    }
    let statements = parsed.statements;
    let ids: BTreeSet<i64> = target
        .get("sentences")?
        .as_array()?
        .iter()
        .filter_map(|line| line.get("sentence_id").and_then(Value::as_i64))
        .collect();
    let lines: Vec<Value> = statements
        .into_iter()
        .filter(|line| ids.contains(&line.sentence_id))
        .map(|line| json!({"sentence_id":line.sentence_id,"time":line.time,"text":line.text}))
        .collect();
    if lines.len() != ids.len() {
        return None;
    }
    let who = solstone_core_transcripts::load_who_spoke(directory.path(), Some(&source), None);
    if who.owner_unreadable.is_some() {
        return None;
    }
    let speakers:Vec<Value>=ids.iter().map(|id|json!({"sentence_id":id,"speaker":who.rows.iter()
        .find(|row|row.get("sentence_id").and_then(Value::as_i64)==Some(*id)).and_then(|row|row.get("speaker"))})).collect();
    let owner_tokens: Vec<Value> = ids
        .iter()
        .filter_map(|id| {
            who.owner_set
                .get(id)
                .map(|answer| json!({"sentence_id":id,"set_at":answer.set_at}))
        })
        .collect();
    let person = if let Some(id) = value
        .get("speaker")
        .and_then(|speaker| speaker.get("person"))
        .and_then(Value::as_str)
    {
        if solstone_core_entity::merged_away(root, id).ok()?.is_some() {
            return None;
        }
        let identity = solstone_core_entity::read_entity_identity(root, id).ok()??;
        if solstone_core_entity::entity_is_hidden(identity.value())
            || identity.value().get("type").and_then(Value::as_str) != Some("Person")
        {
            return None;
        }
        json!({"id":id,"type":identity.value().get("type"),"blocked":identity.value().get("blocked")})
    } else if value.get("speaker").and_then(Value::as_str) == Some("owner") {
        solstone_core_entity::read_journal_principal(root)
            .ok()?
            .and_then(|identity| identity.get("id").cloned())
            .unwrap_or(Value::Null)
    } else {
        Value::Null
    };
    Some(
        json!({"source":source,"lines":lines,"speakers":speakers,"owner_tokens":owner_tokens,"person":person}),
    )
}

fn ask_once(
    root: &Path,
    id: &str,
    action: &str,
    day: &str,
    activities: &[solstone_core_system::owner_ask::AskedActivity],
) -> Result<(), solstone_core_system::owner_ask::AskError> {
    use solstone_core_system::owner_ask::AskError;
    let cached = super::store::read_ask_outcome(root, id, action)
        .map_err(|error| AskError::Write(error.to_string()))?;
    if let Some(outcome) = cached {
        return if outcome.asked {
            Ok(())
        } else {
            Err(AskError::Write(outcome.reason.unwrap_or_default()))
        };
    }
    // Reserve the attempt durably before requesting work. An interrupted attempt is reported as unconfirmed and is never repeated.
    super::store::save_ask_outcome(
        root,
        id,
        action,
        &super::store::AskOutcome {
            asked: false,
            reason: Some("owner_ask_unconfirmed".to_owned()),
        },
    )
    .map_err(|error| AskError::Write(error.to_string()))?;
    let result = if crate::suggestions::force_suggestion_ask_failure() {
        Err(AskError::Write("injected_failure".to_owned()))
    } else {
        solstone_core_system::owner_ask::ask_corrected(root, day, activities)
    };
    let outcome = super::store::AskOutcome {
        asked: result.is_ok(),
        reason: result
            .as_ref()
            .err()
            .map(|error| error.reason_code().to_owned()),
    };
    super::store::save_ask_outcome(root, id, action, &outcome)
        .map_err(|error| AskError::Write(error.to_string()))?;
    result
}
