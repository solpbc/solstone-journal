// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Bootstrap the journal-local MCP endpoint owner identity.
//!
//! The sole public operation deliberately accepts only a journal path and
//! returns an opaque proof-of-bootstrap handle. Callers cannot inject parsed
//! configuration or link material, and the handle exposes neither signing nor
//! persistence operations.

use std::fmt;
use std::path::Path;

use std::sync::Arc;
use tokio::sync::watch;

use solstone_core_journal_config::McpEndpointCertificateEnvironment;
use solstone_core_journal_io::journal_root::JournalRoot;

#[allow(dead_code)]
mod account_wire;
mod activity;
mod audit;
#[cfg(all(test, feature = "full-tests"))]
mod boundary_tests;
mod bridge_carrier;
mod bridge_forwarder;
mod bridge_pop;
mod bridge_session;
pub mod byo_dns;
pub mod byo_door;
pub mod cloudflare_admission;
mod dispatch;
mod http1;
mod jsonrpc;
pub mod lan_door;
pub mod local_door;
mod oauth;
mod owner_state;
mod owner_web;
mod permissions;
mod permits;
mod proxy_preface;
mod references;
mod registry;
#[cfg(unix)]
mod rlimit;
mod server;
mod service_process;
pub mod serving_epoch;
mod session;
mod signals;
#[cfg(all(unix, any(test, feature = "test-hooks")))]
mod test_seam;
#[cfg(all(test, not(feature = "full-tests")))]
mod tests;
mod tls;
mod tokens;
mod tools;
#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows_state;
#[cfg(windows)]
use windows_state as unix;

pub use activity::{
    ActivityAnchor, ActivityEntry, ActivityPage, ActivityQuery, ActivityReadError,
    MAX_EXAMINED_RECORDS, RecordedOutcome, read_activity, tally,
};
pub use bridge_carrier::McpBridgeCarrierError;
pub use bridge_session::{McpBridgeSession, McpPublicStream};
pub use dispatch::{McpProbeError, run_mcp_probe};
pub use lan_door::{LanDoorRun, LanDoorState, read_lan_door_state, write_lan_door_state};
pub use local_door::{
    LocalDoorRun, LocalDoorState, read_local_door_state, run_local_door_async,
    run_local_door_with_hosted_parent, write_local_door_state,
};
pub use oauth::store::{
    CreatedPairingCode, OAuthClientSummary, OAuthGrantSummary, OAuthStore, OAuthStoreError,
    PairingCodeSummary,
};
pub use owner_state::{McpOwnerState, read_mcp_owner_state};
pub use owner_web::owner_routes;
pub use permissions::{
    ConnectionPermissionRecord, ConnectionReadSnapshot, PermissionDecision, PermissionStore,
    PermissionStoreError, PermissionsFile, ReadPermission, ReadScope, evaluate_connection_read,
    resolve_permission_facet_names,
};
pub use service_process::{McpServiceError, run_native_service_with_hosted_parent};
pub use serving_epoch::{EndpointDoor, EpochCompletion, ServingEpoch};
/// The closed audit vocabulary, re-exported for the owner's CLI.
///
/// ⛔ This is the record *type*, not a reader: `solstone-core-mcp-audit` stays a
/// write-only leaf, and the owner's read lives in this crate beside the closed
/// registry that keeps it away from the wire.
pub use solstone_core_mcp_audit::{
    Outcome as AuditOutcome, PermissionSnapshotRecord, RequestRecord, ResultShape,
    ToolName as AuditToolName,
};
pub use tls::{
    McpEndpointCertificateLifecycleError, McpEndpointTlsService, mcp_endpoint_server_config,
};
pub use tokens::{CreatedToken, TokenStore, TokenStoreError, TokenSummary, VerifiedToken};

/// Scratch root for test journals: `/var/tmp` on Unix, which stays on disk
/// outside the gate's capped tmpfs, and the user's temp directory on Windows.
#[cfg(test)]
pub(crate) fn test_scratch() -> std::path::PathBuf {
    #[cfg(unix)]
    {
        std::path::PathBuf::from("/var/tmp")
    }
    #[cfg(windows)]
    {
        std::env::temp_dir()
    }
}

/// One authenticated bridge session paired with its authorized TLS service.
///
/// The service can be handed to Lane B before the opaque bridge session is
/// consumed by the forwarder. Neither field exposes a hostname or key.
pub struct McpEndpointTunnel {
    tls: McpEndpointTlsService,
    session: McpBridgeSession,
}

impl McpEndpointTunnel {
    /// Borrow the sole opaque TLS service for the dedicated MCP listener.
    pub fn tls_service(&self) -> &McpEndpointTlsService {
        &self.tls
    }

    /// Transfer the authenticated bridge session to the Lane-A forwarder.
    pub fn into_bridge_session(self) -> McpBridgeSession {
        self.session
    }

    /// Transfer the paired TLS owner and authenticated session to the native
    /// service composition without widening either capability publicly.
    pub(crate) fn into_service_parts(self) -> (McpEndpointTlsService, McpBridgeSession) {
        (self.tls, self.session)
    }
}

/// Bootstrap the committed owner identity and durable Ed25519 proof-of-possession key.
///
/// ```compile_fail,E0308
/// use solstone_core_journal_config::JournalConfigRead;
/// use solstone_core_mcp_endpoint::bootstrap_mcp_endpoint_owner_identity;
///
/// let read = JournalConfigRead {
///     present: false,
///     sha256: None,
///     config: None,
/// };
/// let _ = bootstrap_mcp_endpoint_owner_identity(&read);
/// ```
///
/// ```compile_fail,E0308
/// use solstone_core_journal_config::McpEndpointCapability;
/// use solstone_core_mcp_endpoint::bootstrap_mcp_endpoint_owner_identity;
///
/// let _ = bootstrap_mcp_endpoint_owner_identity(&McpEndpointCapability::Enabled);
/// ```
pub fn bootstrap_mcp_endpoint_owner_identity(
    journal_root: &Path,
) -> Result<Option<McpEndpointOwnerContext>, McpEndpointBootstrapError> {
    unix::bootstrap(journal_root)
}

/// A successfully admitted committed owner identity and private Ed25519 PoP key.
///
/// ```compile_fail,E0603
/// use solstone_core_mcp_endpoint::account_wire;
/// ```
///
/// ```compile_fail,E0603
/// use solstone_core_mcp_endpoint::account_wire::build_account_registration_request;
/// ```
///
/// ```compile_fail,E0603
/// use solstone_core_mcp_endpoint::account_wire::{McpAccountRequest, McpAccountWireError};
/// ```
///
/// ```compile_fail,E0603
/// use solstone_core_mcp_endpoint::account_wire::parse_account_registration_response;
/// ```
///
/// ```compile_fail,E0603
/// use solstone_core_mcp_endpoint::account_wire::McpAccountResponseWire;
/// ```
///
/// ```compile_fail,E0603
/// use solstone_core_mcp_endpoint::account_wire::McpAccountResponseWireError;
/// ```
///
/// ```compile_fail,E0432
/// use solstone_core_mcp_endpoint::CommittedIdentity;
/// ```
///
/// ```compile_fail,E0432
/// use solstone_core_mcp_endpoint::LocalCa;
/// ```
///
/// ```compile_fail,E0451
/// use solstone_core_mcp_endpoint::McpEndpointOwnerContext;
///
/// let _ = McpEndpointOwnerContext { _private: () };
/// ```
///
/// ```compile_fail,E0599
/// use solstone_core_mcp_endpoint::McpEndpointOwnerContext;
///
/// fn cannot_sign(context: &McpEndpointOwnerContext) {
///     let _ = context.sign(b"caller supplied message");
/// }
/// ```
///
/// ```compile_fail,E0599
/// use solstone_core_mcp_endpoint::McpEndpointOwnerContext;
///
/// fn cannot_verify(context: &McpEndpointOwnerContext) {
///     let _ = context.verify(b"message", b"signature");
/// }
/// ```
///
/// ```compile_fail,E0599
/// use solstone_core_mcp_endpoint::McpEndpointOwnerContext;
///
/// fn cannot_reach_storage(context: &McpEndpointOwnerContext) {
///     let _ = context.persistence_path();
///     let _ = context.ca();
/// }
/// ```
///
/// ```compile_fail,E0308
/// use solstone_core_mcp_endpoint::McpEndpointOwnerContext;
///
/// fn cannot_clone(context: McpEndpointOwnerContext) {
///     let _: McpEndpointOwnerContext = context.clone();
/// }
/// ```
///
/// ```compile_fail,E0277
/// use solstone_core_mcp_endpoint::McpEndpointOwnerContext;
///
/// fn cannot_debug(context: &McpEndpointOwnerContext) {
///     let _ = format!("{context:?}");
/// }
/// ```
///
/// ```compile_fail,E0277
/// use solstone_core_mcp_endpoint::McpEndpointOwnerContext;
///
/// fn require_serialize<T: serde::Serialize>(_value: T) {}
///
/// fn cannot_serialize(context: McpEndpointOwnerContext) {
///     require_serialize(context);
/// }
/// ```
pub struct McpEndpointOwnerContext {
    _private: (),
    committed: Arc<solstone_core_sol_link::committed::CommittedIdentity>,
    keypair: Arc<ring::signature::Ed25519KeyPair>,
    journal_root: Arc<JournalRoot>,
    certificate_environment: McpEndpointCertificateEnvironment,
    force_staging_renewal: bool,
}

#[cfg(all(unix, any(test, feature = "test-hooks")))]
impl McpEndpointOwnerContext {
    #[allow(dead_code)]
    pub(crate) fn test_verifying_key_bytes(&self) -> Vec<u8> {
        use ring::signature::KeyPair as _;

        self.keypair.public_key().as_ref().to_vec()
    }
}

impl McpEndpointOwnerContext {
    /// Connect one fixed WebPKI-authenticated bridge carrier for this enabled journal.
    ///
    /// The returned carrier remains opaque: callers cannot inspect the account
    /// authority, hostname, proof key, or underlying TLS stream.
    pub async fn connect_mcp_bridge(
        &self,
        shutdown: &mut watch::Receiver<bool>,
    ) -> Result<McpBridgeSession, McpBridgeCarrierError> {
        self.connect_mcp_bridge_with_epoch(
            shutdown,
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )
        .await
    }

    pub async fn connect_mcp_bridge_with_epoch(
        &self,
        shutdown: &mut watch::Receiver<bool>,
        epoch_closed: Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<McpBridgeSession, McpBridgeCarrierError> {
        account_wire::establish_mcp_bridge_carrier(self, None, shutdown)
            .await?
            .into_session(epoch_closed)
    }

    /// Reconnect a bridge carrier only when it is authorized for the existing
    /// endpoint TLS service.
    ///
    /// The native service keeps its listener and certificate state across a
    /// recoverable carrier loss.  A later account registration is therefore
    /// admitted only if it names the exact hostname already bound to that TLS
    /// service; a changed hostname must fail closed instead of being forwarded
    /// through the old certificate.
    pub(crate) async fn connect_mcp_bridge_for_tls(
        &self,
        tls: &McpEndpointTlsService,
        shutdown: &mut watch::Receiver<bool>,
        epoch_closed: Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<McpBridgeSession, McpBridgeCarrierError> {
        account_wire::establish_mcp_bridge_carrier(self, Some(tls), shutdown)
            .await?
            .into_session(epoch_closed)
    }

    /// Authenticate one bridge generation and derive its matching opaque TLS
    /// service from the same account-authorized hostname binding.
    pub async fn connect_mcp_endpoint_tunnel(
        &self,
        shutdown: &mut watch::Receiver<bool>,
    ) -> Result<McpEndpointTunnel, McpBridgeCarrierError> {
        self.connect_mcp_endpoint_tunnel_with_epoch(
            shutdown,
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )
        .await
    }

    pub async fn connect_mcp_endpoint_tunnel_with_epoch(
        &self,
        shutdown: &mut watch::Receiver<bool>,
        epoch_closed: Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<McpEndpointTunnel, McpBridgeCarrierError> {
        account_wire::establish_mcp_bridge_carrier(self, None, shutdown)
            .await?
            .into_tunnel(epoch_closed)
    }

    /// Keep the authenticated bridge tunnel connected and forward only its
    /// bridge-opened public streams to the fixed journal-local MCP listener.
    ///
    /// This never creates a listener or changes the capability gate. The
    /// caller owns supervision and supplies its one shutdown signal.
    pub async fn run_mcp_bridge_forwarder(
        &self,
        shutdown: &mut watch::Receiver<bool>,
    ) -> Result<(), McpBridgeCarrierError> {
        bridge_forwarder::run(self, shutdown).await
    }

    pub(crate) fn renewal_owner(&self) -> Self {
        Self {
            _private: (),
            committed: Arc::clone(&self.committed),
            keypair: Arc::clone(&self.keypair),
            journal_root: Arc::clone(&self.journal_root),
            certificate_environment: self.certificate_environment,
            force_staging_renewal: self.force_staging_renewal,
        }
    }

    pub(crate) fn proof_keypair(&self) -> Arc<ring::signature::Ed25519KeyPair> {
        Arc::clone(&self.keypair)
    }

    pub(crate) fn tls_service_for(
        &self,
        hostname: String,
    ) -> Result<McpEndpointTlsService, McpBridgeCarrierError> {
        tls::McpEndpointTlsService::for_authorized_hostname(
            Arc::clone(&self.journal_root),
            hostname,
            self.certificate_environment,
            self.force_staging_renewal,
        )
        .map_err(|_| McpBridgeCarrierError::State)
    }

    pub(crate) fn journal_path(&self) -> &Path {
        self.journal_root.canonical_path()
    }
}

/// Bootstrap failure category, intentionally without filesystem or key material.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum McpEndpointBootstrapError {
    /// The journal configuration could not be read or parsed.
    ConfigRead,
    /// `mcp_endpoint.enabled` was not a boolean.
    Capability,
    /// The enabled endpoint has no supported platform backend.
    UnsupportedPlatform,
    /// Committed identity, endpoint ownership, persistence, or key validation failed.
    Endpoint,
}

impl fmt::Display for McpEndpointBootstrapError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ConfigRead => "MCP endpoint configuration could not be read",
            Self::Capability => "MCP endpoint capability is invalid",
            Self::UnsupportedPlatform => "MCP endpoint is unsupported on this platform",
            Self::Endpoint => "MCP endpoint owner bootstrap failed",
        })
    }
}

impl std::error::Error for McpEndpointBootstrapError {}
