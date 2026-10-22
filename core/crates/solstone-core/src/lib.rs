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

#[cfg(all(test, feature = "full-tests"))]
#[path = "credential_binding_composition.rs"]
mod credential_binding_composition;

#[cfg(feature = "journal-mcp-endpoint")]
pub use solstone_core_mcp_endpoint::{
    ActivityAnchor, ActivityEntry, ActivityPage, ActivityQuery, ActivityReadError, AuditOutcome,
    AuditToolName, ConnectionReadSnapshot, CreatedPairingCode, McpEndpointTlsService,
    McpProbeError, McpServiceError, OAuthClientSummary, OAuthGrantSummary, OAuthStore,
    OAuthStoreError, PermissionDecision, PermissionStore, PermissionStoreError, PermissionsFile,
    ReadPermission, ReadScope, RecordedOutcome, RequestRecord, ResultShape, TokenStore,
    TokenStoreError, TokenSummary, VerifiedToken, evaluate_connection_read,
    mcp_endpoint_server_config, read_activity, resolve_permission_facet_names,
    run_local_door_with_hosted_parent, run_mcp_probe, run_native_service_with_hosted_parent, tally,
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
                    AdmittedCategory::ScreenText,
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

    #[test]
    fn mcp_permission_lifecycle() {
        owner_permission_show_set_clear_lifecycle();
    }
}

#[cfg(test)]
mod service_legacy_tests {
    use std::collections::BTreeMap;

    use solstone_core_installation_identity::{GuardFields, parse_service_guard_environment};
    use solstone_core_service_unit::{
        WindowsServiceAction, is_exact_legacy_launchd_plist, is_exact_legacy_systemd_unit,
        render_launchd_plist, render_systemd_unit,
    };

    fn test_guard() -> GuardFields {
        parse_service_guard_environment(&BTreeMap::from([
            (
                "SOLSTONE_INSTALLATION_NAMESPACE".to_owned(),
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_owned(),
            ),
            (
                "SOLSTONE_INSTALLATION_ID".to_owned(),
                "0123456789abcdef0123456789abcdef".to_owned(),
            ),
            (
                "SOLSTONE_INSTALLATION_GENERATION".to_owned(),
                "1".to_owned(),
            ),
            (
                "SOLSTONE_INSTALLATION_JOURNAL_TOKEN".to_owned(),
                "2f6a6f75726e616c".to_owned(),
            ),
        ]))
        .unwrap()
        .unwrap()
    }

    #[test]
    fn exact_legacy_writer_units_are_selected_for_republish_and_dropins_are_rejected() {
        let temp = tempfile::tempdir_in("/var/tmp").unwrap();
        let home = temp.path();
        let legacy_launcher = home.join(".local/bin/journal");

        let historical_systemd = format!("ExecStart={} start 5015\n", legacy_launcher.display());
        assert!(is_exact_legacy_systemd_unit(&historical_systemd, home));

        let mut historical_plist_dict = plist::Dictionary::new();
        historical_plist_dict.insert(
            "ProgramArguments".into(),
            plist::Value::Array(vec![
                plist::Value::String(legacy_launcher.display().to_string()),
                plist::Value::String("start".into()),
                plist::Value::String("5015".into()),
            ]),
        );
        let mut historical_plist = Vec::new();
        plist::Value::Dictionary(historical_plist_dict)
            .to_writer_xml(&mut historical_plist)
            .unwrap();
        assert!(is_exact_legacy_launchd_plist(&historical_plist, home));

        let solstone_launcher = home.join(".local/bin/solstone");
        let current_systemd = render_systemd_unit(
            &BTreeMap::new(),
            solstone_launcher.to_str().unwrap(),
            "5015",
        );
        assert!(current_systemd.contains("journal start 5015"));
        assert!(!is_exact_legacy_systemd_unit(&current_systemd, home));

        let current_plist = render_launchd_plist(
            &BTreeMap::new(),
            solstone_launcher.to_str().unwrap(),
            "5015",
        );
        let plist_val = plist::Value::from_reader(std::io::Cursor::new(&current_plist)).unwrap();
        let args = plist_val
            .as_dictionary()
            .unwrap()
            .get("ProgramArguments")
            .unwrap()
            .as_array()
            .unwrap();
        assert_eq!(args[1].as_string(), Some("journal"));
        assert_eq!(args[2].as_string(), Some("start"));
        assert_eq!(args[3].as_string(), Some("5015"));
        assert!(!is_exact_legacy_launchd_plist(&current_plist, home));

        let action = WindowsServiceAction {
            port: 5015,
            journal: "C:\\journal".to_owned(),
            guard: test_guard(),
        };
        let current_windows_args = action.arguments().unwrap();
        assert_eq!(current_windows_args[0], "journal");
        let legacy_windows_args = current_windows_args[1..].to_vec();
        assert_eq!(legacy_windows_args.len(), 13);
        assert_eq!(legacy_windows_args[0], "supervisor");
        assert_eq!(
            WindowsServiceAction::parse(&legacy_windows_args).unwrap(),
            action
        );

        // A different binary
        let diff_bin = historical_systemd.replace("/.local/bin/journal", "/.local/bin/sol");
        assert!(!is_exact_legacy_systemd_unit(&diff_bin, home));
        let diff_bin_plist = String::from_utf8(historical_plist.clone())
            .unwrap()
            .replace("/.local/bin/journal", "/.local/bin/sol");
        assert!(!is_exact_legacy_launchd_plist(
            diff_bin_plist.as_bytes(),
            home
        ));

        // An unrecognized verb
        let diff_verb = historical_systemd.replace(" start 5015", " stop 5015");
        assert!(!is_exact_legacy_systemd_unit(&diff_verb, home));
        let diff_verb_plist = String::from_utf8(historical_plist.clone())
            .unwrap()
            .replace("<string>start</string>", "<string>stop</string>");
        assert!(!is_exact_legacy_launchd_plist(
            diff_verb_plist.as_bytes(),
            home
        ));

        // A *.conf drop-in beside the unit
        let drop_in_dir = home.join(".config/systemd/user/solstone.service.d");
        std::fs::create_dir_all(&drop_in_dir).unwrap();
        std::fs::write(
            drop_in_dir.join("override.conf"),
            "[Service]\nRestartSec=10\n",
        )
        .unwrap();
        let has_conf_drop_in = std::fs::read_dir(&drop_in_dir)
            .ok()
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .any(|entry| entry.path().extension().is_some_and(|ext| ext == "conf"))
            })
            .unwrap_or(false);
        assert!(has_conf_drop_in);
    }
}
