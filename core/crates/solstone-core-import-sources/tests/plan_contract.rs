// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

mod support;

use serde_json::json;
use solstone_core_import_sources::{SkipLocator, SkipReason, chatgpt, gemini, kindle};
use support::{TempTree, write_zip};

#[test]
fn gemini_rich_takeout_pins_prompt_response_html_and_skip_branches() {
    let tree = TempTree::new();
    let plan = gemini::plan(&support::gemini_archive(&tree)).unwrap();
    assert_eq!(plan.item_count, 4);
    assert!(
        plan.segments[0]
            .entries
            .iter()
            .any(|entry| entry.text == "response only &\u{a0}!?")
    );
    assert!(
        plan.segments[0]
            .entries
            .iter()
            .any(|entry| entry.text == "name fallback")
    );
    assert_eq!(plan.skipped.len(), 4);
    assert!(
        plan.skipped
            .contains(&solstone_core_import_sources::SkippedEntry {
                locator: SkipLocator::Activity { activity_index: 5 },
                reason: SkipReason::MissingActivityTimestamp,
            })
    );
    assert!(
        plan.skipped
            .contains(&solstone_core_import_sources::SkippedEntry {
                locator: SkipLocator::Activity { activity_index: 6 },
                reason: SkipReason::InvalidActivityTimestamp,
            })
    );
}

#[test]
fn malformed_entry_is_reported_with_its_locator_and_clean_fixture_is_quiet() {
    let tree = TempTree::new();
    let path = tree.path().join("malformed.zip");
    let conversations = json!([{
        "mapping": {
            "root": {"message": {"author": {"role": "user"}, "content": {"parts": ["ok"]}, "create_time": 1773230400.0}, "parent": null},
            "leaf": {"message": {"author": {"role": "assistant"}, "content": {"parts": ["bad"]}}, "parent": "root"}
        },
        "current_node": "leaf"
    }]);
    write_zip(
        &path,
        &[(
            "conversations.json".to_owned(),
            conversations.to_string().into_bytes(),
        )],
    );
    let malformed = chatgpt::plan(&path).unwrap();
    assert!(
        malformed
            .skipped
            .contains(&solstone_core_import_sources::SkippedEntry {
                locator: SkipLocator::Conversation {
                    conversation_index: 0,
                    message_index: Some(1)
                },
                reason: SkipReason::InvalidMessageTimestamp,
            })
    );
    assert!(
        chatgpt::plan(&support::chatgpt_archive(&tree))
            .unwrap()
            .skipped
            .is_empty()
    );
    assert!(
        kindle::plan(&support::kindle_clippings(&tree))
            .unwrap()
            .skipped
            .is_empty()
    );
}

#[test]
fn affected_days_are_explicit_sorted_and_deduplicated() {
    let tree = TempTree::new();
    let path = tree.path().join("multiple-days.zip");
    let activities = json!([
        {"time": "2026-03-11T12:00:00Z", "subtitles": [{"value": "first"}]},
        {"time": "2026-03-12T12:00:00Z", "subtitles": [{"value": "second"}]},
        {"time": "2026-03-11T12:01:00Z", "subtitles": [{"value": "third"}]}
    ]);
    write_zip(
        &path,
        &[(
            "Takeout/My Activity/Gemini Apps/MyActivity.json".to_owned(),
            activities.to_string().into_bytes(),
        )],
    );
    let plan = gemini::plan(&path).unwrap();
    assert_eq!(plan.affected_days, vec!["20260311", "20260312"]);
    assert!(plan.affected_days.windows(2).all(|days| days[0] < days[1]));
}

#[test]
fn empty_kindle_preview_names_the_atomic_clipping_unit() {
    let tree = TempTree::new();
    let path = tree.file("empty-clippings.txt", b"Book\nmetadata\n==========\n");
    let preview = kindle::preview(&path).unwrap();
    assert_eq!(preview.item_count, 0);
    assert_eq!(preview.summary, "0 highlights from 0 books");
}
