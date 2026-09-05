// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Timeline segment-summary hook stage.

use std::fs;
use std::path::Path;

use serde_json::{Map, Value};
use solstone_core_talent_config::is_truthy;
use solstone_core_timeline::{
    ActivitySourceSnapshot, ArtifactCurrentness, AttemptOutcome, AttemptStateV1,
    CURRENT_SCHEMA_VERSION, GenerationProvenanceV1, SEGMENT_SOURCE_SCHEMA_VERSION,
    SegmentBindingV1, SegmentSelectorV1, SegmentSourceV1, SegmentSummaryV1, SegmentTimelineV1,
    TimelineError, TimelineKind, evaluate_artifact_currentness, new_attempt_id, origin_for_binding,
    publish_segment_timeline, resolve_activity_source, resolve_segment_binding, segment_directory,
    segment_input_digest, segment_subject_key, validate_segment_timeline,
};

use crate::contract::{CommitPlan, GateDecision, ParsedOutput, PrePostState};
use crate::writers::WriteIntent;
use crate::{
    ExecutionContext, PreparedTalent, RuntimeOutcome, StageError, apply_template_vars, stage_error,
};

#[derive(Clone, Debug, PartialEq)]
pub struct TimelinePreState {
    activity_text: String,
    segment_rel_path: String,
    binding: SegmentBindingV1,
    source: SegmentSourceV1,
    input_digest: String,
    provenance: Option<Box<GenerationProvenanceV1>>,
}

fn fields(prepared: &PreparedTalent) -> Option<(&str, &str, Option<&str>)> {
    let day = prepared.config.get("day")?.as_str()?;
    let segment = prepared.config.get("segment")?.as_str()?;
    (!day.is_empty() && !segment.is_empty()).then_some((
        day,
        segment,
        prepared.config.get("stream").and_then(Value::as_str),
    ))
}

fn resolve_activity(
    journal: &Path,
    day: &str,
    segment: &str,
    stream: Option<&str>,
) -> Result<Option<(SegmentBindingV1, std::path::PathBuf, ActivitySourceSnapshot)>, TimelineError> {
    let selector = SegmentSelectorV1 {
        day: day.to_owned(),
        segment: segment.to_owned(),
        stream: stream.map(ToOwned::to_owned),
    };
    let binding = match resolve_segment_binding(journal, &selector) {
        Ok(binding) => binding,
        Err(
            error @ TimelineError::SegmentNotFound {
                stream: Some(_), ..
            },
        ) => {
            return Err(error);
        }
        Err(TimelineError::SegmentNotFound { .. }) => return Ok(None),
        Err(error) => return Err(error),
    };
    let segment_dir = segment_directory(journal, &binding)?;
    let Some(source) = resolve_activity_source(journal, &binding)? else {
        return Ok(None);
    };
    Ok(Some((binding, segment_dir, source)))
}

fn generated_source(snapshot: &ActivitySourceSnapshot) -> SegmentSourceV1 {
    SegmentSourceV1::GeneratedActivity {
        schema_version: SEGMENT_SOURCE_SCHEMA_VERSION,
        relative_path: snapshot.relative_path.clone(),
        sha256: snapshot.sha256.clone(),
    }
}

pub fn gate(
    prepared: &PreparedTalent,
    context: &ExecutionContext,
) -> Result<GateDecision, StageError> {
    let Some((day, segment, stream)) = fields(prepared) else {
        return Ok(GateDecision::Skip("no_activity_md".to_owned()));
    };
    let resolved = resolve_activity(&context.journal, day, segment, stream).map_err(|error| {
        stage_error(
            "gate",
            "timeline:segment_summary",
            prepared,
            error.to_string(),
        )
    })?;
    let Some((binding, segment_dir, activity)) = resolved else {
        return Ok(GateDecision::Skip("no_activity_md".to_owned()));
    };
    if let Err(error) = solstone_core_timeline::ensure_timeline_conversion(
        &context.journal,
        &segment_subject_key(&binding),
    ) {
        return Ok(GateDecision::Skip(format!(
            "timeline_conversion_required: {error}"
        )));
    }
    let source = generated_source(&activity);
    let input_digest = segment_input_digest(&binding, &source).map_err(|error| {
        stage_error(
            "gate",
            "timeline:segment_summary",
            prepared,
            error.to_string(),
        )
    })?;
    if !prepared.config.get("refresh").is_some_and(is_truthy) {
        let timeline_path = segment_dir.join("timeline.json");
        if let Ok(text) = fs::read_to_string(timeline_path)
            && let Ok(timeline) = serde_json::from_str::<SegmentTimelineV1>(&text)
            && validate_segment_timeline(&timeline).is_ok()
            && timeline.binding == binding
            && timeline.input_digest == input_digest
            && matches!(
                evaluate_artifact_currentness(
                    &context.journal,
                    &segment_subject_key(&binding),
                    &timeline.input_digest,
                    timeline.generated_at_ms,
                    &text,
                ),
                Ok(ArtifactCurrentness::Current)
            )
        {
            return Ok(GateDecision::Skip("timeline_current".to_owned()));
        }
    }
    Ok(GateDecision::Proceed)
}

pub fn build(
    prepared: &mut PreparedTalent,
    context: &ExecutionContext,
) -> Result<PrePostState, RuntimeOutcome> {
    let Some((day, segment, stream)) = fields(prepared) else {
        return Err(skipped(prepared));
    };
    let resolved = resolve_activity(&context.journal, day, segment, stream).map_err(|error| {
        RuntimeOutcome::StageFailed(stage_error(
            "build",
            "timeline:segment_summary",
            prepared,
            error.to_string(),
        ))
    })?;
    let Some((binding, _, activity)) = resolved else {
        return Err(skipped(prepared));
    };
    solstone_core_timeline::ensure_timeline_conversion(
        &context.journal,
        &segment_subject_key(&binding),
    )
    .map_err(|error| {
        RuntimeOutcome::StageFailed(stage_error(
            "build",
            "timeline:segment_summary",
            prepared,
            error.to_string(),
        ))
    })?;
    let source = generated_source(&activity);
    let input_digest = segment_input_digest(&binding, &source).map_err(|error| {
        RuntimeOutcome::StageFailed(stage_error(
            "build",
            "timeline:segment_summary",
            prepared,
            error.to_string(),
        ))
    })?;
    let segment_rel_path = origin_for_binding(&binding).map_err(|error| {
        RuntimeOutcome::StageFailed(stage_error(
            "build",
            "timeline:segment_summary",
            prepared,
            error.to_string(),
        ))
    })?;
    Ok(PrePostState::Timeline(TimelinePreState {
        activity_text: activity.text,
        segment_rel_path,
        binding,
        source,
        input_digest,
        provenance: None,
    }))
}

pub(crate) fn attach_generated_provenance(
    state: &mut PrePostState,
    response: &solstone_core_generate::GeneratedResponse,
) -> Result<(), String> {
    let PrePostState::Timeline(state) = state else {
        return Err("missing timeline state".to_owned());
    };
    state.provenance = Some(Box::new(GenerationProvenanceV1 {
        model: response.model.clone(),
        finish_reason: response.finish_reason.clone(),
        schema_validation: response.schema_validation.clone().unwrap_or(Value::Null),
        inference: response.inference.clone().unwrap_or(Value::Null),
        usage: response.usage.clone(),
    }));
    Ok(())
}

fn skipped(prepared: &PreparedTalent) -> RuntimeOutcome {
    RuntimeOutcome::Skipped {
        stage: "timeline:segment_summary".to_owned(),
        talent: prepared.name.clone(),
        reason: "no_activity_md".to_owned(),
    }
}

pub fn apply_prompt_override(
    prepared: &mut PreparedTalent,
    state: &PrePostState,
) -> Result<(), StageError> {
    let PrePostState::Timeline(state) = state else {
        return Err(stage_error(
            "prompt_override",
            "timeline:segment_summary",
            prepared,
            "missing timeline state",
        ));
    };
    apply_template_vars(
        &mut prepared.config,
        &Map::from_iter([
            (
                "activity_text".to_owned(),
                Value::String(state.activity_text.clone()),
            ),
            (
                "segment_rel_path".to_owned(),
                Value::String(state.segment_rel_path.clone()),
            ),
        ]),
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
            "timeline:segment_summary",
            prepared,
            "expected text output",
        ));
    };
    // Preserve solstone/apps/timeline/talent/segment_summary.py:222-244: invalid output is ignored.
    let Ok(Value::Object(result)) = serde_json::from_str(&output) else {
        return Ok(CommitPlan::NoOutput);
    };
    let PrePostState::Timeline(state) = state else {
        return Err(stage_error(
            "commit",
            "timeline:segment_summary",
            prepared,
            "missing timeline state",
        ));
    };
    let Some(provenance) = state.provenance.clone() else {
        return Err(stage_error(
            "commit",
            "timeline:segment_summary",
            prepared,
            "missing generated provenance",
        ));
    };
    Ok(CommitPlan::Write(WriteIntent::TimelineSegmentSummary {
        result: Value::Object(result),
        binding: state.binding.clone(),
        source: Box::new(state.source.clone()),
        input_digest: state.input_digest.clone(),
        provenance,
    }))
}

pub fn apply_result(
    journal: &Path,
    result: &Value,
    binding: SegmentBindingV1,
    source: SegmentSourceV1,
    input_digest: String,
    provenance: GenerationProvenanceV1,
) -> Result<(), String> {
    let generated_at_ms = chrono::Utc::now().timestamp_millis();
    let timeline = SegmentTimelineV1 {
        schema_version: CURRENT_SCHEMA_VERSION,
        kind: TimelineKind::Segment,
        summary: SegmentSummaryV1 {
            title: result
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            description: result
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            origin: origin_for_binding(&binding).map_err(|error| error.to_string())?,
            continuation_of: None,
        },
        binding,
        input_digest: input_digest.clone(),
        source: Some(source),
        generated_at_ms,
        provenance: Some(provenance),
    };
    let attempt = AttemptStateV1 {
        attempt_id: new_attempt_id("segment"),
        input_digest,
        started_at_ms: generated_at_ms,
        finished_at_ms: None,
        outcome: AttemptOutcome::Running,
        detail: String::new(),
    };
    publish_segment_timeline(journal, &timeline, attempt).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;

    use super::*;
    use crate::contract::{CommitDisposition, TIMELINE_SEGMENT_SUMMARY};
    use crate::generate_and_write;

    fn prepared(refresh: bool) -> PreparedTalent {
        PreparedTalent {
            name: "timeline:segment_summary".to_owned(),
            config: Map::from_iter([
                ("day".to_owned(), json!("20260101")),
                ("segment".to_owned(), json!("090000_300")),
                ("model".to_owned(), json!("test-model")),
                (
                    "prompt".to_owned(),
                    json!("Segment: $segment_rel_path\nActivity: $activity_text"),
                ),
                ("refresh".to_owned(), json!(refresh)),
            ]),
        }
    }

    fn seeded_context(root: &tempfile::TempDir) -> ExecutionContext {
        let segment = root.path().join("chronicle/20260101/090000_300");
        fs::create_dir_all(segment.join("talents")).unwrap();
        fs::write(
            segment.join("talents/activity.md"),
            "Completed focused work.\n",
        )
        .unwrap();
        ExecutionContext {
            journal: root.path().to_owned(),
        }
    }

    #[test]
    fn stage_builds_vars_commits_intent_and_persists_timeline() {
        // Derived from solstone/apps/timeline/talent/segment_summary.py:189-208.
        let root = tempfile::tempdir().unwrap();
        let context = seeded_context(&root);
        let mut prepared = prepared(false);

        assert_eq!(gate(&prepared, &context).unwrap(), GateDecision::Proceed);
        let state = build(&mut prepared, &context).unwrap();
        apply_prompt_override(&mut prepared, &state).unwrap();
        assert_eq!(
            prepared.config["prompt"],
            "Segment: 20260101/090000_300\nActivity: Completed focused work.\n"
        );
        let client =
            solstone_core_generate::OneShotClient::at_path(crate::test_support::one_shot_stub(
                root.path(),
                r#"{"title":"Focus complete","description":"Completes focused work."}"#,
            ));
        let mut sink = Vec::new();
        let cogitate = solstone_core_cogitate_wire::CogitateOneShotClient::at_path(
            root.path().join("unused-cogitate"),
        );
        let outcome = generate_and_write(
            &mut prepared,
            &context,
            &client,
            &cogitate,
            &mut sink,
            crate::cogitate::EngineKind::Generate,
            Some((&TIMELINE_SEGMENT_SUMMARY, state)),
        );
        assert!(matches!(
            outcome,
            RuntimeOutcome::Finished {
                disposition: CommitDisposition::CommittedNoOutput,
                ..
            }
        ));
        let timeline: solstone_core_timeline::SegmentTimelineV1 = serde_json::from_str(
            &fs::read_to_string(
                root.path()
                    .join("chronicle/20260101/090000_300/timeline.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(timeline.summary.title, "Focus complete");
        assert_eq!(timeline.summary.origin, "20260101/090000_300");
        let provenance = timeline.provenance.expect("generated provenance persists");
        assert_eq!(provenance.model, "test-model");
        assert_eq!(provenance.finish_reason, "stop");
        assert_eq!(provenance.schema_validation, Value::Null);
        assert_eq!(provenance.inference, Value::Null);
        assert_eq!(provenance.usage, json!({}));
        assert!(!timeline.input_digest.is_empty());
        assert_eq!(
            gate(&prepared, &context).unwrap(),
            GateDecision::Skip("timeline_current".to_owned())
        );
    }

    #[test]
    fn gate_skips_missing_activity_and_rebuilds_legacy_timeline_without_refresh() {
        // Derived from solstone/apps/timeline/talent/segment_summary.py:189-205.
        let root = tempfile::tempdir().unwrap();
        let context = ExecutionContext {
            journal: root.path().to_owned(),
        };
        assert_eq!(
            gate(&prepared(false), &context).unwrap(),
            GateDecision::Skip("no_activity_md".to_owned())
        );

        let context = seeded_context(&root);
        let timeline = root
            .path()
            .join("chronicle/20260101/090000_300/timeline.json");
        fs::write(timeline, "{}\n").unwrap();
        assert_eq!(
            gate(&prepared(false), &context).unwrap(),
            GateDecision::Proceed
        );
        assert_eq!(
            gate(&prepared(true), &context).unwrap(),
            GateDecision::Proceed
        );
    }

    #[test]
    fn explicit_stale_stream_does_not_fall_back_to_another_segment_layout() {
        let root = tempfile::tempdir().unwrap();
        let segment = root.path().join("chronicle/20260101/actual/090000_300");
        fs::create_dir_all(segment.join("talents")).unwrap();
        fs::write(segment.join("talents/activity.md"), "Recovered activity.\n").unwrap();
        let context = ExecutionContext {
            journal: root.path().to_owned(),
        };
        let mut prepared = prepared(false);
        prepared.config.insert("stream".to_owned(), json!("stale"));

        assert!(
            matches!(gate(&prepared, &context), Err(StageError { detail, .. }) if detail.contains("not found"))
        );
    }

    #[test]
    fn activity_change_between_generation_and_apply_fails_closed() {
        let root = tempfile::tempdir().unwrap();
        let context = seeded_context(&root);
        let mut prepared = prepared(false);
        let mut state = build(&mut prepared, &context).unwrap();
        let PrePostState::Timeline(timeline_state) = &state else {
            panic!("timeline pre-state");
        };
        let original_digest = timeline_state.input_digest.clone();
        attach_generated_provenance(
            &mut state,
            &solstone_core_generate::GeneratedResponse {
                id: None,
                text: r#"{"title":"Stale","description":"V1"}"#.to_owned(),
                model: "test-model".to_owned(),
                usage: json!({}),
                finish_reason: "stop".to_owned(),
                thinking: None,
                schema_validation: Some(json!({"valid": true, "errors": []})),
                input_budget: None,
                request_budget: None,
                inference: None,
                hints_applied: Vec::new(),
            },
        )
        .unwrap();
        let plan = commit(
            ParsedOutput::Text(r#"{"title":"Stale","description":"V1"}"#.to_owned()),
            &prepared,
            &state,
        )
        .unwrap();
        fs::write(
            root.path()
                .join("chronicle/20260101/090000_300/talents/activity.md"),
            "Changed after generation.\n",
        )
        .unwrap();

        let error = crate::writers::apply(plan, &context)
            .expect_err("stale generated output must not commit");
        assert!(error.detail.contains("source"), "{}", error.detail);
        assert!(
            !root
                .path()
                .join("chronicle/20260101/090000_300/timeline.json")
                .exists()
        );
        let state = solstone_core_timeline::load_timeline_record(
            root.path(),
            "segment:20260101/_default/090000_300",
        )
        .unwrap()
        .unwrap();
        assert!(state.attempts.iter().any(|attempt| {
            attempt.outcome == AttemptOutcome::Failed && attempt.input_digest != original_digest
        }));
    }
    #[test]
    fn conversion_gate_stops_unchanged_activity_and_refresh_before_build() {
        let root = tempfile::tempdir().unwrap();
        let context = seeded_context(&root);
        fs::create_dir_all(root.path().join("health/timeline")).unwrap();
        fs::write(
            solstone_core_timeline::timeline_state_path(root.path()),
            b"legacy document",
        )
        .unwrap();
        for refresh in [false, true] {
            let mut prepared = prepared(refresh);
            assert!(
                matches!(gate(&prepared, &context).unwrap(), GateDecision::Skip(reason) if reason.contains("timeline_conversion_required"))
            );
            assert!(matches!(
                build(&mut prepared, &context),
                Err(RuntimeOutcome::StageFailed(_))
            ));
        }
        assert!(
            !root
                .path()
                .join("chronicle/20260101/090000_300/timeline.state.json")
                .exists()
        );
    }
}
