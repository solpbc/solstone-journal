// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Test-only speakers-analyze process fixture for native transcribe reachability coverage.

use std::fs;
use std::io::{self, Read};

use serde_json::{Value, json};

const REQUEST_SCHEMA: &str = "solstone-speaker-analyze-request-v1";
const RESPONSE_SCHEMA: &str = "solstone-speaker-analyze-response-v1";
const EMBEDDING_WIDTH: usize = 256;

fn main() {
    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .expect("read speakers request");
    let request: Value = serde_json::from_str(&input).expect("parse speakers request");
    assert_eq!(request["schema"], REQUEST_SCHEMA);

    let statement_spans = request["statement_embedding"]["spans"]
        .as_array()
        .expect("statement spans");
    let statement_ids: Vec<i64> = statement_spans
        .iter()
        .map(|span| span["statement_id"].as_i64().expect("statement id"))
        .collect();
    let durations: Vec<f64> = statement_spans
        .iter()
        .map(|span| span["end_s"].as_f64().expect("end") - span["start_s"].as_f64().expect("start"))
        .collect();
    let spans_s: Vec<Value> = statement_spans
        .iter()
        .map(|span| json!([span["start_s"], span["end_s"]]))
        .collect();
    let payload_path = request["output_payload_f32le_path"]
        .as_str()
        .expect("payload path");
    let payload = vec![0_u8; statement_ids.len() * EMBEDDING_WIDTH * size_of::<f32>()];
    fs::write(payload_path, payload).expect("write embeddings");

    println!(
        "{}",
        serde_json::to_string(&json!({
            "schema": RESPONSE_SCHEMA,
            "sample_rate_hz": request["sample_rate_hz"],
            "inputs": {
                "statement_embedding": {
                    "statement_ids": statement_ids,
                    "spans_s": spans_s,
                },
                "diarization": {
                    "statement_ids": statement_spans.iter().map(|span| span["statement_id"].clone()).collect::<Vec<_>>(),
                    "spans_s": statement_spans.iter().map(|span| json!([span["start_s"], span["end_s"]])).collect::<Vec<_>>(),
                },
            },
            "statement_embeddings": {
                "audio_buffer": "full",
                "encoder": "wespeaker-resnet34-256",
                "payload_format": "raw-f32le-row-major-v1",
                "payload_path": payload_path,
                "dtype": "float32-le",
                "shape": [statement_ids.len(), EMBEDDING_WIDTH],
                "byte_count": statement_ids.len() * EMBEDDING_WIDTH * size_of::<f32>(),
                "statement_ids": statement_ids,
                "durations_s": durations,
                "admitted_count": statement_spans.len(),
                "skipped_count": 0,
            },
            "pyannote": {"window_stats": []},
            "evidence": {
                "speaker_evidence": "none",
                "multi_window_fraction": 0.0,
                "mean_window_overlap_share": 0.0,
                "overlap_fraction": 0.0,
            },
            "diarization": {
                "intervals": null,
                "valid_intervals": null,
                "interval_embeddings": null,
                "cluster_labels": null,
                "statement_labels": null,
                "silhouette_k": null,
                "effective_k": null,
            },
        }))
        .expect("serialize speakers response")
    );
}
