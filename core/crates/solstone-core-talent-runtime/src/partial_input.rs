// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::{PreparedTalent, contract::CommitPlan, writers::WriteIntent};
use serde_json::{Map, Value, json};

use solstone_core_format::content::PARTIAL_INPUT_NOTICE as NOTICE;

pub(crate) fn marker(prepared: &PreparedTalent) -> Option<Value> {
    prepared
        .config
        .get(crate::INPUT_BUDGET_KEY)
        .and_then(from_budget)
}

pub(crate) fn from_budget(budget: &Value) -> Option<Value> {
    if budget.get("clipped").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    let mut value = Map::new();
    for key in ["dropped_entries", "dropped_chars"] {
        value.insert(
            key.into(),
            budget
                .get(key)
                .filter(|v| v.as_u64().is_some())
                .cloned()
                .unwrap_or(Value::Null),
        );
    }
    Some(Value::Object(value))
}

pub(crate) fn stamp(value: &mut Map<String, Value>, marker: Option<&Value>) {
    value.remove("partial_input");
    if let Some(marker) = marker {
        value.insert("partial_input".into(), marker.clone());
    }
}

pub(crate) fn json_text(text: &str, marker: Option<&Value>) -> String {
    let Ok(Value::Object(mut value)) = serde_json::from_str(text) else {
        return text.to_owned();
    };
    if value.get("partial_input") == marker {
        return text.to_owned();
    }
    stamp(&mut value, marker);
    Value::Object(value).to_string()
}

pub(crate) fn markdown(text: &str, marker: Option<&Value>) -> String {
    if marker.is_some() {
        format!("{}\n\n{NOTICE}\n", text.trim_end())
    } else {
        text.to_owned()
    }
}

pub(crate) fn output(text: &str, prepared: &PreparedTalent) -> String {
    let marker = marker(prepared);
    if prepared.config.get("output").and_then(Value::as_str) == Some("json")
        || prepared.config.contains_key("json_schema")
    {
        json_text(text, marker.as_ref())
    } else {
        markdown(text, marker.as_ref())
    }
}

pub(crate) fn degraded(
    prepared: &PreparedTalent,
    previous: Option<Box<Value>>,
) -> Option<Box<Value>> {
    previous.or_else(|| {
        marker(prepared)
            .map(|partial| Box::new(json!({"reason":"partial_input", "partial_input":partial})))
    })
}

pub(crate) fn decorate(mut plan: CommitPlan, prepared: &PreparedTalent) -> CommitPlan {
    let partial = marker(prepared);
    if let CommitPlan::Write(intent) = &mut plan {
        match intent {
            WriteIntent::DayAccumulator { record, .. } => stamp(record, partial.as_ref()),
            WriteIntent::Story { value, .. } => {
                if let Some(value) = value.as_object_mut() {
                    stamp(value, partial.as_ref());
                }
            }
            WriteIntent::DailySchedule { output, .. }
            | WriteIntent::Participation { output, .. }
            | WriteIntent::EntityDetection { output, .. }
            | WriteIntent::EntitiesReview { output, .. }
            | WriteIntent::EntitySuggest { output, .. }
            | WriteIntent::EntityObserver { output, .. }
            | WriteIntent::SpeakerAttribution { output, .. } => {
                *output = json_text(output, partial.as_ref());
            }
            WriteIntent::WeeklyReflection {
                markdown: body,
                document,
                ..
            } => {
                *body = markdown(body, partial.as_ref());
                *document = json_text(document, partial.as_ref());
            }
        }
    }
    plan
}

/// These bytes have already crossed the runtime's post-parse authority boundary.
pub(crate) fn copy_marker(output: &str, target: &mut Map<String, Value>) {
    let value = serde_json::from_str::<Value>(output).ok();
    stamp(
        target,
        value
            .as_ref()
            .and_then(|v| v.get("partial_input"))
            .filter(|v| v.is_object()),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ExecutionContext, RuntimeOutcome};
    use solstone_core_generate::{ClientError, GenerateRequest, GenerateResponse};
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn prepared(path: &std::path::Path) -> PreparedTalent {
        PreparedTalent { name:"sense".into(), config:json!({"output":"json","output_path":path,"max_output_tokens":512,"prompt":"evidence","json_schema":{"type":"object"},"input_budget":{"clipped":true,"dropped_entries":999}}).as_object().unwrap().clone() }
    }

    #[test]
    fn accepted_attempt_owns_saved_returned_and_completion_markers() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("result.json");
        let mut prepared = prepared(&path);
        let context = ExecutionContext {
            journal: root.path().to_owned(),
        };
        // Reuse one destination in both directions, including an unreported budget.
        for budget in [
            json!({"clipped":true,"dropped_entries":2,"dropped_chars":31}),
            json!({"clipped":false}),
            json!({"clipped":true,"dropped_entries":3}),
            Value::Null,
        ] {
            let execute = |_: &GenerateRequest| -> Result<GenerateResponse, ClientError> {
                let mut value = crate::test_support::generated_response_value(
                    r#"{"body":"kept","partial_input":{"forged":true}}"#,
                    json!({"valid":true}),
                );
                value["input_budget"] = budget.clone();
                Ok(solstone_core_generate::decode_one_shot_response(&value.to_string()).unwrap())
            };
            let outcome = crate::generate_and_write(
                &mut prepared,
                &context,
                &execute,
                &mut Vec::new(),
                None,
                &solstone_core_generate::Cancellation::never(),
            );
            let RuntimeOutcome::Finished {
                output, degraded, ..
            } = outcome
            else {
                panic!("{outcome:?}");
            };
            let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            assert_eq!(serde_json::from_str::<Value>(&output).unwrap(), saved);
            assert_eq!(saved["body"], "kept");
            assert_eq!(saved.get("partial_input"), from_budget(&budget).as_ref());
            assert_eq!(degraded.is_some(), from_budget(&budget).is_some());
            assert!(saved["partial_input"].get("forged").is_none());
        }
        let ordinal = AtomicUsize::new(0);
        let execute = |_: &GenerateRequest| -> Result<GenerateResponse, ClientError> {
            let first = ordinal.fetch_add(1, Ordering::SeqCst) == 0;
            let mut value =
                crate::test_support::generated_response_value("{}", json!({"valid":!first}));
            value["input_budget"] = json!({"clipped":first,"dropped_entries":10});
            Ok(solstone_core_generate::decode_one_shot_response(&value.to_string()).unwrap())
        };
        let outcome = crate::generate_and_write(
            &mut prepared,
            &context,
            &execute,
            &mut Vec::new(),
            None,
            &solstone_core_generate::Cancellation::never(),
        );
        assert!(matches!(
            outcome,
            RuntimeOutcome::Finished { degraded: None, .. }
        ));
        assert_eq!(ordinal.load(Ordering::SeqCst), 2);
        assert!(
            !std::fs::read_to_string(path)
                .unwrap()
                .contains("partial_input")
        );
    }

    #[test]
    fn declaration_controls_output_shape_and_unknown_counts_stay_bounded() {
        let root = tempfile::tempdir().unwrap();
        let mut prepared = prepared(&root.path().join("result"));
        prepared.config.insert(crate::INPUT_BUDGET_KEY.into(),json!({"clipped":true,"dropped_entries":-1,"dropped_chars":"bad","extra":"x".repeat(10000)}));
        assert_eq!(
            marker(&prepared),
            Some(json!({"dropped_entries":null,"dropped_chars":null}))
        );
        assert_eq!(output("[]", &prepared), "[]");
        prepared.config.remove("json_schema");
        prepared.config.remove("output");
        assert!(output("{}", &prepared).contains(NOTICE));
        assert_eq!(
            degraded(&prepared, Some(Box::new(json!({"reason":"unavailable"})))).unwrap()["reason"],
            "unavailable"
        );
    }
    #[test]
    fn owned_outcome_files_keep_and_clear_the_marker() {
        use std::collections::{BTreeMap, BTreeSet};
        let root = tempfile::tempdir().unwrap();
        let context = ExecutionContext {
            journal: root.path().to_owned(),
        };
        solstone_core_facets::create_facet(root.path(), "work", "Work", "", "", "", None).unwrap();
        std::fs::create_dir_all(root.path().join("chronicle/20260101/090000_300/talents")).unwrap();
        let mut prepared = prepared(&root.path().join("unused.json"));
        let cancel = solstone_core_generate::Cancellation::never();
        for clipped in [true, false, true] {
            prepared.config.insert(
                crate::INPUT_BUDGET_KEY.into(),
                json!({"clipped":clipped,"dropped_entries":2}),
            );
            let cases = [
                (
                    WriteIntent::DayAccumulator {
                        day: "20260101".into(),
                        agent: "pulse".into(),
                        record: json!({"title":"Kept"}).as_object().unwrap().clone(),
                    },
                    "chronicle/20260101/talents/pulse.jsonl",
                ),
                (
                    WriteIntent::EntitySuggest {
                        output: r#"{"entities":[]}"#.into(),
                        facet: "work".into(),
                        day: "20260101".into(),
                    },
                    "facets/work/entities/20260101_observer_suggestions.json",
                ),
                (
                    WriteIntent::EntityDetection {
                        output: r#"{"detections":[]}"#.into(),
                        day: "20260101".into(),
                        segment: "090000_300".into(),
                        stream: None,
                    },
                    "chronicle/20260101/090000_300/talents/detection_outcome.json",
                ),
                (
                    WriteIntent::EntitiesReview {
                        output: r#"{"promotions":[],"merges":[]}"#.into(),
                        facet: "work".into(),
                        day: "20260101".into(),
                    },
                    "facets/work/entities/20260101_review_outcome.json",
                ),
                (
                    WriteIntent::EntityObserver {
                        output: r#"{"entities":[]}"#.into(),
                        facet: "work".into(),
                        day: "20260101".into(),
                        served_ids: BTreeSet::new(),
                        shown_observation_ids: BTreeMap::new(),
                    },
                    "facets/work/entities/20260101_observer_outcome.json",
                ),
                (
                    WriteIntent::DailySchedule {
                        output: r#"{"primary":"03:00"}"#.into(),
                        output_path: Some(
                            root.path()
                                .join("daily.json")
                                .to_string_lossy()
                                .into_owned(),
                        ),
                    },
                    "daily.json",
                ),
            ];
            for (intent, path) in cases {
                crate::writers::apply_prepared(
                    CommitPlan::Write(intent),
                    &context,
                    &prepared,
                    &cancel,
                )
                .unwrap();
                let text = std::fs::read_to_string(root.path().join(path)).unwrap();
                let last: Value = serde_json::from_str(text.lines().last().unwrap()).unwrap();
                assert_eq!(
                    last.get("partial_input").is_some(),
                    clipped,
                    "{path}: {last}"
                );
            }
        }
    }
}
