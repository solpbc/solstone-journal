// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use axum::body::to_bytes;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use chrono::Utc;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use solstone_core_convey_http::envelope::error_envelope;
use solstone_core_journal_io::{LockOptions, SegmentLayout, hold_lock};

use crate::speakers_attribution::action;
use crate::{HostedLaunchContext, JournalRoot};
#[cfg(test)]
use solstone_core_speaker_resolve::discovery_scan::{
    DiscoveryCandidate, MAX_UNMATCHED_EMBEDDINGS, numpy_choice_indexes, retain_discovery_clusters,
};
use solstone_core_speaker_resolve::discovery_scan::{
    DiscoveryRefresh, DiscoveryRefreshError, refresh_discovery_cache,
};
use solstone_core_speaker_resolve::segment_catalog::{
    DirectSupport, SegmentLookup, UNSUPPORTED_LAYOUT_DETAIL, UNSUPPORTED_LAYOUT_MESSAGE,
    UNSUPPORTED_LAYOUT_REASON, decode_stream_layout_value, lookup_segment,
};

const OWNER_VOICE_UNAVAILABLE: &str = "speaker_discovery_owner_voice_unavailable";
const OWNER_VOICE_UNAVAILABLE_MESSAGE: &str =
    "i need your voice set up before looking for new voices.";
const INVALID_EMBEDDINGS: &str = "speaker_discovery_invalid_embeddings";
const INVALID_EMBEDDINGS_MESSAGE: &str =
    "i skipped some voice samples because they were not usable.";

type DiscoveryMember = (String, String, String, String, String, i64);

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum IdentifyPreflightError {
    UnsupportedLayout,
    Failed(String),
}

/// Resolve every cached identify target before the speaker-resolve mutator runs.
///
/// The cache remains backward compatible: absent `stream_layout` means Named.
/// Direct is represented losslessly by discovery, but identify is still a
/// Named-only mutation until speaker-resolve accepts explicit locators.
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
        if layout == SegmentLayout::Direct {
            return Err(IdentifyPreflightError::UnsupportedLayout);
        }
        let day = member["day"].as_str().expect("canonical day");
        let stream = member["stream"].as_str().expect("canonical stream");
        let segment = member["segment_key"]
            .as_str()
            .expect("canonical segment key");
        match lookup_segment(
            root,
            day,
            stream,
            segment,
            Ok(layout),
            DirectSupport::Refuse,
        ) {
            SegmentLookup::Present(_) => {}
            SegmentLookup::UnsupportedLayout => {
                return Err(IdentifyPreflightError::UnsupportedLayout);
            }
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
    if let Err(error) = preflight_identify_cluster(&root.0, cluster_id) {
        return preflight_error(error);
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
                return command(error, StatusCode::INTERNAL_SERVER_ERROR);
            }
            map(value)
        }
        Err(error) => identify_error(error.to_string()),
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
        Err(error) => identify_error(error.to_string()),
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
            "that speaker review couldn't be loaded.",
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
pub async fn scan(
    Extension(root): Extension<Arc<JournalRoot>>,
    Extension(hosted_launch): Extension<HostedLaunchContext>,
) -> Response {
    let scan_root = root.clone();
    let result =
        tokio::task::spawn_blocking(move || refresh_discovery_cache(&scan_root.0, hosted_launch.0))
            .await;
    let (clusters, dropped_invalid) = match result {
        Ok(Ok(DiscoveryRefresh::IdentityInvalid)) => {
            return error(
                "speaker_owner_identity_invalid",
                "looking for new voices couldn't start because your configured owner identity needs attention.",
                "configured owner identity is not admitted",
                StatusCode::BAD_REQUEST,
            );
        }
        Ok(Ok(DiscoveryRefresh::NoConfirmedOwner)) => {
            return Json(json!({
                "status":"degraded", "clusters":[],
                "issues":[issue(OWNER_VOICE_UNAVAILABLE, OWNER_VOICE_UNAVAILABLE_MESSAGE, 0)],
            }))
            .into_response();
        }
        Ok(Ok(DiscoveryRefresh::Refreshed {
            clusters,
            dropped_invalid,
        })) => (clusters, dropped_invalid),
        Ok(Err(DiscoveryRefreshError::Input(error))) => {
            return command(error, StatusCode::INTERNAL_SERVER_ERROR);
        }
        failure => {
            let retryable = !matches!(failure, Ok(Err(DiscoveryRefreshError::Helper(ref error))) if error.stage != "invoke");
            let status = if retryable {
                StatusCode::SERVICE_UNAVAILABLE
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            return (status, Json(json!({
                "error":"i couldn't look for new voices right now.", "reason_code":"speaker_discovery_failed",
                "detail":"", "retryable":retryable,
            }))).into_response();
        }
    };
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
    if clusters.is_empty() {
        return Json(scan_result(Vec::new(), issues)).into_response();
    }
    let visible_clusters = match serialize_scan_clusters(&root.0, &clusters) {
        Ok(clusters) => clusters,
        Err(error) => return command(error, StatusCode::INTERNAL_SERVER_ERROR),
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

    let mut folded = Vec::new();
    let mut seen = vec![false; events.len()];
    for start in 0..events.len() {
        if seen[start] {
            continue;
        }
        let mut stack = vec![start];
        let mut component = BTreeSet::new();
        seen[start] = true;
        while let Some(current) = stack.pop() {
            component.extend(events[current].iter().cloned());
            for next in 0..events.len() {
                if !seen[next] && overlap_ratio_at_least_half(&events[current], &events[next]) {
                    seen[next] = true;
                    stack.push(next);
                }
            }
        }
        folded.push(component);
    }
    Ok(folded)
}

fn dismissal_suppressed(
    candidate: &BTreeSet<DiscoveryMember>,
    dismissals: &[BTreeSet<DiscoveryMember>],
) -> bool {
    !candidate.is_empty()
        && dismissals
            .iter()
            .any(|dismissal| candidate.intersection(dismissal).count() * 2 >= candidate.len())
}

fn overlap_ratio_at_least_half(
    left: &BTreeSet<DiscoveryMember>,
    right: &BTreeSet<DiscoveryMember>,
) -> bool {
    let denominator = left.len().min(right.len());
    denominator != 0 && left.intersection(right).count() * 2 >= denominator
}

fn member_set(members: &[Value]) -> Result<BTreeSet<DiscoveryMember>, String> {
    canonical_members(members).map(|members| {
        members
            .into_iter()
            .map(|member| {
                (
                    member["day"].as_str().expect("canonical day").to_owned(),
                    member["stream_layout"]
                        .as_str()
                        .expect("canonical stream layout")
                        .to_owned(),
                    member["stream"]
                        .as_str()
                        .expect("canonical stream")
                        .to_owned(),
                    member["segment_key"]
                        .as_str()
                        .expect("canonical segment key")
                        .to_owned(),
                    member["source"]
                        .as_str()
                        .expect("canonical source")
                        .to_owned(),
                    member["sentence_id"]
                        .as_i64()
                        .expect("canonical sentence id"),
                )
            })
            .collect()
    })
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
        let stream_layout = object
            .get("stream_layout")
            .map(|_| field("stream_layout"))
            .transpose()?
            .unwrap_or_else(|| "named".to_owned());
        if !matches!(stream_layout.as_str(), "direct" | "named") {
            return Err("invalid cluster member provenance".to_owned());
        }
        tuples.insert((
            field("day")?,
            stream_layout,
            field("stream")?,
            field("segment_key")?,
            field("source")?,
            sentence_id,
        ));
    }
    Ok(tuples.into_iter().map(|(day, stream_layout, stream, segment_key, source, sentence_id)| json!({"day":day,"stream_layout":stream_layout,"stream":stream,"segment_key":segment_key,"source":source,"sentence_id":sentence_id})).collect())
}

fn preflight_error(failure: IdentifyPreflightError) -> Response {
    match failure {
        IdentifyPreflightError::UnsupportedLayout => error(
            UNSUPPORTED_LAYOUT_REASON,
            UNSUPPORTED_LAYOUT_MESSAGE,
            UNSUPPORTED_LAYOUT_DETAIL,
            StatusCode::BAD_REQUEST,
        ),
        IdentifyPreflightError::Failed(detail) => {
            command(detail, StatusCode::INTERNAL_SERVER_ERROR)
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
fn map(value: Value) -> Response {
    match value.get("status").and_then(Value::as_str).unwrap_or("") {
        "identified" | "resolved" | "ambiguous" | "no_match" | "principal_match" | "undone"
        | "already_undone" => Json(value).into_response(),
        "recoverable" | "in_progress" | "undoing" => error(
            "speaker_identify_recoverable",
            "that speaker identify operation didn't finish, but it can be retried.",
            &value.to_string(),
            StatusCode::CONFLICT,
        ),
        "repair_required" | "undo_repair_required" => error(
            "speaker_identify_repair_required",
            "that speaker identify operation couldn't finish safely without repair.",
            &value.to_string(),
            StatusCode::CONFLICT,
        ),
        "conflict" | "operation_already_undone" => error(
            "speaker_identify_conflict",
            "that speaker identify operation couldn't run because it conflicts with existing state.",
            &value.to_string(),
            StatusCode::CONFLICT,
        ),
        "not_found" => error(
            "speaker_identify_operation_not_found",
            "that speaker identify operation couldn't be found.",
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

fn identify_error(detail: String) -> Response {
    let lower = detail.to_ascii_lowercase();
    if lower.contains("busy") || lower.contains("lock") {
        let (code, message) =
            if lower.contains("speaker_labels") || lower.contains("speaker_corrections") {
                (
                    "speaker_labels_busy",
                    "speaker labels couldn't be updated because another update is running.",
                )
            } else {
                (
                    "speaker_voiceprint_busy",
                    "that voice couldn't be updated because another update is running.",
                )
            };
        return error(code, message, &detail, StatusCode::SERVICE_UNAVAILABLE);
    }
    command(detail, StatusCode::INTERNAL_SERVER_ERROR)
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
        "that request couldn't be used.",
        detail,
        StatusCode::BAD_REQUEST,
    )
}
fn command(detail: String, status: StatusCode) -> Response {
    error(
        "speaker_command_failed",
        "that speaker command didn't finish.",
        &detail,
        status,
    )
}
fn error(code: &str, message: &str, detail: &str, status: StatusCode) -> Response {
    error_envelope(code, message, detail, status).into_response()
}

#[cfg(test)]
mod tests {
    use super::{
        DiscoveryCandidate, MAX_UNMATCHED_EMBEDDINGS, canonical_members, map, numpy_choice_indexes,
        retain_discovery_clusters,
    };
    use axum::response::IntoResponse;
    use serde_json::json;
    use sha2::{Digest, Sha256};
    use solstone_core_journal_io::SegmentLayout;

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
