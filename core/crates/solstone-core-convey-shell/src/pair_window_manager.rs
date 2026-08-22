// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Convey-owned lifetime management for relay pairing windows.

use std::{
    collections::HashMap,
    future::Future,
    net::{Ipv4Addr, SocketAddrV4},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use solstone_core_sol_link::pairing::nonces::{NonceStore, NonceStoreError};
use solstone_core_sol_link::pairing::{
    MintRequest, MintResponse, PairingError, commit_relay_pairing, mint_relay_pairing_draft,
};
use solstone_core_spl::{
    LinkServiceTokenRead, PairWindowClientError, PairWindowRegistration, PairWindowSecret,
    RelayPairKey, ServiceToken, attach_pair_window_tunnel, bridge_pair_window_tunnel,
    load_link_service_token, register_pair_window, relay_url,
};
use tokio::{sync::oneshot, task::JoinHandle, time::timeout};

use crate::relay_admission::{DoorAvailability, RelayAdmissionRegistry, RelayNonceIdentity};

/// Process-local relay-window registrations, keyed by their local nonce value.
pub(crate) struct PairWindowManager {
    windows: Arc<Mutex<HashMap<String, JoinHandle<()>>>>,
    relay_admissions: Arc<RelayAdmissionRegistry>,
}

struct PairWindowTask {
    journal_root: PathBuf,
    relay_origin: String,
    service_token: ServiceToken,
    relay_key: RelayPairKey,
    relay_admissions: Arc<RelayAdmissionRegistry>,
    door: DoorAvailability,
    nonce_value: String,
    expires_at: i64,
    registration: PairWindowRegistration,
}

impl PairWindowManager {
    pub(crate) fn new(relay_admissions: Arc<RelayAdmissionRegistry>) -> Self {
        Self {
            windows: Arc::new(Mutex::new(HashMap::new())),
            relay_admissions,
        }
    }

    /// Register a relay window, commit its nonce, and return its v06 link.
    ///
    /// The completed registration remains live in a background task. That task
    /// waits for the single incoming offer, attaches its tunnel, and bridges it
    /// into the local Door.
    pub(crate) async fn mint_and_register(
        &self,
        journal_root: &Path,
        request: &MintRequest,
        now: i64,
    ) -> Result<MintResponse, PairingError> {
        self.with_available_door(|door| {
            self.mint_and_register_after_door_check(journal_root, request, now, door)
        })
        .await
    }

    async fn with_available_door<T, Operation, OperationFuture>(
        &self,
        operation: Operation,
    ) -> Result<T, PairingError>
    where
        Operation: FnOnce(DoorAvailability) -> OperationFuture,
        OperationFuture: Future<Output = Result<T, PairingError>>,
    {
        let door = self
            .relay_admissions
            .door_availability()
            .ok_or(PairingError::RelayPairingUnavailable)?;
        operation(door).await
    }

    async fn register_while_current<Registration, Register, RegisterFuture, Close, CloseFuture>(
        &self,
        door: DoorAvailability,
        register: Register,
        close: Close,
    ) -> Result<Registration, PairingError>
    where
        Register: FnOnce() -> RegisterFuture,
        RegisterFuture: Future<Output = Result<Registration, PairingError>>,
        Close: FnOnce(Registration) -> CloseFuture,
        CloseFuture: Future<Output = ()>,
    {
        let registration = register().await?;
        if !self.relay_admissions.is_current(door) {
            close(registration).await;
            return Err(PairingError::RelayPairingUnavailable);
        }
        Ok(registration)
    }

    async fn mint_and_register_after_door_check(
        &self,
        journal_root: &Path,
        request: &MintRequest,
        now: i64,
        door: DoorAvailability,
    ) -> Result<MintResponse, PairingError> {
        let relay_origin = relay_url(journal_root);
        let service_token = service_token(journal_root)?;
        let draft = mint_relay_pairing_draft(journal_root, request, &relay_origin)?;
        let relay_secret = PairWindowSecret::from(draft.secret_bytes());
        let relay_key = relay_secret.relay_key();
        let registration = self
            .register_while_current(
                door,
                || async {
                    register_pair_window(&relay_origin, &service_token, &relay_key)
                        .await
                        .map_err(registration_error)
                },
                |registration: PairWindowRegistration| async move {
                    let _ = registration.close().await;
                },
            )
            .await?;

        let nonce = match self.relay_admissions.while_current(door, || {
            commit_relay_pairing(
                journal_root,
                draft.secret_hex(),
                draft.device_label(),
                draft.role(),
                now,
            )
        }) {
            Some(Ok(nonce)) => nonce,
            Some(Err(_)) => {
                let _ = registration.close().await;
                return Err(PairingError::RelayPairingNonceCommit);
            }
            None => {
                let _ = registration.close().await;
                return Err(PairingError::RelayPairingUnavailable);
            }
        };
        let response = draft.response();
        let nonce_value = draft.secret_hex().to_owned();
        self.spawn_window(PairWindowTask {
            journal_root: journal_root.to_path_buf(),
            relay_origin,
            service_token,
            relay_key,
            relay_admissions: Arc::clone(&self.relay_admissions),
            door,
            nonce_value,
            expires_at: nonce.expires_at,
            registration,
        });
        Ok(response)
    }

    /// Cancel every registered window and remove only live relay nonce authority.
    pub(crate) async fn retire_all(
        &self,
        journal_root: &Path,
        now: i64,
    ) -> Result<(), PairingError> {
        let tasks = {
            let mut windows = lock_windows(&self.windows);
            windows.drain().map(|(_, task)| task).collect::<Vec<_>>()
        };
        for task in tasks {
            task.abort();
        }
        self.relay_admissions.clear_admissions();
        NonceStore::new(journal_root)
            .cancel_all_relay_windows(now)
            .map_err(PairingError::NonceStore)?;
        Ok(())
    }

    /// Retire one nonce after a pairing ceremony consumes it.
    ///
    /// The registered task must keep relaying the active Door response until its
    /// connection closes, so it remains available for `retire_all` and removes
    /// itself when its wrapper completes.
    pub(crate) async fn retire(
        &self,
        journal_root: &Path,
        nonce_value: &str,
        now: i64,
    ) -> Result<(), PairingError> {
        self.relay_admissions.remove_for_nonce(nonce_value);
        NonceStore::new(journal_root)
            .cancel(nonce_value, now)
            .map_err(PairingError::NonceStore)?;
        Ok(())
    }

    #[cfg(test)]
    fn registered_count(&self) -> usize {
        lock_windows(&self.windows).len()
    }

    fn spawn_window(&self, task: PairWindowTask) {
        let windows = Arc::clone(&self.windows);
        let task_nonce = task.nonce_value.clone();
        let windows_key = task_nonce.clone();
        let (start, started) = oneshot::channel();
        let task = tokio::spawn(async move {
            if started.await.is_ok() {
                serve_window(task).await;
                lock_windows(&windows).remove(&task_nonce);
            }
        });
        lock_windows(&self.windows).insert(windows_key, task);
        let _ = start.send(());
    }
}

/// Remove stale relay authority before Door begins accepting certificate-less peers.
pub(crate) fn cleanup_relay_windows_on_startup(
    journal_root: &Path,
    now: i64,
) -> Result<(), NonceStoreError> {
    NonceStore::new(journal_root).cancel_all_relay_windows(now)?;
    Ok(())
}

async fn serve_window(mut task: PairWindowTask) {
    let wait = Duration::from_secs(
        u64::try_from(task.expires_at.saturating_sub(unix_seconds())).unwrap_or(0),
    );
    let offer = timeout(wait, task.registration.next_offer()).await;
    let tunnel_id = match offer {
        Ok(Ok(offer)) => offer.tunnel_id,
        Ok(Err(_)) | Err(_) => {
            let _ = task.registration.close().await;
            let _ = NonceStore::new(&task.journal_root).cancel(&task.nonce_value, unix_seconds());
            return;
        }
    };
    let tunnel = match attach_pair_window_tunnel(
        &task.relay_origin,
        &tunnel_id,
        &task.service_token,
        &task.relay_key,
    )
    .await
    {
        Ok(tunnel) => tunnel,
        Err(error) => {
            if matches!(error, PairWindowClientError::Rejected(403)) {
                log::debug!(
                    "relay pairing tunnel attach failed: {}",
                    PairingError::RelayPairingTunnelInstanceMismatch
                );
            } else {
                log::debug!("relay pairing tunnel attach failed");
            }
            let _ = task.registration.close().await;
            let _ = NonceStore::new(&task.journal_root).cancel(&task.nonce_value, unix_seconds());
            return;
        }
    };
    let relay_identity = RelayNonceIdentity::new(task.nonce_value.clone());
    let relay_admissions = Arc::clone(&task.relay_admissions);
    let door = task.door;
    let _ = bridge_pair_window_tunnel(
        tunnel,
        SocketAddrV4::new(Ipv4Addr::LOCALHOST, door.port()),
        move |local_addr| {
            relay_admissions
                .insert_while_current(local_addr, relay_identity, door)
                .ok_or(PairWindowClientError::Bridge)
        },
    )
    .await;
    let _ = task.registration.close().await;
    let _ = NonceStore::new(&task.journal_root).cancel(&task.nonce_value, unix_seconds());
}

fn service_token(journal_root: &Path) -> Result<ServiceToken, PairingError> {
    match load_link_service_token(journal_root) {
        LinkServiceTokenRead::Present(token) => Ok(ServiceToken::new(token.as_str().to_owned())),
        LinkServiceTokenRead::Missing
        | LinkServiceTokenRead::Unreadable
        | LinkServiceTokenRead::Malformed => Err(PairingError::RelayPairingUnavailable),
    }
}

fn registration_error(error: PairWindowClientError) -> PairingError {
    match error {
        PairWindowClientError::TimedOut => PairingError::RelayPairingRegistrationTimedOut,
        PairWindowClientError::Rejected(_) => PairingError::RelayPairingRegistrationRefused,
        PairWindowClientError::RelayOrigin
        | PairWindowClientError::TunnelId
        | PairWindowClientError::Request
        | PairWindowClientError::Connection
        | PairWindowClientError::Offer
        | PairWindowClientError::Closed
        | PairWindowClientError::TunnelProtocol
        | PairWindowClientError::Bridge => PairingError::RelayPairingUnavailable,
    }
}

fn lock_windows(
    windows: &Arc<Mutex<HashMap<String, JoinHandle<()>>>>,
) -> std::sync::MutexGuard<'_, HashMap<String, JoinHandle<()>>> {
    windows
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub(crate) fn unix_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .try_into()
        .unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use std::{
        fs, future,
        path::{Path, PathBuf},
        sync::Arc,
        sync::atomic::{AtomicU64, AtomicUsize, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    use solstone_core_sol_link::pairing::{PairingError, nonces::NonceStore};

    use crate::relay_admission::RelayAdmissionRegistry;

    use super::{PairWindowManager, cleanup_relay_windows_on_startup};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static SEQUENCE: AtomicU64 = AtomicU64::new(0);
            let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            let path = std::env::temp_dir()
                .join(format!("solstone-pair-window-manager-{nanos}-{sequence}"));
            fs::create_dir(&path).expect("temporary journal creates");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    async fn door_availability_gates_the_registration_operation() {
        let relay_admissions = Arc::new(RelayAdmissionRegistry::new());
        let manager = PairWindowManager::new(Arc::clone(&relay_admissions));
        let calls = Arc::new(AtomicUsize::new(0));
        let unavailable_calls = Arc::clone(&calls);

        assert!(matches!(
            manager
                .with_available_door(|_| async move {
                    unavailable_calls.fetch_add(1, Ordering::AcqRel);
                    Ok(())
                })
                .await,
            Err(PairingError::RelayPairingUnavailable)
        ));
        assert_eq!(calls.load(Ordering::Acquire), 0);

        relay_admissions.set_door_port(47_657);
        let available_calls = Arc::clone(&calls);
        assert!(matches!(
            manager
                .with_available_door(|_| async move {
                    available_calls.fetch_add(1, Ordering::AcqRel);
                    Err::<(), _>(PairingError::RelayPairingRegistrationRefused)
                })
                .await,
            Err(PairingError::RelayPairingRegistrationRefused)
        ));
        assert_eq!(calls.load(Ordering::Acquire), 1);
    }

    #[tokio::test]
    async fn door_invalidation_closes_a_pending_registration() {
        let relay_admissions = Arc::new(RelayAdmissionRegistry::new());
        relay_admissions.set_door_port(47_657);
        let door = relay_admissions
            .door_availability()
            .expect("published Door availability");
        let manager = PairWindowManager::new(Arc::clone(&relay_admissions));
        let closed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let closed_by_helper = Arc::clone(&closed);
        let (started_sender, started_receiver) = tokio::sync::oneshot::channel();
        let (release_sender, release_receiver) = tokio::sync::oneshot::channel();

        let registration = manager.register_while_current(
            door,
            || async move {
                let _ = started_sender.send(());
                let _ = release_receiver.await;
                Ok(())
            },
            move |()| async move {
                closed_by_helper.store(true, Ordering::Release);
            },
        );
        let invalidate = async {
            started_receiver.await.expect("registration starts");
            relay_admissions.clear_door_port();
            let _ = release_sender.send(());
        };
        let (result, ()) = tokio::join!(registration, invalidate);

        assert!(matches!(result, Err(PairingError::RelayPairingUnavailable)));
        assert!(closed.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn retire_all_aborts_registered_windows_and_is_empty_safe() {
        let temporary = TempDir::new();
        fs::create_dir_all(temporary.path().join("link")).expect("link directory");
        let relay_admissions = Arc::new(RelayAdmissionRegistry::new());
        relay_admissions.set_door_port(47_657);
        let published_door = relay_admissions
            .door_availability()
            .expect("published Door");
        let manager = PairWindowManager::new(Arc::clone(&relay_admissions));
        assert_eq!(manager.registered_count(), 0);
        manager
            .retire_all(temporary.path(), 10)
            .await
            .expect("empty retire");
        assert!(
            relay_admissions.is_current(published_door),
            "disabling and later re-enabling Private Link must not unpublish the bound Door"
        );

        let task = tokio::spawn(async { future::pending::<()>().await });
        super::lock_windows(&manager.windows).insert("nonce".to_owned(), task);
        assert_eq!(manager.registered_count(), 1);
        manager
            .retire_all(temporary.path(), 10)
            .await
            .expect("retire");
        assert_eq!(manager.registered_count(), 0);
    }

    #[tokio::test]
    async fn retire_cancels_nonce_without_aborting_live_window() {
        let temporary = TempDir::new();
        let store = NonceStore::new(temporary.path());
        store
            .add_relay("nonce".into(), "phone".into(), "observer".into(), 10)
            .expect("relay nonce");
        let manager = PairWindowManager::new(Arc::new(RelayAdmissionRegistry::new()));
        let task = tokio::spawn(async { future::pending::<()>().await });
        super::lock_windows(&manager.windows).insert("nonce".to_owned(), task);

        manager
            .retire(temporary.path(), "nonce", 11)
            .await
            .expect("retire nonce");

        assert!(store.peek("nonce").is_none());
        assert_eq!(manager.registered_count(), 1);
        assert!(
            !super::lock_windows(&manager.windows)
                .get("nonce")
                .expect("registered window")
                .is_finished()
        );

        manager
            .retire_all(temporary.path(), 11)
            .await
            .expect("test cleanup");
    }

    #[test]
    fn startup_cleanup_removes_stale_relay_authority_before_door_start() {
        let temporary = TempDir::new();
        let store = NonceStore::new(temporary.path());
        store
            .add_relay("relay".into(), "phone".into(), "observer".into(), 10)
            .expect("relay nonce");

        cleanup_relay_windows_on_startup(temporary.path(), 11).expect("startup cleanup");
        assert!(store.peek("relay").is_none());
    }
}
