// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;
use solstone_core_generate::{GenerateResponse, GeneratedResponse};
use std::sync::Mutex;

fn setup() -> (PreparedTalent, Value) {
    let source = "A complete source passage about a pending launch question.";
    let current = "Composed this input: “Is the launch ready?”".to_owned();
    let bound = Bound {
        kind: "editable_input_box_visible",
        current_sha: digest(&current),
        current: current.clone(),
        screen_sha: digest(source),
        timestamp: json!(0),
        scope: json!(["s", "w", "p"]),
        screen: source.into(),
        start: 0,
    };
    let rows = vec![
        Passage {
            id: "T0".into(),
            text: source.into(),
            scope: bound.scope.clone(),
            timestamp: json!(0),
            screen_sha: digest(source),
            raw_sha: digest(source),
            start: 0,
            end: source.len(),
        },
        Passage {
            id: "T1".into(),
            text: "A later complete source passage describes the revised plan.".into(),
            scope: bound.scope.clone(),
            timestamp: json!(1),
            screen_sha: digest("later"),
            raw_sha: digest("later"),
            start: 0,
            end: 5,
        },
    ];
    let schema_native = json!({"type":"object","additionalProperties":false,"required":["activity_summary","facets","quotes","entities"],"properties":{"activity_summary":{"type":"string"},"facets":{"type":"array","items":{"type":"object","properties":{"activity":{"type":"string"}}}},"quotes":{"type":"array"},"entities":{"type":"array","items":{"type":"object","properties":{"source":{"type":"string","enum":["voice","speaker_label","transcript","screen","other"]}}}}}});
    let mut prepared=PreparedTalent::new("sense".into(),json!({"max_output_tokens":100,"transcript":"native source","user_instruction":"native task","json_schema":schema_native}).as_object().unwrap().clone());
    let original = crate::native_generate_request(&prepared).unwrap();
    let mut request = original.clone();
    request.json_schema = Some(schema(&schema_native, 0, &bound, &rows).unwrap());
    prepared.terminal_sense = Some(Candidate {
        request,
        original,
        bound,
        passages: rows,
    });
    let value = json!({"model_plan":{"anchor_input_id":0,"current_act":"question","earlier_work_state":"not_established","pending_decision":true,"requested_follow_through":null},"sense":{"activity_summary":{"background":["T0"],"current":current},"facets":[],"quotes":[],"entities":[]}});
    (prepared, value)
}
fn generated(text: &str, valid: bool, i: u64, o: u64) -> GenerateResponse {
    GenerateResponse::Generated(Box::new(GeneratedResponse {
        id: None,
        text: text.into(),
        model: "model".into(),
        usage: json!({"input_tokens":i,"output_tokens":o,"total_tokens":i+o}),
        finish_reason: "stop".into(),
        thinking: None,
        schema_validation: Some(json!({"valid":valid})),
        input_budget: None,
        request_budget: None,
        inference: None,
        hints_applied: vec![],
    }))
}
fn run(
    prepared: &mut PreparedTalent,
    responses: Vec<GenerateResponse>,
) -> (crate::RuntimeOutcome, Vec<GenerateRequest>, Vec<Value>) {
    let pending = Mutex::new(std::collections::VecDeque::from(responses));
    let calls = Mutex::new(Vec::new());
    let executor = |r: &GenerateRequest| {
        calls.lock().unwrap().push(r.clone());
        Ok(pending
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected call"))
    };
    let mut events = Vec::new();
    let result = crate::generate_and_write(
        prepared,
        &ExecutionContext {
            journal: "unused".into(),
        },
        &executor,
        &mut events,
        None,
        &solstone_core_generate::Cancellation::never(),
    );
    let events = String::from_utf8(events)
        .unwrap()
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    (result, calls.into_inner().unwrap(), events)
}
#[test]
fn source_resolution_preserves_two_selections_and_rejects_unissued_or_altered_fields() {
    let (prepared, value) = setup();
    let candidate = prepared.terminal_sense.unwrap();
    for keys in [json!(["T0"]), json!(["T1", "T0"])] {
        let mut output = value.clone();
        output["sense"]["activity_summary"]["background"] = keys.clone();
        let text = candidate.render(&output.to_string()).unwrap();
        let result: Value = serde_json::from_str(&text).unwrap();
        let text = result["activity_summary"].as_str().unwrap();
        assert!(text.contains(&candidate.passages[0].text));
        assert!(result.get("model_plan").is_none());
        assert!(result["activity_summary"].is_string());
        if keys.as_array().unwrap().len() == 2 {
            assert!(
                text.find(&candidate.passages[0].text) < text.find(&candidate.passages[1].text)
            );
        }
    }
    for keys in [
        json!([]),
        json!(["missing"]),
        json!(["T0", "T0"]),
        json!(["T0", "T1", "T0"]),
        json!([0]),
        json!(true),
    ] {
        let mut output = value.clone();
        output["sense"]["activity_summary"]["background"] = keys.clone();
        assert!(candidate.render(&output.to_string()).is_err());
    }
    let mut output = value;
    output["sense"]["activity_summary"]["current"] = json!("approved the launch");
    assert!(candidate.render(&output.to_string()).is_err());
}
#[test]
fn canonical_candidate_uses_existing_writer_without_private_wrapper() {
    let (mut prepared, value) = setup();
    prepared
        .config
        .insert(crate::INPUT_BUDGET_KEY.into(), json!({"clipped":true}));
    let (result, calls, _) = run(
        &mut prepared,
        vec![generated(&value.to_string(), true, 100, 10)],
    );
    let crate::RuntimeOutcome::Finished { output, .. } = result else {
        panic!("candidate failed")
    };
    let output: Value = serde_json::from_str(&output).unwrap();
    assert!(output.get("model_plan").is_none());
    assert!(output.get("input_budget").is_none());
    assert!(
        output["activity_summary"]
            .as_str()
            .unwrap()
            .contains("Is the launch ready?")
    );
    assert_eq!(calls.len(), 1);
}
#[test]
fn retries_and_native_fallback_retain_all_usage_and_original_input() {
    let (mut prepared, value) = setup();
    let original = crate::native_generate_request(&prepared).unwrap();
    let mut invalid = value.clone();
    invalid["sense"]["activity_summary"]["current"] = json!("altered");
    let native = json!({"activity_summary":"Native result","facets":[],"quotes":[],"entities":[]})
        .to_string();
    let (result, calls, events) = run(
        &mut prepared,
        vec![
            generated(&value.to_string(), false, 100, 10),
            generated(&invalid.to_string(), true, 200, 20),
            generated(&native, false, 300, 30),
            generated(&native, true, 400, 40),
        ],
    );
    let crate::RuntimeOutcome::Finished {
        output,
        usage: Some(usage),
        ..
    } = result
    else {
        panic!("fallback failed")
    };
    assert_eq!(output, native);
    assert_eq!(usage["input_tokens"], 1000);
    assert_eq!(usage["output_tokens"], 100);
    assert_eq!(usage["total_tokens"], 1100);
    assert_eq!(calls.len(), 4);
    for call in &calls[2..] {
        assert_eq!(call.contents, original.contents);
        assert_eq!(call.json_schema, original.json_schema);
    }
    assert_eq!(
        events
            .iter()
            .filter(|e| e["event"] == "generate_attempt")
            .count(),
        4
    );
}
#[test]
fn cancellation_and_security_or_transport_refusals_do_not_fall_back() {
    for code in ["attestation_failed", "http_401", "connection_error"] {
        let (mut prepared, _) = setup();
        let response = solstone_core_generate_wire::refusal_for(
            &solstone_core_generate_wire::LaneOutcome::EndpointFailure(
                solstone_core_generate_wire::EndpointFailure {
                    reason_code: Some(code.into()),
                    detail: None,
                },
            ),
            "local",
            None,
        );
        let (outcome, calls, _) = run(&mut prepared, vec![GenerateResponse::Refused(response)]);
        assert!(matches!(
            outcome,
            crate::RuntimeOutcome::GenerateRefused { .. }
        ));
        assert_eq!(calls.len(), 1);
    }
    let (mut prepared, _) = setup();
    let (outcome, calls, _) = run(&mut prepared, vec![GenerateResponse::Cancelled]);
    assert!(matches!(outcome, crate::RuntimeOutcome::Cancelled));
    assert_eq!(calls.len(), 1);
}

#[test]
fn a_trimmed_timeline_is_an_ordinary_partial_result() {
    let (mut prepared, value) = setup();
    let budget =
        json!({"clipped":true,"dropped_chars":120,"dropped_entries":2,"budget_tokens":900});
    let GenerateResponse::Generated(mut response) = generated(&value.to_string(), true, 100, 10)
    else {
        unreachable!()
    };
    response.input_budget = Some(budget.clone());
    let (result, calls, events) = run(&mut prepared, vec![GenerateResponse::Generated(response)]);
    let crate::RuntimeOutcome::Finished { output, .. } = result else {
        panic!("trimmed candidate failed")
    };
    assert!(output.contains("Is the launch ready?"));
    assert_eq!(calls.len(), 1);
    assert_eq!(prepared.config.get(crate::INPUT_BUDGET_KEY), Some(&budget));
    assert!(prepared.config.get(crate::story::INPUT_WHOLE_KEY).is_none());
    assert!(!events.iter().any(|e| e["branch"] == "native_fallback"));
}

#[test]
fn context_window_refusals_take_one_unchanged_native_fallback() {
    for code in [
        "context_budget_exceeded",
        "context_fitted_overflow",
        "context_preserved_overflow",
        "context_window_exceeded",
    ] {
        let (mut prepared, _) = setup();
        let original = crate::native_generate_request(&prepared).unwrap();
        let response = solstone_core_generate_wire::refusal_for(
            &solstone_core_generate_wire::LaneOutcome::EndpointFailure(
                solstone_core_generate_wire::EndpointFailure {
                    reason_code: Some(code.into()),
                    detail: None,
                },
            ),
            "local",
            None,
        );
        let native =
            json!({"activity_summary":"Native fallback","facets":[],"quotes":[],"entities":[]})
                .to_string();
        let (outcome, calls, _) = run(
            &mut prepared,
            vec![
                GenerateResponse::Refused(response),
                generated(&native, true, 100, 10),
            ],
        );
        assert!(matches!(outcome, crate::RuntimeOutcome::Finished { .. }));
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[1].contents, original.contents);
        assert_eq!(calls[1].json_schema, original.json_schema);
    }
}

#[test]
fn completed_attempt_usage_survives_cancellation_before_publication() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let (mut prepared, value) = setup();
    let cancelled = AtomicBool::new(false);
    let check = || cancelled.load(Ordering::SeqCst);
    let executor = |_: &GenerateRequest| {
        cancelled.store(true, Ordering::SeqCst);
        Ok(generated(&value.to_string(), true, 100, 10))
    };
    let mut events = Vec::new();
    let outcome = crate::generate_and_write(
        &mut prepared,
        &ExecutionContext {
            journal: "unused".into(),
        },
        &executor,
        &mut events,
        None,
        &solstone_core_generate::Cancellation::from_fn(&check),
    );
    assert!(matches!(outcome, crate::RuntimeOutcome::Cancelled));
    let events = String::from_utf8(events).unwrap();
    let usage = events
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .find(|v| v["event"] == "terminal_sense_usage")
        .unwrap();
    assert_eq!(usage["usage"]["input_tokens"], 100);
    assert_eq!(usage["usage"]["output_tokens"], 10);
}
