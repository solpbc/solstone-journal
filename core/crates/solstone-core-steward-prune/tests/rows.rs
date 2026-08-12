// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_steward_prune::{RowSplitter, Terminator};

#[test]
fn splitter_preserves_all_physical_terminators() {
    let rows = RowSplitter::new(b"one\r\ntwo\rthree\nfour").collect::<Vec<_>>();
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0].content, b"one");
    assert_eq!(rows[0].terminator, Terminator::Crlf);
    assert_eq!(rows[1].content, b"two");
    assert_eq!(rows[1].terminator, Terminator::Cr);
    assert_eq!(rows[2].content, b"three");
    assert_eq!(rows[2].terminator, Terminator::Lf);
    assert_eq!(rows[3].content, b"four");
    assert_eq!(rows[3].terminator, Terminator::None);
}

#[test]
fn empty_input_and_final_terminator_do_not_invent_rows() {
    assert!(RowSplitter::new(b"").next().is_none());
    let rows = RowSplitter::new(b"\r\n").collect::<Vec<_>>();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].content, b"");
    assert_eq!(rows[0].terminator, Terminator::Crlf);
}
