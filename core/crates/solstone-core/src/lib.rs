// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Library seams shared by the native supervisor executable and hosted callers.

pub mod installation_context;
#[cfg(unix)]
pub mod supervisor;

// The route command is owned by the binary, but these protocol and lock units
// have no binary-only dependencies and belong in the crate's routine unit gate.
#[cfg(all(test, unix))]
#[path = "journal_route/coordination_lock.rs"]
mod journal_route_coordination_lock;
#[cfg(all(test, unix))]
#[path = "journal_route/record.rs"]
mod journal_route_record;

#[cfg(all(unix, feature = "journal-mcp-endpoint"))]
pub use solstone_core_mcp_endpoint::{
    CreatedPairingCode, McpEndpointTlsService, McpServiceError, OAuthClientSummary, OAuthStore,
    OAuthStoreError, TokenStore, TokenStoreError, mcp_endpoint_server_config,
    run_native_service_with_hosted_parent,
};

#[cfg(all(test, unix, feature = "journal-mcp-endpoint"))]
mod mcp_endpoint_public_surface_tests {
    use std::sync::Arc;

    use super::{McpEndpointTlsService, mcp_endpoint_server_config};
    use solstone_core_journal_config::MCP_ENDPOINT_LOOPBACK_PORT;

    #[test]
    fn lane_b_can_consume_the_one_root_tls_and_port_seam() {
        fn consume(service: &McpEndpointTlsService) -> Arc<rustls::ServerConfig> {
            mcp_endpoint_server_config(service)
        }

        let _ = consume;
        assert_eq!(MCP_ENDPOINT_LOOPBACK_PORT, 7658);
    }
}
