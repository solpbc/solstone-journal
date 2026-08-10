// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use axum::body::to_bytes;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use chrono::Utc;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use solstone_core_convey_http::envelope::error_envelope;
use solstone_core_journal_io::{LockOptions, hold_lock};

use crate::JournalRoot;

const DISCOVERY_UNIT_NORM_TOLERANCE: f32 = 1.0e-3;
const MIN_CLUSTER_SIZE: usize = 5;
const OWNER_VOICE_UNAVAILABLE: &str = "speaker_discovery_owner_voice_unavailable";
const OWNER_VOICE_UNAVAILABLE_MESSAGE: &str =
    "i need your voice set up before looking for new voices.";
const INVALID_EMBEDDINGS: &str = "speaker_discovery_invalid_embeddings";
const INVALID_EMBEDDINGS_MESSAGE: &str =
    "i skipped some voice samples because they were not usable.";
static DISCOVERY_CACHE_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DiscoveryCandidate {
    pub(crate) day: String,
    pub(crate) stream: String,
    pub(crate) segment_key: String,
    pub(crate) source: String,
    pub(crate) sentence_id: i64,
    pub(crate) embedding: Vec<f32>,
}

pub(crate) enum DiscoveryCandidates {
    NoConfirmedOwner,
    Candidates {
        rows: Vec<DiscoveryCandidate>,
        dropped_invalid: usize,
    },
}

pub(crate) fn discovery_candidates(root: &std::path::Path) -> DiscoveryCandidates {
    let Some(principal) = crate::speakers_calendar::journal_principal_id(root) else {
        return DiscoveryCandidates::NoConfirmedOwner;
    };
    let Some(owner) =
        solstone_core_speaker_resolve::owner_centroid::load_owner_centroid(root, &principal)
            .ok()
            .flatten()
    else {
        return DiscoveryCandidates::NoConfirmedOwner;
    };
    let mut rows = Vec::new();
    let mut dropped = 0;
    let chronicle = root.join("chronicle");
    let Ok(days) = std::fs::read_dir(chronicle) else {
        return DiscoveryCandidates::Candidates {
            rows,
            dropped_invalid: dropped,
        };
    };
    for day in days.flatten() {
        let day_name = day.file_name().to_string_lossy().into_owned();
        let Ok(streams) = std::fs::read_dir(day.path()) else {
            continue;
        };
        for stream in streams.flatten() {
            let stream_name = stream.file_name().to_string_lossy().into_owned();
            let Ok(segments) = std::fs::read_dir(stream.path()) else {
                continue;
            };
            for segment in segments.flatten() {
                let key = segment.file_name().to_string_lossy().into_owned();
                let labels = std::fs::read(segment.path().join("talents/speaker_labels.json"))
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
                let attributed = labels
                    .and_then(|value| value.get("labels").and_then(Value::as_array).cloned())
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|row| row.get("speaker").is_some_and(|value| !value.is_null()))
                    .filter_map(|row| row.get("sentence_id").and_then(Value::as_i64))
                    .collect::<std::collections::BTreeSet<_>>();
                let Ok(entries) = std::fs::read_dir(segment.path()) else {
                    continue;
                };
                for entry in entries.flatten() {
                    let path = entry.path();
                    let Some(source) = path.file_stem().and_then(|name| name.to_str()) else {
                        continue;
                    };
                    if path.extension().and_then(|ext| ext.to_str()) != Some("npz") {
                        continue;
                    }
                    let Some(file) =
                        solstone_core_speaker_id::embeddings::load_embeddings_file(&path)
                            .ok()
                            .flatten()
                    else {
                        continue;
                    };
                    for (sentence_id, embedding) in file.statements {
                        if attributed.contains(&sentence_id) {
                            continue;
                        }
                        let norm = embedding
                            .iter()
                            .map(|value| value * value)
                            .sum::<f32>()
                            .sqrt();
                        if !norm.is_finite()
                            || embedding.iter().any(|value| !value.is_finite())
                            || (norm - 1.0).abs() > DISCOVERY_UNIT_NORM_TOLERANCE
                        {
                            dropped += 1;
                            continue;
                        }
                        let score: f32 = embedding
                            .iter()
                            .zip(&owner.centroid)
                            .map(|(left, right)| left * right)
                            .sum();
                        if score >= owner.threshold {
                            continue;
                        }
                        rows.push(DiscoveryCandidate {
                            day: day_name.clone(),
                            stream: stream_name.clone(),
                            segment_key: key.clone(),
                            source: source.to_owned(),
                            sentence_id,
                            embedding,
                        });
                    }
                }
            }
        }
    }
    DiscoveryCandidates::Candidates {
        rows,
        dropped_invalid: dropped,
    }
}

pub(crate) fn retain_discovery_clusters(
    rows: &[DiscoveryCandidate],
    labels: &[i64],
) -> std::collections::BTreeMap<String, Vec<Value>> {
    let mut output = std::collections::BTreeMap::new();
    for label in labels
        .iter()
        .copied()
        .filter(|label| *label != -1)
        .collect::<std::collections::BTreeSet<_>>()
    {
        let selected = rows
            .iter()
            .zip(labels)
            .filter(|(_, current)| **current == label)
            .map(|(row, _)| row)
            .collect::<Vec<_>>();
        if selected
            .iter()
            .map(|row| (&row.day, &row.stream, &row.segment_key))
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            < 3
        {
            continue;
        }
        let mut mean = vec![0.0; selected[0].embedding.len()];
        for row in &selected {
            for (value, input) in mean.iter_mut().zip(&row.embedding) {
                *value += *input
            }
        }
        let norm = mean.iter().map(|value| value * value).sum::<f32>().sqrt();
        if norm == 0.0 {
            continue;
        }
        for value in &mut mean {
            *value /= norm
        }
        let mut selected = selected;
        selected.sort_by(|left, right| {
            let score = |row: &DiscoveryCandidate| {
                row.embedding
                    .iter()
                    .zip(&mean)
                    .map(|(a, b)| a * b)
                    .sum::<f32>()
            };
            score(right).total_cmp(&score(left))
        });
        output.insert(label.to_string(),selected.into_iter().map(|row|json!({"day":row.day,"stream":row.stream,"segment_key":row.segment_key,"source":row.source,"sentence_id":row.sentence_id})).collect());
    }
    output
}

pub async fn identify(Extension(root): Extension<Arc<JournalRoot>>, request: Request) -> Response {
    let body = match body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let Some(cluster_id) = body.get("cluster_id").and_then(Value::as_i64) else {
        return bad("missing_required_field", "cluster_id is required");
    };
    let name = body
        .get("name")
        .map(|value| value.to_string().trim_matches('"').trim().to_owned())
        .filter(|value| !value.is_empty());
    let entity_id = body
        .get("entity_id")
        .map(|value| value.to_string().trim_matches('"').trim().to_owned())
        .filter(|value| !value.is_empty());
    if name.is_none() && entity_id.is_none() {
        return bad("missing_required_field", "entity_id or name is required");
    }
    let request = solstone_core_speaker_resolve::identify_cluster::IdentifyClusterRequest {
        journal_root: root.0.clone(),
        cluster_id,
        name,
        entity_id,
        resolve_only: body
            .get("resolve_only")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        create_new: body
            .get("create_new")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        entity_type: body
            .get("entity_type")
            .and_then(Value::as_str)
            .unwrap_or("Person")
            .to_owned(),
        request_id: body
            .get("request_id")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("native-web")
            .to_owned(),
        reviewed_near_match_entity_ids: body
            .get("reviewed_near_match_entity_ids")
            .and_then(Value::as_array)
            .map(|rows| {
                rows.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
        caller: "apps.speakers.discovery.identify".to_owned(),
        actor: None,
    };
    match solstone_core_speaker_resolve::identify_cluster::identify_cluster(&request, &encoder()) {
        Ok(value) => map(value),
        Err(error) => command(error.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

pub async fn undo(Extension(root): Extension<Arc<JournalRoot>>, request: Request) -> Response {
    let body = match body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let Some(operation_id) = body
        .get("operation_id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
    else {
        return bad("missing_required_field", "operation_id is required");
    };
    match solstone_core_speaker_resolve::identify_undo::undo_identify_operation(
        &root.0,
        operation_id.trim(),
        &encoder(),
    ) {
        Ok(value) => map(value),
        Err(error) => command(error.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

pub async fn dismiss(Extension(root): Extension<Arc<JournalRoot>>, request: Request) -> Response {
    let body = match body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let Some(cluster_id) = body.get("cluster_id").and_then(Value::as_i64) else {
        return bad(
            "missing_required_field",
            "cluster_id and disposition are required",
        );
    };
    let Some(disposition) = body
        .get("disposition")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    else {
        return bad(
            "missing_required_field",
            "cluster_id and disposition are required",
        );
    };
    if !matches!(disposition, "not_a_person" | "quiet") {
        return bad(
            "invalid_request_value",
            &format!("unknown cluster dismissal disposition: {disposition}"),
        );
    }
    let members = fs::read(root.0.join("awareness/discovery_clusters.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|cache| {
            cache
                .get("clusters")?
                .get(cluster_id.to_string())?
                .as_array()
                .cloned()
        });
    let Some(members) = members.filter(|members| !members.is_empty()) else {
        return error(
            "speaker_review_unavailable",
            "I couldn't load that speaker review.",
            &format!("Cluster {cluster_id} was not found. Run a discovery scan first."),
            StatusCode::NOT_FOUND,
        );
    };
    let members = match canonical_members(&members) {
        Ok(members) => members,
        Err(detail) => return command(detail, StatusCode::INTERNAL_SERVER_ERROR),
    };
    let ts = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let canonical = json!({"disposition":disposition,"members":members,"ts":ts});
    let mut hasher = Sha256::new();
    hasher.update(serde_json::to_string(&canonical).expect("json"));
    let id = format!("cdev_{:x}", hasher.finalize());
    let id = id[..29].to_owned();
    let event = json!({"schema_version":1,"event_kind":"dismiss","dismiss_event_id":id,"disposition":disposition,"members":members,"member_count":members.len(),"ts":ts});
    let path = root.0.join("speakers/cluster-dismissals.jsonl");
    let _lock = match hold_lock(&path, LockOptions::default()) {
        Ok(lock) => lock,
        Err(error) => return command(error.to_string(), StatusCode::SERVICE_UNAVAILABLE),
    };
    if let Some(parent) = path.parent()
        && let Err(error) = fs::create_dir_all(parent)
    {
        return command(error.to_string(), StatusCode::SERVICE_UNAVAILABLE);
    };
    if let Err(error) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut file| writeln!(file, "{}", event))
    {
        return command(error.to_string(), StatusCode::SERVICE_UNAVAILABLE);
    };
    Json(json!({"status":"dismissed","dismiss_event_id":id,"disposition":disposition,"member_count":members.len()})).into_response()
}

/// Run a native discovery scan and publish its derived cache on successful clustering.
pub async fn scan(Extension(root): Extension<Arc<JournalRoot>>) -> Response {
    let (rows, issues) = match discovery_candidates(&root.0) {
        DiscoveryCandidates::NoConfirmedOwner => {
            return Json(json!({
                "status": "degraded",
                "clusters": [],
                "issues": [issue(OWNER_VOICE_UNAVAILABLE, OWNER_VOICE_UNAVAILABLE_MESSAGE, 0)],
            }))
            .into_response();
        }
        DiscoveryCandidates::Candidates {
            rows,
            dropped_invalid,
        } => {
            let issues = (dropped_invalid != 0)
                .then(|| {
                    issue(
                        INVALID_EMBEDDINGS,
                        INVALID_EMBEDDINGS_MESSAGE,
                        dropped_invalid,
                    )
                })
                .into_iter()
                .collect();
            (rows, issues)
        }
    };

    if rows.len() < MIN_CLUSTER_SIZE {
        if let Err(error) = clear_discovery_cache(&root.0) {
            return command(error, StatusCode::INTERNAL_SERVER_ERROR);
        }
        return Json(scan_result(Vec::new(), issues)).into_response();
    }

    let embeddings = rows
        .iter()
        .map(|row| row.embedding.clone())
        .collect::<Vec<_>>();
    let labels = match crate::speakers_analyze_client::discovery_cluster(embeddings).await {
        Ok(labels) => labels,
        Err(error) => {
            let status = if error.stage == "invoke" {
                StatusCode::SERVICE_UNAVAILABLE
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            let retryable = error.stage == "invoke";
            return (
                status,
                Json(json!({
                    "error": "i couldn't look for new voices right now.",
                    "reason_code": "speaker_discovery_failed",
                    "detail": "",
                    "retryable": retryable,
                })),
            )
                .into_response();
        }
    };
    let clusters = retain_discovery_clusters(&rows, &labels);
    if clusters.is_empty() {
        if let Err(error) = clear_discovery_cache(&root.0) {
            return command(error, StatusCode::INTERNAL_SERVER_ERROR);
        }
        return Json(scan_result(Vec::new(), issues)).into_response();
    }
    if let Err(error) = write_discovery_cache(&root.0, &clusters) {
        return command(error, StatusCode::INTERNAL_SERVER_ERROR);
    }
    Json(scan_result(serialize_scan_clusters(&clusters), issues)).into_response()
}

fn issue(reason_code: &str, message: &str, count: usize) -> Value {
    json!({"reason_code":reason_code,"message":message,"count":count})
}

fn scan_result(clusters: Vec<Value>, issues: Vec<Value>) -> Value {
    json!({
        "status": if issues.is_empty() { "ok" } else { "degraded" },
        "clusters": clusters,
        "issues": issues,
    })
}

fn clear_discovery_cache(root: &Path) -> Result<(), String> {
    for name in [
        "discovery_clusters.json",
        "discovery_clusters.resolved.json",
    ] {
        let path = root.join("awareness").join(name);
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(())
}

fn write_discovery_cache(
    root: &Path,
    clusters: &std::collections::BTreeMap<String, Vec<Value>>,
) -> Result<(), String> {
    let awareness = root.join("awareness");
    fs::create_dir_all(&awareness).map_err(|error| error.to_string())?;
    let cache = awareness.join("discovery_clusters.json");
    let temp = awareness.join(format!(
        "discovery_clusters.{}.{}.tmp",
        std::process::id(),
        DISCOVERY_CACHE_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed),
    ));
    let payload = json!({
        "version": Utc::now().to_rfc3339(),
        "clusters": clusters,
    });
    let bytes = serde_json::to_vec_pretty(&payload).map_err(|error| error.to_string())?;
    let result = fs::write(&temp, bytes).and_then(|()| fs::rename(&temp, &cache));
    if let Err(error) = result {
        let _ = fs::remove_file(&temp);
        return Err(error.to_string());
    }
    Ok(())
}

fn serialize_scan_clusters(
    clusters: &std::collections::BTreeMap<String, Vec<Value>>,
) -> Vec<Value> {
    let mut output = clusters
        .iter()
        .filter_map(|(raw_id, members)| {
            let cluster_id = raw_id.parse::<i64>().ok()?;
            let segments = members
                .iter()
                .filter_map(|member| {
                    Some((
                        member.get("day")?.as_str()?,
                        member.get("stream")?.as_str()?,
                        member.get("segment_key")?.as_str()?,
                    ))
                })
                .collect::<BTreeSet<_>>();
            Some(json!({
                "cluster_id": cluster_id,
                "size": members.len(),
                "segment_count": segments.len(),
                // The cache provenance is complete; audio and transcript enrichment remains
                // the read-cache route's responsibility for this initial scan wiring.
                "samples": members.iter().take(3).cloned().collect::<Vec<_>>(),
            }))
        })
        .collect::<Vec<_>>();
    output.sort_by(|left, right| {
        right["size"]
            .as_u64()
            .cmp(&left["size"].as_u64())
            .then_with(|| {
                left["cluster_id"]
                    .as_i64()
                    .cmp(&right["cluster_id"].as_i64())
            })
    });
    output
}

fn canonical_members(members: &[Value]) -> Result<Vec<Value>, String> {
    let mut tuples = BTreeSet::new();
    for member in members {
        let object = member
            .as_object()
            .ok_or_else(|| "invalid cluster member provenance".to_owned())?;
        let field = |name: &str| {
            object
                .get(name)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| "invalid cluster member provenance".to_owned())
        };
        let sentence_id = object
            .get("sentence_id")
            .and_then(Value::as_i64)
            .ok_or_else(|| "invalid cluster member provenance".to_owned())?;
        tuples.insert((
            field("day")?,
            field("stream")?,
            field("segment_key")?,
            field("source")?,
            sentence_id,
        ));
    }
    Ok(tuples.into_iter().map(|(day, stream, segment_key, source, sentence_id)| json!({"day":day,"stream":stream,"segment_key":segment_key,"source":source,"sentence_id":sentence_id})).collect())
}
fn encoder() -> solstone_core_entity::EncoderIdentity {
    solstone_core_entity::EncoderIdentity {
        id: "unresolved".to_owned(),
        sha256: "0".repeat(64),
        width: 256,
    }
}
fn map(value: Value) -> Response {
    match value.get("status").and_then(Value::as_str).unwrap_or("") {
        "identified" | "resolved" | "ambiguous" | "no_match" | "principal_match" | "undone"
        | "already_undone" => Json(value).into_response(),
        "recoverable" | "in_progress" | "undoing" => error(
            "speaker_identify_recoverable",
            "I couldn't finish that speaker identify operation, but it can be retried.",
            &value.to_string(),
            StatusCode::CONFLICT,
        ),
        "repair_required" | "undo_repair_required" => error(
            "speaker_identify_repair_required",
            "I couldn't safely finish that speaker identify operation without repair.",
            &value.to_string(),
            StatusCode::CONFLICT,
        ),
        "conflict" | "operation_already_undone" => error(
            "speaker_identify_conflict",
            "I couldn't run that speaker identify operation because it conflicts with existing state.",
            &value.to_string(),
            StatusCode::CONFLICT,
        ),
        "not_found" => error(
            "speaker_identify_operation_not_found",
            "I couldn't find that speaker identify operation.",
            &value.to_string(),
            StatusCode::NOT_FOUND,
        ),
        "invalid_request" => bad(
            "invalid_request_value",
            value
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("invalid request"),
        ),
        _ => command(
            format!(
                "Unexpected speaker identify result status: {}",
                value
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("missing")
            ),
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    }
}
async fn body(request: Request) -> Result<Value, Response> {
    let bytes = to_bytes(request.into_body(), usize::MAX)
        .await
        .map_err(|_| bad("missing_request_body", "No data provided"))?;
    serde_json::from_slice(&bytes)
        .map_err(|_| bad("invalid_json_request", "request body must be a JSON object"))
}
fn bad(code: &str, detail: &str) -> Response {
    error(
        code,
        "I couldn't use that request.",
        detail,
        StatusCode::BAD_REQUEST,
    )
}
fn command(detail: String, status: StatusCode) -> Response {
    error(
        "speaker_command_failed",
        "I couldn't finish that speaker command.",
        &detail,
        status,
    )
}
fn error(code: &str, message: &str, detail: &str, status: StatusCode) -> Response {
    error_envelope(code, message, detail, status).into_response()
}

#[cfg(test)]
mod tests {
    use super::{DiscoveryCandidate, canonical_members, map, retain_discovery_clusters};
    use axum::response::IntoResponse;
    use serde_json::json;

    #[test]
    fn canonical_members_are_tuple_sorted_and_deduplicated() {
        let members = canonical_members(&[
            json!({"source":"audio","sentence_id":10,"stream":"b","day":"20260102","segment_key":"2_1","ignored":true}),
            json!({"day":"20260101","stream":"a","segment_key":"1_1","source":"audio","sentence_id":2}),
            json!({"day":"20260102","stream":"b","segment_key":"2_1","source":"audio","sentence_id":10}),
        ])
        .expect("canonical members");
        assert_eq!(members.len(), 2);
        assert_eq!(members[0]["day"], "20260101");
        assert_eq!(members[1]["sentence_id"], 10);
    }

    #[test]
    fn identify_status_classes_match_the_http_contract() {
        for status in ["recoverable", "repair_required", "conflict"] {
            assert_eq!(map(json!({"status":status})).into_response().status(), 409);
        }
        assert_eq!(
            map(json!({"status":"not_found"})).into_response().status(),
            404
        );
        assert_eq!(
            map(json!({"status":"invalid_request"}))
                .into_response()
                .status(),
            400
        );
        assert_eq!(
            map(json!({"status":"unexpected"})).into_response().status(),
            500
        );
    }

    fn candidate(
        day: &str,
        stream: &str,
        key: &str,
        embedding: Vec<f32>,
        sentence_id: i64,
    ) -> DiscoveryCandidate {
        DiscoveryCandidate {
            day: day.to_owned(),
            stream: stream.to_owned(),
            segment_key: key.to_owned(),
            source: "audio".to_owned(),
            sentence_id,
            embedding,
        }
    }

    #[test]
    fn retention_requires_three_segments_and_orders_by_similarity() {
        let two = vec![
            candidate("20260101", "a", "1_1", vec![1.0, 0.0], 1),
            candidate("20260101", "a", "2_1", vec![1.0, 0.0], 2),
        ];
        assert!(retain_discovery_clusters(&two, &[1, 1]).is_empty());
        let rows = vec![
            candidate("20260101", "a", "1_1", vec![1.0, 0.0], 1),
            candidate("20260101", "a", "2_1", vec![0.8, 0.6], 2),
            candidate("20260101", "a", "3_1", vec![1.0, 0.0], 3),
        ];
        let retained = retain_discovery_clusters(&rows, &[1, 1, 1]);
        assert_eq!(retained["1"].len(), 3);
        assert_eq!(retained["1"][0]["sentence_id"], 1);
    }

    #[test]
    fn retention_rejects_a_zero_mean_cluster() {
        let rows = vec![
            candidate("20260101", "a", "1_1", vec![1.0, 0.0], 1),
            candidate("20260101", "a", "2_1", vec![-1.0, 0.0], 2),
            candidate("20260101", "a", "3_1", vec![0.0, 0.0], 3),
        ];
        assert!(retain_discovery_clusters(&rows, &[2, 2, 2]).is_empty());
    }
}
