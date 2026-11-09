// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Replaying the recorded Body app corpus against the routes, on a journal this
//! file builds. Run the ignored `regenerate_corpus` test to record a change on
//! purpose, and say why in the commit.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::app_tests::{dated, get, reading, steps, window, workout};

const CORPUS: &str = include_str!("../../../fixtures/convey_body_corpus.json");
const FIXED: &[(&str, &str)] = &[
    ("/app/body/api/index", "/app/body/api/index"),
    ("/app/body/api/stats/<month>", "/app/body/api/stats/202601"),
    ("/app/body/api/status", "/app/body/api/status"),
    (
        "/app/body/api/recent",
        "/app/body/api/recent?before=20260120",
    ),
    ("/app/body/api/day/<day>", "/app/body/api/day/20260110"),
    (
        "/app/body/api/window",
        "/app/body/api/window?from=2026-01-10T09:00:00-07:00&to=2026-01-10T10:00:00-07:00",
    ),
];

fn seed_journal(root: &Path) {
    let imports = root.join("imports");
    fs::create_dir_all(&imports).unwrap();
    fs::write(
        imports.join(".apple_health-zone.json"),
        r#"{"zone":"America/Denver"}"#,
    )
    .unwrap();
    window(
        root,
        "20260110",
        "import.apple_health",
        "060000_3600",
        &[
            workout(
                "w1",
                "2026-01-10 06:31:00 -0700",
                "2026-01-10 07:20:00 -0700",
            ),
            steps(
                "s1",
                "2026-01-10 06:40:00 -0700",
                "2026-01-10 06:45:00 -0700",
                "1200",
            ),
        ],
    );
    window(
        root,
        "20260110",
        "import.apple_health",
        "090000_3600",
        &[reading("r1", "2026-01-10 09:05:00 -0700")],
    );
    window(
        root,
        "20260111",
        "import.oura",
        "000000_86400",
        &[dated("d1", "20260111")],
    );
    let done = imports.join("20260112_120000");
    fs::create_dir_all(&done).unwrap();
    fs::write(done.join("body-touched.jsonl"), "").unwrap();
    fs::write(done.join("body-progress.json"), "{}").unwrap();
    fs::write(
        done.join("manifest.json"),
        r#"{"source_type":"apple_health"}"#,
    )
    .unwrap();
    fs::write(done.join("summary.md"), "# Apple Health import\n").unwrap();
}

async fn recorded(phase: &str, root: &Path) -> Vec<Value> {
    let mut cases = Vec::new();
    for (rule, path) in FIXED {
        let (status, body) = get(root, path).await;
        cases.push(json!({"rule": rule, "path": path, "status": status.as_u16(), "json": body}));
    }
    json!({"phase": phase, "cases": cases})["cases"]
        .as_array()
        .unwrap()
        .clone()
}

fn corpus_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/convey_body_corpus.json")
}

async fn current() -> Value {
    let first = tempfile::tempdir().unwrap();
    let fixed = tempfile::tempdir().unwrap();
    seed_journal(fixed.path());
    json!({
        "schema": "solstone-convey-body-corpus-v2",
        "note": "Recorded Body app responses over synthetic body windows, for every route that reads windows. A change here is a change to the page's contract: rerun the ignored regenerate_corpus test and say why in the commit.",
        "cases": {
            "first_run": recorded("first_run", first.path()).await,
            "fixed": recorded("fixed", fixed.path()).await,
        }
    })
}

#[tokio::test]
async fn the_recorded_corpus_matches_every_route() {
    let expected: Value = serde_json::from_str(CORPUS).expect("corpus parses");
    let actual = current().await;
    for phase in ["first_run", "fixed"] {
        for (index, case) in expected["cases"][phase]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
        {
            assert_eq!(
                actual["cases"][phase][index], *case,
                "{phase} {}",
                case["path"]
            );
        }
    }
    assert_eq!(actual["schema"], expected["schema"]);
}

#[tokio::test]
#[ignore = "writes the fixture; run on purpose"]
async fn regenerate_corpus() {
    let mut text = serde_json::to_string_pretty(&current().await).unwrap();
    text.push('\n');
    fs::write(corpus_path(), text).unwrap();
}
