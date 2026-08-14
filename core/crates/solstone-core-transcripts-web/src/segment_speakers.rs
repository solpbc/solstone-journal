// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::Read;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use solstone_core_entity::{read_entity_identity, read_journal_principal};
use solstone_core_npy::parse_npy;
use zip::ZipArchive;

#[derive(Clone, Serialize)]
pub(crate) struct SpeakerLabelsState {
    pub(crate) present: bool,
    pub(crate) loaded: bool,
    pub(crate) source: Option<String>,
    pub(crate) ambiguous: bool,
}
#[derive(Clone, Serialize)]
pub(crate) struct SpeakerLabel {
    pub(crate) name: String,
    pub(crate) entity_id: String,
    pub(crate) confidence: Value,
    pub(crate) confidence_state: String,
    pub(crate) method: Value,
    pub(crate) owner_margin_declined: bool,
    pub(crate) acoustic_margin_declined: bool,
    pub(crate) is_owner: bool,
}
pub(crate) struct SpeakerJoin {
    pub(crate) state: SpeakerLabelsState,
    pub(crate) labels: BTreeMap<i64, SpeakerLabel>,
    pub(crate) warnings: Vec<crate::segment::WarningDetail>,
}

pub(crate) fn load(dir: &Path, journal_root: &Path, now: DateTime<Utc>) -> SpeakerJoin {
    let path = dir.join("talents/speaker_labels.json");
    let present = path.is_file();
    let audio_sources = audio_sources(dir);
    let (source, ambiguous) = if present {
        source(dir, &audio_sources)
    } else {
        (None, false)
    };
    let mut warnings = Vec::new();
    if present && ambiguous {
        warnings.push(warning(
            &path,
            "speaker label source is ambiguous for this segment",
            now,
        ));
    }
    let mut labels = BTreeMap::new();
    let mut loaded = false;
    if present {
        match fs_read_json(&path) {
            Some(Value::Object(payload)) => match payload.get("labels").and_then(Value::as_array) {
                Some(rows) => {
                    loaded = true;
                    let principal = read_journal_principal(journal_root)
                        .ok()
                        .flatten()
                        .and_then(|v| v.get("id").and_then(Value::as_str).map(str::to_owned));
                    for row in rows.iter().filter_map(Value::as_object) {
                        let Some(id) = row.get("sentence_id").and_then(Value::as_i64) else {
                            continue;
                        };
                        let Some(entity_id) = row.get("speaker").and_then(Value::as_str) else {
                            continue;
                        };
                        let entity = read_entity_identity(journal_root, entity_id).ok().flatten();
                        let name = entity
                            .as_ref()
                            .and_then(|value| value.value().get("name"))
                            .and_then(Value::as_str)
                            .unwrap_or(entity_id)
                            .to_owned();
                        let confidence = row.get("confidence").cloned().unwrap_or(Value::Null);
                        let confidence_state = match confidence.as_str() {
                            Some("high" | "medium") => confidence.as_str().unwrap().to_owned(),
                            _ => "unknown".into(),
                        };
                        labels.insert(
                            id,
                            SpeakerLabel {
                                name,
                                entity_id: entity_id.into(),
                                confidence,
                                confidence_state,
                                method: row.get("method").cloned().unwrap_or(Value::Null),
                                owner_margin_declined: row
                                    .get("owner_margin_declined")
                                    .and_then(Value::as_bool)
                                    .unwrap_or(false),
                                acoustic_margin_declined: row
                                    .get("acoustic_margin_declined")
                                    .and_then(Value::as_bool)
                                    .unwrap_or(false),
                                is_owner: principal.as_deref() == Some(entity_id),
                            },
                        );
                    }
                }
                None => warnings.push(warning(
                    &path,
                    "speaker labels are unavailable for this segment",
                    now,
                )),
            },
            _ => warnings.push(warning(
                &path,
                "speaker labels are unavailable for this segment",
                now,
            )),
        }
    }
    SpeakerJoin {
        state: SpeakerLabelsState {
            present,
            loaded,
            source,
            ambiguous,
        },
        labels,
        warnings,
    }
}

pub(crate) fn embedding_ids(dir: &Path, source: &str) -> BTreeSet<i64> {
    read_statement_ids(&dir.join(format!("{source}.npz")))
        .unwrap_or_default()
        .into_iter()
        .collect()
}
pub(crate) fn statement_ordinals(text: &str) -> Vec<Option<i64>> {
    let mut next = 1;
    text.lines()
        .map(|line| match serde_json::from_str::<Value>(line.trim()) {
            Ok(Value::Object(value)) if value.contains_key("start") => {
                let id = next;
                next += 1;
                Some(id)
            }
            _ => None,
        })
        .collect()
}

fn audio_sources(dir: &Path) -> Vec<String> {
    let mut values = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            name.ends_with("audio.jsonl")
                .then(|| name.trim_end_matches(".jsonl").into())
        })
        .collect::<Vec<String>>();
    values.sort();
    values
}
fn source(dir: &Path, audio: &[String]) -> (Option<String>, bool) {
    let mut npz = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let stem = name.strip_suffix(".npz")?;
            (stem == "audio" || stem.ends_with("_audio")).then(|| stem.into())
        })
        .collect::<Vec<String>>();
    npz.sort();
    if let Some(value) = npz.into_iter().next() {
        (Some(value), false)
    } else if audio.len() == 1 {
        (Some(audio[0].clone()), false)
    } else {
        (None, audio.len() > 1)
    }
}
fn warning(path: &Path, message: &str, now: DateTime<Utc>) -> crate::segment::WarningDetail {
    crate::segment::WarningDetail {
        kind: "speaker_labels".into(),
        file: path.display().to_string(),
        message: message.into(),
        ts: now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    }
}
fn fs_read_json(path: &Path) -> Option<Value> {
    serde_json::from_reader(File::open(path).ok()?).ok()
}
fn read_statement_ids(path: &Path) -> Option<Vec<i64>> {
    let mut archive = ZipArchive::new(File::open(path).ok()?).ok()?;
    let mut member = archive.by_name("statement_ids.npy").ok()?;
    let mut bytes = Vec::new();
    member.read_to_end(&mut bytes).ok()?;
    let blob = parse_npy(&bytes).ok()?;
    if blob.fortran_order || blob.shape.len() != 1 {
        return None;
    }
    match blob.descr.as_str() {
        "<i4" => blob
            .payload
            .chunks_exact(4)
            .map(|b| Some(i32::from_le_bytes(b.try_into().ok()?) as i64))
            .collect(),
        "<i8" => blob
            .payload
            .chunks_exact(8)
            .map(|b| Some(i64::from_le_bytes(b.try_into().ok()?)))
            .collect(),
        _ => None,
    }
}
