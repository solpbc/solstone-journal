// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::json;
use solstone_core_journal_config::{
    JournalConfigRead, MCP_ENDPOINT_LOOPBACK_PORT, McpEndpointCapability,
    McpEndpointCapabilityError, mcp_endpoint_capability,
};

fn read(config: serde_json::Value) -> JournalConfigRead {
    JournalConfigRead {
        present: true,
        sha256: None,
        config: config.as_object().cloned(),
    }
}

#[test]
fn public_mcp_endpoint_surface_is_exhaustive() {
    let enabled = read(json!({"mcp_endpoint": {"enabled": true}}));
    match mcp_endpoint_capability(&enabled).expect("enabled configuration should parse") {
        McpEndpointCapability::Disabled => panic!("enabled configuration should be enabled"),
        McpEndpointCapability::Enabled => {}
    }

    let disabled = read(json!({"mcp_endpoint": {"enabled": false}}));
    match mcp_endpoint_capability(&disabled).expect("disabled configuration should parse") {
        McpEndpointCapability::Disabled => {}
        McpEndpointCapability::Enabled => panic!("disabled configuration should be disabled"),
    }

    let invalid = read(json!({"mcp_endpoint": {"enabled": "yes"}}));
    match mcp_endpoint_capability(&invalid) {
        Ok(McpEndpointCapability::Disabled) => panic!("invalid configuration must fail"),
        Ok(McpEndpointCapability::Enabled) => panic!("invalid configuration must fail"),
        Err(McpEndpointCapabilityError::EnabledMustBeBoolean) => {}
    }

    assert_eq!(MCP_ENDPOINT_LOOPBACK_PORT, 7658u16);
}
