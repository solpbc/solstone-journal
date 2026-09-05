// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Speaker-attribution hook, combining native Layers 1--3 with Layer 4 output.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde_json::{Map, Value, json};
use solstone_core_speaker_resolve::admission::{
    admissible_person_pool, admissible_resolution_entities, saved_choice_excluded_by_admission,
};
use solstone_core_speaker_resolve::resolve::{ResolveMetadata, ResolveOutcome, ResolveOutput};
use solstone_core_speaker_resolve::voiceprint_accumulation::{
    AccumulationEmbedding, AccumulationLabel, AccumulationRequest, accumulate_voiceprints,
};

use crate::contract::{CommitPlan, ParsedOutput, PrePostState};
use crate::writers::WriteIntent;
use crate::{
    ExecutionContext, PreparedTalent, RuntimeOutcome, StageError, apply_template_vars, stage_error,
};

#[derive(Clone, Debug, PartialEq)]
pub struct SpeakerAttributionState {
    resolved: Box<ResolveOutput>,
}

fn fields(prepared: &PreparedTalent) -> Option<(&str, &str, &str)> {
    let day = prepared.config.get("day")?.as_str()?;
    let segment = prepared.config.get("segment")?.as_str()?;
    (!day.is_empty() && !segment.is_empty()).then_some((
        day,
        segment,
        prepared
            .config
            .get("stream")
            .and_then(Value::as_str)
            .filter(|stream| !stream.is_empty())
            .unwrap_or(solstone_core_journal_io::DEFAULT_STREAM),
    ))
}

fn skipped(prepared: &PreparedTalent, reason: impl Into<String>) -> RuntimeOutcome {
    RuntimeOutcome::Skipped {
        stage: "speaker_attribution".to_owned(),
        talent: prepared.name.clone(),
        reason: reason.into(),
    }
}

fn segment_dir(
    journal: &Path,
    day: &str,
    segment: &str,
    stream: &str,
    create: bool,
) -> Result<PathBuf, String> {
    solstone_core_speaker_resolve::segment_path(journal, day, segment, stream, create)
        .map_err(|error| error.to_string())
}

fn label_values(output: &ResolveOutput) -> Vec<Value> {
    output
        .labels
        .iter()
        .map(|label| {
            json!({
                "sentence_id": label.sentence_id,
                "speaker": label.speaker,
                "confidence": label.confidence,
                "method": label.method,
                "owner_margin_declined": label.owner_margin_declined,
                "acoustic_margin_declined": label.acoustic_margin_declined,
            })
        })
        .collect()
}

fn metadata_values(metadata: &ResolveMetadata) -> Map<String, Value> {
    Map::from_iter([
        (
            "owner_centroid_last_refreshed_at".to_owned(),
            metadata
                .owner_centroid_last_refreshed_at
                .clone()
                .map_or(Value::Null, Value::String),
        ),
        (
            "voiceprint_versions".to_owned(),
            serde_json::to_value(&metadata.voiceprint_versions)
                .unwrap_or_else(|_| Value::Object(Map::new())),
        ),
        (
            "candidate_evidence".to_owned(),
            Value::Array(
                metadata
                    .candidate_evidence
                    .iter()
                    .map(|evidence| {
                        json!({"entity_id": evidence.entity_id, "sources": evidence.sources})
                    })
                    .collect(),
            ),
        ),
    ])
}

fn has_embeddings(segment: &Path) -> bool {
    std::fs::read_dir(segment)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .any(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "npz")
        })
}

fn try_accumulate(journal: &Path, day: &str, segment: &str, stream: &str, output: &ResolveOutput) {
    let Some(source) = output.source.as_deref() else {
        return;
    };
    let result = (|| {
        let dir = segment_dir(journal, day, segment, stream, false)?;
        let Some(embeddings) = solstone_core_speaker_id::embeddings::load_embeddings_file(
            &dir.join(format!("{source}.npz")),
        )
        .map_err(|error| error.to_string())?
        else {
            return Ok(());
        };
        accumulate_with_embeddings(
            journal,
            day,
            segment,
            stream,
            output,
            embeddings
                .statements
                .into_iter()
                .map(|(sentence_id, values)| AccumulationEmbedding {
                    sentence_id,
                    values,
                })
                .collect(),
        )
    })();
    // Preserve solstone/talent/speaker_attribution.py:73-79,204-210: flywheel failures warn only.
    if let Err(error) = result {
        log::warn!("voiceprint accumulation failed: {error}");
    }
}

fn accumulate_with_embeddings(
    journal: &Path,
    day: &str,
    segment: &str,
    stream: &str,
    output: &ResolveOutput,
    embeddings: Vec<AccumulationEmbedding>,
) -> Result<(), String> {
    let Some(source) = output.source.as_deref() else {
        return Ok(());
    };
    let entity_ids = output
        .labels
        .iter()
        .filter_map(|label| label.speaker.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let request = AccumulationRequest {
        journal_root: journal.to_owned(),
        day: day.to_owned(),
        stream: stream.to_owned(),
        segment_key: segment.to_owned(),
        source: source.to_owned(),
        now_ms: Utc::now().timestamp_millis(),
        encoder: solstone_core_entity::EncoderIdentity {
            id: "unresolved".to_owned(),
            sha256: "0".repeat(64),
            width: 256,
        },
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
        embeddings,
        entity_ids,
    };
    accumulate_voiceprints(&request)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn is_dry_run(prepared: &PreparedTalent) -> bool {
    // Exactly two stages honor DRY_RUN_KEY (steward, speaker_attribution); not a general per-stage dry-run flag.
    prepared
        .config
        .get(crate::DRY_RUN_KEY)
        .is_some_and(|value| value == &Value::Bool(true))
}

pub fn build(
    prepared: &mut PreparedTalent,
    context: &ExecutionContext,
) -> Result<PrePostState, RuntimeOutcome> {
    let Some((day, segment, stream)) = fields(prepared) else {
        return Err(skipped(prepared, "no_segment_context"));
    };
    let dry_run = is_dry_run(prepared);
    // Preserve solstone/talent/speaker_attribution.py:40: this lookup creates a first-run segment
    // unless preview/dry-run asked for a read-only pass.
    let segment_dir = segment_dir(&context.journal, day, segment, stream, !dry_run)
        .map_err(|error| skipped(prepared, error))?;
    let resolved = match solstone_core_speaker_resolve::resolve::resolve(
        &context.journal,
        day,
        stream,
        segment,
        dry_run,
        Utc::now().timestamp_millis(),
    ) {
        Ok(resolved) => resolved,
        Err(error) => {
            let reason = error.to_string();
            if !dry_run && has_embeddings(&segment_dir) {
                solstone_core_speaker_id::labels::write_stub_labels(&segment_dir, &reason)
                    .map_err(|error| skipped(prepared, error.to_string()))?;
            }
            return Err(skipped(prepared, reason));
        }
    };
    // Every non-resolved outcome used to report `no_embeddings`, so a broken owner
    // identity and a segment holding a perfectly good `audio.npz` produced the same
    // sentence. Name the outcome that actually occurred: only `Empty` is about
    // missing embeddings, and the other two are separately actionable.
    let resolved = match resolved {
        ResolveOutcome::Resolved(resolved) => resolved,
        outcome => {
            let reason = match outcome {
                ResolveOutcome::SegmentMissing => "no_segment",
                ResolveOutcome::IdentityInvalid => "identity_invalid",
                ResolveOutcome::NoOwnerCentroid => "no_owner_centroid",
                ResolveOutcome::Empty { .. } => "no_embeddings",
                ResolveOutcome::Resolved(_) => unreachable!("matched above"),
            };
            if !dry_run && has_embeddings(&segment_dir) {
                solstone_core_speaker_id::labels::write_stub_labels(&segment_dir, reason)
                    .map_err(|error| skipped(prepared, error.to_string()))?;
            }
            return Err(skipped(prepared, reason));
        }
    };
    if resolved.labels.is_empty() {
        let reason = "no_embeddings";
        if !dry_run && has_embeddings(&segment_dir) {
            solstone_core_speaker_id::labels::write_stub_labels(&segment_dir, reason)
                .map_err(|error| skipped(prepared, error.to_string()))?;
        }
        return Err(skipped(prepared, reason));
    }
    let state = SpeakerAttributionState { resolved };
    if state.resolved.unmatched.is_empty() {
        if !dry_run {
            solstone_core_speaker_id::labels::write_full_labels(
                &segment_dir,
                label_values(&state.resolved),
                &metadata_values(&state.resolved.metadata),
            )
            .map_err(|error| skipped(prepared, error.to_string()))?;
            try_accumulate(&context.journal, day, segment, stream, &state.resolved);
        }
        // Preserve solstone/talent/speaker_attribution.py:73-81: this writes from build because
        // the reference writes before it skips generation.
        return Err(skipped(prepared, "all_resolved"));
    }
    Ok(PrePostState::SpeakerAttribution(state))
}

pub fn apply_prompt_override(
    prepared: &mut PreparedTalent,
    state: &PrePostState,
) -> Result<(), StageError> {
    let PrePostState::SpeakerAttribution(state) = state else {
        return Err(stage_error(
            "prompt_override",
            "speaker_attribution",
            prepared,
            "missing speaker attribution state",
        ));
    };
    let mut lines = vec![
        "## Speaker Attribution — Layer 4 Analysis Required".to_owned(),
        String::new(),
        "Layers 1-3 (owner detection, structural heuristics, acoustic matching)".to_owned(),
        "resolved most sentences.  The following need contextual identification.".to_owned(),
        String::new(),
    ];
    if !state.resolved.candidates.is_empty() {
        lines.push(format!(
            "**Known speakers in this segment:** {}",
            state.resolved.candidates.join(", ")
        ));
        lines.push(String::new());
    }
    lines.extend(["**Unmatched sentences:**".to_owned(), String::new()]);
    for sentence_id in &state.resolved.unmatched {
        let text = state
            .resolved
            .unmatched_texts
            .get(sentence_id)
            .map(String::as_str)
            .unwrap_or("[text unavailable]");
        lines.push(format!("- Sentence {sentence_id}: \"{text}\""));
    }
    lines.push(String::new());
    apply_template_vars(
        &mut prepared.config,
        &Map::from_iter([(
            "unmatched_context".to_owned(),
            Value::String(lines.join("\n")),
        )]),
    );
    Ok(())
}

pub fn parse(
    output: &str,
    _: &PreparedTalent,
    _: &PrePostState,
) -> Result<ParsedOutput, StageError> {
    Ok(ParsedOutput::Text(output.to_owned()))
}

pub fn commit(
    parsed: ParsedOutput,
    prepared: &PreparedTalent,
    state: &PrePostState,
) -> Result<CommitPlan, StageError> {
    let ParsedOutput::Text(output) = parsed else {
        return Err(stage_error(
            "commit",
            "speaker_attribution",
            prepared,
            "expected text output",
        ));
    };
    let (Some((day, segment, stream)), PrePostState::SpeakerAttribution(state)) =
        (fields(prepared), state)
    else {
        return Ok(CommitPlan::NoOutput);
    };
    Ok(CommitPlan::Write(WriteIntent::SpeakerAttribution {
        output,
        day: day.to_owned(),
        segment: segment.to_owned(),
        stream: stream.to_owned(),
        state: state.clone(),
    }))
}

pub fn apply_result(
    journal: &Path,
    output: &str,
    day: &str,
    segment: &str,
    stream: &str,
    state: &SpeakerAttributionState,
) -> Result<(), String> {
    let mut layer4 = HashMap::new();
    if !output.is_empty() {
        let parsed = serde_json::from_str::<Value>(output)
            .map_err(|error| format!("failed to parse Layer 4 result: {error}"));
        if let Ok(parsed) = parsed {
            let items = parsed
                .as_object()
                .and_then(|object| object.get("attributions"))
                .or_else(|| parsed.as_array().map(|_| &parsed));
            if let Some(items) = items.and_then(Value::as_array) {
                let loaded = solstone_core_entity::load_all_journal_entities(journal)
                    .map_err(|error| error.to_string())?;
                let all_entities = loaded.iter().collect::<Vec<_>>();
                let unblocked = loaded
                    .iter()
                    .filter(|entity| !entity.is_blocked())
                    .collect::<Vec<_>>();
                let pool = admissible_person_pool(&unblocked);
                let resolution_entities = admissible_resolution_entities(&pool);
                let scope = json!({"kind":"journal"});
                for item in items.iter().filter_map(Value::as_object) {
                    let (Some(sentence_id), Some(speaker)) = (
                        item.get("sentence_id").and_then(Value::as_i64),
                        item.get("speaker")
                            .and_then(Value::as_str)
                            .filter(|name| !name.is_empty()),
                    ) else {
                        continue;
                    };
                    if saved_choice_excluded_by_admission(journal, &scope, speaker, &all_entities)
                        .map_err(|error| error.to_string())?
                    {
                        continue;
                    }
                    let resolution = solstone_core_entity::record_entity_resolution(
                        journal,
                        speaker,
                        &resolution_entities,
                        scope.clone(),
                        json!({"lane":"talent.speaker_attribution","day":day,"segment_id":segment,"field":"layer4.speaker"}),
                        90.0,
                        false,
                    )
                    .map_err(|error| error.to_string())?;
                    if resolution.outcome == solstone_core_entity::EntityResolutionOutcome::Resolved
                        && let Some(id) = resolution
                            .entity_index
                            .and_then(|index| pool.get(index).map(|entity| entity.id.clone()))
                    {
                        layer4.insert(sentence_id, id);
                    }
                }
            }
        }
    }
    let mut resolved = (*state.resolved).clone();
    for label in &mut resolved.labels {
        if label.speaker.is_none()
            && let Some(speaker) = layer4.get(&label.sentence_id)
        {
            label.speaker = Some(speaker.clone());
            label.confidence = Some("medium".to_owned());
            label.method = Some("contextual".to_owned());
        }
    }
    // Preserve solstone/talent/speaker_attribution.py:200: post processing creates the segment path.
    let segment_dir = segment_dir(journal, day, segment, stream, true)?;
    solstone_core_speaker_id::labels::write_full_labels(
        &segment_dir,
        label_values(&resolved),
        &metadata_values(&resolved.metadata),
    )
    .map_err(|error| error.to_string())?;
    try_accumulate(journal, day, segment, stream, &resolved);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::io::{Cursor, Write};

    use solstone_core_npy::write_npy;
    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, ZipWriter};

    use super::*;

    fn output(unmatched: Vec<i64>) -> Box<ResolveOutput> {
        Box::new(ResolveOutput {
            labels: vec![solstone_core_speaker_resolve::layer1::Label {
                sentence_id: 7,
                speaker: None,
                confidence: None,
                method: None,
                owner_margin_declined: None,
                acoustic_margin_declined: None,
            }],
            unmatched,
            unmatched_texts: HashMap::from([(7, "Who said this?".to_owned())]),
            source: None,
            candidates: vec!["Ada".to_owned()],
            metadata: ResolveMetadata {
                owner_centroid_last_refreshed_at: None,
                voiceprint_versions: HashMap::new(),
                candidate_evidence: Vec::new(),
                candidate_evidence_gaps: None,
                voiceprint_gaps: None,
            },
        })
    }

    #[test]
    fn layer_four_prompt_and_state_match_the_reference() {
        // Derived from solstone/talent/speaker_attribution.py:65-66,87-108.
        let state = PrePostState::SpeakerAttribution(SpeakerAttributionState {
            resolved: output(vec![7]),
        });
        let mut prepared = PreparedTalent {
            name: "speaker_attribution".to_owned(),
            config: Map::from_iter([(
                "prompt".to_owned(),
                Value::String("$unmatched_context".to_owned()),
            )]),
        };
        apply_prompt_override(&mut prepared, &state).unwrap();
        assert_eq!(
            prepared.config["prompt"],
            "## Speaker Attribution — Layer 4 Analysis Required\n\nLayers 1-3 (owner detection, structural heuristics, acoustic matching)\nresolved most sentences.  The following need contextual identification.\n\n**Known speakers in this segment:** Ada\n\n**Unmatched sentences:**\n\n- Sentence 7: \"Who said this?\"\n"
        );
        assert!(matches!(state, PrePostState::SpeakerAttribution(_)));
    }

    #[test]
    fn segment_path_preserves_create_and_read_only_polarities() {
        // Derived from solstone/talent/speaker_attribution.py:40,200 and solstone/think/utils.py:389.
        let root = tempfile::tempdir().unwrap();
        let read_only = segment_dir(root.path(), "20260101", "090000_300", "main", false).unwrap();
        assert!(!read_only.exists());
        let created = segment_dir(root.path(), "20260101", "090000_300", "main", true).unwrap();
        assert!(created.is_dir());
    }

    #[test]
    fn direct_record_creation_does_not_create_a_named_default_tree() {
        let root = tempfile::tempdir().unwrap();
        let expected = root.path().join("chronicle/20260101/090000_300");
        assert_eq!(
            segment_dir(root.path(), "20260101", "090000_300", "_default", false).unwrap(),
            expected
        );
        assert!(!expected.exists());
        let mut prepared = PreparedTalent {
            name: "speaker_attribution".to_owned(),
            config: Map::from_iter([
                ("day".to_owned(), Value::String("20260101".to_owned())),
                ("segment".to_owned(), Value::String("090000_300".to_owned())),
            ]),
        };
        let _ = build(
            &mut prepared,
            &ExecutionContext {
                journal: root.path().to_owned(),
            },
        );
        assert!(expected.is_dir());
        assert!(!root.path().join("chronicle/20260101/_default").exists());
        assert!(segment_dir(root.path(), "20260101", "../outside", "_default", true).is_err());
        assert!(!root.path().join("chronicle/outside").exists());
    }

    #[test]
    fn criterion_7_build_writes_unless_dry_run() {
        let root = tempfile::tempdir().unwrap();
        let mut prepared = PreparedTalent {
            name: "speaker_attribution".to_owned(),
            config: Map::from_iter([
                ("day".to_owned(), Value::String("20260101".to_owned())),
                ("segment".to_owned(), Value::String("090000_300".to_owned())),
                ("stream".to_owned(), Value::String("main".to_owned())),
            ]),
        };
        let _ = build(
            &mut prepared,
            &ExecutionContext {
                journal: root.path().to_owned(),
            },
        );
        assert!(
            root.path()
                .join("chronicle/20260101/main/090000_300")
                .is_dir(),
            "a real pre-step must create the segment directory"
        );

        let preview = tempfile::tempdir().unwrap();
        let mut dry = prepared;
        dry.config
            .insert(crate::DRY_RUN_KEY.to_owned(), Value::Bool(true));
        let _ = build(
            &mut dry,
            &ExecutionContext {
                journal: preview.path().to_owned(),
            },
        );
        assert!(
            !preview
                .path()
                .join("chronicle/20260101/main/090000_300")
                .exists(),
            "preview dry-run must not create the segment directory"
        );
    }

    #[test]
    fn missing_context_skips_generation() {
        // Derived from solstone/talent/speaker_attribution.py:37.
        let root = tempfile::tempdir().unwrap();
        let mut prepared = PreparedTalent {
            name: "speaker_attribution".to_owned(),
            config: Map::new(),
        };
        let outcome = build(
            &mut prepared,
            &ExecutionContext {
                journal: root.path().to_owned(),
            },
        )
        .unwrap_err();
        assert!(
            matches!(outcome, RuntimeOutcome::Skipped { reason, .. } if reason == "no_segment_context")
        );
    }

    #[test]
    fn embeddings_gate_writes_a_stub_before_skipping() {
        // Derived from solstone/talent/speaker_attribution.py:45-58.
        let root = tempfile::tempdir().unwrap();
        let segment = segment_dir(root.path(), "20260101", "090000_300", "main", true).unwrap();
        std::fs::write(segment.join("transcript.npz"), "fixture").unwrap();
        let mut prepared = PreparedTalent {
            name: "speaker_attribution".to_owned(),
            config: Map::from_iter([
                ("day".to_owned(), Value::String("20260101".to_owned())),
                ("segment".to_owned(), Value::String("090000_300".to_owned())),
                ("stream".to_owned(), Value::String("main".to_owned())),
            ]),
        };
        let outcome = build(
            &mut prepared,
            &ExecutionContext {
                journal: root.path().to_owned(),
            },
        )
        .unwrap_err();
        // This fixture writes no owner identity at all, so the honest reason names
        // that rather than blaming absent embeddings.
        let RuntimeOutcome::Skipped { reason, .. } = outcome else {
            panic!("expected a skip, got {outcome:?}");
        };
        assert_eq!(reason, "identity_invalid");
        assert_eq!(
            serde_json::from_str::<Value>(
                &std::fs::read_to_string(segment.join("talents/speaker_labels.json")).unwrap(),
            )
            .unwrap()["reason"],
            "identity_invalid"
        );
    }

    #[test]
    fn state_value_reaches_post_and_writes_layer_four_label() {
        // Derived from solstone/talent/speaker_attribution.py:129-130,157-210.
        let root = tempfile::tempdir().unwrap();
        let entity = root.path().join("entities/ada");
        std::fs::create_dir_all(&entity).unwrap();
        std::fs::write(
            entity.join("entity.json"),
            json!({"id":"ada","name":"Ada","type":"Person"}).to_string(),
        )
        .unwrap();
        let state = SpeakerAttributionState {
            resolved: output(vec![7]),
        };
        apply_result(
            root.path(),
            r#"{"attributions":[{"sentence_id":7,"speaker":"Ada"}]}"#,
            "20260101",
            "090000_300",
            "main",
            &state,
        )
        .unwrap();
        let labels: Value = serde_json::from_str(
            &std::fs::read_to_string(
                root.path()
                    .join("chronicle/20260101/main/090000_300/talents/speaker_labels.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(labels["labels"][0]["speaker"], "ada");
        assert_eq!(labels["labels"][0]["method"], "contextual");
    }

    #[test]
    fn known_speakers_prompt_omits_non_person_candidate_names() {
        let root = tempfile::tempdir().unwrap();
        write_layer4_entity(root.path(), "principal", "Principal", Some("Person"), false);
        write_layer4_entity(root.path(), "alice", "Alice", Some("Person"), false);
        write_layer4_entity(root.path(), "tool", "Terminal", Some("Tool"), false);
        std::fs::write(
            root.path().join("entities/principal/entity.json"),
            json!({"id":"principal","name":"Principal","type":"Person","is_principal":true})
                .to_string(),
        )
        .unwrap();
        solstone_core_speaker_resolve::owner_centroid::write_owner_centroid(
            root.path(),
            "principal",
            &solstone_core_speaker_resolve::owner_centroid::OwnerCentroidWriteInput {
                centroid: vector(1.0, 0.0),
                cluster_size: 10,
                timestamp: "2026-08-08T12:00:00Z".to_owned(),
                evidence_tier: "high".to_owned(),
            },
        )
        .unwrap();
        let segment = segment_dir(root.path(), "20260808", "120000_300", "mic", true).unwrap();
        std::fs::create_dir_all(segment.join("talents")).unwrap();
        write_statement_embeddings(
            &segment.join("mic_audio.npz"),
            &[1, 2],
            &[vector(1.0, 0.0), vector(0.0, 1.0)],
        );
        std::fs::write(
            segment.join("talents/speakers.json"),
            r#"["Alice", "Terminal"]"#,
        )
        .unwrap();
        let outcome = solstone_core_speaker_resolve::resolve::resolve(
            root.path(),
            "20260808",
            "mic",
            "120000_300",
            true,
            1,
        )
        .expect("resolve");
        let ResolveOutcome::Resolved(resolved) = outcome else {
            panic!("expected resolved");
        };
        let state = PrePostState::SpeakerAttribution(SpeakerAttributionState { resolved });
        let mut prepared = PreparedTalent {
            name: "speaker_attribution".to_owned(),
            config: Map::from_iter([(
                "prompt".to_owned(),
                Value::String("$unmatched_context".to_owned()),
            )]),
        };
        apply_prompt_override(&mut prepared, &state).unwrap();
        let prompt = prepared.config["prompt"].as_str().expect("prompt text");
        assert!(
            prompt.contains("**Known speakers in this segment:**"),
            "prompt should list known speakers: {prompt}"
        );
        assert!(
            prompt.contains("Alice"),
            "prompt should include the Person name: {prompt}"
        );
        assert!(
            !prompt.contains("Terminal"),
            "prompt must not include the Tool name: {prompt}"
        );
    }

    fn write_layer4_entity(
        root: &Path,
        id: &str,
        name: &str,
        entity_type: Option<&str>,
        blocked: bool,
    ) {
        let entity = root.join("entities").join(id);
        std::fs::create_dir_all(&entity).unwrap();
        let mut value = json!({"id": id, "name": name});
        let object = value.as_object_mut().expect("entity object");
        if let Some(entity_type) = entity_type {
            object.insert("type".to_owned(), Value::String(entity_type.to_owned()));
        }
        if blocked {
            object.insert("blocked".to_owned(), Value::Bool(true));
        }
        std::fs::write(entity.join("entity.json"), value.to_string()).unwrap();
    }

    fn write_resolved_choice(root: &Path, query: &str, entity_id: &str) {
        let normalized = solstone_core_entity_matching::normalize_resolution_query(query);
        let row = json!({
            "schema_version": 1,
            "ambiguity_id": solstone_core_entity::ambiguity_id(&format!("journal|{normalized}")),
            "scope": {"kind": "journal"},
            "normalized_query": normalized,
            "original_query": query,
            "latest_query": query,
            "first_seen": "2026-08-01T00:00:00Z",
            "last_seen": "2026-08-01T00:00:00Z",
            "observed_tier": 8,
            "status": "resolved",
            "resolved_entity_id": entity_id,
            "resolved_at": "2026-08-01T00:00:00Z",
            "ranked_candidates": [{
                "id": entity_id,
                "name": query,
                "tier": 8,
                "score": 90.0
            }],
            "origins": [{"lane": "test"}],
            "origin_keys": ["test"],
            "occurrence_count": 1,
            "audit": {"prior_choices": []}
        });
        let path = root.join("entities/ambiguities.jsonl");
        std::fs::create_dir_all(path.parent().expect("ambiguities parent")).unwrap();
        std::fs::write(&path, format!("{row}\n")).unwrap();
    }

    fn layer4_labels(root: &Path, speaker: &str) -> Value {
        let state = SpeakerAttributionState {
            resolved: output(vec![7]),
        };
        apply_result(
            root,
            &format!(r#"{{"attributions":[{{"sentence_id":7,"speaker":"{speaker}"}}]}}"#),
            "20260101",
            "090000_300",
            "main",
            &state,
        )
        .unwrap();
        serde_json::from_str(
            &std::fs::read_to_string(
                root.join("chronicle/20260101/main/090000_300/talents/speaker_labels.json"),
            )
            .unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn apply_result_does_not_label_non_person_or_unresolved_speakers() {
        for (id, name, entity_type, blocked, speaker) in [
            ("tool", "Terminal", Some("Tool"), false, "Terminal"),
            ("project", "Atlas", Some("Project"), false, "Atlas"),
            ("company", "Acme", Some("Company"), false, "Acme"),
            ("blocked", "Blocked", Some("Person"), true, "Blocked"),
            ("untyped", "Untyped", None, false, "Untyped"),
        ] {
            let root = tempfile::tempdir().unwrap();
            write_layer4_entity(root.path(), id, name, entity_type, blocked);
            let labels = layer4_labels(root.path(), speaker);
            assert!(
                labels["labels"][0]["speaker"].is_null(),
                "{speaker} must stay unlabeled"
            );
            assert!(
                !root
                    .path()
                    .join("entities")
                    .join(id)
                    .join("voiceprints.npz")
                    .exists(),
                "{id} must not receive a voiceprint"
            );
        }

        let ambiguous = tempfile::tempdir().unwrap();
        write_layer4_entity(
            ambiguous.path(),
            "sam-one",
            "Sam Person",
            Some("Person"),
            false,
        );
        write_layer4_entity(
            ambiguous.path(),
            "sam-two",
            "Sam Person",
            Some("Person"),
            false,
        );
        let labels = layer4_labels(ambiguous.path(), "Sam Person");
        assert!(labels["labels"][0]["speaker"].is_null());

        let unmatched = tempfile::tempdir().unwrap();
        write_layer4_entity(unmatched.path(), "ada", "Ada", Some("Person"), false);
        let labels = layer4_labels(unmatched.path(), "Nobody");
        assert!(labels["labels"][0]["speaker"].is_null());
    }

    #[test]
    fn apply_result_same_name_person_and_tool_labels_the_person() {
        let root = tempfile::tempdir().unwrap();
        write_layer4_entity(root.path(), "alex", "Alex", Some("Person"), false);
        write_layer4_entity(root.path(), "tool", "Alex", Some("Tool"), false);
        let labels = layer4_labels(root.path(), "Alex");
        assert_eq!(labels["labels"][0]["speaker"], "alex");
        assert_eq!(labels["labels"][0]["method"], "contextual");
    }

    #[test]
    fn apply_result_saved_choice_naming_a_tool_is_unmatched() {
        let root = tempfile::tempdir().unwrap();
        write_layer4_entity(root.path(), "tool", "Terminal", Some("Tool"), false);
        write_resolved_choice(root.path(), "Terminal", "tool");
        let labels = layer4_labels(root.path(), "Terminal");
        assert!(labels["labels"][0]["speaker"].is_null());
        assert!(!root.path().join("entities/tool/voiceprints.npz").exists());
    }

    #[test]
    fn apply_result_saved_choice_naming_a_blocked_entity_is_unmatched() {
        let root = tempfile::tempdir().unwrap();
        write_layer4_entity(root.path(), "alice", "Alice", Some("Person"), true);
        write_resolved_choice(root.path(), "Alice", "alice");
        let labels = layer4_labels(root.path(), "Alice");
        assert!(labels["labels"][0]["speaker"].is_null());
        assert!(!root.path().join("entities/alice/voiceprints.npz").exists());
    }

    #[test]
    fn apply_result_does_not_write_a_tool_voiceprint_when_accumulation_runs() {
        let root = tempfile::tempdir().unwrap();
        let mut resolved = accumulation_fixture(root.path());
        write_layer4_entity(root.path(), "tool", "Terminal", Some("Tool"), false);
        resolved.labels[0].speaker = None;
        resolved.labels[0].confidence = None;
        resolved.labels[0].method = None;
        resolved.unmatched = vec![7];
        let segment = segment_dir(root.path(), "20260101", "090000_300", "main", false).unwrap();
        write_statement_embeddings(&segment.join("transcript.npz"), &[7], &[vector(0.0, 1.0)]);
        apply_result(
            root.path(),
            r#"{"attributions":[{"sentence_id":7,"speaker":"Terminal"}]}"#,
            "20260101",
            "090000_300",
            "main",
            &SpeakerAttributionState { resolved },
        )
        .unwrap();
        let labels: Value = serde_json::from_str(
            &std::fs::read_to_string(
                root.path()
                    .join("chronicle/20260101/main/090000_300/talents/speaker_labels.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert!(labels["labels"][0]["speaker"].is_null());
        assert!(
            !root.path().join("entities/tool/voiceprints.npz").exists(),
            "Tool must not receive a voiceprint when accumulation actually runs"
        );
    }

    fn write_statement_embeddings(path: &Path, ids: &[i32], values: &[Vec<f32>]) {
        let flat = values.iter().flatten().copied().collect::<Vec<_>>();
        let f32_bytes = flat
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect::<Vec<_>>();
        let i32_bytes = ids
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect::<Vec<_>>();
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        writer
            .start_file("embeddings.npy", options)
            .expect("member");
        writer
            .write_all(&write_npy(
                "<f4",
                &format!("({}, 256)", values.len()),
                &f32_bytes,
            ))
            .expect("embeddings");
        writer
            .start_file("statement_ids.npy", options)
            .expect("member");
        writer
            .write_all(&write_npy("<i4", &format!("({},)", ids.len()), &i32_bytes))
            .expect("ids");
        std::fs::write(path, writer.finish().expect("archive").into_inner()).expect("npz");
    }

    fn accumulation_fixture(root: &Path) -> Box<ResolveOutput> {
        for (id, principal) in [("owner", true), ("ada", false)] {
            let entity = root.join("entities").join(id);
            std::fs::create_dir_all(&entity).unwrap();
            std::fs::write(
                entity.join("entity.json"),
                json!({"id":id,"name":id,"type":"Person","is_principal":principal}).to_string(),
            )
            .unwrap();
        }
        solstone_core_speaker_resolve::owner_centroid::write_owner_centroid(
            root,
            "owner",
            &solstone_core_speaker_resolve::owner_centroid::OwnerCentroidWriteInput {
                centroid: vector(1.0, 0.0),
                cluster_size: 10,
                timestamp: "2026-08-08T12:00:00Z".to_owned(),
                evidence_tier: "high".to_owned(),
            },
        )
        .unwrap();
        let segment = segment_dir(root, "20260101", "090000_300", "main", true).unwrap();
        std::fs::write(segment.join("transcript.jsonl"), "{}\n").unwrap();
        let mut resolved = output(Vec::new());
        resolved.labels[0].speaker = Some("ada".to_owned());
        resolved.labels[0].confidence = Some("high".to_owned());
        resolved.labels[0].method = Some("acoustic".to_owned());
        resolved.source = Some("transcript".to_owned());
        resolved
    }

    fn vector(x: f32, y: f32) -> Vec<f32> {
        let mut values = vec![0.0; 256];
        values[0] = x;
        values[1] = y;
        values
    }

    #[test]
    fn all_resolved_pre_path_accumulates_eligible_voiceprints() {
        // Derived from solstone/talent/speaker_attribution.py:73-79.
        let root = tempfile::tempdir().unwrap();
        let resolved = accumulation_fixture(root.path());
        accumulate_with_embeddings(
            root.path(),
            "20260101",
            "090000_300",
            "main",
            &resolved,
            vec![AccumulationEmbedding {
                sentence_id: 7,
                values: vector(0.0, 1.0),
            }],
        )
        .unwrap();
        assert!(solstone_core_entity::load_entity_voiceprints_file(root.path(), "ada").is_some());
    }

    #[test]
    fn post_path_accumulates_eligible_voiceprints() {
        // Derived from solstone/talent/speaker_attribution.py:204-210.
        let root = tempfile::tempdir().unwrap();
        let resolved = accumulation_fixture(root.path());
        accumulate_with_embeddings(
            root.path(),
            "20260101",
            "090000_300",
            "main",
            &resolved,
            vec![AccumulationEmbedding {
                sentence_id: 7,
                values: vector(0.0, 1.0),
            }],
        )
        .unwrap();
        assert!(solstone_core_entity::load_entity_voiceprints_file(root.path(), "ada").is_some());
    }
}
