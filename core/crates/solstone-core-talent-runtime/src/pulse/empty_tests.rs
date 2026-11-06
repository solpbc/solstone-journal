// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

fn execute_fixture(
    shape: &str,
    response_kind: &str,
) -> (RuntimeOutcome, usize, Option<Value>, Vec<Value>) {
    let root = tempfile::tempdir().unwrap();
    let journal = root.path().join("journal");
    fs::create_dir_all(journal.join("config")).unwrap();
    fs::write(
        journal.join("config/journal.json"),
        r#"{"providers":{"active":{"provider":"test","model":"configured-model"}}}"#,
    )
    .unwrap();
    let mut segments = Vec::new();
    let mut activities = Vec::new();
    if ["segment", "whitespace", "missing"].contains(&shape) {
        segments
            .push(json!({"day":"20261105","segment":"120000_300","stream":"device_audio","ts":1}));
        if shape != "missing" {
            let dir = journal.join("chronicle/20261105/device_audio/120000_300/talents");
            fs::create_dir_all(&dir).unwrap();
            fs::write(
                dir.join("activity.md"),
                if shape == "whitespace" {
                    "  \n"
                } else {
                    "Rowan proposed a repair; approval remains open."
                },
            )
            .unwrap();
        }
    }
    if ["activity", "hidden"].contains(&shape) {
        let dir = journal.join("facets/work/activities");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("20261105.jsonl"),json!({"id":"a1","title":"Repair proposal","description":"Rowan proposed a repair; approval remains open.","hidden":shape=="hidden","source":"user"}).to_string()).unwrap();
        activities.push(json!({"day":"20261105","facet":"work","activity":"a1","ts":1}));
    }
    if !["segment", "activity"].contains(&shape) {
        let dir = journal.join("chronicle/20261105/talents");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("pulse.jsonl"),json!({"full_details":"Old background plan.","needs_you":["Unknown old obligation"],"ts":1}).to_string()+"\n").unwrap();
    }
    let calls = AtomicUsize::new(0);
    let executor = |_: &solstone_core_generate::GenerateRequest| {
        calls.fetch_add(1, Ordering::SeqCst);
        if response_kind == "transport" {
            return Err(solstone_core_generate::ClientError::Resolve(
                "fixture transport failure".into(),
            ));
        }
        let mut response = if response_kind == "refused" {
            crate::test_support::refused_response_value(
                Some("provider_request_rejected"),
                false,
                true,
                "test",
                "fixture refusal",
            )
        } else {
            let raw=json!({"title":"Repair proposal","one_sentence":"Rowan proposed a repair.","full_details":"Rowan proposed a repair; approval remains open.","needs_you":["Review the proposal."]}).to_string();
            crate::test_support::generated_response_value(
                &raw,
                if response_kind == "schema" {
                    json!({"valid":false,"errors":["fixture"]})
                } else {
                    json!({"valid":true,"errors":[]})
                },
            )
        };
        if response_kind == "clipped" {
            response["input_budget"] = json!({"clipped":true,"dropped_entries":1});
        }
        Ok(solstone_core_generate::decode_one_shot_response(&response.to_string()).unwrap())
    };
    let packaged = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../payload/solstone");
    let paths = crate::prepare::RuntimePaths {
        talent_root: packaged.join("talent"),
        apps_root: packaged.join("apps"),
        templates_dir: packaged.join("templates"),
    };
    let request = json!({"name":"pulse","prompt":"","day":"20261105","input_budget":{"clipped":true,"dropped_entries":999},"cadence_window":{"since_ms":7,"segments":segments,"activities":activities}});
    let mut events = Vec::new();
    let context = ExecutionContext {
        journal: journal.clone(),
    };
    let outcome = crate::execute_request(
        request.as_object().unwrap().clone(),
        &paths,
        &context,
        &executor,
        &mut events,
        &solstone_core_generate::Cancellation::never(),
    );
    let file = journal.join("chronicle/20261105/talents/pulse.jsonl");
    let record = file.is_file().then(|| {
        serde_json::from_str::<Value>(fs::read_to_string(file).unwrap().lines().last().unwrap())
            .unwrap()
    });
    if let Some(record) = &record {
        let home = solstone_core_home::HomeContext::new(&journal, Utc::now());
        let narrative = solstone_core_home::readers::load_pulse_narrative(&home, "20261105");
        assert_eq!(
            narrative.content.as_deref(),
            record["full_details"].as_str()
        );
        assert!(narrative.updated_at.is_some());
        if !["segment", "activity"].contains(&shape) {
            assert!(narrative.needs.is_empty());
        }
    }
    (
        outcome,
        calls.load(Ordering::SeqCst),
        record,
        String::from_utf8(events)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect(),
    )
}

#[test]
fn empty_retained_evidence_publishes_default_without_generation() {
    for shape in ["none", "missing", "whitespace", "hidden"] {
        let (outcome, calls, record, events) = execute_fixture(shape, "transport");
        assert_eq!(calls, 0, "{shape}");
        let RuntimeOutcome::Finished { output, usage, .. } = outcome else {
            panic!("{outcome:?}")
        };
        assert_eq!(output, "");
        assert!(usage.is_none());
        assert!(
            !events
                .iter()
                .any(|e| e["event"] == "generate_input" || e["event"] == "generate_call")
        );
        assert!(
            events
                .iter()
                .any(|e| e["event"] == "unavailable_selection" && e["trigger"] == "no_activity")
        );
        let record = record.unwrap();
        let default = summary_value(&default_pulse());
        for key in ["title", "one_sentence", "full_details", "needs_you"] {
            assert_eq!(record[key], default[key]);
        }
        assert!(record["model"].is_null());
        assert!(record.get("generated_at").is_none());
        assert!(record.get("partial_input").is_none());
        assert!(record["ts"].is_i64());
        assert_eq!(record["window"]["segments"], 0);
        assert_eq!(record["window"]["activities"], 0);
        assert_eq!(record["window"]["since_ms"], 7);
        assert_eq!(
            record["window"]["input_segments"],
            if ["missing", "whitespace"].contains(&shape) {
                1
            } else {
                0
            }
        );
        assert_eq!(
            record["window"]["input_activities"],
            if shape == "hidden" { 1 } else { 0 }
        );
    }
}

#[test]
fn nonempty_success_and_clipping_keep_normal_publication() {
    for shape in ["segment", "activity"] {
        for response in ["success", "clipped"] {
            let (outcome, calls, record, events) = execute_fixture(shape, response);
            assert_eq!(calls, 1);
            let RuntimeOutcome::Finished { output, .. } = outcome else {
                panic!("{outcome:?}")
            };
            let output: Value = serde_json::from_str(&output).unwrap();
            let record = record.unwrap();
            assert_eq!(output["needs_you"], json!(["Review the proposal."]));
            for key in ["title", "one_sentence", "full_details", "needs_you"] {
                assert_eq!(record[key], output[key]);
            }
            assert_eq!(record["model"], "configured-model");
            assert!(record["generated_at"].is_string());
            if response == "clipped" {
                assert_eq!(
                    record["partial_input"],
                    json!({"dropped_entries":1,"dropped_chars":null})
                );
                assert_eq!(output["partial_input"], record["partial_input"]);
            } else {
                assert!(record.get("partial_input").is_none());
                assert!(output.get("partial_input").is_none());
            }
            assert!(!events.iter().any(|e| e["event"] == "unavailable_selection"));
        }
    }
}

#[test]
fn nonempty_refusal_schema_and_transport_keep_failure_paths() {
    assert!(crate::contract::PULSE.unavailable_commit.is_none());
    for kind in ["refused", "schema", "transport"] {
        let (outcome, calls, record, events) = execute_fixture("segment", kind);
        assert!(record.is_none());
        assert!(!events.iter().any(|e| e["event"] == "unavailable_selection"));
        match kind {
            "refused" => assert!(matches!(outcome, RuntimeOutcome::GenerateRefused { .. })),
            "schema" => assert!(matches!(
                outcome,
                RuntimeOutcome::SchemaValidationFailed { .. }
            )),
            "transport" => assert!(matches!(outcome, RuntimeOutcome::StageFailed(_))),
            _ => unreachable!(),
        }
        assert_eq!(
            calls,
            if kind == "schema" {
                crate::VALIDATION_RETRY_ATTEMPTS + 1
            } else {
                1
            }
        );
    }
}

#[test]
fn empty_previews_report_no_generation_without_publishing() {
    use solstone_core_talent_cli::preview::{PreviewRequest, PromptPreview};
    let root = tempfile::tempdir().unwrap();
    let journal = root.path().join("journal");
    fs::create_dir_all(journal.join("config")).unwrap();
    fs::write(journal.join("config/journal.json"), "{}").unwrap();
    let packaged = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../payload/solstone");
    let paths = crate::prepare::RuntimePaths {
        talent_root: packaged.join("talent"),
        apps_root: packaged.join("apps"),
        templates_dir: packaged.join("templates"),
    };
    let context = ExecutionContext {
        journal: journal.clone(),
    };
    for (name, reason) in [
        ("pulse", "no_activity"),
        ("weekly_reflection", "zero_slots"),
    ] {
        let request = PreviewRequest {
            name: name.into(),
            day: Some("20260308".into()),
            segment: None,
            facet: None,
            activity: None,
        };
        let preview = crate::assemble::assemble_prompt_preview(&request, &paths, &context);
        assert_eq!(
            preview,
            PromptPreview::WouldNotRun {
                reason: reason.into()
            }
        );
        assert!(!journal.join("chronicle").exists());
        assert_eq!(
            fs::read_to_string(journal.join("config/journal.json")).unwrap(),
            "{}"
        );
        assert_eq!(fs::read_dir(&journal).unwrap().count(), 1);
    }
}
