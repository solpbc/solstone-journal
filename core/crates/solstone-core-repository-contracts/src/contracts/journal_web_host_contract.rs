// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Generated-contract oracle for the journal web host contract and browser boundary.
//!
//! Source definition: `contracts/journal-web-host/v1.json`
//! Regenerate committed artifacts with:
//! `cargo test --manifest-path core/Cargo.toml -p solstone-core-repository-contracts --lib journal_web_host_contract::regenerate_journal_web_host_contract -- --ignored`
//!
//! No-retyping rule: `contracts/journal-web-host/v1.json` is the only hand-authored occurrence
//! of the product token and capability name in the repository.

use std::fs;
use std::path::{Path, PathBuf};

use regex::Regex;
use serde_json::Value;

const SOURCE_REL_PATH: &str = "contracts/journal-web-host/v1.json";
const HOST_CONTRACT_REL_PATH: &str = "contracts/journal-web-host/host-contract.json";
const CLIENT_JS_REL_PATH: &str =
    "core/crates/solstone-core-convey-shell/assets/static/journal-web-host.js";

const SOURCE_JSON_STR: &str = include_str!("../../../../../contracts/journal-web-host/v1.json");

fn repository_root() -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("repository checkout root")
        .to_path_buf();
    assert!(
        root.join("Makefile").is_file(),
        "repository root has Makefile"
    );
    root
}

fn generate_artifacts(source_json: &str) -> (Vec<u8>, Vec<u8>) {
    let parsed: Value = serde_json::from_str(source_json).expect("parse source json");
    let version = parsed
        .get("version")
        .and_then(Value::as_i64)
        .expect("version integer");
    assert_eq!(version, 1, "v1 definition must declare supported version 1");
    let user_agent_product = parsed
        .get("user_agent_product")
        .and_then(Value::as_str)
        .expect("user_agent_product string");
    let javascript_capability = parsed
        .get("javascript_capability")
        .and_then(Value::as_str)
        .expect("javascript_capability string");

    let ident_re = Regex::new(r"^[A-Za-z_$][A-Za-z0-9_$]*$").expect("valid regex");
    assert!(
        ident_re.is_match(javascript_capability),
        "javascript_capability must be a valid JavaScript identifier: {javascript_capability}"
    );

    let init_script =
        format!("if (window.top === window) window[\"{javascript_capability}\"] = {version};");

    // 1. host-contract.json
    let mut host_obj = serde_json::Map::new();
    host_obj.insert("version".to_string(), Value::from(version));
    host_obj.insert(
        "user_agent_product".to_string(),
        Value::from(user_agent_product),
    );
    host_obj.insert(
        "javascript_capability".to_string(),
        Value::from(javascript_capability),
    );
    host_obj.insert(
        "initialization_script".to_string(),
        Value::from(init_script),
    );

    let mut host_bytes =
        serde_json::to_vec_pretty(&Value::Object(host_obj)).expect("serialize host contract");
    host_bytes.push(b'\n');

    // 2. journal-web-host.js
    let js_text = format!(
        "// SPDX-License-Identifier: AGPL-3.0-only\n\
// Copyright (c) 2026 sol pbc\n\
\n\
// Generated from {}\n\
// Regenerate with:\n\
//   cargo test --manifest-path core/Cargo.toml -p solstone-core-repository-contracts --lib journal_web_host_contract::regenerate_journal_web_host_contract -- --ignored\n\
// Do not edit directly; the source definition is the only hand-authored occurrence of the product token and the capability name.\n\
\n\
window.solstoneJournalWebHost = {{\n\
  version: {},\n\
  userAgentProduct: {},\n\
  javascriptCapability: {},\n\
}};\n",
        SOURCE_REL_PATH,
        version,
        serde_json::to_string(user_agent_product).expect("serialize UA product"),
        serde_json::to_string(javascript_capability).expect("serialize capability name")
    );

    let import = import_contract();
    let auth = auth_contract();
    let export = export_contract();
    let js_text = format!(
        "{js_text}\nwindow.solstoneImportContract = Object.freeze({import});\nwindow.solstoneAuthContract = Object.freeze({auth});\nwindow.solstoneExportContract = Object.freeze({export});\n"
    );
    (host_bytes, js_text.into_bytes())
}

const IMPORT_CONTRACT: &str = "contracts/journal-web-host/import-host-contract.json";
const AUTH_CONTRACT: &str = "contracts/journal-web-host/auth-host-contract.json";
const EXPORT_CONTRACT: &str = "contracts/journal-web-host/export-host-contract.json";

fn import_contract() -> Value {
    let mut contract: Value = serde_json::from_str(include_str!(
        "../../../../../contracts/journal-web-host/v2.json"
    ))
    .expect("parse v2 definition");
    contract.sort_all_objects();
    contract
}

fn import_bytes() -> Vec<u8> {
    let mut bytes = serde_json::to_vec_pretty(&import_contract()).expect("serialize v2 definition");
    bytes.push(b'\n');
    bytes
}

fn auth_contract() -> Value {
    let mut contract: Value = serde_json::from_str(include_str!(
        "../../../../../contracts/journal-web-host/auth.json"
    ))
    .expect("parse auth definition");
    assert_eq!(
        contract.get("version").and_then(Value::as_i64),
        Some(2),
        "auth contract must target version 2"
    );
    assert_eq!(
        contract.get("capability").and_then(Value::as_str),
        Some("solstoneJournalHost"),
        "auth contract must ride on solstoneJournalHost"
    );
    for key in [
        "prepare",
        "prepared",
        "authorize",
        "result",
        "cancel",
        "unsupported",
    ] {
        assert!(
            contract
                .get(key)
                .and_then(Value::as_str)
                .is_some_and(|value| !value.is_empty()),
            "auth contract missing string token for {key}"
        );
    }
    contract.sort_all_objects();
    contract
}

fn auth_bytes() -> Vec<u8> {
    let mut bytes = serde_json::to_vec_pretty(&auth_contract()).expect("serialize auth definition");
    bytes.push(b'\n');
    bytes
}

fn export_contract() -> Value {
    let mut contract: Value = serde_json::from_str(include_str!(
        "../../../../../contracts/journal-web-host/export.json"
    ))
    .expect("parse export definition");
    assert_eq!(
        contract.get("version").and_then(Value::as_i64),
        Some(2),
        "export contract must target version 2"
    );
    assert_eq!(
        contract.get("capability").and_then(Value::as_str),
        Some("solstoneJournalHost"),
        "export contract must ride on solstoneJournalHost"
    );
    for key in [
        "start",
        "status",
        "pause",
        "resume",
        "cancel",
        "reveal",
        "deviceSpace",
        "event",
        "unsupported",
    ] {
        assert!(
            contract
                .get(key)
                .and_then(Value::as_str)
                .is_some_and(|value| !value.is_empty()),
            "export contract missing string token for {key}"
        );
    }
    contract.sort_all_objects();
    contract
}

fn export_bytes() -> Vec<u8> {
    let mut bytes =
        serde_json::to_vec_pretty(&export_contract()).expect("serialize export definition");
    bytes.push(b'\n');
    bytes
}

#[test]
fn generated_journal_web_host_artifacts_match_committed() {
    let root = repository_root();
    assert_eq!(
        fs::read(root.join(IMPORT_CONTRACT)).unwrap(),
        import_bytes()
    );
    assert_eq!(
        fs::read(root.join(AUTH_CONTRACT)).unwrap(),
        auth_bytes(),
        "committed {AUTH_CONTRACT} differs from generated"
    );
    assert_eq!(
        fs::read(root.join(EXPORT_CONTRACT)).unwrap(),
        export_bytes(),
        "committed {EXPORT_CONTRACT} differs from generated"
    );
    let (expected_host_json, expected_client_js) = generate_artifacts(SOURCE_JSON_STR);

    let actual_host_json = fs::read(root.join(HOST_CONTRACT_REL_PATH))
        .unwrap_or_else(|err| panic!("read committed {HOST_CONTRACT_REL_PATH}: {err}"));
    assert_eq!(
        actual_host_json, expected_host_json,
        "committed {HOST_CONTRACT_REL_PATH} differs from generated"
    );

    let actual_client_js = fs::read(root.join(CLIENT_JS_REL_PATH))
        .unwrap_or_else(|err| panic!("read committed {CLIENT_JS_REL_PATH}: {err}"));
    assert_eq!(
        actual_client_js, expected_client_js,
        "committed {CLIENT_JS_REL_PATH} differs from generated"
    );
}

#[test]
fn auth_conformance_cases_match_auth_contract() {
    let contract = auth_contract();
    let cases: Value = serde_json::from_str(include_str!(
        "../../../../../contracts/journal-web-host/conformance/auth-cases.json"
    ))
    .expect("parse auth conformance cases");
    let prepare_type = contract["prepare"].as_str().unwrap();
    let authorize_type = contract["authorize"].as_str().unwrap();
    let cancel_type = contract["cancel"].as_str().unwrap();

    assert_ne!(cases["unknown"]["type"].as_str(), Some(prepare_type));
    assert_ne!(cases["unknown"]["type"].as_str(), Some(authorize_type));
    assert_ne!(cases["unknown"]["type"].as_str(), Some(cancel_type));
    assert_eq!(cases["prepare_all"]["type"].as_str(), Some(prepare_type));
    assert_eq!(
        cases["prepare_loopback_only"]["type"].as_str(),
        Some(prepare_type)
    );
    assert_eq!(
        cases["prepare_custom_scheme_only"]["type"].as_str(),
        Some(prepare_type)
    );
    assert_eq!(
        cases["prepare_invalid_host"]["type"].as_str(),
        Some(prepare_type)
    );
    assert_eq!(
        cases["prepare_invalid_path"]["type"].as_str(),
        Some(prepare_type)
    );
    assert_eq!(
        cases["prepare_empty_preferences"]["type"].as_str(),
        Some(prepare_type)
    );
    assert_eq!(cases["authorize"]["type"].as_str(), Some(authorize_type));
    assert_eq!(
        cases["authorize_http"]["type"].as_str(),
        Some(authorize_type)
    );
    assert_eq!(
        cases["authorize_javascript"]["type"].as_str(),
        Some(authorize_type)
    );
    assert_eq!(
        cases["authorize_userinfo"]["type"].as_str(),
        Some(authorize_type)
    );
    assert_eq!(
        cases["authorize_unknown_session"]["type"].as_str(),
        Some(authorize_type)
    );
    assert_eq!(cases["cancel"]["type"].as_str(), Some(cancel_type));
}

#[test]
#[ignore = "writes committed contract artifacts; run explicitly when regenerating"]
fn regenerate_journal_web_host_contract() {
    let root = repository_root();
    fs::write(root.join(IMPORT_CONTRACT), import_bytes()).unwrap();
    fs::write(root.join(AUTH_CONTRACT), auth_bytes()).unwrap();
    fs::write(root.join(EXPORT_CONTRACT), export_bytes()).unwrap();
    let (host_json, client_js) = generate_artifacts(SOURCE_JSON_STR);

    let host_path = root.join(HOST_CONTRACT_REL_PATH);
    if let Some(parent) = host_path.parent() {
        fs::create_dir_all(parent).expect("create host contract dir");
    }
    fs::write(&host_path, host_json)
        .unwrap_or_else(|err| panic!("write {HOST_CONTRACT_REL_PATH}: {err}"));

    let client_path = root.join(CLIENT_JS_REL_PATH);
    if let Some(parent) = client_path.parent() {
        fs::create_dir_all(parent).expect("create client js dir");
    }
    fs::write(&client_path, client_js)
        .unwrap_or_else(|err| panic!("write {CLIENT_JS_REL_PATH}: {err}"));
}
