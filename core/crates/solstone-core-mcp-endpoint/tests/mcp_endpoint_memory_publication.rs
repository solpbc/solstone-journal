// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use chrono::{TimeZone, Utc};
use solstone_core_journal_io::{BoundPublicationPrimitive, run_with_bound_publication_fault};

#[test]
fn parent_sync_fault_cannot_return_a_stored_result() {
    let journal = tempfile::Builder::new()
        .prefix("solstone-agent-memory-")
        .tempdir()
        .expect("journal fixture");
    let now = Utc.with_ymd_and_hms(2026, 1, 2, 12, 0, 0).unwrap();
    let (result, fault_consumed) =
        run_with_bound_publication_fault(BoundPublicationPrimitive::ParentSync, 1, 5, || {
            solstone_core_mcp_endpoint::append_connection_memory_test_hook(
                journal.path(),
                "bearer:publication-test",
                "private note",
                "operation:1",
                now,
            )
        });

    assert!(fault_consumed);
    assert_eq!(result, Ok(false));
}
