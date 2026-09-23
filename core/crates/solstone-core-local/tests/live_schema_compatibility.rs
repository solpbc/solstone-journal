// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Explicit live grammar-compilation probe for supported local-provider engines.
//!
//! This is ignored by ordinary offline CI. A release or schema-change check sets
//! `SOLSTONE_SCHEMA_PROBE_BASE_URL`, `SOLSTONE_SCHEMA_PROBE_MODEL`, and
//! `SOLSTONE_SCHEMA_PROBE_BACKEND`; `SOLSTONE_SCHEMA_PROBE_API_KEY` is optional.
//! The probe intentionally requests one output token: an HTTP success proves the
//! prepared grammar was admitted, while useful model output is outside this gate.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};
use solstone_core_local::prepare_local_schema;

const TIMEOUT: Duration = Duration::from_secs(120);

#[test]
#[ignore = "requires an explicitly configured live llama.cpp or SGLang endpoint"]
fn every_shipped_prepared_schema_is_admitted_by_the_configured_engine() {
    let base_url = required_env("SOLSTONE_SCHEMA_PROBE_BASE_URL");
    let model = required_env("SOLSTONE_SCHEMA_PROBE_MODEL");
    let backend = required_env("SOLSTONE_SCHEMA_PROBE_BACKEND");
    let credential = std::env::var("SOLSTONE_SCHEMA_PROBE_API_KEY").ok();
    let schemas = shipped_schema_files();
    assert!(schemas.len() >= 3, "expected shipped talent schemas");

    eprintln!(
        "local schema probe: backend={backend:?} model={model:?} schemas={}",
        schemas.len()
    );
    let invalid_control = json!({"type": "string", "pattern": "["});
    let control_status = post_status(
        &base_url,
        credential.as_deref(),
        &schema_request_body(&model, invalid_control),
    )
    .unwrap_or_else(|error| panic!("negative control via {backend:?}: {error}"));
    assert_eq!(
        control_status, 400,
        "{backend:?} did not reject the deliberately invalid pattern with HTTP 400; a green schema probe would be vacuous"
    );
    eprintln!("local schema probe: negative control PASS HTTP {control_status}");
    let mut rejected = Vec::new();
    for path in schemas {
        let contents = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        let schema: Value = serde_json::from_str(&contents)
            .unwrap_or_else(|error| panic!("parse {}: {error}", path.display()));
        let prepared = prepare_local_schema(&schema);
        let body = schema_request_body(&model, prepared);
        let status = post_status(&base_url, credential.as_deref(), &body)
            .unwrap_or_else(|error| panic!("probe {} via {backend:?}: {error}", path.display()));
        if (200..300).contains(&status) {
            eprintln!("local schema probe: PASS {} HTTP {status}", path.display());
        } else {
            eprintln!("local schema probe: FAIL {} HTTP {status}", path.display());
            rejected.push((path, status));
        }
    }
    assert!(
        rejected.is_empty(),
        "{} prepared schema(s) were rejected by {backend:?}: {}; response bodies are intentionally not captured",
        rejected.len(),
        rejected
            .iter()
            .map(|(path, status)| format!("{} (HTTP {status})", path.display()))
            .collect::<Vec<_>>()
            .join(", ")
    );
}

fn schema_request_body(model: &str, schema: Value) -> Value {
    json!({
        "model": model,
        "messages": [{"role": "user", "content": "Return one JSON object."}],
        "max_tokens": 1,
        "temperature": 0,
        "stream": false,
        "response_format": {
            "type": "json_schema",
            "json_schema": {
                "name": "solstone_schema_probe",
                "strict": true,
                "schema": schema
            }
        }
    })
}

fn required_env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} must be set for the ignored live probe"))
}

fn shipped_schema_files() -> Vec<PathBuf> {
    let repository_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("repository root");
    let mut roots = vec![repository_root.join("core/payload/solstone/talent")];
    let apps = repository_root.join("core/payload/solstone/apps");
    if let Ok(entries) = std::fs::read_dir(apps) {
        roots.extend(
            entries
                .flatten()
                .map(|entry| entry.path().join("talent"))
                .filter(|path| path.is_dir()),
        );
    }
    let mut schemas = roots
        .into_iter()
        .filter_map(|root| std::fs::read_dir(root).ok())
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".schema.json"))
        })
        .collect::<Vec<_>>();
    schemas.sort();
    schemas
}

fn post_status(base_url: &str, credential: Option<&str>, body: &Value) -> Result<u16, String> {
    let config = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_connect(Some(TIMEOUT))
        .timeout_recv_response(Some(TIMEOUT))
        .timeout_recv_body(Some(TIMEOUT))
        .timeout_global(Some(TIMEOUT))
        .build();
    let agent = ureq::Agent::new_with_config(config);
    let url = format!("{}/v1/chat/completions", base_url.trim_end_matches('/'));
    let mut request = agent.post(&url).header("Content-Type", "application/json");
    if let Some(credential) = credential {
        request = request.header("Authorization", &format!("Bearer {credential}"));
    }
    request
        .send(serde_json::to_string(body).expect("JSON value serializes"))
        .map(|response| response.status().as_u16())
        .map_err(|error| error.to_string())
}
