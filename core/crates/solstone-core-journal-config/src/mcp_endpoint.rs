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
/// selects the production directory.
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

/// Loopback port reserved for the direct local agent door.
pub const MCP_LOCAL_DOOR_PORT: u16 = 7659;

/// Dedicated LAN port reserved for the direct local agent door.
pub const MCP_LAN_DOOR_PORT: u16 = 7660;

/// Loopback origin for the direct local agent door.
pub const MCP_LOCAL_DOOR_ORIGIN: &str = "http://127.0.0.1:7659";

/// Loopback MCP resource URI for the direct local agent door.
pub const MCP_LOCAL_DOOR_RESOURCE: &str = "http://127.0.0.1:7659/mcp";

/// URN resource identifier for LAN-door OAuth grants and pairing codes.
pub const MCP_LAN_DOOR_RESOURCE: &str = "urn:solstone:mcp-door:lan";

/// Configuration status of the direct local agent door.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalDoorConfig {
    On,
    Off,
    Invalid,
}

/// Return the local door configuration from an already-loaded journal config.
pub fn local_door_config(read: &JournalConfigRead) -> LocalDoorConfig {
    let Some(config) = read.config.as_ref() else {
        return LocalDoorConfig::On;
    };
    let Some(endpoint) = config.get("mcp_endpoint") else {
        return LocalDoorConfig::On;
    };
    let Some(endpoint) = endpoint.as_object() else {
        return LocalDoorConfig::Invalid;
    };
    match endpoint.get("local_door") {
        None | Some(Value::Bool(true)) => LocalDoorConfig::On,
        Some(Value::Bool(false)) => LocalDoorConfig::Off,
        Some(_) => LocalDoorConfig::Invalid,
    }
}

/// Return whether the local door is enabled (true only when `LocalDoorConfig::On`).
pub fn local_door_enabled(read: &JournalConfigRead) -> bool {
    matches!(local_door_config(read), LocalDoorConfig::On)
}

/// Return the LAN door configuration from an already-loaded journal config.
pub fn lan_door_config(read: &JournalConfigRead) -> LocalDoorConfig {
    let Some(config) = read.config.as_ref() else {
        return LocalDoorConfig::Off;
    };
    let Some(endpoint) = config.get("mcp_endpoint") else {
        return LocalDoorConfig::Off;
    };
    let Some(endpoint) = endpoint.as_object() else {
        return LocalDoorConfig::Invalid;
    };
    match endpoint.get("lan_door") {
        None | Some(Value::Bool(false)) => LocalDoorConfig::Off,
        Some(Value::Bool(true)) => LocalDoorConfig::On,
        Some(_) => LocalDoorConfig::Invalid,
    }
}

/// Return whether the LAN door is enabled (true only when `LocalDoorConfig::On`).
pub fn lan_door_enabled(read: &JournalConfigRead) -> bool {
    matches!(lan_door_config(read), LocalDoorConfig::On)
}

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
/// Missing configuration or an absent key deliberately selects the production
/// directory. Only the exact lowercase literal `staging` selects staging; only
/// exact `production` (or absence) selects production; anything else fails closed.
pub fn mcp_endpoint_certificate_environment(
    read: &JournalConfigRead,
) -> Result<McpEndpointCertificateEnvironment, McpEndpointCertificateEnvironmentError> {
    let Some(config) = read.config.as_ref() else {
        return Ok(McpEndpointCertificateEnvironment::Production);
    };
    let Some(endpoint) = config.get("mcp_endpoint") else {
        return Ok(McpEndpointCertificateEnvironment::Production);
    };
    let Some(endpoint) = endpoint.as_object() else {
        return Err(
            McpEndpointCertificateEnvironmentError::CertificateEnvironmentMustBeStagingOrProduction,
        );
    };
    match endpoint.get("certificate_environment") {
        None => Ok(McpEndpointCertificateEnvironment::Production),
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

/// Stored BYO hostname configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ByoHostnameConfig {
    pub hostname: Option<String>,
    pub enabled: bool,
    pub generation: u64,
}

/// Tri-state configuration status for BYO hostname.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ByoHostnameConfigStatus {
    None,
    Configured(ByoHostnameConfig),
    Invalid,
}

/// Rejection reasons for BYO hostname canonicalization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ByoHostnameError {
    Empty,
    TooLong,
    NonAscii,
    ForbiddenCharacter,
    IpLiteral,
    LocalHostOrSuffix,
    SolstoneSuffix,
    SingleLabel,
    EmptyLabel,
    LabelTooLong,
    LabelHyphenBoundary,
    LabelPunycode,
}

impl std::fmt::Display for ByoHostnameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "hostname cannot be empty"),
            Self::TooLong => write!(f, "hostname exceeds maximum length of 253 bytes"),
            Self::NonAscii => write!(f, "hostname must contain only ASCII characters"),
            Self::ForbiddenCharacter => write!(f, "hostname contains forbidden characters"),
            Self::IpLiteral => write!(f, "IP literals are not permitted as hostnames"),
            Self::LocalHostOrSuffix => {
                write!(f, "localhost and local domain names are not permitted")
            }
            Self::SolstoneSuffix => write!(f, "solstone domains are reserved"),
            Self::SingleLabel => write!(f, "hostname must have at least two labels"),
            Self::EmptyLabel => write!(f, "hostname cannot contain empty labels"),
            Self::LabelTooLong => write!(f, "hostname label exceeds 63 characters"),
            Self::LabelHyphenBoundary => {
                write!(f, "hostname label cannot start or end with a hyphen")
            }
            Self::LabelPunycode => write!(f, "punycode labels (xn--) are not permitted"),
        }
    }
}

impl std::error::Error for ByoHostnameError {}

/// Errors during BYO hostname configuration transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ByoHostnameTransitionError {
    InvalidHostname(ByoHostnameError),
    EnableSeparately,
    NoStoredHostnameToEnable,
    NoStoredConfigToRemove,
}

impl std::fmt::Display for ByoHostnameTransitionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidHostname(e) => write!(f, "{e}"),
            Self::EnableSeparately => write!(f, "enable cannot be true during initial set"),
            Self::NoStoredHostnameToEnable => write!(f, "no stored hostname to enable"),
            Self::NoStoredConfigToRemove => write!(f, "no stored configuration to remove"),
        }
    }
}

impl std::error::Error for ByoHostnameTransitionError {}

/// Operations on the BYO hostname configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ByoHostnameOp<'a> {
    SetHostname { hostname: &'a str, enabled: bool },
    SetEnabled { enabled: bool },
    RemoveHostname,
}

/// Canonicalize an owner-supplied BYO hostname.
pub fn canonicalize_byo_hostname(raw: &str) -> Result<String, ByoHostnameError> {
    if raw.is_empty() {
        return Err(ByoHostnameError::Empty);
    }
    if raw.len() > 253 {
        return Err(ByoHostnameError::TooLong);
    }
    if !raw.is_ascii() {
        return Err(ByoHostnameError::NonAscii);
    }
    if raw.bytes().any(|b| {
        b <= 0x20
            || b == 0x7f
            || matches!(
                b,
                b':' | b'/' | b'?' | b'#' | b'@' | b'*' | b'%' | b'\\' | b'[' | b']'
            )
    }) {
        return Err(ByoHostnameError::ForbiddenCharacter);
    }
    let lower = raw.to_ascii_lowercase();

    // Reject dotted quads / IP literals
    let parts: Vec<&str> = lower.split('.').collect();
    if parts.len() == 4
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
    {
        return Err(ByoHostnameError::IpLiteral);
    }
    if lower.contains(':') {
        return Err(ByoHostnameError::IpLiteral);
    }

    // Reject localhost and local/internal suffixes
    if lower == "localhost"
        || lower.ends_with(".localhost")
        || lower.ends_with(".local")
        || lower.ends_with(".localdomain")
        || lower.ends_with(".internal")
        || lower.ends_with(".arpa")
    {
        return Err(ByoHostnameError::LocalHostOrSuffix);
    }

    // Reject solstone service domains and descendants
    if lower == "solstone.me"
        || lower.ends_with(".solstone.me")
        || lower == "solstone.app"
        || lower.ends_with(".solstone.app")
        || lower == "solpbc.org"
        || lower.ends_with(".solpbc.org")
    {
        return Err(ByoHostnameError::SolstoneSuffix);
    }

    if parts.len() < 2 {
        return Err(ByoHostnameError::SingleLabel);
    }

    for label in parts {
        if label.is_empty() {
            return Err(ByoHostnameError::EmptyLabel);
        }
        if label.len() > 63 {
            return Err(ByoHostnameError::LabelTooLong);
        }
        if label.starts_with('-') || label.ends_with('-') {
            return Err(ByoHostnameError::LabelHyphenBoundary);
        }
        if label.starts_with("xn--") {
            return Err(ByoHostnameError::LabelPunycode);
        }
        if !label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return Err(ByoHostnameError::ForbiddenCharacter);
        }
    }

    Ok(lower)
}

/// Read the BYO hostname configuration status from a journal config map.
pub fn byo_hostname_config_from_map(
    config: &serde_json::Map<String, Value>,
) -> ByoHostnameConfigStatus {
    let Some(endpoint) = config.get("mcp_endpoint") else {
        return ByoHostnameConfigStatus::None;
    };
    let Some(endpoint) = endpoint.as_object() else {
        return ByoHostnameConfigStatus::None;
    };
    let Some(byo) = endpoint.get("byo_hostname") else {
        return ByoHostnameConfigStatus::None;
    };
    let Some(byo_obj) = byo.as_object() else {
        return ByoHostnameConfigStatus::Invalid;
    };
    let generation = match byo_obj.get("generation") {
        Some(Value::Number(n)) => match n.as_u64() {
            Some(g) if g >= 1 => g,
            _ => return ByoHostnameConfigStatus::Invalid,
        },
        _ => return ByoHostnameConfigStatus::Invalid,
    };
    let enabled = match byo_obj.get("enabled") {
        Some(Value::Bool(b)) => *b,
        _ => return ByoHostnameConfigStatus::Invalid,
    };
    let hostname = match byo_obj.get("hostname") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => match canonicalize_byo_hostname(s) {
            Ok(canon) if &canon == s => Some(canon),
            _ => return ByoHostnameConfigStatus::Invalid,
        },
        _ => return ByoHostnameConfigStatus::Invalid,
    };
    ByoHostnameConfigStatus::Configured(ByoHostnameConfig {
        hostname,
        enabled,
        generation,
    })
}

/// Read the BYO hostname configuration status from journal config.
pub fn byo_hostname_config(read: &JournalConfigRead) -> ByoHostnameConfigStatus {
    let Some(config) = read.config.as_ref() else {
        return ByoHostnameConfigStatus::None;
    };
    byo_hostname_config_from_map(config)
}

/// Transition function for mutating BYO hostname configuration.
pub fn transition_byo_hostname(
    current: Option<&ByoHostnameConfig>,
    op: ByoHostnameOp<'_>,
) -> Result<ByoHostnameConfig, ByoHostnameTransitionError> {
    match op {
        ByoHostnameOp::SetHostname { hostname, enabled } => {
            let canonical = canonicalize_byo_hostname(hostname)
                .map_err(ByoHostnameTransitionError::InvalidHostname)?;
            match current {
                None => {
                    if enabled {
                        return Err(ByoHostnameTransitionError::EnableSeparately);
                    }
                    Ok(ByoHostnameConfig {
                        hostname: Some(canonical),
                        enabled: false,
                        generation: 1,
                    })
                }
                Some(cur) => {
                    let same_name = cur.hostname.as_deref() == Some(&canonical);
                    if enabled && !same_name {
                        return Err(ByoHostnameTransitionError::EnableSeparately);
                    }
                    let generation = if same_name {
                        cur.generation
                    } else {
                        cur.generation.saturating_add(1)
                    };
                    Ok(ByoHostnameConfig {
                        hostname: Some(canonical),
                        enabled: if same_name { enabled } else { false },
                        generation,
                    })
                }
            }
        }
        ByoHostnameOp::SetEnabled { enabled } => {
            let cur = current.ok_or(ByoHostnameTransitionError::NoStoredHostnameToEnable)?;
            if cur.hostname.is_none() {
                return Err(ByoHostnameTransitionError::NoStoredHostnameToEnable);
            }
            Ok(ByoHostnameConfig {
                hostname: cur.hostname.clone(),
                enabled,
                generation: cur.generation,
            })
        }
        ByoHostnameOp::RemoveHostname => {
            let cur = current.ok_or(ByoHostnameTransitionError::NoStoredConfigToRemove)?;
            Ok(ByoHostnameConfig {
                hostname: None,
                enabled: false,
                generation: cur.generation.saturating_add(1),
            })
        }
    }
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
    fn certificate_environment_absent_config_is_production() {
        for config in [None, Some(Map::new())] {
            let read = read(config);
            assert_eq!(
                mcp_endpoint_certificate_environment(&read),
                Ok(McpEndpointCertificateEnvironment::Production)
            );
        }
    }

    #[test]
    fn certificate_environment_absent_key_is_production() {
        for config in [
            Some(config_with_endpoint(json!({}))),
            Some(config_with_endpoint(json!({"enabled": false}))),
            Some(config_with_endpoint(json!({"enabled": true}))),
        ] {
            let read = read(config);
            assert_eq!(
                mcp_endpoint_certificate_environment(&read),
                Ok(McpEndpointCertificateEnvironment::Production)
            );
        }
    }

    #[test]
    fn certificate_environment_explicit_staging_is_staging() {
        assert_eq!(
            mcp_endpoint_certificate_environment(&read(Some(config_with_endpoint(json!({
                "certificate_environment": "staging",
            }))))),
            Ok(McpEndpointCertificateEnvironment::Staging)
        );
    }

    #[test]
    fn certificate_environment_explicit_production_is_production() {
        assert_eq!(
            mcp_endpoint_certificate_environment(&read(Some(config_with_endpoint(json!({
                "certificate_environment": "production",
            }))))),
            Ok(McpEndpointCertificateEnvironment::Production)
        );
    }

    #[test]
    fn certificate_environment_malformed_values_fail_closed() {
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
        assert_eq!(
            mcp_endpoint_force_staging_renewal(&read(Some(config_with_endpoint(json!({
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

    #[test]
    fn local_door_config_matrix() {
        // Missing file
        let missing_file = read(None);
        assert_eq!(local_door_config(&missing_file), LocalDoorConfig::On);
        assert!(local_door_enabled(&missing_file));

        // Missing mcp_endpoint key
        let missing_endpoint = read(Some(Map::new()));
        assert_eq!(local_door_config(&missing_endpoint), LocalDoorConfig::On);
        assert!(local_door_enabled(&missing_endpoint));

        // Missing local_door key inside mcp_endpoint
        let missing_key = read(Some(config_with_endpoint(json!({}))));
        assert_eq!(local_door_config(&missing_key), LocalDoorConfig::On);
        assert!(local_door_enabled(&missing_key));

        // Explicit true
        let explicit_true = read(Some(config_with_endpoint(json!({"local_door": true}))));
        assert_eq!(local_door_config(&explicit_true), LocalDoorConfig::On);
        assert!(local_door_enabled(&explicit_true));

        // Explicit false
        let explicit_false = read(Some(config_with_endpoint(json!({"local_door": false}))));
        assert_eq!(local_door_config(&explicit_false), LocalDoorConfig::Off);
        assert!(!local_door_enabled(&explicit_false));

        // Invalid values
        for invalid_val in [
            json!(null),
            json!(1),
            json!("true"),
            json!("on"),
            json!([]),
            json!({}),
        ] {
            let invalid_read = read(Some(config_with_endpoint(
                json!({"local_door": invalid_val}),
            )));
            assert_eq!(local_door_config(&invalid_read), LocalDoorConfig::Invalid);
            assert!(!local_door_enabled(&invalid_read));
        }

        // Non-object mcp_endpoint
        let non_object = read(Some(config_with_endpoint(json!(false))));
        assert_eq!(local_door_config(&non_object), LocalDoorConfig::Invalid);
        assert!(!local_door_enabled(&non_object));
    }

    #[test]
    fn lan_door_config_matrix() {
        // Missing file -> Off
        let missing_file = read(None);
        assert_eq!(lan_door_config(&missing_file), LocalDoorConfig::Off);
        assert!(!lan_door_enabled(&missing_file));

        // Missing mcp_endpoint key -> Off
        let missing_endpoint = read(Some(Map::new()));
        assert_eq!(lan_door_config(&missing_endpoint), LocalDoorConfig::Off);
        assert!(!lan_door_enabled(&missing_endpoint));

        // Missing lan_door key inside mcp_endpoint -> Off
        let missing_key = read(Some(config_with_endpoint(json!({}))));
        assert_eq!(lan_door_config(&missing_key), LocalDoorConfig::Off);
        assert!(!lan_door_enabled(&missing_key));

        // Explicit false -> Off
        let explicit_false = read(Some(config_with_endpoint(json!({"lan_door": false}))));
        assert_eq!(lan_door_config(&explicit_false), LocalDoorConfig::Off);
        assert!(!lan_door_enabled(&explicit_false));

        // Explicit true -> On
        let explicit_true = read(Some(config_with_endpoint(json!({"lan_door": true}))));
        assert_eq!(lan_door_config(&explicit_true), LocalDoorConfig::On);
        assert!(lan_door_enabled(&explicit_true));

        // Invalid values (null, number, string, array, object) -> Invalid
        for invalid_val in [
            json!(null),
            json!(1),
            json!("true"),
            json!("on"),
            json!([]),
            json!({}),
        ] {
            let invalid_read = read(Some(config_with_endpoint(json!({"lan_door": invalid_val}))));
            assert_eq!(lan_door_config(&invalid_read), LocalDoorConfig::Invalid);
            assert!(!lan_door_enabled(&invalid_read));
        }

        // Non-object mcp_endpoint -> Invalid
        let non_object = read(Some(config_with_endpoint(json!(false))));
        assert_eq!(lan_door_config(&non_object), LocalDoorConfig::Invalid);
        assert!(!lan_door_enabled(&non_object));
    }

    #[test]
    fn canonicalize_byo_hostname_table() {
        assert_eq!(
            canonicalize_byo_hostname("MCP.Example.COM").unwrap(),
            "mcp.example.com"
        );
        assert_eq!(
            canonicalize_byo_hostname("a-b.c-d.org").unwrap(),
            "a-b.c-d.org"
        );

        // Reject classes
        assert_eq!(canonicalize_byo_hostname(""), Err(ByoHostnameError::Empty));
        assert_eq!(
            canonicalize_byo_hostname(&format!("{}.com", "a".repeat(250))),
            Err(ByoHostnameError::TooLong)
        );
        assert_eq!(
            canonicalize_byo_hostname("mcp.exämple.com"),
            Err(ByoHostnameError::NonAscii)
        );
        assert_eq!(
            canonicalize_byo_hostname("mcp:443"),
            Err(ByoHostnameError::ForbiddenCharacter)
        );
        assert_eq!(
            canonicalize_byo_hostname("mcp/path"),
            Err(ByoHostnameError::ForbiddenCharacter)
        );
        assert_eq!(
            canonicalize_byo_hostname("mcp?query"),
            Err(ByoHostnameError::ForbiddenCharacter)
        );
        assert_eq!(
            canonicalize_byo_hostname("mcp#frag"),
            Err(ByoHostnameError::ForbiddenCharacter)
        );
        assert_eq!(
            canonicalize_byo_hostname("user@host.com"),
            Err(ByoHostnameError::ForbiddenCharacter)
        );
        assert_eq!(
            canonicalize_byo_hostname("*.example.com"),
            Err(ByoHostnameError::ForbiddenCharacter)
        );
        assert_eq!(
            canonicalize_byo_hostname("mcp%20.com"),
            Err(ByoHostnameError::ForbiddenCharacter)
        );
        assert_eq!(
            canonicalize_byo_hostname("mcp name.com"),
            Err(ByoHostnameError::ForbiddenCharacter)
        );
        assert_eq!(
            canonicalize_byo_hostname("127.0.0.1"),
            Err(ByoHostnameError::IpLiteral)
        );
        assert_eq!(
            canonicalize_byo_hostname("192.168.1.1"),
            Err(ByoHostnameError::IpLiteral)
        );
        assert_eq!(
            canonicalize_byo_hostname("[::1]"),
            Err(ByoHostnameError::ForbiddenCharacter)
        );
        assert_eq!(
            canonicalize_byo_hostname("localhost"),
            Err(ByoHostnameError::LocalHostOrSuffix)
        );
        assert_eq!(
            canonicalize_byo_hostname("foo.localhost"),
            Err(ByoHostnameError::LocalHostOrSuffix)
        );
        assert_eq!(
            canonicalize_byo_hostname("host.local"),
            Err(ByoHostnameError::LocalHostOrSuffix)
        );
        assert_eq!(
            canonicalize_byo_hostname("host.localdomain"),
            Err(ByoHostnameError::LocalHostOrSuffix)
        );
        assert_eq!(
            canonicalize_byo_hostname("host.internal"),
            Err(ByoHostnameError::LocalHostOrSuffix)
        );
        assert_eq!(
            canonicalize_byo_hostname("1.0.0.127.in-addr.arpa"),
            Err(ByoHostnameError::LocalHostOrSuffix)
        );
        assert_eq!(
            canonicalize_byo_hostname("solstone.me"),
            Err(ByoHostnameError::SolstoneSuffix)
        );
        assert_eq!(
            canonicalize_byo_hostname("my.solstone.me"),
            Err(ByoHostnameError::SolstoneSuffix)
        );
        assert_eq!(
            canonicalize_byo_hostname("solstone.app"),
            Err(ByoHostnameError::SolstoneSuffix)
        );
        assert_eq!(
            canonicalize_byo_hostname("services.solstone.app"),
            Err(ByoHostnameError::SolstoneSuffix)
        );
        assert_eq!(
            canonicalize_byo_hostname("updates.solstone.app"),
            Err(ByoHostnameError::SolstoneSuffix)
        );
        assert_eq!(
            canonicalize_byo_hostname("solpbc.org"),
            Err(ByoHostnameError::SolstoneSuffix)
        );
        assert_eq!(
            canonicalize_byo_hostname("sub.solpbc.org"),
            Err(ByoHostnameError::SolstoneSuffix)
        );
        assert_eq!(
            canonicalize_byo_hostname("singlelabel"),
            Err(ByoHostnameError::SingleLabel)
        );
        assert_eq!(
            canonicalize_byo_hostname(".leading.dot.com"),
            Err(ByoHostnameError::EmptyLabel)
        );
        assert_eq!(
            canonicalize_byo_hostname("trailing.dot.com."),
            Err(ByoHostnameError::EmptyLabel)
        );
        assert_eq!(
            canonicalize_byo_hostname("double..dot.com"),
            Err(ByoHostnameError::EmptyLabel)
        );
        assert_eq!(
            canonicalize_byo_hostname(&format!("{}.com", "a".repeat(64))),
            Err(ByoHostnameError::LabelTooLong)
        );
        assert_eq!(
            canonicalize_byo_hostname("-leading.hyphen.com"),
            Err(ByoHostnameError::LabelHyphenBoundary)
        );
        assert_eq!(
            canonicalize_byo_hostname("trailing-.hyphen.com"),
            Err(ByoHostnameError::LabelHyphenBoundary)
        );
        assert_eq!(
            canonicalize_byo_hostname("xn--example.com"),
            Err(ByoHostnameError::LabelPunycode)
        );
    }

    #[test]
    fn transition_byo_hostname_matrix() {
        // Initial set
        let init = transition_byo_hostname(
            None,
            ByoHostnameOp::SetHostname {
                hostname: "MCP.Example.COM",
                enabled: false,
            },
        )
        .unwrap();
        assert_eq!(
            init,
            ByoHostnameConfig {
                hostname: Some("mcp.example.com".to_string()),
                enabled: false,
                generation: 1,
            }
        );

        // Cannot set with enabled: true directly
        assert_eq!(
            transition_byo_hostname(
                None,
                ByoHostnameOp::SetHostname {
                    hostname: "mcp.example.com",
                    enabled: true,
                }
            ),
            Err(ByoHostnameTransitionError::EnableSeparately)
        );

        // Enable same hostname
        let enabled =
            transition_byo_hostname(Some(&init), ByoHostnameOp::SetEnabled { enabled: true })
                .unwrap();
        assert_eq!(
            enabled,
            ByoHostnameConfig {
                hostname: Some("mcp.example.com".to_string()),
                enabled: true,
                generation: 1,
            }
        );

        // Disable same hostname
        let disabled =
            transition_byo_hostname(Some(&enabled), ByoHostnameOp::SetEnabled { enabled: false })
                .unwrap();
        assert_eq!(
            disabled,
            ByoHostnameConfig {
                hostname: Some("mcp.example.com".to_string()),
                enabled: false,
                generation: 1,
            }
        );

        // Set same hostname with enabled: true succeeds with same generation
        let reenabled = transition_byo_hostname(
            Some(&disabled),
            ByoHostnameOp::SetHostname {
                hostname: "mcp.example.com",
                enabled: true,
            },
        )
        .unwrap();
        assert_eq!(
            reenabled,
            ByoHostnameConfig {
                hostname: Some("mcp.example.com".to_string()),
                enabled: true,
                generation: 1,
            }
        );

        // Set different hostname with enabled: true fails with EnableSeparately
        assert_eq!(
            transition_byo_hostname(
                Some(&reenabled),
                ByoHostnameOp::SetHostname {
                    hostname: "other.example.org",
                    enabled: true,
                },
            ),
            Err(ByoHostnameTransitionError::EnableSeparately)
        );

        // Change hostname bumps generation
        let changed = transition_byo_hostname(
            Some(&disabled),
            ByoHostnameOp::SetHostname {
                hostname: "other.example.org",
                enabled: false,
            },
        )
        .unwrap();
        assert_eq!(
            changed,
            ByoHostnameConfig {
                hostname: Some("other.example.org".to_string()),
                enabled: false,
                generation: 2,
            }
        );

        // Remove hostname bumps generation and clears hostname
        let removed =
            transition_byo_hostname(Some(&changed), ByoHostnameOp::RemoveHostname).unwrap();
        assert_eq!(
            removed,
            ByoHostnameConfig {
                hostname: None,
                enabled: false,
                generation: 3,
            }
        );

        // Cannot enable removed config
        assert_eq!(
            transition_byo_hostname(Some(&removed), ByoHostnameOp::SetEnabled { enabled: true }),
            Err(ByoHostnameTransitionError::NoStoredHostnameToEnable)
        );

        // Re-add after remove bumps generation
        let readded = transition_byo_hostname(
            Some(&removed),
            ByoHostnameOp::SetHostname {
                hostname: "other.example.org",
                enabled: false,
            },
        )
        .unwrap();
        assert_eq!(
            readded,
            ByoHostnameConfig {
                hostname: Some("other.example.org".to_string()),
                enabled: false,
                generation: 4,
            }
        );
    }
}
