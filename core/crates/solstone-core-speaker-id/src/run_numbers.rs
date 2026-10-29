// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Speaker numbers that hold through one continuous recording.
//!
//! Diarization numbers speakers afresh in every segment, so "Speaker 1" in one
//! segment and "Speaker 1" in the next are unrelated. Within a run, the
//! segments of one stream and audio source no more than a minute apart, each
//! segment's speakers are matched one to one against the voices already heard
//! in the run. A speaker keeps that voice's number when their embeddings agree,
//! and anything else takes the next number. Nothing is written: the numbers are
//! recomputed from the embeddings on every read, and a run never crosses a gap,
//! a day or a stream.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::Path;

use serde_json::Value;

use crate::embeddings::load_embeddings_file;
use crate::transcript::read_transcript_rows;

/// The longest silence, in seconds, between two segments of one run.
pub const RUN_GAP_S: i64 = 60;
/// The cosine at which a speaker is the same voice as one heard earlier in the
/// run; the speaker pool merges voices at the same value.
pub const RUN_MATCH: f64 = 0.72;

struct Cluster {
    speaker: i64,
    statements: f64,
    centroid: Vec<f64>,
}

struct RunSegment {
    key: String,
    start: i64,
    clusters: Vec<Cluster>,
}

/// This segment's diarization numbers mapped to their numbers in its run, for
/// one audio source. Empty when the segment has no usable embeddings.
#[must_use]
pub fn run_speaker_numbers(segment: &Path, source: &str) -> BTreeMap<i64, i64> {
    let (Some(stream_dir), Some(target)) = (
        segment.parent(),
        segment.file_name().and_then(|name| name.to_str()),
    ) else {
        return BTreeMap::new();
    };
    let Some((target_start, _)) = segment_times(target) else {
        return BTreeMap::new();
    };
    let Ok(entries) = fs::read_dir(stream_dir) else {
        return BTreeMap::new();
    };
    let mut keys = entries
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter_map(|key| Some((segment_times(&key)?, key)))
        .filter(|((start, _), _)| *start <= target_start)
        .collect::<Vec<_>>();
    keys.sort();

    // Walk back from this segment to the start of its run.
    let mut run: Vec<RunSegment> = Vec::new();
    for ((start, end), key) in keys.into_iter().rev() {
        let Some(clusters) = segment_clusters(&stream_dir.join(&key), source) else {
            continue;
        };
        if let Some(next) = run.last()
            && next.start - end > RUN_GAP_S
        {
            break;
        }
        if run.is_empty() && key != target {
            return BTreeMap::new();
        }
        run.push(RunSegment {
            key,
            start,
            clusters,
        });
    }
    run.reverse();
    number_run(&run).remove(target).unwrap_or_default()
}

/// Run numbers, from 1, for every segment of one run in time order.
fn number_run(run: &[RunSegment]) -> HashMap<String, BTreeMap<i64, i64>> {
    let mut voices: Vec<Vec<f64>> = Vec::new();
    let mut numbered = HashMap::new();
    for segment in run {
        let candidates = voices.iter().map(|sum| unit(sum)).collect::<Vec<_>>();
        let mut pairs = Vec::new();
        for (i, cluster) in segment.clusters.iter().enumerate() {
            for (j, voice) in candidates.iter().enumerate() {
                if let Some(voice) = voice {
                    let score = dot(&cluster.centroid, voice);
                    if score >= RUN_MATCH {
                        pairs.push((score, i, j));
                    }
                }
            }
        }
        pairs.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
        let mut matched: HashMap<usize, usize> = HashMap::new();
        let mut taken = vec![false; voices.len()];
        for (_, i, j) in pairs {
            if !matched.contains_key(&i) && !taken[j] {
                matched.insert(i, j);
                taken[j] = true;
            }
        }
        let mut numbers = BTreeMap::new();
        for (i, cluster) in segment.clusters.iter().enumerate() {
            let voice = match matched.get(&i) {
                Some(&j) => j,
                None => {
                    voices.push(vec![0.0; cluster.centroid.len()]);
                    voices.len() - 1
                }
            };
            for (sum, value) in voices[voice].iter_mut().zip(&cluster.centroid) {
                *sum += value * cluster.statements;
            }
            numbers.insert(cluster.speaker, voice as i64 + 1);
        }
        numbered.insert(segment.key.clone(), numbers);
    }
    numbered
}

/// Each numbered speaker's statements and centroid, in order of first
/// statement. `None` when the source has no transcript, no embeddings or no
/// numbered speaker with an embedded statement.
fn segment_clusters(segment: &Path, source: &str) -> Option<Vec<Cluster>> {
    let bytes = fs::read(segment.join(format!("{source}.jsonl"))).ok()?;
    let rows = read_transcript_rows(&bytes).ok()?.rows;
    let speakers = rows
        .iter()
        .filter_map(|row| match row.value.get("speaker") {
            Some(Value::Number(number)) => Some((row.sentence_id, number.as_i64()?)),
            _ => None,
        })
        .collect::<HashMap<_, _>>();
    if speakers.is_empty() {
        return None;
    }
    let embeddings = load_embeddings_file(&segment.join(format!("{source}.npz")))
        .ok()
        .flatten()?;
    let mut order = Vec::new();
    let mut sums: HashMap<i64, (f64, Vec<f64>)> = HashMap::new();
    for (sentence_id, embedding) in &embeddings.statements {
        let Some(&speaker) = speakers.get(sentence_id) else {
            continue;
        };
        let row = embedding
            .iter()
            .map(|value| f64::from(*value))
            .collect::<Vec<_>>();
        let Some(row) = unit(&row) else {
            continue;
        };
        let (count, sum) = sums.entry(speaker).or_insert_with(|| {
            order.push(speaker);
            (0.0, vec![0.0; row.len()])
        });
        *count += 1.0;
        for (total, value) in sum.iter_mut().zip(&row) {
            *total += value;
        }
    }
    let clusters = order
        .into_iter()
        .filter_map(|speaker| {
            let (statements, sum) = sums.remove(&speaker)?;
            Some(Cluster {
                speaker,
                statements,
                centroid: unit(&sum)?,
            })
        })
        .collect::<Vec<_>>();
    (!clusters.is_empty()).then_some(clusters)
}

/// A segment key's start and end, in seconds of the day: `HHMMSS_LEN`.
fn segment_times(key: &str) -> Option<(i64, i64)> {
    let (clock, length) = key.split_once('_')?;
    if clock.len() != 6
        || !clock.bytes().all(|byte| byte.is_ascii_digit())
        || length.is_empty()
        || !length.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let part = |range: std::ops::Range<usize>| clock[range].parse::<i64>().ok();
    let start = part(0..2)? * 3600 + part(2..4)? * 60 + part(4..6)?;
    Some((start, start + length.parse::<i64>().ok()?))
}

fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn unit(vector: &[f64]) -> Option<Vec<f64>> {
    let norm = dot(vector, vector).sqrt();
    (norm > 0.0 && norm.is_finite()).then(|| vector.iter().map(|value| value / norm).collect())
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use solstone_core_npy::write_npy;
    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, ZipWriter};

    use super::*;

    /// A voice: one of four directions in the 256-wide embedding, with a little
    /// per-line noise across those four.
    fn voice(axis: usize, noise: f32) -> Vec<f32> {
        let mut row = vec![0.0; 256];
        row[..4].fill(noise);
        row[axis] = 1.0;
        row
    }

    /// One segment's transcript and embeddings: `(speaker, embedding)` per line.
    fn segment(root: &Path, key: &str, lines: &[(i64, Vec<f32>)]) {
        let dir = root.join(key);
        fs::create_dir_all(&dir).unwrap();
        let mut jsonl = String::from("{\"raw\":\"audio.m4a\"}\n");
        for (index, (speaker, _)) in lines.iter().enumerate() {
            jsonl.push_str(&format!(
                "{{\"start\":\"00:00:{index:02}\",\"speaker\":{speaker},\"text\":\"line\"}}\n"
            ));
        }
        fs::write(dir.join("audio.jsonl"), jsonl).unwrap();
        let mut payload = Vec::new();
        let mut ids = Vec::new();
        for (index, (_, embedding)) in lines.iter().enumerate() {
            for value in embedding {
                payload.extend_from_slice(&value.to_le_bytes());
            }
            ids.extend_from_slice(&(index as i32 + 1).to_le_bytes());
        }
        let mut zip = ZipWriter::new(fs::File::create(dir.join("audio.npz")).unwrap());
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        zip.start_file("embeddings.npy", options).unwrap();
        zip.write_all(&write_npy(
            "<f4",
            &format!("({}, 256)", lines.len()),
            &payload,
        ))
        .unwrap();
        zip.start_file("statement_ids.npy", options).unwrap();
        zip.write_all(&write_npy("<i4", &format!("({},)", lines.len()), &ids))
            .unwrap();
        zip.finish().unwrap();
    }

    #[test]
    fn a_voice_keeps_its_number_through_a_recording() {
        let root = tempfile::tempdir().unwrap();
        // Diarization calls the owner 2 and a guest 1 in the first segment,
        // then swaps them in the second; a third voice joins in the third.
        segment(
            root.path(),
            "100000_300",
            &[(2, voice(0, 0.1)), (1, voice(1, 0.1))],
        );
        segment(
            root.path(),
            "100500_300",
            &[(1, voice(0, 0.2)), (2, voice(1, 0.0))],
        );
        segment(
            root.path(),
            "101000_300",
            &[(3, voice(2, 0.0)), (1, voice(1, 0.1)), (2, voice(0, 0.1))],
        );

        let first = run_speaker_numbers(&root.path().join("100000_300"), "audio");
        let second = run_speaker_numbers(&root.path().join("100500_300"), "audio");
        let third = run_speaker_numbers(&root.path().join("101000_300"), "audio");
        assert_eq!(first, BTreeMap::from([(2, 1), (1, 2)]));
        assert_eq!(second, BTreeMap::from([(1, 1), (2, 2)]));
        assert_eq!(third, BTreeMap::from([(3, 3), (1, 2), (2, 1)]));
    }

    #[test]
    fn a_pause_or_a_different_voice_starts_a_new_number() {
        let root = tempfile::tempdir().unwrap();
        segment(root.path(), "100000_300", &[(1, voice(0, 0.0))]);
        // A different voice right after: no match, so the next number.
        segment(root.path(), "100500_300", &[(1, voice(1, 0.0))]);
        // The first voice again after more than a minute of silence: a new run.
        segment(root.path(), "102000_300", &[(1, voice(0, 0.0))]);

        assert_eq!(
            run_speaker_numbers(&root.path().join("100500_300"), "audio"),
            BTreeMap::from([(1, 2)])
        );
        assert_eq!(
            run_speaker_numbers(&root.path().join("102000_300"), "audio"),
            BTreeMap::from([(1, 1)])
        );
    }

    #[test]
    fn a_segment_without_embeddings_has_no_run_numbers() {
        let root = tempfile::tempdir().unwrap();
        segment(root.path(), "100000_300", &[(1, voice(0, 0.0))]);
        let bare = root.path().join("100500_300");
        fs::create_dir_all(&bare).unwrap();
        fs::write(
            bare.join("audio.jsonl"),
            "{\"raw\":\"audio.m4a\"}\n{\"start\":\"00:00:00\",\"speaker\":1,\"text\":\"x\"}\n",
        )
        .unwrap();
        assert!(run_speaker_numbers(&bare, "audio").is_empty());
        assert!(run_speaker_numbers(&root.path().join("nope_1"), "audio").is_empty());
    }
}
