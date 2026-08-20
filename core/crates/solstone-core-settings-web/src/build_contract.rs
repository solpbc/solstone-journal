// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

mod source_manifest {
    include!(concat!(env!("OUT_DIR"), "/settings_sources.rs"));
}

/// AC 15 — no Python interpreter spawn. `retention_executor.rs` is the single
/// exemption: it launches the native Rust sibling whose absence is a load-bearing
/// refusal, so linking its library would make that safety property unreachable.
#[test]
fn ac15_settings_sources_allow_only_bounded_retention_executor_spawn() {
    let sources = [
        ("lib.rs", include_str!("lib.rs")),
        ("assets.rs", include_str!("assets.rs")),
        ("convey.rs", include_str!("convey.rs")),
        ("corpus.rs", include_str!("corpus.rs")),
        ("config.rs", include_str!("config.rs")),
        ("facets.rs", include_str!("facets.rs")),
        ("activities.rs", include_str!("activities.rs")),
        ("icons.rs", include_str!("icons.rs")),
        ("http.rs", include_str!("http.rs")),
        ("keys.rs", include_str!("keys.rs")),
        ("logs.rs", include_str!("logs.rs")),
        ("observe.rs", include_str!("observe.rs")),
        ("processing.rs", include_str!("processing.rs")),
        ("request_body.rs", include_str!("request_body.rs")),
        ("router_contracts.rs", include_str!("router_contracts.rs")),
        ("storage.rs", include_str!("storage.rs")),
        ("sol_voice.rs", include_str!("sol_voice.rs")),
        ("state.rs", include_str!("state.rs")),
        ("sync.rs", include_str!("sync.rs")),
        ("test_support.rs", include_str!("test_support.rs")),
        ("transcribe.rs", include_str!("transcribe.rs")),
        ("vision.rs", include_str!("vision.rs")),
        ("mutations.rs", include_str!("mutations.rs")),
        ("retention.rs", include_str!("retention.rs")),
        (
            "retention_executor.rs",
            include_str!("retention_executor.rs"),
        ),
        ("retention_tests.rs", include_str!("retention_tests.rs")),
        ("build_contract.rs", include_str!("build_contract.rs")),
    ];
    let mut declared = sources.iter().map(|(name, _)| *name).collect::<Vec<_>>();
    declared.sort();
    assert_eq!(
        declared,
        source_manifest::SOURCES,
        "every source must be explicitly scanned; build_contract is listed but excluded from substring scans because it contains the search terms"
    );
    for (name, source) in sources {
        if name == "build_contract.rs" {
            continue;
        }
        if name == "retention_executor.rs" {
            assert_eq!(source.matches("Command::new").count(), 1);
            assert_eq!(source.matches(".spawn(").count(), 1);
            assert_eq!(source.matches(".output(").count(), 0);
            assert_eq!(source.matches("tokio::process").count(), 0);
        } else {
            for forbidden in ["Command::new", ".spawn(", ".output(", "tokio::process"] {
                assert_eq!(
                    source.matches(forbidden).count(),
                    0,
                    "{name} contains forbidden launch API"
                );
            }
        }
    }
}
