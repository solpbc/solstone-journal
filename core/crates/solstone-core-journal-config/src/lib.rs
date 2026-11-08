// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Strict journal configuration reads and default materialization.

/// Existing child-only credential selector, shared with credential lifetime
/// accounting. Conventional ambient provider keys are never selectors.
pub const GENERATE_API_KEY_OVERRIDE_ENV: &str = "SOLSTONE_GENERATE_API_KEY_OVERRIDE";

mod body_facet;
mod capture_zone;
mod defaults;
mod direct_port;
mod mcp_endpoint;
mod name;
mod notification_labels;
pub mod parakeet_coreml;
mod path;
pub mod processing;
mod read;
pub mod work_priority;
mod zone;

#[cfg(test)]
mod test_support;

pub use body_facet::{
    BODY_FACET_KEY, BodyFacetChoice, BodyFacetOffer, FALLBACK_BODY_FACET, IMPORTS_SECTION,
    PREFERRED_BODY_FACET, StoredBodyFacet, body_facet_value, is_body_facet_name, parse_body_facet,
};
pub use capture_zone::{CaptureZone, capture_zone_view};
pub use defaults::{materialized_defaults, plain_defaults};
pub use direct_port::{
    DEFAULT_DIRECT_DOOR_PORT, DirectDoorPortError, DirectDoorPortValueError,
    direct_door_port_from_config, read_direct_door_port,
};
pub use mcp_endpoint::{
    ByoHostnameConfig, ByoHostnameConfigStatus, ByoHostnameError, ByoHostnameOp,
    ByoHostnameTransitionError, ByoRelayConfig, LocalDoorConfig, MCP_BYO_INGRESS_PORT,
    MCP_ENDPOINT_LOOPBACK_PORT, MCP_LAN_DOOR_PORT, MCP_LAN_DOOR_RESOURCE, MCP_LOCAL_DOOR_ORIGIN,
    MCP_LOCAL_DOOR_PORT, MCP_LOCAL_DOOR_RESOURCE, McpEndpointCapability,
    McpEndpointCapabilityError, McpEndpointCertificateEnvironment,
    McpEndpointCertificateEnvironmentError, McpEndpointForceStagingRenewalError,
    byo_hostname_config, byo_hostname_config_from_map, byo_relay_config, byo_relay_config_from_map,
    canonicalize_byo_hostname, is_public_relay_address, lan_door_config, lan_door_enabled,
    local_door_config, local_door_enabled, mcp_endpoint_capability,
    mcp_endpoint_certificate_environment, mcp_endpoint_force_staging_renewal,
    transition_byo_hostname,
};
pub use name::is_path_shaped_name;
pub use notification_labels::{
    SYSTEM_NOTIFICATIONS, SYSTEM_NOTIFICATIONS_LINUX, SYSTEM_NOTIFICATIONS_MACOS,
};
pub use path::get_journal_config_path;
#[cfg(any(unix, windows))]
pub use read::read_journal_config_bound;
pub use read::{
    ConfigLoadError, DailyInputSinceError, JournalConfigMutationBase, JournalConfigRead, SavedLane,
    daily_input_since, entity_tier8_typo_acceptance_enabled, fingerprint_journal_config_bytes,
    load_mutation_base, no_thinking_engine_chosen, parse_daily_input_day, read_journal_config,
    saved_thinking_lane,
};
#[cfg(any(unix, windows))]
pub use read::{read_journal_config_in_directory, read_journal_config_observed_in_directory};
pub use zone::{Tz, host_zone, owner_zone, parse_zone, placed_zone, zone_label};

#[cfg(test)]
mod tests;
