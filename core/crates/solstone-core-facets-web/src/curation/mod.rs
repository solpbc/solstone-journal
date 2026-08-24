// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native owner-facing curation routes.

use std::path::{Path, PathBuf};

use axum::{
    Json, Router,
    body::Bytes,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde_json::{Map, Value, json};
use solstone_core_entity::{
    EncoderIdentity, EntityMergeError, EntityMergeOptions, EntityReviewCandidateError,
    EntityWriteError, LockError, MalformedPolicy, accept_merge_candidate, commit_entity_merge,
    dismiss_merge_candidate, load_merge_candidates, preview_entity_merge, read_ambiguities,
};
use solstone_core_speaker_resolve::{
    candidate_tracker::CandidateTracker, keep_separate::find_assertion,
    speaker_candidate_pair_review_candidates as pair_store,
    speaker_review_candidates as speaker_store,
};

use crate::{assets, http};

mod copy;

const ENTITY_BUSY: &str = "entity merge candidates are busy; try again";
const SPEAKER_BUSY: &str = "speaker suggestions are busy; try again";

pub fn routes(root: PathBuf) -> Router {
    Router::new()
        .route("/app/curation/", get(|| async { assets::shell() }))
        .route(
            "/app/curation/workspace",
            get(|| async { assets::curation_workspace() }),
        )
        .route(
            "/app/curation/static/curation_evidence.js",
            get(|| async { assets::curation_evidence_js() }),
        )
        .route("/app/curation/api/state", get(state))
        .route("/app/curation/api/entity/preview", post(entity_preview))
        .route("/app/curation/api/entity/accept", post(entity_accept))
        .route("/app/curation/api/entity/dismiss", post(entity_dismiss))
        .route(
            "/app/curation/api/entity/accept-batch",
            post(entity_accept_batch),
        )
        .route(
            "/app/curation/api/entity/dismiss-batch",
            post(entity_dismiss_batch),
        )
        .route("/app/curation/api/speaker/preview", post(speaker_preview))
        .route("/app/curation/api/speaker/accept", post(speaker_accept))
        .route("/app/curation/api/speaker/dismiss", post(speaker_dismiss))
        .route(
            "/app/curation/api/speaker-candidate-pair/accept",
            post(pair_accept),
        )
        .route(
            "/app/curation/api/speaker-candidate-pair/dismiss",
            post(pair_dismiss),
        )
        .with_state(root)
}

async fn state(State(root): State<PathBuf>) -> Response {
    if let Some(response) = corrupt_config(&root) {
        return response;
    }
    match load_state(&root) {
        Ok(value) => Json(value).into_response(),
        Err(error) => http::error(
            "entity_operation_failed",
            "I couldn't complete that entity operation.",
            error,
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    }
}

fn load_state(root: &Path) -> Result<Value, String> {
    let facet_items = solstone_core_facets::load_candidates(root)
        .map_err(|error| error.to_string())?
        .into_iter()
        .filter(|row| row.get("status").and_then(Value::as_str) == Some("open"))
        .map(facet_item)
        .collect::<Vec<_>>();
    let entity_items = load_merge_candidates(root, None, Some("open"))
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(entity_item)
        .collect::<Vec<_>>();
    let ambiguity_items = read_ambiguities(root, MalformedPolicy::Raise)
        .map_err(|error| error.to_string())?
        .into_iter()
        .filter(|row| row.get("status").and_then(Value::as_str) == Some("open"))
        .map(ambiguity_item)
        .collect::<Vec<_>>();
    let speaker_items = speaker_store::load_candidates(root)
        .map_err(|error| error.to_string())?
        .into_iter()
        .filter(|row| include_speaker(root, row))
        .map(speaker_item)
        .collect::<Vec<_>>();
    let pair_items = pair_store::load_candidates(root)
        .map_err(|error| error.to_string())?
        .into_iter()
        .filter(|row| row.get("status").and_then(Value::as_str) == Some("open"))
        .map(pair_item)
        .collect::<Vec<_>>();
    Ok(json!({
        "facet_items": sorted(facet_items),
        "entity_items": sorted(entity_items),
        "ambiguity_items": sorted(ambiguity_items),
        "speaker_items": sorted(speaker_items),
        "speaker_candidate_pair_items": sorted(pair_items),
        "copy": copy::payload(),
    }))
}

fn sorted(mut items: Vec<Value>) -> Vec<Value> {
    items.sort_by(|left, right| {
        let left_composite = left
            .get("composite")
            .and_then(Value::as_f64)
            .unwrap_or_default();
        let right_composite = right
            .get("composite")
            .and_then(Value::as_f64)
            .unwrap_or_default();
        right_composite.total_cmp(&left_composite).then_with(|| {
            left["key"]
                .as_str()
                .unwrap_or_default()
                .cmp(right["key"].as_str().unwrap_or_default())
        })
    });
    items
}

fn integer(value: Option<&Value>) -> i64 {
    value
        .and_then(Value::as_i64)
        .or_else(|| {
            value
                .and_then(Value::as_str)
                .and_then(|value| value.parse().ok())
        })
        .unwrap_or_default()
}

#[allow(clippy::too_many_arguments)] // The API item shape has these exact independent fields.
fn item(
    kind: &str,
    key: String,
    name: Option<String>,
    facet: Option<String>,
    source: Option<String>,
    source_slug: Option<String>,
    target: Option<String>,
    target_slug: Option<String>,
    evidence: Value,
    strength: i64,
) -> Value {
    json!({
        "kind": kind, "key": key, "name": name, "facet": facet,
        "source": source, "source_slug": source_slug, "target": target,
        "target_slug": target_slug, "evidence": evidence,
        "strength": strength, "composite": strength as f64,
    })
}

fn object(row: &Value, key: &str) -> serde_json::Map<String, Value> {
    row.get(key)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

fn facet_item(row: Value) -> Value {
    let strength = integer(row.get("count"));
    let mut evidence = object(&row, "evidence");
    evidence.insert("count".to_owned(), Value::from(strength));
    evidence.insert(
        "window_days".to_owned(),
        row.get("window_days").cloned().unwrap_or(Value::Null),
    );
    let key = string(&row, "name_key");
    item(
        "facet_candidate",
        key.clone(),
        Some(string_or(&row, "name", &key)),
        None,
        None,
        None,
        None,
        None,
        Value::Object(evidence),
        strength,
    )
}

fn entity_item(row: Value) -> Value {
    let mut evidence = object(&row, "evidence");
    let strength = integer(evidence.get("detection_count"));
    // There is deliberately no native neighborhood/Jaccard reader. Keeping this
    // branch degraded is observable contract: only the strength contributes.
    evidence.insert("composite".to_owned(), Value::from(strength as f64));
    let facet = string(&row, "facet");
    let source_slug = string(&row, "source_slug");
    let target_slug = string(&row, "target_slug");
    item(
        "entity_merge",
        format!("{facet}|{source_slug}|{target_slug}"),
        None,
        Some(facet),
        Some(string_or(&row, "source", &source_slug)),
        Some(source_slug),
        Some(string_or(&row, "target", &target_slug)),
        Some(target_slug),
        Value::Object(evidence),
        strength,
    )
}

fn ambiguity_item(row: Value) -> Value {
    let query = row
        .get("original_query")
        .or_else(|| row.get("latest_query"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let mut evidence = serde_json::Map::new();
    for key in [
        "observed_tier",
        "ranked_candidates",
        "origins",
        "occurrence_count",
    ] {
        evidence.insert(
            key.to_owned(),
            row.get(key).cloned().unwrap_or_else(|| {
                if key == "ranked_candidates" || key == "origins" {
                    json!([])
                } else {
                    Value::from(0)
                }
            }),
        );
    }
    item(
        "entity_ambiguity",
        string(&row, "ambiguity_id"),
        Some(query.clone()),
        None,
        Some(query),
        None,
        None,
        None,
        Value::Object(evidence),
        integer(row.get("occurrence_count")),
    )
}

fn include_speaker(root: &Path, row: &Value) -> bool {
    let status = row.get("status").and_then(Value::as_str);
    if status != Some("open")
        && !(status == Some("suppressed")
            && row.get("suppressed_by_keep_separate") == Some(&Value::Bool(true)))
    {
        return false;
    }
    let evidence = object(row, "evidence");
    let detection_count = integer(evidence.get("detection_count")).max(1);
    let source_id = string(row, "source_id");
    let target_id = string(row, "target_id");
    find_assertion(root, &source_id, &target_id)
        .ok()
        .flatten()
        .is_none_or(|assertion| {
            assertion
                .sources
                .iter()
                .all(|source| detection_count > source.detection_count)
        })
}

fn speaker_item(row: Value) -> Value {
    let mut evidence = object(&row, "evidence");
    let similarity = row
        .get("similarity")
        .and_then(Value::as_f64)
        .unwrap_or_default();
    evidence.insert("similarity".to_owned(), Value::from(similarity));
    evidence.insert(
        "readiness".to_owned(),
        row.get("readiness").cloned().unwrap_or(Value::Null),
    );
    let source_slug = string(&row, "source_id");
    let target_slug = string(&row, "target_id");
    item(
        "speaker_name_variant",
        speaker_key(&source_slug, &target_slug),
        None,
        None,
        Some(string_or(&row, "source_label", &source_slug)),
        Some(source_slug),
        Some(string_or(&row, "target_label", &target_slug)),
        Some(target_slug),
        Value::Object(evidence),
        (similarity * 100.0).round() as i64,
    )
}

fn pair_item(row: Value) -> Value {
    let mut evidence = object(&row, "evidence");
    let similarity = row
        .get("similarity")
        .and_then(Value::as_f64)
        .or_else(|| evidence.get("similarity").and_then(Value::as_f64))
        .unwrap_or_default();
    evidence.insert("similarity".to_owned(), Value::from(similarity));
    let source_slug = string(&row, "anchor_a");
    let target_slug = string(&row, "anchor_b");
    item(
        "speaker_candidate_pair",
        pair_key(&source_slug, &target_slug),
        None,
        None,
        Some("candidate A".to_owned()),
        Some(source_slug),
        Some("candidate B".to_owned()),
        Some(target_slug),
        Value::Object(evidence),
        (similarity * 100.0).round() as i64,
    )
}

fn string(row: &Value, key: &str) -> String {
    row.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}
fn string_or(row: &Value, key: &str, fallback: &str) -> String {
    row.get(key)
        .and_then(Value::as_str)
        .unwrap_or(fallback)
        .to_owned()
}
fn speaker_key(a: &str, b: &str) -> String {
    if a <= b {
        format!("{a}|{b}")
    } else {
        format!("{b}|{a}")
    }
}
fn pair_key(a: &str, b: &str) -> String {
    if a <= b {
        json!([a, b]).to_string()
    } else {
        json!([b, a]).to_string()
    }
}

async fn entity_preview(State(root): State<PathBuf>, body: Bytes) -> Response {
    let body = curation_body(&body);
    let (facet, source, target) = match entity_fields(&body) {
        Ok(fields) => fields,
        Err(response) => return *response,
    };
    let candidate = match entity_candidate(&root, &facet, &source, &target) {
        Ok(Some(value)) => value,
        Ok(None) => {
            return result_error(
                "entity_merge",
                entity_key(&facet, &source, &target),
                "candidate not found",
            );
        }
        Err(error) => return internal(error),
    };
    if string(&candidate, "status") != "open" {
        return result_error(
            "entity_merge",
            entity_key(&facet, &source, &target),
            &format!(
                "cannot preview candidate with status {}",
                string(&candidate, "status")
            ),
        );
    }
    // Native previews currently compute only identity additions. The remaining
    // response fields retain the Flask shape until core entity exposes them.
    match preview_entity_merge(&root, &source, &target, EntityMergeOptions::default()) {
        Ok(preview) => Json(json!({"status":"preview","kind":"entity_merge","key":entity_key(&facet,&source,&target),"merge":{"would_identity":{"akas_added":preview.aliases_added,"emails_added_count":preview.emails_added}},"preview":{"akas_added":preview.aliases_added,"emails_added_count":preview.emails_added,"facet_moved_count":0,"facet_merged_count":0,"observations_appended":0,"labels_rewritten":0,"corrections_rewritten":0,"segment_errors":[],"voiceprints_added":0,"voiceprints_target_total":0}})).into_response(),
        Err(error) => result_error("entity_merge", entity_key(&facet, &source, &target), &error.to_string()),
    }
}

async fn entity_accept(State(root): State<PathBuf>, body: Bytes) -> Response {
    let body = curation_body(&body);
    entity_transition(&root, body, true)
}
async fn entity_dismiss(State(root): State<PathBuf>, body: Bytes) -> Response {
    let body = curation_body(&body);
    entity_transition(&root, body, false)
}

fn entity_transition(root: &Path, body: Value, accept: bool) -> Response {
    let (facet, source, target) = match entity_fields(&body) {
        Ok(fields) => fields,
        Err(response) => return *response,
    };
    match entity_transition_value(root, &facet, &source, &target, accept) {
        Ok(value) => result_response(value),
        Err(TransitionFailure::Busy) => busy(ENTITY_BUSY),
        Err(TransitionFailure::Internal(error)) => internal(error),
    }
}

fn entity_transition_value(
    root: &Path,
    facet: &str,
    source: &str,
    target: &str,
    accept: bool,
) -> Result<Value, TransitionFailure> {
    let key = entity_key(facet, source, target);
    let candidate = match entity_candidate(root, facet, source, target) {
        Ok(Some(value)) => value,
        Ok(None) => {
            return Ok(result_error_value(
                "entity_merge",
                key,
                "candidate not found",
            ));
        }
        Err(error) => return Err(TransitionFailure::Internal(error)),
    };
    let status = string(&candidate, "status");
    if accept && status == "accepted" {
        let merge_id = merge_id(&candidate);
        return Ok(
            json!({"status":"already_accepted","kind":"entity_merge","key":key,"candidate":candidate,"merge_id":merge_id,"undo":entity_merge_undo(merge_id)}),
        );
    }
    if !accept && status == "dismissed" {
        return Ok(
            json!({"status":"already_dismissed","kind":"entity_merge","key":key,"candidate":candidate}),
        );
    }
    if status != "open" {
        return Ok(result_error_value(
            "entity_merge",
            key,
            &format!(
                "cannot {} candidate with status {status}",
                if accept { "accept" } else { "dismiss" }
            ),
        ));
    }
    if accept {
        let report = match commit_entity_merge(
            root,
            source,
            target,
            EntityMergeOptions::default(),
            &unresolved_voiceprint_encoder(),
        ) {
            Ok(report) => report,
            Err(error) if merge_busy(&error) => return Err(TransitionFailure::Busy),
            Err(error) => return Ok(result_error_value("entity_merge", key, &error.to_string())),
        };
        let merge_id = report.merge_id.clone();
        return match accept_merge_candidate(root, facet, source, target, Some(&merge_id)) {
            Ok(Some(candidate)) => Ok(
                json!({"status":"accepted","kind":"entity_merge","key":key,"merge":merge_report_value(&report),"candidate":candidate,"merge_id":merge_id,"undo":entity_merge_undo(Some(&merge_id))}),
            ),
            Ok(None) => Ok(result_error_value(
                "entity_merge",
                key,
                "candidate not found",
            )),
            Err(error) if entity_busy(&error) => Err(TransitionFailure::Busy),
            Err(error) => Err(TransitionFailure::Internal(error.to_string())),
        };
    }
    match dismiss_merge_candidate(root, facet, source, target) {
        Ok(Some(candidate)) => {
            Ok(json!({"status":"dismissed","kind":"entity_merge","key":key,"candidate":candidate}))
        }
        Ok(None) => Ok(result_error_value(
            "entity_merge",
            key,
            "candidate not found",
        )),
        Err(error) if entity_busy(&error) => Err(TransitionFailure::Busy),
        Err(error) => Err(TransitionFailure::Internal(error.to_string())),
    }
}

async fn entity_accept_batch(State(root): State<PathBuf>, body: Bytes) -> Response {
    let body = curation_body(&body);
    entity_batch(&root, body, true)
}
async fn entity_dismiss_batch(State(root): State<PathBuf>, body: Bytes) -> Response {
    let body = curation_body(&body);
    entity_batch(&root, body, false)
}

fn entity_batch(root: &Path, body: Value, accept: bool) -> Response {
    let Some(items) = body
        .get("items")
        .and_then(Value::as_array)
        .filter(|items| !items.is_empty())
    else {
        return missing("items");
    };
    let mut results = Vec::new();
    let mut ok = 0;
    for row in items {
        let facet = string(row, "facet");
        let source = string(row, "source_slug");
        let target = string(row, "target_slug");
        let result = if facet.is_empty() || source.is_empty() || target.is_empty() {
            json!({"facet":facet,"source_slug":source,"target_slug":target,"status":"error","error":"candidate is missing facet, source_slug, or target_slug"})
        } else {
            match entity_transition_value(root, &facet, &source, &target, accept) {
                Ok(value) => batch_item_result(&facet, &source, &target, value, accept),
                // Batch paths retain per-item reporting instead of exposing a
                // route-level timeout.
                Err(TransitionFailure::Busy) => {
                    json!({"facet":facet,"source_slug":source,"target_slug":target,"status":"error","error":ENTITY_BUSY})
                }
                Err(TransitionFailure::Internal(error)) => {
                    json!({"facet":facet,"source_slug":source,"target_slug":target,"status":"error","error":error})
                }
            }
        };
        if success_status(&result, accept) {
            ok += 1;
        }
        results.push(result);
    }
    Json(json!({"results":results, if accept {"accepted"} else {"dismissed"}:ok, "failed":items.len()-ok})).into_response()
}

async fn speaker_preview(State(root): State<PathBuf>, body: Bytes) -> Response {
    let body = curation_body(&body);
    let (_, source, target) = match speaker_fields(&body) {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let candidate = match find_speaker(&root, &source, &target) {
        Ok(Some(candidate)) => candidate,
        Ok(None) => {
            return result_error(
                "speaker_name_variant",
                speaker_key(&source, &target),
                "candidate not found",
            );
        }
        Err(error) => return internal(error),
    };
    if string(&candidate, "status") != "open" {
        return result_error(
            "speaker_name_variant",
            speaker_key(&source, &target),
            &format!(
                "cannot preview candidate with status {}",
                string(&candidate, "status")
            ),
        );
    }
    // Native previews currently compute only identity additions. The remaining
    // response fields retain the Flask shape until core entity exposes them.
    match preview_entity_merge(&root,&source,&target,EntityMergeOptions { keep_source_as_aka: true }) { Ok(preview) => Json(json!({"status":"preview","kind":"speaker_name_variant","key":speaker_key(&source,&target),"merge":{"would_identity":{"akas_added":preview.aliases_added,"emails_added_count":preview.emails_added}},"preview":{"akas_added":preview.aliases_added,"emails_added_count":preview.emails_added,"facet_moved_count":0,"facet_merged_count":0,"observations_appended":0,"labels_rewritten":0,"corrections_rewritten":0,"segment_errors":[],"voiceprints_added":0,"voiceprints_target_total":0}})).into_response(), Err(error) => result_error("speaker_name_variant",speaker_key(&source,&target),&error.to_string()) }
}

async fn speaker_accept(State(root): State<PathBuf>, body: Bytes) -> Response {
    let body = curation_body(&body);
    speaker_transition(&root, body, true)
}
async fn speaker_dismiss(State(root): State<PathBuf>, body: Bytes) -> Response {
    let body = curation_body(&body);
    speaker_transition(&root, body, false)
}
fn speaker_transition(root: &Path, body: Value, accept: bool) -> Response {
    let (_, source, target) = match speaker_fields(&body) {
        Ok(value) => value,
        Err(response) => return *response,
    };
    match speaker_transition_value(root, &source, &target, accept) {
        Ok(value) => result_response(value),
        Err(TransitionFailure::Busy) => busy(SPEAKER_BUSY),
        Err(TransitionFailure::Internal(error)) => internal(error),
    }
}

fn speaker_transition_value(
    root: &Path,
    source: &str,
    target: &str,
    accept: bool,
) -> Result<Value, TransitionFailure> {
    let key = speaker_key(source, target);
    let candidate = match find_speaker(root, source, target) {
        Ok(Some(value)) => value,
        Ok(None) => {
            return Ok(result_error_value(
                "speaker_name_variant",
                key,
                "candidate not found",
            ));
        }
        Err(error) => return Err(TransitionFailure::Internal(error)),
    };
    let status = string(&candidate, "status");
    if accept && status == "accepted" {
        let merge_id = merge_id(&candidate);
        return Ok(
            json!({"status":"already_accepted","kind":"speaker_name_variant","key":key,"candidate":candidate,"merge_id":merge_id,"undo":entity_merge_undo(merge_id)}),
        );
    }
    if !accept && status == "dismissed" {
        return Ok(
            json!({"status":"already_dismissed","kind":"speaker_name_variant","key":key,"candidate":candidate}),
        );
    }
    if status != "open" {
        return Ok(result_error_value(
            "speaker_name_variant",
            key,
            &format!(
                "cannot {} candidate with status {status}",
                if accept { "accept" } else { "dismiss" }
            ),
        ));
    }
    if accept {
        let report = match commit_entity_merge(
            root,
            source,
            target,
            EntityMergeOptions {
                keep_source_as_aka: true,
            },
            &unresolved_voiceprint_encoder(),
        ) {
            Ok(report) => report,
            Err(error) if merge_busy(&error) => return Err(TransitionFailure::Busy),
            Err(error) => {
                return Ok(result_error_value(
                    "speaker_name_variant",
                    key,
                    &error.to_string(),
                ));
            }
        };
        let merge_id = report.merge_id.clone();
        return match speaker_store::accept_candidate(root, source, target, Some(&merge_id)) {
            Ok(Some(candidate)) => Ok(
                json!({"status":"accepted","kind":"speaker_name_variant","key":key,"merge":merge_report_value(&report),"candidate":candidate,"merge_id":merge_id,"undo":entity_merge_undo(Some(&merge_id))}),
            ),
            Ok(None) => Ok(result_error_value(
                "speaker_name_variant",
                key,
                "candidate not found",
            )),
            Err(error) if speaker_busy(&error) => Err(TransitionFailure::Busy),
            Err(error) => Err(TransitionFailure::Internal(error.to_string())),
        };
    }
    match speaker_store::dismiss_candidate(root, source, target) {
        Ok(Some(candidate)) => Ok(
            json!({"status":"dismissed","kind":"speaker_name_variant","key":key,"candidate":candidate}),
        ),
        Ok(None) => Ok(result_error_value(
            "speaker_name_variant",
            key,
            "candidate not found",
        )),
        Err(error) if speaker_busy(&error) => Err(TransitionFailure::Busy),
        Err(error) => Err(TransitionFailure::Internal(error.to_string())),
    }
}

async fn pair_accept(State(root): State<PathBuf>, body: Bytes) -> Response {
    let body = curation_body(&body);
    pair_transition(&root, body, true)
}
async fn pair_dismiss(State(root): State<PathBuf>, body: Bytes) -> Response {
    let body = curation_body(&body);
    pair_transition(&root, body, false)
}
fn pair_transition(root: &Path, body: Value, accept: bool) -> Response {
    // This merges candidate anchors only; it never selects or writes an active speaker identity.
    let (_, a, b) = match pair_fields(&body) {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let key = pair_key(&a, &b);
    let candidate = match pair_store::load_candidates(root)
        .map_err(|error| error.to_string())
        .map(|rows| {
            rows.into_iter()
                .find(|row| row.get("key").and_then(Value::as_str) == Some(key.as_str()))
        }) {
        Ok(Some(value)) => value,
        Ok(None) => return result_error("speaker_candidate_pair", key, "candidate not found"),
        Err(error) => return internal(error),
    };
    let status = string(&candidate, "status");
    if accept && status == "accepted" {
        return Json(json!({"status":"already_accepted","kind":"speaker_candidate_pair","key":key,"candidate":candidate,"undo":{"available":false,"merge_id":null,"reason":"Speaker candidate-pair merges cannot be undone."}})).into_response();
    }
    if !accept && status == "dismissed" {
        return Json(json!({"status":"already_dismissed","kind":"speaker_candidate_pair","key":key,"candidate":candidate})).into_response();
    }
    if status != "open" {
        return result_error(
            "speaker_candidate_pair",
            key,
            &format!(
                "cannot {} candidate with status {status}",
                if accept { "accept" } else { "dismiss" }
            ),
        );
    }
    if accept {
        let merged = match CandidateTracker::new(root).merge_candidate_pair(&a, &b) {
            Ok(value) => value,
            Err(error) if error.to_string().contains("timed out") => return busy(SPEAKER_BUSY),
            Err(error) => return internal(error.to_string()),
        };
        if merged["status"] != "merged" {
            return result_error(
                "speaker_candidate_pair",
                key,
                merged["error"]
                    .as_str()
                    .unwrap_or("candidate pair is already merged"),
            );
        }
        match pair_store::accept_candidate(root,&a,&b){Ok(Some(row))=>Json(json!({"status":"accepted","kind":"speaker_candidate_pair","key":key,"merge":merged,"candidate":row,"undo":{"available":false,"merge_id":null,"reason":"Speaker candidate-pair merges cannot be undone."}})).into_response(),Ok(None)=>result_error("speaker_candidate_pair",key,"candidate not found"),Err(error)=>internal(error.to_string())}
    } else {
        match pair_store::dismiss_candidate(root,&a,&b){Ok(Some(row))=>Json(json!({"status":"dismissed","kind":"speaker_candidate_pair","key":key,"candidate":row})).into_response(),Ok(None)=>result_error("speaker_candidate_pair",key,"candidate not found"),Err(error)=>internal(error.to_string())}
    }
}

type FieldResult<T> = Result<T, Box<Response>>;

fn entity_fields(body: &Value) -> FieldResult<(String, String, String)> {
    required(body, "facet").and_then(|facet| {
        Ok((
            facet,
            required(body, "source_slug")?,
            required(body, "target_slug")?,
        ))
    })
}
fn speaker_fields(body: &Value) -> FieldResult<(String, String, String)> {
    let key = required(body, "key")?;
    let source = required(body, "source_id")?;
    let target = required(body, "target_id")?;
    if key != speaker_key(&source, &target) {
        return Err(Box::new(invalid("key does not match source_id/target_id")));
    }
    Ok((key, source, target))
}
fn pair_fields(body: &Value) -> FieldResult<(String, String, String)> {
    let key = required(body, "key")?;
    let a = required(body, "anchor_a")?;
    let b = required(body, "anchor_b")?;
    if key != pair_key(&a, &b) {
        return Err(Box::new(invalid("key does not match anchor_a/anchor_b")));
    }
    Ok((key, a, b))
}
fn required(body: &Value, field: &str) -> FieldResult<String> {
    body.get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| Box::new(missing(field)))
}
fn curation_body(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes)
        .ok()
        .filter(Value::is_object)
        .unwrap_or_else(|| Value::Object(Map::new()))
}
fn entity_candidate(
    root: &Path,
    facet: &str,
    source: &str,
    target: &str,
) -> Result<Option<Value>, String> {
    load_merge_candidates(root, Some(facet), None)
        .map_err(|error| error.to_string())
        .map(|rows| {
            rows.into_iter().find(|row| {
                string(row, "source_slug") == source && string(row, "target_slug") == target
            })
        })
}
fn find_speaker(root: &Path, source: &str, target: &str) -> Result<Option<Value>, String> {
    speaker_store::load_candidates(root)
        .map_err(|error| error.to_string())
        .map(|rows| {
            rows.into_iter().find(|row| {
                speaker_key(&string(row, "source_id"), &string(row, "target_id"))
                    == speaker_key(source, target)
                    && string(row, "source_id") == source
                    && string(row, "target_id") == target
            })
        })
}
fn entity_key(facet: &str, source: &str, target: &str) -> String {
    format!("{facet}|{source}|{target}")
}

enum TransitionFailure {
    Busy,
    Internal(String),
}

fn unresolved_voiceprint_encoder() -> EncoderIdentity {
    EncoderIdentity {
        id: "unresolved".to_owned(),
        sha256: "0000000000000000000000000000000000000000000000000000000000000000".to_owned(),
        width: 256,
    }
}

fn merge_report_value(report: &solstone_core_entity::EntityMergeReport) -> Value {
    json!({
        "merge_id": report.merge_id,
        "source_id": report.source_id,
        "target_id": report.target_id,
        "completed_phases": report.completed_phases,
        "aliases_added": report.aliases_added,
        "emails_added": report.emails_added,
    })
}

fn merge_id(candidate: &Value) -> Option<&str> {
    candidate
        .get("merge_id")
        .and_then(Value::as_str)
        .filter(|merge_id| !merge_id.is_empty())
}

fn entity_merge_undo(merge_id: Option<&str>) -> Value {
    json!({
        "available": merge_id.is_some(),
        "merge_id": merge_id,
        "reason": merge_id.is_none().then_some("No recorded merge id is available."),
    })
}

fn merge_busy(error: &EntityMergeError) -> bool {
    matches!(
        error,
        EntityMergeError::Write(EntityWriteError::TrustLock(
            solstone_core_entity::EntityTrustLockError::Lock(LockError::Timeout(_))
        )) | EntityMergeError::Write(EntityWriteError::AmbiguityLock(LockError::Timeout(_)))
    )
}

fn entity_busy(error: &EntityReviewCandidateError) -> bool {
    matches!(
        error,
        EntityReviewCandidateError::Lock(LockError::Timeout(_))
            | EntityReviewCandidateError::TrustLock(
                solstone_core_entity::EntityTrustLockError::Lock(LockError::Timeout(_))
            )
    )
}

fn speaker_busy(error: &speaker_store::SpeakerReviewCandidateError) -> bool {
    matches!(
        error,
        speaker_store::SpeakerReviewCandidateError::Lock(LockError::Timeout(_))
    )
}

fn result_response(result: Value) -> Response {
    if result.get("status").and_then(Value::as_str) == Some("error") {
        (StatusCode::BAD_REQUEST, Json(result)).into_response()
    } else {
        Json(result).into_response()
    }
}

fn result_error_value(kind: &str, key: String, error: &str) -> Value {
    json!({"status":"error","kind":kind,"key":key,"error":error})
}

fn success_status(result: &Value, accept: bool) -> bool {
    matches!(
        (accept, result.get("status").and_then(Value::as_str)),
        (true, Some("accepted" | "already_accepted"))
            | (false, Some("dismissed" | "already_dismissed"))
    )
}

fn batch_item_result(
    facet: &str,
    source: &str,
    target: &str,
    result: Value,
    accept: bool,
) -> Value {
    let status = result
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mut item = Map::from_iter([
        ("facet".to_owned(), Value::String(facet.to_owned())),
        ("source_slug".to_owned(), Value::String(source.to_owned())),
        ("target_slug".to_owned(), Value::String(target.to_owned())),
        ("status".to_owned(), Value::String(status.to_owned())),
        (
            "error".to_owned(),
            result.get("error").cloned().unwrap_or(Value::Null),
        ),
    ]);
    if success_status(&result, accept) && result.get("undo").is_some() {
        item.insert(
            "merge_id".to_owned(),
            result.get("merge_id").cloned().unwrap_or(Value::Null),
        );
        item.insert("undo".to_owned(), result["undo"].clone());
    }
    if result.get("operation_state").and_then(Value::as_str) == Some("repair_required") {
        for field in [
            "operation_state",
            "mutation_applied",
            "source_state",
            "target_state",
            "safe_remediation",
        ] {
            if let Some(value) = result.get(field) {
                item.insert(field.to_owned(), value.clone());
            }
        }
    }
    Value::Object(item)
}

fn missing(field: &str) -> Response {
    http::error(
        "missing_required_field",
        "I couldn't find a required field.",
        format!("Missing {field}"),
        StatusCode::BAD_REQUEST,
    )
}
fn invalid(detail: &str) -> Response {
    http::error(
        "invalid_request_value",
        "I couldn't use one of those values.",
        detail.to_owned(),
        StatusCode::BAD_REQUEST,
    )
}
fn busy(detail: &str) -> Response {
    http::error(
        "entity_busy",
        "The entity operation is busy.",
        detail.to_owned(),
        StatusCode::SERVICE_UNAVAILABLE,
    )
}
fn internal(detail: String) -> Response {
    http::error(
        "entity_operation_failed",
        "I couldn't complete that entity operation.",
        detail,
        StatusCode::INTERNAL_SERVER_ERROR,
    )
}
fn result_error(kind: &str, key: String, error: &str) -> Response {
    result_response(result_error_value(kind, key, error))
}

fn corrupt_config(root: &Path) -> Option<Response> {
    let path = root.join("config/journal.json");
    std::fs::read_to_string(&path)
        .ok()
        .filter(|contents| serde_json::from_str::<Value>(contents).is_err())
        .map(|_| {
            http::error(
                "corrupt_config",
                "I couldn't read your settings.",
                format!("I couldn't read your settings file at {}. Your settings were NOT changed. Repair the file or restore config/journal.json from a backup, then try again.", path.display()),
                StatusCode::INTERNAL_SERVER_ERROR,
            )
        })
}

#[cfg(test)]
mod tests {
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;

    use super::*;
    #[test]
    fn entity_evidence_uses_the_degraded_no_neighborhood_branch() {
        let row = json!({"facet":"work","source_slug":"a","target_slug":"b","evidence":{"detection_count":3}});
        let evidence = entity_item(row)["evidence"].clone();
        assert!(evidence.get("shared_neighbors").is_none());
        assert!(evidence.get("neighborhood_similarity").is_none());
    }

    #[tokio::test]
    async fn malformed_curation_body_reaches_missing_field_validation() {
        let root = crate::test_support::phase_root("established_empty");
        for body in [Body::empty(), Body::from("not json")] {
            let response = routes(root.path().to_path_buf())
                .oneshot(
                    Request::post("/app/curation/api/entity/accept")
                        .body(body)
                        .expect("request"),
                )
                .await
                .expect("response");
            let body: Value = serde_json::from_slice(
                &axum::body::to_bytes(response.into_body(), usize::MAX)
                    .await
                    .expect("body"),
            )
            .expect("json");
            assert_eq!(body["reason_code"], "missing_required_field");
            assert_eq!(body["detail"], "Missing facet");
        }
    }

    fn mergeable_entity_root() -> tempfile::TempDir {
        let root = crate::test_support::phase_root("established_empty");
        crate::test_support::write(
            &root.path().join("entities/review-candidates.jsonl"),
            "{\"facet\":\"work\",\"source_slug\":\"source\",\"target_slug\":\"target\",\"status\":\"open\",\"evidence\":{\"detection_count\":1}}\n",
        );
        solstone_core_entity::save_entity_identity(
            root.path(),
            "source",
            &json!({"id":"source","name":"Source","aka":["Source Alias"],"emails":[]}),
            None,
        )
        .expect("source identity");
        solstone_core_entity::save_entity_identity(
            root.path(),
            "target",
            &json!({"id":"target","name":"Target","aka":[],"emails":[]}),
            None,
        )
        .expect("target identity");
        root
    }

    async fn post_json(router: Router, path: &str, body: Value) -> Value {
        let response = router
            .oneshot(
                Request::post(path)
                    .body(Body::from(body.to_string()))
                    .expect("request"),
            )
            .await
            .expect("response");
        serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body"),
        )
        .expect("json response")
    }

    #[tokio::test]
    async fn entity_accept_commits_the_merge_before_accepting_the_candidate() {
        let root = mergeable_entity_root();
        let response = post_json(
            routes(root.path().to_path_buf()),
            "/app/curation/api/entity/accept",
            json!({"facet":"work","source_slug":"source","target_slug":"target"}),
        )
        .await;

        assert_eq!(response["status"], "accepted");
        assert!(response["merge_id"].is_string());
        assert_eq!(response["candidate"]["merge_id"], response["merge_id"]);
        assert_eq!(response["undo"]["available"], true);
        let target = solstone_core_entity::read_entity_identity(root.path(), "target")
            .expect("target identity")
            .expect("target exists");
        assert!(
            target.value()["aka"]
                .as_array()
                .expect("aliases")
                .contains(&Value::String("Source Alias".to_owned()))
        );
    }

    #[tokio::test]
    async fn entity_batch_preserves_accepted_and_already_accepted_merge_details() {
        let root = mergeable_entity_root();
        let body =
            json!({"items":[{"facet":"work","source_slug":"source","target_slug":"target"}]});
        let accepted = post_json(
            routes(root.path().to_path_buf()),
            "/app/curation/api/entity/accept-batch",
            body.clone(),
        )
        .await;
        assert_eq!(accepted["accepted"], 1);
        assert_eq!(accepted["failed"], 0);
        assert_eq!(accepted["results"][0]["status"], "accepted");
        assert!(accepted["results"][0]["merge_id"].is_string());
        assert_eq!(accepted["results"][0]["undo"]["available"], true);

        let already_accepted = post_json(
            routes(root.path().to_path_buf()),
            "/app/curation/api/entity/accept-batch",
            body,
        )
        .await;
        assert_eq!(already_accepted["accepted"], 1);
        assert_eq!(already_accepted["failed"], 0);
        assert_eq!(already_accepted["results"][0]["status"], "already_accepted");
        assert_eq!(
            already_accepted["results"][0]["merge_id"],
            accepted["results"][0]["merge_id"]
        );
        assert_eq!(already_accepted["results"][0]["undo"]["available"], true);
    }
}
