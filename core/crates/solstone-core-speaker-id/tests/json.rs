// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Map, Value};
use solstone_core_speaker_id::json::write_python_compatible_json;

const FULL: &[u8] = b"{\n  \"unknown_top\": {\n    \"preserve\": \"yes\"\n  },\n  \"labels\": [\n    {\n      \"sentence_id\": 1,\n      \"speaker\": \"Jos\\u00e9 \\ud83d\\ude3a\",\n      \"confidence\": \"medium\",\n      \"method\": \"acoustic\",\n      \"owner_margin_declined\": true,\n      \"unknown_row\": \"kept\"\n    },\n    {\n      \"sentence_id\": 2,\n      \"speaker\": null,\n      \"confidence\": \"low\",\n      \"method\": \"context\",\n      \"unknown_row\": \"also_kept\"\n    }\n  ],\n  \"owner_centroid_last_refreshed_at\": \"2026-08-08T00:00:00Z\",\n  \"voiceprint_versions\": {\n    \"jos\\u00e9\": 2\n  },\n  \"candidate_evidence\": [\n    {\n      \"name\": \"Jos\\u00e9 \\ud83d\\ude3a\"\n    }\n  ],\n  \"candidate_evidence_gaps\": [\n    {\n      \"source\": \"screen\",\n      \"reason\": \"Caf\\u00e9 \\ud83e\\uddea\"\n    }\n  ]\n}\n";

const STUB: &[u8] =
    b"{\n  \"labels\": [],\n  \"skipped\": true,\n  \"reason\": \"d\\u00e9j\\u00e0 \\ud83c\\udf1f\"\n}\n";

#[test]
fn ac1_parse_then_reserialize_preserves_original_key_order() {
    let mut value: Value = serde_json::from_str(
        r#"{
  "unknown_top": {"preserve": "before"},
  "labels": [
    {
      "sentence_id": 1,
      "speaker": "José 😺",
      "confidence": "medium",
      "method": "acoustic",
      "owner_margin_declined": true,
      "unknown_row": "kept"
    },
    {
      "sentence_id": 2,
      "speaker": null,
      "confidence": "low",
      "method": "context",
      "unknown_row": "also_kept"
    }
  ],
  "owner_centroid_last_refreshed_at": "before",
  "voiceprint_versions": {"josé": 2},
  "candidate_evidence": [{"name": "José 😺"}],
  "candidate_evidence_gaps": [{"source": "screen", "reason": "Café 🧪"}]
}"#,
    )
    .expect("fixture is valid JSON");

    let object = value.as_object_mut().expect("fixture is an object");
    object
        .get_mut("unknown_top")
        .and_then(Value::as_object_mut)
        .expect("unknown_top is an object")
        .insert("preserve".to_owned(), Value::String("yes".to_owned()));
    object.insert(
        "owner_centroid_last_refreshed_at".to_owned(),
        Value::String("2026-08-08T00:00:00Z".to_owned()),
    );

    let mut bytes = write_python_compatible_json(&value, 2)
        .expect("serialization succeeds")
        .into_bytes();
    bytes.push(b'\n');
    assert_eq!(bytes, FULL);
}

#[test]
fn ac2_serializes_stub_with_python_ascii_escaping() {
    let mut stub = Map::new();
    stub.insert("labels".to_owned(), Value::Array(Vec::new()));
    stub.insert("skipped".to_owned(), Value::Bool(true));
    stub.insert("reason".to_owned(), Value::String("déjà 🌟".to_owned()));

    let mut bytes = write_python_compatible_json(&Value::Object(stub), 2)
        .expect("serialization succeeds")
        .into_bytes();
    bytes.push(b'\n');
    assert_eq!(bytes, STUB);
}
