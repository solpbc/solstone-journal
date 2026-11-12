// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Value, json};
use solstone_core_format::calendar::parse_calendar_note;

use super::*;

const GOLDEN: &str = include_str!("../../../fixtures/calendar_notes/golden.jsonl");

fn line(index: usize, changes: Value) -> String {
    let Value::Object(mut record) =
        serde_json::from_str::<Value>(GOLDEN.lines().nth(index).unwrap()).unwrap()
    else {
        panic!("golden line is an object");
    };
    for (key, value) in changes.as_object().unwrap() {
        record.insert(key.clone(), value.clone());
    }
    Value::Object(record).to_string()
}

fn content(text: &str) -> String {
    content_of(&parse_calendar_note(text).unwrap())
}

#[test]
fn a_note_noticed_again_says_the_same_and_an_edit_or_removal_does_not() {
    let sent = content(&line(0, json!({})));
    let noticed_later = content(&line(
        0,
        json!({"change": "changed", "changed_at": "2026-11-12T10:00:00-07:00"}),
    ));
    assert_eq!(sent, noticed_later);

    let retitled = content(&line(0, json!({"title": "design review, moved"})));
    assert_ne!(sent, retitled);

    let removed = content(&line(0, json!({"change": "removed"})));
    assert_ne!(sent, removed);
}

#[test]
fn an_answer_names_every_file_as_it_arrived() {
    let mut receipt = Receipt::default();
    receipt.trimmed.insert(
        "calendar_kept.jsonl".to_owned(),
        ("calendar_sent.jsonl".to_owned(), 40, "aa".to_owned()),
    );
    receipt.held.push(FileDescriptor {
        submitted: "calendar_held.jsonl".to_owned(),
        written: "calendar_held.jsonl".to_owned(),
        size: 7,
        sha256: "bb".to_owned(),
        extra: Map::new(),
    });
    let written = FileDescriptor {
        submitted: "calendar_kept.jsonl".to_owned(),
        written: "calendar_kept.jsonl".to_owned(),
        size: 20,
        sha256: "cc".to_owned(),
        extra: Map::new(),
    };
    let restored = receipt.restore(vec![written]);
    let named = restored
        .iter()
        .map(|descriptor| {
            (
                descriptor.submitted.as_str(),
                descriptor.written.as_str(),
                descriptor.size,
                descriptor.sha256.as_str(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        named,
        vec![
            ("calendar_sent.jsonl", "calendar_kept.jsonl", 40, "aa"),
            ("calendar_held.jsonl", "calendar_held.jsonl", 7, "bb"),
        ]
    );
}

#[test]
fn an_unreadable_record_refuses_rather_than_reading_as_empty() {
    let dir = tempfile::tempdir().unwrap();
    let record = dir.path().join(RECORD_DIR);
    std::fs::create_dir_all(&record).unwrap();
    std::fs::write(record.join(BUILT_MARKER), b"1\n").unwrap();
    let text = format!("{}\n", line(0, json!({})));
    let key = identity_key(&parse_calendar_note(text.trim_end()).unwrap());
    std::fs::write(record.join(format!("{}.json", &key[..2])), b"not json").unwrap();
    let file = IncomingFile {
        submitted: calendar_file_name(text.as_bytes()),
        bytes: text.into_bytes(),
        descriptor_extra: Map::new(),
    };
    assert!(prepare(dir.path(), vec![file]).is_err());
}
