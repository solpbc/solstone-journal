// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Loader for transcribed cluster inputs to feed the candidate pool.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::Path;

use serde_json::{Value, json};
use solstone_core_entity::normalize_embedding;
use solstone_core_speaker_id::embeddings::load_embeddings_file;
use solstone_core_speaker_id::transcript::{TranscriptError, read_transcript_rows};
use thiserror::Error;

use crate::candidate_tracker::{ClusterInput, trim_solo_cluster_indices};
use crate::segment_catalog::catalog_day;

#[derive(Debug, Error)]
pub enum TranscribedClusterError {
    #[error("failed to read transcript: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to parse transcript: {0}")]
    Transcript(#[from] TranscriptError),
}

#[derive(Debug, Clone, PartialEq)]
pub enum TranscribedClusterLoad {
    Clusters(Vec<ClusterInput>),
    NoClusters,
    Unreadable,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadKind {
    Transcript,
    Embeddings,
}

/// A test-only hook slot.
#[cfg(test)]
type TestHook<F> = std::cell::RefCell<Option<Box<F>>>;
#[cfg(test)]
type ReadHookFn = dyn Fn(ReadKind, &Path);

#[cfg(test)]
thread_local! {
    static CATALOG_FILTER_HOOK: TestHook<dyn Fn(&Path) -> bool> = const { std::cell::RefCell::new(None) };
    static READ_HOOK: TestHook<ReadHookFn> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub struct CatalogFilterHookGuard;
#[cfg(test)]
impl Drop for CatalogFilterHookGuard {
    fn drop(&mut self) {
        CATALOG_FILTER_HOOK.with(|cell| *cell.borrow_mut() = None);
    }
}
#[cfg(test)]
pub fn set_catalog_filter_hook(hook: impl Fn(&Path) -> bool + 'static) -> CatalogFilterHookGuard {
    CATALOG_FILTER_HOOK.with(|cell| *cell.borrow_mut() = Some(Box::new(hook)));
    CatalogFilterHookGuard
}

#[cfg(test)]
pub struct ReadHookGuard;
#[cfg(test)]
impl Drop for ReadHookGuard {
    fn drop(&mut self) {
        READ_HOOK.with(|cell| *cell.borrow_mut() = None);
    }
}
#[cfg(test)]
pub fn set_read_hook(hook: impl Fn(ReadKind, &Path) + 'static) -> ReadHookGuard {
    READ_HOOK.with(|cell| *cell.borrow_mut() = Some(Box::new(hook)));
    ReadHookGuard
}

/// Load cluster inputs from a completed transcription.
pub fn load_transcribed_cluster_inputs(
    journal: &Path,
    transcript_path: &Path,
    embeddings_path: &Path,
) -> Result<TranscribedClusterLoad, TranscribedClusterError> {
    let Some(seg_dir) = transcript_path.parent() else {
        return Ok(TranscribedClusterLoad::Unreadable);
    };
    let day = seg_dir
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .filter(|s| s.len() == 8 && s.chars().all(|c| c.is_ascii_digit()))
        .or_else(|| {
            seg_dir
                .parent()
                .and_then(|p| p.parent())
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .filter(|s| s.len() == 8 && s.chars().all(|c| c.is_ascii_digit()))
        });
    let Some(day) = day else {
        return Ok(TranscribedClusterLoad::Unreadable);
    };

    let catalog = match catalog_day(journal, day) {
        Ok(catalog) => catalog,
        Err(_) => return Ok(TranscribedClusterLoad::Unreadable),
    };
    let seg_canon = seg_dir.canonicalize().ok();
    let segment = catalog.iter().find(|s| {
        #[cfg(test)]
        {
            let filtered = CATALOG_FILTER_HOOK.with(|cell| {
                if let Some(ref hook) = *cell.borrow() {
                    hook(&s.path)
                } else {
                    false
                }
            });
            if filtered {
                return false;
            }
        }
        s.path == seg_dir
            || seg_canon
                .as_ref()
                .is_some_and(|c| s.path.canonicalize().ok().as_ref() == Some(c))
    });
    let Some(segment) = segment else {
        return Ok(TranscribedClusterLoad::Unreadable);
    };

    let source = transcript_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_owned();

    #[cfg(test)]
    READ_HOOK.with(|cell| {
        if let Some(ref hook) = *cell.borrow() {
            hook(ReadKind::Transcript, journal);
        }
    });

    let bytes = fs::read(transcript_path)?;
    let is_single = is_single_evidence_header(&bytes);
    let read = read_transcript_rows(&bytes)?;

    let mut integer_speakers = HashMap::new();
    for row in read.rows {
        if let Some(speaker) = row.value.get("speaker").and_then(Value::as_i64) {
            integer_speakers.insert(row.sentence_id, speaker);
        }
    }

    if integer_speakers.is_empty() && !is_single {
        return Ok(TranscribedClusterLoad::NoClusters);
    }

    #[cfg(test)]
    READ_HOOK.with(|cell| {
        if let Some(ref hook) = *cell.borrow() {
            hook(ReadKind::Embeddings, journal);
        }
    });

    let emb_file = match load_embeddings_file(embeddings_path) {
        Ok(Some(file)) => file,
        Ok(None) | Err(_) => return Ok(TranscribedClusterLoad::Unreadable),
    };

    let layout_str = segment.layout.as_str();
    let mut clusters = Vec::new();

    if !integer_speakers.is_empty() {
        let mut grouped: BTreeMap<i64, Vec<(i64, Vec<f32>, f64)>> = BTreeMap::new();
        for (idx, (sid, embedding)) in emb_file.statements.iter().enumerate() {
            if let Some(&label) = integer_speakers.get(sid)
                && let Some(norm) = normalize_embedding(embedding)
            {
                let dur = emb_file.durations_s.get(idx).copied().unwrap_or(0.0);
                grouped.entry(label).or_default().push((*sid, norm, dur));
            }
        }

        for (label, mut members) in grouped {
            if members.is_empty() {
                continue;
            }
            members.sort_by_key(|m| m.0);
            let sentence_ids: Vec<i64> = members.iter().map(|m| m.0).collect();
            let embeddings: Vec<Vec<f32>> = members.iter().map(|m| m.1.clone()).collect();
            let durations_s: Vec<f64> = members.iter().map(|m| m.2).collect();
            let source_segment = json!({
                "day": segment.day,
                "stream_layout": layout_str,
                "stream": segment.stream,
                "segment_key": segment.name,
                "source": source,
                "cluster_label": label,
                "sentence_ids": sentence_ids,
            });
            clusters.push(ClusterInput {
                source_segment,
                embeddings,
                durations_s,
            });
        }
    } else if is_single {
        let mut members: Vec<(i64, Vec<f32>, f64)> = Vec::new();
        for (idx, (sid, embedding)) in emb_file.statements.iter().enumerate() {
            if let Some(norm) = normalize_embedding(embedding) {
                let dur = emb_file.durations_s.get(idx).copied().unwrap_or(0.0);
                members.push((*sid, norm, dur));
            }
        }
        if members.is_empty() {
            return Ok(TranscribedClusterLoad::NoClusters);
        }
        let norm_rows: Vec<Vec<f32>> = members.iter().map(|m| m.1.clone()).collect();
        let (kept_indices, center) = trim_solo_cluster_indices(&norm_rows);
        if center.is_none() || kept_indices.is_empty() {
            return Ok(TranscribedClusterLoad::NoClusters);
        }

        let mut kept_members = Vec::with_capacity(kept_indices.len());
        for index in kept_indices {
            if let Some(member) = members.get(index) {
                kept_members.push(member.clone());
            }
        }
        kept_members.sort_by_key(|m| m.0);

        let sentence_ids: Vec<i64> = kept_members.iter().map(|m| m.0).collect();
        let embeddings: Vec<Vec<f32>> = kept_members.iter().map(|m| m.1.clone()).collect();
        let durations_s: Vec<f64> = kept_members.iter().map(|m| m.2).collect();
        let source_segment = json!({
            "day": segment.day,
            "stream_layout": layout_str,
            "stream": segment.stream,
            "segment_key": segment.name,
            "source": source,
            "cluster_label": -1,
            "sentence_ids": sentence_ids,
        });
        clusters.push(ClusterInput {
            source_segment,
            embeddings,
            durations_s,
        });
    }

    if clusters.is_empty() {
        Ok(TranscribedClusterLoad::NoClusters)
    } else {
        Ok(TranscribedClusterLoad::Clusters(clusters))
    }
}

fn is_single_evidence_header(bytes: &[u8]) -> bool {
    let Some(first_line) = bytes.split(|&b| b == b'\n').next() else {
        return false;
    };
    let Ok(val) = serde_json::from_slice::<Value>(first_line) else {
        return false;
    };
    let Some(obj) = val.as_object() else {
        return false;
    };
    let evidence = obj.get("speaker_evidence").and_then(Value::as_str);
    let version = obj.get("speaker_evidence_version").and_then(Value::as_str);
    let multi_fraction = obj
        .get("speaker_evidence_multi_fraction")
        .and_then(Value::as_f64);
    matches!(evidence, Some("none" | "single" | "multi"))
        && version == Some("windowed-slots-v1")
        && multi_fraction.is_some()
        && evidence == Some("single")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEST_SEQ: AtomicUsize = AtomicUsize::new(0);

    struct TempJournal(PathBuf);
    impl TempJournal {
        fn new() -> Self {
            let seq = TEST_SEQ.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "solstone-transcribed-clusters-test-{}-{seq}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("create temp journal");
            Self(path)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TempJournal {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn write_segment_data(
        journal: &Path,
        day: &str,
        stream: Option<&str>,
        segment_dir_name: &str,
        transcript_content: &str,
        statement_ids: &[i32],
        embeddings: &[Vec<f32>],
        durations: &[f32],
    ) -> (PathBuf, PathBuf) {
        let seg_path = match stream {
            Some(st) => journal
                .join("chronicle")
                .join(day)
                .join(st)
                .join(segment_dir_name),
            None => journal.join("chronicle").join(day).join(segment_dir_name),
        };
        fs::create_dir_all(&seg_path).unwrap();
        let jsonl_path = seg_path.join("audio.jsonl");
        let npz_path = seg_path.join("audio.npz");
        fs::write(&jsonl_path, transcript_content).unwrap();

        let payload_path = seg_path.join("payload.f32");
        let mut raw_bytes = Vec::new();
        for row in embeddings {
            for col in row {
                raw_bytes.extend_from_slice(&col.to_le_bytes());
            }
        }
        fs::write(&payload_path, raw_bytes).unwrap();

        let statements: Vec<Value> = statement_ids
            .iter()
            .enumerate()
            .map(|(i, &id)| {
                json!({
                    "id": id,
                    "start_offset_us": (i as i64) * 1_000_000,
                    "text": format!("statement {id}"),
                })
            })
            .collect();

        let req = json!({
            "schema": "solstone-speaker-transcript-write-request-v1",
            "output": {
                "jsonl_path": jsonl_path.display().to_string(),
                "npz_path": npz_path.display().to_string(),
                "redo": true,
            },
            "base_time_us_of_day": 100_000_u64,
            "source": "audio",
            "statements": statements,
            "header": {"raw": "audio.wav", "model": "model", "device": "cpu", "compute_type": "int8"},
            "embeddings": {
                "payload_path": payload_path,
                "payload_format": "raw-f32le-row-major-v1",
                "dtype": "float32-le",
                "shape": [embeddings.len(), 256],
                "byte_count": embeddings.len() * 256 * 4,
                "statement_ids": statement_ids,
                "durations_s": durations,
                "encoder": "test",
            }
        });
        solstone_core_speaker_id::writer::write_request(
            serde_json::to_vec(&req).unwrap().as_slice(),
        )
        .unwrap();
        // Overwrite jsonl with exact test transcript content
        fs::write(&jsonl_path, transcript_content).unwrap();
        (jsonl_path, npz_path)
    }

    #[test]
    fn persisted_sentence_id_is_stored_not_line_number() {
        let journal = TempJournal::new();
        let mut vec_one = vec![0.0_f32; 256];
        vec_one[0] = 1.0;

        let header = json!({"raw": "audio.wav"}).to_string();
        let row = json!({"sentence_id": 40, "speaker": 1, "text": "hello"}).to_string();
        let transcript = format!("{header}\n{row}\n");

        let (jsonl_path, npz_path) = write_segment_data(
            journal.path(),
            "20260101",
            Some("mic"),
            "090000_60",
            &transcript,
            &[40],
            &[vec_one],
            &[2.5],
        );

        let TranscribedClusterLoad::Clusters(clusters) =
            load_transcribed_cluster_inputs(journal.path(), &jsonl_path, &npz_path).unwrap()
        else {
            panic!("expected clusters");
        };
        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0].source_segment["sentence_ids"], json!([40]));
    }

    #[test]
    fn integer_speakers_win_over_non_single_header() {
        let journal = TempJournal::new();
        let mut vec_one = vec![0.0_f32; 256];
        vec_one[0] = 1.0;

        let header = json!({
            "speaker_evidence": "multi",
            "speaker_evidence_version": "windowed-slots-v1",
            "speaker_evidence_multi_fraction": 0.8
        })
        .to_string();
        let row1 = json!({"sentence_id": 1, "speaker": 1, "text": "one"}).to_string();
        let row2 = json!({"sentence_id": 2, "speaker": 2, "text": "two"}).to_string();
        let transcript = format!("{header}\n{row1}\n{row2}\n");

        let (jsonl_path, npz_path) = write_segment_data(
            journal.path(),
            "20260101",
            Some("mic"),
            "090000_60",
            &transcript,
            &[1, 2],
            &[vec_one.clone(), vec_one],
            &[1.0, 2.0],
        );

        let TranscribedClusterLoad::Clusters(clusters) =
            load_transcribed_cluster_inputs(journal.path(), &jsonl_path, &npz_path).unwrap()
        else {
            panic!("expected clusters");
        };
        assert_eq!(clusters.len(), 2);
        assert_eq!(clusters[0].source_segment["cluster_label"], 1);
        assert_eq!(clusters[1].source_segment["cluster_label"], 2);
    }

    #[test]
    fn no_integers_and_multi_or_missing_header_returns_empty_without_opening_npz() {
        let journal = TempJournal::new();
        let header = json!({
            "speaker_evidence": "multi",
            "speaker_evidence_version": "windowed-slots-v1",
            "speaker_evidence_multi_fraction": 0.8
        })
        .to_string();
        let row = json!({"sentence_id": 1, "text": "no speaker"}).to_string();
        let transcript = format!("{header}\n{row}\n");

        let seg_path = journal.path().join("chronicle/20260101/mic/090000_60");
        fs::create_dir_all(&seg_path).unwrap();
        let jsonl_path = seg_path.join("audio.jsonl");
        fs::write(&jsonl_path, transcript).unwrap();
        let garbage_npz = seg_path.join("nonexistent_garbage.npz");

        let load =
            load_transcribed_cluster_inputs(journal.path(), &jsonl_path, &garbage_npz).unwrap();
        assert_eq!(load, TranscribedClusterLoad::NoClusters);
    }

    #[test]
    fn single_header_no_integers_produces_solo_trimmed_cluster() {
        let journal = TempJournal::new();
        let mut v1 = vec![0.0_f32; 256];
        v1[0] = 1.0;
        let mut v2 = vec![0.0_f32; 256];
        v2[0] = 1.0;
        let mut v_outlier = vec![0.0_f32; 256];
        v_outlier[0] = -1.0; // opposite direction, cosine -1.0 < 0.43

        let header = json!({
            "speaker_evidence": "single",
            "speaker_evidence_version": "windowed-slots-v1",
            "speaker_evidence_multi_fraction": 0.05
        })
        .to_string();
        let row1 = json!({"sentence_id": 10, "text": "a"}).to_string();
        let row2 = json!({"sentence_id": 20, "text": "b"}).to_string();
        let row3 = json!({"sentence_id": 30, "text": "outlier"}).to_string();
        let transcript = format!("{header}\n{row1}\n{row2}\n{row3}\n");

        let (jsonl_path, npz_path) = write_segment_data(
            journal.path(),
            "20260101",
            Some("mic"),
            "090000_60",
            &transcript,
            &[10, 20, 30],
            &[v1, v2, v_outlier],
            &[1.0, 2.0, 3.0],
        );

        let TranscribedClusterLoad::Clusters(clusters) =
            load_transcribed_cluster_inputs(journal.path(), &jsonl_path, &npz_path).unwrap()
        else {
            panic!("expected clusters");
        };
        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0].source_segment["cluster_label"], -1);
        assert_eq!(clusters[0].source_segment["sentence_ids"], json!([10, 20]));
        assert_eq!(clusters[0].durations_s, vec![1.0, 2.0]);
    }

    #[test]
    fn zero_vector_dropped() {
        let journal = TempJournal::new();
        let mut v1 = vec![0.0_f32; 256];
        v1[0] = 1.0;
        let zero_vec = vec![0.0_f32; 256];

        let header = json!({
            "speaker_evidence": "single",
            "speaker_evidence_version": "windowed-slots-v1",
            "speaker_evidence_multi_fraction": 0.0
        })
        .to_string();
        let row1 = json!({"sentence_id": 1, "text": "a"}).to_string();
        let row2 = json!({"sentence_id": 2, "text": "zero"}).to_string();
        let transcript = format!("{header}\n{row1}\n{row2}\n");

        let (jsonl_path, npz_path) = write_segment_data(
            journal.path(),
            "20260101",
            Some("mic"),
            "090000_60",
            &transcript,
            &[1, 2],
            &[v1, zero_vec],
            &[1.0, 2.0],
        );

        let TranscribedClusterLoad::Clusters(clusters) =
            load_transcribed_cluster_inputs(journal.path(), &jsonl_path, &npz_path).unwrap()
        else {
            panic!("expected clusters");
        };
        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0].source_segment["sentence_ids"], json!([1]));
    }

    #[test]
    fn direct_layout_stores_direct_and_default_stream() {
        let journal = TempJournal::new();
        let mut v1 = vec![0.0_f32; 256];
        v1[0] = 1.0;

        let header = json!({"speaker_evidence": "single", "speaker_evidence_version": "windowed-slots-v1", "speaker_evidence_multi_fraction": 0.0}).to_string();
        let row = json!({"sentence_id": 5, "text": "msg"}).to_string();
        let transcript = format!("{header}\n{row}\n");

        let (jsonl_path, npz_path) = write_segment_data(
            journal.path(),
            "20260101",
            None, // direct layout
            "090000_60",
            &transcript,
            &[5],
            &[v1],
            &[1.0],
        );

        let TranscribedClusterLoad::Clusters(clusters) =
            load_transcribed_cluster_inputs(journal.path(), &jsonl_path, &npz_path).unwrap()
        else {
            panic!("expected clusters");
        };
        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0].source_segment["stream_layout"], "direct");
        assert_eq!(clusters[0].source_segment["stream"], "_default");
    }

    #[test]
    fn named_layout_and_suffixed_basename_stored_whole() {
        let journal = TempJournal::new();
        let mut v1 = vec![0.0_f32; 256];
        v1[0] = 1.0;

        let header = json!({"speaker_evidence": "single", "speaker_evidence_version": "windowed-slots-v1", "speaker_evidence_multi_fraction": 0.0}).to_string();
        let row = json!({"sentence_id": 5, "text": "msg"}).to_string();
        let transcript = format!("{header}\n{row}\n");

        let (jsonl_path, npz_path) = write_segment_data(
            journal.path(),
            "20260101",
            Some("office"),
            "093000_300_summary",
            &transcript,
            &[5],
            &[v1],
            &[1.0],
        );

        let TranscribedClusterLoad::Clusters(clusters) =
            load_transcribed_cluster_inputs(journal.path(), &jsonl_path, &npz_path).unwrap()
        else {
            panic!("expected clusters");
        };
        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0].source_segment["stream_layout"], "named");
        assert_eq!(clusters[0].source_segment["stream"], "office");
        assert_eq!(
            clusters[0].source_segment["segment_key"],
            "093000_300_summary"
        );
    }

    #[test]
    fn sentence_ids_are_sorted_ascending() {
        let journal = TempJournal::new();
        let mut v = vec![0.0_f32; 256];
        v[0] = 1.0;

        let header = json!({"speaker_evidence": "single", "speaker_evidence_version": "windowed-slots-v1", "speaker_evidence_multi_fraction": 0.0}).to_string();
        let row1 = json!({"sentence_id": 99, "text": "high"}).to_string();
        let row2 = json!({"sentence_id": 3, "text": "low"}).to_string();
        let transcript = format!("{header}\n{row1}\n{row2}\n");

        let (jsonl_path, npz_path) = write_segment_data(
            journal.path(),
            "20260101",
            Some("mic"),
            "090000_60",
            &transcript,
            &[99, 3],
            &[v.clone(), v],
            &[1.0, 2.0],
        );

        let TranscribedClusterLoad::Clusters(clusters) =
            load_transcribed_cluster_inputs(journal.path(), &jsonl_path, &npz_path).unwrap()
        else {
            panic!("expected clusters");
        };
        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0].source_segment["sentence_ids"], json!([3, 99]));
    }
}
