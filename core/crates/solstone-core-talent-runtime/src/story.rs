// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Map, Value, json};

use crate::contract::{CommitPlan, ParsedOutput, PrePostState};
use crate::{PreparedTalent, StageError, stage_error};

const CLOSURES: &[&str] = &["sent", "done", "signed", "dropped", "deferred"];
pub(crate) const RELATIONS: &[&str] = &[
    "works-with",
    "works-at",
    "reports-to",
    "family-of",
    "knows",
    "uses",
    "created",
    "other",
];

/// Empty body or topics is a parse error, not a silent merge.
/// `generate_and_write` turns a Story parse error into
/// `Finished`/`RejectedNoMutation` rather than `StageFailed`.
pub fn parse(
    output: &str,
    prepared: &PreparedTalent,
    _: &PrePostState,
) -> Result<ParsedOutput, StageError> {
    let mut value: Value = serde_json::from_str(output.trim())
        .map_err(|_| error(prepared, "story output is not JSON"))?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| error(prepared, "story output is not an object"))?;
    let body = object
        .get("body")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .ok_or_else(|| error(prepared, "story output has invalid body"))?
        .to_owned();
    let topics = object
        .get("topics")
        .and_then(Value::as_array)
        .ok_or_else(|| error(prepared, "story output has invalid topics"))?;
    let mut clean_topics = Vec::new();
    for topic in topics {
        let topic = topic
            .as_str()
            .ok_or_else(|| error(prepared, "story output has invalid topics"))?
            .trim()
            .to_lowercase();
        if !topic.is_empty() && !clean_topics.contains(&topic) && clean_topics.len() < 10 {
            clean_topics.push(topic);
        }
    }
    if clean_topics.is_empty() {
        return Err(error(prepared, "story output has invalid topics"));
    }
    let confidence = object
        .get("confidence")
        .and_then(Value::as_f64)
        .filter(|number| !number.is_nan())
        .ok_or_else(|| error(prepared, "story output has invalid confidence"))?;
    let clamped = confidence.clamp(0.0, 1.0);
    if clamped != confidence {
        log::warn!("story hook: clamped confidence {confidence} to {clamped}");
    }
    for field in ["commitments", "closures", "decisions", "relations"] {
        if !object.get(field).is_some_and(Value::is_array) {
            return Err(error(
                prepared,
                &format!("story output has invalid {field}"),
            ));
        }
    }
    object.insert("body".into(), Value::String(body));
    object.insert("topics".into(), json!(clean_topics));
    object.insert("confidence".into(), json!(clamped));
    Ok(ParsedOutput::Json(value))
}

pub fn commit(
    parsed: ParsedOutput,
    prepared: &PreparedTalent,
    _: &PrePostState,
) -> Result<CommitPlan, StageError> {
    let ParsedOutput::Json(mut value) = parsed else {
        return Err(error(prepared, "story output is not JSON"));
    };
    // Only the runtime says a story's input was partial; never the model.
    let object = value
        .as_object_mut()
        .ok_or_else(|| error(prepared, "story output is not an object"))?;
    object.remove("partial_input");
    if let Some(budget) = prepared.config.get(crate::INPUT_BUDGET_KEY) {
        object.insert(
            "partial_input".into(),
            json!({
                "dropped_entries": budget.get("dropped_entries").cloned().unwrap_or(Value::Null),
                "dropped_chars": budget.get("dropped_chars").cloned().unwrap_or(Value::Null),
            }),
        );
    }
    let facet = required(prepared, "facet")?;
    let day = required(prepared, "day")?;
    let record_id = prepared
        .config
        .get("activity")
        .and_then(Value::as_object)
        .and_then(|item| item.get("id"))
        .and_then(Value::as_str)
        .ok_or_else(|| error(prepared, "story is missing activity record id"))?;
    // Python returns "" after the mutation; CommittedNoOutput is explicit.
    Ok(CommitPlan::Write(crate::writers::WriteIntent::Story {
        destination_id: required(prepared, "destination_id")?.to_owned(),
        talent: prepared.name.clone(),
        facet: facet.to_owned(),
        day: day.to_owned(),
        record_id: record_id.to_owned(),
        value,
    }))
}

pub fn apply_story(
    root: &std::path::Path,
    talent: &str,
    facet: &str,
    destination_id: &str,
    day: &str,
    record_id: &str,
    value: &Value,
) -> Result<(), String> {
    let _guard = solstone_core_facets::hold_activity_enrichment(root, facet, destination_id)
        .map_err(|error| error.to_string())?;
    if solstone_core_facets::get_activity_record(root, facet, day, record_id)
        .map_err(|error| error.to_string())?
        .is_none()
    {
        return Err("activity no longer exists".to_owned());
    }
    let entities = crate::detected_resolution_entities(root, facet, day)?;
    let resolve = |name: &str, field: &str| -> Result<Value, String> {
        let result = solstone_core_entity::record_entity_resolution(root, name, &entities, json!({"kind":"facet","facet":facet}), json!({"lane":"talent.story","facet":facet,"day":day,"record_id":record_id,"field":field}), 90.0, false).map_err(|error| error.to_string())?;
        Ok(
            if matches!(
                result.outcome,
                solstone_core_entity::EntityResolutionOutcome::Resolved
            ) {
                result
                    .entity_index
                    .and_then(|index| entities[index].id.clone())
                    .map_or(Value::Null, Value::String)
            } else {
                Value::Null
            },
        )
    };
    let rows = |group: &str, required: &[&str]| -> Result<Vec<Value>, String> {
        let mut output = Vec::new();
        for item in value[group].as_array().unwrap() {
            let Some(source) = item.as_object() else {
                log::warn!("story hook: skipping {group}: expected object");
                continue;
            };
            if required
                .iter()
                .any(|field| !source.get(*field).is_some_and(Value::is_string))
            {
                log::warn!("story hook: skipping {group}: missing required string field");
                continue;
            }
            let mut row = source.clone();
            if group == "closures" && !CLOSURES.contains(&row["resolution"].as_str().unwrap()) {
                log::warn!("story hook: skipping closure: invalid resolution");
                continue;
            }
            if group == "relations"
                && (!RELATIONS.contains(&row["kind"].as_str().unwrap())
                    || row["kind"] == "other" && row["note"].as_str().is_none_or(str::is_empty))
            {
                log::warn!("story hook: skipping relation: invalid kind");
                continue;
            }
            if group == "decisions"
                && row
                    .get("counterparty")
                    .is_some_and(|item| !item.is_string() && !item.is_null())
            {
                log::warn!("story hook: skipping decision: invalid counterparty");
                continue;
            }
            if group == "relations" && row.get("quote").is_some_and(|item| !item.is_string()) {
                log::warn!("story hook: skipping relation: invalid quote");
                continue;
            }
            for (from, to) in [
                ("owner", "owner_entity_id"),
                ("counterparty", "counterparty_entity_id"),
                ("from", "from_entity_id"),
                ("to", "to_entity_id"),
            ] {
                if let Some(name) = row.get(from).and_then(Value::as_str) {
                    row.insert(
                        to.into(),
                        if group == "decisions" && from == "counterparty" && name.trim().is_empty()
                        {
                            Value::Null
                        } else {
                            resolve(name, &format!("{group}.{from}"))?
                        },
                    );
                } else if group == "decisions" && from == "counterparty" {
                    row.insert(to.into(), Value::Null);
                }
            }
            output.push(Value::Object(row));
        }
        Ok(output)
    };
    let mut patch = Map::new();
    let mut story = json!({"talent":talent,"body":value["body"],"topics":value["topics"],"confidence":value["confidence"]});
    // A story written from a fitted, clipped request says so on the record.
    if let Some(partial) = value.get("partial_input").filter(|item| item.is_object()) {
        story["partial_input"] = partial.clone();
    }
    patch.insert("story".into(), story);
    patch.insert(
        "commitments".into(),
        Value::Array(rows(
            "commitments",
            &["owner", "action", "counterparty", "when", "context"],
        )?),
    );
    patch.insert(
        "closures".into(),
        Value::Array(rows(
            "closures",
            &["owner", "action", "counterparty", "resolution", "context"],
        )?),
    );
    patch.insert(
        "decisions".into(),
        Value::Array(rows("decisions", &["owner", "action", "context"])?),
    );
    patch.insert(
        "relations".into(),
        Value::Array(rows("relations", &["from", "to", "kind", "note"])?),
    );
    // Existing native writer encodes Python's null note as an empty string.
    solstone_core_facets::update_activity_record(
        root,
        facet,
        day,
        record_id,
        &patch,
        "story",
        "",
        &chrono::Utc::now().to_rfc3339(),
    )
    .map_err(|error| error.to_string())?
    .ok_or("activity no longer exists")?;
    Ok(())
}

fn required<'a>(prepared: &'a PreparedTalent, field: &str) -> Result<&'a str, StageError> {
    prepared
        .config
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| error(prepared, &format!("story is missing {field}")))
}
fn error(prepared: &PreparedTalent, detail: &str) -> StageError {
    stage_error("post-parse", "story", prepared, detail)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ExecutionContext;
    #[cfg(all(test, feature = "full-tests"))]
    use crate::contract::{CommitDisposition, STORY};
    #[cfg(all(test, feature = "full-tests"))]
    use crate::generate_and_write;
    use std::fs;
    #[test]
    fn decision_counterparty_contract_survives_the_story_writer() {
        let root = tempfile::tempdir().unwrap();
        solstone_core_facets::create_facet(root.path(), "work", "Work", "", "", "", None).unwrap();
        let activity_path = root.path().join("facets/work/activities/20260101.jsonl");
        fs::create_dir_all(activity_path.parent().unwrap()).unwrap();
        fs::write(&activity_path, "{\"id\":\"activity-1\"}\n").unwrap();
        let prepared = PreparedTalent {
            name: "conversation".into(),
            config: Map::from_iter([
                ("facet".into(), json!("work")),
                (
                    "destination_id".into(),
                    json!(
                        solstone_core_facets::observe_facet_write_identity(root.path(), "work")
                            .unwrap()
                    ),
                ),
                ("day".into(), json!("20260101")),
                ("activity".into(), json!({"id":"activity-1"})),
            ]),
        };
        let mut decisions = vec![json!({"owner":"Owner","action":"absent","context":"meeting"})];
        for (action, counterparty) in [
            ("null", Value::Null),
            ("empty", json!("")),
            ("named", json!("Someone")),
            ("number", json!(42)),
            ("object", json!({})),
            ("array", json!([])),
        ] {
            decisions.push(json!({"owner":"Owner", "action":action, "context":"meeting", "counterparty":counterparty}));
        }
        let output = json!({"body":"A completed conversation.","topics":["work"],"confidence":0.9,
            "commitments":[],"closures":[],"relations":[],"decisions":decisions})
        .to_string();
        let plan = commit(
            parse(&output, &prepared, &PrePostState::None).unwrap(),
            &prepared,
            &PrePostState::None,
        )
        .unwrap();
        crate::writers::apply(
            plan,
            &ExecutionContext {
                journal: root.path().into(),
            },
        )
        .unwrap();
        let record = solstone_core_facets::get_activity_record(
            root.path(),
            "work",
            "20260101",
            "activity-1",
        )
        .unwrap()
        .unwrap();
        let decisions = record["decisions"].as_array().unwrap();
        assert_eq!(
            decisions
                .iter()
                .map(|row| row["action"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["absent", "null", "empty", "named"]
        );
        assert!(decisions[1]["counterparty"].is_null());
        for row in &decisions[..3] {
            assert!(row["counterparty_entity_id"].is_null());
        }
    }

    #[test]
    fn criterion_22_story_stage_uses_plain_resolution_for_written_id() {
        let root = tempfile::tempdir().unwrap();
        solstone_core_facets::create_facet(root.path(), "work", "Work", "", "", "", None).unwrap();
        let activity_path = root.path().join("facets/work/activities/20260101.jsonl");
        let entity_path = root.path().join("facets/work/entities/20260101.jsonl");
        fs::create_dir_all(activity_path.parent().unwrap()).unwrap();
        fs::create_dir_all(entity_path.parent().unwrap()).unwrap();
        fs::write(&activity_path, "{\"id\":\"activity-1\"}\n").unwrap();
        // The written id is the only matching evidence: this must resolve through
        // the story stage's plain resolver, not its name-evidence sibling.
        fs::write(
            &entity_path,
            "{\"id\":\"owner-id\",\"name\":\"Different\",\"type\":\"person\"}\n",
        )
        .unwrap();
        let prepared = PreparedTalent {
            name: "conversation".into(),
            config: Map::from_iter([
                ("facet".into(), json!("work")),
                (
                    "destination_id".into(),
                    json!(
                        solstone_core_facets::observe_facet_write_identity(root.path(), "work")
                            .unwrap()
                    ),
                ),
                ("day".into(), json!("20260101")),
                ("activity".into(), json!({"id":"activity-1"})),
            ]),
        };
        let value = r#"{
          "body":"A completed conversation.", "topics":["work"], "confidence":0.9,
          "commitments":[{"owner":"owner-id","action":"follow up","counterparty":"nobody","when":"tomorrow","context":"meeting"}],
          "closures":[], "decisions":[], "relations":[]
        }"#;
        let plan = commit(
            parse(value, &prepared, &PrePostState::None).unwrap(),
            &prepared,
            &PrePostState::None,
        )
        .unwrap();
        crate::writers::apply(
            plan,
            &ExecutionContext {
                journal: root.path().into(),
            },
        )
        .unwrap();
        let record = solstone_core_facets::get_activity_record(
            root.path(),
            "work",
            "20260101",
            "activity-1",
        )
        .unwrap()
        .unwrap();
        assert_eq!(record["commitments"][0]["owner_entity_id"], "owner-id");
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn criterion_7_story_validation_precedes_on_disk_mutation() {
        let root = tempfile::tempdir().unwrap();
        solstone_core_facets::create_facet(root.path(), "work", "Work", "", "", "", None).unwrap();
        let activity_path = root.path().join("facets/work/activities/20260101.jsonl");
        let entity_path = root.path().join("facets/work/entities/20260101.jsonl");
        fs::create_dir_all(activity_path.parent().unwrap()).unwrap();
        fs::create_dir_all(entity_path.parent().unwrap()).unwrap();
        fs::write(
            &activity_path,
            "{\"id\":\"activity-1\",\"story\":{\"old\":true},\"commitments\":[\"old\"]}\n",
        )
        .unwrap();
        fs::write(
            &entity_path,
            "{\"id\":\"owner-id\",\"name\":\"Different owner\",\"aka\":[\"Owner\"],\"type\":\"person\"}\n{\"id\":\"counterparty-id\",\"name\":\"Different counterparty\",\"emails\":[\"counterparty@example.com\"],\"type\":\"person\"}\n",
        )
        .unwrap();
        let prepared = PreparedTalent {
            name: "conversation".into(),
            config: Map::from_iter([
                ("facet".into(), json!("work")),
                (
                    "destination_id".into(),
                    json!(
                        solstone_core_facets::observe_facet_write_identity(root.path(), "work")
                            .unwrap()
                    ),
                ),
                ("day".into(), json!("20260101")),
                ("activity".into(), json!({"id":"activity-1"})),
            ]),
        };
        let valid = r#"{
          "body":"A completed conversation.", "topics":["Work"], "confidence":0.9,
          "commitments":[{"owner":"Owner","action":"follow up","counterparty":"counterparty@example.com","when":"tomorrow","context":"meeting"}],
          "closures":[{"owner":"Owner","action":"close","counterparty":"counterparty@example.com","resolution":"done","context":"meeting"}],
          "decisions":[{"owner":"Owner","action":"decide","context":"meeting","counterparty":"counterparty@example.com"}],
          "relations":[{"from":"Owner","to":"counterparty@example.com","kind":"works-with","note":"colleagues"}]
        }"#;
        let plan = commit(
            parse(valid, &prepared, &PrePostState::None).unwrap(),
            &prepared,
            &PrePostState::None,
        )
        .unwrap();
        assert_eq!(
            crate::writers::apply(
                plan,
                &ExecutionContext {
                    journal: root.path().into()
                }
            )
            .unwrap(),
            CommitDisposition::CommittedNoOutput
        );
        let written = solstone_core_facets::get_activity_record(
            root.path(),
            "work",
            "20260101",
            "activity-1",
        )
        .unwrap()
        .unwrap();
        for field in ["story", "commitments", "closures", "decisions", "relations"] {
            assert!(written.contains_key(field));
        }
        assert_eq!(written["story"]["talent"], "conversation");
        assert_eq!(written["commitments"][0]["owner_entity_id"], "owner-id");
        assert_eq!(written["relations"][0]["to_entity_id"], "counterparty-id");
        assert_eq!(written["edits"].as_array().unwrap().len(), 1);
        assert_eq!(written["edits"][0]["actor"], "story");
        assert!(!root.path().join("output.md").exists());

        let unchanged = fs::read(&activity_path).unwrap();
        let client = solstone_core_generate::OneShotClient::at_path(
            crate::test_support::one_shot_stub(root.path(), "not json"),
        );
        let mut sink = Vec::new();
        let cogitate = solstone_core_cogitate_wire::CogitateOneShotClient::at_path(
            root.path().join("unused-cogitate"),
        );
        let outcome = generate_and_write(
            &mut prepared.clone(),
            &ExecutionContext {
                journal: root.path().into(),
            },
            &client,
            &cogitate,
            &mut sink,
            crate::cogitate::EngineKind::Generate,
            Some((&STORY, PrePostState::None)),
        );
        assert!(matches!(
            outcome,
            crate::RuntimeOutcome::Finished {
                disposition: CommitDisposition::RejectedNoMutation,
                ..
            }
        ));
        assert_eq!(fs::read(&activity_path).unwrap(), unchanged);
        assert!(!root.path().join("output.md").exists());
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn a_story_from_a_clipped_request_says_its_input_was_partial() {
        let root = tempfile::tempdir().unwrap();
        solstone_core_facets::create_facet(root.path(), "work", "Work", "", "", "", None).unwrap();
        let activity_path = root.path().join("facets/work/activities/20260101.jsonl");
        fs::create_dir_all(activity_path.parent().unwrap()).unwrap();
        fs::write(&activity_path, "{\"id\":\"activity-1\"}\n").unwrap();
        let prepared = PreparedTalent {
            name: "work".into(),
            config: Map::from_iter([
                ("facet".into(), json!("work")),
                (
                    "destination_id".into(),
                    json!(
                        solstone_core_facets::observe_facet_write_identity(root.path(), "work")
                            .unwrap()
                    ),
                ),
                ("day".into(), json!("20260101")),
                ("activity".into(), json!({"id":"activity-1"})),
            ]),
        };
        // The model cannot claim or hide partial input; only the request can.
        let output = json!({"body":"You worked through a long terminal session.","topics":["work"],
            "confidence":0.7,"commitments":[],"closures":[],"decisions":[],"relations":[],
            "partial_input":{"dropped_entries":0}})
        .to_string();
        let cogitate = solstone_core_cogitate_wire::CogitateOneShotClient::at_path(
            root.path().join("unused-cogitate"),
        );
        let context = ExecutionContext {
            journal: root.path().into(),
        };
        let run = |input_budget: Value| {
            let client = solstone_core_generate::OneShotClient::at_path(
                crate::test_support::one_shot_stub_with_input_budget(
                    root.path(),
                    &output,
                    input_budget,
                ),
            );
            let outcome = generate_and_write(
                &mut prepared.clone(),
                &context,
                &client,
                &cogitate,
                &mut Vec::new(),
                crate::cogitate::EngineKind::Generate,
                Some((&STORY, PrePostState::None)),
            );
            assert!(matches!(
                outcome,
                crate::RuntimeOutcome::Finished {
                    disposition: CommitDisposition::CommittedNoOutput,
                    ..
                }
            ));
            solstone_core_facets::get_activity_record(root.path(), "work", "20260101", "activity-1")
                .unwrap()
                .unwrap()
        };

        let clipped = run(
            json!({"clipped":true,"dropped_chars":90_000,"dropped_entries":3,"budget_tokens":1_000}),
        );
        assert_eq!(
            clipped["story"]["partial_input"],
            json!({"dropped_entries":3,"dropped_chars":90_000})
        );

        let whole = run(Value::Null);
        assert_eq!(whole["story"]["talent"], "work");
        assert!(whole["story"].get("partial_input").is_none());
    }
}
