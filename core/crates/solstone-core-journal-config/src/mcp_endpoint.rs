// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Journal-local MCP endpoint capability.

use serde_json::Value;

use crate::JournalConfigRead;

/// Whether the journal-local MCP endpoint is enabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpEndpointCapability {
    Disabled,
    Enabled,
}

/// Invalid explicit `mcp_endpoint.enabled` configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpEndpointCapabilityError {
    EnabledMustBeBoolean,
}

/// The ACME directory selected for the journal-local MCP certificate.
///
/// This setting never enables the endpoint. Missing configuration deliberately
/// selects the non-production directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpEndpointCertificateEnvironment {
    Staging,
    Production,
}

/// Invalid explicit `mcp_endpoint.certificate_environment` configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpEndpointCertificateEnvironmentError {
    CertificateEnvironmentMustBeStagingOrProduction,
}

/// Invalid explicit `mcp_endpoint.force_staging_renewal` configuration.
///
/// The switch exists solely to prove the offline-recovery path against the
/// ACME staging directory. Production issuance is deliberately unavailable
/// through this probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpEndpointForceStagingRenewalError {
    ForceStagingRenewalMustBeBoolean,
    ForceStagingRenewalRequiresStaging,
}

/// Loopback port reserved for the journal-local MCP endpoint.
pub const MCP_ENDPOINT_LOOPBACK_PORT: u16 = 7658;

/// Return the MCP endpoint capability from an already-loaded journal config.
pub fn mcp_endpoint_capability(
    read: &JournalConfigRead,
) -> Result<McpEndpointCapability, McpEndpointCapabilityError> {
    let Some(config) = read.config.as_ref() else {
        return Ok(McpEndpointCapability::Disabled);
    };
    let Some(endpoint) = config.get("mcp_endpoint") else {
        return Ok(McpEndpointCapability::Disabled);
    };
    let Some(endpoint) = endpoint.as_object() else {
        return Err(McpEndpointCapabilityError::EnabledMustBeBoolean);
    };
    match endpoint.get("enabled") {
        None | Some(Value::Bool(false)) => Ok(McpEndpointCapability::Disabled),
        Some(Value::Bool(true)) => Ok(McpEndpointCapability::Enabled),
        Some(_) => Err(McpEndpointCapabilityError::EnabledMustBeBoolean),
    }
}

/// Return the certificate environment without changing the capability gate.
///
/// Only the exact lowercase literal `production` can select the production
/// directory. An absent endpoint object or key is always staging.
pub fn mcp_endpoint_certificate_environment(
    read: &JournalConfigRead,
) -> Result<McpEndpointCertificateEnvironment, McpEndpointCertificateEnvironmentError> {
    let Some(config) = read.config.as_ref() else {
        return Ok(McpEndpointCertificateEnvironment::Staging);
    };
    let Some(endpoint) = config.get("mcp_endpoint") else {
        return Ok(McpEndpointCertificateEnvironment::Staging);
    };
    let Some(endpoint) = endpoint.as_object() else {
        return Err(
            McpEndpointCertificateEnvironmentError::CertificateEnvironmentMustBeStagingOrProduction,
        );
    };
    match endpoint.get("certificate_environment") {
        None => Ok(McpEndpointCertificateEnvironment::Staging),
        Some(Value::String(value)) if value == "staging" => {
            Ok(McpEndpointCertificateEnvironment::Staging)
        }
        Some(Value::String(value)) if value == "production" => {
            Ok(McpEndpointCertificateEnvironment::Production)
        }
        Some(_) => Err(
            McpEndpointCertificateEnvironmentError::CertificateEnvironmentMustBeStagingOrProduction,
        ),
    }
}

/// Return whether this staging-only process start must reissue its certificate.
///
/// The omitted setting is inert. A true value is rejected unless the same
/// configuration resolves to ACME staging, so a diagnostic restart cannot
/// spend a production issuance.
pub fn mcp_endpoint_force_staging_renewal(
    read: &JournalConfigRead,
) -> Result<bool, McpEndpointForceStagingRenewalError> {
    let Some(config) = read.config.as_ref() else {
        return Ok(false);
    };
    let Some(endpoint) = config.get("mcp_endpoint") else {
        return Ok(false);
    };
    let Some(endpoint) = endpoint.as_object() else {
        return Err(McpEndpointForceStagingRenewalError::ForceStagingRenewalMustBeBoolean);
    };
    let force = match endpoint.get("force_staging_renewal") {
        None | Some(Value::Bool(false)) => return Ok(false),
        Some(Value::Bool(true)) => true,
        Some(_) => {
            return Err(McpEndpointForceStagingRenewalError::ForceStagingRenewalMustBeBoolean);
        }
    };
    if force
        && !matches!(
            mcp_endpoint_certificate_environment(read),
            Ok(McpEndpointCertificateEnvironment::Staging)
        )
    {
        return Err(McpEndpointForceStagingRenewalError::ForceStagingRenewalRequiresStaging);
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use serde_json::{Map, Value, json};

    use super::*;

    fn read(config: Option<Map<String, Value>>) -> JournalConfigRead {
        JournalConfigRead {
            present: config.is_some(),
            sha256: None,
            config,
        }
    }

    fn config_with_endpoint(endpoint: Value) -> Map<String, Value> {
        let mut config = Map::new();
        config.insert("mcp_endpoint".to_owned(), endpoint);
        config
    }

    #[test]
    fn missing_config_is_disabled() {
        assert_eq!(
            mcp_endpoint_capability(&JournalConfigRead {
                present: false,
                sha256: None,
                config: None,
            }),
            Ok(McpEndpointCapability::Disabled)
        );
    }

    #[test]
    fn empty_config_is_disabled() {
        assert_eq!(
            mcp_endpoint_capability(&read(Some(Map::new()))),
            Ok(McpEndpointCapability::Disabled)
        );
    }

    #[test]
    fn endpoint_without_enabled_is_disabled() {
        assert_eq!(
            mcp_endpoint_capability(&read(Some(config_with_endpoint(json!({}))))),
            Ok(McpEndpointCapability::Disabled)
        );
    }

    #[test]
    fn false_enabled_is_disabled() {
        assert_eq!(
            mcp_endpoint_capability(&read(Some(config_with_endpoint(json!({"enabled": false}))))),
            Ok(McpEndpointCapability::Disabled)
        );
    }

    #[test]
    fn true_enabled_is_enabled() {
        assert_eq!(
            mcp_endpoint_capability(&read(Some(config_with_endpoint(json!({"enabled": true}))))),
            Ok(McpEndpointCapability::Enabled)
        );
    }

    #[test]
    fn invalid_enabled_values_fail_closed() {
        for value in [
            json!(null),
            json!("x"),
            json!(1),
            json!([1]),
            json!({"a": 1}),
        ] {
            assert_eq!(
                mcp_endpoint_capability(&read(Some(config_with_endpoint(json!({
                    "enabled": value
                }))))),
                Err(McpEndpointCapabilityError::EnabledMustBeBoolean)
            );
        }
    }

    #[test]
    fn non_object_endpoint_fails_closed() {
        assert_eq!(
            mcp_endpoint_capability(&read(Some(config_with_endpoint(json!("not-an-object"))))),
            Err(McpEndpointCapabilityError::EnabledMustBeBoolean)
        );
    }

    #[test]
    fn unrelated_sibling_keys_are_inert() {
        let config = json!({
            "mcp_endpoint": {"enabled": true, "unrelated": 1},
            "other_top_level_key": "x",
        });
        assert_eq!(
            mcp_endpoint_capability(&read(config.as_object().cloned())),
            Ok(McpEndpointCapability::Enabled)
        );
    }

    #[test]
    fn certificate_environment_missing_is_staging_without_changing_the_gate() {
        for config in [
            None,
            Some(Map::new()),
            Some(config_with_endpoint(json!({}))),
            Some(config_with_endpoint(json!({"enabled": false}))),
            Some(config_with_endpoint(json!({"enabled": true}))),
        ] {
            let read = read(config);
            assert_eq!(
                mcp_endpoint_certificate_environment(&read),
                Ok(McpEndpointCertificateEnvironment::Staging)
            );
        }
    }

    #[test]
    fn certificate_environment_requires_one_exact_literal() {
        for (value, expected) in [
            (
                json!("staging"),
                Ok(McpEndpointCertificateEnvironment::Staging),
            ),
            (
                json!("production"),
                Ok(McpEndpointCertificateEnvironment::Production),
            ),
        ] {
            assert_eq!(
                mcp_endpoint_certificate_environment(&read(Some(config_with_endpoint(json!({
                    "certificate_environment": value,
                }))))),
                expected
            );
        }
        for value in [
            json!(null),
            json!(false),
            json!(0),
            json!([]),
            json!({}),
            json!("Staging"),
            json!("production "),
            json!(" prod"),
            json!("test"),
        ] {
            assert_eq!(
                mcp_endpoint_certificate_environment(&read(Some(config_with_endpoint(json!({
                    "certificate_environment": value,
                }))))),
                Err(
                    McpEndpointCertificateEnvironmentError::CertificateEnvironmentMustBeStagingOrProduction
                )
            );
        }
    }

    #[test]
    fn malformed_endpoint_is_rejected_by_both_independent_parsers() {
        let read = read(Some(config_with_endpoint(json!(false))));
        assert_eq!(
            mcp_endpoint_capability(&read),
            Err(McpEndpointCapabilityError::EnabledMustBeBoolean)
        );
        assert_eq!(
            mcp_endpoint_certificate_environment(&read),
            Err(
                McpEndpointCertificateEnvironmentError::CertificateEnvironmentMustBeStagingOrProduction
            )
        );
    }

    #[test]
    fn forced_staging_renewal_is_default_false_and_staging_only() {
        for config in [
            None,
            Some(Map::new()),
            Some(config_with_endpoint(json!({}))),
            Some(config_with_endpoint(
                json!({"force_staging_renewal": false}),
            )),
        ] {
            assert_eq!(mcp_endpoint_force_staging_renewal(&read(config)), Ok(false));
        }

        assert_eq!(
            mcp_endpoint_force_staging_renewal(&read(Some(config_with_endpoint(json!({
                "certificate_environment": "staging",
                "force_staging_renewal": true,
            }))))),
            Ok(true)
        );
        assert_eq!(
            mcp_endpoint_force_staging_renewal(&read(Some(config_with_endpoint(json!({
                "certificate_environment": "production",
                "force_staging_renewal": true,
            }))))),
            Err(McpEndpointForceStagingRenewalError::ForceStagingRenewalRequiresStaging)
        );
        for value in [json!(null), json!("yes"), json!(1), json!([]), json!({})] {
            assert_eq!(
                mcp_endpoint_force_staging_renewal(&read(Some(config_with_endpoint(json!({
                    "force_staging_renewal": value,
                }))))),
                Err(McpEndpointForceStagingRenewalError::ForceStagingRenewalMustBeBoolean)
            );
        }
    }
}
