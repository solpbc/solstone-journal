// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Timeline segment-summary hook stage.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use solstone_core_system_health::find_segment_dir;
use solstone_core_talent_config::is_truthy;

use crate::contract::{CommitPlan, GateDecision, ParsedOutput, PrePostState};
use crate::writers::WriteIntent;
use crate::{
    ExecutionContext, PreparedTalent, RuntimeOutcome, StageError, apply_template_vars, stage_error,
};

#[derive(Clone, Debug, PartialEq)]
pub struct TimelinePreState {
    activity_text: String,
    segment_rel_path: String,
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

fn segment_dir(journal: &Path, day: &str, segment: &str, stream: Option<&str>) -> Option<PathBuf> {
    stream
        .filter(|stream| !stream.is_empty())
        .and_then(|stream| find_segment_dir(journal, day, segment, Some(stream)))
        .or_else(|| find_segment_dir(journal, day, segment, None))
}

fn resolve_activity(
    journal: &Path,
    day: &str,
    segment: &str,
    stream: Option<&str>,
) -> Option<(PathBuf, PathBuf)> {
    let segment_dir = segment_dir(journal, day, segment, stream)?;
    ["talents/activity.md", "activity.md"]
        .into_iter()
        .map(|relative| segment_dir.join(relative))
        .find(|path| path.is_file())
        .map(|activity| (segment_dir, activity))
}

pub fn gate(
    prepared: &PreparedTalent,
    context: &ExecutionContext,
) -> Result<GateDecision, StageError> {
    let Some((day, segment, stream)) = fields(prepared) else {
        return Ok(GateDecision::Skip("no_activity_md".to_owned()));
    };
    let Some((segment_dir, activity_path)) =
        resolve_activity(&context.journal, day, segment, stream)
    else {
        return Ok(GateDecision::Skip("no_activity_md".to_owned()));
    };
    if segment_dir.join("timeline.json").exists()
        && !prepared.config.get("refresh").is_some_and(is_truthy)
    {
        return Ok(GateDecision::Skip("timeline_exists".to_owned()));
    }
    if fs::read_to_string(activity_path).is_err() {
        return Ok(GateDecision::Skip("no_activity_md".to_owned()));
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
    let Some((segment_dir, activity_path)) =
        resolve_activity(&context.journal, day, segment, stream)
    else {
        return Err(skipped(prepared));
    };
    let Ok(activity_text) = fs::read_to_string(activity_path) else {
        return Err(skipped(prepared));
    };
    Ok(PrePostState::Timeline(TimelinePreState {
        activity_text,
        segment_rel_path: solstone_core_maintenance::bodies::timeline::origin_for_segment(
            &segment_dir,
        ),
    }))
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
    _: &PrePostState,
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
    let Some((day, segment, stream)) = fields(prepared) else {
        return Ok(CommitPlan::NoOutput);
    };
    Ok(CommitPlan::Write(WriteIntent::TimelineSegmentSummary {
        result: Value::Object(result),
        day: day.to_owned(),
        segment: segment.to_owned(),
        stream: stream.map(str::to_owned),
        model: prepared
            .config
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
    }))
}

pub fn apply_result(
    journal: &Path,
    result: &Value,
    day: &str,
    segment: &str,
    stream: Option<&str>,
    model: &str,
) -> Result<(), String> {
    let Some(segment_dir) = segment_dir(journal, day, segment, stream) else {
        return Ok(());
    };
    let payload = json!({
        "title": result.get("title").and_then(Value::as_str).unwrap_or_default(),
        "description": result.get("description").and_then(Value::as_str).unwrap_or_default(),
        "origin": solstone_core_maintenance::bodies::timeline::origin_for_segment(&segment_dir),
        "model": model,
        "generated_at": chrono::Utc::now().timestamp(),
    });
    solstone_core_maintenance::bodies::timeline::write_segment_timeline(&segment_dir, &payload)
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
        let plan = commit(
            parse(
                r#"{"title":"Focus complete","description":"Completes focused work."}"#,
                &prepared,
                &state,
            )
            .unwrap(),
            &prepared,
            &state,
        )
        .unwrap();
        assert!(matches!(
            plan,
            CommitPlan::Write(WriteIntent::TimelineSegmentSummary { .. })
        ));

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
        let timeline: Value = serde_json::from_str(
            &fs::read_to_string(
                root.path()
                    .join("chronicle/20260101/090000_300/timeline.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(timeline["title"], "Focus complete");
        assert_eq!(timeline["origin"], "20260101/090000_300");
        assert_eq!(timeline["model"], "test-model");
    }

    #[test]
    fn gate_skips_missing_activity_and_existing_timeline_without_refresh() {
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
            GateDecision::Skip("timeline_exists".to_owned())
        );
        assert_eq!(
            gate(&prepared(true), &context).unwrap(),
            GateDecision::Proceed
        );
    }

    #[test]
    fn stale_stream_falls_back_to_the_matching_segment() {
        // Derived from solstone/apps/timeline/talent/segment_summary.py:91-133.
        let root = tempfile::tempdir().unwrap();
        let segment = root.path().join("chronicle/20260101/actual/090000_300");
        fs::create_dir_all(segment.join("talents")).unwrap();
        fs::write(segment.join("talents/activity.md"), "Recovered activity.\n").unwrap();
        let context = ExecutionContext {
            journal: root.path().to_owned(),
        };
        let mut prepared = prepared(false);
        prepared.config.insert("stream".to_owned(), json!("stale"));

        assert_eq!(gate(&prepared, &context).unwrap(), GateDecision::Proceed);
        let state = build(&mut prepared, &context).unwrap();
        apply_prompt_override(&mut prepared, &state).unwrap();
        assert!(
            prepared.config["prompt"]
                .as_str()
                .unwrap()
                .contains("Segment: 20260101/actual/090000_300\nActivity: Recovered activity.")
        );
    }

    #[test]
    fn continuation_summary_is_publicly_reachable_from_maintenance() {
        // Derived from solstone/apps/timeline/talent/segment_summary.py:70-88.
        let root = tempfile::tempdir().unwrap();
        let segment = root.path().join("chronicle/20260101/090000_300");
        fs::create_dir_all(&segment).unwrap();

        solstone_core_maintenance::bodies::timeline::write_continuation_summary(
            &segment,
            "080000_300",
        )
        .unwrap();
        let timeline = segment.join("timeline.json");
        let first = fs::read(&timeline).unwrap();
        solstone_core_maintenance::bodies::timeline::write_continuation_summary(
            &segment,
            "080000_300",
        )
        .unwrap();
        assert_eq!(fs::read(&timeline).unwrap(), first);
        assert_eq!(
            serde_json::from_slice::<Value>(&first).unwrap(),
            json!({
                "title":"Continued",
                "description":"Unchanged from the prior window.",
                "origin":"20260101/090000_300",
                "continuation_of":"080000_300",
            })
        );
    }
}
