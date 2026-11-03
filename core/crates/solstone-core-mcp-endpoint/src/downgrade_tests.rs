// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::Path;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use solstone_core_indexer_query::{AdmittedCategory, ConnectionScope};

// --- Copied verbatim from git show 790a0ae42:core/crates/solstone-core-mcp-endpoint/src/permissions.rs ---

const ENDPOINT_DIRECTORY: &str = "mcp-endpoint";
const PERMISSIONS_FILE: &str = "permissions.json";
const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PermissionsFile {
    pub schema: u32,
    #[serde(default)]
    pub permissions: Vec<ConnectionPermissionRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectionPermissionRecord {
    pub connection: String,
    pub schema: u32,
    pub generation: u64,
    pub evaluation: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read: Option<ReadPermission>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadPermission {
    pub categories: Vec<String>,
    pub scope: ReadScope,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReadScope {
    WholeJournal,
    Facets { ids: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionDecision {
    Snapshot(ConnectionReadSnapshot),
    Denied { reason: &'static str },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionReadSnapshot {
    pub categories: BTreeSet<AdmittedCategory>,
    pub scope: ConnectionScope,
    pub generation: u64,
}

#[must_use]
pub fn evaluate_connection_read(journal_root: &Path, connection_key: &str) -> PermissionDecision {
    let path = journal_root.join(ENDPOINT_DIRECTORY).join(PERMISSIONS_FILE);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return PermissionDecision::Denied {
                reason: "no_permission",
            };
        }
        Err(_) => {
            return PermissionDecision::Denied {
                reason: "unenforceable",
            };
        }
    };

    let file: PermissionsFile = match serde_json::from_slice(&bytes) {
        Ok(f) => f,
        Err(_) => {
            return PermissionDecision::Denied {
                reason: "unenforceable",
            };
        }
    };

    if file.schema != SCHEMA_VERSION {
        return PermissionDecision::Denied {
            reason: "unenforceable",
        };
    }

    let Some(record) = file
        .permissions
        .iter()
        .find(|p| p.connection == connection_key)
    else {
        return PermissionDecision::Denied {
            reason: "no_permission",
        };
    };

    if record.schema != SCHEMA_VERSION || record.evaluation != "enforce" {
        return PermissionDecision::Denied {
            reason: "unenforceable",
        };
    }

    let Some(ref read) = record.read else {
        return PermissionDecision::Denied {
            reason: "no_permission",
        };
    };

    let categories = read
        .categories
        .iter()
        .filter_map(|category| match category.as_str() {
            "transcripts" => Some(AdmittedCategory::Transcripts),
            "entities" => Some(AdmittedCategory::Entities),
            "facets" => Some(AdmittedCategory::Facets),
            "screen_text" => Some(AdmittedCategory::ScreenText),
            _ => None,
        })
        .collect();
    let scope = match &read.scope {
        ReadScope::WholeJournal => ConnectionScope::WholeJournal,
        ReadScope::Facets { ids } => ConnectionScope::ChosenFacets {
            ids: ids.iter().cloned().collect(),
        },
    };
    PermissionDecision::Snapshot(ConnectionReadSnapshot {
        categories,
        scope,
        generation: record.generation,
    })
}

// --- Copied verbatim from git show 790a0ae42:core/crates/solstone-core-mcp-endpoint/src/tokens.rs ---

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TokenStoreFile {
    schema: u32,
    tokens: Vec<StoredToken>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredToken {
    id: String,
    label: String,
    verifier: String,
    created_at: DateTime<Utc>,
}

// --- Copied verbatim from git show 790a0ae42:core/crates/solstone-core-mcp-endpoint/src/oauth/store.rs ---

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReplayNotice {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) door: String,
    pub(crate) dismissed: bool,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplayNoticesFile {
    notices: Vec<ReplayNotice>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OAuthStoreFile {
    schema: u32,
    #[serde(default)]
    pairing_generation: u64,
    clients: Vec<StoredClient>,
    grants: Vec<StoredGrant>,
    pending: Vec<StoredPending>,
    pairing: Option<StoredPairing>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredClient {
    id: String,
    client_id: String,
    redirect_uris: Vec<String>,
    client_name: Option<String>,
    source: String,
    created_at: DateTime<Utc>,
    last_used_at: Option<DateTime<Utc>>,
    revocation_generation: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredGrant {
    id: String,
    client_record_id: String,
    client_id: String,
    access_verifier: String,
    refresh_verifier: String,
    refresh_generation: u64,
    revocation_generation: u64,
    access_expires_at: DateTime<Utc>,
    refresh_expires_at: DateTime<Utc>,
    created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resource: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    generation: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredPending {
    transaction_id: String,
    client_record_id: String,
    redirect_uri: String,
    resource: String,
    issuer: String,
    pkce_s256: String,
    pkce_method: String,
    state: Option<String>,
    source: String,
    created_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    failure_count: u8,
    authorization_code_verifier: Option<String>,
    code_expires_at: Option<DateTime<Utc>>,
    #[serde(default)]
    permission: Option<ReadPermission>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    generation: Option<u64>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pairing_verified: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pairing_window: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredPairing {
    verifier: String,
    expires_at: DateTime<Utc>,
    generation: u64,
    locked: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    door: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    config_generation: Option<u64>,
}

// --- Test Helpers & Downgrade Proofs ---

fn parse_oauth_downgrade(root: &Path) -> Result<OAuthStoreFile, serde_json::Error> {
    let bytes = fs::read(root.join("mcp-endpoint/oauth.json")).unwrap();
    serde_json::from_slice(&bytes)
}

fn parse_permissions_downgrade(root: &Path) -> Result<PermissionsFile, serde_json::Error> {
    let bytes = fs::read(root.join("mcp-endpoint/permissions.json")).unwrap();
    serde_json::from_slice(&bytes)
}

fn parse_tokens_downgrade(root: &Path) -> Result<TokenStoreFile, serde_json::Error> {
    let bytes = fs::read(root.join("mcp-endpoint/tokens.json")).unwrap();
    serde_json::from_slice(&bytes)
}

fn parse_all_downgrade(root: &Path) -> (OAuthStoreFile, PermissionsFile, TokenStoreFile) {
    (
        parse_oauth_downgrade(root).expect("oauth downgrade parse"),
        parse_permissions_downgrade(root).expect("permissions downgrade parse"),
        parse_tokens_downgrade(root).expect("tokens downgrade parse"),
    )
}

struct ClockGuard;
impl Drop for ClockGuard {
    fn drop(&mut self) {
        crate::oauth::store::set_routine_now(None);
    }
}

#[test]
fn downgrade_owner_pause_idle_pause_resume_takeover_and_source_redemption() {
    let _guard = ClockGuard;
    let base_now = 1_800_000_000_i64;
    crate::oauth::store::set_routine_now(Some(base_now));

    let journal = tempfile::Builder::new()
        .prefix("solstone-mcp-downgrade-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    let root = journal.path();
    let oauth = crate::oauth::OAuthRuntime::new(root, "https://mcp.test".to_owned());
    let source = "192.0.2.1";

    let client_a = oauth
        .store
        .register_client(
            "https://client.a/cimd.json",
            vec!["http://127.0.0.1/callback".to_owned()],
            Some("Client A".to_owned()),
            source,
        )
        .unwrap();
    let pairing_a = oauth
        .store
        .generate_pairing_code_with_door("relay")
        .unwrap();
    let challenge_a = URL_SAFE_NO_PAD.encode(Sha256::digest(b"verifier-a"));
    let tx_a = oauth
        .store
        .create_transaction_with_generation(
            &client_a.id,
            "http://127.0.0.1/callback",
            oauth.binding().canonical(),
            "https://mcp.test",
            &challenge_a,
            "S256",
            None,
            source,
            oauth.binding().stored_grant_generation(),
        )
        .unwrap();
    let auth_a = oauth
        .store
        .complete_pairing_with_permission(
            &tx_a,
            &pairing_a.code,
            Some(crate::permissions::ReadPermission {
                categories: vec!["facets".to_owned()],
                scope: crate::permissions::ReadScope::Facets {
                    ids: vec!["f1".to_owned()],
                },
            }),
            &oauth.binding(),
        )
        .unwrap();
    let tokens_a = oauth
        .store
        .redeem_authorization_code(
            &auth_a.code,
            &client_a.client_id,
            "http://127.0.0.1/callback",
            oauth.binding().canonical(),
            "verifier-a",
            &oauth.binding(),
        )
        .unwrap();
    let oauth_id = tokens_a.token_id;
    let oauth_conn = format!("oauth:{oauth_id}");

    let token_store = crate::tokens::TokenStore::open(root);
    let _bearer = token_store.create("Bearer Token").unwrap();
    let bearer_id = token_store
        .list()
        .unwrap()
        .into_iter()
        .find(|t| t.label == "Bearer Token")
        .unwrap()
        .id;
    let bearer_conn = format!("bearer:{bearer_id}");
    crate::permissions::PermissionStore::open(root)
        .set_permission(
            &bearer_conn,
            crate::permissions::ReadPermission {
                categories: vec!["facets".to_owned()],
                scope: crate::permissions::ReadScope::Facets {
                    ids: vec!["f2".to_owned()],
                },
            },
        )
        .unwrap();

    // Verify initial downgrade parse and evaluation
    let _ = parse_all_downgrade(root);
    match evaluate_connection_read(root, &oauth_conn) {
        PermissionDecision::Snapshot(ConnectionReadSnapshot { scope, .. }) => {
            assert_eq!(
                scope,
                ConnectionScope::ChosenFacets {
                    ids: ["f1".to_owned()].into_iter().collect(),
                }
            );
        }
        other => panic!("expected Snapshot, got {other:?}"),
    }
    match evaluate_connection_read(root, &bearer_conn) {
        PermissionDecision::Snapshot(ConnectionReadSnapshot { scope, .. }) => {
            assert_eq!(
                scope,
                ConnectionScope::ChosenFacets {
                    ids: ["f2".to_owned()].into_iter().collect(),
                }
            );
        }
        other => panic!("expected Snapshot, got {other:?}"),
    }

    // 1. Owner pause of an oauth grant and of a bearer key
    let now = crate::oauth::store::current_time();
    crate::agent_state::set_owner_pause(root, &oauth_conn, now, &crate::tokens::SystemRandomSource)
        .unwrap();
    crate::permissions::PermissionStore::open(root)
        .set_evaluation_only(&oauth_conn, "hold")
        .unwrap();

    crate::agent_state::set_owner_pause(
        root,
        &bearer_conn,
        now,
        &crate::tokens::SystemRandomSource,
    )
    .unwrap();
    crate::permissions::PermissionStore::open(root)
        .set_evaluation_only(&bearer_conn, "hold")
        .unwrap();

    let _ = parse_all_downgrade(root);
    assert_eq!(
        evaluate_connection_read(root, &oauth_conn),
        PermissionDecision::Denied {
            reason: "unenforceable",
        }
    );
    assert_eq!(
        evaluate_connection_read(root, &bearer_conn),
        PermissionDecision::Denied {
            reason: "unenforceable",
        }
    );

    // 2. Resume of each
    crate::agent_state::clear_owner_pause(root, &oauth_conn).unwrap();
    crate::permissions::PermissionStore::open(root)
        .set_evaluation_only(&oauth_conn, "enforce")
        .unwrap();

    crate::agent_state::clear_owner_pause(root, &bearer_conn).unwrap();
    crate::permissions::PermissionStore::open(root)
        .set_evaluation_only(&bearer_conn, "enforce")
        .unwrap();

    let _ = parse_all_downgrade(root);
    match evaluate_connection_read(root, &oauth_conn) {
        PermissionDecision::Snapshot(ConnectionReadSnapshot { scope, .. }) => {
            assert_eq!(
                scope,
                ConnectionScope::ChosenFacets {
                    ids: ["f1".to_owned()].into_iter().collect(),
                }
            );
        }
        other => panic!("expected Snapshot, got {other:?}"),
    }
    match evaluate_connection_read(root, &bearer_conn) {
        PermissionDecision::Snapshot(ConnectionReadSnapshot { scope, .. }) => {
            assert_eq!(
                scope,
                ConnectionScope::ChosenFacets {
                    ids: ["f2".to_owned()].into_iter().collect(),
                }
            );
        }
        other => panic!("expected Snapshot, got {other:?}"),
    }

    // 3. Idle pause (clock one second past refresh_expires_at, no owner-pause row; permission bytes unchanged)
    let expiry = oauth
        .store
        .grant_refresh_expires_at(&oauth_id)
        .unwrap()
        .unwrap();
    let perm_bytes_before = fs::read(root.join("mcp-endpoint/permissions.json")).unwrap();
    crate::oauth::store::set_routine_now(Some(expiry.timestamp() + 1));
    assert!(
        crate::agent_state::is_owner_paused(root, &oauth_conn)
            .unwrap()
            .is_none()
    );
    let perm_bytes_after = fs::read(root.join("mcp-endpoint/permissions.json")).unwrap();
    assert_eq!(perm_bytes_before, perm_bytes_after);

    let _ = parse_all_downgrade(root);
    match evaluate_connection_read(root, &oauth_conn) {
        PermissionDecision::Snapshot(ConnectionReadSnapshot { scope, .. }) => {
            assert_eq!(
                scope,
                ConnectionScope::ChosenFacets {
                    ids: ["f1".to_owned()].into_iter().collect(),
                }
            );
        }
        other => panic!("expected Snapshot, got {other:?}"),
    }

    oauth.store.resume_refresh_expiry(&oauth_id).unwrap();
    let _ = parse_all_downgrade(root);

    // 4. A finished takeover
    let client_b = oauth
        .store
        .register_client(
            "https://client.b/cimd.json",
            vec!["http://127.0.0.1/callback".to_owned()],
            Some("Client B".to_owned()),
            source,
        )
        .unwrap();
    let pairing_takeover = oauth
        .store
        .generate_pairing_code_with_door_and_reconnect("relay", Some(format!("oauth:{oauth_id}")))
        .unwrap();
    let challenge_b = URL_SAFE_NO_PAD.encode(Sha256::digest(b"verifier-b"));
    let tx_takeover = oauth
        .store
        .create_transaction_with_generation(
            &client_b.id,
            "http://127.0.0.1/callback",
            oauth.binding().canonical(),
            "https://mcp.test",
            &challenge_b,
            "S256",
            None,
            source,
            oauth.binding().stored_grant_generation(),
        )
        .unwrap();
    let verified_tx = oauth
        .store
        .verify_pairing(&tx_takeover, &pairing_takeover.code, &oauth.binding())
        .unwrap();
    let auth_takeover = oauth
        .store
        .complete_confirm_pairing(
            &verified_tx,
            Some(crate::permissions::ReadPermission {
                categories: vec!["facets".to_owned()],
                scope: crate::permissions::ReadScope::Facets {
                    ids: vec!["f3".to_owned()],
                },
            }),
            &oauth.binding(),
        )
        .unwrap()
        .unwrap();
    let issued_takeover = oauth
        .store
        .redeem_authorization_code(
            &auth_takeover.code,
            &client_b.client_id,
            "http://127.0.0.1/callback",
            oauth.binding().canonical(),
            "verifier-b",
            &oauth.binding(),
        )
        .unwrap();
    assert_eq!(issued_takeover.token_id, oauth_id);

    let _ = parse_all_downgrade(root);
    let oauth_raw = fs::read_to_string(root.join("mcp-endpoint/oauth.json")).unwrap();
    assert!(!oauth_raw.contains("reconnect_to"));

    match evaluate_connection_read(root, &oauth_conn) {
        PermissionDecision::Snapshot(ConnectionReadSnapshot { scope, .. }) => {
            assert_eq!(
                scope,
                ConnectionScope::ChosenFacets {
                    ids: ["f3".to_owned()].into_iter().collect(),
                }
            );
        }
        other => panic!("expected Snapshot, got {other:?}"),
    }

    // 5. Redemption of a code whose target is source:<64 lowercase hex>
    let hex = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    let seg_dir = root
        .join("chronicle/20261103")
        .join(format!("agent-memory-{hex}"));
    fs::create_dir_all(&seg_dir).unwrap();
    fs::write(seg_dir.join("note.txt"), b"memory segment").unwrap();

    let client_c = oauth
        .store
        .register_client(
            "https://client.c/cimd.json",
            vec!["http://127.0.0.1/callback".to_owned()],
            Some("Client C".to_owned()),
            source,
        )
        .unwrap();
    let pairing_source = oauth
        .store
        .generate_pairing_code_with_door_and_reconnect("relay", Some(format!("source:{hex}")))
        .unwrap();
    let challenge_c = URL_SAFE_NO_PAD.encode(Sha256::digest(b"verifier-c"));
    let tx_source = oauth
        .store
        .create_transaction_with_generation(
            &client_c.id,
            "http://127.0.0.1/callback",
            oauth.binding().canonical(),
            "https://mcp.test",
            &challenge_c,
            "S256",
            None,
            source,
            oauth.binding().stored_grant_generation(),
        )
        .unwrap();
    let verified_source_tx = oauth
        .store
        .verify_pairing(&tx_source, &pairing_source.code, &oauth.binding())
        .unwrap();
    let auth_source = oauth
        .store
        .complete_confirm_pairing(
            &verified_source_tx,
            Some(crate::permissions::ReadPermission {
                categories: vec!["facets".to_owned()],
                scope: crate::permissions::ReadScope::Facets {
                    ids: vec!["f4".to_owned()],
                },
            }),
            &oauth.binding(),
        )
        .unwrap()
        .unwrap();
    let issued_source = oauth
        .store
        .redeem_authorization_code(
            &auth_source.code,
            &client_c.client_id,
            "http://127.0.0.1/callback",
            oauth.binding().canonical(),
            "verifier-c",
            &oauth.binding(),
        )
        .unwrap();

    let conn_c = format!("oauth:{}", issued_source.token_id);
    assert_eq!(
        crate::agent_state::memory_source(root, &conn_c)
            .unwrap()
            .component(),
        hex
    );

    let _ = parse_all_downgrade(root);
    let oauth_raw = fs::read_to_string(root.join("mcp-endpoint/oauth.json")).unwrap();
    assert!(!oauth_raw.contains("reconnect_to"));
}

#[test]
fn downgrade_targeted_endings_leave_no_reconnect_to() {
    let _guard = ClockGuard;
    let base_now = 1_800_000_000_i64;
    crate::oauth::store::set_routine_now(Some(base_now));

    let journal = tempfile::Builder::new()
        .prefix("solstone-mcp-downgrade-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    let root = journal.path();
    let oauth = crate::oauth::OAuthRuntime::new(root, "https://mcp.test".to_owned());
    let source = "192.0.2.1";

    let client = oauth
        .store
        .register_client(
            "https://client.example/cimd.json",
            vec!["http://127.0.0.1/callback".to_owned()],
            Some("Client".to_owned()),
            source,
        )
        .unwrap();

    // Setup an initial grant to target
    let pairing0 = oauth
        .store
        .generate_pairing_code_with_door("relay")
        .unwrap();
    let challenge0 = URL_SAFE_NO_PAD.encode(Sha256::digest(b"v0"));
    let tx0 = oauth
        .store
        .create_transaction_with_generation(
            &client.id,
            "http://127.0.0.1/callback",
            oauth.binding().canonical(),
            "https://mcp.test",
            &challenge0,
            "S256",
            None,
            source,
            oauth.binding().stored_grant_generation(),
        )
        .unwrap();
    let auth0 = oauth
        .store
        .complete_pairing_with_permission(
            &tx0,
            &pairing0.code,
            Some(crate::permissions::ReadPermission {
                categories: vec!["facets".to_owned()],
                scope: crate::permissions::ReadScope::Facets {
                    ids: vec!["f0".to_owned()],
                },
            }),
            &oauth.binding(),
        )
        .unwrap();
    let tokens0 = oauth
        .store
        .redeem_authorization_code(
            &auth0.code,
            &client.client_id,
            "http://127.0.0.1/callback",
            oauth.binding().canonical(),
            "v0",
            &oauth.binding(),
        )
        .unwrap();
    let target_grant_id = tokens0.token_id;

    // 1. A targeted code still open fails parse (reconnect_to present on pairing)
    let pairing1 = oauth
        .store
        .generate_pairing_code_with_door_and_reconnect(
            "relay",
            Some(format!("oauth:{target_grant_id}")),
        )
        .unwrap();
    assert!(parse_oauth_downgrade(root).is_err());

    // 2. Consent submit refuses because target grant was revoked:
    // Open transaction and verify pairing -> reconnect_to on pending transaction
    let challenge1 = URL_SAFE_NO_PAD.encode(Sha256::digest(b"v1"));
    let tx1 = oauth
        .store
        .create_transaction_with_generation(
            &client.id,
            "http://127.0.0.1/callback",
            oauth.binding().canonical(),
            "https://mcp.test",
            &challenge1,
            "S256",
            None,
            source,
            oauth.binding().stored_grant_generation(),
        )
        .unwrap();
    let verified1 = oauth
        .store
        .verify_pairing(&tx1, &pairing1.code, &oauth.binding())
        .unwrap();
    // Transaction open with reconnect_to -> parse fails
    assert!(parse_oauth_downgrade(root).is_err());

    // Revoke target grant
    oauth.store.revoke_grant_by_id(&target_grant_id).unwrap();
    let consent_res = oauth.store.complete_confirm_pairing(
        &verified1,
        Some(crate::permissions::ReadPermission {
            categories: vec!["facets".to_owned()],
            scope: crate::permissions::ReadScope::Facets {
                ids: vec!["f1".to_owned()],
            },
        }),
        &oauth.binding(),
    );
    assert!(matches!(
        consent_res,
        Err(crate::oauth::store::OAuthStoreError::InvalidGrant)
    ));
    // After consent refusal, reconnect_to is cleared and parse succeeds
    assert!(parse_oauth_downgrade(root).is_ok());
    let oauth_raw = fs::read_to_string(root.join("mcp-endpoint/oauth.json")).unwrap();
    assert!(!oauth_raw.contains("reconnect_to"));

    // Setup another grant for ending 3
    let pairing_t2 = oauth
        .store
        .generate_pairing_code_with_door("relay")
        .unwrap();
    let tx_t2 = oauth
        .store
        .create_transaction_with_generation(
            &client.id,
            "http://127.0.0.1/callback",
            oauth.binding().canonical(),
            "https://mcp.test",
            &challenge0,
            "S256",
            None,
            source,
            oauth.binding().stored_grant_generation(),
        )
        .unwrap();
    let auth_t2 = oauth
        .store
        .complete_pairing_with_permission(
            &tx_t2,
            &pairing_t2.code,
            Some(crate::permissions::ReadPermission {
                categories: vec!["facets".to_owned()],
                scope: crate::permissions::ReadScope::Facets {
                    ids: vec!["f0".to_owned()],
                },
            }),
            &oauth.binding(),
        )
        .unwrap();
    let tokens_t2 = oauth
        .store
        .redeem_authorization_code(
            &auth_t2.code,
            &client.client_id,
            "http://127.0.0.1/callback",
            oauth.binding().canonical(),
            "v0",
            &oauth.binding(),
        )
        .unwrap();
    let target_grant_2 = tokens_t2.token_id;

    // 3. Token exchange refuses because target was revoked after auth code issued
    let pairing2 = oauth
        .store
        .generate_pairing_code_with_door_and_reconnect(
            "relay",
            Some(format!("oauth:{target_grant_2}")),
        )
        .unwrap();
    let challenge2 = URL_SAFE_NO_PAD.encode(Sha256::digest(b"v2"));
    let tx2 = oauth
        .store
        .create_transaction_with_generation(
            &client.id,
            "http://127.0.0.1/callback",
            oauth.binding().canonical(),
            "https://mcp.test",
            &challenge2,
            "S256",
            None,
            source,
            oauth.binding().stored_grant_generation(),
        )
        .unwrap();
    let verified2 = oauth
        .store
        .verify_pairing(&tx2, &pairing2.code, &oauth.binding())
        .unwrap();
    let auth2 = oauth
        .store
        .complete_confirm_pairing(
            &verified2,
            Some(crate::permissions::ReadPermission {
                categories: vec!["facets".to_owned()],
                scope: crate::permissions::ReadScope::Facets {
                    ids: vec!["f2".to_owned()],
                },
            }),
            &oauth.binding(),
        )
        .unwrap()
        .unwrap();
    // Auth code issued and pending row carries reconnect_to -> parse fails
    assert!(parse_oauth_downgrade(root).is_err());

    // Revoke target grant before token exchange
    oauth.store.revoke_grant_by_id(&target_grant_2).unwrap();
    let redeem_res = oauth.store.redeem_authorization_code(
        &auth2.code,
        &client.client_id,
        "http://127.0.0.1/callback",
        oauth.binding().canonical(),
        "v2",
        &oauth.binding(),
    );
    assert!(matches!(
        redeem_res,
        Err(crate::oauth::store::OAuthStoreError::InvalidGrant)
    ));
    // After exchange refusal, pending row is dropped and parse succeeds
    assert!(parse_oauth_downgrade(root).is_ok());
    let oauth_raw = fs::read_to_string(root.join("mcp-endpoint/oauth.json")).unwrap();
    assert!(!oauth_raw.contains("reconnect_to"));

    // 4. Pairing expires, and a later oauth write prunes the expired pairing
    let _pairing3 = oauth
        .store
        .generate_pairing_code_with_door_and_reconnect("relay", Some("source:somehex".to_owned()))
        .unwrap();
    assert!(parse_oauth_downgrade(root).is_err());

    // Move clock past pairing expiry (PAIRING_CODE_TTL_SECS = 300)
    crate::oauth::store::set_routine_now(Some(base_now + 301));
    // A later oauth write prunes the expired pairing
    oauth
        .store
        .generate_pairing_code_with_door("relay")
        .unwrap();

    assert!(parse_oauth_downgrade(root).is_ok());
    let oauth_raw = fs::read_to_string(root.join("mcp-endpoint/oauth.json")).unwrap();
    assert!(!oauth_raw.contains("reconnect_to"));
}

#[test]
fn downgrade_hold_evaluation_denial_split() {
    let _guard = ClockGuard;
    let base_now = 1_800_000_000_i64;
    crate::oauth::store::set_routine_now(Some(base_now));

    let journal = tempfile::Builder::new()
        .prefix("solstone-mcp-downgrade-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    let root = journal.path();

    // 1. Direct hold without owner pause
    let conn = "bearer:hold_test_conn";
    crate::permissions::PermissionStore::open(root)
        .set_permission(
            conn,
            crate::permissions::ReadPermission {
                categories: vec!["facets".to_owned()],
                scope: crate::permissions::ReadScope::Facets {
                    ids: vec!["f1".to_owned()],
                },
            },
        )
        .unwrap();
    crate::permissions::PermissionStore::open(root)
        .set_evaluation_only(conn, "hold")
        .unwrap();

    assert!(
        crate::agent_state::is_owner_paused(root, conn)
            .unwrap()
            .is_none()
    );

    // The copied evaluator denies
    assert_eq!(
        evaluate_connection_read(root, conn),
        PermissionDecision::Denied {
            reason: "unenforceable",
        }
    );
    // The live evaluator serves Snapshot
    assert!(matches!(
        crate::permissions::evaluate_connection_read(root, conn),
        crate::permissions::PermissionDecision::Snapshot(_)
    ));

    // 2. Resume that stops after resume_refresh_expiry and clear_owner_pause, leaving hold
    let oauth = crate::oauth::OAuthRuntime::new(root, "https://mcp.test".to_owned());
    let client = oauth
        .store
        .register_client(
            "https://client.split/cimd.json",
            vec!["http://127.0.0.1/callback".to_owned()],
            Some("Client Split".to_owned()),
            "192.0.2.1",
        )
        .unwrap();
    let pairing = oauth
        .store
        .generate_pairing_code_with_door("relay")
        .unwrap();
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(b"vsplit"));
    let tx = oauth
        .store
        .create_transaction_with_generation(
            &client.id,
            "http://127.0.0.1/callback",
            oauth.binding().canonical(),
            "https://mcp.test",
            &challenge,
            "S256",
            None,
            "192.0.2.1",
            oauth.binding().stored_grant_generation(),
        )
        .unwrap();
    let auth = oauth
        .store
        .complete_pairing_with_permission(
            &tx,
            &pairing.code,
            Some(crate::permissions::ReadPermission {
                categories: vec!["facets".to_owned()],
                scope: crate::permissions::ReadScope::Facets {
                    ids: vec!["f2".to_owned()],
                },
            }),
            &oauth.binding(),
        )
        .unwrap();
    let tokens = oauth
        .store
        .redeem_authorization_code(
            &auth.code,
            &client.client_id,
            "http://127.0.0.1/callback",
            oauth.binding().canonical(),
            "vsplit",
            &oauth.binding(),
        )
        .unwrap();
    let grant_id = tokens.token_id;
    let grant_conn = format!("oauth:{grant_id}");

    let expiry = oauth
        .store
        .grant_refresh_expires_at(&grant_id)
        .unwrap()
        .unwrap();
    // Set owner pause + evaluation hold + advance clock past expiry (idle)
    let pause_now = crate::oauth::store::current_time();
    crate::agent_state::set_owner_pause(
        root,
        &grant_conn,
        pause_now,
        &crate::tokens::SystemRandomSource,
    )
    .unwrap();
    crate::permissions::PermissionStore::open(root)
        .set_evaluation_only(&grant_conn, "hold")
        .unwrap();
    crate::oauth::store::set_routine_now(Some(expiry.timestamp() + 10));

    // Stop after resume_refresh_expiry and clear_owner_pause, leaving hold
    oauth.store.resume_refresh_expiry(&grant_id).unwrap();
    crate::agent_state::clear_owner_pause(root, &grant_conn).unwrap();
    // Note: did NOT call set_evaluation_only(grant_conn, "enforce")

    // The copied evaluator denies
    assert_eq!(
        evaluate_connection_read(root, &grant_conn),
        PermissionDecision::Denied {
            reason: "unenforceable",
        }
    );
    // The live evaluator is a Snapshot
    assert!(matches!(
        crate::permissions::evaluate_connection_read(root, &grant_conn),
        crate::permissions::PermissionDecision::Snapshot(_)
    ));
}
