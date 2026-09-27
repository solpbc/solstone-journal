// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

mod support;

use chrono::{FixedOffset, Utc};
use serde_json::json;
use solstone_core_import_sources::{chatgpt, claude};
use support::{TempTree, write_zip};

#[test]
fn conversation_entries_keep_roles_timestamps_and_five_minute_windows() {
    let tree = TempTree::new();
    let claude_plan = claude::plan(&support::claude_archive(&tree), &Utc).unwrap();
    let chatgpt_plan = chatgpt::plan(&support::chatgpt_archive(&tree), &Utc).unwrap();
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
fn a_message_lands_on_the_day_and_minute_of_the_zone_it_is_planned_in() {
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

    // Late evening in the owner's zone is the next morning in UTC. The journal is kept in
    // the owner's local time, so the owner's zone decides the day.
    let pacific = FixedOffset::west_opt(8 * 3600).unwrap();
    let local = claude::plan(&path, &pacific).unwrap();
    assert_eq!(
        local.date_range,
        ("20260311".to_owned(), "20260311".to_owned())
    );
    assert_eq!(local.segments[0].day, "20260311");
    assert_eq!(local.segments[0].segment_key, "233000_300");

    let utc = claude::plan(&path, &Utc).unwrap();
    assert_eq!(utc.segments[0].day, "20260312");
    assert_eq!(utc.segments[0].segment_key, "073000_300");
}
