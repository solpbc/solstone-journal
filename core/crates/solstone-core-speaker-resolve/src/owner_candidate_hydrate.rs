// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Response-time owner candidate sample hydration and evidentiary verification.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use solstone_core_entity::normalize_embedding;
use solstone_core_speaker_id::embeddings::load_embeddings_file;
use solstone_core_speaker_id::transcript::{TranscriptError, read_transcript_rows};

use crate::audio_sample::{AUDIO_FORMATS, audio_info};
use crate::owner_candidate::OwnerCandidate;
use crate::segment_catalog::{decode_stream_layout_value, lookup_segment};

/// Status and classification for one sample's evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceState {
    Ready,
    Unbound,
    Unavailable,
    Ambiguous,
    Mismatch,
}

impl EvidenceState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Unbound => "unbound",
            Self::Unavailable => "unavailable",
            Self::Ambiguous => "ambiguous",
            Self::Mismatch => "mismatch",
        }
    }
}

/// Hydrate candidate samples against current journal artifacts without rewriting durable state.
pub fn hydrate_owner_candidate_samples(
    journal_root: &Path,
    snapshot: &OwnerCandidate,
    samples: &[Value],
) -> Vec<Value> {
    samples
        .iter()
        .map(|sample| hydrate_single_sample(journal_root, snapshot, sample))
        .collect()
}

#[derive(Debug, Clone, PartialEq)]
enum TranscriptEvidence {
    Missing(String),
    Ambiguous,
    Blank(Option<f64>),
    Ready(String, Option<f64>),
}

fn hydrate_single_sample(journal_root: &Path, snapshot: &OwnerCandidate, sample: &Value) -> Value {
    let day = sample
        .get("day")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let stream_layout_val = sample.get("stream_layout");
    let stream_layout_str = stream_layout_val.and_then(Value::as_str).unwrap_or("named");
    let stream = sample
        .get("stream")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let segment_key = sample
        .get("segment_key")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let source = sample
        .get("source")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let sentence_id = sample
        .get("sentence_id")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let duration_s = sample.get("duration_s").and_then(Value::as_f64);

    let layout_res = decode_stream_layout_value(stream_layout_val);
    let segment_dir = match lookup_segment(journal_root, day, stream, segment_key, layout_res) {
        crate::segment_catalog::SegmentLookup::Present(path) => Some(path),
        _ => None,
    };

    let sample_version = sample.get("version").and_then(Value::as_str);
    let sample_embedding_sha256 = sample.get("embedding_sha256").and_then(Value::as_str);
    let audio_sha256_val = sample.get("audio_sha256");

    let is_unbound =
        sample_version.is_none() || sample_embedding_sha256.is_none() || audio_sha256_val.is_none();

    let (audio_file_path, audio_basename, audio_bytes, current_audio_sha256) =
        resolve_audio_evidence(segment_dir.as_deref(), source);

    let (current_embedding, current_embedding_sha256, cosine_similarity) =
        resolve_embedding_evidence(segment_dir.as_deref(), source, sentence_id, snapshot);

    let transcript_evidence =
        resolve_transcript_evidence(segment_dir.as_deref(), source, sentence_id, segment_key);

    let mut transcript_text: Option<String> = None;
    let mut offset_s: Option<f64> = None;
    match &transcript_evidence {
        TranscriptEvidence::Ready(text, off) => {
            transcript_text = Some(text.clone());
            offset_s = *off;
        }
        TranscriptEvidence::Blank(off) => {
            offset_s = *off;
        }
        _ => {}
    }

    let mut artifact_eligible = false;
    let evidence_state;
    let evidence_reason;
    let mut audio_url: Option<String> = None;

    if is_unbound {
        evidence_state = EvidenceState::Unbound;
        evidence_reason = None;
        if let Some(ref dir) = segment_dir
            && let Ok(layout) = layout_res
        {
            let (url, _) = audio_info(dir, day, stream, segment_key, source, layout);
            audio_url = url;
        }
    } else {
        let sample_audio_sha256 = audio_sha256_val.and_then(Value::as_str);
        let version_matches = sample_version == Some(snapshot.version.as_str());

        if !version_matches {
            evidence_state = EvidenceState::Mismatch;
            evidence_reason = Some("version_mismatch".to_owned());
        } else if audio_sha256_val == Some(&Value::Null) {
            evidence_state = EvidenceState::Unavailable;
            evidence_reason = Some("audio_missing".to_owned());
        } else if let Some(expected_audio_sha) = sample_audio_sha256 {
            let expected_embedding_sha = sample_embedding_sha256.unwrap();

            if audio_file_path.is_none() {
                evidence_state = EvidenceState::Mismatch;
                evidence_reason = Some("audio_missing".to_owned());
            } else if audio_bytes.as_ref().is_none_or(Vec::is_empty) {
                evidence_state = EvidenceState::Mismatch;
                evidence_reason = Some("audio_empty".to_owned());
            } else if current_audio_sha256.as_deref() != Some(expected_audio_sha) {
                evidence_state = EvidenceState::Mismatch;
                evidence_reason = Some("audio_hash_mismatch".to_owned());
            } else if current_embedding.is_none() {
                evidence_state = EvidenceState::Mismatch;
                evidence_reason = Some("embedding_missing".to_owned());
            } else if current_embedding_sha256.as_deref() != Some(expected_embedding_sha) {
                evidence_state = EvidenceState::Mismatch;
                evidence_reason = Some("embedding_hash_mismatch".to_owned());
            } else if cosine_similarity.is_none_or(|cos| cos < snapshot.threshold) {
                evidence_state = EvidenceState::Mismatch;
                evidence_reason = Some("below_threshold".to_owned());
            } else {
                artifact_eligible = true;
                if let Some(ref dir) = segment_dir
                    && let Ok(layout) = layout_res
                {
                    let (url, _) = audio_info(dir, day, stream, segment_key, source, layout);
                    audio_url = url;
                }
                match transcript_evidence {
                    TranscriptEvidence::Missing(reason) => {
                        evidence_state = EvidenceState::Unavailable;
                        evidence_reason = Some(reason);
                    }
                    TranscriptEvidence::Ambiguous => {
                        evidence_state = EvidenceState::Ambiguous;
                        evidence_reason = Some("transcript_ambiguous".to_owned());
                    }
                    TranscriptEvidence::Blank(_) => {
                        evidence_state = EvidenceState::Ready;
                        evidence_reason = Some("transcript_blank".to_owned());
                    }
                    TranscriptEvidence::Ready(_, _) => {
                        evidence_state = EvidenceState::Ready;
                        evidence_reason = None;
                    }
                }
            }
        } else {
            evidence_state = EvidenceState::Mismatch;
            evidence_reason = Some("audio_hash_missing".to_owned());
        }
    }

    let mut map = Map::new();
    map.insert("day".to_owned(), json!(day));
    map.insert("stream_layout".to_owned(), json!(stream_layout_str));
    map.insert("stream".to_owned(), json!(stream));
    map.insert("segment_key".to_owned(), json!(segment_key));
    map.insert("source".to_owned(), json!(source));
    map.insert("sentence_id".to_owned(), json!(sentence_id));

    if let Some(v) = sample_version {
        map.insert("version".to_owned(), json!(v));
    }
    if let Some(emb_sha) = sample_embedding_sha256 {
        map.insert("embedding_sha256".to_owned(), json!(emb_sha));
    }
    if let Some(audio_sha) = audio_sha256_val {
        map.insert("audio_sha256".to_owned(), audio_sha.clone());
    }
    if let Some(basename) = audio_basename {
        map.insert("audio_basename".to_owned(), json!(basename));
    }

    map.insert("evidence_state".to_owned(), json!(evidence_state.as_str()));
    if let Some(reason) = evidence_reason {
        map.insert("evidence_reason".to_owned(), json!(reason));
    }

    map.insert("artifact_eligible".to_owned(), json!(artifact_eligible));

    if let Some(dur) = duration_s {
        map.insert("duration_s".to_owned(), json!(dur));
    }
    if let Some(url) = audio_url {
        map.insert("audio_url".to_owned(), json!(url));
    }
    if let Some(txt) = transcript_text {
        map.insert("text".to_owned(), json!(txt));
    }
    if let Some(off) = offset_s {
        map.insert("offset_s".to_owned(), json!(off));
    }

    Value::Object(map)
}

fn resolve_audio_evidence(
    segment_dir: Option<&Path>,
    source: &str,
) -> (
    Option<PathBuf>,
    Option<String>,
    Option<Vec<u8>>,
    Option<String>,
) {
    let Some(segment_dir) = segment_dir else {
        return (None, None, None, None);
    };

    for (extension, _) in AUDIO_FORMATS {
        let path = segment_dir.join(format!("{source}{extension}"));
        if path.is_file() {
            let Ok(bytes) = fs::read(&path) else {
                continue;
            };
            if bytes.is_empty() {
                let basename = path.file_name().and_then(|n| n.to_str()).map(str::to_owned);
                return (Some(path), basename, Some(bytes), None);
            }
            let hash = format!("{:x}", Sha256::digest(&bytes));
            let basename = path.file_name().and_then(|n| n.to_str()).map(str::to_owned);
            return (Some(path), basename, Some(bytes), Some(hash));
        }
    }

    (None, None, None, None)
}

fn resolve_embedding_evidence(
    segment_dir: Option<&Path>,
    source: &str,
    sentence_id: i64,
    snapshot: &OwnerCandidate,
) -> (Option<Vec<f32>>, Option<String>, Option<f32>) {
    let Some(segment_dir) = segment_dir else {
        return (None, None, None);
    };

    let npz_path = segment_dir.join(format!("{source}.npz"));
    if !npz_path.is_file() {
        return (None, None, None);
    }

    let Ok(Some(file)) = load_embeddings_file(&npz_path) else {
        return (None, None, None);
    };

    let Some((_, raw_embedding)) = file
        .statements
        .into_iter()
        .find(|(id, _)| *id == sentence_id)
    else {
        return (None, None, None);
    };

    let Some(norm_embedding) = normalize_embedding(&raw_embedding) else {
        return (None, None, None);
    };

    let mut le_bytes = Vec::with_capacity(norm_embedding.len() * 4);
    for val in &norm_embedding {
        le_bytes.extend_from_slice(&val.to_le_bytes());
    }
    let hash = format!("{:x}", Sha256::digest(&le_bytes));

    let cosine = norm_embedding
        .iter()
        .zip(&snapshot.centroid)
        .map(|(a, b)| a * b)
        .sum::<f32>();

    (Some(norm_embedding), Some(hash), Some(cosine))
}

fn resolve_transcript_evidence(
    segment_dir: Option<&Path>,
    source: &str,
    sentence_id: i64,
    segment_key: &str,
) -> TranscriptEvidence {
    let Some(segment_dir) = segment_dir else {
        return TranscriptEvidence::Missing("segment_missing".to_owned());
    };

    let transcript_path = segment_dir.join(format!("{source}.jsonl"));
    if !transcript_path.is_file() {
        return TranscriptEvidence::Missing("transcript_missing".to_owned());
    }

    let bytes = match fs::read(&transcript_path) {
        Ok(b) => b,
        Err(_) => return TranscriptEvidence::Missing("transcript_missing".to_owned()),
    };

    let read_result = match read_transcript_rows(&bytes) {
        Ok(r) => r,
        Err(TranscriptError::InvalidUtf8) => {
            return TranscriptEvidence::Missing("transcript_invalid_utf8".to_owned());
        }
    };

    let matching_rows: Vec<_> = read_result
        .rows
        .iter()
        .filter(|r| r.sentence_id == sentence_id)
        .collect();

    if matching_rows.is_empty() {
        return TranscriptEvidence::Missing("transcript_missing".to_owned());
    }
    if matching_rows.len() > 1 {
        return TranscriptEvidence::Ambiguous;
    }

    let row = matching_rows[0];
    let raw_text = row
        .value
        .get("text")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();

    let offset_s = compute_offset(&row.value, segment_key);

    if raw_text.is_empty() {
        TranscriptEvidence::Blank(offset_s)
    } else {
        TranscriptEvidence::Ready(raw_text.to_owned(), offset_s)
    }
}

fn compute_offset(value: &Value, segment_key: &str) -> Option<f64> {
    let statement_start_s = if let Some(start_str) = value.get("start").and_then(Value::as_str) {
        time_to_seconds(start_str)? as f64
    } else {
        let start_num = value.get("start").and_then(Value::as_f64)?;
        if !start_num.is_finite() || start_num < 0.0 {
            return None;
        }
        start_num
    };

    let (segment_start_s, duration_s) = parse_segment_key_timing(segment_key)?;
    let offset = (statement_start_s - segment_start_s).max(0.0);

    if let Some(dur) = duration_s
        && offset > dur
    {
        return None;
    }

    Some(offset)
}

fn parse_segment_key_timing(segment_key: &str) -> Option<(f64, Option<f64>)> {
    let (time_part, rest) = segment_key.split_once('_')?;
    if time_part.len() < 6 {
        return None;
    }
    let hours = time_part[0..2].parse::<f64>().ok()?;
    let minutes = time_part[2..4].parse::<f64>().ok()?;
    let seconds = time_part[4..6].parse::<f64>().ok()?;
    let start_s = hours * 3600.0 + minutes * 60.0 + seconds;

    let duration_s = rest
        .split('_')
        .next()
        .and_then(|d| d.parse::<f64>().ok())
        .filter(|&d| d > 0.0);

    Some((start_s, duration_s))
}

fn time_to_seconds(time: &str) -> Option<i64> {
    let mut parts = time.split(':');
    let hours = parts.next()?.parse::<i64>().ok()?;
    let minutes = parts.next()?.parse::<i64>().ok()?;
    let seconds = parts.next()?.parse::<i64>().ok()?;
    parts
        .next()
        .is_none()
        .then_some(hours * 3600 + minutes * 60 + seconds)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use tempfile::TempDir;
    use zip::write::{SimpleFileOptions, ZipWriter};

    fn make_test_snapshot(version: &str, threshold: f32) -> OwnerCandidate {
        let mut centroid = vec![0.0f32; 256];
        centroid[0] = 1.0;
        OwnerCandidate {
            centroid,
            cluster_size: 5,
            threshold,
            version: version.to_owned(),
            evidence_tier: "direct".to_owned(),
        }
    }

    fn write_test_npz(path: &Path, statement_ids: &[i32], embeddings: &[Vec<f32>]) {
        let file = File::create(path).unwrap();
        let mut zip = ZipWriter::new(file);

        let options = SimpleFileOptions::default();
        zip.start_file("statement_ids.npy", options).unwrap();
        let ids_npy = crate::owner_centroid::i32_vector_npy(statement_ids);
        std::io::Write::write_all(&mut zip, &ids_npy).unwrap();

        zip.start_file("embeddings.npy", options).unwrap();
        let mut emb_data = Vec::new();
        for emb in embeddings {
            for val in emb {
                emb_data.extend_from_slice(&val.to_le_bytes());
            }
        }
        let header = format!(
            "{{'descr': '<f4', 'fortran_order': False, 'shape': ({}, {}), }}\n",
            embeddings.len(),
            if embeddings.is_empty() {
                256
            } else {
                embeddings[0].len()
            }
        );
        let mut header_bytes = header.into_bytes();
        let rem = (header_bytes.len() + 10) % 64;
        let pad = if rem == 0 { 0 } else { 64 - rem };
        header_bytes.extend(std::iter::repeat_n(b' ', pad));
        header_bytes.push(b'\n');

        let mut npy = Vec::new();
        npy.extend_from_slice(b"\x93NUMPY\x01\x00");
        npy.extend_from_slice(&(header_bytes.len() as u16).to_le_bytes());
        npy.extend_from_slice(&header_bytes);
        npy.extend_from_slice(&emb_data);

        std::io::Write::write_all(&mut zip, &npy).unwrap();
        zip.finish().unwrap();
    }

    #[test]
    fn hydrator_sentence_id_not_ordinal() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        let seg_dir = root.join("chronicle/20260918/default/143000_120");
        fs::create_dir_all(&seg_dir).unwrap();

        // Line 1 is header, line 2 has sentence_id=7 and text "target row", line 3 has sentence_id=1 and text "first row"
        let transcript = b"{\"created_at\": 1}\n{\"sentence_id\": 7, \"text\": \"target row\", \"start\": \"14:30:10\"}\n{\"sentence_id\": 1, \"text\": \"first row\", \"start\": \"14:30:05\"}\n";
        fs::write(seg_dir.join("mic.jsonl"), transcript).unwrap();

        let mut emb = vec![0.0f32; 256];
        emb[0] = 1.0;
        write_test_npz(
            &seg_dir.join("mic.npz"),
            &[7, 1],
            &[emb.clone(), emb.clone()],
        );
        fs::write(seg_dir.join("mic.flac"), b"RIFFfakeaudio").unwrap();

        let audio_hash = format!("{:x}", Sha256::digest(b"RIFFfakeaudio"));
        let mut le = Vec::new();
        for v in &emb {
            le.extend_from_slice(&v.to_le_bytes());
        }
        let emb_hash = format!("{:x}", Sha256::digest(&le));

        let snapshot = make_test_snapshot("2026-09-18T00:00:00Z", 0.43);
        let sample = json!({
            "day": "20260918",
            "stream_layout": "named",
            "stream": "default",
            "segment_key": "143000_120",
            "source": "mic",
            "sentence_id": 7,
            "version": "2026-09-18T00:00:00Z",
            "embedding_sha256": emb_hash,
            "audio_sha256": audio_hash,
        });

        let hydrated = hydrate_owner_candidate_samples(root, &snapshot, &[sample]);
        assert_eq!(hydrated.len(), 1);
        assert_eq!(hydrated[0]["text"], "target row");
        assert_eq!(hydrated[0]["artifact_eligible"], true);
        assert_eq!(hydrated[0]["evidence_state"], "ready");
    }

    #[test]
    fn hydrator_legacy_unbound_remains_unbound_without_rewrite() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        let seg_dir = root.join("chronicle/20260918/default/143000_120");
        fs::create_dir_all(&seg_dir).unwrap();

        let snapshot = make_test_snapshot("2026-09-18T00:00:00Z", 0.43);
        // Legacy sample missing version, embedding_sha256, audio_sha256
        let sample = json!({
            "day": "20260918",
            "stream_layout": "named",
            "stream": "default",
            "segment_key": "143000_120",
            "source": "mic",
            "sentence_id": 1,
            "duration_s": 2.5,
        });

        let hydrated = hydrate_owner_candidate_samples(root, &snapshot, &[sample]);
        assert_eq!(hydrated.len(), 1);
        assert_eq!(hydrated[0]["evidence_state"], "unbound");
        assert_eq!(hydrated[0]["artifact_eligible"], false);
        assert!(hydrated[0].get("version").is_none());
    }

    #[test]
    fn hydrator_explicit_null_audio_not_unbound() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        let snapshot = make_test_snapshot("2026-09-18T00:00:00Z", 0.43);
        let sample = json!({
            "day": "20260918",
            "stream_layout": "named",
            "stream": "default",
            "segment_key": "143000_120",
            "source": "mic",
            "sentence_id": 1,
            "version": "2026-09-18T00:00:00Z",
            "embedding_sha256": "abcdef",
            "audio_sha256": null,
        });

        let hydrated = hydrate_owner_candidate_samples(root, &snapshot, &[sample]);
        assert_eq!(hydrated.len(), 1);
        assert_eq!(hydrated[0]["evidence_state"], "unavailable");
        assert_eq!(hydrated[0]["evidence_reason"], "audio_missing");
        assert_eq!(hydrated[0]["artifact_eligible"], false);
        assert!(hydrated[0].get("audio_url").is_none());
    }

    #[test]
    fn hydrator_empty_audio_is_not_eligible() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        let seg_dir = root.join("chronicle/20260918/default/143000_120");
        fs::create_dir_all(&seg_dir).unwrap();

        fs::write(seg_dir.join("mic.flac"), b"").unwrap(); // 0 bytes

        let snapshot = make_test_snapshot("2026-09-18T00:00:00Z", 0.43);
        let sample = json!({
            "day": "20260918",
            "stream_layout": "named",
            "stream": "default",
            "segment_key": "143000_120",
            "source": "mic",
            "sentence_id": 1,
            "version": "2026-09-18T00:00:00Z",
            "embedding_sha256": "abcdef",
            "audio_sha256": "123456",
        });

        let hydrated = hydrate_owner_candidate_samples(root, &snapshot, &[sample]);
        assert_eq!(hydrated.len(), 1);
        assert_eq!(hydrated[0]["evidence_state"], "mismatch");
        assert_eq!(hydrated[0]["evidence_reason"], "audio_empty");
        assert_eq!(hydrated[0]["artifact_eligible"], false);
    }

    #[test]
    fn hydrator_hash_mismatch_and_below_threshold() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        let seg_dir = root.join("chronicle/20260918/default/143000_120");
        fs::create_dir_all(&seg_dir).unwrap();

        fs::write(seg_dir.join("mic.flac"), b"actual_audio_content").unwrap();

        let mut emb = vec![0.0f32; 256];
        emb[1] = 1.0; // Orthogonal to centroid[0] = 1.0 -> cosine = 0.0 < 0.43
        write_test_npz(&seg_dir.join("mic.npz"), &[1], &[emb.clone()]);

        let mut le = Vec::new();
        for v in &emb {
            le.extend_from_slice(&v.to_le_bytes());
        }
        let emb_hash = format!("{:x}", Sha256::digest(&le));

        let snapshot = make_test_snapshot("2026-09-18T00:00:00Z", 0.43);
        let sample = json!({
            "day": "20260918",
            "stream_layout": "named",
            "stream": "default",
            "segment_key": "143000_120",
            "source": "mic",
            "sentence_id": 1,
            "version": "2026-09-18T00:00:00Z",
            "embedding_sha256": emb_hash,
            "audio_sha256": "wrong_audio_hash",
        });

        let hydrated = hydrate_owner_candidate_samples(root, &snapshot, &[sample]);
        assert_eq!(hydrated.len(), 1);
        assert_eq!(hydrated[0]["evidence_state"], "mismatch");
        assert_eq!(hydrated[0]["evidence_reason"], "audio_hash_mismatch");
        assert_eq!(hydrated[0]["artifact_eligible"], false);
    }

    #[test]
    fn hydrator_ambiguous_transcript_omits_text_and_sets_ambiguous() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        let seg_dir = root.join("chronicle/20260918/default/143000_120");
        fs::create_dir_all(&seg_dir).unwrap();

        let transcript = b"{\"created_at\": 1}\n{\"sentence_id\": 1, \"text\": \"row 1\", \"start\": \"14:30:00\"}\n{\"sentence_id\": 1, \"text\": \"row 1 duplicate\", \"start\": \"14:30:05\"}\n";
        fs::write(seg_dir.join("mic.jsonl"), transcript).unwrap();

        let mut emb = vec![0.0f32; 256];
        emb[0] = 1.0;
        write_test_npz(&seg_dir.join("mic.npz"), &[1], &[emb.clone()]);
        fs::write(seg_dir.join("mic.flac"), b"audio").unwrap();

        let audio_hash = format!("{:x}", Sha256::digest(b"audio"));
        let mut le = Vec::new();
        for v in &emb {
            le.extend_from_slice(&v.to_le_bytes());
        }
        let emb_hash = format!("{:x}", Sha256::digest(&le));

        let snapshot = make_test_snapshot("2026-09-18T00:00:00Z", 0.43);
        let sample = json!({
            "day": "20260918",
            "stream_layout": "named",
            "stream": "default",
            "segment_key": "143000_120",
            "source": "mic",
            "sentence_id": 1,
            "version": "2026-09-18T00:00:00Z",
            "embedding_sha256": emb_hash,
            "audio_sha256": audio_hash,
        });

        let hydrated = hydrate_owner_candidate_samples(root, &snapshot, &[sample]);
        assert_eq!(hydrated.len(), 1);
        assert_eq!(hydrated[0]["evidence_state"], "ambiguous");
        assert_eq!(hydrated[0]["evidence_reason"], "transcript_ambiguous");
        assert_eq!(hydrated[0]["artifact_eligible"], true);
        assert!(hydrated[0].get("text").is_none());
    }

    #[test]
    fn hydrator_blank_transcript_omits_text_and_sets_transcript_blank() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        let seg_dir = root.join("chronicle/20260918/default/143000_120");
        fs::create_dir_all(&seg_dir).unwrap();

        let transcript = b"{\"created_at\": 1}\n{\"sentence_id\": 1, \"text\": \"   \", \"start\": \"14:30:00\"}\n";
        fs::write(seg_dir.join("mic.jsonl"), transcript).unwrap();

        let mut emb = vec![0.0f32; 256];
        emb[0] = 1.0;
        write_test_npz(&seg_dir.join("mic.npz"), &[1], &[emb.clone()]);
        fs::write(seg_dir.join("mic.flac"), b"audio").unwrap();

        let audio_hash = format!("{:x}", Sha256::digest(b"audio"));
        let mut le = Vec::new();
        for v in &emb {
            le.extend_from_slice(&v.to_le_bytes());
        }
        let emb_hash = format!("{:x}", Sha256::digest(&le));

        let snapshot = make_test_snapshot("2026-09-18T00:00:00Z", 0.43);
        let sample = json!({
            "day": "20260918",
            "stream_layout": "named",
            "stream": "default",
            "segment_key": "143000_120",
            "source": "mic",
            "sentence_id": 1,
            "version": "2026-09-18T00:00:00Z",
            "embedding_sha256": emb_hash,
            "audio_sha256": audio_hash,
        });

        let hydrated = hydrate_owner_candidate_samples(root, &snapshot, &[sample]);
        assert_eq!(hydrated.len(), 1);
        assert_eq!(hydrated[0]["evidence_state"], "ready");
        assert_eq!(hydrated[0]["evidence_reason"], "transcript_blank");
        assert_eq!(hydrated[0]["artifact_eligible"], true);
        assert!(hydrated[0].get("text").is_none());
    }

    #[test]
    fn hydrator_sibling_isolation_sample_failure_does_not_affect_sibling() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        let seg_dir = root.join("chronicle/20260918/default/143000_120");
        fs::create_dir_all(&seg_dir).unwrap();

        let transcript = b"{\"created_at\": 1}\n{\"sentence_id\": 1, \"text\": \"valid text\", \"start\": \"14:30:00\"}\n";
        fs::write(seg_dir.join("mic.jsonl"), transcript).unwrap();

        let mut emb = vec![0.0f32; 256];
        emb[0] = 1.0;
        write_test_npz(&seg_dir.join("mic.npz"), &[1], &[emb.clone()]);
        fs::write(seg_dir.join("mic.flac"), b"audio").unwrap();

        let audio_hash = format!("{:x}", Sha256::digest(b"audio"));
        let mut le = Vec::new();
        for v in &emb {
            le.extend_from_slice(&v.to_le_bytes());
        }
        let emb_hash = format!("{:x}", Sha256::digest(&le));

        let snapshot = make_test_snapshot("2026-09-18T00:00:00Z", 0.43);
        let sample1 = json!({
            "day": "20260918",
            "stream_layout": "named",
            "stream": "default",
            "segment_key": "143000_120",
            "source": "mic",
            "sentence_id": 1,
            "version": "2026-09-18T00:00:00Z",
            "embedding_sha256": emb_hash,
            "audio_sha256": audio_hash,
        });
        let sample2 = json!({
            "day": "20260918",
            "stream_layout": "named",
            "stream": "default",
            "segment_key": "143000_120",
            "source": "mic",
            "sentence_id": 999, // Missing sentence
            "version": "2026-09-18T00:00:00Z",
            "embedding_sha256": emb_hash,
            "audio_sha256": audio_hash,
        });

        let hydrated = hydrate_owner_candidate_samples(root, &snapshot, &[sample1, sample2]);
        assert_eq!(hydrated.len(), 2);
        assert_eq!(hydrated[0]["evidence_state"], "ready");
        assert_eq!(hydrated[0]["artifact_eligible"], true);
        assert_eq!(hydrated[0]["text"], "valid text");

        assert_eq!(hydrated[1]["evidence_state"], "mismatch");
        assert_eq!(hydrated[1]["evidence_reason"], "embedding_missing");
        assert_eq!(hydrated[1]["artifact_eligible"], false);
    }
}
