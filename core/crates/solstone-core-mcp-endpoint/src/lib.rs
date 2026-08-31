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

#[cfg(unix)]
use std::sync::Arc;
#[cfg(unix)]
use tokio::sync::watch;

#[cfg(unix)]
use solstone_core_journal_config::McpEndpointCertificateEnvironment;
#[cfg(unix)]
use solstone_core_journal_io::journal_root::JournalRoot;

#[cfg(not(unix))]
use solstone_core_journal_config::{McpEndpointCapability, mcp_endpoint_capability};

#[cfg(unix)]
#[allow(dead_code)]
mod account_wire;
#[cfg(unix)]
mod audit;
#[cfg(unix)]
mod bridge_carrier;
#[cfg(unix)]
mod bridge_forwarder;
#[cfg(unix)]
mod bridge_pop;
#[cfg(unix)]
mod bridge_session;
#[cfg(unix)]
mod http1;
#[cfg(unix)]
mod jsonrpc;
#[cfg(unix)]
mod permits;
#[cfg(unix)]
mod proxy_preface;
#[cfg(unix)]
mod server;
#[cfg(unix)]
mod service_process;
#[cfg(unix)]
mod session;
#[cfg(all(unix, any(test, feature = "test-hooks")))]
mod test_seam;
#[cfg(all(test, unix, not(feature = "full-tests")))]
mod tests;
#[cfg(unix)]
mod tls;
#[cfg(unix)]
mod tokens;
#[cfg(unix)]
mod tools;
#[cfg(unix)]
mod unix;

#[cfg(unix)]
pub use bridge_carrier::McpBridgeCarrierError;
#[cfg(unix)]
pub use bridge_session::{McpBridgeSession, McpPublicStream};
#[cfg(unix)]
pub use service_process::{McpServiceError, run_native_service_with_hosted_parent};
#[cfg(unix)]
pub use tls::{
    McpEndpointCertificateLifecycleError, McpEndpointTlsService, mcp_endpoint_server_config,
};
#[cfg(unix)]
pub use tokens::{CreatedToken, TokenStore, TokenStoreError, TokenSummary, VerifiedToken};

/// One authenticated bridge session paired with its authorized TLS service.
///
/// The service can be handed to Lane B before the opaque bridge session is
/// consumed by the forwarder. Neither field exposes a hostname or key.
#[cfg(unix)]
pub struct McpEndpointTunnel {
    tls: McpEndpointTlsService,
    session: McpBridgeSession,
}

#[cfg(unix)]
impl McpEndpointTunnel {
    /// Borrow the sole opaque TLS service for the dedicated MCP listener.
    pub fn tls_service(&self) -> &McpEndpointTlsService {
        &self.tls
    }

    /// Transfer the authenticated bridge session to the Lane-A forwarder.
    pub fn into_bridge_session(self) -> McpBridgeSession {
        self.session
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
    #[cfg(unix)]
    {
        unix::bootstrap(journal_root)
    }

    #[cfg(not(unix))]
    {
        let config = solstone_core_journal_config::read_journal_config(journal_root)
            .map_err(|_| McpEndpointBootstrapError::ConfigRead)?;
        match mcp_endpoint_capability(&config).map_err(|_| McpEndpointBootstrapError::Capability)? {
            McpEndpointCapability::Disabled => Ok(None),
            McpEndpointCapability::Enabled => Err(McpEndpointBootstrapError::UnsupportedPlatform),
        }
    }
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
    #[cfg(unix)]
    committed: Arc<solstone_core_sol_link::committed::CommittedIdentity>,
    #[cfg(unix)]
    keypair: Arc<ring::signature::Ed25519KeyPair>,
    #[cfg(unix)]
    journal_root: Arc<JournalRoot>,
    #[cfg(unix)]
    certificate_environment: McpEndpointCertificateEnvironment,
    #[cfg(unix)]
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

#[cfg(unix)]
impl McpEndpointOwnerContext {
    /// Connect one fixed WebPKI-authenticated bridge carrier for this enabled journal.
    ///
    /// The returned carrier remains opaque: callers cannot inspect the account
    /// authority, hostname, proof key, or underlying TLS stream.
    pub async fn connect_mcp_bridge(
        &self,
        shutdown: &mut watch::Receiver<bool>,
    ) -> Result<McpBridgeSession, McpBridgeCarrierError> {
        account_wire::establish_mcp_bridge_carrier(self, shutdown)
            .await?
            .into_session()
    }

    /// Authenticate one bridge generation and derive its matching opaque TLS
    /// service from the same account-authorized hostname binding.
    pub async fn connect_mcp_endpoint_tunnel(
        &self,
        shutdown: &mut watch::Receiver<bool>,
    ) -> Result<McpEndpointTunnel, McpBridgeCarrierError> {
        account_wire::establish_mcp_bridge_carrier(self, shutdown)
            .await?
            .into_tunnel()
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
