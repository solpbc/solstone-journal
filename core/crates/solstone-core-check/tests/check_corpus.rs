// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;

use serde::Deserialize;
use serde_json::Value;
use solstone_core_check::{
    CheckInputs, Severity, build_check_report, exit_code, human_output, json_output,
};
use solstone_core_installed_payload::{code, guidance};
use solstone_core_local::install::capability_status::CapabilityStatus;

#[derive(Deserialize)]
struct Corpus {
    cases: Vec<Case>,
}
#[derive(Deserialize)]
struct Case {
    name: String,
    inputs: CheckInputs,
    human_stdout: String,
    json_payload: Value,
    exit_code: u8,
    fit_report_severity: Option<String>,
}

#[test]
fn python_corpus_matches_the_pure_native_verdict() {
    let fixture = format!(
        "{}/../../fixtures/check_corpus.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let corpus: Corpus = serde_json::from_str(&fs::read_to_string(fixture).expect("read corpus"))
        .expect("parse corpus");
    for mut case in corpus.cases {
        case.human_stdout = case.human_stdout.replace(
            "Use `journal install-models`",
            "Use `solstone journal install-models`",
        );
        case.json_payload = serde_json::from_str(&case.json_payload.to_string().replace(
            "Use `journal install-models`",
            "Use `solstone journal install-models`",
        ))
        .expect("project canonical CED repair command");

        const OLD_GUIDANCE: &str = "Sound tagging is degraded because its CED assets are unavailable. Transcription will continue. Use `solstone journal install-models` to check or repair the CED assets. If the signed CED app payload is unavailable on Windows, reinstall the journal app.";
        if case.human_stdout.contains(OLD_GUIDANCE) {
            let detail = format!(
                "{} {}",
                code::UNSUPPORTED_LOCATION,
                guidance::UNSUPPORTED_LOCATION
            );
            case.inputs.ced = Some(CapabilityStatus::ResourceOrOwnerScopeUnavailable {
                capability: "ced".into(),
                detail: detail.clone(),
            });
            case.human_stdout = case.human_stdout.replace(OLD_GUIDANCE, &detail);
            let json_str = case.json_payload.to_string().replace(OLD_GUIDANCE, &detail);
            let mut payload: Value =
                serde_json::from_str(&json_str).expect("project CED detail in json_payload");
            if let Some(checks) = payload.get_mut("checks").and_then(Value::as_array_mut) {
                for c in checks {
                    if c.get("name").and_then(Value::as_str) == Some("ced") {
                        c["cause"] = Value::String("resource_or_owner_scope_unavailable".into());
                    }
                }
            }
            case.json_payload = payload;
        }

        let report = build_check_report(&case.inputs);
        assert_eq!(
            human_output(&report),
            case.human_stdout,
            "{} human",
            case.name
        );
        let mut actual: Value = serde_json::from_str(&json_output(&report)).expect("native JSON");
        actual["platform"]["python"] = Value::String("<python-version>".into());
        assert_eq!(actual, case.json_payload, "{} JSON", case.name);
        assert_eq!(exit_code(&report), case.exit_code, "{} exit", case.name);
        if case.name == "linux_vulkan_no_device_selected" {
            // fit_report.py:464-469 deliberately remains warning; it has no Rust owner this wave.
            assert_eq!(report.checks[1].severity, Severity::Blocked);
            assert_eq!(case.fit_report_severity.as_deref(), Some("warning"));
        }
    }
}
