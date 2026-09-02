// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::env;
use std::fs;
use std::io::{self, Read, Write};

use serde_json::json;
use solstone_core_generate::{
    GenerateResponse, GeneratedResponse, ProtocolError, ReasonCode, ReasonCodeValue, RefusalReason,
    RefusedResponse, STDERR_LIMIT, STDOUT_LIMIT, UnknownReasonCode, decode_one_shot_request,
    encode_one_shot_response, encode_protocol_error,
};

const MODE_ENV: &str = "SOLSTONE_GENERATE_ONE_SHOT_STUB_MODE";
const OVERRIDES_PATH_ENV: &str = "SOLSTONE_GENERATE_ONE_SHOT_STUB_OVERRIDES_PATH";
const API_KEY_OVERRIDE_ENV: &str = "SOLSTONE_GENERATE_API_KEY_OVERRIDE";
const MODEL_OVERRIDE_ENV: &str = "SOLSTONE_GENERATE_MODEL_OVERRIDE";
const PROVIDER_OVERRIDE_ENV: &str = "SOLSTONE_GENERATE_PROVIDER_OVERRIDE";
const PID_PATH_ENV: &str = "SOLSTONE_GENERATE_ONE_SHOT_STUB_PID_PATH";
const ARGV_PATH_ENV: &str = "SOLSTONE_GENERATE_ONE_SHOT_STUB_ARGV_PATH";
const EXPECT_ARGV_ENV: &str = "SOLSTONE_GENERATE_ONE_SHOT_STUB_EXPECT_ARGV";

fn main() {
    let arguments = env::args().collect::<Vec<_>>();
    write_argv(&arguments);
    write_pid();
    check_expect_argv(&arguments);

    let mode = env::var(MODE_ENV).unwrap_or_else(|_| "generated".to_owned());
    match mode.as_str() {
        "close_stdin_early" => {
            eprint!("early-exit");
            std::process::exit(70);
        }
        "abort" => std::process::abort(),
        _ => {}
    }

    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .expect("one-shot stub stdin is readable");

    match mode.as_str() {
        "pressure" => {
            let stdout = vec![b'x'; STDOUT_LIMIT + 1];
            let stderr = vec![b'y'; STDERR_LIMIT + 1];
            io::stdout()
                .write_all(&stdout)
                .expect("pressure stdout writes");
            io::stderr()
                .write_all(&stderr)
                .expect("pressure stderr writes");
            std::process::exit(70);
        }
        "success_oversized" => {
            let stdout = vec![b'x'; STDOUT_LIMIT + 1];
            eprint!("oversized diagnostic");
            io::stdout()
                .write_all(&stdout)
                .expect("oversized stdout writes");
            return;
        }
        "success_malformed" => {
            eprint!("malformed diagnostic");
            println!("not-json");
            return;
        }
        "plain_stderr" => {
            eprint!("Usage:\n  solstone-core --version\n");
            std::process::exit(64);
        }
        "empty_stderr" => std::process::exit(70),
        "invalid_utf8_stderr" => {
            io::stderr()
                .write_all(&[0xff])
                .expect("invalid stderr writes");
            std::process::exit(70);
        }
        "malformed_json_stderr" => {
            eprint!("{{not json");
            std::process::exit(70);
        }
        _ => {}
    }

    let request = decode_one_shot_request(&input).expect("one-shot request is valid");
    record_overrides();

    match mode.as_str() {
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
        "hard_failure" => protocol_failure(request.id, 70),
        "protocol_64" => protocol_failure(request.id, 64),
        mode => panic!("unsupported one-shot stub mode {mode}"),
    }
}

fn write_argv(arguments: &[String]) {
    let Ok(path) = env::var(ARGV_PATH_ENV) else {
        return;
    };
    fs::write(
        path,
        serde_json::to_vec(arguments).expect("argv record serializes"),
    )
    .expect("argv record writes");
}

fn write_pid() {
    let Ok(path) = env::var(PID_PATH_ENV) else {
        return;
    };
    fs::write(path, std::process::id().to_string()).expect("pid record writes");
}

fn check_expect_argv(arguments: &[String]) {
    if let Ok(expected) = env::var(EXPECT_ARGV_ENV) {
        let expected = expected
            .split(',')
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>();
        let actual = arguments.get(1..).unwrap_or(&[]);
        assert_eq!(
            actual, expected,
            "one-shot stub argv mismatch: expected {expected:?}, got {actual:?}"
        );
        return;
    }
    assert_eq!(arguments.len(), 2, "one-shot stub accepts only --one-shot");
    assert_eq!(
        arguments[1], "--one-shot",
        "one-shot stub requires --one-shot"
    );
}

fn protocol_failure(id: Option<String>, exit: i32) -> ! {
    eprint!(
        "{}",
        encode_protocol_error(&ProtocolError {
            id,
            reason: "stub_failure".to_owned(),
            detail: "one-shot stub hard failure".to_owned(),
        })
        .expect("protocol error serializes")
    );
    std::process::exit(exit);
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
