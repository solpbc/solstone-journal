// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use solstone_core_sol_client::seam::ScriptedHttpTransport;
use solstone_core_sol_client_cli::{DispatchSeams, dispatch_sol_speaker_id_with_seams};

fn temporary_segment(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let segment = std::env::temp_dir().join(format!(
        "solstone-core-sol-client-cli-speaker-id-{name}-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir_all(&segment).expect("temporary segment is created");
    segment
}

#[test]
fn ac1_speaker_id_full_dispatch_writes_labels() {
    let segment = temporary_segment("full");
    let transport = ScriptedHttpTransport::new(vec![]);
    let args = vec!["full".to_owned(), segment.display().to_string()];
    let output = dispatch_sol_speaker_id_with_seams(
        &args,
        &BTreeMap::new(),
        r#"{"labels":[{"sentence_id":1,"speaker":"José 😺"}],"metadata":{}}"#,
        "20260808",
        DispatchSeams {
            transport: &transport,
            clock: None,
            chat_events: None,
            files: None,
            build_identity: None,
            client_item_ids: None,
            notification_sink: None,
        },
    );

    assert_eq!(output.exit, 0);
    assert!(output.stderr.is_empty());
    let labels_path = segment.join("talents").join("speaker_labels.json");
    let labels: Value =
        serde_json::from_slice(&fs::read(&labels_path).expect("labels are written"))
            .expect("labels are valid JSON");
    assert_eq!(labels["labels"][0]["speaker"], "José 😺");
    transport.assert_done();
    fs::remove_dir_all(segment).expect("temporary segment is removed");
}

#[test]
fn ac2_speaker_id_append_correction_dispatch_writes_corrections() {
    let segment = temporary_segment("append-correction");
    let transport = ScriptedHttpTransport::new(vec![]);
    let args = vec![
        "append-correction".to_owned(),
        segment.display().to_string(),
    ];
    let output = dispatch_sol_speaker_id_with_seams(
        &args,
        &BTreeMap::new(),
        r#"{"sentence_id":1,"corrected_speaker":"Owner"}"#,
        "20260808",
        DispatchSeams {
            transport: &transport,
            clock: None,
            chat_events: None,
            files: None,
            build_identity: None,
            client_item_ids: None,
            notification_sink: None,
        },
    );

    assert_eq!(output.exit, 0);
    assert!(output.stderr.is_empty());
    let corrections_path = segment.join("talents").join("speaker_corrections.json");
    let corrections: Value =
        serde_json::from_slice(&fs::read(&corrections_path).expect("corrections are written"))
            .expect("corrections are valid JSON");
    assert_eq!(corrections["corrections"][0]["corrected_speaker"], "Owner");
    transport.assert_done();
    fs::remove_dir_all(segment).expect("temporary segment is removed");
}
