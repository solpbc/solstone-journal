// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Windows counterpart of the endpoint's owner-only state layer.
//!
//! The Unix layer binds every private key and certificate read and write to a
//! retained directory descriptor and checks owner, mode and identity at each
//! step. Windows has no equivalent of that layer yet, so every operation here
//! refuses. The consequences are deliberate: an enabled operated endpoint
//! refuses to bootstrap, and no hostname or certificate state is read or
//! written. The owner-hostname door keeps its keys here too; its runtime is not
//! compiled for Windows at all. The loopback and LAN doors keep no state
//! behind this layer and are unaffected.

use std::io;
use std::path::Path;

use solstone_core_journal_config::{
    McpEndpointCapability, mcp_endpoint_capability, read_journal_config,
};
use solstone_core_journal_io::journal_root::JournalRoot;

use crate::{McpEndpointBootstrapError, McpEndpointOwnerContext};

pub(crate) const MAX_TLS_STATE_BYTES: usize = 256 * 1024;
pub(crate) const MAX_TLS_ACME_ACCOUNT_BYTES: usize = 1024;
pub(crate) const MAX_TLS_ACME_URL_BYTES: usize = 512;
pub(crate) const MAX_TLS_REPLACE_INTENT_BYTES: usize = 512;
pub(crate) const MAX_TLS_ACCOUNT_POSTURE_BYTES: usize = 1024;
pub(crate) const MAX_TLS_ACCOUNT_REPLACED_BYTES: usize = 1024;

/// Never constructed on Windows: the directory cannot be opened.
pub(crate) enum TlsStateDirectory {}

fn unsupported() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "endpoint owner state is not available on this platform",
    )
}

pub(super) fn bootstrap(
    journal_root: &Path,
) -> Result<Option<McpEndpointOwnerContext>, McpEndpointBootstrapError> {
    let config =
        read_journal_config(journal_root).map_err(|_| McpEndpointBootstrapError::ConfigRead)?;
    match mcp_endpoint_capability(&config).map_err(|_| McpEndpointBootstrapError::Capability)? {
        McpEndpointCapability::Disabled => Ok(None),
        McpEndpointCapability::Enabled => Err(McpEndpointBootstrapError::UnsupportedPlatform),
    }
}

pub(crate) fn open_tls_state_directory(_root: &JournalRoot) -> io::Result<TlsStateDirectory> {
    Err(unsupported())
}

pub(crate) fn read_tls_state_bytes(directory: &TlsStateDirectory) -> io::Result<Option<Vec<u8>>> {
    match *directory {}
}

pub(crate) fn persist_tls_state_bytes(
    directory: &TlsStateDirectory,
    _bytes: &[u8],
) -> io::Result<()> {
    match *directory {}
}

pub(crate) fn read_tls_acme_account_bytes(
    directory: &TlsStateDirectory,
    _production: bool,
) -> io::Result<Option<Vec<u8>>> {
    match *directory {}
}

pub(crate) fn persist_tls_acme_account_bytes(
    directory: &TlsStateDirectory,
    _production: bool,
    _bytes: &[u8],
) -> io::Result<()> {
    match *directory {}
}

pub(crate) fn read_tls_acme_account_url_bytes(
    directory: &TlsStateDirectory,
    _production: bool,
) -> io::Result<Option<Vec<u8>>> {
    match *directory {}
}

pub(crate) fn persist_tls_acme_account_url_bytes(
    directory: &TlsStateDirectory,
    _production: bool,
    _bytes: &[u8],
) -> io::Result<()> {
    match *directory {}
}

pub(crate) fn rename_canonical_pk8_to_aside(
    directory: &TlsStateDirectory,
    _production: bool,
    _timestamp_secs: i64,
) -> io::Result<()> {
    match *directory {}
}

pub(crate) fn persist_tls_replace_intent_bytes(
    directory: &TlsStateDirectory,
    _bytes: &[u8],
) -> io::Result<()> {
    match *directory {}
}

pub(crate) fn read_tls_replace_intent_bytes(
    directory: &TlsStateDirectory,
) -> io::Result<Option<Vec<u8>>> {
    match *directory {}
}

pub(crate) fn read_tls_account_posture_bytes(
    directory: &TlsStateDirectory,
) -> io::Result<Option<Vec<u8>>> {
    match *directory {}
}

pub(crate) fn persist_tls_account_posture_bytes(
    directory: &TlsStateDirectory,
    _bytes: &[u8],
) -> io::Result<()> {
    match *directory {}
}

pub(crate) fn delete_tls_account_posture(directory: &TlsStateDirectory) -> io::Result<()> {
    match *directory {}
}

pub(crate) fn read_tls_account_replaced_bytes(
    directory: &TlsStateDirectory,
) -> io::Result<Option<Vec<u8>>> {
    match *directory {}
}

pub(crate) fn persist_tls_account_replaced_bytes(
    directory: &TlsStateDirectory,
    _bytes: &[u8],
) -> io::Result<()> {
    match *directory {}
}

pub(crate) fn delete_tls_account_replaced_bytes(directory: &TlsStateDirectory) -> io::Result<()> {
    match *directory {}
}
