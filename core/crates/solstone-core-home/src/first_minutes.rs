// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Home getting-started first-minutes pure projection and reader.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

use chrono::{Offset, TimeDelta, TimeZone};
use chrono_tz::Tz;
use serde_json::Value;
use solstone_core_sol_link::http::{SetupProgress, SetupProgressError};

use crate::HomeContext;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoundStream {
    pub name: String,
    pub cid: Option<String>,
    pub platform_allocation: Option<String>,
}

#[derive(Clone, Debug)]
pub struct SegmentCandidate {
    pub day: String,
    pub key: String,
    pub stream: String,
    pub data_state: solstone_core_system_health::DataStateMap,
    pub reported_tz: Option<String>,
    pub reported_offset_seconds: Option<i32>,
}

#[derive(Clone, Debug)]
pub struct ProgressView {
    pub progress: BTreeMap<
        solstone_core_system_health::SegmentIdentity,
        solstone_core_system_health::SegmentProgress,
    >,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WinningSegment {
    pub day: String,
    pub key: String,
    pub stream: String,
    pub start_at: String,
    pub candidate_index: usize,
}

pub type SegmentWordsReader = Box<dyn Fn(&SegmentCandidate) -> Option<String>>;

pub struct FirstMinutesInput {
    pub now_ms: i64,
    pub owner_zone: Tz,
    pub capture: Value,
    pub reported_platforms: Option<BTreeMap<String, Option<String>>>,
    pub ledger_platforms: BTreeMap<String, String>,
    pub bound_streams: Option<Vec<BoundStream>>,
    pub segments: Option<Vec<SegmentCandidate>>,
    pub earliest_progress: Option<ProgressView>,
    pub brain_state: String,
    pub brain_reason: Option<String>,
    pub brain_progressing: bool,
    pub ui_lane: Option<String>,
    pub segment_words: SegmentWordsReader,
}

fn encode_path_component(val: &str) -> String {
    let mut encoded = String::new();
    for b in val.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-') {
            encoded.push(b as char);
        } else {
            encoded.push_str(&format!("%{:02X}", b));
        }
    }
    encoded
}

fn transcripts_href(day: &str, stream: &str, key: &str) -> String {
    format!(
        "/app/transcripts/{}?stream={}#{}",
        encode_path_component(day),
        encode_path_component(stream),
        encode_path_component(key),
    )
}

fn segment_wall_start(day: &str, key: &str) -> Option<chrono::NaiveDateTime> {
    let hhmmss = key.get(..6)?;
    let digits =
        |text: &str, len: usize| text.len() == len && text.bytes().all(|b| b.is_ascii_digit());
    if !digits(day, 8) || !digits(hhmmss, 6) {
        return None;
    }
    chrono::NaiveDateTime::parse_from_str(&format!("{day}{hhmmss}"), "%Y%m%d%H%M%S").ok()
}

fn candidate_instant(
    input: &FirstMinutesInput,
    candidate: &SegmentCandidate,
) -> Option<chrono::DateTime<chrono::Utc>> {
    let wall = segment_wall_start(&candidate.day, &candidate.key)?;
    let offset_seconds = if let Some(view) = solstone_core_journal_config::capture_zone_view(
        &candidate.day,
        &candidate.key,
        candidate.reported_tz.as_deref(),
        candidate.reported_offset_seconds,
        &input.owner_zone,
    ) {
        view.utc_offset_seconds
    } else {
        let offset = input
            .owner_zone
            .offset_from_local_datetime(&wall)
            .earliest()?;
        offset.fix().local_minus_utc()
    };
    let utc_naive = wall - TimeDelta::seconds(i64::from(offset_seconds));
    Some(chrono::DateTime::from_naive_utc_and_offset(
        utc_naive,
        chrono::Utc,
    ))
}

pub fn winning_segment(input: &FirstMinutesInput) -> Option<WinningSegment> {
    let segments = input.segments.as_ref()?;
    let mut winner: Option<(chrono::DateTime<chrono::Utc>, usize)> = None;

    for (idx, candidate) in segments.iter().enumerate() {
        let instant = match candidate_instant(input, candidate) {
            Some(inst) => inst,
            None => continue,
        };

        match &winner {
            Some((best_instant, best_idx)) => {
                if instant < *best_instant || (instant == *best_instant && idx < *best_idx) {
                    winner = Some((instant, idx));
                }
            }
            None => {
                winner = Some((instant, idx));
            }
        }
    }

    winner.map(|(instant, idx)| {
        let cand = &segments[idx];
        WinningSegment {
            day: cand.day.clone(),
            key: cand.key.clone(),
            stream: cand.stream.clone(),
            start_at: instant.to_rfc3339(),
            candidate_index: idx,
        }
    })
}

fn read_segment_words(dir: &std::path::Path) -> Option<String> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut files: Vec<std::path::PathBuf> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|ft| ft.is_file()).unwrap_or(false))
        .map(|e| e.path())
        .filter(|p| solstone_core_system_health::is_audio_jsonl(p))
        .collect();
    files.sort_by(|a, b| a.file_name().cmp(&b.file_name()));

    let mut all_words = Vec::new();
    for file in files {
        let Ok(bytes) = std::fs::read(&file) else {
            continue;
        };
        let Ok(read) = solstone_core_speaker_id::transcript::read_transcript_rows(&bytes) else {
            continue;
        };
        for row in read.rows {
            let val = &row.value;
            let text = ["corrected", "text"]
                .iter()
                .filter_map(|key| val.get(*key).and_then(Value::as_str))
                .map(str::trim)
                .find(|t| !t.is_empty());
            if let Some(t) = text {
                all_words.push(t.to_string());
            }
        }
    }

    if all_words.is_empty() {
        None
    } else {
        Some(all_words.join(" "))
    }
}

fn trim_proof_text(s: &str) -> String {
    let char_count = s.chars().count();
    if char_count <= 200 {
        return s.to_string();
    }
    let words: Vec<&str> = s.split_whitespace().collect();
    if !words.is_empty() {
        let last_word = words.last().unwrap();
        let last_word_len = last_word.chars().count();
        if last_word_len <= 199 {
            let mut total_len = 0;
            let mut word_count = 0;
            for (idx, w) in words.iter().rev().enumerate() {
                let w_len = w.chars().count();
                let additional = if idx == 0 { w_len } else { 1 + w_len };
                if total_len + additional <= 199 {
                    total_len += additional;
                    word_count += 1;
                } else {
                    break;
                }
            }
            if word_count > 0 {
                let suffix = words[words.len() - word_count..].join(" ");
                return format!("…{suffix}");
            }
        }
    }
    let skip = char_count.saturating_sub(199);
    let suffix: String = s.chars().skip(skip).collect();
    format!("…{suffix}")
}

pub fn project_first_minutes(input: &FirstMinutesInput) -> Value {
    let winner = winning_segment(input);

    let devices_known =
        input.capture.get("registry").and_then(Value::as_str) != Some("registry_unknown");

    let (devices_json, checks_phone) = if !devices_known {
        (serde_json::json!([]), Value::Null)
    } else {
        struct RawDevice {
            name: String,
            platform: Option<String>,
            state: &'static str,
            last_added_label: Option<String>,
            parsed_time: Option<i64>,
            original_index: usize,
        }

        let mut raw_devices = Vec::new();
        let mut idx = 0;

        if let Some(clients) = input.capture.get("clients").and_then(Value::as_array) {
            for c in clients {
                let name = c
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let cid = c.get("cid").and_then(Value::as_str).unwrap_or("");
                let status = c.get("status").and_then(Value::as_str);
                let state = match status {
                    Some("active") => "receiving",
                    Some("stale") | Some("offline") => "quiet",
                    Some("awaiting_first_delivery") => "awaiting_first",
                    _ => "other",
                };

                let reported_platform = input
                    .reported_platforms
                    .as_ref()
                    .and_then(|m| m.get(cid).and_then(|p| p.as_deref()));
                let ledger_platform = input
                    .ledger_platforms
                    .get(cid)
                    .and_then(|w| solstone_core_sol_link::Platform::from_wire(w));
                let allocation_input = input.bound_streams.as_ref().and_then(|streams| {
                    streams
                        .iter()
                        .find(|s| s.cid.as_deref() == Some(cid) && s.platform_allocation.is_some())
                        .and_then(|s| s.platform_allocation.as_deref())
                });
                let platform = crate::provenance::resolve_client_platform(
                    reported_platform,
                    ledger_platform,
                    allocation_input,
                );

                let parsed_time = c
                    .get("last_accepted_ingest_at")
                    .and_then(Value::as_str)
                    .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
                    .map(|dt| dt.timestamp_millis());

                let last_added_label = parsed_time.map(|delivery_ms| {
                    let seconds = ((input.now_ms - delivery_ms) / 1000) as f64;
                    format!("{} ago", crate::formatting::relative_time(seconds))
                });

                raw_devices.push(RawDevice {
                    name,
                    platform,
                    state,
                    last_added_label,
                    parsed_time,
                    original_index: idx,
                });
                idx += 1;
            }
        }

        if let Some(unassessed) = input.capture.get("unassessed").and_then(Value::as_array) {
            for c in unassessed {
                let name = c
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let cid = c.get("cid").and_then(Value::as_str).unwrap_or("");
                let reason = c.get("reason").and_then(Value::as_str);
                let state = match reason {
                    Some("awaiting_first_delivery") => "awaiting_first",
                    _ => "other",
                };

                let reported_platform = input
                    .reported_platforms
                    .as_ref()
                    .and_then(|m| m.get(cid).and_then(|p| p.as_deref()));
                let ledger_platform = input
                    .ledger_platforms
                    .get(cid)
                    .and_then(|w| solstone_core_sol_link::Platform::from_wire(w));
                let allocation_input = input.bound_streams.as_ref().and_then(|streams| {
                    streams
                        .iter()
                        .find(|s| s.cid.as_deref() == Some(cid) && s.platform_allocation.is_some())
                        .and_then(|s| s.platform_allocation.as_deref())
                });
                let platform = crate::provenance::resolve_client_platform(
                    reported_platform,
                    ledger_platform,
                    allocation_input,
                );

                let parsed_time = c
                    .get("last_accepted_ingest_at")
                    .and_then(Value::as_str)
                    .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
                    .map(|dt| dt.timestamp_millis());

                let last_added_label = parsed_time.map(|delivery_ms| {
                    let seconds = ((input.now_ms - delivery_ms) / 1000) as f64;
                    format!("{} ago", crate::formatting::relative_time(seconds))
                });

                raw_devices.push(RawDevice {
                    name,
                    platform,
                    state,
                    last_added_label,
                    parsed_time,
                    original_index: idx,
                });
                idx += 1;
            }
        }

        raw_devices.sort_by(|a, b| match (a.parsed_time, b.parsed_time) {
            (Some(ta), Some(tb)) => tb
                .cmp(&ta)
                .then_with(|| a.original_index.cmp(&b.original_index)),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a.original_index.cmp(&b.original_index),
        });

        let has_phone = raw_devices.iter().any(|d| {
            d.platform.as_deref() == Some("ios") || d.platform.as_deref() == Some("android")
        });

        let devices_array: Vec<Value> = raw_devices
            .into_iter()
            .map(|d| {
                let mut obj = serde_json::json!({
                    "name": d.name,
                    "platform": d.platform,
                    "state": d.state,
                });
                if let Some(lbl) = d.last_added_label {
                    obj["last_added_label"] = Value::String(lbl);
                }
                obj
            })
            .collect();

        (Value::Array(devices_array), Value::Bool(has_phone))
    };

    let (thinking_state, thinking_step) = if input.brain_state == "ready" {
        ("ready", "none")
    } else if input.brain_state == "checking"
        || (input.brain_state == "blocked" && input.brain_progressing)
    {
        ("progressing", "none")
    } else if input.brain_reason.as_deref() == Some("thinking_engine_not_chosen") {
        ("not_chosen", "choose")
    } else if input.brain_reason.as_deref() == Some("gpu_unavailable") {
        ("cant_run_local", "cant_run")
    } else if input.brain_state == "blocked"
        && !input.brain_progressing
        && matches!(
            input.brain_reason.as_deref(),
            Some(
                "provider_key_missing"
                    | "endpoint_configuration_incomplete"
                    | "local_artifact_not_ready"
            )
        )
    {
        let step = if input.brain_reason.as_deref() == Some("local_artifact_not_ready") {
            "local"
        } else if input.ui_lane.as_deref() == Some("confidential") {
            "confidential"
        } else {
            "byo"
        };
        ("needs_step", step)
    } else {
        ("other", "none")
    };

    let check_thinking = thinking_step == "none";

    let first_segment = match (input.segments.as_ref(), winner) {
        (Some(segs), Some(w)) => {
            let cand = &segs[w.candidate_index];
            let seg_input = solstone_core_system_health::SegmentInput {
                key: cand.key.clone(),
                stream: cand.stream.clone(),
                data_state: cand.data_state.clone(),
            };
            let thinking_val =
                if !solstone_core_system_health::segment_requires_processing(&seg_input)
                    || !solstone_core_system_health::segment_fully_sensed(&cand.data_state)
                {
                    "none"
                } else if let Some(pv) = &input.earliest_progress {
                    let progress = solstone_core_system_health::lookup_segment_progress(
                        &pv.progress,
                        &cand.stream,
                        &cand.key,
                    );
                    if solstone_core_system_health::segment_fully_thought(progress)
                        == solstone_core_system_health::ThoughtVerdict::Complete
                    {
                        "read"
                    } else if thinking_state == "ready" || thinking_state == "progressing" {
                        "pending"
                    } else {
                        "none"
                    }
                } else {
                    "none"
                };

            serde_json::json!({
                "start_at": w.start_at,
                "href": transcripts_href(&w.day, &w.stream, &w.key),
                "thinking": thinking_val,
            })
        }
        _ => Value::Null,
    };

    let (proof, checks_heard) = {
        let has_devices = {
            let clients_len = input
                .capture
                .get("clients")
                .and_then(Value::as_array)
                .map(|a| a.len())
                .unwrap_or(0);
            let unassessed_len = input
                .capture
                .get("unassessed")
                .and_then(Value::as_array)
                .map(|a| a.len())
                .unwrap_or(0);
            clients_len > 0 || unassessed_len > 0
        };

        if !devices_known || !has_devices || input.bound_streams.is_none() {
            (serde_json::json!({"state": "none"}), Value::Null)
        } else if let Some(segs) = &input.segments {
            if segs.is_empty() {
                (serde_json::json!({"state": "waiting"}), Value::Bool(false))
            } else if !segs.iter().any(|c| c.data_state.0.contains_key("audio")) {
                (serde_json::json!({"state": "no_audio"}), Value::Bool(false))
            } else {
                struct AudioCand<'a> {
                    instant: chrono::DateTime<chrono::Utc>,
                    candidate_index: usize,
                    candidate: &'a SegmentCandidate,
                }

                let mut audio_candidates = Vec::new();
                for (idx, cand) in segs.iter().enumerate() {
                    if !cand.data_state.0.contains_key("audio") {
                        continue;
                    }
                    let Some(inst) = candidate_instant(input, cand) else {
                        continue;
                    };
                    audio_candidates.push(AudioCand {
                        instant: inst,
                        candidate_index: idx,
                        candidate: cand,
                    });
                }

                if audio_candidates.is_empty() {
                    (serde_json::json!({"state": "none"}), Value::Null)
                } else {
                    audio_candidates.sort_by(|a, b| {
                        b.instant
                            .cmp(&a.instant)
                            .then_with(|| a.candidate_index.cmp(&b.candidate_index))
                    });
                    audio_candidates.truncate(300);

                    let mut heard_proof = None;
                    for ac in &audio_candidates {
                        if ac.candidate.data_state.0.get("audio").map(String::as_str)
                            != Some("analyzed")
                        {
                            continue;
                        }
                        if let Some(words) =
                            (input.segment_words)(ac.candidate).filter(|w| !w.is_empty())
                        {
                            heard_proof = Some((ac, words));
                            break;
                        }
                    }

                    if let Some((ac, raw_words)) = heard_proof {
                        let start_at = ac.instant.to_rfc3339();
                        let href = transcripts_href(
                            &ac.candidate.day,
                            &ac.candidate.stream,
                            &ac.candidate.key,
                        );
                        let text = trim_proof_text(&raw_words);
                        let start_ms = ac.instant.timestamp_millis();
                        let fresh = input.now_ms - start_ms < 1_800_000;
                        (
                            serde_json::json!({
                                "state": "heard",
                                "start_at": start_at,
                                "href": href,
                                "text": text,
                                "fresh": fresh,
                            }),
                            Value::Bool(true),
                        )
                    } else {
                        let newest = &audio_candidates[0];
                        let audio_val = newest
                            .candidate
                            .data_state
                            .0
                            .get("audio")
                            .map(String::as_str);
                        match audio_val {
                            Some("pending") | Some("analyzing") => (
                                serde_json::json!({
                                    "state": "transcribing",
                                    "start_at": newest.instant.to_rfc3339(),
                                }),
                                Value::Bool(false),
                            ),
                            Some("failed") | Some("failed_final") => (
                                serde_json::json!({
                                    "state": "failed",
                                    "start_at": newest.instant.to_rfc3339(),
                                    "href": "/app/health/",
                                }),
                                Value::Bool(false),
                            ),
                            Some("empty") => (
                                serde_json::json!({
                                    "state": "nothing_heard",
                                    "start_at": newest.instant.to_rfc3339(),
                                }),
                                Value::Bool(false),
                            ),
                            _ => (serde_json::json!({"state": "none"}), Value::Null),
                        }
                    }
                }
            }
        } else {
            (serde_json::json!({"state": "none"}), Value::Null)
        }
    };

    serde_json::json!({
        "window_open": true,
        "devices_known": devices_known,
        "devices": devices_json,
        "first_segment": first_segment,
        "proof": proof,
        "checks": {
            "phone": checks_phone,
            "thinking": check_thinking,
            "heard": checks_heard,
        },
        "thinking": {
            "state": thinking_state,
            "lane": input.ui_lane,
            "step": thinking_step,
        }
    })
}

/// Trusted host reader for the same setup evidence used by `/init/api/state`.
pub type SetupProgressReader =
    Arc<dyn Fn(&Path) -> Result<SetupProgress, SetupProgressError> + Send + Sync>;

/// The setup facts the card shows: download progress and local capability from
/// the setup reader, the saved thinking choice from the journal config, and
/// whether this browser may take setup actions. A setup reader that fails
/// leaves the progress facts unknown and keeps the saved choice.
fn project_setup(
    progress: Result<SetupProgress, SetupProgressError>,
    saved_choice: Option<&str>,
    actions: bool,
) -> Value {
    let (downloads, capability) = match progress {
        Ok(progress) => (
            serde_json::to_value(progress.downloads).expect("setup downloads serialize"),
            Value::String(progress.thinking.capability),
        ),
        Err(error) => {
            log::warn!("getting started could not read setup progress: {error}");
            (Value::Null, Value::Null)
        }
    };
    let choice = saved_choice.filter(|choice| matches!(*choice, "local" | "byo" | "spp" | "later"));
    serde_json::json!({
        "downloads": downloads,
        "thinking": {
            "capability": capability,
            "choice": choice,
        },
        "actions": actions,
    })
}

/// `setup_actions` says whether the requesting browser may take setup actions;
/// they answer only on the journal's own computer.
pub fn read_first_minutes(context: &HomeContext, setup_actions: bool) -> Value {
    read_first_minutes_with(
        context,
        setup_actions,
        solstone_core_sol_link::http::read_setup_progress,
    )
}

/// Read first minutes with the embedding host's setup authority.
pub fn read_first_minutes_with(
    context: &HomeContext,
    setup_actions: bool,
    read_progress: impl FnOnce(&Path) -> Result<SetupProgress, SetupProgressError>,
) -> Value {
    let config_path = context.journal_root().join("config/journal.json");
    let Ok(bytes) = std::fs::read(&config_path) else {
        return serde_json::json!({"window_open": false});
    };
    let Ok(value): Result<Value, _> = serde_json::from_slice(&bytes) else {
        return serde_json::json!({"window_open": false});
    };
    let completed_at = value
        .get("setup")
        .and_then(|s| s.get("completed_at"))
        .and_then(|v| {
            v.as_i64()
                .or_else(|| v.as_u64().and_then(|u| i64::try_from(u).ok()))
        });
    let Some(completed_at) = completed_at else {
        return serde_json::json!({"window_open": false});
    };
    let elapsed = context.now_ms() - completed_at;
    if elapsed >= 604_800_000 {
        return serde_json::json!({"window_open": false});
    }

    let capture = crate::readers::get_capture_health(context);

    let reported_platforms =
        match solstone_core_sol_link::client_description_store::read_descriptions(
            context.journal_root(),
        ) {
            Ok(descriptions) => {
                let map: BTreeMap<String, Option<String>> = descriptions
                    .into_iter()
                    .map(|(cid, desc)| (cid, desc.reported.and_then(|r| r.platform)))
                    .collect();
                Some(map)
            }
            Err(_) => None,
        };

    let ledger_path = context.journal_root().join("link/authorized_clients.json");
    let ledger_platforms: BTreeMap<String, String> =
        match solstone_core_sol_link::ledger::read_authorized_clients(&ledger_path) {
            solstone_core_sol_link::ledger::AuthorizedClientsRead::Present(clients) => clients
                .into_iter()
                .filter_map(|e| e.platform.map(|p| (e.fingerprint, p.as_wire().to_string())))
                .collect(),
            _ => BTreeMap::new(),
        };

    let bound_streams = match solstone_core_segment::list_stream_records(context.journal_root()) {
        Ok(records) => {
            let streams: Vec<BoundStream> = records
                .into_iter()
                .map(|(name, record)| {
                    let platform_allocation = record
                        .allocation
                        .as_ref()
                        .filter(|a| a.base == solstone_core_segment::StreamAllocationBase::Platform)
                        .map(|a| a.base_input.clone());
                    BoundStream {
                        name,
                        cid: record.cid,
                        platform_allocation,
                    }
                })
                .collect();
            Some(streams)
        }
        Err(_) => None,
    };

    let mut paired_cids = BTreeSet::new();
    if let Some(clients) = capture.get("clients").and_then(Value::as_array) {
        for c in clients {
            if let Some(cid) = c.get("cid").and_then(Value::as_str) {
                paired_cids.insert(cid.to_string());
            }
        }
    }
    if let Some(unassessed) = capture.get("unassessed").and_then(Value::as_array) {
        for c in unassessed {
            if let Some(cid) = c.get("cid").and_then(Value::as_str) {
                paired_cids.insert(cid.to_string());
            }
        }
    }

    let bound_stream_names: BTreeSet<String> = match &bound_streams {
        Some(streams) => streams
            .iter()
            .filter(|s| s.cid.as_ref().is_some_and(|cid| paired_cids.contains(cid)))
            .map(|s| s.name.clone())
            .collect(),
        None => BTreeSet::new(),
    };

    let days_result = solstone_core_segment::list_days(context.journal_root());
    let segments = match days_result {
        Ok(days) => {
            let mut candidates = Vec::new();
            let mut walk_ok = true;
            for (day, _) in days {
                match solstone_core_system_health::scan_day(
                    &solstone_core_system_health::FilesystemSegmentSource,
                    context.journal_root(),
                    &day,
                    context.now_utc,
                ) {
                    Ok((_, _, segs)) => {
                        for seg in segs {
                            if bound_stream_names.contains(&seg.stream) {
                                let (reported_tz, reported_offset_seconds) =
                                    if let Some(segment_dir) =
                                        solstone_core_system_health::find_segment_dir(
                                            context.journal_root(),
                                            &day,
                                            &seg.key,
                                            Some(&seg.stream),
                                        )
                                    {
                                        if let Some(zone) =
                                            solstone_core_callosum::read_reported_zone(
                                                context.journal_root(),
                                                &segment_dir,
                                            )
                                        {
                                            (zone.tz, zone.utc_offset_seconds)
                                        } else {
                                            (None, None)
                                        }
                                    } else {
                                        (None, None)
                                    };

                                candidates.push(SegmentCandidate {
                                    day: day.clone(),
                                    key: seg.key,
                                    stream: seg.stream,
                                    data_state: seg.data_state,
                                    reported_tz,
                                    reported_offset_seconds,
                                });
                            }
                        }
                    }
                    Err(_) => {
                        walk_ok = false;
                        break;
                    }
                }
            }
            if walk_ok { Some(candidates) } else { None }
        }
        Err(_) => None,
    };

    let brain_snapshot = crate::readers::build_brain_snapshot(context);
    let brain_state = brain_snapshot["state"]
        .as_str()
        .unwrap_or("unknown")
        .to_string();
    let brain_reason = brain_snapshot["reason_code"]
        .as_str()
        .map(|s| s.to_string());
    let brain_progressing = brain_snapshot["progressing"].as_bool().unwrap_or(false);

    let ui_lane = match solstone_core_thinking::read_config(context.journal_root()) {
        Ok(config) => {
            let lane = solstone_core_thinking::providers::ui_lane(&config);
            if lane == "none" {
                None
            } else {
                Some(lane.to_string())
            }
        }
        Err(_) => None,
    };

    let journal_root = context.journal_root().to_path_buf();
    let segment_words: SegmentWordsReader = Box::new(move |cand| {
        let dir = solstone_core_system_health::find_segment_dir(
            &journal_root,
            &cand.day,
            &cand.key,
            Some(&cand.stream),
        )?;
        read_segment_words(&dir)
    });

    let mut input = FirstMinutesInput {
        now_ms: context.now_ms(),
        owner_zone: context.zone(),
        capture,
        reported_platforms,
        ledger_platforms,
        bound_streams,
        segments,
        earliest_progress: None,
        brain_state,
        brain_reason,
        brain_progressing,
        ui_lane,
        segment_words,
    };

    let winner = winning_segment(&input);
    if let Some(w) = &winner {
        let log_source =
            solstone_core_system_health::FilesystemHealthLogSource::new(context.journal_root());
        let progress = solstone_core_system_health::read_segment_progress(&log_source, &w.day)
            .ok()
            .map(|fold| ProgressView {
                progress: fold.value,
            });
        input.earliest_progress = progress;
    }

    let saved_choice = value
        .get("setup")
        .and_then(|setup| setup.get("thinking_choice"))
        .and_then(Value::as_str);
    let mut out = project_first_minutes(&input);
    out["setup"] = project_setup(
        read_progress(context.journal_root()),
        saved_choice,
        setup_actions,
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono_tz::Tz;
    use serde_json::json;
    use solstone_core_system_health::DataStateMap;

    fn no_segment_words() -> SegmentWordsReader {
        Box::new(|_| None)
    }

    fn no_setup_progress(_: &Path) -> Result<SetupProgress, SetupProgressError> {
        Err(SetupProgressError {
            code: "status_unreadable",
            detail: "not read in this test".into(),
        })
    }

    fn fixed_setup_progress(_: &Path) -> Result<SetupProgress, SetupProgressError> {
        use solstone_core_sol_link::http::{
            SetupDownloadError, SetupDownloadTask, SetupDownloads, SetupProgressThinking,
        };
        Ok(SetupProgress {
            downloads: SetupDownloads {
                speech: SetupDownloadTask {
                    state: "error".into(),
                    attempt_id: Some("speech-1".into()),
                    bytes_done: 5,
                    bytes_total: Some(10),
                    error: Some(SetupDownloadError {
                        code: "install_failed".into(),
                        detail: "interrupted".into(),
                    }),
                },
                thinking: SetupDownloadTask {
                    state: "downloading".into(),
                    attempt_id: Some("local-1".into()),
                    bytes_done: 3,
                    bytes_total: Some(9),
                    error: None,
                },
            },
            thinking: SetupProgressThinking {
                capability: "blocked".into(),
            },
        })
    }

    #[test]
    fn setup_projection_carries_progress_choice_and_actions() {
        let out = project_setup(fixed_setup_progress(Path::new("/")), Some("byo"), true);
        assert_eq!(
            out,
            json!({
                "downloads": {
                    "speech": {
                        "state": "error",
                        "attempt_id": "speech-1",
                        "bytes_done": 5,
                        "bytes_total": 10,
                        "error": {"code": "install_failed", "detail": "interrupted"},
                    },
                    "thinking": {
                        "state": "downloading",
                        "attempt_id": "local-1",
                        "bytes_done": 3,
                        "bytes_total": 9,
                        "error": null,
                    },
                },
                "thinking": {"capability": "blocked", "choice": "byo"},
                "actions": true,
            })
        );

        // A failed read leaves progress unknown and still continues the saved choice.
        let failed = project_setup(no_setup_progress(Path::new("/")), Some("spp"), false);
        assert_eq!(
            failed,
            json!({
                "downloads": null,
                "thinking": {"capability": null, "choice": "spp"},
                "actions": false,
            })
        );

        // Only the choices setup saves are continued.
        for (saved, expected) in [
            (Some("local"), json!("local")),
            (Some("later"), json!("later")),
            (Some("cloud"), Value::Null),
            (Some(""), Value::Null),
            (None, Value::Null),
        ] {
            let out = project_setup(no_setup_progress(Path::new("/")), saved, false);
            assert_eq!(out["thinking"]["choice"], expected, "{saved:?}");
        }
    }

    #[test]
    fn reader_projects_setup_only_while_the_window_is_open() {
        let temp = tempfile::tempdir_in("/var/tmp").unwrap();
        let journal = temp.path();
        let completed_at = 1_000_000_000_000i64;
        std::fs::create_dir_all(journal.join("config")).unwrap();
        std::fs::write(
            journal.join("config/journal.json"),
            json!({
                "identity": {"timezone": "UTC"},
                "setup": {"completed_at": completed_at, "thinking_choice": "spp"}
            })
            .to_string(),
        )
        .unwrap();
        let open = HomeContext::with_zone(
            journal,
            chrono::DateTime::from_timestamp_millis(completed_at + 86_400_000).unwrap(),
            Tz::UTC,
        );
        let out = read_first_minutes_with(&open, false, fixed_setup_progress);
        assert_eq!(out["window_open"], true);
        assert_eq!(out["setup"]["thinking"]["choice"], "spp");
        assert_eq!(out["setup"]["thinking"]["capability"], "blocked");
        assert_eq!(
            out["setup"]["downloads"]["thinking"]["state"],
            "downloading"
        );
        assert_eq!(out["setup"]["actions"], false);
        let local = read_first_minutes_with(&open, true, fixed_setup_progress);
        assert_eq!(local["setup"]["actions"], true);

        let closed = HomeContext::with_zone(
            journal,
            chrono::DateTime::from_timestamp_millis(completed_at + 604_800_000).unwrap(),
            Tz::UTC,
        );
        assert_eq!(
            read_first_minutes_with(&closed, true, fixed_setup_progress),
            json!({"window_open": false})
        );
    }

    #[test]
    fn devices_unknown_when_registry_unknown() {
        let input = FirstMinutesInput {
            now_ms: 1_000_000,
            owner_zone: Tz::UTC,
            capture: json!({
                "registry": "registry_unknown",
                "clients": [],
                "unassessed": [],
            }),
            reported_platforms: Some(BTreeMap::new()),
            ledger_platforms: BTreeMap::new(),
            bound_streams: Some(vec![]),
            segments: Some(vec![]),
            earliest_progress: None,
            brain_state: "ready".into(),
            brain_reason: None,
            brain_progressing: false,
            ui_lane: None,
            segment_words: no_segment_words(),
        };
        let out = project_first_minutes(&input);
        assert_eq!(out["window_open"], true);
        assert_eq!(out["devices_known"], false);
        assert_eq!(out["devices"], json!([]));
        assert_eq!(out["checks"]["phone"], Value::Null);
    }

    #[test]
    fn devices_state_order_labels_and_platforms() {
        let mut reported = BTreeMap::new();
        reported.insert("cid_a".into(), Some("ios".into()));
        reported.insert("cid_b".into(), None);

        let mut ledger = BTreeMap::new();
        ledger.insert("cid_b".into(), "android".into());

        let bound = vec![BoundStream {
            name: "stream_c".into(),
            cid: Some("cid_c".into()),
            platform_allocation: Some("macos".into()),
        }];

        let capture = json!({
            "registry": "registry_complete",
            "clients": [
                {
                    "name": "Device A",
                    "cid": "cid_a",
                    "status": "active",
                    "last_accepted_ingest_at": "2026-09-29T12:00:00Z"
                },
                {
                    "name": "Device B",
                    "cid": "cid_b",
                    "status": "stale",
                    "last_accepted_ingest_at": "2026-09-29T12:05:00Z"
                },
                {
                    "name": "Device C",
                    "cid": "cid_c",
                    "status": "degraded"
                }
            ],
            "unassessed": [
                {
                    "name": "Device D",
                    "cid": "cid_d",
                    "reason": "awaiting_first_delivery"
                }
            ]
        });

        // 2026-09-29T12:10:00Z in ms is 1790683800000
        let now_dt = chrono::DateTime::parse_from_rfc3339("2026-09-29T12:10:00Z").unwrap();
        let now_ms = now_dt.timestamp_millis();

        let input = FirstMinutesInput {
            now_ms,
            owner_zone: Tz::UTC,
            capture,
            reported_platforms: Some(reported),
            ledger_platforms: ledger,
            bound_streams: Some(bound),
            segments: Some(vec![]),
            earliest_progress: None,
            brain_state: "ready".into(),
            brain_reason: None,
            brain_progressing: false,
            ui_lane: None,
            segment_words: no_segment_words(),
        };

        let out = project_first_minutes(&input);
        assert_eq!(out["devices_known"], true);
        assert_eq!(out["checks"]["phone"], true);

        let devs = out["devices"].as_array().unwrap();
        assert_eq!(devs.len(), 4);
        // B (12:05) comes before A (12:00) because 12:05 > 12:00
        assert_eq!(devs[0]["name"], "Device B");
        assert_eq!(devs[0]["platform"], "android");
        assert_eq!(devs[0]["state"], "quiet");
        assert_eq!(devs[0]["last_added_label"], "5 minutes ago");

        assert_eq!(devs[1]["name"], "Device A");
        assert_eq!(devs[1]["platform"], "ios");
        assert_eq!(devs[1]["state"], "receiving");
        assert_eq!(devs[1]["last_added_label"], "10 minutes ago");

        // C has no time, D has no time; clients order first (C), then unassessed (D)
        assert_eq!(devs[2]["name"], "Device C");
        assert_eq!(devs[2]["platform"], "macos");
        assert_eq!(devs[2]["state"], "other");
        assert!(devs[2].get("last_added_label").is_none());

        assert_eq!(devs[3]["name"], "Device D");
        assert_eq!(devs[3]["state"], "awaiting_first");
        assert!(devs[3].get("last_added_label").is_none());
    }

    #[test]
    fn winning_segment_choice_and_thinking_verdicts() {
        let mut data_state_map = BTreeMap::new();
        data_state_map.insert("audio".into(), "analyzed".into());
        let sensed_state = DataStateMap(data_state_map);

        let candidate_early = SegmentCandidate {
            day: "20260929".into(),
            key: "090000_300".into(),
            stream: "stream/one".into(),
            data_state: sensed_state.clone(),
            reported_tz: Some("UTC".into()),
            reported_offset_seconds: Some(0),
        };
        let candidate_late = SegmentCandidate {
            day: "20260929".into(),
            key: "100000_300".into(),
            stream: "stream/two".into(),
            data_state: sensed_state.clone(),
            reported_tz: Some("UTC".into()),
            reported_offset_seconds: Some(0),
        };

        let mut progress_map = BTreeMap::new();
        let ident = solstone_core_system_health::SegmentIdentity {
            stream: Some("stream/one".into()),
            segment: "090000_300".into(),
        };
        let progress = solstone_core_system_health::SegmentProgress {
            sensed: true,
            density: Some("idle".into()),
            ..Default::default()
        };
        progress_map.insert(ident.clone(), progress);

        let mut input = FirstMinutesInput {
            now_ms: 1_790_683_800_000,
            owner_zone: Tz::UTC,
            capture: json!({"registry": "registry_complete", "clients": [], "unassessed": []}),
            reported_platforms: Some(BTreeMap::new()),
            ledger_platforms: BTreeMap::new(),
            bound_streams: Some(vec![]),
            segments: Some(vec![candidate_late.clone(), candidate_early.clone()]),
            earliest_progress: Some(ProgressView {
                progress: progress_map.clone(),
            }),
            brain_state: "ready".into(),
            brain_reason: None,
            brain_progressing: false,
            ui_lane: None,
            segment_words: no_segment_words(),
        };

        let out = project_first_minutes(&input);
        assert_eq!(
            out["first_segment"]["start_at"],
            "2026-09-29T09:00:00+00:00"
        );
        assert_eq!(
            out["first_segment"]["href"],
            "/app/transcripts/20260929?stream=stream%2Fone#090000_300"
        );
        assert_eq!(out["first_segment"]["thinking"], "read");

        // When not fully thought, and brain is ready -> pending
        let pending_progress = solstone_core_system_health::SegmentProgress {
            sensed: true,
            ..Default::default()
        };
        let mut pending_map = BTreeMap::new();
        pending_map.insert(ident, pending_progress);
        input.earliest_progress = Some(ProgressView {
            progress: pending_map,
        });
        let out_pending = project_first_minutes(&input);
        assert_eq!(out_pending["first_segment"]["thinking"], "pending");

        // When not fully thought and brain is other -> none
        input.brain_state = "blocked".into();
        input.brain_reason = Some("thinking_engine_not_chosen".into());
        let out_none = project_first_minutes(&input);
        assert_eq!(out_none["first_segment"]["thinking"], "none");
    }

    #[test]
    fn thinking_state_and_step_mappings() {
        // 1. ready
        let mut input = FirstMinutesInput {
            now_ms: 1_000_000,
            owner_zone: Tz::UTC,
            capture: json!({"registry": "registry_unknown"}),
            reported_platforms: None,
            ledger_platforms: BTreeMap::new(),
            bound_streams: None,
            segments: None,
            earliest_progress: None,
            brain_state: "ready".into(),
            brain_reason: None,
            brain_progressing: false,
            ui_lane: None,
            segment_words: no_segment_words(),
        };
        let out = project_first_minutes(&input);
        assert_eq!(out["thinking"]["state"], "ready");
        assert_eq!(out["thinking"]["step"], "none");
        assert_eq!(out["checks"]["thinking"], true);

        // 2. not chosen
        input.brain_state = "blocked".into();
        input.brain_reason = Some("thinking_engine_not_chosen".into());
        let out = project_first_minutes(&input);
        assert_eq!(out["thinking"]["state"], "not_chosen");
        assert_eq!(out["thinking"]["step"], "choose");
        assert_eq!(out["checks"]["thinking"], false);

        // 3. cant run local
        input.brain_reason = Some("gpu_unavailable".into());
        let out = project_first_minutes(&input);
        assert_eq!(out["thinking"]["state"], "cant_run_local");
        assert_eq!(out["thinking"]["step"], "cant_run");
        assert_eq!(out["checks"]["thinking"], false);

        // 4. needs step: local
        input.brain_reason = Some("local_artifact_not_ready".into());
        let out = project_first_minutes(&input);
        assert_eq!(out["thinking"]["state"], "needs_step");
        assert_eq!(out["thinking"]["step"], "local");
        assert_eq!(out["checks"]["thinking"], false);

        // 5. needs step: confidential
        input.brain_reason = Some("provider_key_missing".into());
        input.ui_lane = Some("confidential".into());
        let out = project_first_minutes(&input);
        assert_eq!(out["thinking"]["state"], "needs_step");
        assert_eq!(out["thinking"]["step"], "confidential");
        assert_eq!(out["checks"]["thinking"], false);

        // 6. needs step: byo
        input.ui_lane = Some("byo".into());
        let out = project_first_minutes(&input);
        assert_eq!(out["thinking"]["state"], "needs_step");
        assert_eq!(out["thinking"]["step"], "byo");
        assert_eq!(out["checks"]["thinking"], false);

        // 7. fallback snapshot
        input.brain_state = "unknown".into();
        input.brain_reason = Some("brain_record_unavailable".into());
        input.brain_progressing = false;
        input.ui_lane = None;
        let out = project_first_minutes(&input);
        assert_eq!(out["thinking"]["state"], "other");
        assert_eq!(out["thinking"]["step"], "none");
        assert_eq!(out["checks"]["thinking"], true);
    }

    #[test]
    fn reader_closed_window_proof() {
        let temp = tempfile::tempdir().unwrap();
        let journal = temp.path();

        // 1. config/journal.json: {"identity":{"timezone":"UTC"},"setup":{"completed_at":1000000000000}}
        let completed_at = 1_000_000_000_000i64;
        std::fs::create_dir_all(journal.join("config")).unwrap();
        std::fs::write(
            journal.join("config/journal.json"),
            json!({
                "identity": {"timezone": "UTC"},
                "setup": {"completed_at": completed_at}
            })
            .to_string(),
        )
        .unwrap();

        // 2. link/authorized_clients.json
        std::fs::create_dir_all(journal.join("link")).unwrap();
        let ledger = json!([{
            "fingerprint": "sha256:phone",
            "device_label": "Pocket",
            "paired_at": "2026-10-10T00:00:00Z",
            "instance_id": "inst",
            "role": "",
            "kind": "cert",
            "platform": "ios"
        }]);
        std::fs::write(
            journal.join("link/authorized_clients.json"),
            ledger.to_string(),
        )
        .unwrap();

        // 3. streams/phone.json
        std::fs::create_dir_all(journal.join("streams")).unwrap();
        let stream = json!({
            "name": "phone",
            "kind": "observer",
            "host": null,
            "platform": null,
            "created_at": 1000000000000u64,
            "last_day": null,
            "last_segment": null,
            "seq": 0,
            "cid": "sha256:phone",
            "source": ""
        });
        std::fs::write(journal.join("streams/phone.json"), stream.to_string()).unwrap();

        // 4. chronicle/20261010/phone/120000_300/audio.jsonl
        let seg_dir = journal.join("chronicle/20261010/phone/120000_300");
        std::fs::create_dir_all(&seg_dir).unwrap();
        std::fs::write(
            seg_dir.join("audio.jsonl"),
            "{\"start\":\"12:00:00\",\"text\":\"hello\"}\n",
        )
        .unwrap();

        // Closed read: elapsed >= 604_800_000
        let now_closed_exact = completed_at + 604_800_000;
        let ctx_closed_exact = HomeContext::new(
            journal,
            chrono::DateTime::from_timestamp_millis(now_closed_exact).unwrap(),
        );
        assert_eq!(
            read_first_minutes_with(&ctx_closed_exact, false, no_setup_progress),
            json!({"window_open": false})
        );

        let now_closed_more = completed_at + 700_000_000;
        let ctx_closed_more = HomeContext::new(
            journal,
            chrono::DateTime::from_timestamp_millis(now_closed_more).unwrap(),
        );
        assert_eq!(
            read_first_minutes_with(&ctx_closed_more, false, no_setup_progress),
            json!({"window_open": false})
        );

        // Missing completed_at on copy of journal
        let temp_missing = tempfile::tempdir().unwrap();
        let journal_missing = temp_missing.path();
        std::fs::create_dir_all(journal_missing.join("config")).unwrap();
        std::fs::write(
            journal_missing.join("config/journal.json"),
            json!({"identity": {"timezone": "UTC"}, "setup": {}}).to_string(),
        )
        .unwrap();
        let ctx_missing = HomeContext::new(
            journal_missing,
            chrono::DateTime::from_timestamp_millis(completed_at + 86_400_000).unwrap(),
        );
        assert_eq!(
            read_first_minutes_with(&ctx_missing, false, no_setup_progress),
            json!({"window_open": false})
        );

        // Non-numeric completed_at on copy of journal
        let temp_nonnum = tempfile::tempdir().unwrap();
        let journal_nonnum = temp_nonnum.path();
        std::fs::create_dir_all(journal_nonnum.join("config")).unwrap();
        std::fs::write(
            journal_nonnum.join("config/journal.json"),
            json!({"identity": {"timezone": "UTC"}, "setup": {"completed_at": "yesterday"}})
                .to_string(),
        )
        .unwrap();
        let ctx_nonnum = HomeContext::new(
            journal_nonnum,
            chrono::DateTime::from_timestamp_millis(completed_at + 86_400_000).unwrap(),
        );
        assert_eq!(
            read_first_minutes_with(&ctx_nonnum, false, no_setup_progress),
            json!({"window_open": false})
        );

        // Open read: completed_at + 86_400_000
        let now_open_ms = completed_at + 86_400_000;
        let ctx_open = HomeContext::new(
            journal,
            chrono::DateTime::from_timestamp_millis(now_open_ms).unwrap(),
        );
        let out_open = read_first_minutes_with(&ctx_open, false, no_setup_progress);
        assert_eq!(out_open["window_open"], true);
        assert_eq!(out_open["devices"][0]["name"], "Pocket");
        assert_eq!(
            out_open["first_segment"]["href"],
            "/app/transcripts/20261010?stream=phone#120000_300"
        );
        if ctx_open.zone() == Tz::UTC {
            assert_eq!(
                out_open["first_segment"]["start_at"],
                "2026-10-10T12:00:00+00:00"
            );
        }
    }

    #[test]
    fn unassessed_activity_unavailable_and_registry_complete() {
        let input = FirstMinutesInput {
            now_ms: 1_000_000,
            owner_zone: Tz::UTC,
            capture: json!({
                "status": "unknown",
                "registry": "registry_complete",
                "clients": [],
                "unassessed": [{
                    "name": "Dev",
                    "cid": "c1",
                    "reason": "activity_unavailable",
                }]
            }),
            reported_platforms: None,
            ledger_platforms: BTreeMap::new(),
            bound_streams: None,
            segments: None,
            earliest_progress: None,
            brain_state: "ready".into(),
            brain_reason: None,
            brain_progressing: false,
            ui_lane: None,
            segment_words: no_segment_words(),
        };
        let out = project_first_minutes(&input);
        assert_eq!(out["devices_known"], true);
        let devs = out["devices"].as_array().unwrap();
        assert_eq!(devs.len(), 1);
        assert_eq!(devs[0]["name"], "Dev");
        assert_eq!(devs[0]["state"], "other");
    }

    #[test]
    fn client_platform_resolution_and_phone_check() {
        // reported platform "iOS" on awaiting_first client with no bound stream
        let mut reported = BTreeMap::new();
        reported.insert("c_ios".into(), Some("iOS".into()));
        let input = FirstMinutesInput {
            now_ms: 1_000_000,
            owner_zone: Tz::UTC,
            capture: json!({
                "registry": "registry_complete",
                "clients": [{
                    "name": "Phone",
                    "cid": "c_ios",
                    "status": "awaiting_first_delivery",
                }],
                "unassessed": []
            }),
            reported_platforms: Some(reported),
            ledger_platforms: BTreeMap::new(),
            bound_streams: None,
            segments: None,
            earliest_progress: None,
            brain_state: "ready".into(),
            brain_reason: None,
            brain_progressing: false,
            ui_lane: None,
            segment_words: no_segment_words(),
        };
        let out = project_first_minutes(&input);
        assert_eq!(out["checks"]["phone"], true);
        assert_eq!(out["devices"][0]["platform"], "ios");
        assert_eq!(out["devices"][0]["state"], "awaiting_first");

        // ledger-only "ios" with no stream does the same
        let mut ledger = BTreeMap::new();
        ledger.insert("c_ledger".into(), "ios".into());
        let input_ledger = FirstMinutesInput {
            now_ms: 1_000_000,
            owner_zone: Tz::UTC,
            capture: json!({
                "registry": "registry_complete",
                "clients": [{
                    "name": "Phone2",
                    "cid": "c_ledger",
                    "status": "awaiting_first_delivery",
                }],
                "unassessed": []
            }),
            reported_platforms: Some(BTreeMap::new()),
            ledger_platforms: ledger,
            bound_streams: None,
            segments: None,
            earliest_progress: None,
            brain_state: "ready".into(),
            brain_reason: None,
            brain_progressing: false,
            ui_lane: None,
            segment_words: no_segment_words(),
        };
        let out_ledger = project_first_minutes(&input_ledger);
        assert_eq!(out_ledger["checks"]["phone"], true);
        assert_eq!(out_ledger["devices"][0]["platform"], "ios");
        assert_eq!(out_ledger["devices"][0]["state"], "awaiting_first");

        // Desktops (macos) and device with no platform -> checks.phone: false
        let mut ledger_desktop = BTreeMap::new();
        ledger_desktop.insert("c_mac".into(), "macos".into());
        let input_desktop = FirstMinutesInput {
            now_ms: 1_000_000,
            owner_zone: Tz::UTC,
            capture: json!({
                "registry": "registry_complete",
                "clients": [
                    {
                        "name": "Mac",
                        "cid": "c_mac",
                        "status": "active",
                    },
                    {
                        "name": "NoPlatform",
                        "cid": "c_none",
                        "status": "active",
                    }
                ],
                "unassessed": []
            }),
            reported_platforms: Some(BTreeMap::new()),
            ledger_platforms: ledger_desktop,
            bound_streams: None,
            segments: None,
            earliest_progress: None,
            brain_state: "ready".into(),
            brain_reason: None,
            brain_progressing: false,
            ui_lane: None,
            segment_words: no_segment_words(),
        };
        let out_desktop = project_first_minutes(&input_desktop);
        assert_eq!(out_desktop["checks"]["phone"], false);
    }

    #[test]
    fn winning_segment_timezone_instant_comparison() {
        let mut data_state_map = BTreeMap::new();
        data_state_map.insert("audio".into(), "analyzed".into());
        let sensed_state = DataStateMap(data_state_map);

        // Candidate 1: wall clock 10:00:00, offset +05:00 -> UTC 05:00:00
        let candidate_offset = SegmentCandidate {
            day: "20261010".into(),
            key: "100000_300".into(),
            stream: "phone".into(),
            data_state: sensed_state.clone(),
            reported_tz: None,
            reported_offset_seconds: Some(5 * 3600),
        };
        // Candidate 2: wall clock 08:00:00, no reported zone (uses owner_zone UTC) -> UTC 08:00:00
        let candidate_owner = SegmentCandidate {
            day: "20261010".into(),
            key: "080000_300".into(),
            stream: "desk".into(),
            data_state: sensed_state.clone(),
            reported_tz: None,
            reported_offset_seconds: None,
        };

        let input = FirstMinutesInput {
            now_ms: 1_000_000,
            owner_zone: Tz::UTC,
            capture: json!({"registry": "registry_complete"}),
            reported_platforms: None,
            ledger_platforms: BTreeMap::new(),
            bound_streams: None,
            segments: Some(vec![candidate_owner.clone(), candidate_offset.clone()]),
            earliest_progress: None,
            brain_state: "ready".into(),
            brain_reason: None,
            brain_progressing: false,
            ui_lane: None,
            segment_words: no_segment_words(),
        };
        let winner = winning_segment(&input).expect("winner");
        // Candidate offset wins because UTC 05:00:00 < 08:00:00
        assert_eq!(winner.stream, "phone");
        assert_eq!(winner.key, "100000_300");
    }

    #[test]
    fn segment_processing_and_sensed_verdicts() {
        let mut data_state_map = BTreeMap::new();
        data_state_map.insert("audio".into(), "analyzed".into());
        let sensed_state = DataStateMap(data_state_map);

        let candidate = SegmentCandidate {
            day: "20261010".into(),
            key: "120000_300".into(),
            stream: "phone".into(),
            data_state: sensed_state.clone(),
            reported_tz: Some("UTC".into()),
            reported_offset_seconds: Some(0),
        };

        // earliest_progress: None with fully sensed segment -> returns segment, thinking: "none"
        let input_no_progress = FirstMinutesInput {
            now_ms: 1_000_000,
            owner_zone: Tz::UTC,
            capture: json!({"registry": "registry_complete"}),
            reported_platforms: None,
            ledger_platforms: BTreeMap::new(),
            bound_streams: None,
            segments: Some(vec![candidate.clone()]),
            earliest_progress: None,
            brain_state: "ready".into(),
            brain_reason: None,
            brain_progressing: false,
            ui_lane: None,
            segment_words: no_segment_words(),
        };
        let out_no_progress = project_first_minutes(&input_no_progress);
        assert_eq!(out_no_progress["first_segment"]["thinking"], "none");
        assert_eq!(
            out_no_progress["first_segment"]["href"],
            "/app/transcripts/20261010?stream=phone#120000_300"
        );

        // Not fully sensed segment -> thinking: "none"
        let mut empty_state = BTreeMap::new();
        empty_state.insert("audio".into(), "empty".into());
        let not_sensed_candidate = SegmentCandidate {
            day: "20261010".into(),
            key: "120000_300".into(),
            stream: "phone".into(),
            data_state: DataStateMap(empty_state),
            reported_tz: Some("UTC".into()),
            reported_offset_seconds: Some(0),
        };
        let input_not_sensed = FirstMinutesInput {
            now_ms: 1_000_000,
            owner_zone: Tz::UTC,
            capture: json!({"registry": "registry_complete"}),
            reported_platforms: None,
            ledger_platforms: BTreeMap::new(),
            bound_streams: None,
            segments: Some(vec![not_sensed_candidate]),
            earliest_progress: None,
            brain_state: "ready".into(),
            brain_reason: None,
            brain_progressing: false,
            ui_lane: None,
            segment_words: no_segment_words(),
        };
        let out_not_sensed = project_first_minutes(&input_not_sensed);
        assert_eq!(out_not_sensed["first_segment"]["thinking"], "none");
    }

    #[test]
    fn additional_brain_state_and_step_mappings() {
        // brain unhealthy -> state: other, step: none, checks.thinking: true
        let mut input = FirstMinutesInput {
            now_ms: 1_000_000,
            owner_zone: Tz::UTC,
            capture: json!({"registry": "registry_complete"}),
            reported_platforms: None,
            ledger_platforms: BTreeMap::new(),
            bound_streams: None,
            segments: None,
            earliest_progress: None,
            brain_state: "unhealthy".into(),
            brain_reason: None,
            brain_progressing: false,
            ui_lane: None,
            segment_words: no_segment_words(),
        };
        let out = project_first_minutes(&input);
        assert_eq!(out["thinking"]["state"], "other");
        assert_eq!(out["thinking"]["step"], "none");
        assert_eq!(out["checks"]["thinking"], true);

        // brain checking -> progressing / none
        input.brain_state = "checking".into();
        let out_checking = project_first_minutes(&input);
        assert_eq!(out_checking["thinking"]["state"], "progressing");
        assert_eq!(out_checking["thinking"]["step"], "none");
        assert_eq!(out_checking["checks"]["thinking"], true);

        // blocked + brain_progressing: true -> progressing / none even when reason is gpu_unavailable
        input.brain_state = "blocked".into();
        input.brain_reason = Some("gpu_unavailable".into());
        input.brain_progressing = true;
        let out_progressing = project_first_minutes(&input);
        assert_eq!(out_progressing["thinking"]["state"], "progressing");
        assert_eq!(out_progressing["thinking"]["step"], "none");
        assert_eq!(out_progressing["checks"]["thinking"], true);

        // endpoint_configuration_incomplete with ui_lane: null -> needs_step / byo
        input.brain_progressing = false;
        input.brain_reason = Some("endpoint_configuration_incomplete".into());
        input.ui_lane = None;
        let out_byo = project_first_minutes(&input);
        assert_eq!(out_byo["thinking"]["state"], "needs_step");
        assert_eq!(out_byo["thinking"]["step"], "byo");
        assert_eq!(out_byo["checks"]["thinking"], false);
    }

    #[test]
    fn proof_projection_states_and_rules() {
        let base_cand = SegmentCandidate {
            day: "20261010".into(),
            key: "110000_300".into(),
            stream: "stream1".into(),
            data_state: DataStateMap(BTreeMap::from([("audio".into(), "analyzed".into())])),
            reported_tz: Some("UTC".into()),
            reported_offset_seconds: Some(0),
        };

        let base_input = || FirstMinutesInput {
            now_ms: 1_791_639_600_000,
            owner_zone: Tz::UTC,
            capture: json!({
                "registry": "registry_complete",
                "clients": [{"cid": "c1", "name": "phone"}],
                "unassessed": []
            }),
            reported_platforms: None,
            ledger_platforms: BTreeMap::new(),
            bound_streams: Some(vec![BoundStream {
                name: "stream1".into(),
                cid: Some("c1".into()),
                platform_allocation: None,
            }]),
            segments: Some(vec![base_cand.clone()]),
            earliest_progress: None,
            brain_state: "ready".into(),
            brain_reason: None,
            brain_progressing: false,
            ui_lane: None,
            segment_words: no_segment_words(),
        };

        // 1. waiting
        let mut input = base_input();
        input.segments = Some(vec![]);
        let out = project_first_minutes(&input);
        assert_eq!(out["proof"], json!({"state": "waiting"}));
        assert_eq!(out["checks"]["heard"], false);

        // 2. no_audio
        let mut input = base_input();
        let mut ds = BTreeMap::new();
        ds.insert("screen".into(), "analyzed".into());
        let mut no_aud_cand = base_cand.clone();
        no_aud_cand.data_state = DataStateMap(ds);
        input.segments = Some(vec![no_aud_cand]);
        let out = project_first_minutes(&input);
        assert_eq!(out["proof"], json!({"state": "no_audio"}));
        assert_eq!(out["checks"]["heard"], false);

        // 3. transcribing: pending & analyzing
        for state in ["pending", "analyzing"] {
            let mut input = base_input();
            let mut cand = base_cand.clone();
            cand.data_state = DataStateMap(BTreeMap::from([("audio".into(), state.into())]));
            input.segments = Some(vec![cand]);
            let out = project_first_minutes(&input);
            assert_eq!(out["proof"]["state"], "transcribing");
            assert_eq!(out["checks"]["heard"], false);
            assert!(out["proof"]["start_at"].is_string());
            assert!(out["proof"]["href"].is_null());
        }

        // 4. failed: failed & failed_final
        for state in ["failed", "failed_final"] {
            let mut input = base_input();
            let mut cand = base_cand.clone();
            cand.data_state = DataStateMap(BTreeMap::from([("audio".into(), state.into())]));
            input.segments = Some(vec![cand]);
            let out = project_first_minutes(&input);
            assert_eq!(out["proof"]["state"], "failed");
            assert_eq!(out["checks"]["heard"], false);
            assert_eq!(out["proof"]["href"], "/app/health/");
            assert!(out["proof"]["start_at"].is_string());
        }

        // 5. nothing_heard: empty
        let mut input = base_input();
        let mut cand = base_cand.clone();
        cand.data_state = DataStateMap(BTreeMap::from([("audio".into(), "empty".into())]));
        input.segments = Some(vec![cand]);
        let out = project_first_minutes(&input);
        assert_eq!(out["proof"]["state"], "nothing_heard");
        assert_eq!(out["checks"]["heard"], false);
        assert!(out["proof"]["start_at"].is_string());
        assert!(out["proof"]["href"].is_null());

        // 6. none conditions:
        // devices_known false
        let mut input = base_input();
        input.capture = json!({"registry": "registry_unknown"});
        input.segment_words = Box::new(|_| Some("words".into()));
        let out = project_first_minutes(&input);
        assert_eq!(out["proof"]["state"], "none");
        assert_eq!(out["checks"]["heard"], Value::Null);

        // empty device list
        let mut input = base_input();
        input.capture = json!({"registry": "registry_complete", "clients": [], "unassessed": []});
        input.segment_words = Box::new(|_| Some("words".into()));
        let out = project_first_minutes(&input);
        assert_eq!(out["proof"]["state"], "none");
        assert_eq!(out["checks"]["heard"], Value::Null);

        // bound_streams: None
        let mut input = base_input();
        input.bound_streams = None;
        input.segment_words = Box::new(|_| Some("words".into()));
        let out = project_first_minutes(&input);
        assert_eq!(out["proof"]["state"], "none");
        assert_eq!(out["checks"]["heard"], Value::Null);

        // segments: None
        let mut input = base_input();
        input.segments = None;
        input.segment_words = Box::new(|_| Some("words".into()));
        let out = project_first_minutes(&input);
        assert_eq!(out["proof"]["state"], "none");
        assert_eq!(out["checks"]["heard"], Value::Null);

        // purged audio segment
        let mut input = base_input();
        let mut cand = base_cand.clone();
        cand.data_state = DataStateMap(BTreeMap::from([("audio".into(), "purged".into())]));
        input.segments = Some(vec![cand]);
        let out = project_first_minutes(&input);
        assert_eq!(out["proof"]["state"], "none");
        assert_eq!(out["checks"]["heard"], Value::Null);

        // analyzed segment with reader returning no words
        let mut input = base_input();
        input.segment_words = Box::new(|_| None);
        let out = project_first_minutes(&input);
        assert_eq!(out["proof"]["state"], "none");
        assert_eq!(out["checks"]["heard"], Value::Null);

        // 7. heard when newer is empty and older has words
        let mut newer = base_cand.clone();
        newer.key = "120000_300".into();
        newer.data_state = DataStateMap(BTreeMap::from([("audio".into(), "empty".into())]));

        let mut older = base_cand.clone();
        older.key = "110000_300".into();
        older.data_state = DataStateMap(BTreeMap::from([("audio".into(), "analyzed".into())]));

        let reader_called_keys = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let called_keys = reader_called_keys.clone();
        let mut input = base_input();
        input.segments = Some(vec![newer, older]);
        input.segment_words = Box::new(move |c| {
            called_keys.borrow_mut().push(c.key.clone());
            if c.key == "110000_300" {
                Some("hello there".into())
            } else {
                None
            }
        });
        let out = project_first_minutes(&input);
        assert_eq!(out["proof"]["state"], "heard");
        assert_eq!(out["checks"]["heard"], true);
        assert_eq!(out["proof"]["text"], "hello there");
        assert_eq!(out["proof"]["start_at"], "2026-10-10T11:00:00+00:00");
        assert_eq!(
            out["proof"]["href"],
            "/app/transcripts/20261010?stream=stream1#110000_300"
        );
        assert_eq!(*reader_called_keys.borrow(), vec!["110000_300".to_string()]);

        // 8. Stream name stream/one on heard segment
        let mut stream_cand = base_cand.clone();
        stream_cand.stream = "stream/one".into();
        let mut input = base_input();
        input.segments = Some(vec![stream_cand]);
        input.segment_words = Box::new(|_| Some("words".into()));
        let out = project_first_minutes(&input);
        assert_eq!(
            out["proof"]["href"],
            "/app/transcripts/20261010?stream=stream%2Fone#110000_300"
        );

        // 9. fresh boundary
        let start_ms =
            chrono::DateTime::parse_from_rfc3339(out["proof"]["start_at"].as_str().unwrap())
                .unwrap()
                .timestamp_millis();
        let mut input = base_input();
        input.segment_words = Box::new(|_| Some("words".into()));

        // true at start_ms + 1_799_999
        input.now_ms = start_ms + 1_799_999;
        assert_eq!(project_first_minutes(&input)["proof"]["fresh"], true);

        // false at start_ms + 1_800_000
        input.now_ms = start_ms + 1_800_000;
        assert_eq!(project_first_minutes(&input)["proof"]["fresh"], false);

        // true when now_ms is before start_ms
        input.now_ms = start_ms - 100_000;
        assert_eq!(project_first_minutes(&input)["proof"]["fresh"], true);
    }

    #[test]
    fn injected_reader_budget_and_order() {
        use std::cell::RefCell;
        use std::rc::Rc;

        let mut segments = Vec::new();
        let start_date = chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        for i in 0..301 {
            let day = (start_date + chrono::TimeDelta::days(i))
                .format("%Y%m%d")
                .to_string();
            segments.push(SegmentCandidate {
                day,
                key: "120000_300".into(),
                stream: "stream1".into(),
                data_state: DataStateMap(BTreeMap::from([("audio".into(), "analyzed".into())])),
                reported_tz: Some("UTC".into()),
                reported_offset_seconds: Some(0),
            });
        }

        // Case A: Words only on the oldest (Candidate 0, 301st-newest).
        let calls = Rc::new(RefCell::new(0usize));
        let calls_clone = calls.clone();
        let oldest_day = segments[0].day.clone();
        let oldest_passed = Rc::new(RefCell::new(false));
        let oldest_passed_clone = oldest_passed.clone();

        let mut input = FirstMinutesInput {
            now_ms: 2_000_000_000_000,
            owner_zone: Tz::UTC,
            capture: json!({"registry": "registry_complete", "clients": [{"cid": "c1"}], "unassessed": []}),
            reported_platforms: None,
            ledger_platforms: BTreeMap::new(),
            bound_streams: Some(vec![BoundStream {
                name: "stream1".into(),
                cid: Some("c1".into()),
                platform_allocation: None,
            }]),
            segments: Some(segments.clone()),
            earliest_progress: None,
            brain_state: "ready".into(),
            brain_reason: None,
            brain_progressing: false,
            ui_lane: None,
            segment_words: {
                let oldest = oldest_day.clone();
                Box::new(move |c| {
                    *calls_clone.borrow_mut() += 1;
                    if c.day == oldest {
                        *oldest_passed_clone.borrow_mut() = true;
                        Some("oldest words".into())
                    } else {
                        None
                    }
                })
            },
        };

        let out = project_first_minutes(&input);
        assert_eq!(*calls.borrow(), 300);
        assert!(!*oldest_passed.borrow());
        assert_ne!(out["proof"]["state"], "heard");

        // Case B: Words on the 300th-newest (Candidate 1, day 1).
        let target_day = segments[1].day.clone();
        let calls_b = Rc::new(RefCell::new(0usize));
        let calls_b_clone = calls_b.clone();
        let oldest_passed_b = Rc::new(RefCell::new(false));
        let oldest_passed_b_clone = oldest_passed_b.clone();

        input.segment_words = {
            let oldest = oldest_day.clone();
            let target = target_day.clone();
            Box::new(move |c| {
                *calls_b_clone.borrow_mut() += 1;
                if c.day == oldest {
                    *oldest_passed_b_clone.borrow_mut() = true;
                }
                if c.day == target {
                    Some("300th words".into())
                } else {
                    None
                }
            })
        };

        let out_b = project_first_minutes(&input);
        assert_eq!(out_b["proof"]["state"], "heard");
        assert_eq!(out_b["proof"]["text"], "300th words");
        assert!(!*oldest_passed_b.borrow());
        assert_eq!(
            out_b["proof"]["href"],
            format!("/app/transcripts/{target_day}?stream=stream1#120000_300")
        );

        // Case C: A newer non-analyzed segment (pending or empty) is not passed to the reader
        let segs_c = vec![
            SegmentCandidate {
                day: "20261012".into(),
                key: "120000_300".into(),
                stream: "stream1".into(),
                data_state: DataStateMap(BTreeMap::from([("audio".into(), "pending".into())])),
                reported_tz: Some("UTC".into()),
                reported_offset_seconds: Some(0),
            },
            SegmentCandidate {
                day: "20261011".into(),
                key: "120000_300".into(),
                stream: "stream1".into(),
                data_state: DataStateMap(BTreeMap::from([("audio".into(), "analyzed".into())])),
                reported_tz: Some("UTC".into()),
                reported_offset_seconds: Some(0),
            },
        ];
        let pending_passed = Rc::new(RefCell::new(false));
        let pending_passed_clone = pending_passed.clone();
        input.segments = Some(segs_c);
        input.segment_words = Box::new(move |c| {
            if c.day == "20261012" {
                *pending_passed_clone.borrow_mut() = true;
            }
            if c.day == "20261011" {
                Some("older words win".into())
            } else {
                None
            }
        });
        let out_c = project_first_minutes(&input);
        assert_eq!(out_c["proof"]["state"], "heard");
        assert_eq!(out_c["proof"]["text"], "older words win");
        assert!(!*pending_passed.borrow());
    }

    #[test]
    fn trim_proof_text_geometry() {
        // A 300-scalar space-separated string
        let original = "abcd ".repeat(60);
        assert_eq!(original.chars().count(), 300);
        let trimmed = trim_proof_text(&original);

        assert!(trimmed.starts_with('…'));
        let suffix = &trimmed['…'.len_utf8()..];
        assert!(!suffix.starts_with(' '));
        assert!(original.contains(&format!(" {suffix}")) || original.starts_with(suffix));
        assert!(suffix.split_whitespace().count() > 1);
        assert!(trimmed.chars().count() <= 200);
        assert!(suffix.chars().count() + 5 > 199);

        // 250 repeats of 'é' (one scalar) and no whitespace
        let repeat_e = "é".repeat(250);
        let trimmed_e = trim_proof_text(&repeat_e);
        assert_eq!(trimmed_e.chars().count(), 200);
        assert!(trimmed_e.starts_with('…'));
        assert_eq!(
            &trimmed_e['…'.len_utf8()..],
            &repeat_e[repeat_e.len() - 199 * 'é'.len_utf8()..]
        );

        // A 200-scalar string and shorter are unchanged
        let s200 = "x".repeat(200);
        assert_eq!(trim_proof_text(&s200), s200);

        let s_short = "hello world";
        assert_eq!(trim_proof_text(s_short), s_short);
    }

    #[test]
    fn test_read_segment_words_filesystem() {
        let temp = tempfile::tempdir().unwrap();
        let seg_dir = temp.path();

        let header =
            "{\"_solstone_processing\":{\"state\":\"analyzed\",\"kind\":\"transcript\"}}\n";

        // Case 1: clip_audio.jsonl + screen.jsonl + talents/ child
        std::fs::write(
            seg_dir.join("clip_audio.jsonl"),
            format!("{header}{{\"text\":\"clip speech\"}}\n"),
        )
        .unwrap();
        std::fs::write(
            seg_dir.join("screen.jsonl"),
            format!("{header}{{\"text\":\"screen text\"}}\n"),
        )
        .unwrap();
        let talents_dir = seg_dir.join("talents");
        std::fs::create_dir_all(&talents_dir).unwrap();
        std::fs::write(
            talents_dir.join("extra_audio.jsonl"),
            format!("{header}{{\"text\":\"talent speech\"}}\n"),
        )
        .unwrap();

        assert_eq!(read_segment_words(seg_dir), Some("clip speech".into()));

        // Case 2: audio.jsonl then mic_audio.jsonl concatenate in filename order
        let temp2 = tempfile::tempdir().unwrap();
        let seg2 = temp2.path();
        std::fs::write(
            seg2.join("mic_audio.jsonl"),
            format!("{header}{{\"text\":\"mic words\"}}\n"),
        )
        .unwrap();
        std::fs::write(
            seg2.join("audio.jsonl"),
            format!("{header}{{\"text\":\"audio words\"}}\n"),
        )
        .unwrap();
        assert_eq!(
            read_segment_words(seg2),
            Some("audio words mic words".into())
        );

        // Case 3: Sibling audio jsonl with bad UTF-8 skipped
        let temp3 = tempfile::tempdir().unwrap();
        let seg3 = temp3.path();
        std::fs::write(
            seg3.join("audio.jsonl"),
            format!("{header}{{\"text\":\"readable audio\"}}\n"),
        )
        .unwrap();
        std::fs::write(seg3.join("bad_audio.jsonl"), [0xFF, 0xFE, 0xFD]).unwrap();
        assert_eq!(read_segment_words(seg3), Some("readable audio".into()));

        // Case 4: corrected "" and text "hello" -> hello; whitespace corrected falls through; non-empty corrected wins
        let temp4 = tempfile::tempdir().unwrap();
        let seg4 = temp4.path();
        let row1 = "{\"corrected\":\"\",\"text\":\"hello\"}\n";
        let row2 = "{\"corrected\":\"   \",\"text\":\"world\"}\n";
        let row3 = "{\"corrected\":\"fixed\",\"text\":\"original\"}\n";
        std::fs::write(
            seg4.join("audio.jsonl"),
            format!("{header}{row1}{row2}{row3}"),
        )
        .unwrap();
        assert_eq!(read_segment_words(seg4), Some("hello world fixed".into()));

        // Case 5: rows carrying start, speaker, emotion, beside *_transcript.md
        let temp5 = tempfile::tempdir().unwrap();
        let seg5 = temp5.path();
        let extra_row = "{\"start\":\"12:00:00\",\"speaker\":1,\"emotion\":\"neutral\",\"text\":\"only speech\"}\n";
        std::fs::write(seg5.join("audio.jsonl"), format!("{header}{extra_row}")).unwrap();
        std::fs::write(
            seg5.join("clip_transcript.md"),
            "Different markdown contents",
        )
        .unwrap();
        assert_eq!(read_segment_words(seg5), Some("only speech".into()));
    }

    #[test]
    fn test_paired_streams_read_first_minutes() {
        let temp = tempfile::tempdir().unwrap();
        let journal = temp.path();

        let completed_at = 1_000_000_000_000i64;
        std::fs::create_dir_all(journal.join("config")).unwrap();
        std::fs::write(
            journal.join("config/journal.json"),
            json!({
                "identity": {"timezone": "UTC"},
                "setup": {"completed_at": completed_at}
            })
            .to_string(),
        )
        .unwrap();

        std::fs::create_dir_all(journal.join("link")).unwrap();
        let ledger = json!([{
            "fingerprint": "sha256:paired_client",
            "device_label": "Phone",
            "paired_at": "2026-10-10T00:00:00Z",
            "instance_id": "inst",
            "role": "",
            "kind": "cert",
            "platform": "ios"
        }]);
        std::fs::write(
            journal.join("link/authorized_clients.json"),
            ledger.to_string(),
        )
        .unwrap();

        std::fs::create_dir_all(journal.join("streams")).unwrap();
        let stream = json!({
            "name": "paired_stream",
            "kind": "observer",
            "host": null,
            "platform": null,
            "created_at": 1000000000000u64,
            "last_day": null,
            "last_segment": null,
            "seq": 0,
            "cid": "sha256:paired_client",
            "source": ""
        });
        std::fs::write(
            journal.join("streams/paired_stream.json"),
            stream.to_string(),
        )
        .unwrap();

        let header =
            "{\"_solstone_processing\":{\"state\":\"analyzed\",\"kind\":\"transcript\"}}\n";

        // Paired segment
        let paired_seg_dir = journal.join("chronicle/20261010/paired_stream/120000_300");
        std::fs::create_dir_all(&paired_seg_dir).unwrap();
        std::fs::write(
            paired_seg_dir.join("audio.jsonl"),
            format!("{header}{{\"start\":\"12:00:00\",\"text\":\"paired words\"}}\n"),
        )
        .unwrap();

        // Newer segment on unbound stream (no streams/<name>.json)
        let unbound_seg_dir = journal.join("chronicle/20261010/unbound_stream/130000_300");
        std::fs::create_dir_all(&unbound_seg_dir).unwrap();
        std::fs::write(
            unbound_seg_dir.join("audio.jsonl"),
            format!("{header}{{\"start\":\"13:00:00\",\"text\":\"unbound words\"}}\n"),
        )
        .unwrap();

        let now_ms = completed_at + 86_400_000;
        let ctx = HomeContext::with_zone(
            journal,
            chrono::DateTime::from_timestamp_millis(now_ms).unwrap(),
            Tz::UTC,
        );
        let out = read_first_minutes_with(&ctx, false, no_setup_progress);

        assert_eq!(out["window_open"], true);
        assert_eq!(out["proof"]["state"], "heard");
        assert_eq!(out["proof"]["text"], "paired words");
        assert_eq!(
            out["proof"]["href"],
            "/app/transcripts/20261010?stream=paired_stream#120000_300"
        );
    }
}
