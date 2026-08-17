// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::observer_render_support::{NOW_MS, seed_full_fixture};
use solstone_core_observer::store::format::{TimeDisplay, render_list};
use solstone_core_observer::store::reload::load_observers;

const EXPECTED_LIST_HUMAN: &str = "\
Name                 Prefix             Status         Binding    Last Seen          Last Segment   Segments        Bytes
----------------------------------------------------------------------------------------------------------------------
revoked-never        cccccccc           revoked        unbound    never              —                     4       4.0 KB
unbound-stale        bbbbbbbb           disconnected   unbound    2026-01-01 02:55   —                     3       2.0 KB
bound-live           aaaaaaaa           connected      cert       2026-01-01 02:59   —                     2       1.0 KB
";

#[test]
fn list_human_table_matches_utc_fixture() {
    let root = tempfile::tempdir().expect("journal");
    seed_full_fixture(root.path());
    let output = render_list(
        &load_observers(root.path()).expect("records"),
        false,
        NOW_MS,
        TimeDisplay::Utc,
    );
    assert_eq!(
        format!("{output}\n"),
        EXPECTED_LIST_HUMAN,
        "list human table"
    );
}
