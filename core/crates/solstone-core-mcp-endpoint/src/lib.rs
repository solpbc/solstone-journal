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

#[cfg(not(unix))]
use solstone_core_journal_config::{McpEndpointCapability, mcp_endpoint_capability};

#[cfg(all(unix, any(test, feature = "test-hooks")))]
mod test_seam;
#[cfg(all(test, unix))]
mod tests;
#[cfg(unix)]
mod unix;

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
    #[allow(dead_code)]
    committed: solstone_core_sol_link::committed::CommittedIdentity,
    #[cfg(unix)]
    _keypair: ring::signature::Ed25519KeyPair,
}

#[cfg(all(unix, any(test, feature = "test-hooks")))]
impl McpEndpointOwnerContext {
    #[allow(dead_code)]
    pub(crate) fn test_verifying_key_bytes(&self) -> Vec<u8> {
        use ring::signature::KeyPair as _;

        self._keypair.public_key().as_ref().to_vec()
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
