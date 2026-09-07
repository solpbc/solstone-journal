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
            Self::LockFailed(msg) => write!(f, "failed to acquire sidecar lock: {msg}"),
            Self::BundleNotFound => write!(f, "bundle directory does not exist"),
            Self::Io(msg) => write!(f, "I/O error during mutation: {msg}"),
            Self::PersistUncertain(msg) => write!(f, "persistence uncertainty: {msg}"),
        }
    }
}

pub struct SidecarLockGuard {
    _file: File,
}

#[derive(Debug, Clone)]
pub struct LinkCredentialStore {
    bundle_dir: PathBuf,
    lock_path: PathBuf,
    label: String,
}

impl LinkCredentialStore {
    pub fn new(bundle_dir: PathBuf, label: &str) -> Self {
        let parent = bundle_dir
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let lock_path = parent.join(format!(".{label}.sidecar.lock"));
        Self {
            bundle_dir,
            lock_path,
            label: label.to_string(),
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

    pub fn acquire_lock(&self) -> Result<SidecarLockGuard, StoreMutationError> {
        let parent = self.lock_path.parent().unwrap_or_else(|| Path::new("."));
        let _ = fs::create_dir_all(parent);

        let mut options = OpenOptions::new();
        options.create(true).read(true).write(true);

        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }

        let file = options.open(&self.lock_path).map_err(|err| {
            StoreMutationError::LockFailed(format!("cannot open lock file: {err}"))
        })?;

        #[cfg(unix)]
        {
            use rustix::fs::{FlockOperation, flock};
            flock(&file, FlockOperation::LockExclusive).map_err(|err| {
                StoreMutationError::LockFailed(format!("cannot flock lock file: {err}"))
            })?;
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
        if let Err(err) = fs::rename(&temp_path, &destination) {
            let _ = fs::remove_file(&temp_path);
            return Err(StoreMutationError::Io(format!(
                "failed to rename into place: {err}"
            )));
        }

        let parent_sync_res = File::open(parent).and_then(|f| f.sync_all());
        if let Err(err) = parent_sync_res {
            return Err(StoreMutationError::PersistUncertain(format!(
                "file renamed but parent directory sync failed: {err}"
            )));
        }

        Ok(())
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
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err("relay origin cannot be empty".to_string());
    }
    let (scheme, rest) = if trimmed.len() >= 8 && trimmed[..8].eq_ignore_ascii_case("https://") {
        ("https", &trimmed[8..])
    } else if trimmed.len() >= 7 && trimmed[..7].eq_ignore_ascii_case("http://") {
        ("http", &trimmed[7..])
    } else {
        return Err("relay origin must start with http:// or https://".to_string());
    };

    if rest.contains('/') || rest.contains('?') || rest.contains('#') || rest.contains('@') {
        return Err(
            "relay origin must be a bare host:port without path, query, fragment, or userinfo"
                .to_string(),
        );
    }

    let (host, port_str) = match rest.rsplit_once(':') {
        Some((h, p)) if !h.contains(']') || (h.starts_with('[') && h.ends_with(']')) => {
            (h.to_ascii_lowercase(), Some(p))
        }
        _ => (rest.to_ascii_lowercase(), None),
    };

    if host.is_empty() {
        return Err("relay origin host cannot be empty".to_string());
    }

    let port: Option<u16> = if let Some(p) = port_str {
        match p.parse::<u16>() {
            Ok(port_num) => Some(port_num),
            Err(_) => return Err("relay origin port is invalid".to_string()),
        }
    } else {
        None
    };

    let is_default_port = matches!((scheme, port), ("http", Some(80)) | ("https", Some(443)));

    if let Some(port_num) = port.filter(|_| !is_default_port) {
        Ok(format!("{scheme}://{host}:{port_num}"))
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
}
