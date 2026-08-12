// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

mod support;

use solstone_core_import_sources::{chatgpt, claude};
use support::TempTree;

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
