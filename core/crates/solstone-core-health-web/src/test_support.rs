// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use chrono::{Datelike, Local, TimeZone};
use serde_json::{Value, json};
use std::{fs, path::Path};

const POPULATED_DAY: &str = "20260403";

pub fn root() -> tempfile::TempDir {
    phase_root("established_empty")
}

pub fn phase_root(phase: &str) -> tempfile::TempDir {
    let root = tempfile::tempdir().expect("temporary journal");
    match phase {
        "unestablished" => {}
        "corrupt" => write(root.path().join("config/journal.json"), b"{not json\n"),
        "established_empty" => config(root.path(), false),
        "established_populated" => populated(root.path(), 3, true, false),
        "populated_single_failure" => populated(root.path(), 1, true, false),
        "stats_absent" => populated(root.path(), 0, false, false),
        "stats_unparseable" => populated(root.path(), 0, false, true),
        _ => panic!("unknown health corpus phase {phase}"),
    }
    root
}

fn config(root: &Path, disabled_chat: bool) {
    let mut value = json!({"setup":{"completed_at":1700000000000_i64},"identity":{"name":"Corpus Owner","timezone":"UTC"}});
    if disabled_chat {
        value["talent_overrides"] = json!({"chat":{"disabled":true}});
    }
    json_file(root.join("config/journal.json"), &value);
}

fn populated(root: &Path, failures: usize, write_stats: bool, bad_stats: bool) {
    config(root, failures == 1);
    if write_stats {
        json_file(
            root.join("stats.json"),
            &json!({"backlog":{"generated_at":"2026-04-03T12:00:00Z","items":[],"status":"clear"},"summary":{"completed":2,"pending":1}}),
        );
        set_mtime(&root.join("stats.json"), 1_700_000_000);
    }
    if bad_stats {
        write(root.join("stats.json"), b"not valid json\n");
    }
    markers(root);
    tokens(root);
    runs(root, failures);
    write(
        root.join(format!(
            "chronicle/{POPULATED_DAY}/talents/example-output.md"
        )),
        b"# Corpus output\n\nDeterministic fixture content.\n",
    );
    if bad_stats {
        fs::create_dir_all(root.join("talents/20990101.jsonl"))
            .expect("unreadable index directory");
    }
}

fn markers(root: &Path) {
    for (day, daily, stream) in [
        ("20260214", None, 1_700_000_100),
        ("20260315", Some(1_700_000_200), 1_700_000_300),
        ("20260403", Some(1_700_000_500), 1_700_000_400),
    ] {
        let health = root.join(format!("chronicle/{day}/health"));
        fs::create_dir_all(&health).expect("health markers");
        write(health.join("stream.updated"), b"");
        set_mtime(&health.join("stream.updated"), stream);
        if let Some(daily) = daily {
            write(health.join("daily.updated"), b"");
            set_mtime(&health.join("daily.updated"), daily);
        }
    }
}

fn tokens(root: &Path) {
    jsonl(
        root.join("tokens/20260403.jsonl"),
        &[
            json!({"context":"talent.system.daily_digest","model":"gpt-5.5","segment":"090000_60","timestamp":"2026-04-03T09:00:00Z","type":"generate","usage":{"cached_tokens":300,"input_tokens":1000,"output_tokens":200,"reasoning_tokens":0,"total_tokens":1200}}),
            json!({"context":"talent.system.review","model":"claude-sonnet-4-6","segment":"100000_120","timestamp":"2026-04-03T10:00:00Z","type":"cogitate","usage":{"cached_tokens":0,"input_tokens":800,"output_tokens":400,"reasoning_tokens":100,"total_tokens":1200}}),
        ],
    );
}

fn runs(root: &Path, failures: usize) {
    let request = |use_id, name, ts| json!({"day":POPULATED_DAY,"event":"request","facet":"work","name":name,"prompt":"Corpus prompt","provider":"openai","ts":ts,"use_id":use_id});
    jsonl(
        root.join("talents/daily/1710000000000.jsonl"),
        &[
            request("1710000000000", "daily_digest", 1_710_000_000_000_i64),
            json!({"event":"start","model":"gpt-5.5","provider":"openai","ts":1710000000100_i64}),
            json!({"event":"finish","ts":1710000001000_i64,"usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}}),
        ],
    );
    jsonl(
        root.join("talents/review/1710000060000_active.jsonl"),
        &[request("1710000060000", "review", 1_710_000_060_000_i64)],
    );
    write(root.join("talents/summary/1710000120000.jsonl"), b"");
    jsonl(
        root.join("talents/20260403.jsonl"),
        &[
            json!({"facet":"work","name":"daily_digest","provider":"openai","status":"completed","ts":1710000000000_i64,"use_id":"1710000000000"}),
        ],
    );
    let today = Local::now().format("%Y%m%d").to_string();
    let noon = Local
        .with_ymd_and_hms(
            Local::now().year(),
            Local::now().month(),
            Local::now().day(),
            0,
            0,
            12,
        )
        .single()
        .expect("today")
        .timestamp_millis();
    let entries=(0..failures).map(|i|json!({"name":format!("failed_talent_{}",i+1),"provider":"openai","reason_code":"provider_error","status":"error","ts":noon+i as i64,"use_id":format!("capture-failure-{}",i+1)})).collect::<Vec<_>>();
    jsonl(root.join(format!("talents/{today}.jsonl")), &entries);
}

fn json_file(path: impl AsRef<Path>, value: &Value) {
    write(
        path,
        format!("{}\n", serde_json::to_string(value).expect("json")).as_bytes(),
    );
}
fn jsonl(path: impl AsRef<Path>, values: &[Value]) {
    write(
        path,
        values
            .iter()
            .map(|v| format!("{}\n", serde_json::to_string(v).expect("json")))
            .collect::<String>()
            .as_bytes(),
    );
}
fn write(path: impl AsRef<Path>, bytes: &[u8]) {
    let path = path.as_ref();
    fs::create_dir_all(path.parent().expect("parent")).expect("parent");
    fs::write(path, bytes).expect("write");
}
fn set_mtime(path: &Path, seconds: i64) {
    filetime::set_file_mtime(path, filetime::FileTime::from_unix_time(seconds, 0))
        .expect("set fixture mtime");
}

#[test]
fn populated_phase_preserves_seed_mtimes() {
    let root = phase_root("established_populated");
    let modified = |path: &Path| {
        path.metadata()
            .expect("metadata")
            .modified()
            .expect("mtime")
            .duration_since(std::time::UNIX_EPOCH)
            .expect("post-epoch")
            .as_secs()
    };
    assert_eq!(modified(&root.path().join("stats.json")), 1_700_000_000);
    assert_eq!(
        modified(&root.path().join("chronicle/20260315/health/daily.updated")),
        1_700_000_200
    );
    assert_eq!(
        modified(&root.path().join("chronicle/20260403/health/stream.updated")),
        1_700_000_400
    );
}
