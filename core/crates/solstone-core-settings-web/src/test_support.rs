// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::Path;

use axum::Router;
use serde_json::Value;
use tempfile::TempDir;

pub fn corpus() -> Value {
    serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/convey_settings_corpus.json"
    )))
    .expect("settings corpus")
}

pub fn established_root() -> TempDir {
    root_with_config(established_config())
}

pub fn phase_root(phase: &str) -> TempDir {
    match phase {
        "established" => established_root(),
        "rich" => root_with_config(rich_config()),
        "tokened" => {
            let mut config = rich_config();
            config["env"]["PLAUD_ACCESS_TOKEN"] =
                Value::String("plaud-token-MUST-NOT-LEAK".to_owned());
            root_with_config(config)
        }
        "populated" => populated_root(),
        "corrupt" => corrupt_root(),
        _ => panic!("known corpus phase: {phase}"),
    }
}

fn root_with_config(config: Value) -> TempDir {
    let root = TempDir::new().expect("temporary journal");
    write_json(root.path(), "config/journal.json", &config);
    root
}

fn established_config() -> Value {
    serde_json::json!({"setup": {"completed_at": 1_700_000_000_000_i64}})
}

fn rich_config() -> Value {
    serde_json::json!({
        "setup": {"completed_at": 1_700_000_000_000_i64},
        "identity": {"name":"Ada Lovelace", "preferred":"Ada", "bio":"first programmer", "pronouns":{"subject":"she","object":"her","possessive":"her","reflexive":"herself"}, "aliases":["ada","AAL"], "email_addresses":["ada@example.org"], "timezone":"Europe/London"},
        "journal":{"name":"Analytical Engine"},
        "agent":{"name":"sol","name_status":"named","named_date":"2026-01-02"},
        "support":{"enabled":true,"proactive":false,"anonymous_feedback":true,"portal_url":"https://support.example.org"},
        "env":{"PLAUD_ACCESS_TOKEN":""},
        "transcribe":{"backend":"parakeet","preserve_all":true,"confidential_audio":false,"parakeet":{"model_version":"v3","device":"auto","timeout_sec":120.0},"whisper":{"model":"large-v3"},"not_a_real_key":"should not survive projection"},
        "observe":{"tmux":{"enabled":false,"capture_interval":17}},
        "describe":{"max_extractions":42,"redact":["password","secret"],"categories":{}},
        "retention":{"raw_media":"days","raw_media_days":90,"per_stream":{"tmux":{"raw_media":"processed"}},"journal_logs":{"enabled":true,"days":14}},
        "processing":{},
        "convey":{"secret":"MUST-NOT-LEAK","password_hash":"MUST-NOT-LEAK","password":"MUST-NOT-LEAK","bind":"127.0.0.1"},
        "providers":{"active":{"provider":"unused"}},
        "service_key_validation":{"plaud":{"valid":false,"timestamp":"2026-01-01T00:00:00Z"},"bogus":{"valid":true}},
        "some_future_section":{"kept":true,"n":7}
    })
}

pub fn populated_root() -> TempDir {
    let root = TempDir::new().expect("temporary journal");
    let corpus = corpus();
    let files = corpus["phases"]["populated"]["_journal_tree"]["files"]
        .as_object()
        .expect("populated tree");
    for (relative, value) in files {
        let target = root.path().join(relative);
        fs::create_dir_all(target.parent().expect("parent")).expect("tree parent");
        match value.as_str().expect("tree text") {
            "<BINARY>" if relative.ends_with("audio.flac") => {
                fs::write(target, vec![0_u8; 4096]).expect("audio")
            }
            "<BINARY>" if relative.ends_with("monitor_1_diff.png") => {
                fs::write(target, vec![0_u8; 2048]).expect("screen")
            }
            "<BINARY>" => panic!("unknown binary fixture file: {relative}"),
            text => fs::write(target, text).expect("tree file"),
        }
    }
    root
}

pub fn corrupt_root() -> TempDir {
    let root = TempDir::new().expect("temporary journal");
    fs::create_dir_all(root.path().join("config")).expect("config directory");
    fs::write(
        root.path().join("config/journal.json"),
        "{\"identity\": {\"name\": \"Ada\",\n",
    )
    .expect("corrupt config");
    root
}

pub fn shell_router(root: &Path) -> Router {
    solstone_core_convey_shell::router(root.to_path_buf())
}

pub fn request_path(case: &str) -> String {
    let name = case.strip_prefix("GET ").expect("GET case");
    let translated = match name {
        "api/facet/absent" => "api/facet/no-such-facet",
        "api/facet/absent/logs" => "api/facet/no-such-facet/logs",
        "api/facet/absent/activities" => "api/facet/no-such-facet/activities",
        "api/icons?q=sett" => "api/icons?q=sett&limit=5",
        other => other,
    };
    format!("/app/settings/{translated}")
}

fn write_json(root: &Path, relative: &str, value: &Value) {
    let target = root.join(relative);
    fs::create_dir_all(target.parent().expect("parent")).expect("directory");
    fs::write(
        target,
        format!("{}\n", serde_json::to_string_pretty(value).expect("JSON")),
    )
    .expect("config");
}
