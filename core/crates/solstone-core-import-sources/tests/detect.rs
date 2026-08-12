// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

mod support;

use serde_json::json;
use solstone_core_import_sources::{SourceError, chatgpt, claude, gemini, kindle};
use support::{TempTree, write_zip};

#[test]
fn chat_archives_are_claimed_by_exactly_one_source_in_both_directions() {
    let tree = TempTree::new();
    let claude_path = tree.path().join("claude.zip");
    write_zip(
        &claude_path,
        &[(
            "conversations.json".to_owned(),
            json!([{"chat_messages": []}]).to_string().into_bytes(),
        )],
    );
    assert!(claude::detect(&claude_path).unwrap());
    assert!(!chatgpt::detect(&claude_path).unwrap());

    let chatgpt_path = tree.path().join("chatgpt.zip");
    write_zip(
        &chatgpt_path,
        &[(
            "conversations.json".to_owned(),
            json!([{"mapping": {}}]).to_string().into_bytes(),
        )],
    );
    assert!(chatgpt::detect(&chatgpt_path).unwrap());
    assert!(!claude::detect(&chatgpt_path).unwrap());
}

#[test]
fn claude_claims_its_dms_extension_when_the_archive_shape_matches() {
    let tree = TempTree::new();
    let path = tree.path().join("claude.dms");
    write_zip(
        &path,
        &[(
            "conversations.json".to_owned(),
            json!([{"chat_messages": []}]).to_string().into_bytes(),
        )],
    );
    assert!(claude::detect(&path).unwrap());
}

#[test]
fn gemini_and_kindle_use_content_predicates() {
    let tree = TempTree::new();
    let gemini_path = support::gemini_archive(&tree);
    let kindle_path = support::kindle_clippings(&tree);
    let unrelated_zip = tree.path().join("unrelated.zip");
    write_zip(
        &unrelated_zip,
        &[("unrelated.json".to_owned(), b"[]".to_vec())],
    );
    let unrelated_text = tree.file("unrelated.txt", b"ordinary text");
    let spaced_kind = tree.file(
        "spaced-clippings.txt",
        b"A Book (An Author)\n- Your\tHighlight | Added on Wednesday, March 11, 2026 12:00:00 PM\n\nA highlight\n==========\n",
    );
    assert!(gemini::detect(&gemini_path).unwrap());
    assert!(kindle::detect(&kindle_path).unwrap());
    assert!(!gemini::detect(&unrelated_zip).unwrap());
    assert!(!kindle::detect(&unrelated_text).unwrap());
    assert!(kindle::detect(&spaced_kind).unwrap());
    assert!(!kindle::detect(&gemini_path).unwrap());
}

#[test]
fn all_source_detectors_report_missing_paths_as_io_errors() {
    let tree = TempTree::new();
    let path = tree.path().join("missing");
    for result in [
        claude::detect(&path),
        chatgpt::detect(&path),
        gemini::detect(&path),
        kindle::detect(&path),
    ] {
        assert!(matches!(result, Err(SourceError::Io { .. })));
    }
}
