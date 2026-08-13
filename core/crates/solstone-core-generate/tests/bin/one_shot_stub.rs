// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::env;
use std::fs;
use std::io::{self, Read};

use serde_json::json;
use solstone_core_generate::{
    GenerateResponse, GeneratedResponse, ProtocolError, ReasonCode, ReasonCodeValue, RefusalReason,
    RefusedResponse, UnknownReasonCode, decode_one_shot_request, encode_one_shot_response,
    encode_protocol_error,
};

const MODE_ENV: &str = "SOLSTONE_GENERATE_ONE_SHOT_STUB_MODE";
const OVERRIDES_PATH_ENV: &str = "SOLSTONE_GENERATE_ONE_SHOT_STUB_OVERRIDES_PATH";
const API_KEY_OVERRIDE_ENV: &str = "SOLSTONE_GENERATE_API_KEY_OVERRIDE";
const MODEL_OVERRIDE_ENV: &str = "SOLSTONE_GENERATE_MODEL_OVERRIDE";
const PROVIDER_OVERRIDE_ENV: &str = "SOLSTONE_GENERATE_PROVIDER_OVERRIDE";

fn main() {
    let arguments = env::args().collect::<Vec<_>>();
    assert_eq!(arguments.len(), 2, "one-shot stub accepts only --one-shot");
    assert_eq!(
        arguments[1], "--one-shot",
        "one-shot stub requires --one-shot"
    );

    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .expect("one-shot stub stdin is readable");
    let request = decode_one_shot_request(&input).expect("one-shot request is valid");
    record_overrides();

    match env::var(MODE_ENV).as_deref().unwrap_or("generated") {
        "generated" => respond(GenerateResponse::Generated(Box::new(GeneratedResponse {
            id: request.id,
            text: "one-shot-stub".to_owned(),
            model: "stub-model".to_owned(),
            usage: json!({}),
            finish_reason: "stop".to_owned(),
            thinking: None,
            schema_validation: None,
            input_budget: None,
            request_budget: None,
            inference: None,
            hints_applied: Vec::new(),
        }))),
        "known_refusal" => respond(refusal(
            request.id,
            ReasonCodeValue::Known(ReasonCode::new("model_not_found").expect("known reason code")),
        )),
        "unknown_refusal" => respond(refusal(
            request.id,
            ReasonCodeValue::Unknown(UnknownReasonCode {
                received: "future_reason_code".to_owned(),
                canonical: ReasonCode::new("unknown").expect("unknown reason code"),
            }),
        )),
        "hard_failure" => {
            eprintln!(
                "{}",
                encode_protocol_error(&ProtocolError {
                    id: request.id,
                    reason: "stub_failure".to_owned(),
                    detail: "one-shot stub hard failure".to_owned(),
                })
                .expect("protocol error serializes")
            );
            std::process::exit(70);
        }
        mode => panic!("unsupported one-shot stub mode {mode}"),
    }
}

fn record_overrides() {
    let Ok(path) = env::var(OVERRIDES_PATH_ENV) else {
        return;
    };
    fs::write(
        path,
        serde_json::to_vec(&json!({
            "api_key": env::var(API_KEY_OVERRIDE_ENV).ok(),
            "provider": env::var(PROVIDER_OVERRIDE_ENV).ok(),
            "model": env::var(MODEL_OVERRIDE_ENV).ok(),
        }))
        .expect("override record serializes"),
    )
    .expect("override record writes");
}

fn refusal(id: Option<String>, reason_code: ReasonCodeValue) -> GenerateResponse {
    GenerateResponse::Refused(RefusedResponse {
        id,
        reason: RefusalReason::ProviderResponseInvalid,
        reason_code: Some(reason_code),
        retryable: false,
        blocking: true,
        reset_at_ms: None,
        provider: Some("stub".to_owned()),
        detail: "one-shot stub refusal".to_owned(),
    })
}

fn respond(response: GenerateResponse) {
    println!(
        "{}",
        encode_one_shot_response(&response).expect("one-shot response serializes")
    );
}
