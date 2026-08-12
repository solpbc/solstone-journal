// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

mod support;

use serde_json::json;
use solstone_core_import_sources::{chatgpt, claude};
use support::{TempTree, write_zip};

#[test]
fn conversation_entries_keep_roles_timestamps_and_utc_windows() {
    let tree = TempTree::new();
    let claude_plan = claude::plan(&support::claude_archive(&tree)).unwrap();
    let chatgpt_plan = chatgpt::plan(&support::chatgpt_archive(&tree)).unwrap();
    for plan in [&claude_plan, &chatgpt_plan] {
        assert_eq!(
            plan.date_range,
            ("20260311".to_owned(), "20260311".to_owned())
        );
        assert_eq!(plan.segments[0].day, "20260311");
        assert_eq!(plan.segments[0].segment_key, "120000_300");
        assert_eq!(plan.segments[0].entries[0].speaker, "Human");
        assert_eq!(plan.segments[0].entries[1].speaker, "Assistant");
        assert_eq!(plan.segments[0].entries[0].start, "00:00:00");
        assert_eq!(plan.segments[0].entries[1].start, "00:01:00");
    }
}

#[test]
fn explicit_offsets_are_normalized_to_utc_before_day_and_window_derivation() {
    let tree = TempTree::new();
    let path = tree.path().join("offset.zip");
    write_zip(
        &path,
        &[(
            "conversations.json".to_owned(),
            json!([{
                "chat_messages": [{
                    "sender": "human",
                    "text": "offset message",
                    "created_at": "2026-03-11T23:30:00-08:00"
                }]
            }])
            .to_string()
            .into_bytes(),
        )],
    );

    let plan = claude::plan(&path).unwrap();
    assert_eq!(
        plan.date_range,
        ("20260312".to_owned(), "20260312".to_owned())
    );
    assert_eq!(plan.segments[0].day, "20260312");
    assert_eq!(plan.segments[0].segment_key, "073000_300");
}
