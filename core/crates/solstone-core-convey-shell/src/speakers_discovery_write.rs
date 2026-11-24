// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::window_catalog;
use solstone_core_convey_http::coalesce::ResponseLocale;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::sync::Arc;

use axum::body::to_bytes;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use jiff::Timestamp;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use solstone_core_convey_http::envelope::error_envelope;
use solstone_core_journal_io::{LockOptions, hold_lock};

use crate::speakers_attribution::action;
use crate::speakers_dismissals::{
    DiscoveryMember, canonical_members, dismissal_suppressed, fold_dismissal_members, member_set,
};
use crate::{HostedLaunchContext, JournalRoot};
#[cfg(test)]
use solstone_core_speaker_resolve::discovery_scan::{
    DiscoveryCandidate, MAX_UNMATCHED_EMBEDDINGS, numpy_choice_indexes, retain_discovery_clusters,
};
use solstone_core_speaker_resolve::discovery_scan::{
    DiscoveryRefresh, DiscoveryRefreshError, refresh_discovery_cache,
};
use solstone_core_speaker_resolve::segment_catalog::{
    SegmentLookup, decode_stream_layout_value, lookup_segment,
};

const OWNER_VOICE_UNAVAILABLE: &str = "speaker_discovery_owner_voice_unavailable";
const OWNER_VOICE_UNAVAILABLE_ID: &str =
    "journal.window.speakers.discovery.owner.voice.unavailable";
const INVALID_EMBEDDINGS: &str = "speaker_discovery_invalid_embeddings";
const INVALID_EMBEDDINGS_ID: &str = "journal.window.speakers.discovery.invalid.embeddings";

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum IdentifyPreflightError {
    Failed(String),
}

/// Resolve every cached identify target before the speaker-resolve mutator runs.
///
/// The cache remains backward compatible: absent `stream_layout` means Named.
pub(crate) fn preflight_identify_cluster(
    root: &Path,
    cluster_id: i64,
) -> Result<(), IdentifyPreflightError> {
    let cache_path = root.join("awareness/discovery_clusters.json");
    let bytes = fs::read(&cache_path).map_err(|error| {
        IdentifyPreflightError::Failed(format!(
            "failed to read discovery cache {}: {error}",
            cache_path.display()
        ))
    })?;
    let cache = serde_json::from_slice::<Value>(&bytes).map_err(|error| {
        IdentifyPreflightError::Failed(format!(
            "failed to parse discovery cache {}: {error}",
            cache_path.display()
        ))
    })?;
    let members = cache
        .get("clusters")
        .and_then(|clusters| clusters.get(cluster_id.to_string()))
        .and_then(Value::as_array)
        .filter(|members| !members.is_empty())
        .ok_or_else(|| {
            IdentifyPreflightError::Failed(format!(
                "Cluster {cluster_id} was not found. Run a discovery scan first."
            ))
        })?;
    let members = canonical_members(members).map_err(IdentifyPreflightError::Failed)?;
    for member in &members {
        let layout = decode_stream_layout_value(member.get("stream_layout")).map_err(|error| {
            IdentifyPreflightError::Failed(format!("invalid discovery member layout: {error}"))
        })?;
        let day = member["day"].as_str().expect("canonical day");
        let stream = member["stream"].as_str().expect("canonical stream");
        let segment = member["segment_key"]
            .as_str()
            .expect("canonical segment key");
        match lookup_segment(root, day, stream, segment, Ok(layout)) {
            SegmentLookup::Present(_) => {}
            SegmentLookup::Absent => {
                return Err(IdentifyPreflightError::Failed(format!(
                    "discovery segment was not found: {day}/{stream}/{segment}"
                )));
            }
            SegmentLookup::MalformedLayout => {
                return Err(IdentifyPreflightError::Failed(format!(
                    "invalid discovery segment identity: {day}/{stream}/{segment}"
                )));
            }
            SegmentLookup::Failed(error) => {
                return Err(IdentifyPreflightError::Failed(error.to_string()));
            }
        }
    }
    Ok(())
}

pub async fn identify(
    Extension(root): Extension<Arc<JournalRoot>>,
    locale: Option<axum::Extension<ResponseLocale>>,
    request: Request,
) -> Response {
    let locale = window_catalog::response_locale(locale);
    let body = match body(locale, request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let Some(cluster_id) = cluster_id_field(&body) else {
        return bad(locale, "missing_required_field", "cluster_id is required");
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
        return bad(
            locale,
            "missing_required_field",
            "entity_id or name is required",
        );
    }
    if let Err(error) = preflight_identify_cluster(&root.0, cluster_id) {
        return preflight_error(locale, error);
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
        caller: "convey_shell.speakers_discovery_write.identify".to_owned(),
        actor: None,
    };
    match solstone_core_speaker_resolve::identify_cluster::identify_cluster(&request, &encoder()) {
        Ok(value) => {
            if value.get("status").and_then(Value::as_str) == Some("identified")
                && let Err(error) = action(
                    &root.0,
                    "speaker_identified",
                    json!({
                        "entity_id": value.get("entity_id"),
                        "entity_name": value.get("entity_name"),
                        "cluster_id": cluster_id,
                        "voiceprints_saved": value.get("voiceprints_saved"),
                        "segments_updated": value.get("segments_updated"),
                    }),
                )
            {
                return command(locale, error, StatusCode::INTERNAL_SERVER_ERROR);
            }
            map(locale, value)
        }
        Err(error) => identify_error(locale, error.to_string()),
    }
}

/// Name one speaker-pool voice as an existing person everywhere it is still unnamed.
///
/// One-way by design: the name is written into each sentence's labels, with
/// corrections and voiceprints, and the pool voice is confirmed.
pub async fn identify_voice(
    Extension(root): Extension<Arc<JournalRoot>>,
    locale: Option<axum::Extension<ResponseLocale>>,
    request: Request,
) -> Response {
    let locale = window_catalog::response_locale(locale);
    let body = match body(locale, request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let Some(voice_id) = body
        .get("voice_id")
        .and_then(Value::as_i64)
        .filter(|id| *id > 0)
    else {
        return bad(locale, "missing_required_field", "voice_id is required");
    };
    let Some(entity_id) = body
        .get("entity_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return bad(locale, "missing_required_field", "entity_id is required");
    };
    // The operation id derives from request_id, so a default would join unrelated taps.
    let Some(request_id) = body
        .get("request_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return bad(locale, "missing_required_field", "request_id is required");
    };
    let text = |field: &str| {
        body.get(field)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    let (Some(day), Some(stream), Some(segment_key), Some(source), Some(sentence_id)) = (
        text("day"),
        text("stream"),
        text("segment_key"),
        text("source"),
        body.get("sentence_id").and_then(Value::as_i64),
    ) else {
        return bad(
            locale,
            "missing_required_field",
            "day, stream, segment_key, source and sentence_id of the chosen sentence are required",
        );
    };
    let anchor = solstone_core_speaker_resolve::voice_members::VoiceAnchor {
        day,
        stream,
        segment_key,
        source,
        sentence_id,
    };
    let request = solstone_core_speaker_resolve::identify_cluster::IdentifyClusterRequest {
        journal_root: root.0.clone(),
        cluster_id: 0,
        name: None,
        entity_id: Some(entity_id.to_owned()),
        resolve_only: false,
        create_new: false,
        entity_type: "Person".to_owned(),
        request_id: request_id.to_owned(),
        reviewed_near_match_entity_ids: Vec::new(),
        caller: "convey_shell.speakers_discovery_write.identify_voice".to_owned(),
        actor: None,
    };
    let value = match solstone_core_speaker_resolve::identify_cluster::identify_voice(
        &request,
        voice_id,
        Some(&anchor),
        &encoder(),
    ) {
        Ok(value) => value,
        Err(error) => return identify_error(locale, error.to_string()),
    };
    match value.get("status").and_then(Value::as_str).unwrap_or("") {
        "identified" => {
            // The names are written; a failed action-log append must not hide that.
            if let Err(error) = action(
                &root.0,
                "speaker_identified",
                json!({
                    "entity_id": value.get("entity_id"),
                    "entity_name": value.get("entity_name"),
                    "voice_id": voice_id,
                    "voiceprints_saved": value.get("voiceprints_saved"),
                    "segments_updated": value.get("segments_updated"),
                }),
            ) {
                log::warn!("speaker voice named without an action-log entry: {error}");
            }
            Json(value).into_response()
        }
        "nothing_to_name" => Json(value).into_response(),
        "voice_not_found" => error(
            locale,
            "speaker_voice_not_found",
            "journal.window.speakers.discovery.voice.not.found",
            &value.to_string(),
            StatusCode::NOT_FOUND,
        ),
        "voice_changed" => error(
            locale,
            "speaker_voice_changed",
            "journal.window.speakers.discovery.voice.changed",
            &value.to_string(),
            StatusCode::CONFLICT,
        ),
        "owner_voice_unavailable" => error(
            locale,
            "speaker_voice_owner_unavailable",
            "journal.window.speakers.discovery.voice.owner.unavailable",
            &value.to_string(),
            StatusCode::CONFLICT,
        ),
        "principal_voice" | "principal_target" => error(
            locale,
            "speaker_voice_principal",
            "journal.window.speakers.discovery.voice.principal",
            &value.to_string(),
            StatusCode::BAD_REQUEST,
        ),
        _ => map(locale, value),
    }
}

pub async fn undo(
    Extension(root): Extension<Arc<JournalRoot>>,
    locale: Option<axum::Extension<ResponseLocale>>,
    request: Request,
) -> Response {
    let locale = window_catalog::response_locale(locale);
    let body = match body(locale, request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let Some(operation_id) = body
        .get("operation_id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
    else {
        return bad(locale, "missing_required_field", "operation_id is required");
    };
    match solstone_core_speaker_resolve::identify_undo::undo_identify_operation(
        &root.0,
        operation_id.trim(),
        &encoder(),
    ) {
        Ok(value) => map(locale, value),
        Err(error) => identify_error(locale, error.to_string()),
    }
}

pub async fn dismiss(
    Extension(root): Extension<Arc<JournalRoot>>,
    locale: Option<axum::Extension<ResponseLocale>>,
    request: Request,
) -> Response {
    let locale = window_catalog::response_locale(locale);
    let body = match body(locale, request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let Some(cluster_id) = cluster_id_field(&body) else {
        return bad(
            locale,
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
            locale,
            "missing_required_field",
            "cluster_id and disposition are required",
        );
    };
    if !matches!(disposition, "not_a_person" | "quiet") {
        return bad(
            locale,
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
            locale,
            "speaker_review_unavailable",
            "journal.window.speakers.error.that.speaker.review.couldn.t.be.loaded",
            &format!("Cluster {cluster_id} was not found. Run a discovery scan first."),
            StatusCode::NOT_FOUND,
        );
    };
    let members = match canonical_members(&members) {
        Ok(members) => members,
        Err(detail) => return command(locale, detail, StatusCode::INTERNAL_SERVER_ERROR),
    };
    let ts = solstone_core_time::utc_z(Timestamp::now(), solstone_core_time::Precision::Secs);
    let canonical = json!({"disposition":disposition,"members":members,"ts":ts});
    let mut hasher = Sha256::new();
    hasher.update(serde_json::to_string(&canonical).expect("json"));
    let id = format!("cdev_{:x}", hasher.finalize());
    let id = id[..29].to_owned();
    let event = json!({"schema_version":1,"event_kind":"dismiss","dismiss_event_id":id,"disposition":disposition,"members":members,"member_count":members.len(),"ts":ts});
    let path = root.0.join("speakers/cluster-dismissals.jsonl");
    let _lock = match hold_lock(&path, LockOptions::default()) {
        Ok(lock) => lock,
        Err(error) => return command(locale, error.to_string(), StatusCode::SERVICE_UNAVAILABLE),
    };
    if let Some(parent) = path.parent()
        && let Err(error) = fs::create_dir_all(parent)
    {
        return command(locale, error.to_string(), StatusCode::SERVICE_UNAVAILABLE);
    };
    // Synced, on a line of its own, under the lock held above: the fold reads
    // every row strictly, so a row may never tear or run onto a cut one.
    if let Err(error) = solstone_core_journal_io::append_jsonl(&path, &event) {
        return command(locale, error.to_string(), StatusCode::SERVICE_UNAVAILABLE);
    };
    Json(json!({"status":"dismissed","dismiss_event_id":id,"disposition":disposition,"member_count":members.len()})).into_response()
}

/// Run a native discovery scan and publish its derived cache on successful clustering.
pub async fn scan(
    Extension(root): Extension<Arc<JournalRoot>>,
    Extension(hosted_launch): Extension<HostedLaunchContext>,
    locale: Option<axum::Extension<ResponseLocale>>,
    #[cfg(windows)] generation: Option<Extension<crate::DiscoveryGenerationContext>>,
) -> Response {
    let locale = window_catalog::response_locale(locale);
    #[cfg(windows)]
    let Some(Extension(generation)) = generation else {
        return command(
            locale,
            "discovery generation was not admitted at Convey entry".to_owned(),
            StatusCode::SERVICE_UNAVAILABLE,
        );
    };
    let scan_root = root.clone();
    let result = tokio::task::spawn_blocking(move || {
        refresh_discovery_cache(
            &scan_root.0,
            hosted_launch.0,
            #[cfg(windows)]
            &generation.0,
        )
    })
    .await;
    let (clusters, dropped_invalid) = match result {
        Ok(Ok(DiscoveryRefresh::IdentityInvalid)) => {
            return error(
                locale,
                "speaker_owner_identity_invalid",
                "journal.window.speakers.discovery.owner.identity.invalid",
                "configured owner identity is not admitted",
                StatusCode::CONFLICT,
            );
        }
        Ok(Ok(DiscoveryRefresh::NoConfirmedOwner)) => {
            return Json(json!({
                "status":"degraded", "clusters":[],
                "issues":[issue(
                    OWNER_VOICE_UNAVAILABLE,
                    &window_catalog::text(locale, OWNER_VOICE_UNAVAILABLE_ID),
                    0,
                )],
            }))
            .into_response();
        }
        Ok(Ok(DiscoveryRefresh::Refreshed {
            clusters,
            dropped_invalid,
        })) => (clusters, dropped_invalid),
        Ok(Err(DiscoveryRefreshError::Input(error))) => {
            return command(locale, error, StatusCode::INTERNAL_SERVER_ERROR);
        }
        failure => {
            let retryable = !matches!(failure, Ok(Err(DiscoveryRefreshError::Helper(ref error))) if error.stage != "invoke");
            let status = if retryable {
                StatusCode::SERVICE_UNAVAILABLE
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            return (status, Json(json!({
                "error": window_catalog::text(locale, "journal.window.speakers.copy.discovery.error"),
                "reason_code":"speaker_discovery_failed",
                "detail":"", "retryable":retryable,
            }))).into_response();
        }
    };
    let issues = (dropped_invalid != 0)
        .then(|| {
            issue(
                INVALID_EMBEDDINGS,
                &window_catalog::text(locale, INVALID_EMBEDDINGS_ID),
                dropped_invalid,
            )
        })
        .into_iter()
        .collect();
    if clusters.is_empty() {
        return Json(scan_result(Vec::new(), issues)).into_response();
    }
    let visible_clusters = match serialize_scan_clusters(&root.0, &clusters) {
        Ok(clusters) => clusters,
        Err(error) => return command(locale, error, StatusCode::INTERNAL_SERVER_ERROR),
    };
    Json(scan_result(visible_clusters, issues)).into_response()
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

fn serialize_scan_clusters(
    root: &Path,
    clusters: &std::collections::BTreeMap<String, Vec<Value>>,
) -> Result<Vec<Value>, String> {
    let dismissals = folded_dismissal_members(root)?;
    let mut output = Vec::new();
    for (raw_id, members) in clusters {
        let cluster_id = raw_id
            .parse::<i64>()
            .map_err(|_| format!("invalid discovery cluster id: {raw_id}"))?;
        let member_set = member_set(members)?;
        if dismissal_suppressed(&member_set, &dismissals) {
            continue;
        }
        let segments = member_set
            .iter()
            .map(|member| (&member.0, &member.1, &member.2, &member.3))
            .collect::<BTreeSet<_>>();
        output.push(json!({
            "cluster_id": cluster_id,
            "size": members.len(),
            "segment_count": segments.len(),
            // The cache provenance is complete; audio and transcript enrichment remains
            // the read-cache route's responsibility for this initial scan wiring.
            "samples": members.iter().take(3).cloned().collect::<Vec<_>>(),
        }));
    }
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
    Ok(output)
}

fn folded_dismissal_members(root: &Path) -> Result<Vec<BTreeSet<DiscoveryMember>>, String> {
    let path = root.join("speakers/cluster-dismissals.jsonl");
    let contents = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(format!(
                "failed to read cluster dismissal store {path:?}: {error}"
            ));
        }
    };
    let mut events = Vec::new();
    for (line, raw) in contents.lines().enumerate() {
        if raw.trim().is_empty() {
            continue;
        }
        let event: Value = serde_json::from_str(raw).map_err(|error| {
            format!(
                "malformed cluster dismissal JSONL at {}:{}: {error}",
                path.display(),
                line + 1
            )
        })?;
        let members = event
            .get("members")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                format!(
                    "invalid cluster dismissal row at {}:{}",
                    path.display(),
                    line + 1
                )
            })?;
        events.push(member_set(members)?);
    }

    Ok(fold_dismissal_members(&events))
}

fn preflight_error(locale: &str, failure: IdentifyPreflightError) -> Response {
    match failure {
        IdentifyPreflightError::Failed(detail) => {
            command(locale, detail, StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}
fn encoder() -> solstone_core_entity::EncoderIdentity {
    solstone_core_entity::EncoderIdentity {
        id: "unresolved".to_owned(),
        sha256: "0".repeat(64),
        width: 256,
    }
}
fn map(locale: &str, value: Value) -> Response {
    match value.get("status").and_then(Value::as_str).unwrap_or("") {
        "identified" | "resolved" | "ambiguous" | "no_match" | "principal_match" | "undone"
        | "already_undone" => Json(value).into_response(),
        "recoverable" | "in_progress" | "undoing" => error(
            locale,
            "speaker_identify_recoverable",
            "journal.window.speakers.error.change.recoverable",
            &value.to_string(),
            StatusCode::CONFLICT,
        ),
        "repair_required" | "undo_repair_required" => error(
            locale,
            "speaker_identify_repair_required",
            "journal.window.speakers.error.change.repair.required",
            &value.to_string(),
            StatusCode::CONFLICT,
        ),
        "conflict" | "operation_already_undone" => error(
            locale,
            "speaker_identify_conflict",
            "journal.window.speakers.error.change.conflict",
            &value.to_string(),
            StatusCode::CONFLICT,
        ),
        "one_way" => error(
            locale,
            "speaker_identify_one_way",
            "journal.window.speakers.error.change.one.way",
            &value.to_string(),
            StatusCode::CONFLICT,
        ),
        "not_found" => error(
            locale,
            "speaker_identify_operation_not_found",
            "journal.window.speakers.error.change.not.found",
            &value.to_string(),
            StatusCode::NOT_FOUND,
        ),
        "invalid_request" => invalid_request(locale, &value),
        _ => command(
            locale,
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

/// Refuse an identify request, keeping the fields the "who is this" sheet
/// reads to recover, such as refreshing a stale set of reviewed near matches.
fn invalid_request(locale: &str, value: &Value) -> Response {
    let detail = value
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or("invalid request");
    let (status, Json(envelope)) = error_envelope(
        "invalid_request_value",
        window_catalog::text(
            locale,
            "journal.window.speakers.error.that.request.couldn.t.be.used",
        ),
        detail,
        StatusCode::BAD_REQUEST,
    );
    let mut body = serde_json::to_value(envelope).expect("error envelope serializes");
    for key in [
        "invalid_request_code",
        "invalid_reviewed_near_match_entity_ids",
    ] {
        if let Some(field) = value.get(key) {
            body[key] = field.clone();
        }
    }
    (status, Json(body)).into_response()
}

fn identify_error(locale: &str, detail: String) -> Response {
    let lower = detail.to_ascii_lowercase();
    if lower.contains("busy") || lower.contains("lock") {
        let (code, message_id) =
            if lower.contains("speaker_labels") || lower.contains("speaker_corrections") {
                (
                    "speaker_labels_busy",
                    "journal.window.speakers.discovery.labels.busy",
                )
            } else {
                (
                    "speaker_voiceprint_busy",
                    "journal.window.speakers.discovery.voiceprint.busy",
                )
            };
        return error(
            locale,
            code,
            message_id,
            &detail,
            StatusCode::SERVICE_UNAVAILABLE,
        );
    }
    command(locale, detail, StatusCode::INTERNAL_SERVER_ERROR)
}
/// Read a request's `cluster_id` as an integer or its decimal string.
///
/// The "who is this" sheet keeps the id as the string it keys its URLs by.
fn cluster_id_field(body: &Value) -> Option<i64> {
    match body.get("cluster_id")? {
        Value::Number(number) => number.as_i64(),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}
async fn body(locale: &str, request: Request) -> Result<Value, Response> {
    let bytes = to_bytes(request.into_body(), usize::MAX)
        .await
        .map_err(|_| bad(locale, "missing_request_body", "No data provided"))?;
    serde_json::from_slice(&bytes).map_err(|_| {
        bad(
            locale,
            "invalid_json_request",
            "request body must be a JSON object",
        )
    })
}
fn bad(locale: &str, code: &str, detail: &str) -> Response {
    error(
        locale,
        code,
        "journal.window.speakers.error.that.request.couldn.t.be.used",
        detail,
        StatusCode::BAD_REQUEST,
    )
}
fn command(locale: &str, detail: String, status: StatusCode) -> Response {
    error(
        locale,
        "speaker_command_failed",
        "journal.window.speakers.error.that.speaker.command.didn.t.finish",
        &detail,
        status,
    )
}
fn error(locale: &str, code: &str, message_id: &str, detail: &str, status: StatusCode) -> Response {
    error_envelope(
        code,
        window_catalog::text(locale, message_id),
        detail,
        status,
    )
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::{
        DiscoveryCandidate, MAX_UNMATCHED_EMBEDDINGS, canonical_members, cluster_id_field, map,
        numpy_choice_indexes, retain_discovery_clusters,
    };
    use axum::response::IntoResponse;
    use serde_json::json;
    use sha2::{Digest, Sha256};
    use solstone_core_journal_io::SegmentLayout;

    use crate::window_catalog;

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
    fn cluster_id_accepts_the_number_or_the_string_the_sheet_sends() {
        assert_eq!(cluster_id_field(&json!({"cluster_id":7})), Some(7));
        assert_eq!(cluster_id_field(&json!({"cluster_id":"7"})), Some(7));
        assert_eq!(cluster_id_field(&json!({"cluster_id":" 0 "})), Some(0));
        for body in [
            json!({}),
            json!({"cluster_id":""}),
            json!({"cluster_id":"seven"}),
            json!({"cluster_id":7.5}),
            json!({"cluster_id":null}),
        ] {
            assert_eq!(cluster_id_field(&body), None, "{body}");
        }
    }

    #[tokio::test]
    async fn invalid_identify_keeps_the_sheet_recovery_code() {
        let response = map(
            window_catalog::ENGLISH,
            json!({
                "status":"invalid_request",
                "error":"reviewed_near_match_entity_ids must match shown near matches",
                "invalid_request_code":"reviewed_near_match_set_mismatch",
            }),
        )
        .into_response();
        assert_eq!(response.status(), 400);
        let body: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body"),
        )
        .expect("json body");
        assert_eq!(body["reason_code"], "invalid_request_value");
        assert_eq!(
            body["invalid_request_code"],
            "reviewed_near_match_set_mismatch"
        );
    }

    #[test]
    fn identify_status_classes_match_the_http_contract() {
        for status in ["recoverable", "repair_required", "conflict"] {
            assert_eq!(
                map(window_catalog::ENGLISH, json!({"status":status}))
                    .into_response()
                    .status(),
                409
            );
        }
        assert_eq!(
            map(window_catalog::ENGLISH, json!({"status":"not_found"}))
                .into_response()
                .status(),
            404
        );
        assert_eq!(
            map(window_catalog::ENGLISH, json!({"status":"invalid_request"}))
                .into_response()
                .status(),
            400
        );
        assert_eq!(
            map(window_catalog::ENGLISH, json!({"status":"unexpected"}))
                .into_response()
                .status(),
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
            stream_layout: SegmentLayout::Named,
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

    #[test]
    fn candidate_cap_matches_numpy_seed_42_choice() {
        let selected = numpy_choice_indexes(MAX_UNMATCHED_EMBEDDINGS + 1, MAX_UNMATCHED_EMBEDDINGS);
        assert_eq!(selected.len(), MAX_UNMATCHED_EMBEDDINGS);
        assert!(!selected.contains(&1591));
        assert!(
            selected
                .iter()
                .enumerate()
                .all(|(index, value)| { *value == if index < 1591 { index } else { index + 1 } })
        );

        let selected = numpy_choice_indexes(500_000, MAX_UNMATCHED_EMBEDDINGS);
        let bytes = selected
            .iter()
            .flat_map(|value| (*value as i64).to_le_bytes())
            .collect::<Vec<_>>();
        assert_eq!(
            format!("{:x}", Sha256::digest(bytes)),
            "22f17a2448e9ce23e3de4589355d68867a5de47ecd7cd96e70b77aad828995b1"
        );
    }
}
