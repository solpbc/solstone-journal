// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Story source-word body regression cases, run through the native caller.
//!
//! Each case writes one segment of invented source files, runs the shipped
//! Story talent through `execute_request` with one scripted completion, and
//! checks what was stored and what was returned.

use std::fs;
use std::path::Path;
use std::sync::Mutex;

use serde_json::{Map, Value, json};
use solstone_core_generate::{
    ClientError, GenerateExecutor, GenerateRequest, GenerateResponse, GeneratedResponse,
};

use crate::contract::CommitDisposition;
use crate::prepare::RuntimePaths;
use crate::{ExecutionContext, RuntimeOutcome, execute_request};

const CASES: &str = include_str!("story_source_conformance.json");
pub(crate) const DAY: &str = "20260101";
pub(crate) const SEGMENT: &str = "090000_300";
pub(crate) const FACET: &str = "work";
pub(crate) const ACTIVITY: &str = "activity_090000_300";

pub(crate) struct Scripted {
    pub(crate) text: String,
    pub(crate) input_budget: Option<Value>,
    pub(crate) requests: Mutex<Vec<GenerateRequest>>,
}

impl GenerateExecutor for Scripted {
    fn execute(&self, request: &GenerateRequest) -> Result<GenerateResponse, ClientError> {
        self.requests.lock().unwrap().push(request.clone());
        Ok(GenerateResponse::Generated(Box::new(GeneratedResponse {
            id: None,
            text: self.text.clone(),
            model: "invented-model".to_owned(),
            usage: json!({"input_tokens": 900, "output_tokens": 120, "total_tokens": 1020}),
            finish_reason: "stop".to_owned(),
            thinking: None,
            schema_validation: Some(json!({"valid": true, "errors": []})),
            input_budget: self.input_budget.clone(),
            request_budget: None,
            inference: None,
            hints_applied: Vec::new(),
        })))
    }
}

/// A journal holding one activity over one segment of the case's files, and
/// the shipped talent it names; `notes` is a custom copy of the Work Story.
pub(crate) fn journal(root: &Path, case: &Value) -> (RuntimePaths, ExecutionContext) {
    let journal = root.join("journal");
    let segment = journal
        .join("chronicle")
        .join(DAY)
        .join(case["stream"].as_str().unwrap())
        .join(SEGMENT);
    fs::create_dir_all(&segment).unwrap();
    for (name, rows) in case["files"].as_object().unwrap() {
        let lines = rows
            .as_array()
            .unwrap()
            .iter()
            .map(|row| match row {
                Value::String(line) => format!("{line}\n"),
                row => format!("{row}\n"),
            })
            .collect::<String>();
        fs::write(segment.join(name), lines).unwrap();
    }
    fs::create_dir_all(journal.join("config")).unwrap();
    fs::write(
        journal.join("config/journal.json"),
        r#"{"identity":{"name":"Rowan Vale"},"providers":{"active":{"provider":"test","model":"test-model"}}}"#,
    )
    .unwrap();
    fs::create_dir_all(journal.join(format!("facets/{FACET}/activities"))).unwrap();
    fs::write(
        journal.join(format!("facets/{FACET}/facet.json")),
        r#"{"name":"work","description":"Work"}"#,
    )
    .unwrap();
    fs::write(
        journal.join(format!("facets/{FACET}/activities/{DAY}.jsonl")),
        format!("{}\n", activity(case)),
    )
    .unwrap();
    let payload = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../payload/solstone");
    let talent_root = root.join("talent");
    fs::create_dir_all(&talent_root).unwrap();
    let talent = case["talent"].as_str().unwrap();
    let (source, schema) = match talent {
        "conversation" => ("conversation", "conversation.schema.json"),
        _ => ("work", "story.schema.json"),
    };
    fs::copy(
        payload.join(format!("talent/{source}.md")),
        talent_root.join(format!("{talent}.md")),
    )
    .unwrap();
    fs::copy(
        payload.join("talent").join(schema),
        talent_root.join(schema),
    )
    .unwrap();
    if let Some(body) = case.get("definition_source_body") {
        let path = talent_root.join(format!("{talent}.md"));
        let mut definition = solstone_core_talent_config::read_frontmatter(&path).unwrap();
        definition
            .metadata
            .insert(crate::story::SOURCE_BODY_KEY.to_owned(), body.clone());
        fs::write(
            path,
            format!(
                "{}\n{}",
                serde_json::to_string_pretty(&definition.metadata).unwrap(),
                definition.body
            ),
        )
        .unwrap();
    }
    fs::create_dir_all(root.join("apps")).unwrap();
    (
        RuntimePaths {
            talent_root,
            apps_root: root.join("apps"),
            templates_dir: payload.join("think/templates"),
        },
        ExecutionContext { journal },
    )
}

pub(crate) fn activity(case: &Value) -> Value {
    json!({
        "id": ACTIVITY,
        "activity": case["activity_kind"],
        "segments": [SEGMENT],
        "stream": case["stream"],
        "description": "An invented activity",
    })
}

struct Run {
    outcome: RuntimeOutcome,
    record: Map<String, Value>,
    requests: Vec<GenerateRequest>,
    response: String,
}

fn run(case: &Value) -> Run {
    let root = tempfile::tempdir().unwrap();
    let (paths, context) = journal(root.path(), case);
    let response = case["response"].to_string();
    let executor = Scripted {
        text: response.clone(),
        input_budget: case.get("input_budget").cloned(),
        requests: Mutex::new(Vec::new()),
    };
    let mut request = json!({
        "name": case["talent"],
        "day": DAY,
        "facet": FACET,
        "activity": activity(case),
        "span": [SEGMENT],
    });
    if let Some(extra) = case.get("request").and_then(Value::as_object) {
        request.as_object_mut().unwrap().extend(extra.clone());
    }
    if let Some(body) = case.get("definition_source_body") {
        let prepared = crate::prepare::prepare(
            request.as_object().unwrap().clone(),
            &paths,
            &context,
            crate::prepare::PrepareMode::Execute,
        )
        .unwrap();
        assert_ne!(
            prepared.config.get(crate::story::SOURCE_BODY_KEY),
            Some(body),
            "{}: a definition cannot supply the source-word body",
            case["name"]
        );
    }
    let outcome = execute_request(
        request.as_object().unwrap().clone(),
        &paths,
        &context,
        &executor,
        &mut Vec::new(),
        &solstone_core_generate::Cancellation::never(),
    );
    let record = solstone_core_facets::get_activity_record(&context.journal, FACET, DAY, ACTIVITY)
        .unwrap()
        .unwrap();
    Run {
        outcome,
        record,
        requests: executor.requests.into_inner().unwrap(),
        response,
    }
}

pub(crate) fn cases() -> Vec<Value> {
    serde_json::from_str::<Value>(CASES).unwrap()["cases"]
        .as_array()
        .unwrap()
        .clone()
}

#[test]
fn every_story_source_case_stores_and_returns_one_story() {
    for case in cases() {
        let name = case["name"].as_str().unwrap();
        let run = run(&case);
        let expect = &case["expect"];
        let model = &case["response"];
        // The ordinary request, once: the body adds no inference.
        assert_eq!(
            run.requests.len(),
            1,
            "{name}: one generate call: {:?}",
            run.outcome
        );
        let RuntimeOutcome::Finished {
            output,
            disposition,
            ..
        } = run.outcome
        else {
            panic!("{name}: the story run did not finish: {:?}", run.outcome);
        };
        if expect["rejected"] == true {
            assert_eq!(disposition, CommitDisposition::RejectedNoMutation, "{name}");
            assert_eq!(
                output, run.response,
                "{name}: a rejected run returns the response"
            );
            assert!(
                !run.record.contains_key("story"),
                "{name}: nothing is stored"
            );
            continue;
        }
        assert_eq!(disposition, CommitDisposition::CommittedNoOutput, "{name}");
        let story = &run.record["story"];
        let quoted = expect.get("body").and_then(Value::as_str);
        let body = quoted.unwrap_or_else(|| model["body"].as_str().unwrap().trim());
        assert_eq!(story["body"], body, "{name}: stored body");
        // Topics, confidence and relations are always the model's.
        let mut topics = Vec::<String>::new();
        for topic in model["topics"].as_array().unwrap() {
            let topic = topic.as_str().unwrap().trim().to_lowercase();
            if !topic.is_empty() && !topics.contains(&topic) {
                topics.push(topic);
            }
        }
        assert_eq!(story["topics"], json!(topics), "{name}: topics");
        let confidence = model["confidence"].as_f64().unwrap().clamp(0.0, 1.0);
        assert_eq!(story["confidence"], json!(confidence), "{name}: confidence");
        let relations = run.record["relations"].as_array().unwrap();
        let expected = model["relations"].as_array().unwrap();
        assert_eq!(relations.len(), expected.len(), "{name}: relations kept");
        for (stored, sent) in relations.iter().zip(expected) {
            for field in ["from", "to", "kind", "note", "quote"] {
                assert_eq!(stored[field], sent[field], "{name}: relation {field}");
            }
        }
        assert_eq!(
            story.get("partial_input").is_some(),
            expect["partial"] == true,
            "{name}: partial input"
        );
        if quoted.is_some() {
            // What the run returns is the Story it stored.
            let returned: Value = serde_json::from_str(&output).unwrap();
            assert_eq!(returned["body"], story["body"], "{name}: returned body");
            assert_eq!(
                returned["topics"], story["topics"],
                "{name}: returned topics"
            );
            assert_eq!(
                returned["confidence"], story["confidence"],
                "{name}: returned confidence"
            );
            assert_eq!(returned["relations"], model["relations"], "{name}");
        } else if expect["partial"] == true {
            let mut returned: Value = serde_json::from_str(&output).unwrap();
            let partial = returned
                .as_object_mut()
                .unwrap()
                .remove("partial_input")
                .unwrap();
            assert_eq!(partial, story["partial_input"]);
            assert_eq!(
                &returned, model,
                "{name}: only runtime-owned limitation metadata changes"
            );
        } else {
            assert_eq!(
                output, run.response,
                "{name}: the response is returned as is"
            );
        }
    }
}

fn request_texts(request: &GenerateRequest) -> Vec<&str> {
    request
        .contents
        .iter()
        .filter_map(|part| match part {
            solstone_core_generate::ContentPart::Text { text } => Some(text.as_str()),
            solstone_core_generate::ContentPart::Image { .. } => None,
        })
        .collect()
}

fn minimal_source_word_task() -> (
    Map<String, Value>,
    Vec<Option<solstone_core_transcripts::EntrySource>>,
) {
    let record = json!({"start":"00:00:01","speaker":1,"text":"The request remains pending."})
        .as_object()
        .unwrap()
        .clone();
    let chunk = "[00:00:01] Speaker 1: The request remains pending.".to_owned();
    let source = solstone_core_transcripts::EntrySource {
        path: "/invented/chronicle/20260101/desktop/090000_300/audio.jsonl".into(),
        sha256: "invented-source-snapshot".into(),
        rendered_range: 0..chunk.len(),
        stream: Some("desktop".into()),
        file: "audio.jsonl".into(),
        records: vec![record.clone()],
        unparsed_lines: 0,
        chunks: vec![(chunk.clone(), record)],
        chunk_offsets: vec![0],
        voice_named: Default::default(),
    };
    let config =
        json!({"name":"work","transcript":chunk,"user_instruction":"Summarize this source.",
        "hook":{"post":"story"},"activity":{"id":"invented-activity"}})
        .as_object()
        .unwrap()
        .clone();
    (config, vec![Some(source)])
}

#[test]
fn protected_activity_context_disqualifies_the_minimal_source_word_task() {
    let (mut config, sources) = minimal_source_word_task();
    // Activity identity alone does not revoke the existing measured capability.
    let source_body = crate::story::source_body("work", &config, &sources).unwrap();
    assert!(source_body.contains("The request remains pending."));
    config.insert(
        "prompt".into(),
        json!("Additional protected activity context."),
    );
    assert!(crate::story::source_body("work", &config, &sources).is_none());
}

#[test]
fn processed_source_selection_never_admits_a_raw_source_word_body() {
    use crate::contract::{ParsedOutput, PrePostState};
    let (mut config, sources) = minimal_source_word_task();
    let source_body = crate::story::source_body("work", &config, &sources).unwrap();
    config.insert(crate::story::SOURCE_BODY_KEY.into(), json!(source_body));
    let mut prepared = crate::PreparedTalent::new("work".into(), config);
    let response =
        json!({"body":"The ordinary model account.","topics":[],"confidence":0.5,"relations":[]})
            .to_string();
    crate::record_input_budget(&mut prepared, Some(&json!({"clipped":false})));
    let ParsedOutput::Json(raw) =
        crate::story::parse(&response, &prepared, &PrePostState::None).unwrap()
    else {
        panic!("JSON Story expected")
    };
    assert_eq!(raw["body"], source_body);

    crate::record_input_budget(
        &mut prepared,
        Some(&json!({"clipped":false,"used_overflow_text":true})),
    );
    assert_eq!(prepared.config[crate::story::INPUT_WHOLE_KEY], true);
    assert!(prepared.story_input_used);
    let ParsedOutput::Json(processed) =
        crate::story::parse(&response, &prepared, &PrePostState::None).unwrap()
    else {
        panic!("JSON Story expected")
    };
    assert_eq!(processed["body"], "The ordinary model account.");
    assert_eq!(
        crate::story::output(&response, &prepared, &PrePostState::None).unwrap(),
        response
    );
}

#[test]
fn a_request_added_instruction_reaches_the_model_and_keeps_its_body() {
    let instruction = "Write the body as a short account in your own words, without quotations.";
    for name in [
        "imported_messages_keep_their_authors_and_message_dates",
        "an_unidentified_utterance_keeps_negation_and_latest_status",
    ] {
        let mut case = cases()
            .into_iter()
            .find(|case| case["name"] == name)
            .unwrap();
        // The empty prompt every scheduled Story request carries.
        case["request"] = json!({"prompt": ""});
        let stock = run(&case);
        assert!(matches!(stock.outcome, RuntimeOutcome::Finished { .. }));
        assert_eq!(request_texts(&stock.requests[0]).len(), 2, "{name}");
        assert_eq!(
            stock.record["story"]["body"], case["expect"]["body"],
            "{name}: the stock request keeps its source words"
        );

        case["request"] = json!({"prompt": instruction});
        let added = run(&case);
        let RuntimeOutcome::Finished {
            output,
            disposition,
            ..
        } = added.outcome
        else {
            panic!("{name}: the story run did not finish: {:?}", added.outcome);
        };
        assert_eq!(added.requests.len(), 1, "{name}");
        let texts = request_texts(&added.requests[0]);
        assert_eq!(texts.len(), 3, "{name}");
        assert_eq!(texts[2], instruction, "{name}: the model read the request");
        assert_eq!(disposition, CommitDisposition::CommittedNoOutput, "{name}");
        assert_eq!(
            added.record["story"]["body"],
            case["response"]["body"].as_str().unwrap().trim(),
            "{name}: the stored body answers the request"
        );
        assert_eq!(
            output, added.response,
            "{name}: the response is returned as is"
        );
    }
}

#[test]
fn a_definition_cannot_supply_the_story_source_body() {
    let all = cases();
    let audio = all
        .iter()
        .find(|case| case["name"] == "an_unidentified_utterance_keeps_negation_and_latest_status")
        .unwrap();
    let screen = all
        .iter()
        .find(|case| case["name"] == "a_terminal_pane_keeps_the_model_body")
        .unwrap();
    for talent in ["work", "conversation"] {
        for mode in [
            "screen", "prompt", "schema", "audio", "partial", "payload", "disabled",
        ] {
            let mut case = if mode == "screen" || mode == "payload" {
                screen.clone()
            } else {
                audio.clone()
            };
            case["name"] = json!(format!("{talent}-{mode}-definition-source-body"));
            case["talent"] = json!(talent);
            case["definition_source_body"] = json!("Definition-supplied words are not source.");
            match mode {
                "prompt" => case["request"] = json!({"prompt":"Write a distinct account."}),
                "schema" => case["request"] = json!({"json_schema":{"type":"object"}}),
                "partial" => {
                    case["input_budget"] =
                        json!({"clipped":true,"dropped_entries":1,"dropped_chars":18});
                }
                "payload" => {
                    case["request"] = json!({
                        "_source_body":"Request-supplied words are not source either.",
                        "_input_whole":true
                    });
                }
                "disabled" => case["request"] = json!({"disabled":true}),
                _ => {}
            }
            let run = run(&case);
            if mode == "disabled" {
                assert!(
                    matches!(
                        run.outcome,
                        RuntimeOutcome::PrepareSkipped { ref reason, .. } if reason == "disabled"
                    ),
                    "{}: {:?}",
                    case["name"],
                    run.outcome
                );
                assert!(run.requests.is_empty());
                assert!(!run.record.contains_key("story"));
                continue;
            }
            let RuntimeOutcome::Finished {
                output,
                disposition,
                ..
            } = run.outcome
            else {
                panic!("{}: {:?}", case["name"], run.outcome);
            };
            assert_eq!(run.requests.len(), 1, "{}", case["name"]);
            assert_eq!(disposition, CommitDisposition::CommittedNoOutput);
            let expected = if mode == "audio" {
                &case["expect"]["body"]
            } else {
                &case["response"]["body"]
            };
            assert_eq!(run.record["story"]["body"], *expected, "{}", case["name"]);
            let returned: Value = serde_json::from_str(&output).unwrap();
            assert_eq!(returned["body"], *expected, "{}", case["name"]);
            assert_eq!(
                run.record["story"].get("partial_input").is_some(),
                mode == "partial",
                "{}",
                case["name"]
            );
            assert_eq!(
                returned.get("partial_input"),
                run.record["story"].get("partial_input"),
                "{}",
                case["name"]
            );
        }
    }
}

/// Multi-message Unicode source with a correction at its end. Every message
/// fits the individual quote limit; only the assembled reader budget varies.
fn source_at_reader_boundary(chars: usize) -> (Value, String) {
    let mut case = cases().remove(0);
    let correction = "I cancelled the Cobalt dispatch. Zero packs were sent.";
    let mut texts = vec!["é".repeat(340); 4];
    texts.push(correction.to_owned());
    let body = |texts: &[String]| {
        texts
            .iter()
            .enumerate()
            .map(|(i, text)| {
                let who = if i % 2 == 0 { "You" } else { "Your agent" };
                format!("{who} said in a message dated 2026-01-01T09:00:0{i}.000Z: \"{text}\"")
            })
            .collect::<Vec<_>>()
            .join(" ")
    };
    let padding = chars.checked_sub(body(&texts).chars().count()).unwrap();
    texts[4] = format!("{}{correction}", "é".repeat(padding));
    assert!(texts.iter().all(|text| text.chars().count() <= 400));
    let expected = body(&texts);
    assert_eq!(expected.chars().count(), chars);
    let mut rows = vec![json!({"imported":{"id":"invented-boundary"}})];
    rows.extend(texts.iter().enumerate().map(|(i, text)| {
        json!({"start":format!("00:00:0{i}"),"speaker":if i % 2 == 0 {"You"} else {"Assistant"},"text":text,"source":"import","source_ms":1767258000000_i64+i as i64*1000})
    }));
    case["files"] = json!({"conversation_transcript.jsonl":rows});
    case["response"] = json!({"body":correction,"topics":[],"confidence":0.8,"relations":[]});
    (case, expected)
}

#[test]
fn source_words_fit_the_reader_budget_whole_or_keep_the_ordinary_body() {
    for chars in [
        crate::story::MAX_STORY_BODY_CHARS,
        crate::story::MAX_STORY_BODY_CHARS + 1,
    ] {
        let (case, quoted) = source_at_reader_boundary(chars);
        let result = run(&case);
        let RuntimeOutcome::Finished { output, .. } = result.outcome else {
            panic!("Story did not finish");
        };
        let expected = if chars == crate::story::MAX_STORY_BODY_CHARS {
            quoted
        } else {
            assert_eq!(
                output, result.response,
                "fallback returns the completion unchanged"
            );
            case["response"]["body"].as_str().unwrap().to_owned()
        };
        assert_eq!(result.record["story"]["body"], expected);
    }
}

#[test]
fn a_long_source_correction_survives_into_activity_records() {
    let (case, quoted) = source_at_reader_boundary(2039);
    let root = tempfile::tempdir().unwrap();
    let (paths, context) = journal(root.path(), &case);
    let story = Scripted {
        text: case["response"].to_string(),
        input_budget: None,
        requests: Mutex::new(Vec::new()),
    };
    let request = json!({"name":"conversation","day":DAY,"facet":FACET,"activity":activity(&case),"span":[SEGMENT]});
    let outcome = execute_request(
        request.as_object().unwrap().clone(),
        &paths,
        &context,
        &story,
        &mut Vec::new(),
        &solstone_core_generate::Cancellation::never(),
    );
    assert!(matches!(outcome, RuntimeOutcome::Finished { .. }));
    let records =
        solstone_core_facets::load_activity_records(&context.journal, FACET, DAY, false).unwrap();
    assert_eq!(records.len(), 1);
    let body = records[0]["story"]["body"].as_str().unwrap();
    assert_eq!(body, case["response"]["body"].as_str().unwrap());
    assert!(
        !body.contains(&quoted),
        "the over-budget quotation keeps the ordinary summary"
    );
}
