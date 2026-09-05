// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Scheduled review suggestions from a locked candidate-pool snapshot.

use std::collections::BTreeMap;
use std::path::{Component, Path};

use serde_json::{Value, json};
use solstone_core_journal_io::SegmentLayout;

use crate::audio_sample::audio_info;
use crate::candidate_tracker::{
    CandidateProfile, CandidateTracker, eligible_for_pair_suggestion, source_segment_anchor,
};
use crate::speaker_candidate_pair_review_candidates::{
    CandidatePairSuggestion, record_candidate_pair,
};

/// Refresh review suggestions without merging candidates or changing identities.
pub fn refresh_candidate_pair_suggestions(
    journal: &Path,
) -> Result<Value, Box<dyn std::error::Error>> {
    let candidates = CandidateTracker::new(journal).snapshot_candidates_locked()?;
    let mut suggestions = Vec::new();
    for (i, left) in candidates.iter().enumerate() {
        for right in &candidates[i + 1..] {
            let score = left
                .centroid
                .iter()
                .zip(&right.centroid)
                .map(|(a, b)| a * b)
                .sum();
            if !eligible_for_pair_suggestion(left, right, score) {
                continue;
            }
            let (source_anchors, source_samples) = candidate_evidence(journal, left)?;
            let (target_anchors, target_samples) = candidate_evidence(journal, right)?;
            suggestions.push(CandidatePairSuggestion {
                source_anchors,
                target_anchors,
                similarity: score,
                source_intervals: left.n_intervals,
                target_intervals: right.n_intervals,
                source_samples,
                target_samples,
            });
        }
    }
    let (mut created, mut updated, mut suppressed) = (0, 0, 0);
    for suggestion in &suggestions {
        let (row, was_created, was_suppressed) = record_candidate_pair(journal, suggestion)?;
        if was_suppressed {
            suppressed += 1;
        } else if row.is_some() && was_created {
            created += 1;
        } else if row.is_some() {
            updated += 1;
        }
    }
    Ok(
        json!({"found":suggestions.len(),"created":created,"updated":updated,"suppressed":suppressed}),
    )
}

fn candidate_evidence(
    journal: &Path,
    candidate: &CandidateProfile,
) -> Result<(std::collections::BTreeSet<String>, Vec<Value>), Box<dyn std::error::Error>> {
    let invalid = || {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "speaker candidate has invalid source anchors",
        )
    };
    let sources = candidate
        .source_segments
        .iter()
        .map(|source| {
            source_segment_anchor(source)
                .map(|anchor| (anchor, source))
                .ok_or_else(invalid)
        })
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    if sources.is_empty() {
        return Err(invalid().into());
    }
    let mut samples = Vec::new();
    for source in sources.values() {
        let day = source["day"].as_str().expect("validated anchor");
        let stream = source["stream"].as_str().expect("validated anchor");
        let segment = source["segment_key"].as_str().expect("validated anchor");
        let audio_source = source["source"].as_str().expect("validated anchor");
        let mut components = Path::new(audio_source).components();
        if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
            return Err(invalid().into());
        }
        let directory = crate::segment_path(journal, day, segment, stream, false)?;
        let layout = if stream == "_default" {
            SegmentLayout::Direct
        } else {
            SegmentLayout::Named
        };
        let (url, _) = audio_info(&directory, day, stream, segment, audio_source, layout);
        if let Some(url) = url {
            samples.push(
                json!({"day":day,"stream":stream,"segment_key":segment,"source":audio_source,
                "cluster_label":source["cluster_label"],"audio_url":url}),
            );
        }
        if samples.len() == 3 {
            break;
        }
    }
    Ok((sources.into_keys().collect(), samples))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::speaker_candidate_pair_review_candidates::{
        accept_candidate, dismiss_candidate, load_candidates,
    };
    use std::fs;

    fn pool(root: &Path) -> Value {
        fs::create_dir_all(root.join("awareness")).unwrap();
        let candidates = [(1, vec![1.0, 0.0], "_default"), (2, vec![0.5, 0.8660254], "device")]
            .into_iter().map(|(id, centroid, stream)| {
                let sources = ["130000_300", "100000_300", "120000_300", "110000_300"].map(|segment| {
                    let dir = if stream == "_default" { root.join(format!("chronicle/20260808/{segment}")) }
                        else { root.join(format!("chronicle/20260808/{stream}/{segment}")) };
                    fs::create_dir_all(&dir).unwrap();
                    fs::write(dir.join("audio.flac"), b"test audio").unwrap();
                    json!({"day":"20260808","stream":stream,"segment_key":segment,"source":"audio","cluster_label":id})
                });
                json!({"cand_id":id,"centroid":centroid,"n_intervals":30,"status":"pending","source_segments":sources})
            }).collect::<Vec<_>>();
        let data = json!({"next_id":3,"candidates":candidates});
        fs::write(
            root.join("awareness/speaker_candidates.json"),
            data.to_string(),
        )
        .unwrap();
        data
    }

    #[test]
    fn refresh_records_stable_pair_and_three_ordered_samples_without_mutating_pool() {
        let root = tempfile::tempdir().unwrap();
        pool(root.path());
        let path = root.path().join("awareness/speaker_candidates.json");
        let before = fs::read(&path).unwrap();
        let report = refresh_candidate_pair_suggestions(root.path()).unwrap();
        assert_eq!(
            report,
            json!({"found":1,"created":1,"updated":0,"suppressed":0})
        );
        let row = &load_candidates(root.path()).unwrap()[0];
        let source = row["evidence"]["source_samples"].as_array().unwrap();
        assert_eq!(source.len(), 3);
        assert_eq!(source[0]["segment_key"], "100000_300");
        assert_eq!(source[2]["segment_key"], "120000_300");
        assert_eq!(
            source[0]["audio_url"],
            "/app/speakers/api/serve_audio/20260808/100000_300/audio.flac"
        );
        assert_eq!(
            row["evidence"]["target_samples"][0]["audio_url"],
            "/app/speakers/api/serve_audio/20260808/device/100000_300/audio.flac"
        );
        assert!(!root.path().join("chronicle/20260808/_default").exists());
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(
            refresh_candidate_pair_suggestions(root.path()).unwrap()["updated"],
            1
        );
        assert_eq!(load_candidates(root.path()).unwrap().len(), 1);
    }

    #[test]
    fn refresh_preserves_accepted_status_and_first_timestamp() {
        let root = tempfile::tempdir().unwrap();
        pool(root.path());
        refresh_candidate_pair_suggestions(root.path()).unwrap();
        let row = load_candidates(root.path()).unwrap().remove(0);
        accept_candidate(
            root.path(),
            row["anchor_a"].as_str().unwrap(),
            row["anchor_b"].as_str().unwrap(),
        )
        .unwrap();
        refresh_candidate_pair_suggestions(root.path()).unwrap();
        let after = load_candidates(root.path()).unwrap().remove(0);
        assert_eq!(after["status"], "accepted");
        assert_eq!(after["first_surfaced"], row["first_surfaced"]);
    }

    #[test]
    fn dismissed_anchors_suppress_even_when_a_new_source_changes_canonical_anchor() {
        let root = tempfile::tempdir().unwrap();
        let mut data = pool(root.path());
        refresh_candidate_pair_suggestions(root.path()).unwrap();
        let row = load_candidates(root.path()).unwrap().remove(0);
        dismiss_candidate(
            root.path(),
            row["anchor_b"].as_str().unwrap(),
            row["anchor_a"].as_str().unwrap(),
        )
        .unwrap();
        let path = root
            .path()
            .join("speakers/candidate-pair-review-candidates.jsonl");
        let before = fs::read(&path).unwrap();
        data["candidates"][0]["source_segments"].as_array_mut().unwrap().push(json!({"day":"20260807","stream":"_default","segment_key":"090000_300","source":"audio","cluster_label":4}));
        fs::write(
            root.path().join("awareness/speaker_candidates.json"),
            data.to_string(),
        )
        .unwrap();
        assert_eq!(
            refresh_candidate_pair_suggestions(root.path()).unwrap(),
            json!({"found":1,"created":0,"updated":0,"suppressed":1})
        );
        assert_eq!(fs::read(&path).unwrap(), before);
    }

    #[test]
    fn malformed_review_rows_and_pool_are_refused_without_rewrite() {
        let root = tempfile::tempdir().unwrap();
        pool(root.path());
        fs::create_dir_all(root.path().join("speakers")).unwrap();
        let store = root
            .path()
            .join("speakers/candidate-pair-review-candidates.jsonl");
        for bytes in [b"{bad\n".as_slice(), b"42\n"] {
            fs::write(&store, bytes).unwrap();
            assert!(refresh_candidate_pair_suggestions(root.path()).is_err());
            assert_eq!(fs::read(&store).unwrap(), bytes);
        }
        fs::write(
            root.path().join("awareness/speaker_candidates.json"),
            "{bad",
        )
        .unwrap();
        assert!(refresh_candidate_pair_suggestions(root.path()).is_err());
        assert_eq!(fs::read(&store).unwrap(), b"42\n");
    }

    #[test]
    fn locked_snapshot_does_not_reuse_a_removed_or_malformed_pool() {
        let root = tempfile::tempdir().unwrap();
        pool(root.path());
        let mut tracker = CandidateTracker::new(root.path());
        let path = root.path().join("awareness/speaker_candidates.json");
        fs::remove_file(&path).unwrap();
        assert!(tracker.snapshot_candidates_locked().unwrap().is_empty());
        fs::write(&path, "{bad").unwrap();
        assert!(tracker.snapshot_candidates_locked().is_err());
        assert_eq!(fs::read(&path).unwrap(), b"{bad");
    }
}
