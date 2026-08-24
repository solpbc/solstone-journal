// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeSet, HashSet};
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
use solstone_core_journal_io::{LockOptions, SegmentLayout, hold_lock};

use crate::JournalRoot;
use crate::speakers_attribution::action;
use crate::speakers_segment_catalog::{
    DirectSupport, SegmentLookup, UNSUPPORTED_LAYOUT_DETAIL, UNSUPPORTED_LAYOUT_MESSAGE,
    UNSUPPORTED_LAYOUT_REASON, catalog_journal, decode_stream_layout_value, lookup_segment,
};

const DISCOVERY_UNIT_NORM_TOLERANCE: f32 = 1.0e-3;
const MIN_CLUSTER_SIZE: usize = 5;
const MAX_UNMATCHED_EMBEDDINGS: usize = 10_000;
const OWNER_VOICE_UNAVAILABLE: &str = "speaker_discovery_owner_voice_unavailable";
const OWNER_VOICE_UNAVAILABLE_MESSAGE: &str =
    "i need your voice set up before looking for new voices.";
const INVALID_EMBEDDINGS: &str = "speaker_discovery_invalid_embeddings";
const INVALID_EMBEDDINGS_MESSAGE: &str =
    "i skipped some voice samples because they were not usable.";
static DISCOVERY_CACHE_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

type DiscoveryMember = (String, String, String, String, String, i64);

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DiscoveryCandidate {
    pub(crate) day: String,
    pub(crate) stream_layout: SegmentLayout,
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

pub(crate) fn discovery_candidates(root: &std::path::Path) -> Result<DiscoveryCandidates, String> {
    let Some(principal) = crate::speakers_calendar::journal_principal_id(root) else {
        return Ok(DiscoveryCandidates::NoConfirmedOwner);
    };
    let owner =
        solstone_core_speaker_resolve::owner_centroid::load_owner_centroid(root, &principal)
            .map_err(|error| error.to_string())?;
    let Some(owner) = owner else {
        return Ok(DiscoveryCandidates::NoConfirmedOwner);
    };
    let mut rows = Vec::new();
    let mut dropped = 0;
    for segment in catalog_journal(root).map_err(|error| error.to_string())? {
        let labels_path = segment.path.join("talents/speaker_labels.json");
        let attributed = match fs::read(&labels_path) {
            Ok(bytes) => {
                let value = serde_json::from_slice::<Value>(&bytes).map_err(|error| {
                    format!("failed to parse {}: {error}", labels_path.display())
                })?;
                let labels = value
                    .get("labels")
                    .and_then(Value::as_array)
                    .ok_or_else(|| {
                        format!("invalid speaker labels at {}", labels_path.display())
                    })?;
                let mut attributed = BTreeSet::new();
                for row in labels {
                    if row.get("speaker").is_some_and(|value| !value.is_null()) {
                        let sentence_id = row
                            .get("sentence_id")
                            .and_then(Value::as_i64)
                            .ok_or_else(|| {
                                format!(
                                    "invalid attributed sentence id at {}",
                                    labels_path.display()
                                )
                            })?;
                        attributed.insert(sentence_id);
                    }
                }
                attributed
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => BTreeSet::new(),
            Err(error) => {
                return Err(format!("failed to read {}: {error}", labels_path.display()));
            }
        };
        let entries = fs::read_dir(&segment.path)
            .map_err(|error| format!("failed to read {}: {error}", segment.path.display()))?;
        for entry in entries {
            let entry = entry.map_err(|error| {
                format!(
                    "failed to read entry in {}: {error}",
                    segment.path.display()
                )
            })?;
            let path = entry.path();
            let Some(source) = path.file_stem().and_then(|name| name.to_str()) else {
                continue;
            };
            if path.extension().and_then(|ext| ext.to_str()) != Some("npz") {
                continue;
            }
            let Some(file) = solstone_core_speaker_id::embeddings::load_embeddings_file(&path)
                .map_err(|error| format!("failed to load {}: {error}", path.display()))?
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
                    day: segment.day.clone(),
                    stream_layout: segment.layout,
                    stream: segment.stream.clone(),
                    segment_key: segment.name.clone(),
                    source: source.to_owned(),
                    sentence_id,
                    embedding,
                });
            }
        }
    }
    Ok(DiscoveryCandidates::Candidates {
        rows,
        dropped_invalid: dropped,
    })
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
            .map(|row| {
                (
                    &row.day,
                    layout_name(row.stream_layout),
                    &row.stream,
                    &row.segment_key,
                )
            })
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
        output.insert(label.to_string(),selected.into_iter().map(|row|json!({"day":row.day,"stream_layout":layout_name(row.stream_layout),"stream":row.stream,"segment_key":row.segment_key,"source":row.source,"sentence_id":row.sentence_id})).collect());
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
        Ok(DiscoveryCandidates::NoConfirmedOwner) => {
            return Json(json!({
                "status": "degraded",
                "clusters": [],
                "issues": [issue(OWNER_VOICE_UNAVAILABLE, OWNER_VOICE_UNAVAILABLE_MESSAGE, 0)],
            }))
            .into_response();
        }
        Ok(DiscoveryCandidates::Candidates {
            rows,
            dropped_invalid,
        }) => {
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
        Err(error) => return command(error, StatusCode::INTERNAL_SERVER_ERROR),
    };

    if rows.len() < MIN_CLUSTER_SIZE {
        if let Err(error) = clear_discovery_cache(&root.0) {
            return command(error, StatusCode::INTERNAL_SERVER_ERROR);
        }
        return Json(scan_result(Vec::new(), issues)).into_response();
    }

    let rows = cap_discovery_candidates(rows);
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
    let visible_clusters = match serialize_scan_clusters(&root.0, &clusters) {
        Ok(clusters) => clusters,
        Err(error) => return command(error, StatusCode::INTERNAL_SERVER_ERROR),
    };
    Json(scan_result(visible_clusters, issues)).into_response()
}

/// Match Python's `default_rng(42).choice(..., replace=False)` admission cap.
///
/// Python sorts the sampled indexes before carrying provenance forward, so the
/// result is stable and independent of the helper's clustering order.
fn cap_discovery_candidates(rows: Vec<DiscoveryCandidate>) -> Vec<DiscoveryCandidate> {
    if rows.len() <= MAX_UNMATCHED_EMBEDDINGS {
        return rows;
    }
    let indexes = numpy_choice_indexes(rows.len(), MAX_UNMATCHED_EMBEDDINGS);
    indexes
        .into_iter()
        .map(|index| rows[index].clone())
        .collect()
}

/// Exact subset selection used by NumPy 2.x's `default_rng(42).choice` here.
///
/// This ports its seeded PCG64 stream, Lemire-bounded draws, and its tail
/// shuffle/Floyd choice split. Callers sort no further: this function returns
/// the same ascending indexes Python produces after `indices.sort()`.
fn numpy_choice_indexes(population: usize, size: usize) -> Vec<usize> {
    debug_assert!(size <= population);
    let mut rng = NumpyPcg64::seed_42();
    let mut indexes = if population > 10_000 && size > population / 50 {
        let mut values = (0..population).collect::<Vec<_>>();
        for index in (population - size..population).rev() {
            let selected = rng.bounded_usize(index);
            values.swap(selected, index);
        }
        values.split_off(population - size)
    } else {
        let mut values = Vec::with_capacity(size);
        let mut seen = HashSet::with_capacity(size);
        for value in population - size..population {
            let selected = rng.bounded_usize(value);
            if !seen.insert(selected) {
                values.push(value);
                seen.insert(value);
            } else {
                values.push(selected);
            }
        }
        values
    };
    indexes.sort_unstable();
    indexes
}

struct NumpyPcg64 {
    state: u128,
    increment: u128,
    cached_u32: Option<u32>,
}

impl NumpyPcg64 {
    // State emitted by NumPy's `default_rng(42).bit_generator.state`.
    const SEED_42_STATE: u128 = 274_674_114_334_540_486_603_088_602_300_644_985_544;
    const SEED_42_INCREMENT: u128 = 332_724_090_758_049_132_448_979_897_138_935_081_983;
    const MULTIPLIER: u128 = 0x2360_ed05_1fc6_5da4_4385_df64_9fcc_f645;

    fn seed_42() -> Self {
        Self {
            state: Self::SEED_42_STATE,
            increment: Self::SEED_42_INCREMENT,
            cached_u32: None,
        }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self
            .state
            .wrapping_mul(Self::MULTIPLIER)
            .wrapping_add(self.increment);
        let high = (self.state >> 64) as u64;
        let low = self.state as u64;
        (high ^ low).rotate_right((high >> 58) as u32)
    }

    fn next_u32(&mut self) -> u32 {
        if let Some(value) = self.cached_u32.take() {
            return value;
        }
        let value = self.next_u64();
        self.cached_u32 = Some((value >> 32) as u32);
        value as u32
    }

    fn bounded_usize(&mut self, inclusive_max: usize) -> usize {
        debug_assert!(inclusive_max <= u32::MAX as usize);
        let range = inclusive_max as u32;
        let range_exclusive = range.wrapping_add(1);
        loop {
            let product = self.next_u32() as u64 * range_exclusive as u64;
            let leftover = product as u32;
            if leftover >= range_exclusive || leftover >= (u32::MAX - range) % range_exclusive {
                return (product >> 32) as usize;
            }
        }
    }
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

fn layout_name(layout: SegmentLayout) -> &'static str {
    match layout {
        SegmentLayout::Direct => "direct",
        SegmentLayout::Named => "named",
    }
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

fn identify_error(detail: String) -> Response {
    let lower = detail.to_ascii_lowercase();
    if lower.contains("busy") || lower.contains("lock") {
        let (code, message) =
            if lower.contains("speaker_labels") || lower.contains("speaker_corrections") {
                (
                    "speaker_labels_busy",
                    "I couldn't update speaker labels because another update is running.",
                )
            } else {
                (
                    "speaker_voiceprint_busy",
                    "I couldn't update that voice because another update is running.",
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
