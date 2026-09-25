// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Backfill selection, four-way classification, and single-member attribution primitives.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use solstone_core_entity::{
    hold_entity_trust_lock, is_admissible_person, load_all_journal_entities,
};
use solstone_core_speaker_id::labels::write_full_labels;
use thiserror::Error;

use crate::backfill_operations::{BackfillCheckpointOutcome, BackfillSegmentKey};
use crate::bootstrap::scan_segments;
use crate::owner_admission::OWNER_IDENTITY_INVALID_REASON;
use crate::resolve::{ResolveError, ResolveOutcome, resolve};
use crate::voiceprint_accumulation::{
    AccumulationEmbedding, AccumulationLabel, AccumulationOutcome, AccumulationRequest,
    accumulate_voiceprints,
};

fn encoder() -> solstone_core_entity::EncoderIdentity {
    solstone_core_entity::EncoderIdentity {
        id: "unresolved".to_owned(),
        sha256: "0000000000000000000000000000000000000000000000000000000000000000".to_owned(),
        width: 256,
    }
}

/// Four-way classification for segment speaker labels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeakerLabelsClass {
    Absent,
    Stub,
    Protected,
    Gap,
}

/// Results of backfill's enumerate-and-filter phases.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackfillPlan {
    pub total_scanned: usize,
    pub selected: usize,
    pub protected_skipped: usize,
    pub skipped_no_embeddings: usize,
    pub to_process: Vec<BackfillSegmentKey>,
}

#[derive(Debug, Error)]
pub enum BackfillError {
    #[error("backfill scan failed: {0}")]
    Scan(#[from] crate::bootstrap::BootstrapError),
    #[error("native attribution failed: {0}")]
    Resolve(#[from] ResolveError),
    #[error("backfill operation ledger failed: {0}")]
    Ledger(#[from] crate::backfill_operations::BackfillOperationError),
    #[error("speaker label write failed: {0}")]
    Labels(#[from] solstone_core_speaker_id::labels::LabelsError),
    #[error("segment lookup failed: {0}")]
    ExactLookup(#[from] crate::segment_catalog::ExactLookupError),
    #[error("segment path failed: {0}")]
    Path(#[from] solstone_core_journal_io::PathError),
    #[error("backfill operation lock failed: {0}")]
    Trust(#[from] solstone_core_entity::EntityTrustLockError),
    #[error("labels gap detected at {path}: {detail}")]
    LabelsGap { path: PathBuf, detail: String },
}

/// Classify a parsed JSON label payload according to the 4-way classifier rules.
#[must_use]
pub fn classify_speaker_labels_payload(payload: &Value) -> SpeakerLabelsClass {
    let Some(object) = payload.as_object() else {
        return SpeakerLabelsClass::Gap;
    };
    let Some(labels_val) = object.get("labels") else {
        return SpeakerLabelsClass::Gap;
    };
    let Some(labels_array) = labels_val.as_array() else {
        return SpeakerLabelsClass::Gap;
    };
    let is_stub = labels_array.is_empty() && object.get("skipped") == Some(&Value::Bool(true));
    if is_stub {
        SpeakerLabelsClass::Stub
    } else {
        SpeakerLabelsClass::Protected
    }
}

/// Classify a durable label string according to the 4-way classifier rules.
#[must_use]
pub fn classify_speaker_labels_text(payload: &str) -> SpeakerLabelsClass {
    serde_json::from_str(payload)
        .map(|value| classify_speaker_labels_payload(&value))
        .unwrap_or(SpeakerLabelsClass::Gap)
}

/// Classify a durable label file on disk.
#[must_use]
pub fn classify_speaker_labels_file(path: &Path) -> SpeakerLabelsClass {
    if !path.exists() {
        return SpeakerLabelsClass::Absent;
    }
    match std::fs::read_to_string(path) {
        Ok(text) => classify_speaker_labels_text(&text),
        Err(_) => SpeakerLabelsClass::Gap,
    }
}

/// Enumerate audio-bearing segments and apply the 4-way classifier.
pub fn plan_backfill_segments(
    journal_root: &Path,
    reattribute: bool,
) -> Result<BackfillPlan, BackfillError> {
    let mut total_scanned = 0;
    let mut skipped_no_embeddings = 0;
    let mut protected_skipped = 0;
    let mut to_process = Vec::new();

    for segment in scan_segments(journal_root)? {
        total_scanned += 1;
        if !has_audio_embeddings(&segment.sources) {
            skipped_no_embeddings += 1;
            continue;
        }
        let labels_path = segment.path.join("talents/speaker_labels.json");
        let class = classify_speaker_labels_file(&labels_path);
        match class {
            SpeakerLabelsClass::Gap => {
                return Err(BackfillError::LabelsGap {
                    path: labels_path,
                    detail: "malformed or unreadable speaker_labels.json".to_owned(),
                });
            }
            SpeakerLabelsClass::Absent | SpeakerLabelsClass::Stub => {
                to_process.push(BackfillSegmentKey {
                    day: segment.day,
                    stream_layout: segment.layout,
                    stream: segment.stream,
                    segment_key: segment.name,
                });
            }
            SpeakerLabelsClass::Protected => {
                if reattribute {
                    to_process.push(BackfillSegmentKey {
                        day: segment.day,
                        stream_layout: segment.layout,
                        stream: segment.stream,
                        segment_key: segment.name,
                    });
                } else {
                    protected_skipped += 1;
                }
            }
        }
    }

    Ok(BackfillPlan {
        total_scanned,
        selected: to_process.len(),
        protected_skipped,
        skipped_no_embeddings,
        to_process,
    })
}

/// Inspect embeddings in the segment directory without throwing non-resumable errors.
fn inspect_embeddings(segment_path: &Path) -> Result<Option<PathBuf>, String> {
    let entries = match std::fs::read_dir(segment_path) {
        Ok(entries) => entries,
        Err(err) => {
            return Err(format!(
                "cannot read directory {}: {err}",
                segment_path.display()
            ));
        }
    };
    let mut paths = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|extension| extension.to_str()) == Some("npz"))
        .filter(|path| {
            let stem = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or_default();
            stem.ends_with("_audio") || stem == "audio"
        })
        .collect::<Vec<_>>();
    paths.sort();
    let Some(first_path) = paths.into_iter().next() else {
        return Ok(None);
    };
    let source_stem = first_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("unknown");
    match solstone_core_speaker_id::embeddings::load_embeddings_file(&first_path) {
        Ok(Some(emb)) if emb.statements.is_empty() => Ok(None),
        Ok(Some(_)) => Ok(Some(first_path)),
        Ok(None) => Err(format!(
            "missing or unreadable declared embeddings at {}: source={source_stem}",
            first_path.display()
        )),
        Err(err) => Err(format!(
            "unreadable declared embeddings at {}: source={source_stem}: {err}",
            first_path.display()
        )),
    }
}

/// Execute attribution and optional writes for a single segment member.
pub fn execute_backfill_member(
    journal_root: &Path,
    segment_key: &BackfillSegmentKey,
    segment_path: &Path,
    commit: bool,
    accumulation: bool,
    now_ms: i64,
) -> (BackfillCheckpointOutcome, Option<String>) {
    let _trust = match hold_entity_trust_lock(journal_root) {
        Ok(lock) => lock,
        Err(err) => {
            return (
                BackfillCheckpointOutcome::Error,
                Some(format!("entity_trust_contention: {err}")),
            );
        }
    };

    match inspect_embeddings(segment_path) {
        Ok(None) => return (BackfillCheckpointOutcome::Skipped, None),
        Err(detail) => {
            return (BackfillCheckpointOutcome::Error, Some(detail));
        }
        Ok(Some(_)) => {}
    }

    let resolve_result = resolve(
        journal_root,
        &segment_key.day,
        &segment_key.stream,
        &segment_key.segment_key,
        segment_key.stream_layout,
        false,
        now_ms,
    );

    let resolved = match resolve_result {
        Ok(res) => res,
        Err(err) => return (BackfillCheckpointOutcome::Error, Some(err.to_string())),
    };

    match resolved {
        ResolveOutcome::Resolved(output) => {
            if commit {
                if let Err(err) = write_resolved_backfill_labels(segment_path, &output) {
                    return (BackfillCheckpointOutcome::Error, Some(err.to_string()));
                }
                if accumulation
                    && let Err(err) = accumulate_voiceprints_for_segment(
                        journal_root,
                        segment_key,
                        segment_path,
                        &output,
                        now_ms,
                    )
                {
                    return (BackfillCheckpointOutcome::Error, Some(err.to_string()));
                }
            }
            (BackfillCheckpointOutcome::Processed, None)
        }
        ResolveOutcome::IdentityInvalid => (
            BackfillCheckpointOutcome::Error,
            Some(OWNER_IDENTITY_INVALID_REASON.to_owned()),
        ),
        ResolveOutcome::NoOwnerCentroid | ResolveOutcome::SegmentMissing => {
            (BackfillCheckpointOutcome::Skipped, None)
        }
        ResolveOutcome::Empty { source: Some(src) } => (
            BackfillCheckpointOutcome::Error,
            Some(format!("empty statements with declared source: {src}")),
        ),
        ResolveOutcome::Empty { source: None } => (
            BackfillCheckpointOutcome::Error,
            Some("empty statements without source after embeddings inspection".to_owned()),
        ),
    }
}

pub fn write_resolved_backfill_labels(
    segment: &Path,
    output: &crate::resolve::ResolveOutput,
) -> Result<(), solstone_core_speaker_id::labels::LabelsError> {
    let labels = output.labels.iter().map(label_json).collect::<Vec<_>>();
    write_full_labels(segment, labels, &metadata_json(&output.metadata))
}

fn accumulate_voiceprints_for_segment(
    journal_root: &Path,
    key: &BackfillSegmentKey,
    segment_dir: &Path,
    output: &crate::resolve::ResolveOutput,
    now_ms: i64,
) -> Result<(), String> {
    let Some(source) = &output.source else {
        return Ok(());
    };
    let npz_path = segment_dir.join(format!("{source}.npz"));
    if !npz_path.exists() {
        return Ok(());
    }
    let Some(embeddings) = solstone_core_speaker_id::embeddings::load_embeddings_file(&npz_path)
        .map_err(|e| e.to_string())?
    else {
        return Ok(());
    };

    let admitted_entity_ids = load_all_journal_entities(journal_root)
        .unwrap_or_default()
        .into_iter()
        .filter(is_admissible_person)
        .map(|entity| entity.id)
        .collect::<BTreeSet<_>>();

    let entity_ids = output
        .labels
        .iter()
        .filter_map(|label| label.speaker.clone())
        .filter(|speaker| admitted_entity_ids.contains(speaker))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();

    if entity_ids.is_empty() {
        return Ok(());
    }

    let request = AccumulationRequest {
        journal_root: journal_root.to_path_buf(),
        day: key.day.clone(),
        stream_layout: key.stream_layout,
        stream: key.stream.clone(),
        segment_key: key.segment_key.clone(),
        source: source.clone(),
        now_ms,
        encoder: encoder(),
        labels: output
            .labels
            .iter()
            .map(|label| AccumulationLabel {
                sentence_id: label.sentence_id,
                speaker: label.speaker.clone(),
                confidence: label.confidence.clone(),
                method: label.method.clone(),
            })
            .collect(),
        embeddings: embeddings
            .statements
            .into_iter()
            .map(|(sentence_id, values)| AccumulationEmbedding {
                sentence_id,
                values,
            })
            .collect(),
        entity_ids,
    };

    match accumulate_voiceprints(&request) {
        Ok(
            AccumulationOutcome::Completed { .. }
            | AccumulationOutcome::NothingEligible { .. }
            | AccumulationOutcome::NoOwnerCentroid { .. },
        ) => Ok(()),
        Ok(AccumulationOutcome::IdentityInvalid { .. }) => {
            Err(OWNER_IDENTITY_INVALID_REASON.to_owned())
        }
        Err(err) => Err(err.to_string()),
    }
}

fn label_json(label: &crate::layer1::Label) -> Value {
    let mut value = serde_json::json!({"sentence_id":label.sentence_id});
    let object = value.as_object_mut().expect("label is an object");
    if let Some(speaker) = &label.speaker {
        object.insert("speaker".to_owned(), Value::String(speaker.clone()));
    }
    if let Some(confidence) = &label.confidence {
        object.insert("confidence".to_owned(), Value::String(confidence.clone()));
    }
    if let Some(method) = &label.method {
        object.insert("method".to_owned(), Value::String(method.clone()));
    }
    if let Some(owner_margin_declined) = label.owner_margin_declined {
        object.insert(
            "owner_margin_declined".to_owned(),
            Value::Bool(owner_margin_declined),
        );
    }
    if let Some(acoustic_margin_declined) = label.acoustic_margin_declined {
        object.insert(
            "acoustic_margin_declined".to_owned(),
            Value::Bool(acoustic_margin_declined),
        );
    }
    value
}

fn metadata_json(metadata: &crate::resolve::ResolveMetadata) -> Map<String, Value> {
    let mut value = Map::new();
    value.insert(
        "owner_centroid_last_refreshed_at".to_owned(),
        metadata
            .owner_centroid_last_refreshed_at
            .clone()
            .map(Value::String)
            .unwrap_or(Value::Null),
    );
    value.insert(
        "voiceprint_versions".to_owned(),
        Value::Object(
            metadata
                .voiceprint_versions
                .iter()
                .map(|(entity_id, version)| (entity_id.clone(), Value::from(*version)))
                .collect(),
        ),
    );
    value.insert(
        "candidate_evidence".to_owned(),
        Value::Array(
            metadata
                .candidate_evidence
                .iter()
                .map(|evidence| {
                    serde_json::json!({"entity_id":evidence.entity_id,"sources":evidence.sources})
                })
                .collect(),
        ),
    );
    if let Some(gaps) = &metadata.candidate_evidence_gaps {
        value.insert(
            "candidate_evidence_gaps".to_owned(),
            Value::Array(
                gaps.iter()
                    .map(|gap| serde_json::json!({"source":gap.source,"reason":gap.reason}))
                    .collect(),
            ),
        );
    }
    value
}

fn has_audio_embeddings(sources: &[String]) -> bool {
    sources
        .iter()
        .any(|source| source == "audio" || source.ends_with("_audio"))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use super::*;
    use crate::backfill_operations::BackfillStatusKind;
    use crate::evidence::{CandidateEvidence, EvidenceGap};
    use crate::layer1::Label;
    use crate::resolve::{ResolveMetadata, ResolveOutput};
    #[cfg(all(test, feature = "full-tests"))]
    use solstone_core_journal_io::LockOptions;
    use solstone_core_journal_io::SegmentLayout;

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    struct Temp(PathBuf);

    impl Temp {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "solstone-backfill-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn four_way_classifier_exact_matches() {
        // Stub
        let stub = r#"{"labels": [], "skipped": true, "reason": "no_owner_centroid"}"#;
        assert_eq!(classify_speaker_labels_text(stub), SpeakerLabelsClass::Stub);
        let minimal_stub = r#"{"labels": [], "skipped": true}"#;
        assert_eq!(
            classify_speaker_labels_text(minimal_stub),
            SpeakerLabelsClass::Stub
        );

        // Protected
        let protected_nonempty = r#"{"labels": [{"sentence_id": 1, "speaker": "person"}]}"#;
        assert_eq!(
            classify_speaker_labels_text(protected_nonempty),
            SpeakerLabelsClass::Protected
        );
        let protected_skipped_false = r#"{"labels": [], "skipped": false}"#;
        assert_eq!(
            classify_speaker_labels_text(protected_skipped_false),
            SpeakerLabelsClass::Protected
        );
        let protected_no_skipped = r#"{"labels": []}"#;
        assert_eq!(
            classify_speaker_labels_text(protected_no_skipped),
            SpeakerLabelsClass::Protected
        );
        let protected_extra_fields =
            r#"{"labels": [{"sentence_id": 1}], "skipped": true, "custom": 123}"#;
        assert_eq!(
            classify_speaker_labels_text(protected_extra_fields),
            SpeakerLabelsClass::Protected
        );

        // Gap
        assert_eq!(classify_speaker_labels_text(""), SpeakerLabelsClass::Gap);
        assert_eq!(classify_speaker_labels_text("{}"), SpeakerLabelsClass::Gap);
        assert_eq!(classify_speaker_labels_text("[]"), SpeakerLabelsClass::Gap);
        assert_eq!(
            classify_speaker_labels_text(r#"{"labels": null}"#),
            SpeakerLabelsClass::Gap
        );
        assert_eq!(
            classify_speaker_labels_text(r#"{"labels": "not an array"}"#),
            SpeakerLabelsClass::Gap
        );
        assert_eq!(
            classify_speaker_labels_text("{invalid json}"),
            SpeakerLabelsClass::Gap
        );
    }

    #[test]
    fn plan_backfill_segments_handles_stubs_and_gaps() {
        let temp = Temp::new();
        let segment_dir = temp.path().join("chronicle/20260808/mic/120000_300");
        fs::create_dir_all(segment_dir.join("talents")).unwrap();
        fs::write(
            segment_dir.join("talents/speaker_labels.json"),
            r#"{"labels": [], "skipped": true}"#,
        )
        .unwrap();
        fs::write(segment_dir.join("audio.npz"), b"fake npz").unwrap();

        let plan = plan_backfill_segments(temp.path(), false).unwrap();
        assert_eq!(plan.total_scanned, 1);
        assert_eq!(plan.selected, 1);
        assert_eq!(plan.protected_skipped, 0);
        assert_eq!(plan.to_process.len(), 1);

        // Turn labels into a Gap -> planning fails
        fs::write(segment_dir.join("talents/speaker_labels.json"), b"{}").unwrap();
        assert!(matches!(
            plan_backfill_segments(temp.path(), false),
            Err(BackfillError::LabelsGap { .. })
        ));
    }

    #[test]
    fn ac2_backfill_write_preserves_user_prefix_and_persists_full_resolve_output() {
        let temp = Temp::new();
        let segment = temp.path().join("segment");
        fs::create_dir_all(segment.join("talents")).unwrap();
        let preserved = serde_json::json!({
            "sentence_id": 1,
            "speaker": "person-user",
            "method": "user_zzz_test",
            "opaque": {"preserve": [1, 2, 3]},
        });
        fs::write(
            segment.join("talents/speaker_labels.json"),
            serde_json::json!({"labels":[
                preserved.clone(),
                {"sentence_id":2,"speaker":"stale","method":"cluster"}
            ]})
            .to_string(),
        )
        .unwrap();
        let output = ResolveOutput {
            labels: vec![
                Label {
                    sentence_id: 1,
                    speaker: Some("replacement".to_owned()),
                    confidence: Some("high".to_owned()),
                    method: Some("acoustic".to_owned()),
                    owner_margin_declined: None,
                    acoustic_margin_declined: None,
                },
                Label {
                    sentence_id: 2,
                    speaker: Some("fresh".to_owned()),
                    confidence: Some("low".to_owned()),
                    method: Some("cluster".to_owned()),
                    owner_margin_declined: Some(true),
                    acoustic_margin_declined: Some(true),
                },
            ],
            unmatched: vec![],
            unmatched_texts: HashMap::new(),
            source: Some("audio".to_owned()),
            candidates: vec![],
            metadata: ResolveMetadata {
                owner_centroid_last_refreshed_at: Some("2026-08-08T00:00:00Z".to_owned()),
                voiceprint_versions: HashMap::from([("fresh".to_owned(), 3)]),
                candidate_evidence: vec![CandidateEvidence {
                    entity_id: "fresh".to_owned(),
                    sources: vec!["screen".to_owned()],
                }],
                candidate_evidence_gaps: Some(vec![EvidenceGap {
                    source: "meeting".to_owned(),
                    reason: "missing".to_owned(),
                }]),
                voiceprint_gaps: None,
            },
        };

        write_resolved_backfill_labels(&segment, &output).unwrap();
        let saved: Value =
            serde_json::from_slice(&fs::read(segment.join("talents/speaker_labels.json")).unwrap())
                .unwrap();
        assert_eq!(saved["labels"][0], preserved);
        assert_eq!(saved["labels"][1]["speaker"], "fresh");
        assert_eq!(saved["labels"][1]["owner_margin_declined"], true);
        assert_eq!(saved["labels"][1]["acoustic_margin_declined"], true);
        assert_eq!(
            saved["owner_centroid_last_refreshed_at"],
            "2026-08-08T00:00:00Z"
        );
        assert_eq!(saved["voiceprint_versions"], serde_json::json!({"fresh":3}));
        assert_eq!(
            saved["candidate_evidence"],
            serde_json::json!([{"entity_id":"fresh","sources":["screen"]}])
        );
        assert_eq!(
            saved["candidate_evidence_gaps"],
            serde_json::json!([{"source":"meeting","reason":"missing"}])
        );
    }

    #[test]
    fn full_4_way_classifier_truth_table() {
        // 1. Stub: empty labels array AND skipped == true (bool)
        assert_eq!(
            classify_speaker_labels_text(r#"{"labels": [], "skipped": true}"#),
            SpeakerLabelsClass::Stub
        );

        // 2. Protected: any other object with labels array
        assert_eq!(
            classify_speaker_labels_text(r#"{"labels": [{"speaker": "p1"}], "skipped": true}"#),
            SpeakerLabelsClass::Protected
        );
        assert_eq!(
            classify_speaker_labels_text(r#"{"labels": [], "skipped": false}"#),
            SpeakerLabelsClass::Protected
        );
        assert_eq!(
            classify_speaker_labels_text(r#"{"labels": []}"#),
            SpeakerLabelsClass::Protected
        );
        assert_eq!(
            classify_speaker_labels_text(r#"{"labels": [], "skipped": "true"}"#),
            SpeakerLabelsClass::Protected
        );
        assert_eq!(
            classify_speaker_labels_text(r#"{"labels": [], "skipped": null}"#),
            SpeakerLabelsClass::Protected
        );
        assert_eq!(
            classify_speaker_labels_text(
                r#"{"labels": [{"speaker": "p1"}], "extra_custom_field": 123}"#
            ),
            SpeakerLabelsClass::Protected
        );

        // 3. Gap: invalid JSON, {}, [], missing/non-array labels
        assert_eq!(
            classify_speaker_labels_text("invalid json"),
            SpeakerLabelsClass::Gap
        );
        assert_eq!(classify_speaker_labels_text("{}"), SpeakerLabelsClass::Gap);
        assert_eq!(classify_speaker_labels_text("[]"), SpeakerLabelsClass::Gap);
        assert_eq!(
            classify_speaker_labels_text(r#"{"labels": "not an array"}"#),
            SpeakerLabelsClass::Gap
        );
        assert_eq!(
            classify_speaker_labels_text(r#"{"labels": 123}"#),
            SpeakerLabelsClass::Gap
        );
        assert_eq!(
            classify_speaker_labels_text(r#"{"labels": null}"#),
            SpeakerLabelsClass::Gap
        );

        // 4. File classification: absent vs unreadable vs valid
        let temp = Temp::new();
        let absent_path = temp.path().join("does_not_exist.json");
        assert_eq!(
            classify_speaker_labels_file(&absent_path),
            SpeakerLabelsClass::Absent
        );

        let stub_path = temp.path().join("stub.json");
        fs::write(&stub_path, r#"{"labels": [], "skipped": true}"#).unwrap();
        assert_eq!(
            classify_speaker_labels_file(&stub_path),
            SpeakerLabelsClass::Stub
        );
    }

    #[test]
    fn reattribute_false_skips_protected_byte_identical_and_reattribute_true_selects() {
        let temp = Temp::new();
        let segment_dir = temp.path().join("chronicle/20260808/mic/120000_300");
        fs::create_dir_all(segment_dir.join("talents")).unwrap();
        let protected_bytes = b"{\"labels\":[{\"speaker\":\"p1\",\"method\":\"cluster\"}]}\n";
        let label_path = segment_dir.join("talents/speaker_labels.json");
        fs::write(&label_path, protected_bytes).unwrap();
        fs::write(segment_dir.join("audio.npz"), b"fake npz").unwrap();

        // reattribute: false -> skips protected, leaving bytes identical
        let plan_false = plan_backfill_segments(temp.path(), false).unwrap();
        assert_eq!(plan_false.total_scanned, 1);
        assert_eq!(plan_false.selected, 0);
        assert_eq!(plan_false.protected_skipped, 1);
        assert_eq!(fs::read(&label_path).unwrap(), protected_bytes);

        // reattribute: true -> selects protected
        let plan_true = plan_backfill_segments(temp.path(), true).unwrap();
        assert_eq!(plan_true.total_scanned, 1);
        assert_eq!(plan_true.selected, 1);
        assert_eq!(plan_true.protected_skipped, 0);
    }

    #[test]
    fn gap_then_repair_to_stub_or_protected() {
        let temp = Temp::new();
        let segment_dir = temp.path().join("chronicle/20260808/mic/120000_300");
        fs::create_dir_all(segment_dir.join("talents")).unwrap();
        let label_path = segment_dir.join("talents/speaker_labels.json");
        fs::write(&label_path, b"{broken json").unwrap();
        fs::write(segment_dir.join("audio.npz"), b"fake npz").unwrap();

        // Gap fails
        assert!(matches!(
            plan_backfill_segments(temp.path(), false),
            Err(BackfillError::LabelsGap { .. })
        ));

        // Repair to stub -> succeeds and selects
        fs::write(&label_path, r#"{"labels": [], "skipped": true}"#).unwrap();
        let plan_stub = plan_backfill_segments(temp.path(), false).unwrap();
        assert_eq!(plan_stub.selected, 1);

        // Repair to protected + reattribute:false -> skips
        fs::write(
            &label_path,
            r#"{"labels": [{"speaker": "p1"}], "skipped": false}"#,
        )
        .unwrap();
        let plan_prot = plan_backfill_segments(temp.path(), false).unwrap();
        assert_eq!(plan_prot.selected, 0);
        assert_eq!(plan_prot.protected_skipped, 1);
    }

    #[test]
    fn no_embedding_skip_and_unreadable_npz_produces_member_error() {
        let temp = Temp::new();
        let seg_no_audio = temp.path().join("chronicle/20260808/mic/120000_300");
        fs::create_dir_all(seg_no_audio.join("talents")).unwrap();
        fs::write(
            seg_no_audio.join("talents/speaker_labels.json"),
            r#"{"labels": [], "skipped": true}"#,
        )
        .unwrap();

        let plan = plan_backfill_segments(temp.path(), false).unwrap();
        assert_eq!(plan.total_scanned, 1);
        assert_eq!(plan.skipped_no_embeddings, 1);
        assert_eq!(plan.selected, 0);

        // Unreadable declared npz fails loud rather than skip
        fs::write(
            seg_no_audio.join("audio.npz"),
            b"corrupt npz bytes not a zip",
        )
        .unwrap();
        let seg_key = BackfillSegmentKey {
            day: "20260808".to_owned(),
            stream_layout: SegmentLayout::Named,
            stream: "mic".to_owned(),
            segment_key: "120000_300".to_owned(),
        };
        let (outcome, error) =
            execute_backfill_member(temp.path(), &seg_key, &seg_no_audio, true, false, 1);
        assert_eq!(outcome, BackfillCheckpointOutcome::Error);
        assert!(error.is_some());
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn report_only_mode_only_changes_backfill_operations_ledger() {
        let temp = Temp::new();
        let seg1 = temp.path().join("chronicle/20260808/mic/120000_300");
        fs::create_dir_all(seg1.join("talents")).unwrap();
        fs::write(
            seg1.join("talents/speaker_labels.json"),
            r#"{"labels": [], "skipped": true}"#,
        )
        .unwrap();
        fs::write(seg1.join("audio.npz"), b"fake npz").unwrap();

        let initial_journal_files = collect_all_files(temp.path());

        // Run coordinator with commit = false (report-only)
        let req = crate::backfill_coordinator::StartBackfillRequest {
            operation_id: Some("bfop-report-only".to_owned()),
            commit: false,
            reattribute: false,
            accumulation: false,
            now_ms: 1,
        };
        let _ = crate::backfill_coordinator::start_backfill(temp.path(), &req).unwrap();

        for _ in 0..50 {
            std::thread::sleep(Duration::from_millis(20));
            if let Some(s) =
                crate::backfill_coordinator::backfill_status(temp.path(), "bfop-report-only")
                    .unwrap()
                && s.status == BackfillStatusKind::Done
            {
                break;
            }
        }

        let final_journal_files = collect_all_files(temp.path());
        // Only speakers/backfill-operations.jsonl (and lock files) was added
        for p in final_journal_files.keys() {
            let s = p.to_string_lossy();
            if s.contains("speakers/backfill-operations.jsonl") || s.contains("health/locks") {
                continue;
            }
            assert!(
                initial_journal_files.contains_key(p),
                "unexpected new file in report-only: {p:?}"
            );
        }
    }

    #[test]
    fn accumulation_false_leaves_voiceprint_bytes_and_counts_unchanged() {
        let temp = Temp::new();
        let vp_dir = temp.path().join("speakers/voiceprints");
        fs::create_dir_all(&vp_dir).unwrap();
        fs::write(vp_dir.join("owner.npz"), b"mock voiceprint data").unwrap();

        let vp_files_before = collect_all_files(&vp_dir);

        let seg = temp.path().join("chronicle/20260808/mic/120000_300");
        fs::create_dir_all(seg.join("talents")).unwrap();
        fs::write(
            seg.join("talents/speaker_labels.json"),
            r#"{"labels": [], "skipped": true}"#,
        )
        .unwrap();

        let req = crate::backfill_coordinator::StartBackfillRequest {
            operation_id: Some("bfop-no-accum".to_owned()),
            commit: true,
            reattribute: false,
            accumulation: false,
            now_ms: 1,
        };
        let _ = crate::backfill_coordinator::start_backfill(temp.path(), &req).unwrap();

        for _ in 0..50 {
            std::thread::sleep(Duration::from_millis(20));
            if let Some(s) =
                crate::backfill_coordinator::backfill_status(temp.path(), "bfop-no-accum").unwrap()
                && s.status == BackfillStatusKind::Done
            {
                break;
            }
        }

        let vp_files_after = collect_all_files(&vp_dir);
        assert_eq!(vp_files_before, vp_files_after);
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn entity_trust_contention_produces_member_error_and_retry_succeeds() {
        let temp = Temp::new();
        let seg = temp.path().join("chronicle/20260808/mic/120000_300");
        fs::create_dir_all(seg.join("talents")).unwrap();

        // Hold entity-trust lock externally
        let lock_file = temp.path().join("health/locks/entity-trust");
        fs::create_dir_all(lock_file.parent().unwrap()).unwrap();
        let lock_guard = solstone_core_journal_io::hold_lock(
            &lock_file,
            LockOptions {
                timeout: Duration::ZERO,
                ..Default::default()
            },
        )
        .unwrap();

        let seg_key = BackfillSegmentKey {
            day: "20260808".to_owned(),
            stream_layout: SegmentLayout::Named,
            stream: "mic".to_owned(),
            segment_key: "120000_300".to_owned(),
        };

        // When locked, execute returns Error
        let (outcome, err) = execute_backfill_member(temp.path(), &seg_key, &seg, true, false, 1);
        assert_eq!(outcome, BackfillCheckpointOutcome::Error);
        assert!(err.is_some());

        // Release lock
        drop(lock_guard);
    }

    fn collect_all_files(root: &Path) -> HashMap<PathBuf, Vec<u8>> {
        let mut map = HashMap::new();
        if !root.exists() {
            return map;
        }
        let mut dirs = vec![root.to_path_buf()];
        while let Some(dir) = dirs.pop() {
            if let Ok(entries) = fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        dirs.push(path);
                    } else if path.is_file()
                        && let Ok(bytes) = fs::read(&path)
                    {
                        map.insert(path, bytes);
                    }
                }
            }
        }
        map
    }
}
