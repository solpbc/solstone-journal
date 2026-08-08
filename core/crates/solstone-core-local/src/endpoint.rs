// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Canonical resolution of the configured local inference endpoint.

use serde_json::{Map, Value};

const DEFAULT_BYO_PARALLEL_SLOTS: u32 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalEndpointResolution {
    Bundled,
    Byo(ByoEndpoint),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ByoEndpoint {
    pub base_url: String,
    pub served_model_id: String,
    pub credential: Option<String>,
    pub parallel_slots: Option<u32>,
    pub is_confidential: bool,
}

pub fn resolve_local_endpoint(config: &Map<String, Value>) -> LocalEndpointResolution {
    let local = config
        .get("providers")
        .and_then(Value::as_object)
        .and_then(|providers| providers.get("local"))
        .and_then(Value::as_object);
    let endpoint_url = local
        .and_then(|local| local.get("endpoint_url"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let served_model_id = local
        .and_then(|local| local.get("served_model_id"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if endpoint_url.is_empty() || served_model_id.is_empty() {
        return LocalEndpointResolution::Bundled;
    }

    let is_confidential = config
        .get("services")
        .and_then(Value::as_object)
        .and_then(|services| services.get("confidential"))
        .is_some_and(Value::is_object);
    let parallel_slots = if is_confidential {
        None
    } else {
        Some(configured_byo_parallel_slots(local))
    };
    LocalEndpointResolution::Byo(ByoEndpoint {
        base_url: normalize_endpoint_url(endpoint_url),
        served_model_id: served_model_id.to_owned(),
        credential: local
            .and_then(|local| local.get("credential"))
            .and_then(Value::as_str)
            .filter(|credential| !credential.is_empty())
            .map(ToOwned::to_owned),
        parallel_slots,
        is_confidential,
    })
}

fn configured_byo_parallel_slots(local: Option<&Map<String, Value>>) -> u32 {
    let Some(value) = local.and_then(|local| local.get("parallel_slots")) else {
        return DEFAULT_BYO_PARALLEL_SLOTS;
    };
    let slots = value
        .as_u64()
        .and_then(|slots| u32::try_from(slots).ok())
        .filter(|slots| *slots >= 1);
    match slots {
        Some(slots) => slots,
        None => {
            eprintln!(
                "invalid providers.local.parallel_slots ({value}); defaulting to {DEFAULT_BYO_PARALLEL_SLOTS}"
            );
            DEFAULT_BYO_PARALLEL_SLOTS
        }
    }
}

fn normalize_endpoint_url(value: &str) -> String {
    let trimmed = value.trim_end_matches('/');
    trimmed
        .strip_suffix("/v1")
        .unwrap_or(trimmed)
        .trim_end_matches('/')
        .to_owned()
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use sha2::{Digest, Sha256};
    use solstone_core_brain::derive_active_brain_lane;

    use super::*;

    fn config(value: Value) -> Map<String, Value> {
        value.as_object().unwrap().clone()
    }

    fn local_config(endpoint_url: Value, served_model_id: Value) -> Value {
        json!({
            "providers": {
                "active": {"provider": "local"},
                "local": {
                    "endpoint_url": endpoint_url,
                    "served_model_id": served_model_id,
                },
            },
        })
    }

    #[test]
    fn incomplete_endpoint_configuration_resolves_to_bundled() {
        for (endpoint_url, served_model_id) in [
            (json!(""), json!("")),
            (json!("   "), json!("served")),
            (json!("https://endpoint"), json!("\t")),
        ] {
            let mut value = local_config(endpoint_url, served_model_id);
            value["services"] = json!({"confidential": {}});
            assert_eq!(
                resolve_local_endpoint(&config(value)),
                LocalEndpointResolution::Bundled
            );
        }
    }

    #[test]
    fn byo_parallel_slots_default_unless_confidential() {
        for value in [None, Some(json!(0)), Some(json!(true)), Some(json!("2"))] {
            let mut config_value = local_config(json!("https://endpoint"), json!("served"));
            if let Some(value) = value {
                config_value["providers"]["local"]["parallel_slots"] = value;
            }
            let LocalEndpointResolution::Byo(endpoint) =
                resolve_local_endpoint(&config(config_value))
            else {
                panic!("complete endpoint configuration must resolve to BYO");
            };
            assert!(!endpoint.is_confidential);
            assert_eq!(endpoint.parallel_slots, Some(DEFAULT_BYO_PARALLEL_SLOTS));
        }

        let mut confidential = local_config(json!("https://endpoint"), json!("served"));
        confidential["providers"]["local"]["parallel_slots"] = json!(0);
        confidential["services"] = json!({"confidential": {}});
        let LocalEndpointResolution::Byo(endpoint) = resolve_local_endpoint(&config(confidential))
        else {
            panic!("complete endpoint configuration must resolve to BYO");
        };
        assert!(endpoint.is_confidential);
        assert_eq!(endpoint.parallel_slots, None);
    }

    #[test]
    fn canonical_resolution_and_brain_lane_mapping_are_explicit() {
        #[derive(Debug, Clone, Copy)]
        enum ExpectedResolution {
            Bundled,
            Byo,
            Confidential,
        }

        let credential_fingerprint = format!("{:x}", Sha256::digest(b"credential"));
        let cases = vec![
            (
                "both empty",
                local_config(json!(""), json!("")),
                ExpectedResolution::Bundled,
                Some("bundled"),
            ),
            (
                "both set",
                local_config(json!("https://endpoint"), json!("served")),
                ExpectedResolution::Byo,
                Some("byo-endpoint"),
            ),
            (
                "whitespace only",
                local_config(json!(" \t "), json!("\n")),
                ExpectedResolution::Bundled,
                Some("bundled"),
            ),
            (
                "trailing slash",
                local_config(json!("https://endpoint/v1/"), json!("served")),
                ExpectedResolution::Byo,
                Some("byo-endpoint"),
            ),
            (
                "endpoint set and served model empty",
                local_config(json!("https://endpoint"), json!("")),
                ExpectedResolution::Bundled,
                None,
            ),
            (
                "endpoint empty and served model set",
                local_config(json!(""), json!("served")),
                ExpectedResolution::Bundled,
                None,
            ),
            (
                "matching confidential provenance",
                json!({
                    "providers": {
                        "active": {"provider": "local"},
                        "local": {
                            "endpoint_url": "https://endpoint/v1/",
                            "served_model_id": "served",
                            "credential": "credential",
                        },
                    },
                    "services": {"confidential": {
                        "endpoint_url": "https://endpoint",
                        "served_model_id": "served",
                        "credential_fingerprint_sha256": credential_fingerprint,
                    }},
                }),
                ExpectedResolution::Confidential,
                Some("spp"),
            ),
            (
                "mismatched confidential provenance",
                json!({
                    "providers": {
                        "active": {"provider": "local"},
                        "local": {
                            "endpoint_url": "https://endpoint",
                            "served_model_id": "served",
                            "credential": "credential",
                        },
                    },
                    "services": {"confidential": {
                        "endpoint_url": "https://other-endpoint",
                        "served_model_id": "served",
                        "credential_fingerprint_sha256": credential_fingerprint,
                    }},
                }),
                ExpectedResolution::Confidential,
                None,
            ),
            (
                "non-string endpoint URL",
                local_config(json!(42), json!("served")),
                ExpectedResolution::Bundled,
                None,
            ),
        ];

        for (name, value, expected_resolution, expected_brain_lane) in cases {
            let config = config(value);
            let resolution = resolve_local_endpoint(&config);
            match expected_resolution {
                ExpectedResolution::Bundled => {
                    assert_eq!(resolution, LocalEndpointResolution::Bundled, "{name}");
                }
                ExpectedResolution::Byo => match resolution {
                    LocalEndpointResolution::Byo(endpoint) => {
                        assert!(!endpoint.is_confidential, "{name}");
                        if name == "trailing slash" {
                            assert_eq!(endpoint.base_url, "https://endpoint", "{name}");
                        }
                    }
                    LocalEndpointResolution::Bundled => panic!("{name}: expected BYO"),
                },
                ExpectedResolution::Confidential => match resolution {
                    LocalEndpointResolution::Byo(endpoint) => {
                        assert!(endpoint.is_confidential, "{name}");
                    }
                    LocalEndpointResolution::Bundled => panic!("{name}: expected confidential BYO"),
                },
            }
            assert_eq!(
                derive_active_brain_lane(&config).lane.as_deref(),
                expected_brain_lane,
                "{name}"
            );
        }
    }
}
