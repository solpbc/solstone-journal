// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::sync::OnceLock;

use serde_json::Value;

const REGISTRY_FIXTURE: &str = include_str!("../../../fixtures/callosum_registry.json");
static REGISTRY: OnceLock<Value> = OnceLock::new();

/// Return the generated Callosum vocabulary fixture for inspection only.
pub fn callosum_registry() -> &'static Value {
    REGISTRY.get_or_init(|| {
        serde_json::from_str(REGISTRY_FIXTURE).expect("callosum registry fixture is valid JSON")
    })
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::callosum_registry;

    #[test]
    fn registry_matches_independently_parsed_fixture() {
        let expected: Value =
            serde_json::from_str(include_str!("../../../fixtures/callosum_registry.json"))
                .expect("callosum registry fixture is valid JSON");

        assert_eq!(callosum_registry(), &expected);
    }
}
