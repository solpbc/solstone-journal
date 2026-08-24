// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only speaker discovery-cache routes.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Json;
use axum::extract::{Extension, Path as RoutePath, Query};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};
use solstone_core_convey_http::envelope::error_envelope;
use solstone_core_journal_io::SegmentLayout;

use crate::JournalRoot;
use crate::speakers_calendar::{
    journal_principal_id, load_all_journal_entities, load_segment_speakers, load_speaker_labels,
    value_truthy,
};
use crate::speakers_review::{audio_info, find_matching_entity};
use crate::speakers_segment_catalog::{
    DirectSupport, SegmentLookup, decode_stream_layout_value, lookup_segment,
};

#[derive(Debug, Deserialize)]
pub struct ResolveStatementQuery {
    voice_day: Option<String>,
    voice_stream_layout: Option<String>,
    voice_stream: Option<String>,
    voice_segment_key: Option<String>,
    voice_source: Option<String>,
    voice_sentence_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum MemberLayout {
    Direct,
    Named,
}

impl MemberLayout {
    fn from_layout(layout: SegmentLayout) -> Self {
        match layout {
            SegmentLayout::Direct => Self::Direct,
            SegmentLayout::Named => Self::Named,
        }
    }

    fn layout(self) -> SegmentLayout {
        match self {
            Self::Direct => SegmentLayout::Direct,
            Self::Named => SegmentLayout::Named,
        }
    }

    fn flag(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Named => "named",
        }
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Member {
    day: String,
    stream: String,
    segment_key: String,
    source: String,
    sentence_id: i64,
    layout: MemberLayout,
}

#[derive(Default)]
struct EvidenceBuckets {
    screen: BTreeSet<ConversationKey>,
    meeting_day: BTreeSet<String>,
    setting: BTreeSet<ConversationKey>,
    speakers: BTreeSet<ConversationKey>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ConversationKey(String, String, String, String, String);

type Evidence = Vec<(String, Vec<String>)>;
type EvidenceGaps = Vec<(String, String)>;

pub async fn cache(Extension(root): Extension<Arc<JournalRoot>>) -> Response {
    let Some(cache) = load_discovery_cache(&root.0) else {
        return Json(json!({"status": "cache_unavailable", "clusters": []})).into_response();
    };
    let clusters = cache
        .get("clusters")
        .and_then(Value::as_object)
        .expect("discovery cache was structurally validated");
    match serialize_clusters(&root.0, clusters) {
        Ok(clusters) => Json(json!({"status": "ok", "clusters": clusters})).into_response(),
        Err(detail) => error_envelope(
            "speaker_command_failed",
            "I couldn't finish that speaker command.",
            detail,
            StatusCode::INTERNAL_SERVER_ERROR,
        )
        .into_response(),
    }
}

pub async fn presence(
    Extension(root): Extension<Arc<JournalRoot>>,
    RoutePath(cluster_id): RoutePath<String>,
) -> Response {
    if cluster_id.is_empty() || !cluster_id.bytes().all(|byte| byte.is_ascii_digit()) {
        return error_envelope(
            "http_error",
            "I couldn't complete that request.",
            "",
            StatusCode::NOT_FOUND,
        )
        .into_response();
    }
    let raw_cluster_id = cluster_id;
    let cluster_id = raw_cluster_id.parse::<i64>().ok();
    let Some(cluster_id) = cluster_id else {
        return cluster_not_found(&raw_cluster_id);
    };
    match cluster_presence(&root.0, cluster_id) {
        Ok(Some(payload)) => Json(payload).into_response(),
        Ok(None) => cluster_not_found(&cluster_id.to_string()),
        Err(detail) => error_envelope(
            "speaker_command_failed",
            "I couldn't finish that speaker command.",
            detail,
            StatusCode::INTERNAL_SERVER_ERROR,
        )
        .into_response(),
    }
}

pub async fn resolve_statement(
    Extension(root): Extension<Arc<JournalRoot>>,
    Query(query): Query<ResolveStatementQuery>,
) -> Response {
    let layout = match crate::speakers_segment_catalog::decode_stream_layout(
        query.voice_stream_layout.as_deref(),
    ) {
        Ok(layout) => MemberLayout::from_layout(layout),
        Err(error) => {
            return error_envelope(
                "invalid_request_value",
                "I couldn't use one of those values.",
                error.to_string(),
                StatusCode::BAD_REQUEST,
            )
            .into_response();
        }
    };
    let values = [
        ("voice_day", query.voice_day),
        ("voice_stream", query.voice_stream),
        ("voice_segment_key", query.voice_segment_key),
        ("voice_source", query.voice_source),
        ("voice_sentence_id", query.voice_sentence_id),
    ];
    let missing = values
        .iter()
        .filter_map(|(name, value)| {
            value
                .as_deref()
                .filter(|value| !value.is_empty())
                .is_none()
                .then_some(*name)
        })
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return error_envelope(
            "missing_required_field",
            "I couldn't complete that request.",
            format!("Missing required fields: {}", missing.join(", ")),
            StatusCode::BAD_REQUEST,
        )
        .into_response();
    }
    let sentence_id = match values[4]
        .1
        .as_deref()
        .unwrap_or_default()
        .trim()
        .parse::<i64>()
    {
        Ok(sentence_id) => sentence_id,
        Err(_) => {
            return error_envelope(
                "invalid_request_value",
                "I couldn't use one of those values.",
                "voice_sentence_id must be an integer",
                StatusCode::BAD_REQUEST,
            )
            .into_response();
        }
    };
    let [
        (_, Some(day)),
        (_, Some(stream)),
        (_, Some(segment_key)),
        (_, Some(source)),
        _,
    ] = values
    else {
        unreachable!("missing required query values returned above");
    };
    Json(resolve_statement_cluster(
        &root.0,
        &day,
        &stream,
        &segment_key,
        &source,
        sentence_id,
        layout,
    ))
    .into_response()
}

fn load_discovery_cache(root: &Path) -> Option<Value> {
    let cache: Value =
        serde_json::from_slice(&fs::read(root.join("awareness/discovery_clusters.json")).ok()?)
            .ok()?;
    cache.as_object()?.get("clusters")?.as_object()?;
    Some(cache)
}

fn normalize_member(member: &Value) -> Option<Member> {
    let object = member.as_object()?;
    let string = |name| {
        object
            .get(name)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    Some(Member {
        day: string("day")?,
        stream: string("stream")?,
        segment_key: string("segment_key")?,
        source: string("source")?,
        sentence_id: member_sentence_id(object.get("sentence_id")?)?,
        layout: MemberLayout::from_layout(
            decode_stream_layout_value(object.get("stream_layout")).ok()?,
        ),
    })
}

fn member_sentence_id(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(|value| value.trim().parse().ok()))
        .or_else(|| value.as_bool().map(i64::from))
}

fn serialize_clusters(
    root: &Path,
    clusters: &serde_json::Map<String, Value>,
) -> Result<Vec<Value>, String> {
    let mut rows = Vec::new();
    for (raw_id, members) in clusters {
        let cluster_id = raw_id
            .parse::<i64>()
            .map_err(|_| format!("invalid discovery cluster id: {raw_id}"))?;
        let members = members
            .as_array()
            .ok_or_else(|| format!("invalid discovery cluster members: {raw_id}"))?;
        let members = members
            .iter()
            .map(normalize_member)
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| format!("invalid discovery cluster member: {raw_id}"))?;
        if !members.is_empty() && !dismissal_suppressed(root, &members)? {
            rows.push(serialize_cluster(root, cluster_id, &members));
        }
    }
    rows.sort_by(|left, right| {
        right["size"]
            .as_u64()
            .cmp(&left["size"].as_u64())
            .then_with(|| {
                left["cluster_id"]
                    .as_i64()
                    .cmp(&right["cluster_id"].as_i64())
            })
    });
    Ok(rows)
}

fn serialize_cluster(root: &Path, cluster_id: i64, members: &[Member]) -> Value {
    let segments = members.iter().map(Member::segment).collect::<BTreeSet<_>>();
    let mut samples = samples_by_segment(root, members);
    if samples.len() < 3 {
        for member in members {
            if let Some(sample) = cluster_sample(root, member, None)
                && !samples.contains(&sample)
            {
                samples.push(sample);
            }
            if samples.len() == 3 {
                break;
            }
        }
    }
    json!({
        "cluster_id": cluster_id,
        "size": members.len(),
        "segment_count": segments.len(),
        "samples": samples,
    })
}

fn cluster_presence(root: &Path, cluster_id: i64) -> Result<Option<Value>, String> {
    let Some(cache) = load_discovery_cache(root) else {
        return Ok(None);
    };
    let clusters = cache
        .get("clusters")
        .and_then(Value::as_object)
        .ok_or_else(|| "invalid discovery cache clusters".to_owned())?;
    let cluster_key = cluster_id.to_string();
    let Some(raw_members) = clusters.get(&cluster_key) else {
        return Ok(None);
    };
    let raw_members = raw_members
        .as_array()
        .ok_or_else(|| format!("invalid discovery cluster members: {cluster_id}"))?;
    if raw_members.is_empty() {
        return Ok(None);
    }
    let members = raw_members
        .iter()
        .map(normalize_member)
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| format!("invalid discovery cluster member: {cluster_id}"))?;
    let context = conversation_context(root, &members);
    let all_entities = load_all_journal_entities(root);
    let principal_id = journal_principal_id(root);
    let mut buckets = BTreeMap::<String, EvidenceBuckets>::new();
    let mut evidence_gaps = Vec::new();
    for segment in &context.segment_order {
        let segment_dir = match lookup_segment(
            root,
            &segment.0,
            &segment.1,
            &segment.2,
            Ok(layout_from_flag(segment.3)),
            DirectSupport::Allow,
        ) {
            SegmentLookup::Present(path) => path,
            SegmentLookup::Absent => {
                evidence_gaps.push(json!({
                    "day": segment.0,
                    "stream_layout": segment.3,
                    "stream": segment.1,
                    "segment_key": segment.2,
                    "source": Value::Null,
                    "reason": "segment_missing",
                }));
                continue;
            }
            SegmentLookup::MalformedLayout => {
                evidence_gaps.push(json!({
                    "day": segment.0,
                    "stream_layout": segment.3,
                    "stream": segment.1,
                    "segment_key": segment.2,
                    "source": Value::Null,
                    "reason": "segment_identity_invalid",
                }));
                continue;
            }
            SegmentLookup::Failed(error) => {
                evidence_gaps.push(json!({
                    "day": segment.0,
                    "stream_layout": segment.3,
                    "stream": segment.1,
                    "segment_key": segment.2,
                    "source": Value::Null,
                    "reason": format!("segment_lookup_failed: {error}"),
                }));
                continue;
            }
            SegmentLookup::UnsupportedLayout => unreachable!("discovery reads allow Direct"),
        };
        let (evidence, gaps) = segment_evidence(&segment_dir, &all_entities);
        for gap in gaps {
            evidence_gaps.push(json!({
                "day": segment.0,
                "stream_layout": segment.3,
                "stream": segment.1,
                "segment_key": segment.2,
                "source": gap.0,
                "reason": gap.1,
            }));
        }
        let conversation = context
            .conversation_keys
            .get(segment)
            .expect("conversation context includes every segment");
        for (entity_id, sources) in evidence {
            let bucket = buckets.entry(entity_id).or_default();
            for source in sources {
                match source.as_str() {
                    "screen" => {
                        bucket.screen.insert(conversation.clone());
                    }
                    "meeting_day" => {
                        bucket.meeting_day.insert(segment.0.clone());
                    }
                    "setting" => {
                        bucket.setting.insert(conversation.clone());
                    }
                    "speakers" => {
                        bucket.speakers.insert(conversation.clone());
                    }
                    _ => {}
                }
            }
        }
    }
    let mut candidates = buckets
        .into_iter()
        .filter(|(entity_id, _)| principal_id.as_deref() != Some(entity_id))
        .filter_map(|(entity_id, bucket)| {
            presence_candidate(root, &all_entities, &entity_id, bucket)
        })
        .collect::<Vec<_>>();
    let mut co_presence = candidates
        .iter()
        .filter(|candidate| {
            candidate["screen_conversations"]
                .as_u64()
                .unwrap_or_default()
                > 0
                || candidate["meeting_days"].as_u64().unwrap_or_default() > 0
        })
        .cloned()
        .collect::<Vec<_>>();
    co_presence.sort_by(presence_sort);
    let co_presence_ids = co_presence
        .iter()
        .filter_map(|candidate| candidate["entity_id"].as_str())
        .collect::<BTreeSet<_>>();
    candidates.retain(|candidate| {
        !co_presence_ids.contains(candidate["entity_id"].as_str().unwrap_or_default())
    });
    let mut mention = candidates
        .into_iter()
        .filter(|candidate| {
            candidate["setting_conversations"]
                .as_u64()
                .unwrap_or_default()
                > 0
                || candidate["speaker_conversations"]
                    .as_u64()
                    .unwrap_or_default()
                    > 0
        })
        .collect::<Vec<_>>();
    mention.sort_by(mention_sort);

    let days = context
        .first_members
        .keys()
        .map(|(day, _, _, _)| day.clone())
        .collect::<BTreeSet<_>>();
    let streams = context
        .first_members
        .keys()
        .map(|(_, stream, _, _)| stream.clone())
        .collect::<BTreeSet<_>>();
    Ok(Some(json!({
        "cluster_id": cluster_id,
        "facts": {
            "statement_count": raw_members.len(),
            "segment_count": context.first_members.len(),
            "day_count": days.len(),
            "streams": streams,
            "conversation_count": context.conversation_keys.values().collect::<BTreeSet<_>>().len(),
            "samples": context
                .segment_order
                .iter()
                .take(3)
                .filter_map(|segment| {
                    cluster_sample(
                        root,
                        context
                            .first_members
                            .get(segment)
                            .expect("segment order refers to a first member"),
                        Some(
                            context
                                .settings
                                .get(segment)
                                .expect("segment order has a setting")
                                .clone(),
                        ),
                    )
                })
                .collect::<Vec<_>>(),
        },
        "evidence_complete": evidence_gaps.is_empty(),
        "evidence_gaps": evidence_gaps,
        "candidates": {"co_presence": co_presence, "mention": mention},
    })))
}

struct ConversationContext {
    segment_order: Vec<(String, String, String, &'static str)>,
    first_members: BTreeMap<(String, String, String, &'static str), Member>,
    conversation_keys: BTreeMap<(String, String, String, &'static str), ConversationKey>,
    settings: BTreeMap<(String, String, String, &'static str), Option<String>>,
}

fn conversation_context(root: &Path, members: &[Member]) -> ConversationContext {
    let mut first_members = BTreeMap::new();
    let mut segment_order = Vec::new();
    for member in members {
        let segment = member.segment();
        if let std::collections::btree_map::Entry::Vacant(entry) = first_members.entry(segment) {
            segment_order.push(entry.key().clone());
            entry.insert(member.clone());
        }
    }
    let mut settings = BTreeMap::new();
    let mut conversation_keys = BTreeMap::new();
    for segment in first_members.keys() {
        let setting = resolved_segment_dir(
            root,
            &segment.0,
            &segment.1,
            &segment.2,
            layout_from_flag(segment.3),
        )
        .and_then(|path| setting_field(&path));
        let key = match &setting {
            Some(setting) if !setting.is_empty() => ConversationKey(
                segment.0.clone(),
                segment.3.to_owned(),
                segment.1.clone(),
                setting.clone(),
                String::new(),
            ),
            _ => ConversationKey(
                segment.0.clone(),
                segment.3.to_owned(),
                segment.1.clone(),
                "__segment__".to_owned(),
                segment.2.clone(),
            ),
        };
        settings.insert(segment.clone(), setting);
        conversation_keys.insert(segment.clone(), key);
    }
    ConversationContext {
        segment_order,
        first_members,
        conversation_keys,
        settings,
    }
}

fn samples_by_segment(root: &Path, members: &[Member]) -> Vec<Value> {
    let mut seen = BTreeSet::new();
    let mut samples = Vec::new();
    for member in members {
        if seen.insert(member.segment())
            && let Some(sample) = cluster_sample(root, member, None)
        {
            samples.push(sample);
        }
        if samples.len() == 3 {
            break;
        }
    }
    samples
}

fn cluster_sample(root: &Path, member: &Member, setting: Option<Option<String>>) -> Option<Value> {
    let segment_dir = resolved_segment_dir(
        root,
        &member.day,
        &member.stream,
        &member.segment_key,
        member.layout.layout(),
    )?;
    let (audio_url, _) = audio_info(
        &segment_dir,
        &member.day,
        &member.stream,
        &member.segment_key,
        &member.source,
        member.layout.layout(),
    );
    let mut sample = json!({
        "day": member.day,
        "stream_layout": member.layout.flag(),
        "stream": member.stream,
        "segment_key": member.segment_key,
        "source": member.source,
        "sentence_id": member.sentence_id,
        "audio_url": audio_url,
        "text": sentence_text(&segment_dir, &member.source, member.sentence_id).unwrap_or_default(),
    });
    if let Some(setting) = setting
        && let Some(object) = sample.as_object_mut()
    {
        object.insert("setting".to_owned(), json!(setting));
    }
    Some(sample)
}

fn sentence_text(segment_dir: &Path, source: &str, sentence_id: i64) -> Option<String> {
    let contents = fs::read_to_string(segment_dir.join(format!("{source}.jsonl"))).ok()?;
    let lines = contents.lines().collect::<Vec<_>>();
    if sentence_id < 1 || usize::try_from(sentence_id).ok()? >= lines.len() {
        return None;
    }
    serde_json::from_str::<Value>(lines[usize::try_from(sentence_id).ok()?])
        .ok()?
        .get("text")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn setting_field(segment_dir: &Path) -> Option<String> {
    let first = fs::read_to_string(segment_dir.join("imported_audio.jsonl"))
        .ok()?
        .lines()
        .next()?
        .to_owned();
    serde_json::from_str::<Value>(&first)
        .ok()?
        .get("setting")?
        .as_str()
        .map(str::to_owned)
}

// This route deliberately scopes the much larger Python attribution reader to
// the frozen journal's speakers channel. The shared matcher already covers all
// eight tiers, including rapidfuzz-backed fuzzy resolution; setting, screen,
// and meeting inputs plus ambiguity-aware resolution need a future full port.
fn segment_evidence(segment_dir: &Path, entities: &[(String, Value)]) -> (Evidence, EvidenceGaps) {
    if let Some(labels) = load_speaker_labels(segment_dir)
        && labels.get("candidate_evidence").is_some()
    {
        let evidence = labels
            .get("candidate_evidence")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|item| {
                let entity_id = item.get("entity_id")?.as_str()?.to_owned();
                let sources = item
                    .get("sources")?
                    .as_array()?
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect();
                Some((entity_id, sources))
            })
            .collect();
        let gaps = labels
            .get("candidate_evidence_gaps")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|gap| {
                Some((
                    gap.get("source")?.as_str()?.to_owned(),
                    gap.get("reason")?.as_str()?.to_owned(),
                ))
            })
            .collect();
        return (evidence, gaps);
    }
    let entities = entities
        .iter()
        .filter(|(_, entity)| !entity.get("blocked").is_some_and(value_truthy))
        .cloned()
        .collect::<Vec<_>>();
    let evidence = load_segment_speakers(segment_dir)
        .into_iter()
        .filter_map(|name| {
            let entity = find_matching_entity(&name, &entities)?;
            entity
                .get("id")
                .and_then(Value::as_str)
                .map(|id| (id.to_owned(), vec!["speakers".to_owned()]))
        })
        .collect();
    (evidence, Vec::new())
}

fn presence_candidate(
    root: &Path,
    entities: &[(String, Value)],
    entity_id: &str,
    bucket: EvidenceBuckets,
) -> Option<Value> {
    let (_, entity) = entities.iter().find(|(id, _)| id == entity_id)?;
    if entity.get("blocked").is_some_and(value_truthy) {
        return None;
    }
    // Read-only presence badge. Merge bookkeeping resolves through
    // entity_memory_path; this listing does not write.
    Some(json!({
        "entity_id": entity_id,
        "name": entity.get("name").cloned().unwrap_or_else(|| json!(entity_id)),
        "has_voice": root.join("entities").join(entity_id).join("voiceprints.npz").is_file(),
        "screen_conversations": bucket.screen.len(),
        "meeting_days": bucket.meeting_day.len(),
        "setting_conversations": bucket.setting.len(),
        "speaker_conversations": bucket.speakers.len(),
    }))
}

fn presence_sort(left: &Value, right: &Value) -> std::cmp::Ordering {
    right["screen_conversations"]
        .as_u64()
        .cmp(&left["screen_conversations"].as_u64())
        .then_with(|| {
            right["meeting_days"]
                .as_u64()
                .cmp(&left["meeting_days"].as_u64())
        })
        .then_with(|| left["name"].as_str().cmp(&right["name"].as_str()))
        .then_with(|| left["entity_id"].as_str().cmp(&right["entity_id"].as_str()))
}

fn mention_sort(left: &Value, right: &Value) -> std::cmp::Ordering {
    right["setting_conversations"]
        .as_u64()
        .cmp(&left["setting_conversations"].as_u64())
        .then_with(|| {
            right["speaker_conversations"]
                .as_u64()
                .cmp(&left["speaker_conversations"].as_u64())
        })
        .then_with(|| left["name"].as_str().cmp(&right["name"].as_str()))
        .then_with(|| left["entity_id"].as_str().cmp(&right["entity_id"].as_str()))
}

fn resolve_statement_cluster(
    root: &Path,
    day: &str,
    stream: &str,
    segment_key: &str,
    source: &str,
    sentence_id: i64,
    layout: MemberLayout,
) -> Value {
    let Some(cache) = load_discovery_cache(root) else {
        return json!({"status": "cache_unavailable", "cluster_id": null});
    };
    let mut clusters = Vec::new();
    for (raw_id, members) in cache["clusters"].as_object().into_iter().flatten() {
        let Some(cluster_id) = raw_id.parse::<i64>().ok() else {
            return json!({"status": "cache_incomplete", "cluster_id": null});
        };
        let Some(members) = members.as_array() else {
            return json!({"status": "cache_incomplete", "cluster_id": null});
        };
        let Some(members) = members
            .iter()
            .map(normalize_member)
            .collect::<Option<Vec<_>>>()
        else {
            return json!({"status": "cache_incomplete", "cluster_id": null});
        };
        clusters.push((cluster_id, members));
    }
    clusters.sort_by_key(|(cluster_id, _)| *cluster_id);
    for (cluster_id, members) in clusters {
        for member in members {
            if member.day == day
                && member.stream == stream
                && member.segment_key == segment_key
                && member.source == source
                && member.sentence_id == sentence_id
                && member.layout == layout
            {
                return json!({"status": "hit", "cluster_id": cluster_id});
            }
        }
    }
    json!({"status": "miss", "cluster_id": null})
}

fn cluster_not_found(cluster_id: &str) -> Response {
    error_envelope(
        "speaker_review_unavailable",
        "I couldn't load that speaker review.",
        format!("Cluster {cluster_id} was not found. Run a discovery scan first."),
        StatusCode::NOT_FOUND,
    )
    .into_response()
}

fn dismissal_suppressed(root: &Path, members: &[Member]) -> Result<bool, String> {
    let candidate = members.iter().cloned().collect::<BTreeSet<_>>();
    if candidate.is_empty() {
        return Ok(false);
    }
    let path = root.join("speakers/cluster-dismissals.jsonl");
    let contents = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(format!("failed to read {}: {error}", path.display())),
    };
    let mut dismissals = Vec::new();
    for (index, line) in contents.lines().enumerate() {
        let event: Value = serde_json::from_str(line)
            .map_err(|error| format!("invalid dismissal line {}: {error}", index + 1))?;
        let members = event
            .get("members")
            .and_then(Value::as_array)
            .ok_or_else(|| format!("invalid dismissal members on line {}", index + 1))?;
        let dismissed = members
            .iter()
            .map(normalize_member)
            .collect::<Option<BTreeSet<_>>>()
            .ok_or_else(|| format!("invalid dismissal member on line {}", index + 1))?;
        dismissals.push(dismissed);
    }
    Ok(dismissals.into_iter().any(|dismissed| {
        !dismissed.is_empty() && candidate.intersection(&dismissed).count() * 2 >= candidate.len()
    }))
}

impl Member {
    fn segment(&self) -> (String, String, String, &'static str) {
        (
            self.day.clone(),
            self.stream.clone(),
            self.segment_key.clone(),
            self.layout.flag(),
        )
    }
}

fn layout_from_flag(flag: &str) -> SegmentLayout {
    match flag {
        "direct" => SegmentLayout::Direct,
        _ => SegmentLayout::Named,
    }
}

fn resolved_segment_dir(
    root: &Path,
    day: &str,
    stream: &str,
    segment_key: &str,
    layout: SegmentLayout,
) -> Option<PathBuf> {
    match lookup_segment(
        root,
        day,
        stream,
        segment_key,
        Ok(layout),
        DirectSupport::Allow,
    ) {
        SegmentLookup::Present(path) => Some(path),
        _ => None,
    }
}
