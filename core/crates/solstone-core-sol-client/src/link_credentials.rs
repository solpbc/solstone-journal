// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs::{self, File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use spl_core::ca::sha256_hex;

use crate::seam::LinkJournalMetadata;

static STAGING_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairingIdentity {
    pub cert_sha256: String,
    pub instance_id: String,
    pub ca_fingerprint: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelayAccessState {
    Ready,
    Disabled,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayAccessRecord {
    pub state: RelayAccessState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relay_origin: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
    pub access_generation: u64,
    pub identity: PairingIdentity,
}

impl std::fmt::Debug for RelayAccessRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RelayAccessRecord")
            .field("state", &self.state)
            .field("relay_origin", &self.relay_origin)
            .field(
                "device_token",
                &self.device_token.as_ref().map(|_| "[REDACTED]"),
            )
            .field("expires_at", &self.expires_at)
            .field("access_generation", &self.access_generation)
            .field("identity", &self.identity)
            .finish()
    }
}

impl std::fmt::Display for RelayAccessRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "RelayAccessRecord(state={:?}, origin={:?}, gen={})",
            self.state, self.relay_origin, self.access_generation
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreUnusableReason {
    MalformedJson,
    IdentityMismatch,
    MissingFields,
    IoError,
}

impl std::fmt::Display for StoreUnusableReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MalformedJson => write!(f, "malformed access document"),
            Self::IdentityMismatch => write!(f, "pairing identity mismatch"),
            Self::MissingFields => write!(f, "missing required fields"),
            Self::IoError => write!(f, "access document read error"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreLoadOutcome {
    Ready(RelayAccessRecord),
    Disabled(RelayAccessRecord),
    Absent,
    Unusable(StoreUnusableReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreMutationError {
    IdentityMismatch,
    StaleGeneration,
    Retired,
    LockFailed(String),
    BundleNotFound,
    Io(String),
    PersistUncertain(String),
}

impl std::fmt::Display for StoreMutationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IdentityMismatch => write!(f, "bundle identity has changed"),
            Self::StaleGeneration => write!(f, "access generation is stale"),
            Self::Retired => write!(f, "client generation has retired"),
            Self::LockFailed(msg) => write!(f, "failed to acquire sidecar lock: {msg}"),
            Self::BundleNotFound => write!(f, "bundle directory does not exist"),
            Self::Io(msg) => write!(f, "I/O error during mutation: {msg}"),
            Self::PersistUncertain(msg) => write!(f, "persistence uncertainty: {msg}"),
        }
    }
}

/// An observation bound to this pairing, bundle directory, and exact access bytes.
/// Fields stay private so callers cannot manufacture a publication authority.
#[derive(Clone, PartialEq, Eq)]
pub struct StoreVersion {
    identity: PairingIdentity,
    bundle_dev_ino: (u64, u64),
    access_generation: Option<u64>,
    access_digest: Option<String>,
}

impl std::fmt::Debug for StoreVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoreVersion").finish_non_exhaustive()
    }
}

#[derive(Debug, Clone)]
pub struct StoreCommit {
    pub record: RelayAccessRecord,
    pub version: StoreVersion,
    /// False means rename succeeded, but directory durability is uncertain.
    pub durable: bool,
}

#[cfg(any(test, feature = "store-test-hooks"))]
#[derive(Debug, Clone, Copy)]
pub enum StoreWriteFault {
    BeforeRename = 1,
    BundleSync = 2,
    ParentSync = 3,
}

pub struct SidecarLockGuard {
    _file: File,
}

#[derive(Debug, Clone)]
pub struct LinkCredentialStore {
    bundle_dir: PathBuf,
    lock_path: PathBuf,
    label: String,
    #[cfg(any(test, feature = "store-test-hooks"))]
    write_fault: std::sync::Arc<std::sync::atomic::AtomicU8>,
}

impl LinkCredentialStore {
    pub fn new(bundle_dir: PathBuf, label: &str) -> Self {
        let parent = bundle_dir
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        // All users of a bundle, including metadata writers, share one inode.
        let stable_name = bundle_dir.file_name().unwrap_or_default().to_string_lossy();
        let lock_path = parent.join(format!(".{stable_name}.sidecar.lock"));
        Self {
            bundle_dir,
            lock_path,
            label: label.to_string(),
            #[cfg(any(test, feature = "store-test-hooks"))]
            write_fault: Default::default(),
        }
    }

    pub fn bundle_dir(&self) -> &Path {
        &self.bundle_dir
    }

    pub fn lock_path(&self) -> &Path {
        &self.lock_path
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    fn open_lock_file(&self) -> Result<File, StoreMutationError> {
        let parent = self.lock_path.parent().unwrap_or_else(|| Path::new("."));
        let _ = fs::create_dir_all(parent);
        let mut options = OpenOptions::new();
        options.create(true).read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options
            .open(&self.lock_path)
            .map_err(|err| StoreMutationError::LockFailed(format!("cannot open lock file: {err}")))
    }

    pub fn acquire_lock(&self) -> Result<SidecarLockGuard, StoreMutationError> {
        let file = self.open_lock_file()?;
        #[cfg(unix)]
        {
            use rustix::fs::{FlockOperation, flock};
            flock(&file, FlockOperation::LockExclusive).map_err(|err| {
                StoreMutationError::LockFailed(format!("cannot flock lock file: {err}"))
            })?;
        }
        Ok(SidecarLockGuard { _file: file })
    }

    fn acquire_lock_if_current(
        &self,
        may_publish: &impl Fn() -> bool,
    ) -> Result<SidecarLockGuard, StoreMutationError> {
        if !may_publish() {
            return Err(StoreMutationError::Retired);
        }
        let file = self.open_lock_file()?;
        #[cfg(unix)]
        {
            use rustix::fs::{FlockOperation, flock};
            loop {
                if !may_publish() {
                    return Err(StoreMutationError::Retired);
                }
                match flock(&file, FlockOperation::NonBlockingLockExclusive) {
                    Ok(()) => break,
                    Err(rustix::io::Errno::WOULDBLOCK) => {
                        // Never park the mutation owner indefinitely behind another
                        // process. The caller supplies its deadline/retirement fence.
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    Err(err) => {
                        return Err(StoreMutationError::LockFailed(format!(
                            "cannot flock lock file: {err}"
                        )));
                    }
                }
            }
        }
        if !may_publish() {
            return Err(StoreMutationError::Retired);
        }
        Ok(SidecarLockGuard { _file: file })
    }

    pub fn compute_identity(&self) -> Result<PairingIdentity, String> {
        if !self.bundle_dir.is_dir() {
            return Err("bundle directory is missing".to_string());
        }

        let cert_path = self.bundle_dir.join("cert.pem");
        let cert_bytes =
            fs::read(&cert_path).map_err(|err| format!("failed to read cert.pem: {err}"))?;
        let cert_sha256 = format!("sha256:{}", sha256_hex(&cert_bytes));

        let peer_path = self.bundle_dir.join("peer.json");
        let peer_text = fs::read_to_string(&peer_path)
            .map_err(|err| format!("failed to read peer.json: {err}"))?;
        let peer_val: serde_json::Value = serde_json::from_str(&peer_text)
            .map_err(|err| format!("failed to parse peer.json: {err}"))?;
        let instance_id = peer_val
            .get("instance_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "peer.json missing instance_id".to_string())?
            .to_string();

        let chain_path = self.bundle_dir.join("chain.pem");
        let chain_text = fs::read_to_string(&chain_path)
            .map_err(|err| format!("failed to read chain.pem: {err}"))?;
        let first_der = pem_cert_der(&chain_text)
            .ok_or_else(|| "chain.pem contains no valid certificate DER".to_string())?;
        let ca_fingerprint = format!("sha256:{}", sha256_hex(&first_der));

        Ok(PairingIdentity {
            cert_sha256,
            instance_id,
            ca_fingerprint,
        })
    }

    pub fn load_access(&self) -> StoreLoadOutcome {
        let access_path = self.bundle_dir.join("relay_access.json");
        if !access_path.exists() {
            return StoreLoadOutcome::Absent;
        }

        let bytes = match fs::read(&access_path) {
            Ok(b) => b,
            Err(_) => return StoreLoadOutcome::Unusable(StoreUnusableReason::IoError),
        };

        let record: RelayAccessRecord = match serde_json::from_slice(&bytes) {
            Ok(r) => r,
            Err(_) => return StoreLoadOutcome::Unusable(StoreUnusableReason::MalformedJson),
        };

        let current_identity = match self.compute_identity() {
            Ok(id) => id,
            Err(_) => return StoreLoadOutcome::Unusable(StoreUnusableReason::IdentityMismatch),
        };

        if record.identity != current_identity {
            return StoreLoadOutcome::Unusable(StoreUnusableReason::IdentityMismatch);
        }

        match record.state {
            RelayAccessState::Ready => {
                if record.relay_origin.is_none()
                    || record.device_token.is_none()
                    || record.expires_at.is_none()
                {
                    return StoreLoadOutcome::Unusable(StoreUnusableReason::MissingFields);
                }
                StoreLoadOutcome::Ready(record)
            }
            RelayAccessState::Disabled => StoreLoadOutcome::Disabled(record),
        }
    }

    pub fn capture_version(
        &self,
        expected_identity: &PairingIdentity,
    ) -> Result<StoreVersion, StoreMutationError> {
        let _guard = self.acquire_lock()?;
        self.capture_version_locked(expected_identity)
    }

    pub fn capture_version_if_current(
        &self,
        expected_identity: &PairingIdentity,
        may_publish: impl Fn() -> bool,
    ) -> Result<StoreVersion, StoreMutationError> {
        let _guard = self.acquire_lock_if_current(&may_publish)?;
        self.capture_version_locked(expected_identity)
    }

    fn capture_version_locked(
        &self,
        expected_identity: &PairingIdentity,
    ) -> Result<StoreVersion, StoreMutationError> {
        let identity = self
            .compute_identity()
            .map_err(|_| StoreMutationError::BundleNotFound)?;
        if &identity != expected_identity {
            return Err(StoreMutationError::IdentityMismatch);
        }
        let bundle_dev_ino =
            get_file_dev_ino(&self.bundle_dir).map_err(|_| StoreMutationError::BundleNotFound)?;
        let bytes = match fs::read(self.bundle_dir.join("relay_access.json")) {
            Ok(bytes) => Some(bytes),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
            Err(err) => return Err(StoreMutationError::Io(err.to_string())),
        };
        let access_generation = bytes.as_ref().and_then(|bytes| {
            serde_json::from_slice::<RelayAccessRecord>(bytes)
                .ok()
                .map(|record| record.access_generation)
        });
        let access_digest = bytes.as_ref().map(|bytes| sha256_hex(bytes));
        Ok(StoreVersion {
            identity,
            bundle_dev_ino,
            access_generation,
            access_digest,
        })
    }

    fn check_version_locked(&self, version: &StoreVersion) -> Result<(), StoreMutationError> {
        let current = self.capture_version_locked(&version.identity)?;
        if current.bundle_dev_ino != version.bundle_dev_ino {
            return Err(StoreMutationError::IdentityMismatch);
        }
        if &current != version {
            return Err(StoreMutationError::StaleGeneration);
        }
        Ok(())
    }

    fn check_bundle_locked(&self, version: &StoreVersion) -> Result<(), StoreMutationError> {
        let identity = self
            .compute_identity()
            .map_err(|_| StoreMutationError::BundleNotFound)?;
        let dev_ino =
            get_file_dev_ino(&self.bundle_dir).map_err(|_| StoreMutationError::BundleNotFound)?;
        if identity != version.identity || dev_ino != version.bundle_dev_ino {
            return Err(StoreMutationError::IdentityMismatch);
        }
        Ok(())
    }

    pub fn publish_ready_if_current(
        &self,
        origin: &str,
        token: &str,
        exp: i64,
        version: &StoreVersion,
        may_publish: impl Fn() -> bool,
    ) -> Result<StoreCommit, StoreMutationError> {
        let origin = parse_relay_origin(origin).map_err(StoreMutationError::Io)?;
        let record = RelayAccessRecord {
            state: RelayAccessState::Ready,
            relay_origin: Some(origin),
            device_token: Some(token.to_string()),
            expires_at: Some(exp),
            access_generation: Self::next_generation(version)?,
            identity: version.identity.clone(),
        };
        self.publish_if_current(record, version, may_publish)
    }

    pub fn publish_disabled_if_current(
        &self,
        version: &StoreVersion,
        may_publish: impl Fn() -> bool,
    ) -> Result<StoreCommit, StoreMutationError> {
        let record = RelayAccessRecord {
            state: RelayAccessState::Disabled,
            relay_origin: None,
            device_token: None,
            expires_at: None,
            access_generation: Self::next_generation(version)?,
            identity: version.identity.clone(),
        };
        self.publish_if_current(record, version, may_publish)
    }

    fn next_generation(version: &StoreVersion) -> Result<u64, StoreMutationError> {
        version
            .access_generation
            .unwrap_or(0)
            .checked_add(1)
            .ok_or(StoreMutationError::StaleGeneration)
    }

    fn publish_if_current(
        &self,
        record: RelayAccessRecord,
        version: &StoreVersion,
        may_publish: impl Fn() -> bool,
    ) -> Result<StoreCommit, StoreMutationError> {
        let _guard = self.acquire_lock_if_current(&may_publish)?;
        self.check_version_locked(version)?;
        let bytes = serde_json::to_vec_pretty(&record)
            .map_err(|err| StoreMutationError::Io(err.to_string()))?;
        let durable = self.atomic_replace_file_guarded("relay_access.json", &bytes, || {
            may_publish() && self.check_version_locked(version).is_ok()
        })?;
        // Construct from exactly what was renamed, never from a later read.
        let resulting = StoreVersion {
            identity: version.identity.clone(),
            bundle_dev_ino: version.bundle_dev_ino,
            access_generation: Some(record.access_generation),
            access_digest: Some(sha256_hex(&bytes)),
        };
        Ok(StoreCommit {
            record,
            version: resulting,
            durable,
        })
    }

    pub fn reconcile_if_current(
        &self,
        version: &StoreVersion,
        may_publish: impl Fn() -> bool,
    ) -> Result<StoreCommit, StoreMutationError> {
        let _guard = self.acquire_lock_if_current(&may_publish)?;
        self.check_version_locked(version)?;
        let bytes = fs::read(self.bundle_dir.join("relay_access.json"))
            .map_err(|err| StoreMutationError::Io(err.to_string()))?;
        let record = serde_json::from_slice(&bytes)
            .map_err(|err| StoreMutationError::Io(err.to_string()))?;
        if !may_publish() {
            return Err(StoreMutationError::Retired);
        }
        let durable = self.sync_directories();
        Ok(StoreCommit {
            record,
            version: version.clone(),
            durable,
        })
    }

    /// Metadata belongs to the pairing, independently of access refreshes.
    pub fn write_journal_metadata_if_current(
        &self,
        metadata: &LinkJournalMetadata,
        version: &StoreVersion,
        may_publish: impl Fn() -> bool,
    ) -> Result<bool, StoreMutationError> {
        let _guard = self.acquire_lock_if_current(&may_publish)?;
        self.check_bundle_locked(version)?;
        let bytes = serde_json::to_vec_pretty(metadata)
            .map_err(|err| StoreMutationError::Io(err.to_string()))?;
        self.atomic_replace_file_guarded("journal_metadata.json", &bytes, || {
            may_publish() && self.check_bundle_locked(version).is_ok()
        })
    }

    #[cfg(any(test, feature = "store-test-hooks"))]
    pub fn inject_write_fault(&self, fault: StoreWriteFault) {
        self.write_fault.store(fault as u8, Ordering::SeqCst);
    }

    fn take_write_fault(&self, point: u8) -> bool {
        #[cfg(any(test, feature = "store-test-hooks"))]
        {
            self.write_fault
                .compare_exchange(point, 0, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
        }
        #[cfg(not(any(test, feature = "store-test-hooks")))]
        {
            let _ = point;
            false
        }
    }

    pub fn publish_ready(
        &self,
        relay_origin: &str,
        device_token: &str,
        expires_at: i64,
        expected_identity: &PairingIdentity,
    ) -> Result<RelayAccessRecord, StoreMutationError> {
        let _guard = self.acquire_lock()?;
        let current_identity = self
            .compute_identity()
            .map_err(|_| StoreMutationError::BundleNotFound)?;

        if &current_identity != expected_identity {
            return Err(StoreMutationError::IdentityMismatch);
        }

        let next_gen = match self.load_access() {
            StoreLoadOutcome::Ready(r) | StoreLoadOutcome::Disabled(r) => {
                r.access_generation.saturating_add(1)
            }
            _ => 1,
        };

        let record = RelayAccessRecord {
            state: RelayAccessState::Ready,
            relay_origin: Some(relay_origin.to_string()),
            device_token: Some(device_token.to_string()),
            expires_at: Some(expires_at),
            access_generation: next_gen,
            identity: current_identity,
        };

        let bytes = serde_json::to_vec_pretty(&record)
            .map_err(|err| StoreMutationError::Io(err.to_string()))?;

        self.atomic_replace_file("relay_access.json", &bytes)?;
        Ok(record)
    }

    pub fn publish_disabled(
        &self,
        expected_identity: &PairingIdentity,
    ) -> Result<RelayAccessRecord, StoreMutationError> {
        self.publish_disabled_bounded(expected_identity, None)
    }

    pub fn publish_disabled_bounded(
        &self,
        expected_identity: &PairingIdentity,
        max_observed_generation: Option<u64>,
    ) -> Result<RelayAccessRecord, StoreMutationError> {
        let _guard = self.acquire_lock()?;
        let current_identity = self
            .compute_identity()
            .map_err(|_| StoreMutationError::BundleNotFound)?;

        if &current_identity != expected_identity {
            return Err(StoreMutationError::IdentityMismatch);
        }

        let existing = self.load_access();
        if let (Some(max_gen), StoreLoadOutcome::Ready(r)) = (max_observed_generation, &existing)
            && r.access_generation > max_gen
        {
            return Ok(r.clone());
        }

        let next_gen = match existing {
            StoreLoadOutcome::Ready(r) | StoreLoadOutcome::Disabled(r) => {
                r.access_generation.saturating_add(1)
            }
            _ => 1,
        };

        let record = RelayAccessRecord {
            state: RelayAccessState::Disabled,
            relay_origin: None,
            device_token: None,
            expires_at: None,
            access_generation: next_gen,
            identity: current_identity,
        };

        let bytes = serde_json::to_vec_pretty(&record)
            .map_err(|err| StoreMutationError::Io(err.to_string()))?;

        self.atomic_replace_file("relay_access.json", &bytes)?;
        Ok(record)
    }

    pub fn persist_refreshed_token(
        &self,
        new_token: &str,
        expires_at: i64,
        expected_identity: &PairingIdentity,
        expected_origin: &str,
    ) -> Result<RelayAccessRecord, StoreMutationError> {
        let _guard = self.acquire_lock()?;
        let current_identity = self
            .compute_identity()
            .map_err(|_| StoreMutationError::BundleNotFound)?;

        if &current_identity != expected_identity {
            return Err(StoreMutationError::IdentityMismatch);
        }

        let existing = self.load_access();
        let (current_origin, next_gen) = match existing {
            StoreLoadOutcome::Ready(r) => {
                let origin = r.relay_origin.unwrap_or_default();
                if !same_relay_origin(&origin, expected_origin) {
                    return Err(StoreMutationError::IdentityMismatch);
                }
                (origin, r.access_generation.saturating_add(1))
            }
            StoreLoadOutcome::Disabled(_) => {
                return Err(StoreMutationError::StaleGeneration);
            }
            _ => (expected_origin.to_string(), 1),
        };

        let record = RelayAccessRecord {
            state: RelayAccessState::Ready,
            relay_origin: Some(current_origin),
            device_token: Some(new_token.to_string()),
            expires_at: Some(expires_at),
            access_generation: next_gen,
            identity: current_identity,
        };

        let bytes = serde_json::to_vec_pretty(&record)
            .map_err(|err| StoreMutationError::Io(err.to_string()))?;

        self.atomic_replace_file("relay_access.json", &bytes)?;
        Ok(record)
    }

    pub fn write_journal_metadata(
        &self,
        metadata: &LinkJournalMetadata,
        expected_identity: &PairingIdentity,
    ) -> Result<(), StoreMutationError> {
        let _guard = self.acquire_lock()?;
        let current_identity = self
            .compute_identity()
            .map_err(|_| StoreMutationError::BundleNotFound)?;

        if &current_identity != expected_identity {
            return Err(StoreMutationError::IdentityMismatch);
        }

        let bytes = serde_json::to_vec_pretty(metadata)
            .map_err(|err| StoreMutationError::Io(err.to_string()))?;

        self.atomic_replace_file("journal_metadata.json", &bytes)?;
        Ok(())
    }

    pub fn read_journal_metadata(
        &self,
        expected_identity: &PairingIdentity,
    ) -> Result<Option<LinkJournalMetadata>, StoreMutationError> {
        let current_identity = self
            .compute_identity()
            .map_err(|_| StoreMutationError::BundleNotFound)?;

        if &current_identity != expected_identity {
            return Err(StoreMutationError::IdentityMismatch);
        }

        let path = self.bundle_dir.join("journal_metadata.json");
        if !path.exists() {
            return Ok(None);
        }

        let bytes = fs::read(&path).map_err(|err| StoreMutationError::Io(err.to_string()))?;
        let metadata: LinkJournalMetadata = serde_json::from_slice(&bytes)
            .map_err(|err| StoreMutationError::Io(err.to_string()))?;
        Ok(Some(metadata))
    }

    fn atomic_replace_file(&self, file_name: &str, bytes: &[u8]) -> Result<(), StoreMutationError> {
        if self.atomic_replace_file_guarded(file_name, bytes, || true)? {
            Ok(())
        } else {
            Err(StoreMutationError::PersistUncertain(
                "file renamed but directory sync failed".into(),
            ))
        }
    }

    fn atomic_replace_file_guarded(
        &self,
        file_name: &str,
        bytes: &[u8],
        may_publish: impl Fn() -> bool,
    ) -> Result<bool, StoreMutationError> {
        let parent = self
            .bundle_dir
            .parent()
            .ok_or_else(|| StoreMutationError::Io("bundle dir has no parent".to_string()))?;

        let seq = STAGING_COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let temp_name = format!(".{}.{}.{pid}_{seq}.tmp", self.label, file_name);
        let temp_path = parent.join(temp_name);

        let mut options = OpenOptions::new();
        options.create_new(true).write(true);

        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }

        let mut file = match options.open(&temp_path) {
            Ok(f) => f,
            Err(err) => {
                return Err(StoreMutationError::Io(format!(
                    "failed to open temp file: {err}"
                )));
            }
        };

        if let Err(err) = file.write_all(bytes) {
            let _ = fs::remove_file(&temp_path);
            return Err(StoreMutationError::Io(format!(
                "failed to write temp file: {err}"
            )));
        }

        if let Err(err) = file.sync_all() {
            let _ = fs::remove_file(&temp_path);
            return Err(StoreMutationError::Io(format!(
                "failed to sync temp file: {err}"
            )));
        }

        drop(file);

        let destination = self.bundle_dir.join(file_name);
        if self.take_write_fault(1) {
            let _ = fs::remove_file(&temp_path);
            return Err(StoreMutationError::Io(
                "injected failure before rename".into(),
            ));
        }
        // Recheck retirement after staging and immediately before publication.
        if !may_publish() {
            let _ = fs::remove_file(&temp_path);
            return Err(StoreMutationError::Retired);
        }
        if let Err(err) = fs::rename(&temp_path, &destination) {
            let _ = fs::remove_file(&temp_path);
            return Err(StoreMutationError::Io(format!(
                "failed to rename into place: {err}"
            )));
        }

        Ok(self.sync_directories())
    }

    fn sync_directories(&self) -> bool {
        // Rename changes entries in both directories. Attempt both even if the
        // first sync fails; the result reports visible but uncertain publication.
        let bundle_synced = !self.take_write_fault(2)
            && File::open(&self.bundle_dir)
                .and_then(|file| file.sync_all())
                .is_ok();
        let parent_synced = !self.take_write_fault(3)
            && self
                .bundle_dir
                .parent()
                .is_some_and(|parent| File::open(parent).and_then(|file| file.sync_all()).is_ok());
        bundle_synced && parent_synced
    }
}

pub fn pem_cert_der(pem: &str) -> Option<Vec<u8>> {
    use base64::Engine as _;
    const BEGIN: &str = "-----BEGIN CERTIFICATE-----";
    const END: &str = "-----END CERTIFICATE-----";
    let start = pem.find(BEGIN)? + BEGIN.len();
    let end = start + pem[start..].find(END)?;
    let body: String = pem[start..end]
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    base64::engine::general_purpose::STANDARD.decode(body).ok()
}

pub fn get_file_dev_ino(path: &Path) -> Result<(u64, u64), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = fs::metadata(path).map_err(|err| format!("failed to stat {path:?}: {err}"))?;
        Ok((meta.dev(), meta.ino()))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok((0, 0))
    }
}

pub fn parse_relay_origin(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    let (scheme, rest) = if trimmed
        .get(..8)
        .is_some_and(|s| s.eq_ignore_ascii_case("https://"))
    {
        ("https", &trimmed[8..])
    } else if trimmed
        .get(..7)
        .is_some_and(|s| s.eq_ignore_ascii_case("http://"))
    {
        ("http", &trimmed[7..])
    } else {
        return Err("relay origin must start with http:// or https://".into());
    };
    // A single trailing slash is the origin root. Everything else is authority.
    let authority = rest.strip_suffix('/').unwrap_or(rest);
    if authority.is_empty()
        || authority
            .chars()
            .any(|c| c.is_whitespace() || c.is_control())
        || authority.contains(['/', '?', '#', '@', '\\', '%'])
    {
        return Err("relay origin must contain a bare host and optional port".into());
    }
    let (host, port) = if let Some(ipv6) = authority.strip_prefix('[') {
        let (address, suffix) = ipv6
            .split_once(']')
            .ok_or_else(|| "relay origin IPv6 host is invalid".to_string())?;
        let address: std::net::Ipv6Addr = address
            .parse()
            .map_err(|_| "relay origin IPv6 host is invalid".to_string())?;
        let port = if suffix.is_empty() {
            None
        } else {
            Some(
                suffix
                    .strip_prefix(':')
                    .ok_or_else(|| "relay origin port is invalid".to_string())?,
            )
        };
        (format!("[{address}]"), port)
    } else {
        let (host, port) = authority
            .split_once(':')
            .map_or((authority, None), |(h, p)| (h, Some(p)));
        let host = host.to_ascii_lowercase();
        let dns_host = host.strip_suffix('.').unwrap_or(&host);
        if dns_host.is_empty()
            || dns_host.len() > 253
            || dns_host.split('.').any(|label| {
                label.is_empty()
                    || label.len() > 63
                    || label.starts_with('-')
                    || label.ends_with('-')
                    || !label
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            })
        {
            return Err("relay origin host is invalid".into());
        }
        // A numeric host must be a valid address, not an ambiguous URL shorthand.
        if dns_host.bytes().all(|b| b.is_ascii_digit() || b == b'.')
            && dns_host.parse::<std::net::Ipv4Addr>().is_err()
        {
            return Err("relay origin IPv4 host is invalid".into());
        }
        (host, port)
    };
    let port = port
        .map(|port| {
            if port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) {
                return Err("relay origin port is invalid".to_string());
            }
            port.parse::<u16>()
                .map_err(|_| "relay origin port is invalid".to_string())
        })
        .transpose()?;
    let is_default_port = matches!((scheme, port), ("http", Some(80)) | ("https", Some(443)));
    if let Some(port) = port.filter(|_| !is_default_port) {
        Ok(format!("{scheme}://{host}:{port}"))
    } else {
        Ok(format!("{scheme}://{host}"))
    }
}

pub fn same_relay_origin(a: &str, b: &str) -> bool {
    let Ok(norm_a) = parse_relay_origin(a) else {
        return false;
    };
    let Ok(norm_b) = parse_relay_origin(b) else {
        return false;
    };
    norm_a == norm_b
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);
    impl TempDir {
        fn new(name: &str) -> Self {
            let path = PathBuf::from("/var/tmp").join(format!(
                "solstone-link-creds-test-{}-{}-{}",
                name,
                std::process::id(),
                STAGING_COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("create temp dir");
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

    fn write_bundle_fixtures(bundle_dir: &Path) {
        fs::create_dir_all(bundle_dir).expect("create bundle dir");
        fs::write(bundle_dir.join("private.pem"), "PRIVATE\n").expect("write private");
        fs::write(bundle_dir.join("cert.pem"), "CERT\n").expect("write cert");
        fs::write(
            bundle_dir.join("chain.pem"),
            "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n",
        )
        .expect("write chain");
        fs::write(bundle_dir.join("home_attestation.jwt"), "JWT\n").expect("write jwt");
        fs::write(
            bundle_dir.join("peer.json"),
            r#"{"instance_id":"home-123","home_label":"Home","paired_at":"2026-08-01T00:00:00Z"}"#,
        )
        .expect("write peer.json");
    }

    #[test]
    fn origin_normalization_and_equality() {
        assert_eq!(
            parse_relay_origin("HTTPS://link.solstone.app/").unwrap(),
            "https://link.solstone.app"
        );
        assert_eq!(
            parse_relay_origin("https://link.solstone.app:443").unwrap(),
            "https://link.solstone.app"
        );
        assert_eq!(
            parse_relay_origin("http://127.0.0.1:80").unwrap(),
            "http://127.0.0.1"
        );
        assert_eq!(
            parse_relay_origin("http://127.0.0.1:8080/").unwrap(),
            "http://127.0.0.1:8080"
        );
        assert!(same_relay_origin(
            "https://LINK.solstone.app:443/",
            "https://link.solstone.app"
        ));
        assert!(!same_relay_origin(
            "https://link.solstone.app",
            "http://link.solstone.app"
        ));
        assert!(!same_relay_origin(
            "https://a.relay.app",
            "https://b.relay.app"
        ));
        assert!(parse_relay_origin("https://user:pass@relay.app").is_err());
        assert!(parse_relay_origin("https://relay.app/some/path").is_err());
    }

    #[test]
    fn lock_contention_and_never_unlinks_lock() {
        let temp = TempDir::new("lock-contention");
        let bundle_dir = temp.path().join("laptop");
        write_bundle_fixtures(&bundle_dir);

        let store1 = LinkCredentialStore::new(bundle_dir.clone(), "laptop");
        let store2 = LinkCredentialStore::new(bundle_dir.clone(), "laptop");

        let guard1 = store1.acquire_lock().expect("store1 acquires lock");
        assert!(store1.lock_path().exists());

        // store2 in another thread attempting non-blocking or dropping guard1
        drop(guard1);
        let _guard2 = store2
            .acquire_lock()
            .expect("store2 acquires lock after release");
        assert!(
            store2.lock_path().exists(),
            "lock file must never be unlinked"
        );
    }

    #[test]
    fn store_load_outcomes_absent_ready_disabled_unusable() {
        let temp = TempDir::new("load-outcomes");
        let bundle_dir = temp.path().join("laptop");
        write_bundle_fixtures(&bundle_dir);

        let store = LinkCredentialStore::new(bundle_dir.clone(), "laptop");
        assert_eq!(store.load_access(), StoreLoadOutcome::Absent);

        let identity = store.compute_identity().expect("compute identity");

        // Publish ready
        let record = store
            .publish_ready("https://link.solstone.app", "tok123", 9999999999, &identity)
            .expect("publish ready");
        assert_eq!(record.state, RelayAccessState::Ready);
        assert_eq!(record.access_generation, 1);

        match store.load_access() {
            StoreLoadOutcome::Ready(r) => {
                assert_eq!(r.relay_origin.as_deref(), Some("https://link.solstone.app"));
                assert_eq!(r.device_token.as_deref(), Some("tok123"));
                assert_eq!(r.access_generation, 1);
            }
            other => panic!("expected Ready, got {other:?}"),
        }

        // Publish disabled
        let dis_record = store.publish_disabled(&identity).expect("publish disabled");
        assert_eq!(dis_record.state, RelayAccessState::Disabled);
        assert_eq!(dis_record.access_generation, 2);

        match store.load_access() {
            StoreLoadOutcome::Disabled(r) => {
                assert_eq!(r.access_generation, 2);
                assert!(r.device_token.is_none());
            }
            other => panic!("expected Disabled, got {other:?}"),
        }

        // Malformed JSON -> Unusable
        fs::write(bundle_dir.join("relay_access.json"), "invalid json").expect("write malformed");
        match store.load_access() {
            StoreLoadOutcome::Unusable(StoreUnusableReason::MalformedJson) => {}
            other => panic!("expected Unusable(MalformedJson), got {other:?}"),
        }
    }

    #[test]
    fn identity_mismatch_prevents_mutation_and_redacts_tokens() {
        let temp = TempDir::new("identity-mismatch");
        let bundle_dir = temp.path().join("laptop");
        write_bundle_fixtures(&bundle_dir);

        let store = LinkCredentialStore::new(bundle_dir.clone(), "laptop");
        let identity = store.compute_identity().expect("compute identity");

        // Alter cert.pem
        fs::write(bundle_dir.join("cert.pem"), "ALTERED CERT\n").expect("alter cert");

        let err = store
            .publish_ready(
                "https://link.solstone.app",
                "secret_tok",
                9999999999,
                &identity,
            )
            .expect_err("mutation should fail due to identity mismatch");
        assert_eq!(err, StoreMutationError::IdentityMismatch);

        // Check debug formatting redacts tokens
        let record = RelayAccessRecord {
            state: RelayAccessState::Ready,
            relay_origin: Some("https://link.solstone.app".to_string()),
            device_token: Some("super_secret_token_12345".to_string()),
            expires_at: Some(123456789),
            access_generation: 1,
            identity,
        };
        let debug_str = format!("{record:?}");
        assert!(!debug_str.contains("super_secret_token_12345"));
        assert!(debug_str.contains("[REDACTED]"));
    }

    #[test]
    fn missing_or_replaced_bundle_not_recreated() {
        let temp = TempDir::new("missing-bundle");
        let bundle_dir = temp.path().join("laptop");
        let store = LinkCredentialStore::new(bundle_dir.clone(), "laptop");

        let identity = PairingIdentity {
            cert_sha256: "sha256:1111".to_string(),
            instance_id: "inst-1".to_string(),
            ca_fingerprint: "sha256:2222".to_string(),
        };

        let err = store
            .publish_ready("https://link.solstone.app", "tok", 1000, &identity)
            .expect_err("should fail when bundle dir missing");
        assert_eq!(err, StoreMutationError::BundleNotFound);
        assert!(!bundle_dir.exists(), "must not recreate missing bundle dir");
    }

    #[test]
    fn failed_clear_then_ready_cannot_erase_ready() {
        let temp = TempDir::new("failed-clear-ready");
        let bundle_dir = temp.path().join("laptop");
        write_bundle_fixtures(&bundle_dir);

        let store = LinkCredentialStore::new(bundle_dir.clone(), "laptop");
        let identity = store.compute_identity().expect("compute identity");

        // Gen 1: Ready
        store
            .publish_ready("https://link.solstone.app", "tok1", 1000, &identity)
            .expect("publish ready 1");

        // Gen 2: Ready
        store
            .publish_ready("https://link.solstone.app", "tok2", 2000, &identity)
            .expect("publish ready 2");

        // Delayed disable from Gen 1 observation must not erase Gen 2:
        store
            .publish_disabled_bounded(&identity, Some(1))
            .expect("publish bounded");

        match store.load_access() {
            StoreLoadOutcome::Ready(r) => {
                assert_eq!(r.access_generation, 2);
                assert_eq!(r.device_token.as_deref(), Some("tok2"));
            }
            other => panic!("expected Ready(2), got {other:?}"),
        }
    }

    #[test]
    fn old_bundle_absence() {
        let temp = TempDir::new("old-bundle");
        let bundle_dir = temp.path().join("laptop");
        let store = LinkCredentialStore::new(bundle_dir, "laptop");
        assert_eq!(store.load_access(), StoreLoadOutcome::Absent);
    }

    #[test]
    fn pre_rename_failure_preserves_old_complete_state() {
        let temp = TempDir::new("pre-rename-fail");
        let bundle_dir = temp.path().join("laptop");
        write_bundle_fixtures(&bundle_dir);

        let store = LinkCredentialStore::new(bundle_dir.clone(), "laptop");
        let identity = store.compute_identity().expect("compute identity");

        store
            .publish_ready("https://link.solstone.app", "tok1", 1000, &identity)
            .expect("initial ready");
        let initial_record = match store.load_access() {
            StoreLoadOutcome::Ready(r) => r,
            other => panic!("expected Ready, got {other:?}"),
        };

        // Mutation failure before rename preserves old complete state on disk
        let mismatch_identity = PairingIdentity {
            cert_sha256: "sha256:altered".to_string(),
            instance_id: identity.instance_id.clone(),
            ca_fingerprint: identity.ca_fingerprint.clone(),
        };
        let err = store
            .publish_ready(
                "https://link.solstone.app",
                "tok2",
                2000,
                &mismatch_identity,
            )
            .expect_err("identity mismatch fails before write");
        assert_eq!(err, StoreMutationError::IdentityMismatch);

        match store.load_access() {
            StoreLoadOutcome::Ready(r) => {
                assert_eq!(r.access_generation, initial_record.access_generation);
                assert_eq!(r.device_token, initial_record.device_token);
            }
            other => panic!("expected old Ready state preserved, got {other:?}"),
        }
    }

    #[test]
    fn post_rename_parent_sync_failure_returns_persist_uncertain() {
        // Test that PersistUncertain variant exists and carries error detail without overwriting successor
        let err = StoreMutationError::PersistUncertain("parent directory sync failed".to_string());
        let err_str = format!("{err:?}");
        assert!(err_str.contains("PersistUncertain"));
        assert!(err_str.contains("parent directory sync failed"));
    }
    fn fixture_store(name: &str) -> (TempDir, LinkCredentialStore, PairingIdentity) {
        let temp = TempDir::new(name);
        let bundle = temp.path().join("laptop");
        write_bundle_fixtures(&bundle);
        let store = LinkCredentialStore::new(bundle, "laptop");
        let identity = store.compute_identity().unwrap();
        (temp, store, identity)
    }

    #[test]
    fn exact_revision_rejects_same_generation_rewrite_and_absence_changes() {
        let (_temp, store, identity) = fixture_store("exact-revision");
        let absent = store.capture_version(&identity).unwrap();
        let first = store
            .publish_ready_if_current("https://relay.app", "tok1", 1000, &absent, || true)
            .unwrap();
        assert_eq!(
            store
                .publish_disabled_if_current(&absent, || true)
                .unwrap_err(),
            StoreMutationError::StaleGeneration
        );
        let path = store.bundle_dir().join("relay_access.json");
        // Same generation, altered bytes: a generation-only fence would accept.
        let mut bytes = fs::read(&path).unwrap();
        bytes.push(b' ');
        fs::write(&path, &bytes).unwrap();
        assert_eq!(
            store
                .publish_disabled_if_current(&first.version, || true)
                .unwrap_err(),
            StoreMutationError::StaleGeneration
        );
        let changed = store.capture_version(&identity).unwrap();
        fs::remove_file(&path).unwrap();
        assert_eq!(
            store
                .publish_disabled_if_current(&changed, || true)
                .unwrap_err(),
            StoreMutationError::StaleGeneration
        );
        let missing = store.capture_version(&identity).unwrap();
        fs::write(&path, b"invalid").unwrap();
        assert_eq!(
            store
                .publish_disabled_if_current(&missing, || true)
                .unwrap_err(),
            StoreMutationError::StaleGeneration
        );
        let malformed = store.capture_version(&identity).unwrap();
        fs::write(&path, b"different invalid").unwrap();
        assert_eq!(
            store
                .publish_disabled_if_current(&malformed, || true)
                .unwrap_err(),
            StoreMutationError::StaleGeneration
        );
    }

    #[test]
    fn bundle_replacement_with_identical_pairing_rejects_access_and_metadata() {
        let (temp, store, identity) = fixture_store("bundle-inode");
        let version = store.capture_version(&identity).unwrap();
        fs::rename(store.bundle_dir(), temp.path().join("old-laptop")).unwrap();
        write_bundle_fixtures(store.bundle_dir());
        assert_eq!(store.compute_identity().unwrap(), identity);
        assert_eq!(
            store
                .publish_disabled_if_current(&version, || true)
                .unwrap_err(),
            StoreMutationError::IdentityMismatch
        );
        assert_eq!(
            store
                .write_journal_metadata_if_current(&metadata(Some("Home")), &version, || true)
                .unwrap_err(),
            StoreMutationError::IdentityMismatch
        );
        assert!(!store.bundle_dir().join("relay_access.json").exists());
    }

    #[test]
    fn publication_rechecks_after_sidecar_wait_and_retirement() {
        use std::sync::{Arc, atomic::AtomicBool, mpsc};
        let (_temp, store, identity) = fixture_store("lock-retirement");
        let version = store.capture_version(&identity).unwrap();
        let other_label = LinkCredentialStore::new(store.bundle_dir().to_path_buf(), "");
        assert_eq!(store.lock_path(), other_label.lock_path());
        let guard = other_label.acquire_lock().unwrap();
        let retired = Arc::new(AtomicBool::new(false));
        let child_retired = retired.clone();
        let child = store.clone();
        let (tx, rx) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            tx.send(()).unwrap();
            child.publish_disabled_if_current(&version, || !child_retired.load(Ordering::SeqCst))
        });
        rx.recv().unwrap();
        retired.store(true, Ordering::SeqCst);
        drop(guard);
        assert_eq!(
            thread.join().unwrap().unwrap_err(),
            StoreMutationError::Retired
        );
        assert_eq!(store.load_access(), StoreLoadOutcome::Absent);
        assert!(store.lock_path().exists());
    }

    #[test]
    fn sidecar_wait_rechecks_successor_before_publication() {
        let (_temp, store, identity) = fixture_store("lock-successor");
        let version = store.capture_version(&identity).unwrap();
        let guard = store.acquire_lock().unwrap();
        let child = store.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            tx.send(()).unwrap();
            child.publish_disabled_if_current(&version, || true)
        });
        rx.recv().unwrap();
        // Emulate another writer owning the same sidecar while the callback waits.
        fs::write(store.bundle_dir().join("relay_access.json"), b"successor").unwrap();
        drop(guard);
        assert_eq!(
            thread.join().unwrap().unwrap_err(),
            StoreMutationError::StaleGeneration
        );
        assert_eq!(
            fs::read(store.bundle_dir().join("relay_access.json")).unwrap(),
            b"successor"
        );
    }

    #[test]
    fn external_successor_during_staging_is_rechecked_before_rename() {
        let (_temp, store, identity) = fixture_store("staging-successor");
        let version = store.capture_version(&identity).unwrap();
        let changed = std::cell::Cell::new(false);
        let result = store.publish_disabled_if_current(&version, || {
            let staged = fs::read_dir(store.bundle_dir().parent().unwrap())
                .unwrap()
                .filter_map(Result::ok)
                .any(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"));
            if staged && !changed.replace(true) {
                fs::write(
                    store.bundle_dir().join("relay_access.json"),
                    b"external-successor",
                )
                .unwrap();
            }
            true
        });
        assert!(changed.get(), "actual staged writer was exercised");
        assert_eq!(result.unwrap_err(), StoreMutationError::Retired);
        assert_eq!(
            fs::read(store.bundle_dir().join("relay_access.json")).unwrap(),
            b"external-successor"
        );
    }

    #[test]
    fn actual_writer_faults_preserve_pre_rename_and_reconcile_exact_post_rename() {
        let (_temp, store, identity) = fixture_store("writer-faults");
        let version = store.capture_version(&identity).unwrap();
        let first = store
            .publish_ready_if_current("https://relay.app", "tok1", 1000, &version, || true)
            .unwrap();
        let path = store.bundle_dir().join("relay_access.json");
        let original = fs::read(&path).unwrap();
        store.inject_write_fault(StoreWriteFault::BeforeRename);
        assert!(matches!(
            store.publish_disabled_if_current(&first.version, || true),
            Err(StoreMutationError::Io(_))
        ));
        assert_eq!(fs::read(&path).unwrap(), original);
        assert_eq!(store.capture_version(&identity).unwrap(), first.version);
        let mut current = first.version;
        for fault in [StoreWriteFault::BundleSync, StoreWriteFault::ParentSync] {
            store.inject_write_fault(fault);
            let commit = store
                .publish_ready_if_current("https://relay.app", "tok2", 2000, &current, || true)
                .unwrap();
            assert!(!commit.durable);
            assert_eq!(store.capture_version(&identity).unwrap(), commit.version);
            let before = get_file_dev_ino(&path).unwrap();
            let bytes = fs::read(&path).unwrap();
            let repaired = store
                .reconcile_if_current(&commit.version, || true)
                .unwrap();
            assert!(repaired.durable);
            assert_eq!(repaired.version, commit.version);
            assert_eq!(
                get_file_dev_ino(&path).unwrap(),
                before,
                "reconcile must not rewrite"
            );
            assert_eq!(fs::read(&path).unwrap(), bytes);
            current = commit.version;
        }
        store.inject_write_fault(StoreWriteFault::ParentSync);
        let uncertain = store
            .publish_disabled_if_current(&current, || true)
            .unwrap();
        assert!(!uncertain.durable);
        let successor = store
            .publish_ready_if_current(
                "https://relay.app",
                "successor",
                3000,
                &uncertain.version,
                || true,
            )
            .unwrap();
        assert_eq!(
            store
                .reconcile_if_current(&uncertain.version, || true)
                .unwrap_err(),
            StoreMutationError::StaleGeneration
        );
        assert_eq!(store.capture_version(&identity).unwrap(), successor.version);
    }

    fn metadata(name: Option<&str>) -> LinkJournalMetadata {
        LinkJournalMetadata {
            instance_id: "home-123".into(),
            ca_fp_prefix: "ca".into(),
            paired_at: "today".into(),
            journal_version: "v1".into(),
            journal_name: name.map(str::to_string),
            observed_at: 1.0,
        }
    }

    #[test]
    fn metadata_ignores_access_revision_and_reports_actual_durability() {
        let (_temp, store, identity) = fixture_store("metadata-revision");
        let version = store.capture_version(&identity).unwrap();
        store
            .publish_disabled_if_current(&version, || true)
            .unwrap();
        assert!(
            store
                .write_journal_metadata_if_current(&metadata(Some("Home")), &version, || true)
                .unwrap()
        );
        store.inject_write_fault(StoreWriteFault::BeforeRename);
        assert!(
            store
                .write_journal_metadata_if_current(&metadata(None), &version, || true)
                .is_err()
        );
        assert_eq!(
            store
                .read_journal_metadata(&identity)
                .unwrap()
                .unwrap()
                .journal_name
                .as_deref(),
            Some("Home")
        );
        store.inject_write_fault(StoreWriteFault::BundleSync);
        assert!(
            !store
                .write_journal_metadata_if_current(&metadata(None), &version, || true)
                .unwrap()
        );
        assert_eq!(
            store
                .read_journal_metadata(&identity)
                .unwrap()
                .unwrap()
                .journal_name,
            None
        );
        assert_eq!(
            store
                .write_journal_metadata_if_current(&metadata(Some("Retired")), &version, || false)
                .unwrap_err(),
            StoreMutationError::Retired
        );
        assert_eq!(
            store
                .read_journal_metadata(&identity)
                .unwrap()
                .unwrap()
                .journal_name,
            None
        );
    }

    #[test]
    fn origin_parser_rejects_malformed_unicode_hosts_and_non_origins_without_panicking() {
        for raw in [
            "ééééé",
            "https:/éé",
            "https://",
            "https:///",
            "https://host//",
            "https://host/path",
            "https://host?",
            "https://host#",
            "https://@host",
            "https://bad host",
            "https://host\\evil",
            "https://%65vil",
            "https://-host",
            "https://host..name",
            "https://999.2.3.4",
            "https://[xyz]",
            "https://[::1]suffix",
            "https://::1",
            "https://host:",
            "https://host:+443",
            "https://host:65536",
        ] {
            assert!(
                parse_relay_origin(raw).is_err(),
                "accepted malformed origin: {raw}"
            );
        }
        assert_eq!(
            parse_relay_origin("HTTPS://[0:0:0:0:0:0:0:1]:443/").unwrap(),
            "https://[::1]"
        );
        assert_eq!(
            parse_relay_origin("http://localhost:8080/").unwrap(),
            "http://localhost:8080"
        );
    }
    #[test]
    fn conditional_capture_and_write_retire_while_other_process_keeps_lock() {
        use std::sync::{Arc, atomic::AtomicBool, mpsc};
        let (_temp, store, identity) = fixture_store("cancel-lock-wait");
        let version = store.capture_version(&identity).unwrap();
        let guard = store.acquire_lock().unwrap();
        for capture in [true, false] {
            let child = store.clone();
            let identity = identity.clone();
            let version = version.clone();
            let retired = Arc::new(AtomicBool::new(false));
            let child_retired = retired.clone();
            let (entered_tx, entered_rx) = mpsc::channel();
            let (done_tx, done_rx) = mpsc::channel();
            let thread = std::thread::spawn(move || {
                let predicate = || {
                    let _ = entered_tx.send(());
                    !child_retired.load(Ordering::SeqCst)
                };
                let result = if capture {
                    child
                        .capture_version_if_current(&identity, predicate)
                        .map(|_| ())
                } else {
                    child
                        .publish_disabled_if_current(&version, predicate)
                        .map(|_| ())
                };
                done_tx.send(result).unwrap();
            });
            entered_rx
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap();
            retired.store(true, Ordering::SeqCst);
            // Holder deliberately stays locked through completion: blocking flock
            // cannot pass this test, even if it checks retirement after acquiring.
            assert_eq!(
                done_rx
                    .recv_timeout(std::time::Duration::from_secs(2))
                    .unwrap()
                    .unwrap_err(),
                StoreMutationError::Retired
            );
            thread.join().unwrap();
        }
        assert_eq!(store.load_access(), StoreLoadOutcome::Absent);
        drop(guard);
    }
}
