// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Process-local, one-shot capabilities for relay-paired Door carriers.

use std::{
    collections::HashMap,
    fmt,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, Weak},
};

/// The exact relay pairing nonce admitted for one Door carrier.
///
/// This intentionally implements no [`std::fmt::Display`].
#[derive(Clone, Eq, PartialEq)]
pub(crate) struct RelayNonceIdentity(String);

impl RelayNonceIdentity {
    pub(crate) fn new(nonce_value: String) -> Self {
        Self(nonce_value)
    }

    pub(crate) fn matches(&self, nonce_value: &str) -> bool {
        self.0 == nonce_value
    }

    pub(crate) fn nonce_value(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for RelayNonceIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RelayNonceIdentity([REDACTED])")
    }
}

/// One-shot Door admission capabilities installed by the trusted relay bridge.
///
/// A map entry is a capability for one pre-bound local socket address, not an
/// identity claim inferred from a loopback peer. The bridge reserves that
/// exact address before connecting Door; merely originating from `127.0.0.1`
/// never grants relay admission.
pub(crate) struct RelayAdmissionRegistry {
    entries: Mutex<HashMap<SocketAddr, RelayNonceIdentity>>,
}

impl RelayAdmissionRegistry {
    pub(crate) fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn insert(
        self: &Arc<Self>,
        local_addr: SocketAddr,
        identity: RelayNonceIdentity,
    ) -> RelayAdmissionLease {
        lock_entries(&self.entries).insert(local_addr, identity);
        RelayAdmissionLease {
            registry: Arc::clone(self),
            local_addr,
        }
    }

    /// Consume the capability for exactly one accepted socket.
    pub(crate) fn take(&self, local_addr: SocketAddr) -> Option<RelayNonceIdentity> {
        lock_entries(&self.entries).remove(&local_addr)
    }

    pub(crate) fn remove(&self, local_addr: SocketAddr) {
        lock_entries(&self.entries).remove(&local_addr);
    }

    pub(crate) fn remove_for_nonce(&self, nonce_value: &str) {
        lock_entries(&self.entries).retain(|_, identity| !identity.matches(nonce_value));
    }

    pub(crate) fn clear(&self) {
        lock_entries(&self.entries).clear();
    }
}

impl fmt::Debug for RelayAdmissionRegistry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RelayAdmissionRegistry")
            .finish_non_exhaustive()
    }
}

/// Removes its address capability when the bridge finishes or fails.
pub(crate) struct RelayAdmissionLease {
    registry: Arc<RelayAdmissionRegistry>,
    local_addr: SocketAddr,
}

impl Drop for RelayAdmissionLease {
    fn drop(&mut self) {
        self.registry.remove(self.local_addr);
    }
}

static REGISTRIES: OnceLock<Mutex<HashMap<PathBuf, Weak<RelayAdmissionRegistry>>>> =
    OnceLock::new();

/// Return the process-local registry for this canonical journal root.
///
/// A root that cannot be canonicalized receives an isolated registry. Calls
/// for that root cannot share a capability across the router and Door, so
/// relay admission fails closed instead of trusting a non-canonical path.
pub(crate) fn admission_registry_for(journal_root: &Path) -> Arc<RelayAdmissionRegistry> {
    let Ok(journal_key) = journal_root.canonicalize() else {
        return Arc::new(RelayAdmissionRegistry::new());
    };
    let registries = REGISTRIES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut registries = registries
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    registries.retain(|_, registry| registry.strong_count() != 0);
    if let Some(registry) = registries.get(&journal_key).and_then(Weak::upgrade) {
        return registry;
    }
    let registry = Arc::new(RelayAdmissionRegistry::new());
    registries.insert(journal_key, Arc::downgrade(&registry));
    registry
}

fn lock_entries(
    entries: &Mutex<HashMap<SocketAddr, RelayNonceIdentity>>,
) -> std::sync::MutexGuard<'_, HashMap<SocketAddr, RelayNonceIdentity>> {
    entries
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use std::{net::SocketAddr, sync::Arc};

    use super::{RelayAdmissionRegistry, RelayNonceIdentity};

    #[test]
    fn take_is_one_shot_and_lease_cleanup_is_idempotent() {
        let registry = Arc::new(RelayAdmissionRegistry::new());
        let address: SocketAddr = "127.0.0.1:4444".parse().expect("socket address");
        let lease = registry.insert(address, RelayNonceIdentity::new("nonce-a".to_owned()));

        assert!(
            registry
                .take(address)
                .is_some_and(|identity| identity.matches("nonce-a"))
        );
        assert!(registry.take(address).is_none());
        drop(lease);
        assert!(registry.take(address).is_none());
    }

    #[test]
    fn clearing_by_nonce_preserves_other_capabilities() {
        let registry = Arc::new(RelayAdmissionRegistry::new());
        let first: SocketAddr = "127.0.0.1:4444".parse().expect("first address");
        let second: SocketAddr = "127.0.0.1:4445".parse().expect("second address");
        let _first = registry.insert(first, RelayNonceIdentity::new("nonce-a".to_owned()));
        let _second = registry.insert(second, RelayNonceIdentity::new("nonce-b".to_owned()));

        registry.remove_for_nonce("nonce-a");

        assert!(registry.take(first).is_none());
        assert!(
            registry
                .take(second)
                .is_some_and(|identity| identity.matches("nonce-b"))
        );
    }
}
