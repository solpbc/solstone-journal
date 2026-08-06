// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

const LIB: &str = include_str!("../src/lib.rs");

#[test]
fn indexer_does_not_reexport_format() {
    assert!(
        !LIB.contains("pub use solstone_core_format"),
        "indexer must not provide a facade for solstone-core-format"
    );
}
