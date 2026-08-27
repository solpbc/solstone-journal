// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

use regex::Regex;
use serde_json::Value;
use solstone_core_cogitate_wire::native_producible_kinds;

const REGISTRY: &str = include_str!("../../../fixtures/callosum_registry.json");
const WIRE_CONTRACT: &str = include_str!("../../../fixtures/cogitate_wire_contract.json");

/// Native supervisor emits this pair, but `core/fixtures/callosum_registry.json`
/// does not declare it.
///
/// Emit site: `core/crates/solstone-core/src/supervisor/tick.rs`,
/// `StatusEmissionPlan::Errors` → `emit(&state.server, "supervisor", "status-error", ...)`.
///
/// When the Python registry goes, re-derive this pair from the native emit site
/// and delete this constant.
const KNOWN_UNDECLARED_NATIVE_PAIRS: &[(&str, &str)] = &[("supervisor", "status-error")];

struct ProducedPair {
    tract: String,
    event: String,
    site: String,
}

#[test]
fn cogitate_wire_contract_covers_registry_and_actual_native_mapping() {
    let registry: Value = serde_json::from_str(REGISTRY).expect("registry fixture is valid JSON");
    let contract: Value = serde_json::from_str(WIRE_CONTRACT).expect("wire contract is valid JSON");
    let registry_kinds = strings(
        registry["registry"]["cortex"]
            .as_array()
            .expect("cortex registry is an array"),
    );
    let schema_kinds = contract["cortex_events"]
        .as_object()
        .expect("wire event contract is an object")
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    let native_kinds = native_producible_kinds()
        .iter()
        .map(|kind| (*kind).to_owned())
        .collect::<BTreeSet<_>>();

    assert!(native_kinds.is_subset(&registry_kinds));
    assert_eq!(schema_kinds, registry_kinds);
    for kind in ["progress", "info", "start"] {
        assert!(
            !native_kinds.contains(kind),
            "{kind} must not be claimed native-producible"
        );
    }
}

fn strings(values: &[Value]) -> BTreeSet<String> {
    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .expect("registry kind is a string")
                .to_owned()
        })
        .collect()
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("repository root")
        .to_path_buf()
}

fn declared_pairs() -> (BTreeSet<(String, String)>, BTreeSet<String>) {
    let fixture: Value = serde_json::from_str(REGISTRY).expect("valid registry fixture");
    let registry = fixture["registry"]
        .as_object()
        .expect("registry fixture has a registry object");
    let mut declared = BTreeSet::new();
    let mut wildcard_tracts = BTreeSet::new();
    for (tract, events) in registry {
        for event in events.as_array().expect("registry event list") {
            let event = event.as_str().expect("registry event string");
            if event == "*" {
                wildcard_tracts.insert(tract.clone());
            } else {
                declared.insert((tract.clone(), event.to_owned()));
            }
        }
    }
    (declared, wildcard_tracts)
}

fn source_site(path: &str, source: &str, byte_offset: usize) -> String {
    let line = source[..byte_offset]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        + 1;
    format!("{path}:{line}")
}

/// Literal text-pattern scan, not a Rust parser: it recognizes only
/// `emit(&..., "tract", "event", ...)` calls. Non-reference first arguments and
/// comment/string matches are not handled correctly. When adding a supervisor emit
/// site, ensure `native_producible_pairs_are_declared` passes; extend the regex
/// for new shapes.
fn rust_supervisor_pairs(repository_root: &std::path::Path) -> Vec<ProducedPair> {
    let emit_pattern = Regex::new(r#"(?s)\bemit\s*\(\s*&[^,]+,\s*"([^"]+)",\s*"([^"]+)",\s*"#)
        .expect("valid native supervisor emit regex");
    let mut produced = Vec::new();
    for relative_path in [
        "core/crates/solstone-core/src/supervisor/bus.rs",
        "core/crates/solstone-core/src/supervisor/tick.rs",
    ] {
        let source = fs::read_to_string(repository_root.join(relative_path))
            .expect("read native supervisor source");
        for captures in emit_pattern.captures_iter(&source) {
            let whole_match = captures.get(0).expect("emit match has full span");
            produced.push(ProducedPair {
                tract: captures[1].to_owned(),
                event: captures[2].to_owned(),
                site: source_site(relative_path, &source, whole_match.start()),
            });
        }
    }
    produced
}

fn rust_cortex_pairs() -> Vec<ProducedPair> {
    native_producible_kinds()
        .iter()
        .map(|event| ProducedPair {
            tract: "cortex".to_owned(),
            event: (*event).to_owned(),
            site: "core/crates/solstone-core-cogitate-wire/src/event.rs:61".to_owned(),
        })
        .collect()
}

fn merge_produced(
    produced: &mut BTreeMap<(String, String), BTreeSet<String>>,
    pairs: impl IntoIterator<Item = ProducedPair>,
) {
    for pair in pairs {
        produced
            .entry((pair.tract, pair.event))
            .or_default()
            .insert(pair.site);
    }
}

#[test]
fn native_producible_pairs_are_declared() {
    let (declared, wildcard_tracts) = declared_pairs();
    let mut produced = BTreeMap::new();
    merge_produced(&mut produced, rust_supervisor_pairs(&repository_root()));
    merge_produced(&mut produced, rust_cortex_pairs());

    let known_undeclared = KNOWN_UNDECLARED_NATIVE_PAIRS
        .iter()
        .map(|(tract, event)| ((*tract).to_owned(), (*event).to_owned()))
        .collect::<BTreeSet<_>>();

    for (tract, event) in KNOWN_UNDECLARED_NATIVE_PAIRS {
        let pair = ((*tract).to_owned(), (*event).to_owned());
        assert!(
            produced.contains_key(&pair),
            "KNOWN_UNDECLARED_NATIVE_PAIRS entry {tract}.{event} is no longer native-producible; delete this constant"
        );
        assert!(
            !wildcard_tracts.contains(*tract) && !declared.contains(&pair),
            "KNOWN_UNDECLARED_NATIVE_PAIRS entry {tract}.{event} is now declared in the fixture; delete this constant"
        );
    }

    let undeclared = produced
        .iter()
        .filter(|((tract, event), _sites)| {
            !known_undeclared.contains(&(tract.clone(), event.clone()))
                && !wildcard_tracts.contains(tract)
                && !declared.contains(&(tract.clone(), event.clone()))
        })
        .map(|((tract, event), sites)| {
            format!(
                "{tract}.{event} ({})",
                sites.iter().cloned().collect::<Vec<_>>().join(", ")
            )
        })
        .collect::<Vec<_>>();
    assert!(
        undeclared.is_empty(),
        "native-producible Callosum pairs are undeclared:\n{}",
        undeclared.join("\n"),
    );
}
