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
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DoorAvailability {
    port: u16,
    generation: u64,
}

pub(crate) enum RelayAdmissionClaim {
    Current(RelayNonceIdentity),
    Stale(RelayNonceIdentity),
}

struct RelayAdmissionEntry {
    identity: RelayNonceIdentity,
    door: DoorAvailability,
    revoked: bool,
}

impl DoorAvailability {
    pub(crate) fn port(self) -> u16 {
        self.port
    }
}

#[derive(Default)]
struct DoorState {
    port: Option<u16>,
    generation: u64,
}

pub(crate) struct RelayAdmissionRegistry {
    entries: Mutex<HashMap<SocketAddr, RelayAdmissionEntry>>,
    door: Mutex<DoorState>,
}

impl RelayAdmissionRegistry {
    pub(crate) fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            door: Mutex::new(DoorState::default()),
        }
    }

    pub(crate) fn set_door_port(&self, port: u16) {
        let mut door = lock_door(&self.door);
        door.generation = door.generation.saturating_add(1);
        door.port = Some(port);
    }

    pub(crate) fn door_availability(&self) -> Option<DoorAvailability> {
        let door = lock_door(&self.door);
        door.port.map(|port| DoorAvailability {
            port,
            generation: door.generation,
        })
    }

    pub(crate) fn is_current(&self, availability: DoorAvailability) -> bool {
        self.door_availability() == Some(availability)
    }

    pub(crate) fn while_current<T>(
        &self,
        availability: DoorAvailability,
        operation: impl FnOnce() -> T,
    ) -> Option<T> {
        let door = lock_door(&self.door);
        (door.port == Some(availability.port) && door.generation == availability.generation)
            .then(operation)
    }

    pub(crate) fn insert_while_current(
        self: &Arc<Self>,
        local_addr: SocketAddr,
        identity: RelayNonceIdentity,
        availability: DoorAvailability,
    ) -> Option<RelayAdmissionLease> {
        let door = lock_door(&self.door);
        if door.port != Some(availability.port) || door.generation != availability.generation {
            return None;
        }
        let lease_identity = identity.clone();
        lock_entries(&self.entries).insert(
            local_addr,
            RelayAdmissionEntry {
                identity,
                door: availability,
                revoked: false,
            },
        );
        Some(RelayAdmissionLease {
            registry: Arc::clone(self),
            local_addr,
            identity: lease_identity,
            door: availability,
        })
    }

    /// Consume the capability for exactly one accepted socket.
    pub(crate) fn take(&self, local_addr: SocketAddr) -> Option<RelayAdmissionClaim> {
        let entry = lock_entries(&self.entries).remove(&local_addr)?;
        Some(if !entry.revoked && self.is_current(entry.door) {
            RelayAdmissionClaim::Current(entry.identity)
        } else {
            RelayAdmissionClaim::Stale(entry.identity)
        })
    }

    fn remove_if_owned(
        &self,
        local_addr: SocketAddr,
        identity: &RelayNonceIdentity,
        door: DoorAvailability,
    ) {
        let mut entries = lock_entries(&self.entries);
        if let Some(entry) = entries.get_mut(&local_addr)
            && entry.identity == *identity
            && entry.door == door
        {
            entry.revoked = true;
        }
    }

    pub(crate) fn remove_for_nonce(&self, nonce_value: &str) {
        for entry in lock_entries(&self.entries).values_mut() {
            if entry.identity.matches(nonce_value) {
                entry.revoked = true;
            }
        }
    }

    /// Stop new bridges from dialing Door without changing the type of an
    /// already-connected relay carrier. Its address capability remains live
    /// until Door consumes it or the owning bridge lease drops.
    pub(crate) fn clear_door_port(&self) {
        let mut door = lock_door(&self.door);
        door.generation = door.generation.saturating_add(1);
        door.port = None;
    }

    pub(crate) fn clear_admissions(&self) {
        for entry in lock_entries(&self.entries).values_mut() {
            entry.revoked = true;
        }
    }

    pub(crate) fn clear(&self) {
        lock_entries(&self.entries).clear();
        self.clear_door_port();
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
    identity: RelayNonceIdentity,
    door: DoorAvailability,
}

impl Drop for RelayAdmissionLease {
    fn drop(&mut self) {
        self.registry
            .remove_if_owned(self.local_addr, &self.identity, self.door);
    }
}

static REGISTRIES: OnceLock<Mutex<HashMap<PathBuf, Weak<RelayAdmissionRegistry>>>> =
    OnceLock::new();

/// Return the process-local registry for this journal root.
///
/// A root that cannot be canonicalized is keyed by its raw path. This shares
/// capabilities only between callers that supplied the same path while the
/// journal root does not yet exist; distinct raw paths remain distinct.
pub(crate) fn admission_registry_for(journal_root: &Path) -> Arc<RelayAdmissionRegistry> {
    let journal_key = journal_root
        .canonicalize()
        .unwrap_or_else(|_| journal_root.to_path_buf());
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
    entries: &Mutex<HashMap<SocketAddr, RelayAdmissionEntry>>,
) -> std::sync::MutexGuard<'_, HashMap<SocketAddr, RelayAdmissionEntry>> {
    entries
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn lock_door(door: &Mutex<DoorState>) -> std::sync::MutexGuard<'_, DoorState> {
    door.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use std::{net::SocketAddr, sync::Arc};

    use super::{
        DoorAvailability, RelayAdmissionClaim, RelayAdmissionRegistry, RelayNonceIdentity,
        admission_registry_for,
    };

    #[test]
    fn take_is_one_shot_and_lease_cleanup_is_idempotent() {
        let registry = Arc::new(RelayAdmissionRegistry::new());
        let address: SocketAddr = "127.0.0.1:4444".parse().expect("socket address");
        registry.set_door_port(47_657);
        let door = registry.door_availability().expect("Door availability");
        let lease = registry
            .insert_while_current(address, RelayNonceIdentity::new("nonce-a".to_owned()), door)
            .expect("current admission");

        assert!(
            registry
                .take(address)
                .is_some_and(|claim| matches!(claim, RelayAdmissionClaim::Current(identity) if identity.matches("nonce-a")))
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
        registry.set_door_port(47_657);
        let door = registry.door_availability().expect("Door availability");
        let _first = registry
            .insert_while_current(first, RelayNonceIdentity::new("nonce-a".to_owned()), door)
            .expect("first admission");
        let _second = registry
            .insert_while_current(second, RelayNonceIdentity::new("nonce-b".to_owned()), door)
            .expect("second admission");

        registry.remove_for_nonce("nonce-a");

        assert!(matches!(
            registry.take(first),
            Some(RelayAdmissionClaim::Stale(_))
        ));
        assert!(
            registry
                .take(second)
                .is_some_and(|claim| matches!(claim, RelayAdmissionClaim::Current(identity) if identity.matches("nonce-b")))
        );
    }

    #[test]
    fn nonexistent_root_reuses_its_raw_path_registry() {
        let temporary = tempfile::TempDir::new_in("/var/tmp").expect("temporary parent");
        let root = temporary.path().join("journal-not-created-yet");
        assert!(!root.exists(), "fixture root is intentionally absent");
        let first = admission_registry_for(&root);
        let second = admission_registry_for(&root);
        let address: SocketAddr = "127.0.0.1:4444".parse().expect("socket address");
        first.set_door_port(47_657);
        let door = first.door_availability().expect("Door availability");
        let lease = first
            .insert_while_current(address, RelayNonceIdentity::new("nonce-a".to_owned()), door)
            .expect("current admission");

        assert!(
            second
                .take(address)
                .is_some_and(|claim| matches!(claim, RelayAdmissionClaim::Current(identity) if identity.matches("nonce-a")))
        );
        drop(lease);
    }

    #[test]
    fn clearing_door_port_keeps_existing_capability_typed_but_stale() {
        let registry = Arc::new(RelayAdmissionRegistry::new());
        let address: SocketAddr = "127.0.0.1:4444".parse().expect("socket address");
        assert_eq!(registry.door_availability(), None);
        registry.set_door_port(47_657);
        let door = registry.door_availability().expect("Door availability");
        let lease = registry
            .insert_while_current(address, RelayNonceIdentity::new("nonce-a".to_owned()), door)
            .expect("current admission");
        assert_eq!(
            registry.door_availability().map(DoorAvailability::port),
            Some(47_657)
        );
        registry.clear_door_port();
        assert_eq!(registry.door_availability(), None);
        assert!(
            matches!(registry.take(address), Some(RelayAdmissionClaim::Stale(_))),
            "Door shutdown must keep the trusted relay source typed and fail closed"
        );
        drop(lease);
    }

    #[test]
    fn clearing_all_admissions_also_unpublishes_door() {
        let registry = Arc::new(RelayAdmissionRegistry::new());
        let address: SocketAddr = "127.0.0.1:4444".parse().expect("socket address");
        registry.set_door_port(47_657);
        let door = registry.door_availability().expect("Door availability");
        let _lease = registry
            .insert_while_current(address, RelayNonceIdentity::new("nonce-a".to_owned()), door)
            .expect("current admission");

        registry.clear();

        assert_eq!(registry.door_availability(), None);
        assert!(registry.take(address).is_none());
    }

    #[test]
    fn clearing_admissions_preserves_the_published_door_generation() {
        let registry = Arc::new(RelayAdmissionRegistry::new());
        registry.set_door_port(47_657);
        let door = registry.door_availability().expect("Door availability");
        let address: SocketAddr = "127.0.0.1:4444".parse().expect("socket address");
        let _lease = registry
            .insert_while_current(address, RelayNonceIdentity::new("nonce-a".to_owned()), door)
            .expect("current admission");

        registry.clear_admissions();

        assert!(registry.is_current(door));
        assert!(matches!(
            registry.take(address),
            Some(RelayAdmissionClaim::Stale(_))
        ));
    }

    #[test]
    fn republishing_the_same_port_invalidates_the_prior_generation() {
        let registry = Arc::new(RelayAdmissionRegistry::new());
        registry.set_door_port(47_657);
        let first = registry.door_availability().expect("first Door generation");
        let stale_address: SocketAddr = "127.0.0.1:4444".parse().expect("stale address");
        let _stale_lease = registry
            .insert_while_current(
                stale_address,
                RelayNonceIdentity::new("nonce-a".to_owned()),
                first,
            )
            .expect("first-generation admission");

        registry.clear_door_port();
        registry.set_door_port(47_657);

        assert!(!registry.is_current(first));
        assert_eq!(
            registry.door_availability().map(DoorAvailability::port),
            Some(47_657)
        );
        assert!(
            matches!(
                registry.take(stale_address),
                Some(RelayAdmissionClaim::Stale(_))
            ),
            "a bridge admitted for the old generation cannot authenticate to a same-port replacement"
        );
        assert!(
            registry
                .insert_while_current(
                    "127.0.0.1:4445".parse().expect("late address"),
                    RelayNonceIdentity::new("nonce-b".to_owned()),
                    first,
                )
                .is_none(),
            "the pre-dial callback rejects a stale generation"
        );
    }
}
