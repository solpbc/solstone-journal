// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg_attr(not(windows), forbid(unsafe_code))]

//! Library seams shared by the native supervisor executable and hosted callers.

#[cfg(windows)]
mod heartbeat_pid_windows;
pub mod installation_context;
#[cfg(any(unix, windows))]
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
    ActivityAnchor, ActivityEntry, ActivityPage, ActivityQuery, ActivityReadError, AuditOutcome,
    AuditToolName, ConnectionReadSnapshot, CreatedPairingCode, McpEndpointTlsService,
    McpProbeError, McpServiceError, OAuthClientSummary, OAuthGrantSummary, OAuthStore,
    OAuthStoreError, PermissionDecision, PermissionStore, PermissionStoreError, PermissionsFile,
    ReadPermission, ReadScope, RecordedOutcome, RequestRecord, ResultShape, TokenStore,
    TokenStoreError, TokenSummary, VerifiedToken, evaluate_connection_read,
    mcp_endpoint_server_config, read_activity, resolve_permission_facet_names, run_mcp_probe,
    run_native_service_with_hosted_parent, tally,
};

#[cfg(all(test, unix, feature = "journal-mcp-endpoint"))]
mod mcp_endpoint_public_surface_tests {
    use std::sync::Arc;
    use tempfile::TempDir;

    use super::{
        ConnectionReadSnapshot, McpEndpointTlsService, PermissionDecision, PermissionStore,
        ReadPermission, ReadScope, TokenStore, evaluate_connection_read,
        mcp_endpoint_server_config,
    };
    use solstone_core_indexer_query::{AdmittedCategory, ConnectionScope};
    use solstone_core_journal_config::MCP_ENDPOINT_LOOPBACK_PORT;

    #[test]
    fn lane_b_can_consume_the_one_root_tls_and_port_seam() {
        fn consume(service: &McpEndpointTlsService) -> Arc<rustls::ServerConfig> {
            mcp_endpoint_server_config(service)
        }

        let _ = consume;
        assert_eq!(MCP_ENDPOINT_LOOPBACK_PORT, 7658);
    }

    #[test]
    fn owner_permission_show_set_clear_lifecycle() {
        let temp = TempDir::new_in("/var/tmp").expect("temp dir");
        let journal_path = temp.path();

        // Create a bearer token
        let token_store = TokenStore::open(journal_path);
        let _created = token_store.create("test-agent").expect("creates token");
        let token_id = token_store
            .find_id_by_label("test-agent")
            .unwrap()
            .expect("finds token id");
        let connection_key = format!("bearer:{token_id}");

        let perm_store = PermissionStore::open(journal_path);

        // Show initially empty
        assert_eq!(
            evaluate_connection_read(journal_path, &connection_key),
            PermissionDecision::Denied {
                reason: "no_permission"
            }
        );
        assert_eq!(perm_store.get_permission(&connection_key).unwrap(), None);

        // Set permission
        let record = perm_store
            .set_permission(&connection_key, ReadPermission::default_whole_journal())
            .expect("sets permission");
        assert_eq!(record.generation, 1);
        assert_eq!(record.read.as_ref().unwrap().scope, ReadScope::WholeJournal);
        assert_eq!(
            evaluate_connection_read(journal_path, &connection_key),
            PermissionDecision::Snapshot(ConnectionReadSnapshot {
                categories: [
                    AdmittedCategory::Transcripts,
                    AdmittedCategory::Entities,
                    AdmittedCategory::Facets,
                ]
                .into_iter()
                .collect(),
                scope: ConnectionScope::WholeJournal,
                generation: 1,
            })
        );

        // Show reflects active grant
        let fetched = perm_store.get_permission(&connection_key).unwrap().unwrap();
        assert_eq!(fetched.generation, 1);
        assert_eq!(fetched.evaluation, "enforce");

        // Clear permission increments generation and removes read grant
        let cleared = perm_store
            .clear_permission(&connection_key)
            .expect("clears permission");
        assert!(cleared);
        let cleared_rec = perm_store.get_permission(&connection_key).unwrap().unwrap();
        assert_eq!(cleared_rec.generation, 2);
        assert_eq!(cleared_rec.read, None);
        assert_eq!(
            evaluate_connection_read(journal_path, &connection_key),
            PermissionDecision::Denied {
                reason: "no_permission"
            }
        );
    }
}
