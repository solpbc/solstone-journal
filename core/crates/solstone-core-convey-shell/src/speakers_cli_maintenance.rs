// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! CLI maintenance routes composed from native speaker primitives.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::sync::Arc;

use axum::body::to_bytes;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use chrono::Utc;
use serde_json::{Map, Value, json};
use solstone_core_convey_http::envelope::error_envelope;
use solstone_core_entity::{is_admissible_person, load_all_journal_entities};
use solstone_core_journal_io::SegmentLayout;

use crate::JournalRoot;
use crate::speakers_segment_catalog::{
    DirectSupport, SegmentLookup, UNSUPPORTED_LAYOUT_DETAIL, UNSUPPORTED_LAYOUT_MESSAGE,
    UNSUPPORTED_LAYOUT_REASON, catalog_journal, decode_stream_layout_value, lookup_segment,
};

pub async fn bootstrap(Extension(root): Extension<Arc<JournalRoot>>, request: Request) -> Response {
    bootstrap_call(root, request, false).await
}
pub async fn seed_from_imports(
    Extension(root): Extension<Arc<JournalRoot>>,
    request: Request,
) -> Response {
    bootstrap_call(root, request, true).await
}

async fn bootstrap_call(root: Arc<JournalRoot>, request: Request, imports: bool) -> Response {
    let body = match json_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let request = solstone_core_speaker_resolve::bootstrap::BootstrapRequest {
        journal_root: root.0.clone(),
        encoder: encoder(),
        added_at: Utc::now().timestamp_millis(),
        dry_run: !body.get("commit").and_then(Value::as_bool).unwrap_or(false),
    };
    let result = if imports {
        solstone_core_speaker_resolve::bootstrap::seed_from_imports(&request).map(seed_value)
    } else {
        solstone_core_speaker_resolve::bootstrap::bootstrap_voiceprints(&request)
            .map(bootstrap_value)
    };
    match result {
        Ok(value) => Json(value).into_response(),
        Err(error) => err(
            "speaker_command_failed",
            "I couldn't finish that speaker command.",
            &error.to_string(),
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    }
}

pub async fn wipe(Extension(root): Extension<Arc<JournalRoot>>, request: Request) -> Response {
    let body = json_body(request).await.unwrap_or_else(|_| json!({}));
    let dry_run = !body.get("commit").and_then(Value::as_bool).unwrap_or(false);
    match solstone_core_speaker_resolve::artifact_wipe::wipe_speaker_artifacts(&root.0, dry_run) {
        Ok(report) => {
            Json(serde_json::to_value(report).expect("wipe report serializes")).into_response()
        }
        Err(solstone_core_speaker_resolve::artifact_wipe::ArtifactWipeError::Lock(
            solstone_core_journal_io::LockError::Timeout(timeout),
        )) => err(
            "speaker_voiceprint_busy",
            "I couldn't update that voice right now because it was busy. Try again in a moment.",
            &timeout.to_string(),
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        Err(error) => err(
            "speaker_command_failed",
            "I couldn't finish that speaker command.",
            &error.to_string(),
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    }
}

pub async fn resolve_names(
    Extension(root): Extension<Arc<JournalRoot>>,
    request: Request,
) -> Response {
    let body = match json_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let commit = body.get("commit").and_then(Value::as_bool).unwrap_or(false);
    let scan =
        match solstone_core_speaker_resolve::name_variant_scan::detect_name_variant_candidates(
            &root.0,
        ) {
            Ok(scan) => scan,
            Err(error) => {
                return err(
                    "speaker_command_failed",
                    "I couldn't finish that speaker command.",
                    &error.to_string(),
                    StatusCode::INTERNAL_SERVER_ERROR,
                );
            }
        };
    let stats = solstone_core_speaker_resolve::name_variant_scan::resolve_name_variant_candidates(
        scan,
        &root.0,
        commit,
        &encoder(),
    );
    Json(stats).into_response()
}

pub async fn attribute(Extension(root): Extension<Arc<JournalRoot>>, request: Request) -> Response {
    let body = match json_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let (day, stream, segment) = match fields(&body) {
        Ok(fields) => fields,
        Err(response) => return response,
    };
    let layout = decode_stream_layout_value(body.get("stream_layout"));
    let directory =
        match lookup_segment(&root.0, day, stream, segment, layout, DirectSupport::Refuse) {
            SegmentLookup::Present(path) => path,
            SegmentLookup::UnsupportedLayout => {
                return err(
                    UNSUPPORTED_LAYOUT_REASON,
                    UNSUPPORTED_LAYOUT_MESSAGE,
                    UNSUPPORTED_LAYOUT_DETAIL,
                    StatusCode::BAD_REQUEST,
                );
            }
            SegmentLookup::MalformedLayout => {
                return err(
                    "invalid_segment_or_stream",
                    "I couldn't use that segment or stream.",
                    "Invalid segment key or stream",
                    StatusCode::BAD_REQUEST,
                );
            }
            SegmentLookup::Absent => {
                return err(
                    "speaker_review_unavailable",
                    "I couldn't load that speaker review.",
                    "No transcript found",
                    StatusCode::NOT_FOUND,
                );
            }
            SegmentLookup::Failed(error) => {
                return err(
                    "speaker_command_failed",
                    "I couldn't finish that speaker command.",
                    &error.to_string(),
                    StatusCode::INTERNAL_SERVER_ERROR,
                );
            }
        };
    let now = Utc::now().timestamp_millis();
    let outcome = match solstone_core_speaker_resolve::resolve::resolve(
        &root.0, day, stream, segment, true, now,
    ) {
        Ok(outcome) => outcome,
        Err(error) => {
            return err(
                "speaker_command_failed",
                "I couldn't finish that speaker command.",
                &error.to_string(),
                StatusCode::INTERNAL_SERVER_ERROR,
            );
        }
    };
    let result = resolve_value(&outcome);
    if matches!(
        outcome,
        solstone_core_speaker_resolve::resolve::ResolveOutcome::IdentityInvalid
    ) {
        return err(
            "speaker_owner_identity_invalid",
            "I couldn't run that speaker command because your configured owner identity needs attention.",
            "configured owner identity is not admitted",
            StatusCode::BAD_REQUEST,
        );
    }
    if matches!(
        outcome,
        solstone_core_speaker_resolve::resolve::ResolveOutcome::NoOwnerCentroid
    ) {
        return err(
            "speaker_owner_centroid_required",
            "I couldn't run that speaker command until your owner voice is set up.",
            "owner centroid unavailable",
            StatusCode::CONFLICT,
        );
    }
    let commit = body.get("commit").and_then(Value::as_bool).unwrap_or(false);
    let save = body.get("save").and_then(Value::as_bool).unwrap_or(true);
    let mut written_path = Value::Null;
    if commit
        && save
        && let solstone_core_speaker_resolve::resolve::ResolveOutcome::Resolved(output) = &outcome
    {
        let metadata = metadata(output);
        if let Err(error) = solstone_core_speaker_id::labels::write_full_labels(
            &directory,
            labels(output),
            &metadata,
        ) {
            return write_error(error.to_string(), true);
        }
        written_path = json!(
            directory
                .join("talents/speaker_labels.json")
                .display()
                .to_string()
        );
    }
    let accumulated = if commit
        && body
            .get("accumulate")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        && let solstone_core_speaker_resolve::resolve::ResolveOutcome::Resolved(output) = &outcome
        && output.source.is_some()
    {
        match accumulate(&root.0, &directory, day, stream, segment, output, now) {
            Ok(value) => value,
            Err(error) => return accumulation_error(error),
        }
    } else {
        Value::Null
    };
    Json(json!({"result":result,"day":day,"stream_layout":layout_name(layout.expect("successful lookup decoded layout")),"stream":stream,"segment_key":segment,"written_path":written_path,"accumulated":accumulated}))
        .into_response()
}

pub async fn backfill(Extension(root): Extension<Arc<JournalRoot>>, request: Request) -> Response {
    let body = json_body(request).await.unwrap_or_else(|_| json!({}));
    let reattribute = body
        .get("reattribute")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let commit = body.get("commit").and_then(Value::as_bool).unwrap_or(false);
    let plan =
        match solstone_core_speaker_resolve::backfill::plan_backfill_segments(&root.0, reattribute)
        {
            Ok(plan) => plan,
            Err(error) => {
                return err(
                    "speaker_command_failed",
                    "I couldn't finish that speaker command.",
                    &error.to_string(),
                    StatusCode::INTERNAL_SERVER_ERROR,
                );
            }
        };
    let mut processed = 0usize;
    let mut errors = Vec::new();
    let mut speakers = BTreeSet::new();
    for segment in &plan.to_process {
        match solstone_core_speaker_resolve::backfill::resolve_backfill_segment(
            &root.0,
            segment,
            Utc::now().timestamp_millis(),
        ) {
            Ok(solstone_core_speaker_resolve::resolve::ResolveOutcome::Resolved(output)) => {
                for label in &output.labels {
                    if let Some(speaker) = &label.speaker {
                        speakers.insert(speaker.clone());
                    }
                }
                let metadata = metadata(&output);
                if commit {
                    if let Err(error) = solstone_core_speaker_id::labels::write_full_labels(
                        &segment.path,
                        labels(&output),
                        &metadata,
                    ) {
                        errors.push(format!("{}: {error}", segment.segment_key));
                        continue;
                    }
                    if output.source.is_some()
                        && let Err(error) = accumulate(
                            &root.0,
                            &segment.path,
                            &segment.day,
                            &segment.stream,
                            &segment.segment_key,
                            &output,
                            Utc::now().timestamp_millis(),
                        )
                    {
                        errors.push(format!("{}: {error}", segment.segment_key));
                        continue;
                    }
                }
                processed += 1;
            }
            Ok(_) => processed += 1,
            Err(error) => errors.push(format!("{}: {error}", segment.segment_key)),
        }
    }
    Json(json!({"total_segments":plan.total_segments,"total_eligible":plan.total_eligible,"already_labeled":plan.already_labeled,"processed":processed,"skipped_no_embed":plan.skipped_no_embed,"errors":errors,"speakers_seen":speakers})).into_response()
}

pub async fn backfill_last_seen(
    Extension(root): Extension<Arc<JournalRoot>>,
    request: Request,
) -> Response {
    let body = json_body(request).await.unwrap_or_else(|_| json!({}));
    let dry_run = !body.get("commit").and_then(Value::as_bool).unwrap_or(false);
    let (entity_max_ts, labels_read) = match last_seen_sources(&root.0) {
        Ok(sources) => sources,
        Err(error) => {
            return err(
                "speaker_command_failed",
                "I couldn't finish that speaker command.",
                &error,
                StatusCode::INTERNAL_SERVER_ERROR,
            );
        }
    };
    let encoder = encoder();
    let mut errors = Vec::new();
    let admitted_entity_ids = load_all_journal_entities(&root.0)
        .unwrap_or_default()
        .into_iter()
        .filter(is_admissible_person)
        .map(|entity| entity.id)
        .collect::<BTreeSet<_>>();
    let mut rows_written = 0usize;
    let mut rows_scanned = 0usize;
    let mut rows_pending = 0usize;
    let mut pending = BTreeMap::new();
    let mut skipped_ineligible = Vec::new();
    for (entity_id, max_ts) in &entity_max_ts {
        if !admitted_entity_ids.contains(entity_id) {
            skipped_ineligible.push(entity_id.clone());
            continue;
        }
        let Some(rows) = solstone_core_entity::load_entity_voiceprints_file(&root.0, entity_id)
        else {
            continue;
        };
        rows_scanned += rows.metadata.len();
        let count = rows
            .metadata
            .iter()
            .filter_map(|row| serde_json::from_str::<Value>(row).ok())
            .filter(|row| {
                row.get("last_seen_ts")
                    .and_then(Value::as_i64)
                    .is_none_or(|value| value < *max_ts)
            })
            .count();
        if count == 0 {
            continue;
        }
        pending.insert(
            entity_id.clone(),
            json!({"rows":count,"last_seen_ts":max_ts}),
        );
        rows_pending += count;
        if dry_run {
            continue;
        }
        match solstone_core_entity::rewrite_voiceprint_metadata(
            &root.0,
            entity_id,
            &encoder,
            |metadata| {
                metadata
                    .iter_mut()
                    .map(|row| {
                        if row
                            .get("last_seen_ts")
                            .and_then(Value::as_i64)
                            .is_none_or(|value| value < *max_ts)
                        {
                            row.as_object_mut().map(|object| {
                                object.insert("last_seen_ts".to_owned(), json!(max_ts))
                            });
                            1
                        } else {
                            0
                        }
                    })
                    .sum()
            },
        ) {
            Ok(count) => rows_written += count,
            Err(error) => errors.push(format!("{entity_id}: {error}")),
        }
    }
    let skipped_ineligible_count = skipped_ineligible.len();
    Json(json!({"dry_run":dry_run,"labels_read":labels_read,"entities_seen":entity_max_ts.len(),"entities_pending":pending.len(),"rows_scanned":rows_scanned,"rows_pending":rows_pending,"rows_written":rows_written,"pending":pending,"skipped_ineligible":skipped_ineligible,"skipped_ineligible_count":skipped_ineligible_count,"errors":errors})).into_response()
}

pub(crate) fn accumulate(
    root: &std::path::Path,
    segment_dir: &std::path::Path,
    day: &str,
    stream: &str,
    segment_key: &str,
    output: &solstone_core_speaker_resolve::resolve::ResolveOutput,
    now_ms: i64,
) -> Result<Value, solstone_core_speaker_resolve::voiceprint_accumulation::AccumulationError> {
    let Some(source) = output.source.as_ref() else {
        return Ok(json!({}));
    };
    let path = segment_dir.join(format!("{source}.npz"));
    let Some(embeddings) = solstone_core_speaker_id::embeddings::load_embeddings_file(&path)
        .map_err(|error| {
            solstone_core_speaker_resolve::voiceprint_accumulation::AccumulationError::Invalid(
                error.to_string(),
            )
        })?
    else {
        return Ok(json!({}));
    };
    let entity_ids = output
        .labels
        .iter()
        .filter_map(|label| label.speaker.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let request = solstone_core_speaker_resolve::voiceprint_accumulation::AccumulationRequest {
        journal_root: root.to_path_buf(),
        day: day.to_owned(),
        stream: stream.to_owned(),
        segment_key: segment_key.to_owned(),
        source: source.clone(),
        now_ms,
        encoder: encoder(),
        labels: output
            .labels
            .iter()
            .map(|label| {
                solstone_core_speaker_resolve::voiceprint_accumulation::AccumulationLabel {
                    sentence_id: label.sentence_id,
                    speaker: label.speaker.clone(),
                    confidence: label.confidence.clone(),
                    method: label.method.clone(),
                }
            })
            .collect(),
        embeddings: embeddings
            .statements
            .into_iter()
            .map(|(sentence_id, values)| {
                solstone_core_speaker_resolve::voiceprint_accumulation::AccumulationEmbedding {
                    sentence_id,
                    values,
                }
            })
            .collect(),
        entity_ids,
    };
    let reports = match solstone_core_speaker_resolve::voiceprint_accumulation::accumulate_voiceprints(&request)? {
        solstone_core_speaker_resolve::voiceprint_accumulation::AccumulationOutcome::IdentityInvalid { .. } => {
            return Ok(json!({"error":"speaker_owner_identity_invalid"}));
        }
        solstone_core_speaker_resolve::voiceprint_accumulation::AccumulationOutcome::NoOwnerCentroid { entity_reports }
        | solstone_core_speaker_resolve::voiceprint_accumulation::AccumulationOutcome::NothingEligible { entity_reports, .. }
        | solstone_core_speaker_resolve::voiceprint_accumulation::AccumulationOutcome::Completed { entity_reports, .. } => entity_reports,
    };
    Ok(Value::Object(
        reports
            .into_iter()
            .filter_map(|(id, report)| {
                (report.written_rows > 0).then(|| (id, json!(report.written_rows)))
            })
            .collect(),
    ))
}

fn accumulation_error(
    error: solstone_core_speaker_resolve::voiceprint_accumulation::AccumulationError,
) -> Response {
    err(
        "speaker_command_failed",
        "I couldn't finish that speaker command.",
        &error.to_string(),
        StatusCode::INTERNAL_SERVER_ERROR,
    )
}

fn last_seen_sources(root: &std::path::Path) -> Result<(BTreeMap<String, i64>, usize), String> {
    let mut entity_max_ts = BTreeMap::<String, i64>::new();
    let mut labels_read = 0;
    for segment in catalog_journal(root).map_err(|error| error.to_string())? {
        let labels = segment.path.join("talents/speaker_labels.json");
        let bytes = match fs::read(&labels) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("failed to read {}: {error}", labels.display())),
        };
        labels_read += 1;
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|error| format!("invalid labels {}: {error}", labels.display()))?;
        let ts = segment_timestamp(&segment.day, &segment.key).map_err(|error| {
            format!(
                "{}/{}/{}: {error}",
                segment.day, segment.stream, segment.name
            )
        })?;
        for speaker in value
            .get("labels")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|row| row.get("speaker").and_then(Value::as_str))
            .filter(|value| !value.is_empty())
        {
            entity_max_ts
                .entry(speaker.to_owned())
                .and_modify(|current| *current = (*current).max(ts))
                .or_insert(ts);
        }
    }
    Ok((entity_max_ts, labels_read))
}

fn segment_timestamp(day: &str, segment: &str) -> Result<i64, String> {
    let time = segment
        .split('_')
        .next()
        .ok_or_else(|| "missing segment time".to_owned())?;
    if day.len() != 8 || time.len() != 6 {
        return Err("invalid day or segment time".to_owned());
    }
    let datetime = chrono::NaiveDateTime::parse_from_str(&format!("{day}{time}"), "%Y%m%d%H%M%S")
        .map_err(|error| error.to_string())?;
    Ok(datetime.and_utc().timestamp_millis())
}

async fn json_body(request: Request) -> Result<Value, Response> {
    let bytes = to_bytes(request.into_body(), usize::MAX)
        .await
        .map_err(|_| {
            err(
                "missing_request_body",
                "I couldn't find any data in that request.",
                "unable to read request body",
                StatusCode::BAD_REQUEST,
            )
        })?;
    if bytes.is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_slice(&bytes).map_err(|_| {
        err(
            "invalid_json_request",
            "I couldn't read that JSON request.",
            "request body must be a JSON object",
            StatusCode::BAD_REQUEST,
        )
    })
}
#[allow(clippy::result_large_err)]
fn fields(body: &Value) -> Result<(&str, &str, &str), Response> {
    let day = body.get("day").and_then(Value::as_str).ok_or_else(|| {
        err(
            "missing_required_field",
            "I couldn't find a required field.",
            "day is required",
            StatusCode::BAD_REQUEST,
        )
    })?;
    let stream = body.get("stream").and_then(Value::as_str).ok_or_else(|| {
        err(
            "missing_required_field",
            "I couldn't find a required field.",
            "stream is required",
            StatusCode::BAD_REQUEST,
        )
    })?;
    let segment = body.get("segment").and_then(Value::as_str).ok_or_else(|| {
        err(
            "missing_required_field",
            "I couldn't find a required field.",
            "segment is required",
            StatusCode::BAD_REQUEST,
        )
    })?;
    Ok((day, stream, segment))
}
fn encoder() -> solstone_core_entity::EncoderIdentity {
    solstone_core_entity::EncoderIdentity {
        id: "unresolved".to_owned(),
        sha256: "0000000000000000000000000000000000000000000000000000000000000000".to_owned(),
        width: 256,
    }
}
pub(crate) fn labels(output: &solstone_core_speaker_resolve::resolve::ResolveOutput) -> Vec<Value> {
    output.labels.iter().map(|label| json!({"sentence_id":label.sentence_id,"speaker":label.speaker,"confidence":label.confidence,"method":label.method,"owner_margin_declined":label.owner_margin_declined,"acoustic_margin_declined":label.acoustic_margin_declined})).collect()
}
pub(crate) fn metadata(
    output: &solstone_core_speaker_resolve::resolve::ResolveOutput,
) -> Map<String, Value> {
    let mut value = Map::new();
    value.insert(
        "owner_centroid_last_refreshed_at".to_owned(),
        output
            .metadata
            .owner_centroid_last_refreshed_at
            .clone()
            .map_or(Value::Null, Value::String),
    );
    value.insert(
        "voiceprint_versions".to_owned(),
        json!(output.metadata.voiceprint_versions),
    );
    value.insert("candidate_evidence".to_owned(), Value::Array(Vec::new()));
    value
}

fn layout_name(layout: SegmentLayout) -> &'static str {
    match layout {
        SegmentLayout::Direct => "direct",
        SegmentLayout::Named => "named",
    }
}
fn resolve_value(outcome: &solstone_core_speaker_resolve::resolve::ResolveOutcome) -> Value {
    match outcome {
        solstone_core_speaker_resolve::resolve::ResolveOutcome::SegmentMissing => {
            json!({"status":"skipped","skip_reason":"segment_missing"})
        }
        solstone_core_speaker_resolve::resolve::ResolveOutcome::IdentityInvalid => {
            json!({"error":"speaker_owner_identity_invalid"})
        }
        solstone_core_speaker_resolve::resolve::ResolveOutcome::NoOwnerCentroid => {
            json!({"error":"owner centroid unavailable"})
        }
        solstone_core_speaker_resolve::resolve::ResolveOutcome::Empty { source } => {
            json!({"status":"skipped","source":source,"skip_reason":"no_embeddings"})
        }
        solstone_core_speaker_resolve::resolve::ResolveOutcome::Resolved(output) => {
            json!({"labels":labels(output),"unmatched":output.unmatched,"unmatched_texts":output.unmatched_texts,"source":output.source,"candidates":output.candidates,"metadata":metadata(output)})
        }
    }
}
fn bootstrap_value(outcome: solstone_core_speaker_resolve::bootstrap::BootstrapOutcome) -> Value {
    match outcome {
        solstone_core_speaker_resolve::bootstrap::BootstrapOutcome::IdentityInvalid => {
            json!({"error":"speaker_owner_identity_invalid"})
        }
        solstone_core_speaker_resolve::bootstrap::BootstrapOutcome::NoOwnerCentroid => {
            json!({"error":"owner_centroid_required"})
        }
        solstone_core_speaker_resolve::bootstrap::BootstrapOutcome::Completed(stats) => {
            json!({"segments_scanned":stats.segments_scanned,"single_speaker_segments":stats.single_speaker_segments,"speakers_found":stats.speakers_found,"entities_created":stats.entities_created,"embeddings_saved":stats.embeddings_saved,"embeddings_skipped_owner":stats.embeddings_skipped_owner,"embeddings_skipped_duplicate":stats.embeddings_skipped_duplicate,"speakers_unmatched":stats.speakers_unmatched,"errors":stats.errors})
        }
    }
}
fn seed_value(outcome: solstone_core_speaker_resolve::bootstrap::SeedFromImportsOutcome) -> Value {
    match outcome {
        solstone_core_speaker_resolve::bootstrap::SeedFromImportsOutcome::IdentityInvalid => {
            json!({"error":"speaker_owner_identity_invalid"})
        }
        solstone_core_speaker_resolve::bootstrap::SeedFromImportsOutcome::NoOwnerCentroid => {
            json!({"error":"owner_centroid_required"})
        }
        solstone_core_speaker_resolve::bootstrap::SeedFromImportsOutcome::Completed(stats) => {
            json!({"segments_scanned":stats.segments_scanned,"segments_with_speakers":stats.segments_with_speakers,"speakers_found":stats.speakers_found,"embeddings_saved":stats.embeddings_saved,"embeddings_skipped_owner":stats.embeddings_skipped_owner,"embeddings_skipped_duplicate":stats.embeddings_skipped_duplicate,"speakers_unmatched":stats.speakers_unmatched,"errors":stats.errors})
        }
    }
}
fn write_error(detail: String, _labels: bool) -> Response {
    err(
        "speaker_labels_busy",
        "I couldn't update those speaker attributions right now because they were busy. Try again in a moment.",
        &detail,
        StatusCode::SERVICE_UNAVAILABLE,
    )
}
fn err(code: &str, message: &str, detail: &str, status: StatusCode) -> Response {
    error_envelope(code, message, detail, status).into_response()
}
