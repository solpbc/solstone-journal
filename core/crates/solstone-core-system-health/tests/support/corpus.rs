// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};

pub const PREVIOUS_DAY: &str = "20990201";
pub const CURRENT_DAY: &str = "20990202";
pub const SINCE_MS: i64 = 200;

pub fn write_corpus(root: &Path) {
    write(
        root,
        PREVIOUS_DAY,
        "001_previous.jsonl",
        &[
            r#"{"event":"talent.complete","ts":300,"mode":"segment","stream":"legacy","segment":"prior","name":"entities"}"#,
        ],
    );
    write(
        root,
        CURRENT_DAY,
        "001_primary.jsonl",
        &[
            "{malformed json",
            r#"{"event":"activity.detected","ts":1,"facet":"work"}"#,
            r#"{"event":"activity.persisted","ts":2,"facet":"work"}"#,
            r#"{"event":"activity.prompts_skipped","ts":3,"facet":"work"}"#,
            r#"{"event":"activity.unchanged","ts":4,"facet":"work"}"#,
            r#"{"event":"group.start","ts":5,"facet":"work"}"#,
            r#"{"event":"group.complete","ts":6,"facet":"work"}"#,
            r#"{"event":"memory_throttle.complete","ts":7}"#,
            r#"{"event":"phase.start","ts":8,"phase":"think"}"#,
            r#"{"event":"phase.complete","ts":9,"phase":"think"}"#,
            r#"{"event":"run.start","ts":10,"ref":"run-1"}"#,
            r#"{"event":"run.complete","ts":11,"ref":"run-1"}"#,
            r#"{"event":"sense.skip","ts":12,"mode":"segment","segment":"skipped"}"#,
            r#"{"event":"sense.complete","ts":13,"mode":"segment","stream":"default","segment":"progress","density":"active"}"#,
            r#"{"event":"sense.change_detect","ts":14,"mode":"segment","stream":"default","segment":"progress","change_class":"changed"}"#,
            r#"{"event":"talent.dispatch","ts":15,"mode":"segment","stream":"default","segment":"progress","name":"entities"}"#,
            r#"{"event":"talent.complete","ts":100,"mode":"segment","stream":"default","segment":"cadence","name":"documents"}"#,
            r#"{"event":"talent.complete","ts":900,"mode":"segment","stream":"default","segment":"cadence","name":"documents","cache_hit":true}"#,
            r#"{"event":"talent.complete","ts":400,"mode":"segment","stream":"default","segment":"current","name":"entities"}"#,
            r#"{"event":"talent.complete","ts":450,"mode":"activity","facet":"work","activity":"meeting","name":"summary"}"#,
            r#"{"event":"talent.complete","ts":500,"mode":"daily","facet":"work","name":"cross-file"}"#,
            r#"{"event":"talent.complete","ts":510,"mode":"daily","name":"completed-daily"}"#,
            r#"{"event":"talent.fail","ts":520,"mode":"daily","facet":"work","name":"daily-deterministic","reason_code":"no_output"}"#,
            r#"{"event":"talent.skip","ts":530,"mode":"segment","stream":"default","segment":"progress","name":"documents","reason":"capped"}"#,
            r#"{"event":"talent.skip","ts":531,"mode":"segment","stream":"default","segment":"progress","name":"entities","reason":"no_config"}"#,
            r#"{"event":"talent.skip","ts":532,"name":"mode-less","reason":"skip_talents_flag","detail":"dispatch disabled","day":"20990202"}"#,
            r#"{"event":"future.event","ts":533,"opaque":{"round_trips":true}}"#,
            r#"{"event":"talent.fail","ts":1000000,"mode":"segment","stream":"default","segment":"cap-true","name":"documents"}"#,
            r#"{"event":"talent.fail","ts":2800000,"mode":"segment","stream":"default","segment":"cap-true","name":"documents"}"#,
            r#"{"event":"talent.fail","ts":4600000,"mode":"segment","stream":"default","segment":"cap-true","name":"documents"}"#,
            r#"{"event":"talent.fail","ts":6400000,"mode":"segment","stream":"default","segment":"cap-true","name":"documents"}"#,
            r#"{"event":"talent.fail","ts":8200000,"mode":"segment","stream":"default","segment":"cap-true","name":"documents"}"#,
            r#"{"event":"talent.fail","ts":9000000,"mode":"segment","stream":"default","segment":"cap-short","name":"documents"}"#,
            r#"{"event":"talent.fail","ts":9000001,"mode":"segment","stream":"default","segment":"cap-short","name":"documents"}"#,
            r#"{"event":"talent.fail","ts":9000002,"mode":"segment","stream":"default","segment":"cap-short","name":"documents"}"#,
            r#"{"event":"talent.fail","ts":9000003,"mode":"segment","stream":"default","segment":"cap-short","name":"documents"}"#,
            r#"{"event":"talent.fail","ts":9000004,"mode":"segment","stream":"default","segment":"cap-short","name":"documents"}"#,
        ],
    );
    write(
        root,
        CURRENT_DAY,
        "999_tiebreak.jsonl",
        &[
            r#"{"event":"talent.fail","ts":500,"mode":"daily","facet":"work","name":"cross-file","reason_code":"no_output"}"#,
        ],
    );
}

fn write(root: &Path, day: &str, file: &str, lines: &[&str]) {
    let health = root.join("chronicle").join(day).join("health");
    fs::create_dir_all(&health).unwrap();
    fs::write(health.join(file), lines.join("\n") + "\n").unwrap();
}

pub fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .unwrap()
        .to_path_buf()
}

pub fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = destination.join(entry.file_name());
        let kind = entry.file_type().unwrap();
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}
