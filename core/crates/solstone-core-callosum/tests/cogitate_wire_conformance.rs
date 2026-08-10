// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;

use serde_json::Value;
use solstone_core_cogitate_wire::native_producible_kinds;

const REGISTRY: &str = include_str!("../../../fixtures/callosum_registry.json");
const WIRE_CONTRACT: &str = include_str!("../../../fixtures/cogitate_wire_contract.json");

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
    for kind in ["warning", "progress", "info", "start"] {
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
