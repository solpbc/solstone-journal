// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Timeline segment-summary hook stage.

use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Map, Value};
use solstone_core_talent_config::is_truthy;
use solstone_core_timeline::{
    ArtifactCurrentness, AttemptOutcome, AttemptStateV1, CURRENT_SCHEMA_VERSION,
    GenerationProvenanceV1, SegmentBindingV1, SegmentSelectorV1, SegmentSummaryV1,
    SegmentTimelineV1, TimelineError, TimelineKind, evaluate_artifact_currentness,
    origin_for_binding, publish_segment_timeline, resolve_segment_binding, segment_directory,
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
    input_digest: String,
    provenance: Option<Box<GenerationProvenanceV1>>,
}

static ATTEMPT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

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
) -> Result<Option<(SegmentBindingV1, std::path::PathBuf, std::path::PathBuf)>, TimelineError> {
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
    ["talents/activity.md", "activity.md"]
        .into_iter()
        .map(|relative| segment_dir.join(relative))
        .find(|path| path.is_file())
        .map(|activity| (binding, segment_dir, activity))
        .map_or(Ok(None), |value| Ok(Some(value)))
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
    let Some((binding, segment_dir, activity_path)) = resolved else {
        return Ok(GateDecision::Skip("no_activity_md".to_owned()));
    };
    let Ok(activity_text) = fs::read_to_string(activity_path) else {
        return Ok(GateDecision::Skip("no_activity_md".to_owned()));
    };
    let input_digest = segment_input_digest(&binding, &activity_text).map_err(|error| {
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
    let Some((binding, _, activity_path)) = resolved else {
        return Err(skipped(prepared));
    };
    let Ok(activity_text) = fs::read_to_string(activity_path) else {
        return Err(skipped(prepared));
    };
    let input_digest = segment_input_digest(&binding, &activity_text).map_err(|error| {
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
        activity_text,
        segment_rel_path,
        binding,
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
        input_digest: state.input_digest.clone(),
        provenance,
    }))
}

pub fn apply_result(
    journal: &Path,
    result: &Value,
    binding: SegmentBindingV1,
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
        generated_at_ms,
        provenance: Some(provenance),
    };
    let sequence = ATTEMPT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let attempt = AttemptStateV1 {
        attempt_id: format!("segment-{}-{sequence}", std::process::id()),
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
}
