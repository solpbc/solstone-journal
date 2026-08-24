// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::env;
use std::fs;

use serde_json::json;

fn main() {
    let mut values = env::args().skip(1);
    assert_eq!(values.next().as_deref(), Some("detect"));
    let mut model = None;
    let mut input = None;
    let mut output = None;
    let mut threshold = None;
    let mut threads = None;
    while let Some(flag) = values.next() {
        let value = values.next().expect("flag value");
        match flag.as_str() {
            "--model" => model = Some(value),
            "--input" => input = Some(value),
            "--output" => output = Some(value),
            "--threshold" => threshold = Some(value),
            "--threads" => threads = Some(value),
            _ => panic!("unexpected flag {flag}"),
        }
    }
    assert!(model.is_some());
    let input = input.expect("input");
    let output = output.expect("output");
    if let Some(path) = env::var_os("SOLSTONE_DESCRIBE_DETECT_STUB_REQUESTS_PATH") {
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .expect("log");
        use std::io::Write;
        writeln!(file, "{}", json!({"input_bytes":fs::metadata(&input).expect("input metadata").len(),"threshold":threshold,"threads":threads})).expect("log row");
    }
    match env::var("SOLSTONE_DESCRIBE_DETECT_STUB_MODE").as_deref().unwrap_or("detected") {
        "detected" => fs::write(output, json!({"image":{"width":1,"height":1},"detections":[{"class_name":"laptop","score":0.9},{"class_name":"person","score":0.1}]}).to_string()).expect("output"),
        "invalid_json" => fs::write(output, "not json").expect("output"),
        "exit_failure" => std::process::exit(1),
        mode => panic!("unknown mode {mode}"),
    }
}
