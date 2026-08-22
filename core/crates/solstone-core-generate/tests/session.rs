// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

mod support;

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use solstone_core_generate::{
    ContentPart, GenerateRequest, GenerateResponse, SessionClient, SessionCompletion,
};

struct Journal {
    path: PathBuf,
}

impl Journal {
    fn no_engine() -> Self {
        let path = std::env::temp_dir().join(format!(
            "solstone-generate-session-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        fs::create_dir_all(path.join("config")).unwrap();
        fs::write(
            path.join("config/journal.json"),
            r#"{"providers":{"active":{"provider":"none"}}}"#,
        )
        .unwrap();
        Self { path }
    }
}

impl Drop for Journal {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn request(id: &str) -> GenerateRequest {
    GenerateRequest {
        id: Some(id.to_owned()),
        context: "test.generate".to_owned(),
        contents: vec![ContentPart::Text {
            text: "OK".to_owned(),
        }],
        system_instruction: None,
        temperature: 0.3,
        max_output_tokens: 16,
        thinking_budget: None,
        timeout_s: Some(3.0),
        json_output: false,
        json_schema: None,
        enforce_responsiveness: true,
        attempt_index: 0,
        exclusive_admission: false,
        transport_retries: None,
    }
}

fn client(journal: &Journal) -> SessionClient {
    SessionClient::at_path(support::core_binary())
        .with_prefix_arguments(support::prefix())
        .with_env("SOLSTONE_JOURNAL", journal.path.as_os_str())
        .spawn(2)
        .unwrap()
}

fn response_id(completion: SessionCompletion) -> String {
    let SessionCompletion::Response(response) = completion else {
        panic!("expected a generate response")
    };
    match response {
        GenerateResponse::Generated(response) => response.id.unwrap(),
        GenerateResponse::Refused(response) => response.id.unwrap(),
    }
}

#[test]
fn criterion_1_real_child_accepts_concurrent_requests() {
    let journal = Journal::no_engine();
    let client = client(&journal);
    client.submit(request("first")).unwrap();
    client.submit(request("second")).unwrap();
    client.close().unwrap();
    let ids = [client.recv().unwrap(), client.recv().unwrap()]
        .into_iter()
        .map(response_id)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        ids,
        BTreeSet::from(["first".to_owned(), "second".to_owned()])
    );
}

#[test]
fn criterion_7_real_child_close_drains_outstanding_request() {
    let journal = Journal::no_engine();
    let client = client(&journal);
    client.submit(request("drain")).unwrap();
    client.close().unwrap();
    assert_eq!(response_id(client.recv().unwrap()), "drain");
}

#[test]
fn criterion_9_real_child_response_keeps_request_id() {
    let journal = Journal::no_engine();
    let client = client(&journal);
    client.submit(request("correlated")).unwrap();
    client.close().unwrap();
    assert_eq!(response_id(client.recv().unwrap()), "correlated");
}
