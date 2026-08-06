// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::env;
use std::io::{self, Write};
use std::thread;
use std::time::Duration;

use solstone_core_generate::{ContentPart, GenerateRequest, SessionClient};

fn main() {
    let wire = env::var_os("SOLSTONE_GENERATE_WIRE").expect("real wire path is configured");
    let journal = env::var_os("SOLSTONE_JOURNAL").expect("journal path is configured");
    let client = SessionClient::at_path(wire)
        .with_env("SOLSTONE_JOURNAL", journal.to_string_lossy())
        .spawn(1)
        .expect("wire session starts");
    client.submit(request()).expect("request submits");
    println!("{}", client.child_id().expect("wire child PID"));
    io::stdout().flush().expect("helper stdout flushes");

    loop {
        thread::sleep(Duration::from_secs(60));
    }
}

fn request() -> GenerateRequest {
    GenerateRequest {
        id: Some("kill-target".to_owned()),
        context: "test.generate".to_owned(),
        contents: vec![ContentPart::Text {
            text: "wait for cancellation".to_owned(),
        }],
        system_instruction: None,
        temperature: 0.3,
        max_output_tokens: 16,
        thinking_budget: None,
        timeout_s: Some(30.0),
        json_output: false,
        json_schema: None,
        enforce_responsiveness: true,
        attempt_index: 0,
        exclusive_admission: false,
        transport_retries: None,
    }
}
