// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Test-only VAD process fixture for native transcribe reachability coverage.

use std::io::{self, Read};

use serde_json::json;

const REQUEST_SCHEMA: &str = "solstone-vad-request-v2";
const RESPONSE_SCHEMA: &str = "solstone-vad-response-v1";

fn main() {
    let mut request = String::new();
    io::stdin()
        .read_to_string(&mut request)
        .expect("read VAD request");
    let value: serde_json::Value = serde_json::from_str(&request).expect("parse VAD request");
    assert_eq!(value["schema"], REQUEST_SCHEMA);
    let models = value["models"].as_object().expect("VAD request models");
    assert_eq!(models.len(), 1);
    let model_path = models["firered_vad_onnx_path"]
        .as_str()
        .expect("FireRed model path");
    assert!(std::path::Path::new(model_path).is_file());
    println!(
        "{}",
        serde_json::to_string(&json!({
            "schema": RESPONSE_SCHEMA,
            "duration": 1.0,
            "speech_duration": 1.0,
            "has_speech": true,
            "speech": [{"start": 0, "end": 16000}],
        }))
        .expect("serialize VAD response")
    );
}
