// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

const MATCHER: &str = include_str!("matcher.rs");
const CONTENT: &str = include_str!("content/mod.rs");
const EDGE_REGISTRY: &str = include_str!("../../solstone-core-indexer/src/edges/registry.rs");

#[test]
fn pattern_resolution_has_one_shared_root_and_cache() {
    let sources = [MATCHER, CONTENT, EDGE_REGISTRY];
    assert_eq!(
        sources
            .iter()
            .map(|source| source.matches("enum PatternRoot").count())
            .sum::<usize>(),
        1,
        "pattern resolution must have one root enum"
    );
    assert_eq!(
        sources
            .iter()
            .map(|source| source.matches("struct Resolver").count())
            .sum::<usize>(),
        1,
        "pattern resolution must have one generic resolver"
    );
    assert!(
        !EDGE_REGISTRY.contains("OnceLock"),
        "edge registry must use the shared resolver cache"
    );
    assert!(
        EDGE_REGISTRY.contains("solstone_core_format::matcher"),
        "edge registry must use the shared matcher"
    );
}
