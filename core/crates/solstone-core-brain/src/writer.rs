// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The single native write authority for `health/brain.json`.
//!
//! Records are deliberately composed as JSON values.  The read model parses
//! timestamps into `DateTime<Utc>`, but write paths must preserve caller and
//! carried-forward timestamp spellings byte-for-byte (`Z` versus `+00:00`).

use std::collections::BTreeMap;
use std::error::Error;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use chrono::{DateTime, Duration, Utc};
use getrandom::fill as fill_random;
use serde_json::{Map, Value, json};
#[cfg(unix)]
use solstone_core_journal_io::{
    BoundAtomicOutcome, FlatDirectory as BrainHealth, atomic_replace_bound_checked,
    create_or_open_flat_directory_bound, open_flat_directory_bound, read_observed_file,
};
use solstone_core_journal_io::{
    BoundParentLock, FileLease, JournalRoot, JsonWriteOptions, LeaseOptions, LockOptions,
    ObjectIdentity, acquire_existing_parent_lock_bound_checked, acquire_file_lease_bound_checked,
    hold_lock, serialize_json_bytes,
};
#[cfg(windows)]
use solstone_core_journal_io::{
    DetailedAtomicOutcome, WindowsFlatDirectory as BrainHealth,
    atomic_replace_detailed_bound_checked, create_or_open_windows_flat_directory_bound,
    open_windows_flat_directory_bound, read_windows_observed_file_bounded,
};

use crate::credential_hold::{
    CredentialHoldOrigin, CredentialHoldSession, CredentialHoldTarget, CredentialMemorySnapshot,
    CredentialPositivePublication, CredentialProviderGeneration, ObservationCarrier,
    ProvisionalCredentialRejection, VolatileCredentialWitness,
};
use crate::fingerprint::{
    LaneResolution, StoredCredentialBinding, build_active_brain_fingerprint,
    classify_stored_binding, derive_active_brain_lane,
};
use crate::fixture::local_contract;
use crate::inspect::{FINGERPRINT_KEY_BYTES, FingerprintKeyMaterial};
#[cfg(test)]
use crate::inspect::{
    brain_fingerprint_key_path, brain_refresh_lease_path, brain_state_path,
    load_existing_fingerprint_key,
};
use crate::record::{
    BrainStateRecord, CredentialObservation, CredentialRejectionHold, ValidationError,
    component_status_for_reason, parse_component, parse_evidence, parse_runtime_failure_marker,
    reduce_evidence_with_runtime, spp_reason_diagnostic, valid_spp_reason,
    validate_brain_state_record,
};

const BRAIN_FILE_MODE: u32 = 0o600;

/// Terminal writer-parity cases from the frozen projection corpus.
///
/// Intermediate checking, absent/foreign fingerprint, marker, none-lane, and
/// configuration-invalid records are composition-only. The remaining 24 cases
/// are owned by `finish_refresh` and retained as a reviewable selector for AC13.
pub const REACHABLE_WRITE_CASES: [&str; 24] = [
    "lane_bundled/evidence_expired",
    "lane_bundled/generate_failed",
    "lane_bundled/prerequisites_blocked",
    "lane_bundled/ready",
    "lane_bundled/ready_expiring_within_the_hour",
    "lane_bundled/updated_at_in_the_future",
    "lane_byo_cloud/evidence_expired",
    "lane_byo_cloud/generate_failed",
    "lane_byo_cloud/prerequisites_blocked",
    "lane_byo_cloud/ready",
    "lane_byo_cloud/ready_expiring_within_the_hour",
    "lane_byo_cloud/updated_at_in_the_future",
    "lane_byo_endpoint/evidence_expired",
    "lane_byo_endpoint/generate_failed",
    "lane_byo_endpoint/prerequisites_blocked",
    "lane_byo_endpoint/ready",
    "lane_byo_endpoint/ready_expiring_within_the_hour",
    "lane_byo_endpoint/updated_at_in_the_future",
    "lane_spp/evidence_expired",
    "lane_spp/generate_failed",
    "lane_spp/prerequisites_blocked",
    "lane_spp/ready",
    "lane_spp/ready_expiring_within_the_hour",
    "lane_spp/updated_at_in_the_future",
];

/// A refresh/renewal permit whose lease remains held until it is consumed.
#[derive(Debug)]
pub struct BrainRefreshPermit {
    pub run_id: String,
    pub started_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub fingerprint_sha256: String,
    pub checking_revision: u64,
    pub runtime_failure_marker_seen: Option<String>,
    credential_holds_seen: BTreeMap<String, CredentialRejectionHold>,
    credential_generation: Option<Box<CredentialProviderGeneration>>,
    credential_authority_seen: Option<Box<RefreshCredentialAuthority>>,
    credential_publication_deadline: Option<std::time::Instant>,
    bundled_runtime_fingerprint_sha256: Option<String>,
    root: Box<BrainWriteRoot>,
    _lease: FileLease,
}

/// Credential authority for the active BYO lane, captured before real work.
/// Model/fingerprint revisions and UI probe metadata are not authentication
/// generations. Only the existing strict config and Brain carriers are read.
#[derive(Debug, Clone, PartialEq)]
struct RefreshCredentialAuthority {
    provider: String,
    config: Option<Value>,
    observation: Option<CredentialObservation>,
}

impl RefreshCredentialAuthority {
    fn capture(
        root: &BrainWriteRoot,
        resolution: &LaneResolution,
    ) -> Result<Option<Self>, WriterError> {
        let provider = match resolution.lane.as_deref() {
            Some("byo-cloud")
                if matches!(
                    resolution.provider.as_str(),
                    "openai" | "anthropic" | "google"
                ) =>
            {
                resolution.provider.as_str()
            }
            Some("byo-endpoint") => "local",
            _ => return Ok(None),
        };
        Ok(Some(Self {
            provider: provider.to_owned(),
            config: credential_config_authority(root, provider)?,
            observation: credential_observation_for_provider(root, provider)?,
        }))
    }

    fn checkpoint(&self, root: &BrainWriteRoot) -> Result<(), WriterError> {
        if credential_config_authority(root, &self.provider)? != self.config
            || credential_observation_for_provider(root, &self.provider)? != self.observation
        {
            return Err(WriterError::Conflict(
                "credential observation changed during refresh".to_owned(),
            ));
        }
        Ok(())
    }
}

/// Retain the admitted root for the entire refresh, including its finish path.
struct BrainWriteRoot {
    root: Arc<JournalRoot>,
    admitted_path: PathBuf,
    health: OnceLock<BrainHealth>,
}

impl fmt::Debug for BrainWriteRoot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("BrainWriteRoot")
            .field(&self.root.identity())
            .finish()
    }
}

impl BrainWriteRoot {
    fn open(journal: &Path) -> Result<Self, WriterError> {
        let root = Arc::new(
            JournalRoot::open(journal).map_err(|error| WriterError::Io(error.to_string()))?,
        );
        Self::open_bound(root, journal)
    }

    fn open_bound(root: Arc<JournalRoot>, journal: &Path) -> Result<Self, WriterError> {
        let admitted_path =
            std::path::absolute(journal).map_err(|error| WriterError::Io(error.to_string()))?;
        let context = Self {
            root,
            admitted_path,
            health: OnceLock::new(),
        };
        context.check().map_err(WriterError::Io)?;
        Ok(context)
    }

    fn checkpoint(&self, journal: &Path) -> Result<(), String> {
        self.root
            .revalidate_canonical_binding()
            .map_err(|error| error.to_string())?;
        let current = JournalRoot::open(journal).map_err(|error| error.to_string())?;
        if current.identity() != self.root.identity() {
            return Err("admitted journal root changed".to_owned());
        }
        if let Some(health) = self.health.get() {
            let named = self
                .open_health()
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "admitted Brain health directory disappeared".to_owned())?;
            if health_identity(&named) != health_identity(health) {
                return Err("admitted Brain health directory changed".to_owned());
            }
        }
        Ok(())
    }

    fn check(&self) -> Result<(), String> {
        self.checkpoint(&self.admitted_path)
    }

    fn open_health(&self) -> Result<Option<BrainHealth>, WriterError> {
        #[cfg(unix)]
        let result = open_flat_directory_bound(
            &*self.root,
            OsStr::new("health"),
            self.root.canonical_path(),
        );
        #[cfg(windows)]
        let result = open_windows_flat_directory_bound(
            &*self.root,
            OsStr::new("health"),
            self.root.canonical_path(),
        );
        result.map_err(|error| WriterError::Io(error.to_string()))
    }

    fn health(&self, create: bool) -> Result<Option<&BrainHealth>, WriterError> {
        self.check().map_err(WriterError::Io)?;
        if self.health.get().is_none() {
            let opened = self.open_health()?;
            let opened = match (opened, create) {
                (Some(health), _) => Some(health),
                (None, false) => None,
                (None, true) => {
                    #[cfg(unix)]
                    let result = create_or_open_flat_directory_bound(
                        &*self.root,
                        OsStr::new("health"),
                        0o777,
                        self.root.canonical_path(),
                    );
                    #[cfg(windows)]
                    let result = create_or_open_windows_flat_directory_bound(
                        &*self.root,
                        OsStr::new("health"),
                        self.root.canonical_path(),
                    );
                    Some(result.map_err(|error| WriterError::Io(error.to_string()))?)
                }
            };
            if let Some(health) = opened {
                let _ = self.health.set(health);
            }
        }
        self.check().map_err(WriterError::Io)?;
        Ok(self.health.get())
    }

    fn read(&self, name: &OsStr) -> Result<Option<Vec<u8>>, WriterError> {
        let Some(health) = self.health(false)? else {
            return Ok(None);
        };
        #[cfg(unix)]
        let result = read_observed_file(health, name);
        #[cfg(windows)]
        let result = read_windows_observed_file_bounded(health, name, usize::MAX);
        let bytes = result
            .map_err(|error| WriterError::Io(error.to_string()))?
            .map(|observation| observation.bytes);
        self.check().map_err(WriterError::Io)?;
        Ok(bytes)
    }

    fn key_material(&self) -> FingerprintKeyMaterial {
        match self.read(key_name()) {
            Ok(None) => FingerprintKeyMaterial::Absent,
            Ok(Some(bytes)) => bytes.try_into().map_or(
                FingerprintKeyMaterial::Unavailable,
                FingerprintKeyMaterial::Ready,
            ),
            Err(_) => FingerprintKeyMaterial::Unavailable,
        }
    }

    fn existing_key(&self) -> Option<[u8; FINGERPRINT_KEY_BYTES]> {
        match self.key_material() {
            FingerprintKeyMaterial::Ready(key) => Some(key),
            FingerprintKeyMaterial::Absent | FingerprintKeyMaterial::Unavailable => None,
        }
    }

    fn lock(
        &self,
        name: &OsStr,
        checkpoint: &dyn Fn() -> Result<(), String>,
    ) -> Result<BoundParentLock, WriterError> {
        checkpoint().map_err(WriterError::Io)?;
        let health = self.health(true)?.expect("created Brain health parent");
        let options = LockOptions::default();
        let check = || {
            self.check()?;
            checkpoint()
        };
        acquire_existing_parent_lock_bound_checked(
            health,
            &sidecar_name(name),
            options.timeout,
            options.poll_interval,
            &check,
        )
        .map_err(|error| WriterError::Io(error.to_string()))
    }

    fn acquire_lease(
        &self,
        checkpoint: &dyn Fn() -> Result<(), String>,
    ) -> Result<Option<FileLease>, WriterError> {
        checkpoint().map_err(WriterError::Io)?;
        let health = self.health(true)?.expect("created Brain health parent");
        let check = || {
            self.check()?;
            checkpoint()
        };
        acquire_file_lease_bound_checked(health, lease_name(), LeaseOptions::default(), &check)
            .map_err(lease_error)
    }

    fn publish(
        &self,
        lock: &BoundParentLock,
        name: &OsStr,
        bytes: &[u8],
        checkpoint: &dyn Fn() -> Result<(), String>,
    ) -> Result<(), WriterError> {
        let health = self
            .health(false)?
            .ok_or_else(|| WriterError::Io("Brain health parent is absent".to_owned()))?;
        let check = || {
            self.check()?;
            checkpoint()?;
            lock.revalidate_bound(health, &sidecar_name(name))
                .map_err(|error| error.to_string())
        };
        #[cfg(unix)]
        match atomic_replace_bound_checked(health, name, bytes, BRAIN_FILE_MODE, &check)
            .map_err(|error| WriterError::Io(error.to_string()))?
        {
            BoundAtomicOutcome::Published { .. } => {}
            BoundAtomicOutcome::PublishedDurabilityUncertain { source, .. }
            | BoundAtomicOutcome::PublishedObservationUncertain { source, .. } => {
                return Err(WriterError::PublicationUncertain(source.to_string()));
            }
        }
        #[cfg(windows)]
        match atomic_replace_detailed_bound_checked(
            health,
            lock,
            name,
            bytes,
            BRAIN_FILE_MODE,
            &check,
        )
        .map_err(|error| WriterError::Io(error.to_string()))?
        {
            DetailedAtomicOutcome::Published => {}
            outcome => return Err(WriterError::PublicationUncertain(format!("{outcome:?}"))),
        }
        self.check().map_err(WriterError::PublicationUncertain)?;
        lock.revalidate_bound(health, &sidecar_name(name))
            .map_err(|error| WriterError::PublicationUncertain(error.to_string()))
    }
}

#[cfg(unix)]
fn health_identity(health: &BrainHealth) -> ObjectIdentity {
    health.identity()
}
#[cfg(windows)]
fn health_identity(health: &BrainHealth) -> ObjectIdentity {
    health.object_identity()
}

fn record_name() -> &'static OsStr {
    Path::new(&local_contract().brain_state.paths.record)
        .file_name()
        .expect("Brain record filename")
}
fn key_name() -> &'static OsStr {
    Path::new(&local_contract().brain_state.paths.fingerprint_key)
        .file_name()
        .expect("Brain key filename")
}
fn lease_name() -> &'static OsStr {
    Path::new(&local_contract().brain_state.paths.refresh_lease)
        .file_name()
        .expect("Brain lease filename")
}
fn sidecar_name(name: &OsStr) -> OsString {
    let mut name = name.to_owned();
    name.push(".lock");
    name
}

impl BrainRefreshPermit {
    pub fn admitted_root(&self) -> Arc<JournalRoot> {
        self.root.root.clone()
    }

    /// Share the existing post-probe publication wait budget with credential
    /// retention/settlement; this does not bound or change provider probe work.
    pub fn share_credential_publication_budget(&mut self, witness: &CredentialValidationWitness) {
        witness.begin_publication_budget();
        self.credential_publication_deadline = witness.publication_deadline.get();
    }

    /// Renew the original lease, root, fingerprint and credential authority.
    /// Owners also use this before remote work; it never adopts a later fence.
    pub fn checkpoint(&self, journal: &Path) -> Result<(), String> {
        self.root.checkpoint(journal)?;
        check_publication_deadline(self.credential_publication_deadline)?;
        let health = self
            .root
            .health
            .get()
            .ok_or_else(|| "Brain refresh health parent is absent".to_owned())?;
        self._lease
            .revalidate_bound(health, lease_name())
            .map_err(|error| error.to_string())?;
        fingerprint_checkpoint(
            &self.root,
            &self.fingerprint_sha256,
            self.bundled_runtime_fingerprint_sha256.clone(),
        )?;
        let holds =
            read_credential_hold_authority(&self.root).map_err(|error| error.to_string())?;
        let holds = crate::record::parse_credential_holds(holds.as_ref())
            .map_err(|error| error.to_string())?;
        if holds != self.credential_holds_seen {
            return Err("credential rejection changed during refresh".to_owned());
        }
        if let Some(authority) = &self.credential_authority_seen {
            authority
                .checkpoint(&self.root)
                .map_err(|error| error.to_string())?;
        }
        if let Some(generation) = &self.credential_generation {
            generation
                .ensure_current()
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }
}

/// A stale expected active-fingerprint assertion or another begin failure.
#[derive(Debug)]
pub enum BeginRefreshError {
    ExpectedFingerprintStale(String),
    InvalidArgument(String),
    Writer(WriterError),
}

impl fmt::Display for BeginRefreshError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ExpectedFingerprintStale(message) | Self::InvalidArgument(message) => {
                formatter.write_str(message)
            }
            Self::Writer(error) => error.fmt(formatter),
        }
    }
}

impl Error for BeginRefreshError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Writer(error) => Some(error),
            Self::ExpectedFingerprintStale(_) | Self::InvalidArgument(_) => None,
        }
    }
}

/// The deliberately distinct prerequisite-renewal begin result.
#[derive(Debug)]
pub enum BeginPrerequisiteRenewal {
    Started(BrainRefreshPermit),
    Busy { reason: String },
    Unsafe { reason: String },
}

/// A runtime-failure attempt, including the source-compatible rejection name.
#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeFailureResult {
    pub accepted: bool,
    pub record: Option<Value>,
    pub rejected_reason: Option<String>,
    pub error: Option<String>,
}

/// Writer failure outside runtime-failure's returned rejection taxonomy.
#[derive(Debug)]
pub enum WriterError {
    Io(String),
    PublicationUncertain(String),
    Config(String),
    Validation(ValidationError),
    Conflict(String),
    Fingerprint(String),
    Random,
    Cancelled(String),
}

impl fmt::Display for WriterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(message)
            | Self::PublicationUncertain(message)
            | Self::Config(message)
            | Self::Conflict(message)
            | Self::Fingerprint(message) => formatter.write_str(message),
            Self::Validation(error) => error.fmt(formatter),
            Self::Random => formatter.write_str("could not generate secure random bytes"),
            Self::Cancelled(message) => formatter.write_str(message),
        }
    }
}

impl Error for WriterError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Validation(error) => Some(error),
            Self::Io(_)
            | Self::PublicationUncertain(_)
            | Self::Config(_)
            | Self::Conflict(_)
            | Self::Fingerprint(_)
            | Self::Random
            | Self::Cancelled(_) => None,
        }
    }
}

impl From<ValidationError> for WriterError {
    fn from(error: ValidationError) -> Self {
        Self::Validation(error)
    }
}

/// Read config, Brain and surviving-process rejection authority for the
/// actual credential selected by an existing owner. This never creates a key,
/// config, health directory, record or validation result.
pub fn classify_credential_admission_bound(
    admitted: Arc<JournalRoot>,
    journal: &Path,
    target: CredentialHoldTarget<'_>,
    selected_validation: Option<&Value>,
) -> Result<StoredCredentialBinding, WriterError> {
    let root = BrainWriteRoot::open_bound(admitted, journal)?;
    let session = CredentialHoldSession::open_bound(root.root.clone(), target)
        .map_err(|error| WriterError::Conflict(error.to_string()))?;
    let provider = target
        .provider_name()
        .map_err(|error| WriterError::Conflict(error.to_string()))?;
    let memory = session
        .memory_snapshot()
        .map_err(|error| WriterError::Conflict(error.to_string()))?;
    let snapshot = CredentialAuthoritySnapshot::read(&root, provider)?;
    let expected = match snapshot.material {
        FingerprintKeyMaterial::Ready(key) => Some(
            target
                .binding_hmac(&key)
                .map_err(|error| WriterError::Conflict(error.to_string()))?,
        ),
        _ => None,
    };
    if snapshot.correction_proven(&memory, &session, expected.as_deref()) {
        session
            .reconcile(&memory)
            .map_err(|error| WriterError::Conflict(error.to_string()))?;
    }
    if session
        .is_held()
        .map_err(|error| WriterError::Conflict(error.to_string()))?
    {
        return Ok(StoredCredentialBinding::Matches);
    }
    let provider = target
        .provider_name()
        .map_err(|error| WriterError::Conflict(error.to_string()))?;
    let material = root.key_material();
    let key = match material {
        FingerprintKeyMaterial::Ready(key) => Some(key),
        FingerprintKeyMaterial::Absent | FingerprintKeyMaterial::Unavailable => None,
    };
    if key.is_none() && credential_observation_for_provider(&root, provider)?.is_some() {
        return Err(WriterError::Fingerprint(
            "protected fingerprint key is absent while credential authority exists".to_owned(),
        ));
    }
    let expected = key
        .as_ref()
        .map(|key| target.binding_hmac(key))
        .transpose()
        .map_err(|error| WriterError::Conflict(error.to_string()))?;
    let holds = read_credential_hold_authority(&root)?;
    let entry = holds
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|holds| holds.get(provider));
    let brain_binding = classify_stored_binding(entry, key.as_ref(), expected.as_deref());
    let config = config(&root)?;
    let configured_entry = config
        .get("providers")
        .and_then(Value::as_object)
        .and_then(|providers| providers.get("key_validation"))
        .and_then(Value::as_object)
        .and_then(|entries| entries.get(provider));
    let config_binding =
        classify_stored_binding(configured_entry, key.as_ref(), expected.as_deref());
    let selected_binding =
        classify_stored_binding(selected_validation, key.as_ref(), expected.as_deref());
    let bindings = [config_binding, selected_binding, brain_binding];
    let result = [
        StoredCredentialBinding::Matches,
        StoredCredentialBinding::Corrupt,
        StoredCredentialBinding::DoesNotReject,
    ]
    .into_iter()
    .find(|candidate| bindings.contains(candidate))
    .unwrap_or(StoredCredentialBinding::Unbound);
    if material == FingerprintKeyMaterial::Unavailable
        && !matches!(
            result,
            StoredCredentialBinding::Matches | StoredCredentialBinding::Corrupt
        )
    {
        return Err(WriterError::Fingerprint(
            "protected fingerprint key is unavailable".to_owned(),
        ));
    }
    root.check().map_err(WriterError::Io)?;
    if root.key_material() != material {
        return Err(WriterError::Conflict(
            "protected fingerprint key changed during admission".to_owned(),
        ));
    }
    // A rejection registered during the durable read also refuses this call.
    if session
        .is_held()
        .map_err(|error| WriterError::Conflict(error.to_string()))?
    {
        return Ok(StoredCredentialBinding::Matches);
    }
    Ok(result)
}

/// Read the same rejection authorities for owner presentation, without
/// admitting a target or changing its process state. A correction is usable
/// only with the same strict provenance and stable snapshots as admission.
pub(crate) struct BoundBrainInspection {
    root: BrainWriteRoot,
    pub(crate) record: Option<Vec<u8>>,
    pub(crate) material: FingerprintKeyMaterial,
}

pub(crate) fn read_brain_inspection_inputs_bound(
    admitted: Arc<JournalRoot>,
    journal: &Path,
) -> Result<BoundBrainInspection, WriterError> {
    let root = BrainWriteRoot::open_bound(admitted, journal)?;
    let record = root.read(record_name())?;
    let material = root.key_material();
    root.check().map_err(WriterError::Conflict)?;
    Ok(BoundBrainInspection {
        root,
        record,
        material,
    })
}

pub(crate) fn inspect_credential_binding_bound(
    inputs: &BoundBrainInspection,
    target: CredentialHoldTarget<'_>,
    selected_validation: Option<&Value>,
) -> Result<StoredCredentialBinding, WriterError> {
    let root = &inputs.root;
    if root.read(record_name())? != inputs.record {
        return Err(WriterError::Conflict(
            "Brain record changed during inspection".to_owned(),
        ));
    }
    let session = CredentialHoldSession::existing_bound(root.root.clone(), target)
        .map_err(|error| WriterError::Conflict(error.to_string()))?;
    let memory = session
        .as_ref()
        .map(CredentialHoldSession::memory_snapshot)
        .transpose()
        .map_err(|error| WriterError::Conflict(error.to_string()))?;
    let provider = target
        .provider_name()
        .map_err(|error| WriterError::Conflict(error.to_string()))?;
    let snapshot = CredentialAuthoritySnapshot::read(root, provider)?;
    if snapshot.material != inputs.material {
        return Err(WriterError::Conflict(
            "protected fingerprint key changed during inspection".to_owned(),
        ));
    }
    let key = match snapshot.material {
        FingerprintKeyMaterial::Ready(key) => Some(key),
        _ => None,
    };
    let expected = key
        .as_ref()
        .map(|key| target.binding_hmac(key))
        .transpose()
        .map_err(|error| WriterError::Conflict(error.to_string()))?;
    let brain_entry = snapshot.hold.as_ref().map(|hold| {
        json!({
            "binding_version":1,"credential_hmac":hold.credential_hmac,
            "reason_code":"provider_key_invalid",
        })
    });
    let bindings = [
        classify_stored_binding(snapshot.config.as_ref(), key.as_ref(), expected.as_deref()),
        classify_stored_binding(selected_validation, key.as_ref(), expected.as_deref()),
        classify_stored_binding(brain_entry.as_ref(), key.as_ref(), expected.as_deref()),
    ];
    let mut result = [
        StoredCredentialBinding::Matches,
        StoredCredentialBinding::Corrupt,
        StoredCredentialBinding::DoesNotReject,
    ]
    .into_iter()
    .find(|candidate| bindings.contains(candidate))
    .unwrap_or(StoredCredentialBinding::Unbound);
    if let (Some(session), Some(memory)) = (&session, &memory)
        && memory.has_matching_rejection(session)
        && !snapshot.correction_proven(memory, session, expected.as_deref())
    {
        result = StoredCredentialBinding::Matches;
    }
    if !matches!(
        result,
        StoredCredentialBinding::Matches | StoredCredentialBinding::Corrupt
    ) && (snapshot.material == FingerprintKeyMaterial::Unavailable
        || (key.is_none() && snapshot.observation.is_some()))
    {
        return Err(WriterError::Fingerprint(
            "protected fingerprint key is unavailable".to_owned(),
        ));
    }
    // A new P, positive reservation or partial publication cannot hide behind
    // an unchanged accepted epoch. This reader retires no process state.
    if snapshot != CredentialAuthoritySnapshot::read(root, provider)? {
        return Err(WriterError::Conflict(
            "credential authority changed during inspection".to_owned(),
        ));
    }
    let after = CredentialHoldSession::existing_bound(root.root.clone(), target)
        .map_err(|error| WriterError::Conflict(error.to_string()))?
        .as_ref()
        .map(CredentialHoldSession::memory_snapshot)
        .transpose()
        .map_err(|error| WriterError::Conflict(error.to_string()))?;
    if memory != after {
        return Err(WriterError::Conflict(
            "credential process state changed during inspection".to_owned(),
        ));
    }
    if root.read(record_name())? != inputs.record {
        return Err(WriterError::Conflict(
            "Brain record changed during inspection".to_owned(),
        ));
    }
    root.check().map_err(WriterError::Conflict)?;
    Ok(result)
}

/// Local authority captured before an existing mutating owner's real probe.
/// It contains no credential bytes and creates no key or durable state.
pub struct CredentialValidationWitness {
    root: BrainWriteRoot,
    provider: &'static str,
    material: FingerprintKeyMaterial,
    target_hmac: Option<String>,
    observed_hold: Option<CredentialRejectionHold>,
    observed_config: Option<Value>,
    observed_observation: Option<CredentialObservation>,
    observation_id: String,
    prospective_config: std::cell::RefCell<Option<Value>>,
    prospective_observation: std::cell::RefCell<Option<CredentialObservation>>,
    retiring_hold: std::cell::Cell<bool>,
    publication_deadline: std::cell::Cell<Option<std::time::Instant>>,
    positive_publication: std::cell::RefCell<Option<CredentialPositivePublication>>,
    volatile: VolatileCredentialWitness,
}

impl fmt::Debug for CredentialValidationWitness {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CredentialValidationWitness")
            .field("root", &self.root.root.identity())
            .field("provider", &self.provider)
            .finish_non_exhaustive()
    }
}

pub fn capture_credential_validation_bound(
    admitted: Arc<JournalRoot>,
    journal: &Path,
    target: CredentialHoldTarget<'_>,
) -> Result<CredentialValidationWitness, WriterError> {
    let root = BrainWriteRoot::open_bound(admitted, journal)?;
    let provider = target
        .provider_name()
        .map_err(|error| WriterError::Conflict(error.to_string()))?;
    let session = CredentialHoldSession::open_bound(root.root.clone(), target)
        .map_err(|error| WriterError::Conflict(error.to_string()))?;
    let memory = session
        .memory_snapshot()
        .map_err(|error| WriterError::Conflict(error.to_string()))?;
    let snapshot = CredentialAuthoritySnapshot::read(&root, provider)?;
    let material = snapshot.material;
    let target_hmac = match material {
        FingerprintKeyMaterial::Ready(key) => Some(
            target
                .binding_hmac(&key)
                .map_err(|error| WriterError::Conflict(error.to_string()))?,
        ),
        FingerprintKeyMaterial::Absent => None,
        FingerprintKeyMaterial::Unavailable => {
            return Err(WriterError::Fingerprint(
                "protected fingerprint key is unavailable".to_owned(),
            ));
        }
    };
    let reconcile = snapshot.correction_proven(&memory, &session, target_hmac.as_deref());
    let volatile = session
        .validation_witness_after_snapshot(
            &memory,
            reconcile,
            &snapshot.ids(target_hmac.as_deref()),
        )
        .map_err(|error| WriterError::Conflict(error.to_string()))?;
    let CredentialAuthoritySnapshot {
        hold: observed_hold,
        config: observed_config,
        observation: observed_observation,
        ..
    } = snapshot;
    if target_hmac.is_none() && (observed_hold.is_some() || observed_observation.is_some()) {
        return Err(WriterError::Fingerprint(
            "protected fingerprint key is absent while credential authority exists".to_owned(),
        ));
    }
    let witness = CredentialValidationWitness {
        root,
        provider,
        material,
        target_hmac,
        observed_hold,
        observed_config,
        observed_observation,
        observation_id: random_observation_id()?,
        prospective_config: std::cell::RefCell::new(None),
        prospective_observation: std::cell::RefCell::new(None),
        retiring_hold: std::cell::Cell::new(false),
        publication_deadline: std::cell::Cell::new(None),
        positive_publication: std::cell::RefCell::new(None),
        volatile,
    };
    witness.checkpoint()?;
    Ok(witness)
}

fn credential_hold_for_provider(
    root: &BrainWriteRoot,
    provider: &str,
) -> Result<Option<CredentialRejectionHold>, WriterError> {
    let authority = read_credential_hold_authority(root)?;
    Ok(crate::record::parse_credential_holds(authority.as_ref())?.remove(provider))
}

fn config_authority_projection(entry: Option<&Value>) -> Option<Value> {
    let entry = entry?;
    let Some(object) = entry.as_object() else {
        return Some(entry.clone());
    };
    // Transient probe metadata is not a newer authentication observation.
    // Preserve compatibility with older bound positive/negative entries; fresh
    // successful writes acquire a unique private observation identity.
    let bound = object.get("binding_version").and_then(Value::as_u64) == Some(1)
        && object
            .get("credential_hmac")
            .and_then(Value::as_str)
            .is_some_and(crate::record::is_sha256);
    if bound
        && object.get("reason_code").and_then(Value::as_str) != Some("provider_key_invalid")
        && let Some(id) = object
            .get("credential_observation_id")
            .and_then(Value::as_str)
            .filter(|id| crate::record::is_sha256(id))
    {
        return Some(json!({"binding_version":1,
            "credential_hmac":object.get("credential_hmac").unwrap(),
            "credential_observation_id":id}));
    }
    if bound && object.get("valid").and_then(Value::as_bool) == Some(true) {
        // Older records without an observation ID remain readable but do not
        // assert a fresh, uniquely settled positive generation.
        return None;
    }
    if bound
        && object.get("valid").and_then(Value::as_bool) == Some(false)
        && object.get("reason_code").and_then(Value::as_str) != Some("provider_key_invalid")
    {
        return None;
    }
    Some(Value::Object(
        object
            .iter()
            .filter(|(name, _)| {
                !matches!(name.as_str(), "timestamp" | "probe_reason_code" | "error")
            })
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect(),
    ))
}

fn credential_config_authority(
    root: &BrainWriteRoot,
    provider: &str,
) -> Result<Option<Value>, WriterError> {
    let config = config(root)?;
    let entry = config
        .get("providers")
        .and_then(|providers| providers.get("key_validation"))
        .and_then(|entries| entries.get(provider));
    if entry
        .and_then(|entry| entry.get("credential_observation_id"))
        .is_some_and(|id| !id.as_str().is_some_and(crate::record::is_sha256))
    {
        return Err(WriterError::Conflict(
            "credential observation identity is invalid".to_owned(),
        ));
    }
    Ok(config_authority_projection(entry))
}

fn credential_observation_for_provider(
    root: &BrainWriteRoot,
    provider: &str,
) -> Result<Option<CredentialObservation>, WriterError> {
    let authority = read_credential_observation_authority(root)?;
    Ok(crate::record::parse_credential_observations(authority.as_ref())?.remove(provider))
}

/// One stable read of the existing owners, shared by reconciliation and the
/// pre-work witness. No registry mutex is held while reading the journal.
#[derive(PartialEq, Eq)]
struct CredentialAuthoritySnapshot {
    material: FingerprintKeyMaterial,
    config: Option<Value>,
    observation: Option<CredentialObservation>,
    hold: Option<CredentialRejectionHold>,
}

impl CredentialAuthoritySnapshot {
    fn read(root: &BrainWriteRoot, provider: &str) -> Result<Self, WriterError> {
        let once = || -> Result<Self, WriterError> {
            root.check().map_err(WriterError::Conflict)?;
            Ok(Self {
                material: root.key_material(),
                config: credential_config_authority(root, provider)?,
                observation: credential_observation_for_provider(root, provider)?,
                hold: credential_hold_for_provider(root, provider)?,
            })
        };
        let snapshot = once()?;
        if snapshot != once()? {
            return Err(WriterError::Conflict(
                "credential authority changed during observation".to_owned(),
            ));
        }
        root.check().map_err(WriterError::Conflict)?;
        Ok(snapshot)
    }

    fn ids(&self, expected: Option<&str>) -> [Option<String>; 2] {
        let config = self
            .config
            .as_ref()
            .filter(|entry| {
                entry.get("credential_hmac").and_then(Value::as_str) == expected
                    && entry.get("reason_code").and_then(Value::as_str)
                        != Some("provider_key_invalid")
            })
            .and_then(|entry| entry.get("credential_observation_id"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let brain = self
            .observation
            .as_ref()
            .filter(|entry| Some(entry.credential_hmac.as_str()) == expected)
            .map(|entry| entry.observation_id.clone());
        [config, brain]
    }

    fn correction_proven(
        &self,
        memory: &CredentialMemorySnapshot,
        session: &CredentialHoldSession,
        expected: Option<&str>,
    ) -> bool {
        let FingerprintKeyMaterial::Ready(key) = self.material else {
            return false;
        };
        if self
            .hold
            .as_ref()
            .is_some_and(|hold| Some(hold.credential_hmac.as_str()) == expected)
            || matches!(
                classify_stored_binding(self.config.as_ref(), Some(&key), expected),
                StoredCredentialBinding::Matches | StoredCredentialBinding::Corrupt
            )
        {
            return false;
        }
        memory.corrected_by(
            session,
            &key,
            &self.ids(expected).into_iter().flatten().collect::<Vec<_>>(),
        )
    }

    fn origin(
        &self,
        session: &CredentialHoldSession,
        expected: Option<&str>,
    ) -> CredentialHoldOrigin {
        let [config_id, brain_id] = self.ids(expected);
        CredentialHoldOrigin {
            config_id,
            brain_id,
            binding_material_id: match self.material {
                FingerprintKeyMaterial::Ready(key) => Some(session.binding_material_id(&key)),
                _ => None,
            },
            older_publications: Vec::new(),
            excluded_publication: None,
        }
    }
}

fn check_publication_deadline(deadline: Option<std::time::Instant>) -> Result<(), String> {
    if deadline.is_some_and(|deadline| std::time::Instant::now() >= deadline) {
        return Err("credential publication lock budget exhausted".to_owned());
    }
    Ok(())
}

impl CredentialValidationWitness {
    /// Start once, after actual remote work and before the first owner write.
    /// Every added sidecar/fallback/settlement checkpoint shares this budget;
    /// repeated calls cannot grant a fresh wait window.
    pub fn begin_publication_budget(&self) {
        if self.publication_deadline.get().is_none() {
            self.publication_deadline.set(Some(
                std::time::Instant::now() + LockOptions::default().timeout,
            ));
        }
    }

    /// Stage only this real credential-valid result's prospective config
    /// identity. This creates no durable authority and retires no rejection.
    pub fn prepare_successful_validation(&self, result: &mut Value) -> Result<(), WriterError> {
        let object = result.as_object_mut().ok_or_else(|| {
            WriterError::Conflict("validation result is not an object".to_owned())
        })?;
        if object.get("valid").and_then(Value::as_bool) != Some(true)
            || object.get("binding_version").and_then(Value::as_u64) != Some(1)
            || !object
                .get("credential_hmac")
                .and_then(Value::as_str)
                .is_some_and(crate::record::is_sha256)
            || self.target_hmac.as_deref().is_some_and(|expected| {
                object.get("credential_hmac").and_then(Value::as_str) != Some(expected)
            })
        {
            return Err(WriterError::Conflict(
                "validation result does not match its credential".to_owned(),
            ));
        }
        object.insert(
            "credential_observation_id".to_owned(),
            Value::String(self.observation_id.clone()),
        );
        *self.prospective_config.borrow_mut() = config_authority_projection(Some(result));
        Ok(())
    }

    fn check_durable_authority(&self, own_config: Option<&Value>) -> Result<(), WriterError> {
        self.check_durable_authority_with_provisional(own_config, None)
    }

    fn check_durable_authority_with_provisional(
        &self,
        own_config: Option<&Value>,
        provisional: Option<&ProvisionalCredentialRejection>,
    ) -> Result<(), WriterError> {
        let current = credential_config_authority(&self.root, self.provider)?;
        let prospective = self.prospective_config.borrow();
        let own = own_config.and_then(|entry| config_authority_projection(Some(entry)));
        if current != self.observed_config
            && prospective
                .as_ref()
                .is_none_or(|entry| current.as_ref() != Some(entry))
            && own
                .as_ref()
                .is_none_or(|entry| current.as_ref() != Some(entry))
            && !self.known_older_publication(
                provisional,
                current
                    .as_ref()
                    .and_then(|entry| entry.get("credential_observation_id"))
                    .and_then(Value::as_str),
                current
                    .as_ref()
                    .and_then(|entry| entry.get("credential_hmac"))
                    .and_then(Value::as_str),
            )
        {
            return Err(WriterError::Conflict(
                "credential observation changed during work".to_owned(),
            ));
        }
        let observed = credential_observation_for_provider(&self.root, self.provider)?;
        if observed != self.observed_observation
            && self
                .prospective_observation
                .borrow()
                .as_ref()
                .is_none_or(|own| observed.as_ref() != Some(own))
            && !self.known_older_publication(
                provisional,
                observed.as_ref().map(|entry| entry.observation_id.as_str()),
                observed
                    .as_ref()
                    .map(|entry| entry.credential_hmac.as_str()),
            )
        {
            return Err(WriterError::Conflict(
                "settled credential observation changed during work".to_owned(),
            ));
        }
        Ok(())
    }

    fn known_older_publication(
        &self,
        provisional: Option<&ProvisionalCredentialRejection>,
        id: Option<&str>,
        hmac: Option<&str>,
    ) -> bool {
        hmac.is_some()
            && hmac == self.target_hmac.as_deref()
            && provisional.is_some_and(|p| {
                p.older_publications()
                    .any(|publication| Some(publication.observation_id.as_str()) == id)
            })
    }

    /// Called by the existing config owner while both config and Brain locks
    /// are held, immediately before its native publication. The reservation
    /// survives partial publication until this witness settles or is dropped.
    pub fn begin_positive_config_publication(&self, key: &[u8; 32]) -> Result<(), WriterError> {
        if self.prospective_config.borrow().is_none() {
            return Ok(());
        }
        self.begin_positive_publication(key, ObservationCarrier::Config)
    }

    fn begin_positive_publication(
        &self,
        key: &[u8; 32],
        carrier: ObservationCarrier,
    ) -> Result<(), WriterError> {
        self.checkpoint_with_binding_key(key)?;
        if self.positive_publication.borrow().is_some() {
            return Ok(());
        }
        let snapshot = CredentialAuthoritySnapshot::read(&self.root, self.provider)?;
        // Equality is renewed before any reconciliation-origin bookkeeping.
        self.checkpoint_with_binding_key(key)?;
        let session = &self.volatile.session_handle();
        let removed = self
            .observed_hold
            .as_ref()
            .filter(|hold| self.target_hmac.as_deref() == Some(hold.credential_hmac.as_str()))
            .map(|hold| hold.marker_id.clone());
        let guard = self
            .volatile
            .reserve_positive(
                &self.observation_id,
                removed,
                carrier,
                &snapshot.ids(self.target_hmac.as_deref()),
                &session.binding_material_id(key),
            )
            .map_err(|error| WriterError::Conflict(error.to_string()))?;
        *self.positive_publication.borrow_mut() = Some(guard);
        Ok(())
    }
    pub fn has_matching_rejection(&self) -> bool {
        self.volatile.observed_matching_hold()
            || self.observed_hold.as_ref().is_some_and(|hold| {
                self.target_hmac.as_deref() == Some(hold.credential_hmac.as_str())
            })
    }

    pub fn admitted_root(&self) -> Arc<JournalRoot> {
        self.root.root.clone()
    }

    /// Check before the probe and before initializing a first-use binding key.
    pub fn checkpoint(&self) -> Result<(), WriterError> {
        self.check(None)
    }

    /// A first-use key may be initialized after the actual probe by the normal
    /// key owner. Existing protected bytes may never change under this witness.
    pub fn checkpoint_with_binding_key(&self, key: &[u8; 32]) -> Result<(), WriterError> {
        self.check(Some(key))
    }

    fn check(&self, initialized: Option<&[u8; 32]>) -> Result<(), WriterError> {
        self.root.check().map_err(WriterError::Conflict)?;
        check_publication_deadline(self.publication_deadline.get())
            .map_err(WriterError::Conflict)?;
        self.volatile
            .ensure_current()
            .map_err(|error| WriterError::Conflict(error.to_string()))?;
        self.check_durable_authority(None)?;
        let hold = credential_hold_for_provider(&self.root, self.provider)?;
        if hold != self.observed_hold && !(self.retiring_hold.get() && hold.is_none()) {
            return Err(WriterError::Conflict(
                "credential rejection changed during validation".to_owned(),
            ));
        }
        let material = self.root.key_material();
        let key_matches = match (self.material, initialized, material) {
            (
                FingerprintKeyMaterial::Ready(original),
                Some(expected),
                FingerprintKeyMaterial::Ready(current),
            ) => original == *expected && current == original,
            (
                FingerprintKeyMaterial::Absent,
                Some(expected),
                FingerprintKeyMaterial::Ready(current),
            ) => current == *expected,
            (original, None, current) => original == current,
            _ => false,
        };
        if !key_matches {
            return Err(WriterError::Fingerprint(
                "protected fingerprint key changed during validation".to_owned(),
            ));
        }
        self.root.check().map_err(WriterError::Conflict)
    }

    /// Settle only after this mutating owner's fresh credential-valid probe and
    /// acknowledged owner work. Settings carries its unique identity in config;
    /// healthy Brain refresh carries it in Brain without a config write.
    /// Read-only validation must never call it.
    /// Failure or publication uncertainty leaves the volatile refusal held.
    pub fn retire_after_validation(
        self,
        key: &[u8; 32],
        checkpoint: &dyn Fn() -> Result<(), String>,
    ) -> Result<bool, WriterError> {
        self.begin_publication_budget();
        let check = || {
            checkpoint()?;
            self.checkpoint_with_binding_key(key)
                .map_err(|error| error.to_string())
        };
        check().map_err(WriterError::Conflict)?;
        // Config publishers take this same interlock after their config lock.
        // Under it, settlement only reads config and never acquires its lock.
        let lock = self.root.lock(record_name(), &check)?;
        check().map_err(WriterError::Conflict)?;
        let remove = self
            .observed_hold
            .as_ref()
            .is_some_and(|hold| self.target_hmac.as_deref() == Some(hold.credential_hmac.as_str()));
        let brain_observation = self.prospective_config.borrow().is_none();
        if brain_observation {
            self.begin_positive_publication(key, ObservationCarrier::Brain)?;
        } else if self.positive_publication.borrow().is_none() {
            return Err(WriterError::Conflict(
                "credential publication reservation is absent".to_owned(),
            ));
        }
        if remove || brain_observation {
            let mut record = match self.root.read(record_name())? {
                Some(bytes) => serde_json::from_slice::<Value>(&bytes).map_err(|_| {
                    WriterError::Conflict("credential authority is unreadable".to_owned())
                })?,
                None => Value::Object(Map::new()),
            };
            let object = record.as_object_mut().ok_or_else(|| {
                WriterError::Conflict("credential authority is not an object".to_owned())
            })?;
            if remove {
                object
                    .get_mut("credential_holds")
                    .and_then(Value::as_object_mut)
                    .ok_or_else(|| {
                        WriterError::Conflict("credential hold authority changed".to_owned())
                    })?
                    .remove(self.provider);
                self.retiring_hold.set(true);
            }
            if brain_observation {
                let credential_hmac = self.target_hmac.clone().ok_or_else(|| {
                    WriterError::Fingerprint(
                        "credential observation lacks original binding".to_owned(),
                    )
                })?;
                let observation = CredentialObservation {
                    credential_hmac,
                    observation_id: self.observation_id.clone(),
                };
                let observations = object
                    .entry("credential_observations")
                    .or_insert_with(|| json!({}))
                    .as_object_mut()
                    .ok_or_else(|| {
                        WriterError::Conflict(
                            "credential observations are not an object".to_owned(),
                        )
                    })?;
                observations.insert(
                    self.provider.to_owned(),
                    json!({
                        "binding_version":1,"credential_hmac":observation.credential_hmac,
                        "observation_id":observation.observation_id,
                    }),
                );
                *self.prospective_observation.borrow_mut() = Some(observation);
            }
            crate::record::parse_credential_holds(record.get("credential_holds"))?;
            crate::record::parse_credential_observations(record.get("credential_observations"))?;
            let bytes = serialize_json_bytes(
                &record,
                JsonWriteOptions {
                    mode: Some(BRAIN_FILE_MODE),
                    indent: Some(2),
                    sort_keys: true,
                },
            )
            .map_err(|error| WriterError::Io(error.to_string()))?;
            self.root.publish(&lock, record_name(), &bytes, &check)?;
            // Keep the Brain lock until this operation has conditionally
            // retired its volatile marker. A newer rejection wins either way.
            checkpoint().map_err(WriterError::Conflict)?;
            self.root.check().map_err(WriterError::Conflict)?;
        }
        check().map_err(WriterError::Conflict)?;
        self.volatile
            .retire()
            .map(|memory| memory || remove)
            .map_err(|error| WriterError::Conflict(error.to_string()))
    }
}

/// Publish a provider rejection in the existing Brain record after its normal
/// config owner could not acknowledge it. The caller supplies the binding it
/// actually sent and renews current-target/generation/cancellation authority.
/// A published-but-uncertain result is never a durable acknowledgment.
pub fn record_credential_hold_bound(
    admitted: Arc<JournalRoot>,
    journal: &Path,
    provider: &str,
    hold: &CredentialRejectionHold,
    binding_key: &[u8; FINGERPRINT_KEY_BYTES],
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<(), WriterError> {
    let root = BrainWriteRoot::open_bound(admitted, journal)?;
    record_credential_hold(&root, provider, hold, binding_key, checkpoint)
}

fn record_credential_hold(
    root: &BrainWriteRoot,
    provider: &str,
    hold: &CredentialRejectionHold,
    binding_key: &[u8; FINGERPRINT_KEY_BYTES],
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<(), WriterError> {
    record_credential_hold_with_transition(
        root,
        provider,
        hold,
        binding_key,
        checkpoint,
        &|| Ok(()),
    )
}

fn record_credential_hold_with_transition(
    root: &BrainWriteRoot,
    provider: &str,
    hold: &CredentialRejectionHold,
    binding_key: &[u8; FINGERPRINT_KEY_BYTES],
    checkpoint: &dyn Fn() -> Result<(), String>,
    transition: &dyn Fn() -> Result<(), String>,
) -> Result<(), WriterError> {
    let entry = json!({
        "binding_version": 1,
        "credential_hmac": hold.credential_hmac,
        "marker_id": hold.marker_id,
        "reason_code": "provider_key_invalid",
    });
    let mut validation = Map::new();
    validation.insert(provider.to_owned(), entry.clone());
    crate::record::parse_credential_holds(Some(&Value::Object(validation)))?;
    let check = || {
        checkpoint()?;
        root.check()?;
        if root.key_material() != FingerprintKeyMaterial::Ready(*binding_key) {
            return Err(
                "protected fingerprint key changed before rejection publication".to_owned(),
            );
        }
        Ok(())
    };
    check().map_err(WriterError::Conflict)?;
    let lock = root.lock(record_name(), &check)?;
    check().map_err(WriterError::Conflict)?;
    let health = root.health.get().expect("held Brain record parent");
    lock.revalidate_bound(health, &sidecar_name(record_name()))
        .map_err(|error| WriterError::Conflict(error.to_string()))?;
    transition().map_err(WriterError::Conflict)?;
    check().map_err(WriterError::Conflict)?;
    let mut holds = read_credential_hold_authority(root)?
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    holds.insert(provider.to_owned(), entry);
    let mut record = match root.read(record_name())? {
        None => Value::Object(Map::new()),
        Some(bytes) => serde_json::from_slice::<Value>(&bytes).map_err(|error| {
            WriterError::Conflict(format!("credential hold authority is unreadable: {error}"))
        })?,
    };
    record
        .as_object_mut()
        .ok_or_else(|| {
            WriterError::Conflict("credential hold authority is not an object".to_owned())
        })?
        .insert("credential_holds".to_owned(), Value::Object(holds));
    // A hold is independent authority, not evidence that a refresh ran. Keep
    // existing derived fields exactly as values; absence/invalid derived state
    // remains explicit until the normal refresh owner repairs it.
    crate::record::parse_credential_holds(record.get("credential_holds"))?;
    let bytes = serialize_json_bytes(
        &record,
        JsonWriteOptions {
            mode: Some(BRAIN_FILE_MODE),
            indent: Some(2),
            sort_keys: true,
        },
    )
    .map_err(|error| WriterError::Io(error.to_string()))?;
    root.publish(&lock, record_name(), &bytes, &check)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialRejectionCarrier {
    Config,
    Brain,
}

#[derive(Debug, PartialEq, Eq)]
pub enum CredentialRejectionFailure {
    Refused(String),
    Unpublished,
}

/// An acknowledged rejection and the exact registration generation that
/// authorized it. It grants no successful-validation or retirement authority.
pub struct CredentialRejectionReceipt {
    root_identity: ObjectIdentity,
    provider: &'static str,
    binding_key: [u8; FINGERPRINT_KEY_BYTES],
    keyless_endpoint: bool,
    hold: CredentialRejectionHold,
    generation: VolatileCredentialWitness,
    carrier: CredentialRejectionCarrier,
    authority: RefreshCredentialAuthority,
}

impl fmt::Debug for CredentialRejectionReceipt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CredentialRejectionReceipt")
            .field("provider", &self.provider)
            .field("carrier", &self.carrier)
            .finish_non_exhaustive()
    }
}

impl CredentialRejectionReceipt {
    pub fn carrier(&self) -> CredentialRejectionCarrier {
        self.carrier
    }

    /// Renew this operation's exact negative publication without recapturing a
    /// provider generation or granting fresh successful-validation authority.
    pub fn checkpoint(&self, witness: &CredentialValidationWitness) -> Result<(), WriterError> {
        if witness.root.root.identity() != self.root_identity
            || witness.provider != self.provider
            || !self.generation.same_target(&witness.volatile)
        {
            return Err(WriterError::Conflict(
                "rejection receipt root/provider changed".to_owned(),
            ));
        }
        witness.root.check().map_err(WriterError::Conflict)?;
        self.generation
            .ensure_current()
            .map_err(|error| WriterError::Conflict(error.to_string()))?;
        self.authority.checkpoint(&witness.root)?;
        if witness.root.key_material() != FingerprintKeyMaterial::Ready(self.binding_key) {
            return Err(WriterError::Fingerprint(
                "protected fingerprint key changed after rejection".to_owned(),
            ));
        }
        let expected = if self.carrier == CredentialRejectionCarrier::Brain {
            Some(&self.hold)
        } else {
            witness.observed_hold.as_ref()
        };
        if credential_hold_for_provider(&witness.root, self.provider)?.as_ref() != expected {
            return Err(WriterError::Conflict(
                "credential rejection changed after publication".to_owned(),
            ));
        }
        if self.carrier == CredentialRejectionCarrier::Config {
            let current = config(&witness.root)?;
            let entry = current
                .get("providers")
                .and_then(Value::as_object)
                .and_then(|providers| providers.get("key_validation"))
                .and_then(Value::as_object)
                .and_then(|entries| entries.get(self.provider));
            if entry
                .and_then(|entry| entry.get("rejection_marker_id"))
                .and_then(Value::as_str)
                != Some(self.hold.marker_id.as_str())
                || entry
                    .and_then(|entry| entry.get("credential_hmac"))
                    .and_then(Value::as_str)
                    != Some(self.hold.credential_hmac.as_str())
                || entry
                    .and_then(|entry| entry.get("valid"))
                    .and_then(Value::as_bool)
                    != Some(false)
                || entry
                    .and_then(|entry| entry.get("reason_code"))
                    .and_then(Value::as_str)
                    != Some("provider_key_invalid")
            {
                return Err(WriterError::Conflict(
                    "configuration rejection changed after publication".to_owned(),
                ));
            }
        }
        Ok(())
    }
}

/// Keep a definitive rejection of an actual current credential before blocking
/// persistence, then use the normal config owner and existing Brain fallback.
/// `None` means config no longer matches the original owner guard; the actual
/// rejected target's process hold remains protected.
pub fn retain_current_credential_rejection_bound(
    journal: &Path,
    target: CredentialHoldTarget<'_>,
    witness: &CredentialValidationWitness,
    caller_checkpoint: &dyn Fn() -> Result<(), String>,
    accepts_config: &dyn Fn(&Map<String, Value>) -> bool,
    probe_fields: Option<&Value>,
) -> Result<Option<CredentialRejectionReceipt>, CredentialRejectionFailure> {
    retain_credential_rejection_with_config_bound(
        journal,
        target,
        witness,
        caller_checkpoint,
        accepts_config,
        accepts_config,
        &|config, entry| {
            let mut replacement = config.clone();
            let providers = replacement
                .entry("providers")
                .or_insert_with(|| json!({}))
                .as_object_mut()?;
            let validations = providers
                .entry("key_validation")
                .or_insert_with(|| json!({}))
                .as_object_mut()?;
            validations.insert(witness.provider.to_owned(), entry.clone());
            Some(replacement)
        },
        probe_fields,
    )
}

/// Retain the actual rejection while an existing config owner installs its
/// negative result. The original state guard and publication share one read;
/// only that state or this exact published negative can authorize continuation.
#[allow(clippy::too_many_arguments)]
pub fn retain_credential_rejection_with_config_bound(
    journal: &Path,
    target: CredentialHoldTarget<'_>,
    witness: &CredentialValidationWitness,
    caller_checkpoint: &dyn Fn() -> Result<(), String>,
    prior_matches: &dyn Fn(&Map<String, Value>) -> bool,
    target_matches: &dyn Fn(&Map<String, Value>) -> bool,
    replacement: &impl Fn(&Map<String, Value>, &Value) -> Option<Map<String, Value>>,
    probe_fields: Option<&Value>,
) -> Result<Option<CredentialRejectionReceipt>, CredentialRejectionFailure> {
    use CredentialRejectionFailure::{Refused, Unpublished};
    let root = &witness.root;
    root.checkpoint(journal).map_err(Refused)?;
    witness.begin_publication_budget();
    let provider = target
        .provider_name()
        .map_err(|error| Refused(error.to_string()))?;
    if provider != witness.provider {
        return Err(Refused("rejection witness provider changed".to_owned()));
    }
    // Register a provisional before any sidecar wait. A specifically known
    // older local publication may have crossed its last native checkpoint;
    // durable continuation is checked against that bounded provenance below.
    witness
        .volatile
        .ensure_current()
        .map_err(|error| Refused(error.to_string()))?;
    caller_checkpoint().map_err(Refused)?;
    let session = CredentialHoldSession::open_bound(root.root.clone(), target)
        .map_err(|error| Refused(error.to_string()))?;
    root.check().map_err(Refused)?;
    let original_authority = CredentialAuthoritySnapshot {
        material: witness.material,
        config: witness.observed_config.clone(),
        observation: witness.observed_observation.clone(),
        hold: witness.observed_hold.clone(),
    };
    let provisional = session
        .prepare_rejection(
            &witness.volatile,
            Some(original_authority.origin(&session, witness.target_hmac.as_deref())),
        )
        .map_err(|error| Refused(error.to_string()))?;
    // Retain the fact before config/key readability can fail. Restoring the
    // exact pre-probe bytes later is not a new successful validation.
    let initial = solstone_core_journal_config::read_journal_config_bound(&root.root)
        .map_err(|_| Unpublished)?;
    if !prior_matches(initial.config.as_ref().unwrap_or(&Map::new())) {
        return Ok(None);
    }
    caller_checkpoint().map_err(Refused)?;
    // The actual sent target is already held in its process cell. A
    // temporarily unreadable/replaced protected key refuses durable work,
    // without forgetting this definitive rejection in the surviving runtime.
    match (witness.material, root.key_material()) {
        (FingerprintKeyMaterial::Ready(original), FingerprintKeyMaterial::Ready(current))
            if original == current => {}
        (FingerprintKeyMaterial::Absent, FingerprintKeyMaterial::Absent) => {}
        _ => {
            return Err(Refused(
                "protected fingerprint key changed before rejection".to_owned(),
            ));
        }
    }
    let marker_id = provisional.marker().to_owned();
    let generation = std::cell::RefCell::new(None::<VolatileCredentialWitness>);
    let own_config = std::cell::RefCell::new(None::<Value>);
    let own_config_seen = std::cell::Cell::new(false);
    let own_hold = std::cell::RefCell::new(None::<CredentialRejectionHold>);
    let current_target = || {
        caller_checkpoint()?;
        root.check()?;
        check_publication_deadline(witness.publication_deadline.get())?;
        if let FingerprintKeyMaterial::Ready(original) = witness.material
            && root.key_material() != FingerprintKeyMaterial::Ready(original)
        {
            return Err(
                "protected fingerprint key changed before rejection publication".to_owned(),
            );
        }
        if let Some(generation) = generation.borrow().as_ref() {
            generation
                .ensure_current()
                .map_err(|error| error.to_string())?;
        } else {
            provisional
                .ensure_current()
                .map_err(|error| error.to_string())?;
        }
        witness
            .check_durable_authority_with_provisional(
                own_config.borrow().as_ref(),
                Some(&provisional),
            )
            .map_err(|error| error.to_string())?;
        let observed_hold =
            credential_hold_for_provider(root, provider).map_err(|error| error.to_string())?;
        if observed_hold != witness.observed_hold
            && own_hold
                .borrow()
                .as_ref()
                .is_none_or(|own| observed_hold.as_ref() != Some(own))
            && !(observed_hold.is_none()
                && witness.observed_hold.as_ref().is_some_and(|old| {
                    provisional.older_publications().any(|publication| {
                        publication.removed_brain_hold.as_deref() == Some(old.marker_id.as_str())
                    })
                }))
        {
            return Err("credential rejection changed before publication".to_owned());
        }
        let current_read = solstone_core_journal_config::read_journal_config_bound(&root.root)
            .map_err(|error| error.to_string())?;
        let current = current_read.config.unwrap_or_default();
        let own = own_config.borrow();
        let exact_own = own.as_ref().is_some_and(|entry| {
            target_matches(&current)
                && current
                    .get("providers")
                    .and_then(|providers| providers.get("key_validation"))
                    .and_then(|entries| entries.get(provider))
                    == Some(entry)
        });
        if exact_own {
            own_config_seen.set(true);
        } else if own_config_seen.get()
            || (initial.sha256.is_none() && current_read.sha256.is_some())
            || !prior_matches(&current)
        {
            return Err("credential selection changed before rejection publication".to_owned());
        }
        Ok(())
    };
    current_target().map_err(Refused)?;
    let key = match root.key_material() {
        FingerprintKeyMaterial::Ready(key) => key,
        FingerprintKeyMaterial::Unavailable => return Err(Unpublished),
        FingerprintKeyMaterial::Absent => {
            match generate_fingerprint_key_bound(root, &current_target) {
                Ok(key) => key,
                Err(_) => {
                    current_target().map_err(Refused)?;
                    return Err(Unpublished);
                }
            }
        }
    };
    let hold = CredentialRejectionHold {
        credential_hmac: target
            .binding_hmac(&key)
            .map_err(|error| Refused(error.to_string()))?,
        marker_id,
    };
    let refused = std::cell::RefCell::new(None::<String>);
    let checkpoint = || {
        if let Some(error) = refused.borrow().as_ref() {
            return Err(error.clone());
        }
        let result = (|| {
            current_target()?;
            if root.key_material() != FingerprintKeyMaterial::Ready(key) {
                return Err(
                    "protected fingerprint key changed before rejection publication".to_owned(),
                );
            }
            Ok(())
        })();
        if let Err(error) = &result {
            *refused.borrow_mut() = Some(error.clone());
        }
        result
    };
    let promote = || {
        checkpoint()?;
        if generation.borrow().is_none() {
            let promoted = provisional.promote().map_err(|error| error.to_string())?;
            *generation.borrow_mut() = Some(promoted);
        }
        checkpoint()
    };
    checkpoint().map_err(Refused)?;
    let mut entry = json!({"valid":false,"reason_code":"provider_key_invalid","rejection_marker_id":hold.marker_id});
    if let Some(fields) = probe_fields.and_then(Value::as_object) {
        for name in ["probe_reason_code", "timestamp"] {
            if let Some(value) = fields.get(name) {
                entry
                    .as_object_mut()
                    .unwrap()
                    .insert(name.to_owned(), value.clone());
            }
        }
    }
    crate::attach_credential_binding(entry.as_object_mut().unwrap(), &hold.credential_hmac);
    *own_config.borrow_mut() = Some(entry.clone());
    let make_replacement = |config: &Map<String, Value>| {
        if !prior_matches(config) {
            return None;
        }
        let next = replacement(config, &entry)?;
        if !target_matches(&next)
            || next
                .get("providers")
                .and_then(|providers| providers.get("key_validation"))
                .and_then(|entries| entries.get(provider))
                != Some(&entry)
        {
            return None;
        }
        Some(next)
    };
    let publication = Some(if let Some(sha) = initial.sha256.as_deref() {
        solstone_core_journal_config_write::conditional_update_journal_config_at_root_interlocked(
            &root.root,
            sha,
            make_replacement,
            LockOptions::default(),
            &checkpoint,
            &promote,
        )
    } else {
        // Genuine first use keeps the normal config owner's defaults and
        // creation semantics. Renew exact absence under its C→B interlock;
        // a newly appearing readable leaf cannot become this probe's origin.
        solstone_core_journal_config_write::mutate_journal_config_at_root_interlocked(
            &root.root,
            LockOptions::default(),
            &checkpoint,
            &promote,
            |config| {
                let still_absent =
                    solstone_core_journal_config::read_journal_config_bound(&root.root)
                        .is_ok_and(|read| read.sha256.is_none() && read.config.is_none());
                let next = still_absent.then(|| make_replacement(config)).flatten();
                let changed = next.is_some();
                if let Some(next) = next {
                    *config = next;
                }
                solstone_core_journal_config_write::JournalConfigMutation { changed, value: () }
            },
        )
        .map(|transaction| transaction.outcome)
    });
    if matches!(
        publication,
        Some(Ok(
            solstone_core_journal_config_write::BoundConfigOutcome::Published(_)
        ))
    ) {
        own_config_seen.set(true);
    }
    checkpoint().map_err(Refused)?;
    let carrier = if matches!(publication,
        Some(Ok(solstone_core_journal_config_write::BoundConfigOutcome::Published(ref facts)))
            if facts.durability_uncertainty.is_none() && facts.observation_uncertainty.is_none() && facts.confirmation_refusal.is_none())
    {
        CredentialRejectionCarrier::Config
    } else {
        *own_hold.borrow_mut() = Some(hold.clone());
        match record_credential_hold_with_transition(
            root,
            provider,
            &hold,
            &key,
            &checkpoint,
            &promote,
        ) {
            Ok(()) => {
                checkpoint().map_err(Refused)?;
                CredentialRejectionCarrier::Brain
            }
            Err(_) => {
                checkpoint().map_err(Refused)?;
                return Err(Unpublished);
            }
        }
    };
    let settled_config =
        credential_config_authority(root, provider).map_err(|error| Refused(error.to_string()))?;
    let settled_observation = credential_observation_for_provider(root, provider)
        .map_err(|error| Refused(error.to_string()))?;
    checkpoint().map_err(Refused)?;
    let receipt = CredentialRejectionReceipt {
        root_identity: root.root.identity(),
        provider,
        binding_key: key,
        keyless_endpoint: matches!(target, CredentialHoldTarget::Endpoint { credential, .. } if credential.is_empty()),
        authority: RefreshCredentialAuthority {
            provider: provider.to_owned(),
            config: settled_config,
            observation: settled_observation,
        },
        hold,
        generation: generation
            .into_inner()
            .ok_or_else(|| Refused("credential rejection was not promoted".to_owned()))?,
        carrier,
    };
    receipt
        .checkpoint(witness)
        .map_err(|error| Refused(error.to_string()))?;
    Ok(Some(receipt))
}

/// Read protected binding material through the caller's retained journal root.
/// Missing and unavailable bytes stay observations; this never creates state.
pub fn read_fingerprint_key_material_bound(
    admitted: Arc<JournalRoot>,
    journal: &Path,
) -> Result<FingerprintKeyMaterial, WriterError> {
    let root = BrainWriteRoot::open_bound(admitted, journal)?;
    let material = root.key_material();
    root.check().map_err(WriterError::Io)?;
    Ok(material)
}

/// Load or generate the protected 32-byte fingerprint key.
pub fn generate_fingerprint_key(
    journal_path: &Path,
) -> Result<[u8; FINGERPRINT_KEY_BYTES], WriterError> {
    generate_fingerprint_key_checked(journal_path, &|| Ok(()))
}

/// Publish the fingerprint key, classifying the file under its lock.
///
/// `Ready` bytes are kept. `Unavailable` material and an absent file that already
/// has a credential-binding authority are left untouched. `checkpoint` runs inside
/// the lock and again immediately before the key file is renamed.
pub fn generate_fingerprint_key_checked(
    journal_path: &Path,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<[u8; FINGERPRINT_KEY_BYTES], WriterError> {
    let root = BrainWriteRoot::open(journal_path)?;
    generate_fingerprint_key_bound(&root, checkpoint)
}

/// Use the existing protected-key owner with the caller's original root.
pub fn generate_fingerprint_key_at_root_checked(
    admitted: Arc<JournalRoot>,
    journal: &Path,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<[u8; FINGERPRINT_KEY_BYTES], WriterError> {
    let root = BrainWriteRoot::open_bound(admitted, journal)?;
    generate_fingerprint_key_bound(&root, checkpoint)
}

fn generate_fingerprint_key_bound(
    root: &BrainWriteRoot,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<[u8; FINGERPRINT_KEY_BYTES], WriterError> {
    let lock = root.lock(key_name(), checkpoint)?;
    let material = root.key_material();
    let key = match material {
        FingerprintKeyMaterial::Ready(key) => key,
        FingerprintKeyMaterial::Unavailable => {
            return Err(WriterError::Fingerprint(
                "protected fingerprint key is unavailable".to_owned(),
            ));
        }
        FingerprintKeyMaterial::Absent => {
            if binding_authority(root)? {
                return Err(WriterError::Fingerprint(
                    "protected fingerprint key is absent while a credential binding exists"
                        .to_owned(),
                ));
            }
            secure_key()?
        }
    };
    checkpoint().map_err(WriterError::Fingerprint)?;
    let publish_checkpoint = || {
        checkpoint()?;
        match (material, root.key_material()) {
            (FingerprintKeyMaterial::Ready(expected), FingerprintKeyMaterial::Ready(current))
                if current == expected =>
            {
                Ok(())
            }
            (FingerprintKeyMaterial::Absent, FingerprintKeyMaterial::Absent) => {
                if binding_authority(root).map_err(|error| error.to_string())? {
                    Err("credential binding appeared before fingerprint key publication".to_owned())
                } else {
                    Ok(())
                }
            }
            _ => Err("protected fingerprint key changed before publication".to_owned()),
        }
    };
    root.publish(&lock, key_name(), &key, &publish_checkpoint)?;
    Ok(key)
}

fn binding_authority(root: &BrainWriteRoot) -> Result<bool, WriterError> {
    let config = config(root).map_err(|error| {
        WriterError::Fingerprint(format!(
            "credential binding authority could not be read: {error}"
        ))
    })?;
    let holds = read_credential_hold_authority(root)?;
    let observations = read_credential_observation_authority(root)?;
    Ok(observations
        .as_ref()
        .and_then(Value::as_object)
        .is_some_and(|entries| !entries.is_empty())
        || key_validation_binds(&config)
        || holds
            .as_ref()
            .and_then(Value::as_object)
            .is_some_and(|holds| !holds.is_empty()))
}

fn key_validation_binds(config: &Map<String, Value>) -> bool {
    config
        .get("providers")
        .and_then(Value::as_object)
        .and_then(|providers| providers.get("key_validation"))
        .and_then(Value::as_object)
        .is_some_and(|entries| {
            entries.values().any(|entry| {
                entry.as_object().is_some_and(|object| {
                    object.contains_key("binding_version") || object.contains_key("credential_hmac")
                })
            })
        })
}

/// Begin a fenced refresh.  Lease contention deliberately returns no permit.
pub fn begin_refresh(
    journal_path: &Path,
    now: DateTime<Utc>,
    run_id: Option<String>,
    expected_active_fingerprint_sha256: Option<&str>,
    expect_active_fingerprint_absent: bool,
    bundled_runtime_fingerprint_sha256: Option<String>,
) -> Result<Option<BrainRefreshPermit>, BeginRefreshError> {
    begin_refresh_checked(
        journal_path,
        now,
        run_id,
        expected_active_fingerprint_sha256,
        expect_active_fingerprint_absent,
        bundled_runtime_fingerprint_sha256,
        &|| Ok(()),
    )
}

pub fn begin_refresh_checked(
    journal_path: &Path,
    now: DateTime<Utc>,
    run_id: Option<String>,
    expected_active_fingerprint_sha256: Option<&str>,
    expect_active_fingerprint_absent: bool,
    bundled_runtime_fingerprint_sha256: Option<String>,
    publication_checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<Option<BrainRefreshPermit>, BeginRefreshError> {
    publication_checkpoint()
        .map_err(|error| BeginRefreshError::Writer(WriterError::Cancelled(error)))?;
    let result = (|| {
        validate_expected(
            expected_active_fingerprint_sha256,
            expect_active_fingerprint_absent,
        )?;
        let expected_contract =
            expected_active_fingerprint_sha256.is_some() || expect_active_fingerprint_absent;
        let root = BrainWriteRoot::open(journal_path).map_err(BeginRefreshError::Writer)?;
        let checkpoint = || {
            publication_checkpoint()?;
            root.checkpoint(journal_path)
        };
        checkpoint().map_err(|error| BeginRefreshError::Writer(WriterError::Io(error)))?;
        let config = match config(&root) {
            Ok(config) => config,
            Err(_) => return Ok(None),
        };
        let initial = derive_active_brain_lane(&config);
        if initial.lane.is_none() && !expected_contract {
            return Ok(None);
        }
        if initial.lane.as_deref() == Some("none") && !expected_contract {
            begin_none_lane(&root, now, &initial, &checkpoint)
                .map_err(BeginRefreshError::Writer)?;
            return Ok(None);
        }

        let Some(lease) = root
            .acquire_lease(&checkpoint)
            .map_err(BeginRefreshError::Writer)?
        else {
            return Ok(None);
        };
        begin_refresh_under_lease(
            now,
            run_id,
            expected_active_fingerprint_sha256,
            expect_active_fingerprint_absent,
            bundled_runtime_fingerprint_sha256,
            lease,
            root,
            publication_checkpoint,
        )
    })();
    if result.is_err() {
        publication_checkpoint()
            .map_err(|error| BeginRefreshError::Writer(WriterError::Cancelled(error)))?;
    }
    result
}

/// Begin the narrow SPP prerequisite-renewal lane.
pub fn begin_prerequisite_renewal(
    journal_path: &Path,
    now: DateTime<Utc>,
    run_id: Option<String>,
    expected_fingerprint_sha256: Option<&str>,
    bundled_runtime_fingerprint_sha256: Option<String>,
) -> BeginPrerequisiteRenewal {
    begin_prerequisite_renewal_checked(
        journal_path,
        now,
        run_id,
        expected_fingerprint_sha256,
        bundled_runtime_fingerprint_sha256,
        &|| Ok(()),
    )
    .unwrap_or_else(|error| BeginPrerequisiteRenewal::Unsafe {
        reason: error.to_string(),
    })
}

pub fn begin_prerequisite_renewal_checked(
    journal_path: &Path,
    now: DateTime<Utc>,
    run_id: Option<String>,
    expected_fingerprint_sha256: Option<&str>,
    bundled_runtime_fingerprint_sha256: Option<String>,
    publication_checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<BeginPrerequisiteRenewal, WriterError> {
    publication_checkpoint().map_err(WriterError::Cancelled)?;
    let result = (|| {
        if expected_fingerprint_sha256.is_some_and(|value| !is_sha256(value)) {
            return BeginPrerequisiteRenewal::Unsafe {
                reason: "fingerprint_mismatch".to_owned(),
            };
        }
        let root = match BrainWriteRoot::open(journal_path) {
            Ok(root) => root,
            Err(error) => {
                return BeginPrerequisiteRenewal::Unsafe {
                    reason: error.to_string(),
                };
            }
        };
        let checkpoint = || {
            publication_checkpoint()?;
            root.checkpoint(journal_path)
        };
        let lease = match root.acquire_lease(&checkpoint) {
            Ok(Some(lease)) => lease,
            Ok(None) => {
                return BeginPrerequisiteRenewal::Busy {
                    reason: "lease_held".to_owned(),
                };
            }
            Err(error) => {
                return BeginPrerequisiteRenewal::Unsafe {
                    reason: error.to_string(),
                };
            }
        };
        let loaded = match fingerprint_for_write(&root, bundled_runtime_fingerprint_sha256.clone())
        {
            Ok(Some(loaded)) => loaded,
            Ok(None) => {
                return BeginPrerequisiteRenewal::Unsafe {
                    reason: "fingerprint_not_available".to_owned(),
                };
            }
            Err(error) => {
                return BeginPrerequisiteRenewal::Unsafe {
                    reason: error.to_string(),
                };
            }
        };
        if loaded.resolution.lane.as_deref() != Some("spp") {
            return BeginPrerequisiteRenewal::Unsafe {
                reason: "non_spp_lane".to_owned(),
            };
        }
        if expected_fingerprint_sha256.is_some_and(|expected| expected != loaded.sha256) {
            return BeginPrerequisiteRenewal::Unsafe {
                reason: "fingerprint_mismatch".to_owned(),
            };
        }
        let run_id = match run_id {
            Some(run_id) => run_id,
            None => match random_id() {
                Ok(run_id) => run_id,
                Err(error) => {
                    return BeginPrerequisiteRenewal::Unsafe {
                        reason: error.to_string(),
                    };
                }
            },
        };
        let expires_at = now + checking_ttl();
        let result: Result<BeginPrerequisiteRenewal, WriterError> = (|| {
            let checkpoint = || {
                publication_checkpoint()?;
                root.checkpoint(journal_path)?;
                lease
                    .revalidate_bound(root.health.get().expect("leased health"), lease_name())
                    .map_err(|error| error.to_string())
            };
            let lock = root.lock(record_name(), &checkpoint)?;
            let Some((raw, current)) = read_current(&root, now)? else {
                return Ok(BeginPrerequisiteRenewal::Unsafe {
                    reason: "brain_record_missing".to_owned(),
                });
            };
            if current.fingerprint_sha256.as_deref() != Some(&loaded.sha256) {
                return Ok(BeginPrerequisiteRenewal::Unsafe {
                    reason: "brain_record_missing".to_owned(),
                });
            }
            let Some(evidence) = safe_prerequisite_evidence(&raw, &current, now) else {
                return Ok(BeginPrerequisiteRenewal::Unsafe {
                    reason: "unsafe_evidence".to_owned(),
                });
            };
            let revision = next_revision(Some(&current));
            let marker_seen = marker_id(Some(&raw));
            let record = compose_checking_record(CheckingRecord {
                revision,
                now,
                expires_at,
                run_id: &run_id,
                loaded: &loaded,
                evidence,
                marker: raw
                    .get("runtime_failure_marker")
                    .cloned()
                    .unwrap_or(Value::Null),
                marker_seen: marker_seen.clone(),
            });
            let publish_checkpoint = || {
                checkpoint()?;
                fingerprint_checkpoint(
                    &root,
                    &loaded.sha256,
                    bundled_runtime_fingerprint_sha256.clone(),
                )
            };
            let written = write_record(&root, &lock, &record, now, &publish_checkpoint)?;
            let credential_holds_seen =
                crate::record::parse_credential_holds(written.get("credential_holds"))?;
            Ok(BeginPrerequisiteRenewal::Started(BrainRefreshPermit {
                run_id,
                started_at: now,
                expires_at,
                fingerprint_sha256: loaded.sha256.clone(),
                checking_revision: revision,
                runtime_failure_marker_seen: marker_seen,
                credential_holds_seen,
                credential_generation: None,
                credential_authority_seen: None,
                credential_publication_deadline: None,
                bundled_runtime_fingerprint_sha256,
                root: Box::new(root),
                _lease: lease,
            }))
        })();
        result.unwrap_or_else(|error| BeginPrerequisiteRenewal::Unsafe {
            reason: error.to_string(),
        })
    })();
    publication_checkpoint().map_err(WriterError::Cancelled)?;
    Ok(result)
}

/// Finish a definitive rejected probe using its exact acknowledged registration.
/// This renews only negative evidence; the receipt cannot authorize success.
pub fn finish_refresh_rejected(
    journal_path: &Path,
    permit: BrainRefreshPermit,
    outcome: Value,
    now: DateTime<Utc>,
    bundled_runtime_fingerprint_sha256: Option<String>,
    receipt: CredentialRejectionReceipt,
) -> Result<Value, WriterError> {
    finish_refresh_rejected_checked(
        journal_path,
        permit,
        outcome,
        now,
        bundled_runtime_fingerprint_sha256,
        receipt,
        &|| Ok(()),
    )
}

pub fn finish_refresh_rejected_checked(
    journal_path: &Path,
    permit: BrainRefreshPermit,
    outcome: Value,
    now: DateTime<Utc>,
    bundled_runtime_fingerprint_sha256: Option<String>,
    receipt: CredentialRejectionReceipt,
    publication_checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<Value, WriterError> {
    publication_checkpoint().map_err(WriterError::Cancelled)?;
    let result = (|| {
        let mut permit = permit;

        let reason = outcome
            .get("generate")
            .and_then(|generate| generate.get("reason_code"))
            .and_then(Value::as_str);
        if !(reason == Some("provider_key_invalid")
            || (reason == Some("provider_key_missing") && receipt.keyless_endpoint))
            || receipt.root_identity != permit.root.root.identity()
            || derive_active_brain_lane(&config(&permit.root)?).provider != receipt.provider
            || permit
                .credential_generation
                .as_ref()
                .is_none_or(|generation| !generation.same_target(&receipt.generation))
        {
            return Err(WriterError::Conflict(
                "rejected refresh receipt does not match its probe".to_owned(),
            ));
        }
        receipt
            .generation
            .ensure_current()
            .map_err(|error| WriterError::Conflict(error.to_string()))?;
        receipt.authority.checkpoint(&permit.root)?;
        permit.credential_generation = Some(Box::new(receipt.generation.provider_generation()));
        permit.credential_authority_seen = Some(Box::new(receipt.authority));
        if receipt.carrier == CredentialRejectionCarrier::Brain {
            permit
                .credential_holds_seen
                .insert(receipt.provider.to_owned(), receipt.hold);
        }
        finish_refresh_checked(
            journal_path,
            permit,
            outcome,
            now,
            bundled_runtime_fingerprint_sha256,
            publication_checkpoint,
        )
    })();
    if result.is_err() {
        publication_checkpoint().map_err(WriterError::Cancelled)?;
    }
    result
}

/// Finish a refresh from the caller's probe outcome.
pub fn finish_refresh(
    journal_path: &Path,
    permit: BrainRefreshPermit,
    outcome: Value,
    now: DateTime<Utc>,
    bundled_runtime_fingerprint_sha256: Option<String>,
) -> Result<Value, WriterError> {
    finish_refresh_checked(
        journal_path,
        permit,
        outcome,
        now,
        bundled_runtime_fingerprint_sha256,
        &|| Ok(()),
    )
}

pub fn finish_refresh_checked(
    journal_path: &Path,
    permit: BrainRefreshPermit,
    outcome: Value,
    now: DateTime<Utc>,
    bundled_runtime_fingerprint_sha256: Option<String>,
    publication_checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<Value, WriterError> {
    publication_checkpoint().map_err(WriterError::Cancelled)?;
    let result = (|| {
        let checkpoint = || {
            publication_checkpoint()?;
            permit.checkpoint(journal_path)
        };
        checkpoint().map_err(WriterError::Io)?;
        parse_evidence(Some(&outcome), now)?;
        let loaded = fingerprint_for_write(&permit.root, bundled_runtime_fingerprint_sha256)?
            .ok_or_else(|| {
                WriterError::Conflict("brain fingerprint key is unavailable".to_owned())
            })?;
        let lock = permit.root.lock(record_name(), &checkpoint)?;
        let Some((_, current)) = read_current(&permit.root, now)? else {
            return Err(WriterError::Conflict(
                "brain refresh checking marker is absent".to_owned(),
            ));
        };
        assert_finish_allowed(&permit, &current, now)?;
        if loaded.sha256 != permit.fingerprint_sha256 {
            return Err(WriterError::Conflict(
                "brain fingerprint changed".to_owned(),
            ));
        }
        let record = compose_record_from_evidence(
            outcome,
            &loaded,
            next_revision(Some(&current)),
            now,
            Value::Null,
            Value::Null,
            Value::Object(Map::new()),
        )?;
        write_record(&permit.root, &lock, &record, now, &checkpoint)
    })();
    if result.is_err() {
        publication_checkpoint().map_err(WriterError::Cancelled)?;
    }
    result
}

/// Abandon a refresh with one evidence-recordable reason.
pub fn abandon_refresh(
    journal_path: &Path,
    permit: BrainRefreshPermit,
    reason_code: &str,
    diagnostic: Map<String, Value>,
    now: DateTime<Utc>,
) -> Result<Value, WriterError> {
    abandon_refresh_checked(journal_path, permit, reason_code, diagnostic, now, &|| {
        Ok(())
    })
}

pub fn abandon_refresh_checked(
    journal_path: &Path,
    permit: BrainRefreshPermit,
    reason_code: &str,
    diagnostic: Map<String, Value>,
    now: DateTime<Utc>,
    publication_checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<Value, WriterError> {
    publication_checkpoint().map_err(WriterError::Cancelled)?;
    let result = (|| {
        let checkpoint = || {
            publication_checkpoint()?;
            permit.checkpoint(journal_path)
        };
        checkpoint().map_err(WriterError::Io)?;
        let component = target_component(reason_code).ok_or_else(|| {
            WriterError::Conflict("brain abandon reason is not recordable evidence".to_owned())
        })?;
        let lock = permit.root.lock(record_name(), &checkpoint)?;
        let Some((raw, current)) = read_current(&permit.root, now)? else {
            return Err(WriterError::Conflict(
                "brain refresh checking marker is absent".to_owned(),
            ));
        };
        assert_finish_allowed(&permit, &current, now)?;
        let mut evidence = object_field(&raw, "evidence")?.clone();
        evidence.insert(
            component.to_owned(),
            component_for_reason(reason_code, diagnostic.clone(), now)?,
        );
        let revision = next_revision(Some(&current));
        let record = compose_direct_record(DirectRecord {
            revision,
            reason_code: reason_code.to_owned(),
            active_lane: current.active_lane.clone(),
            active_provider: current.active_provider.clone(),
            active_model: current.active_model.clone(),
            fingerprint_sha256: current.fingerprint_sha256.clone(),
            evidence: Value::Object(evidence),
            checking: Value::Null,
            marker: raw
                .get("runtime_failure_marker")
                .cloned()
                .unwrap_or(Value::Null),
            diagnostic: Value::Object(diagnostic),
            now,
        });
        write_record(&permit.root, &lock, &record, now, &checkpoint)
    })();
    if result.is_err() {
        publication_checkpoint().map_err(WriterError::Cancelled)?;
    }
    result
}

/// Finish an SPP prerequisite renewal.
pub fn finish_prerequisite_renewal(
    journal_path: &Path,
    permit: BrainRefreshPermit,
    lane_prerequisites: Value,
    now: DateTime<Utc>,
    bundled_runtime_fingerprint_sha256: Option<String>,
) -> Result<Value, WriterError> {
    finish_prerequisite_renewal_checked(
        journal_path,
        permit,
        lane_prerequisites,
        now,
        bundled_runtime_fingerprint_sha256,
        &|| Ok(()),
    )
}

pub fn finish_prerequisite_renewal_checked(
    journal_path: &Path,
    permit: BrainRefreshPermit,
    lane_prerequisites: Value,
    now: DateTime<Utc>,
    bundled_runtime_fingerprint_sha256: Option<String>,
    publication_checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<Value, WriterError> {
    publication_checkpoint().map_err(WriterError::Cancelled)?;
    let result = (|| {
        let checkpoint = || {
            publication_checkpoint()?;
            permit.checkpoint(journal_path)
        };
        checkpoint().map_err(WriterError::Io)?;
        let component = parse_component("lane_prerequisites", &lane_prerequisites, now)?;
        if component.status == "not_attempted" {
            return Err(WriterError::Validation(ValidationError {
                path: "lane_prerequisites.status".to_owned(),
                reason: "prerequisite renewal requires ok or declared failure".to_owned(),
            }));
        }
        let loaded = fingerprint_for_write(&permit.root, bundled_runtime_fingerprint_sha256)?
            .ok_or_else(|| {
                WriterError::Conflict("brain fingerprint key is unavailable".to_owned())
            })?;
        let lock = permit.root.lock(record_name(), &checkpoint)?;
        let Some((raw, current)) = read_current(&permit.root, now)? else {
            return Err(WriterError::Conflict(
                "brain refresh checking marker is absent".to_owned(),
            ));
        };
        assert_finish_allowed(&permit, &current, now)?;
        if loaded.sha256 != permit.fingerprint_sha256 {
            return Err(WriterError::Conflict(
                "brain fingerprint changed".to_owned(),
            ));
        }
        if loaded.resolution.lane.as_deref() != Some("spp") {
            return Err(WriterError::Conflict("brain lane changed".to_owned()));
        }
        let Some(mut evidence) = safe_prerequisite_evidence(&raw, &current, now) else {
            return Err(WriterError::Conflict(
                "brain prerequisite evidence is unsafe".to_owned(),
            ));
        };
        evidence.insert("lane_prerequisites".to_owned(), lane_prerequisites);
        let record = compose_record_from_evidence(
            Value::Object(evidence),
            &loaded,
            next_revision(Some(&current)),
            now,
            Value::Null,
            Value::Null,
            Value::Object(Map::new()),
        )?;
        write_record(&permit.root, &lock, &record, now, &checkpoint)
    })();
    if result.is_err() {
        publication_checkpoint().map_err(WriterError::Cancelled)?;
    }
    result
}

/// Abandon an SPP prerequisite renewal by delegating to its finish path.
pub fn abandon_prerequisite_renewal(
    journal_path: &Path,
    permit: BrainRefreshPermit,
    reason_code: &str,
    diagnostic: Map<String, Value>,
    now: DateTime<Utc>,
    bundled_runtime_fingerprint_sha256: Option<String>,
) -> Result<Value, WriterError> {
    abandon_prerequisite_renewal_checked(
        journal_path,
        permit,
        reason_code,
        diagnostic,
        now,
        bundled_runtime_fingerprint_sha256,
        &|| Ok(()),
    )
}

pub fn abandon_prerequisite_renewal_checked(
    journal_path: &Path,
    permit: BrainRefreshPermit,
    reason_code: &str,
    diagnostic: Map<String, Value>,
    now: DateTime<Utc>,
    bundled_runtime_fingerprint_sha256: Option<String>,
    publication_checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<Value, WriterError> {
    publication_checkpoint().map_err(WriterError::Cancelled)?;
    let result = (|| {
        finish_prerequisite_renewal_checked(
            journal_path,
            permit,
            component_for_reason(reason_code, diagnostic, now)?,
            now,
            bundled_runtime_fingerprint_sha256,
            publication_checkpoint,
        )
    })();
    if result.is_err() {
        publication_checkpoint().map_err(WriterError::Cancelled)?;
    }
    result
}

/// Record an independent runtime failure without acquiring the refresh lease.
pub fn record_runtime_failure(
    journal_path: &Path,
    reason_code: &str,
    component: &str,
    expected_fingerprint_sha256: &str,
    diagnostic: Map<String, Value>,
    now: DateTime<Utc>,
    bundled_runtime_fingerprint_sha256: Option<String>,
) -> RuntimeFailureResult {
    let vocabulary = &local_contract().brain_state;
    if !vocabulary
        .reason_codes
        .iter()
        .any(|reason| reason == reason_code)
        || vocabulary
            .projection_only_reason_codes
            .iter()
            .any(|reason| reason == reason_code)
        || !vocabulary
            .reason_to_aggregate
            .get(reason_code)
            .is_some_and(|aggregate| vocabulary.runtime_failure_aggregates.contains(aggregate))
    {
        return rejected("reason_not_recordable", None);
    }
    if !vocabulary
        .runtime_failure_components
        .iter()
        .any(|candidate| candidate == component)
        || !vocabulary
            .evidence_reason_codes
            .get(component)
            .is_some_and(|reasons| reasons.iter().any(|reason| reason == reason_code))
    {
        return rejected("component_reason_not_allowed", None);
    }
    if component_for_reason_in(component, reason_code, diagnostic.clone(), now).is_err() {
        return rejected("reason_not_recordable", None);
    }
    if !is_sha256(expected_fingerprint_sha256) {
        return rejected("fingerprint_mismatch", None);
    }
    let result = (|| {
        let root = BrainWriteRoot::open(journal_path)?;
        let checkpoint = || root.checkpoint(journal_path);
        let lock = root.lock(record_name(), &checkpoint)?;
        record_runtime_failure_locked(
            &root,
            &lock,
            RuntimeFailureObservation {
                reason_code,
                component,
                expected_fingerprint_sha256,
                diagnostic,
                now,
                bundled_runtime_fingerprint_sha256,
            },
        )
    })();
    match result {
        Ok(record) => RuntimeFailureResult {
            accepted: true,
            record: Some(record),
            rejected_reason: None,
            error: None,
        },
        Err(RuntimeFailureInternal::Rejected(reason, error)) => rejected(reason, error),
        Err(RuntimeFailureInternal::Writer(error)) => {
            rejected("state_unavailable", Some(error.to_string()))
        }
    }
}

struct RuntimeFailureObservation<'a> {
    reason_code: &'a str,
    component: &'a str,
    expected_fingerprint_sha256: &'a str,
    diagnostic: Map<String, Value>,
    now: DateTime<Utc>,
    bundled_runtime_fingerprint_sha256: Option<String>,
}

fn record_runtime_failure_locked(
    root: &BrainWriteRoot,
    lock: &BoundParentLock,
    observation: RuntimeFailureObservation<'_>,
) -> Result<Value, RuntimeFailureInternal> {
    let RuntimeFailureObservation {
        reason_code,
        component,
        expected_fingerprint_sha256,
        diagnostic,
        now,
        bundled_runtime_fingerprint_sha256,
    } = observation;
    let checkpoint = || root.check();
    checkpoint().map_err(WriterError::Io)?;
    let (current_raw, current, current_readable) = match read_current(root, now) {
        Ok(Some((raw, record))) => (Some(raw), Some(record), true),
        Ok(None) => (None, None, true),
        Err(WriterError::Io(error)) => {
            return Err(RuntimeFailureInternal::Rejected(
                "state_unavailable",
                Some(error),
            ));
        }
        Err(_) => (None, None, false),
    };
    let loaded = match fingerprint_for_write(root, bundled_runtime_fingerprint_sha256.clone()) {
        Ok(Some(loaded)) => loaded,
        Ok(None) => {
            return Err(RuntimeFailureInternal::Rejected(
                "fingerprint_not_available",
                None,
            ));
        }
        Err(error) => {
            return Err(RuntimeFailureInternal::Rejected(
                "fingerprint_not_available",
                Some(error.to_string()),
            ));
        }
    };
    if loaded.sha256 != expected_fingerprint_sha256 {
        return Err(RuntimeFailureInternal::Rejected(
            "fingerprint_mismatch",
            None,
        ));
    }
    let revision = if current_readable {
        next_revision(current.as_ref())
    } else {
        1
    };
    let mut evidence = current_raw
        .as_ref()
        .filter(|raw| {
            raw.get("fingerprint_sha256").and_then(Value::as_str) == Some(loaded.sha256.as_str())
        })
        .and_then(|raw| raw.get("evidence").and_then(Value::as_object).cloned())
        .unwrap_or_else(empty_evidence);
    evidence.insert(
        component.to_owned(),
        component_for_reason_in(component, reason_code, diagnostic.clone(), now)?,
    );
    let marker = json!({
        "marker_id": random_id()?,
        "revision": revision,
        "recorded_at": iso(now),
        "reason_code": reason_code,
    });
    let record = compose_record_from_evidence(
        Value::Object(evidence),
        &loaded,
        revision,
        now,
        Value::Null,
        marker,
        Value::Object(diagnostic),
    )?;
    let publish_checkpoint = || {
        checkpoint()?;
        fingerprint_checkpoint(
            root,
            expected_fingerprint_sha256,
            bundled_runtime_fingerprint_sha256.clone(),
        )
    };
    Ok(write_record(root, lock, &record, now, &publish_checkpoint)?)
}

/// Closed identity for an independent runtime-refusal publisher.
#[derive(Clone, Copy)]
pub enum RuntimeRefusalCaller {
    Generate,
    Transcription,
}

enum RefusalObservation {
    Published(Value),
    Unchanged,
    Failed(crate::ObservationFailure),
}

pub fn record_confidential_attestation_refusal(
    journal_path: &Path,
    held_config: &Map<String, Value>,
    raw_reason: &str,
) {
    record_confidential_attestation_refusal_observed(
        journal_path,
        held_config,
        raw_reason,
        None,
        RuntimeRefusalCaller::Generate,
        crate::ObservationPhase::RuntimeEstablish,
        None,
    );
}

/// Observe this invocation's returned publication after its record lock drops.
pub fn record_confidential_attestation_refusal_observed(
    journal_path: &Path,
    held_config: &Map<String, Value>,
    raw_reason: &str,
    sink: Option<&crate::BrainObservationSink>,
    caller: RuntimeRefusalCaller,
    phase: crate::ObservationPhase,
    private_proof: Option<solstone_core_callosum::PrivateProofReceive>,
) {
    let observation = sink.and_then(|sink| {
        sink.begin(match caller {
            RuntimeRefusalCaller::Generate => crate::ObservationCaller::Generate,
            RuntimeRefusalCaller::Transcription => crate::ObservationCaller::Transcription,
        })
    });
    let result = confidential_refusal_result(journal_path, held_config, raw_reason);
    if let Some(mut observation) = observation {
        observation.private_proof = private_proof;
        match result {
            RefusalObservation::Published(record) => observation.publication(phase, &record),
            RefusalObservation::Unchanged => {
                observation.outcome(phase, crate::ObservationOutcome::Unchanged)
            }
            RefusalObservation::Failed(crate::ObservationFailure::PublicationUncertain) => {
                observation.outcome(phase, crate::ObservationOutcome::PublicationUncertain);
            }
            RefusalObservation::Failed(category) => observation.failure(phase, category),
        }
    }
}

/// Record a confidential attestation refusal into the brain record.
fn confidential_refusal_result(
    journal_path: &Path,
    held_config: &Map<String, Value>,
    raw_reason: &str,
) -> RefusalObservation {
    let root = match BrainWriteRoot::open(journal_path) {
        Ok(root) => root,
        Err(error) => {
            log::warn!("the confidential attestation refusal was not recorded: {error}");
            return RefusalObservation::Failed(crate::ObservationFailure::StateUnavailable);
        }
    };
    let checkpoint = || root.checkpoint(journal_path);
    let mapped_reason = valid_spp_reason(raw_reason);
    let diagnostic = spp_reason_diagnostic(raw_reason);
    if derive_active_brain_lane(held_config).lane.as_deref() != Some("spp") {
        return RefusalObservation::Unchanged;
    }
    let Some(key) = root.existing_key() else {
        log::warn!(
            "the confidential attestation refusal was not recorded: fingerprint key unavailable"
        );
        return RefusalObservation::Failed(crate::ObservationFailure::KeyUnavailable);
    };
    let held_fingerprint = match build_active_brain_fingerprint(
        held_config,
        &key,
        None,
        Some(journal_path),
    ) {
        Ok(Some(sha256)) => sha256,
        Ok(None) | Err(_) => {
            log::warn!(
                "the confidential attestation refusal was not recorded: active fingerprint unavailable"
            );
            return RefusalObservation::Failed(crate::ObservationFailure::StateUnavailable);
        }
    };
    let loaded = match fingerprint_for_write(&root, None) {
        Ok(Some(loaded)) => loaded,
        Ok(None) | Err(_) => {
            log::warn!(
                "the confidential attestation refusal was not recorded: active fingerprint unavailable"
            );
            return RefusalObservation::Failed(crate::ObservationFailure::StateUnavailable);
        }
    };
    if held_fingerprint != loaded.sha256 {
        log::warn!(
            "the confidential attestation refusal was not recorded: active fingerprint mismatch"
        );
        return RefusalObservation::Failed(crate::ObservationFailure::FingerprintMismatch);
    }
    let now = Utc::now();
    let lock = match root.lock(record_name(), &checkpoint) {
        Ok(lock) => lock,
        Err(error) => {
            log::warn!("the confidential attestation refusal was not recorded: {error}");
            return RefusalObservation::Failed(crate::ObservationFailure::StateUnavailable);
        }
    };
    match read_current(&root, now) {
        Err(WriterError::Io(error)) => {
            log::warn!("the confidential attestation refusal was not recorded: {error}");
            return RefusalObservation::Failed(crate::ObservationFailure::StateUnavailable);
        }
        Ok(Some((_, current))) => {
            if current.checking.is_none()
                && current.fingerprint_sha256.as_deref() == Some(loaded.sha256.as_str())
                && current
                    .evidence
                    .get("lane_prerequisites")
                    .and_then(Option::as_ref)
                    .is_some_and(|comp| {
                        comp.reason.as_deref() == Some(mapped_reason)
                            && comp
                                .diagnostic
                                .iter()
                                .map(|(key, value)| (key.as_str(), value.as_str()))
                                .eq(diagnostic.iter().map(|(key, value)| {
                                    (key.as_str(), value.as_str().unwrap_or(""))
                                }))
                    })
            {
                return RefusalObservation::Unchanged;
            }
        }
        Ok(None) | Err(_) => {}
    }
    match record_runtime_failure_locked(
        &root,
        &lock,
        RuntimeFailureObservation {
            reason_code: mapped_reason,
            component: "lane_prerequisites",
            expected_fingerprint_sha256: &loaded.sha256,
            diagnostic,
            now,
            bundled_runtime_fingerprint_sha256: None,
        },
    ) {
        Ok(record) => RefusalObservation::Published(record),
        Err(error) => {
            let category = match &error {
                RuntimeFailureInternal::Writer(error) => crate::ObservationFailure::writer(error),
                RuntimeFailureInternal::Rejected(reason, _) => match *reason {
                    "reason_not_recordable" => crate::ObservationFailure::ReasonNotRecordable,
                    "component_reason_not_allowed" => {
                        crate::ObservationFailure::ComponentNotAllowed
                    }
                    "fingerprint_mismatch" => crate::ObservationFailure::FingerprintMismatch,
                    "fingerprint_not_available" => {
                        crate::ObservationFailure::FingerprintUnavailable
                    }
                    "state_unavailable" => crate::ObservationFailure::StateUnavailable,
                    _ => crate::ObservationFailure::UnknownRefusal,
                },
            };
            match error {
                RuntimeFailureInternal::Rejected(reason, detail) => {
                    if let Some(detail) = detail {
                        log::warn!(
                            "the confidential attestation refusal was not recorded: {reason} ({detail})"
                        );
                    } else {
                        log::warn!(
                            "the confidential attestation refusal was not recorded: {reason}"
                        );
                    }
                }
                RuntimeFailureInternal::Writer(error) => {
                    log::warn!("the confidential attestation refusal was not recorded: {error}");
                }
            }
            RefusalObservation::Failed(category)
        }
    }
}

enum RuntimeFailureInternal {
    Rejected(&'static str, Option<String>),
    Writer(WriterError),
}

impl From<WriterError> for RuntimeFailureInternal {
    fn from(error: WriterError) -> Self {
        Self::Writer(error)
    }
}

#[allow(clippy::too_many_arguments)] // Each independent refresh fence and publication admission is retained.
fn begin_refresh_under_lease(
    now: DateTime<Utc>,
    run_id: Option<String>,
    expected: Option<&str>,
    expect_absent: bool,
    bundled_runtime: Option<String>,
    lease: FileLease,
    root: BrainWriteRoot,
    publication_checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<Option<BrainRefreshPermit>, BeginRefreshError> {
    let journal_path = root.admitted_path.as_path();
    let checkpoint = || {
        publication_checkpoint()?;
        root.check()?;
        lease
            .revalidate_bound(root.health.get().expect("leased health"), lease_name())
            .map_err(|error| error.to_string())
    };
    checkpoint().map_err(|error| BeginRefreshError::Writer(WriterError::Io(error)))?;
    let config = match config(&root) {
        Ok(config) => config,
        Err(_) => return Ok(None),
    };
    let resolution = derive_active_brain_lane(&config);
    if resolution.lane.is_none() {
        return if expected.is_some() || expect_absent {
            Err(BeginRefreshError::ExpectedFingerprintStale(
                "active brain fingerprint is unavailable".to_owned(),
            ))
        } else {
            Ok(None)
        };
    }
    if resolution.lane.as_deref() == Some("none") {
        if expected.is_some() || expect_absent {
            return Err(BeginRefreshError::ExpectedFingerprintStale(
                "active brain fingerprint is unavailable".to_owned(),
            ));
        }
        begin_none_lane(&root, now, &resolution, &checkpoint).map_err(BeginRefreshError::Writer)?;
        return Ok(None);
    }
    let key = if expect_absent {
        if root.existing_key().is_some() {
            return Err(BeginRefreshError::ExpectedFingerprintStale(
                "active brain fingerprint is present".to_owned(),
            ));
        }
        generate_fingerprint_key_bound(&root, &checkpoint).map_err(BeginRefreshError::Writer)?
    } else if expected.is_some() {
        root.existing_key().ok_or_else(|| {
            BeginRefreshError::ExpectedFingerprintStale(
                "active brain fingerprint is absent".to_owned(),
            )
        })?
    } else {
        generate_fingerprint_key_bound(&root, &checkpoint).map_err(BeginRefreshError::Writer)?
    };
    let sha256 = match build_active_brain_fingerprint(
        &config,
        &key,
        bundled_runtime.clone().map(Value::String),
        Some(journal_path),
    ) {
        Ok(Some(sha256)) => sha256,
        // Python compares the expected non-null fingerprint to the failed
        // build's `None` first, so this observable error wins over unavailable.
        Ok(None) | Err(_) if expected.is_some() => {
            return Err(BeginRefreshError::ExpectedFingerprintStale(
                "active brain fingerprint changed".to_owned(),
            ));
        }
        Ok(None) | Err(_) if expect_absent => {
            return Err(BeginRefreshError::ExpectedFingerprintStale(
                "active brain fingerprint is unavailable".to_owned(),
            ));
        }
        Ok(None) | Err(_) => return Ok(None),
    };
    if expected.is_some_and(|expected| expected != sha256) {
        return Err(BeginRefreshError::ExpectedFingerprintStale(
            "active brain fingerprint changed".to_owned(),
        ));
    }
    let credential_authority_seen = RefreshCredentialAuthority::capture(&root, &resolution)
        .map_err(BeginRefreshError::Writer)?;
    let credential_generation = match resolution.lane.as_deref() {
        Some("byo-cloud")
            if matches!(
                resolution.provider.as_str(),
                "openai" | "anthropic" | "google"
            ) =>
        {
            Some(
                CredentialProviderGeneration::capture(
                    root.root.clone(),
                    CredentialHoldTarget::Managed {
                        provider: &resolution.provider,
                        credential: config
                            .get("env")
                            .and_then(Value::as_object)
                            .and_then(|env| {
                                env.get(match resolution.provider.as_str() {
                                    "openai" => "OPENAI_API_KEY",
                                    "anthropic" => "ANTHROPIC_API_KEY",
                                    "google" => "GOOGLE_API_KEY",
                                    _ => unreachable!("managed provider"),
                                })
                            })
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .trim(),
                    },
                )
                .map_err(|error| {
                    BeginRefreshError::Writer(WriterError::Conflict(error.to_string()))
                })?,
            )
        }
        Some("byo-endpoint") => {
            let (base_url, credential) = crate::fingerprint::endpoint_binding_parts(&config);
            Some(
                CredentialProviderGeneration::capture(
                    root.root.clone(),
                    CredentialHoldTarget::Endpoint {
                        base_url: &base_url,
                        credential: &credential,
                    },
                )
                .map_err(|error| {
                    BeginRefreshError::Writer(WriterError::Conflict(error.to_string()))
                })?,
            )
        }
        _ => None,
    };
    let run_id = run_id
        .map_or_else(random_id, Ok)
        .map_err(BeginRefreshError::Writer)?;
    let expires_at = now + checking_ttl();
    let lock = root
        .lock(record_name(), &checkpoint)
        .map_err(BeginRefreshError::Writer)?;
    let current = match read_current(&root, now) {
        Ok(current) => current,
        // This record is derived state. A fresh, fenced refresh can replace
        // invalid evidence; configuration and owner outputs are untouched.
        Err(WriterError::Validation(_)) => None,
        Err(error) => return Err(BeginRefreshError::Writer(error)),
    };
    let revision = next_revision(current.as_ref().map(|(_, record)| record));
    let marker_seen = current.as_ref().and_then(|(raw, _)| marker_id(Some(raw)));
    let marker = current
        .as_ref()
        .and_then(|(raw, _)| raw.get("runtime_failure_marker").cloned())
        .unwrap_or(Value::Null);
    let loaded = LoadedFingerprint {
        resolution,
        sha256: sha256.clone(),
    };
    let record = compose_checking_record(CheckingRecord {
        revision,
        now,
        expires_at,
        run_id: &run_id,
        loaded: &loaded,
        evidence: empty_evidence(),
        marker,
        marker_seen: marker_seen.clone(),
    });
    let publish_checkpoint = || {
        checkpoint()?;
        if let Some(authority) = &credential_authority_seen {
            authority
                .checkpoint(&root)
                .map_err(|error| error.to_string())?;
        }
        if let Some(generation) = &credential_generation {
            generation
                .ensure_current()
                .map_err(|error| error.to_string())?;
        }
        fingerprint_checkpoint(&root, &loaded.sha256, bundled_runtime.clone())
    };
    let written = write_record(&root, &lock, &record, now, &publish_checkpoint)
        .map_err(BeginRefreshError::Writer)?;
    let credential_holds_seen =
        crate::record::parse_credential_holds(written.get("credential_holds"))
            .map_err(|error| BeginRefreshError::Writer(error.into()))?;
    Ok(Some(BrainRefreshPermit {
        run_id,
        started_at: now,
        expires_at,
        fingerprint_sha256: sha256,
        checking_revision: revision,
        runtime_failure_marker_seen: marker_seen,
        credential_holds_seen,
        credential_generation: credential_generation.map(Box::new),
        credential_authority_seen: credential_authority_seen.map(Box::new),
        credential_publication_deadline: None,
        bundled_runtime_fingerprint_sha256: bundled_runtime,
        root: Box::new(root),
        _lease: lease,
    }))
}

struct LoadedFingerprint {
    resolution: LaneResolution,
    sha256: String,
}

fn fingerprint_checkpoint(
    root: &BrainWriteRoot,
    expected: &str,
    bundled_runtime: Option<String>,
) -> Result<(), String> {
    let current =
        fingerprint_for_write(root, bundled_runtime).map_err(|error| error.to_string())?;
    if !current.is_some_and(|current| current.sha256 == expected) {
        return Err("brain fingerprint changed before publication".to_owned());
    }
    Ok(())
}

fn fingerprint_for_write(
    root: &BrainWriteRoot,
    bundled_runtime: Option<String>,
) -> Result<Option<LoadedFingerprint>, WriterError> {
    let config = config(root)?;
    let Some(key) = root.existing_key() else {
        return Ok(None);
    };
    let resolution = derive_active_brain_lane(&config);
    let sha256 = build_active_brain_fingerprint(
        &config,
        &key,
        bundled_runtime.map(Value::String),
        Some(&root.admitted_path),
    )
    .map_err(|error| WriterError::Fingerprint(error.to_string()))?
    .ok_or_else(|| {
        WriterError::Fingerprint("active brain fingerprint is unavailable".to_owned())
    })?;
    Ok(Some(LoadedFingerprint { resolution, sha256 }))
}

fn begin_none_lane(
    root: &BrainWriteRoot,
    now: DateTime<Utc>,
    resolution: &LaneResolution,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<(), WriterError> {
    let lock = root.lock(record_name(), checkpoint)?;
    let current = match read_current(root, now) {
        Ok(current) => current,
        Err(WriterError::Validation(_)) => None,
        Err(error) => return Err(error),
    };
    let record = compose_none_record(
        now,
        resolution,
        next_revision(current.as_ref().map(|(_, record)| record)),
    )?;
    let publish_checkpoint = || {
        checkpoint()?;
        let current = config(root).map_err(|error| error.to_string())?;
        if derive_active_brain_lane(&current).lane.as_deref() != Some("none") {
            return Err("brain lane changed before publication".to_owned());
        }
        Ok(())
    };
    write_record(root, &lock, &record, now, &publish_checkpoint).map(|_| ())
}

fn compose_none_record(
    now: DateTime<Utc>,
    resolution: &LaneResolution,
    revision: u64,
) -> Result<Value, WriterError> {
    Ok(compose_direct_record(DirectRecord {
        revision,
        reason_code: "thinking_engine_not_chosen".to_owned(),
        active_lane: "none".to_owned(),
        active_provider: Some(resolution.provider.clone()),
        active_model: resolution.model.clone(),
        fingerprint_sha256: None,
        evidence: Value::Object({
            let mut evidence = empty_evidence();
            evidence.insert(
                "configuration".to_owned(),
                component_for_reason("thinking_engine_not_chosen", Map::new(), now)?,
            );
            evidence
        }),
        checking: Value::Null,
        marker: Value::Null,
        diagnostic: Value::Object(Map::new()),
        now,
    }))
}

struct CheckingRecord<'a> {
    revision: u64,
    now: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    run_id: &'a str,
    loaded: &'a LoadedFingerprint,
    evidence: Map<String, Value>,
    marker: Value,
    marker_seen: Option<String>,
}

fn compose_checking_record(input: CheckingRecord<'_>) -> Value {
    json!({
        "schema_version": local_contract().brain_state.schema_version,
        "revision": input.revision,
        "aggregate_state": "checking",
        "reason_code": "brain_check_in_progress",
        "active_lane": input.loaded.resolution.lane.clone(),
        "active_provider": input.loaded.resolution.provider.clone(),
        "active_model": input.loaded.resolution.model.clone(),
        "fingerprint_sha256": input.loaded.sha256,
        "checking": {
            "run_id": input.run_id,
            "started_at": iso(input.now),
            "expires_at": iso(input.expires_at),
            "fingerprint_sha256": input.loaded.sha256,
            "checking_revision": input.revision,
            "runtime_failure_marker_seen": input.marker_seen,
        },
        "evidence": input.evidence,
        "runtime_failure_marker": input.marker,
        "diagnostic": {},
        "updated_at": iso(input.now),
    })
}

fn compose_record_from_evidence(
    evidence_value: Value,
    loaded: &LoadedFingerprint,
    revision: u64,
    now: DateTime<Utc>,
    checking: Value,
    marker: Value,
    diagnostic: Value,
) -> Result<Value, WriterError> {
    let evidence = parse_evidence(Some(&evidence_value), now)?;
    let marker_typed = (!marker.is_null())
        .then(|| parse_runtime_failure_marker(&marker, now))
        .transpose()?;
    let provisional = BrainStateRecord {
        schema_version: local_contract().brain_state.schema_version,
        revision,
        updated_at: now,
        aggregate_state: "ready".to_owned(),
        reason_code: None,
        active_lane: loaded.resolution.lane.clone().unwrap_or_default(),
        active_provider: Some(loaded.resolution.provider.clone()),
        active_model: loaded.resolution.model.clone(),
        fingerprint_sha256: Some(loaded.sha256.clone()),
        evidence,
        checking: None,
        runtime_failure_marker: marker_typed,
        credential_holds: BTreeMap::new(),
        credential_observations: BTreeMap::new(),
        diagnostic: BTreeMap::new(),
    };
    let runtime_reason = provisional
        .runtime_failure_marker
        .as_ref()
        .and_then(|marker| {
            (u64::try_from(marker.revision).ok() == Some(revision))
                .then_some(marker.reason.as_str())
        });
    let (aggregate, reason) = reduce_evidence_with_runtime(&provisional, now, true, runtime_reason);
    Ok(compose_direct_record(DirectRecord {
        revision,
        reason_code: reason.unwrap_or_default(),
        active_lane: provisional.active_lane,
        active_provider: provisional.active_provider,
        active_model: provisional.active_model,
        fingerprint_sha256: provisional.fingerprint_sha256,
        evidence: evidence_value,
        checking,
        marker,
        diagnostic,
        now,
    })
    .tap_aggregate(aggregate))
}

trait WithAggregate {
    fn tap_aggregate(self, aggregate: String) -> Self;
}

impl WithAggregate for Value {
    fn tap_aggregate(mut self, aggregate: String) -> Self {
        self.as_object_mut()
            .expect("composed record is an object")
            .insert("aggregate_state".to_owned(), Value::String(aggregate));
        self
    }
}

struct DirectRecord {
    revision: u64,
    reason_code: String,
    active_lane: String,
    active_provider: Option<String>,
    active_model: Option<String>,
    fingerprint_sha256: Option<String>,
    evidence: Value,
    checking: Value,
    marker: Value,
    diagnostic: Value,
    now: DateTime<Utc>,
}

fn compose_direct_record(input: DirectRecord) -> Value {
    let aggregate = local_contract()
        .brain_state
        .reason_to_aggregate
        .get(&input.reason_code)
        .cloned()
        .unwrap_or_else(|| "ready".to_owned());
    json!({
        "schema_version": local_contract().brain_state.schema_version,
        "revision": input.revision,
        "aggregate_state": aggregate,
        "reason_code": if input.reason_code.is_empty() { Value::Null } else { Value::String(input.reason_code) },
        "active_lane": input.active_lane,
        "active_provider": input.active_provider,
        "active_model": input.active_model,
        "fingerprint_sha256": input.fingerprint_sha256,
        "checking": input.checking,
        "evidence": input.evidence,
        "runtime_failure_marker": input.marker,
        "diagnostic": input.diagnostic,
        "updated_at": iso(input.now),
    })
}

fn raw_evidence_value(value: &Value) -> Result<Map<String, Value>, WriterError> {
    object_field(value, "evidence").cloned()
}

fn safe_prerequisite_evidence(
    raw: &Value,
    current: &BrainStateRecord,
    now: DateTime<Utc>,
) -> Option<Map<String, Value>> {
    if current.active_lane != "spp" || record_timestamp_invalid(current, now) {
        return None;
    }
    for name in ["configuration", "generate"] {
        let component = current.evidence.get(name).and_then(Option::as_ref)?;
        if component.status != "ok" || component.expires_at.is_none_or(|expires| now >= expires) {
            return None;
        }
    }
    raw_evidence_value(raw).ok()
}

fn record_timestamp_invalid(record: &BrainStateRecord, now: DateTime<Utc>) -> bool {
    if record.updated_at > now
        || record
            .checking
            .as_ref()
            .is_some_and(|checking| checking.started_at > now)
        || record
            .runtime_failure_marker
            .as_ref()
            .is_some_and(|marker| marker.recorded_at > now)
    {
        return true;
    }
    record.evidence.values().flatten().any(|component| {
        component.observed_at > now
            || component
                .expires_at
                .is_none_or(|expires| expires < component.observed_at)
    })
}

fn assert_finish_allowed(
    permit: &BrainRefreshPermit,
    current: &BrainStateRecord,
    now: DateTime<Utc>,
) -> Result<(), WriterError> {
    if current.credential_holds != permit.credential_holds_seen {
        return Err(WriterError::Conflict(
            "credential rejection changed during refresh".to_owned(),
        ));
    }
    // Ownership is represented by the unforgeable, still-live FileLease field.
    if now >= permit.expires_at {
        return Err(WriterError::Conflict(
            "brain refresh permit expired".to_owned(),
        ));
    }
    let checking = current.checking.as_ref().ok_or_else(|| {
        WriterError::Conflict("brain refresh checking marker is absent".to_owned())
    })?;
    if current.revision != permit.checking_revision {
        return Err(WriterError::Conflict(
            "brain refresh record revision changed".to_owned(),
        ));
    }
    if checking.run_id != permit.run_id {
        return Err(WriterError::Conflict(
            "brain refresh run id changed".to_owned(),
        ));
    }
    if u64::try_from(checking.checking_revision).ok() != Some(permit.checking_revision) {
        return Err(WriterError::Conflict(
            "brain refresh revision changed".to_owned(),
        ));
    }
    if checking.runtime_failure_marker_seen != permit.runtime_failure_marker_seen {
        return Err(WriterError::Conflict(
            "brain runtime failure marker changed".to_owned(),
        ));
    }
    Ok(())
}

fn target_component(reason_code: &str) -> Option<&str> {
    let vocabulary = &local_contract().brain_state;
    vocabulary.component_order.iter().find_map(|component| {
        vocabulary
            .evidence_reason_codes
            .get(component)
            .is_some_and(|reasons| reasons.iter().any(|reason| reason == reason_code))
            .then_some(component.as_str())
    })
}

fn component_for_reason(
    reason_code: &str,
    diagnostic: Map<String, Value>,
    now: DateTime<Utc>,
) -> Result<Value, WriterError> {
    let component = target_component(reason_code)
        .ok_or_else(|| WriterError::Conflict("reason is not recordable evidence".to_owned()))?;
    component_for_reason_in(component, reason_code, diagnostic, now)
}

fn component_for_reason_in(
    component: &str,
    reason_code: &str,
    diagnostic: Map<String, Value>,
    now: DateTime<Utc>,
) -> Result<Value, WriterError> {
    let status = component_status_for_reason(reason_code)?;
    let value = json!({
        "status": status,
        "observed_at": iso(now),
        "reason_code": reason_code,
        "diagnostic": diagnostic,
    });
    parse_component(component, &value, now)?;
    Ok(value)
}

fn empty_evidence() -> Map<String, Value> {
    local_contract()
        .brain_state
        .record_fields
        .evidence
        .iter()
        .map(|component| (component.clone(), Value::Null))
        .collect()
}

fn write_record(
    root: &BrainWriteRoot,
    lock: &BoundParentLock,
    record: &Value,
    now: DateTime<Utc>,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<Value, WriterError> {
    checkpoint().map_err(WriterError::Io)?;
    let mut record = record.clone();
    if record.get("credential_holds").is_none()
        && let Some(holds) = read_credential_hold_authority(root)?
    {
        record
            .as_object_mut()
            .expect("composed Brain object")
            .insert("credential_holds".to_owned(), holds);
    }
    if record.get("credential_observations").is_none()
        && let Some(observations) = read_credential_observation_authority(root)?
    {
        record
            .as_object_mut()
            .expect("composed Brain object")
            .insert("credential_observations".to_owned(), observations);
    }
    validate_brain_state_record(&record, now)?;
    let bytes = serialize_json_bytes(
        &record,
        JsonWriteOptions {
            mode: Some(BRAIN_FILE_MODE),
            indent: Some(2),
            sort_keys: true,
        },
    )
    .map_err(|error| WriterError::Io(error.to_string()))?;
    root.publish(lock, record_name(), &bytes, checkpoint)?;
    Ok(record)
}

fn read_credential_hold_authority(root: &BrainWriteRoot) -> Result<Option<Value>, WriterError> {
    let Some(bytes) = root.read(record_name())? else {
        return Ok(None);
    };
    let value: Value = serde_json::from_slice(&bytes).map_err(|error| {
        WriterError::Conflict(format!("credential hold authority is unreadable: {error}"))
    })?;
    let object = value.as_object().ok_or_else(|| {
        WriterError::Conflict("credential hold authority is not an object".to_owned())
    })?;
    crate::record::parse_credential_holds(object.get("credential_holds"))?;
    Ok(object.get("credential_holds").cloned())
}

fn read_credential_observation_authority(
    root: &BrainWriteRoot,
) -> Result<Option<Value>, WriterError> {
    let Some(bytes) = root.read(record_name())? else {
        return Ok(None);
    };
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| {
        WriterError::Conflict("credential observation authority is unreadable".to_owned())
    })?;
    let object = value.as_object().ok_or_else(|| {
        WriterError::Conflict("credential observation authority is not an object".to_owned())
    })?;
    crate::record::parse_credential_observations(object.get("credential_observations"))?;
    Ok(object.get("credential_observations").cloned())
}

fn read_current(
    root: &BrainWriteRoot,
    now: DateTime<Utc>,
) -> Result<Option<(Value, BrainStateRecord)>, WriterError> {
    let Some(bytes) = root.read(record_name())? else {
        return Ok(None);
    };
    let value = serde_json::from_slice(&bytes).map_err(|error| {
        WriterError::Validation(ValidationError {
            path: "record".to_owned(),
            reason: error.to_string(),
        })
    })?;
    let parsed = validate_brain_state_record(&value, now)?;
    Ok(Some((value, parsed)))
}

fn config(root: &BrainWriteRoot) -> Result<Map<String, Value>, WriterError> {
    root.check().map_err(WriterError::Io)?;
    let read = solstone_core_journal_config::read_journal_config_bound(&root.root)
        .map_err(|error| WriterError::Config(error.to_string()))?;
    root.check().map_err(WriterError::Io)?;
    Ok(read.config.unwrap_or_default())
}

/// Exclusive flock on the brain record sidecar. Hidden so writer component
/// tests contend on the same path and mode as production.
#[doc(hidden)]
pub fn hold_record_lock(path: &Path) -> Result<solstone_core_journal_io::FileLock, WriterError> {
    hold_lock(
        path,
        LockOptions {
            mode: Some(BRAIN_FILE_MODE),
            ..LockOptions::default()
        },
    )
    .map_err(lock_error)
}

fn next_revision(record: Option<&BrainStateRecord>) -> u64 {
    record.map_or(1, |record| record.revision + 1)
}

fn marker_id(record: Option<&Value>) -> Option<String> {
    record
        .and_then(|record| record.get("runtime_failure_marker"))
        .and_then(Value::as_object)
        .and_then(|marker| marker.get("marker_id"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn checking_ttl() -> Duration {
    Duration::seconds(local_contract().brain_state.checking_ttl_seconds as i64)
}

fn iso(now: DateTime<Utc>) -> String {
    now.to_rfc3339()
}

fn validate_expected(expected: Option<&str>, expect_absent: bool) -> Result<(), BeginRefreshError> {
    if expected.is_some_and(|value| !is_sha256(value)) {
        return Err(BeginRefreshError::ExpectedFingerprintStale(
            "expected_active_fingerprint_sha256: expected SHA-256 hex string".to_owned(),
        ));
    }
    if expected.is_some() && expect_absent {
        return Err(BeginRefreshError::InvalidArgument(
            "expected active fingerprint and expected absence are mutually exclusive".to_owned(),
        ));
    }
    Ok(())
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

// ⚠ These two are the crate's ONLY use of a cryptographic RNG, and the
// dependency that served them pulled a C build step into a crate the iOS canary
// compiles. `getrandom` is the OS entropy syscall and nothing else: no C, no
// linkage, no native-dependency release proof owed. `docs/PORTING.md` § native
// dependency release proof is the rule that makes the distinction load-bearing.
fn secure_key() -> Result<[u8; FINGERPRINT_KEY_BYTES], WriterError> {
    let mut key = [0_u8; FINGERPRINT_KEY_BYTES];
    fill_random(&mut key).map_err(|_| WriterError::Random)?;
    Ok(key)
}

fn random_id() -> Result<String, WriterError> {
    let mut bytes = [0_u8; 16];
    fill_random(&mut bytes).map_err(|_| WriterError::Random)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn random_observation_id() -> Result<String, WriterError> {
    let mut bytes = [0_u8; 32];
    fill_random(&mut bytes).map_err(|_| WriterError::Random)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn object_field<'a>(value: &'a Value, field: &str) -> Result<&'a Map<String, Value>, WriterError> {
    value
        .get(field)
        .and_then(Value::as_object)
        .ok_or_else(|| WriterError::Conflict(format!("{field} must be an object")))
}

fn rejected(reason: &str, error: Option<String>) -> RuntimeFailureResult {
    RuntimeFailureResult {
        accepted: false,
        record: None,
        rejected_reason: Some(reason.to_owned()),
        error,
    }
}

fn lock_error(error: solstone_core_journal_io::LockError) -> WriterError {
    WriterError::Io(error.to_string())
}

fn lease_error(error: solstone_core_journal_io::LeaseError) -> WriterError {
    WriterError::Io(error.to_string())
}

#[cfg(test)]
mod tests {
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use chrono::DateTime;

    use super::*;
    use crate::InspectionStatus;
    use crate::fixture::projection_fixture;
    use crate::inspect_brain_state;

    static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    struct TestJournal(PathBuf);

    impl TestJournal {
        fn new() -> Self {
            let sequence = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "solstone-core-brain-writer-{}-{}-{sequence}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("time after epoch")
                    .as_nanos()
            ));
            fs::create_dir_all(&path).expect("journal directory");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn write_config(&self, name: &str) {
            let config = projection_fixture()
                .configs
                .get(name)
                .expect("fixture config");
            let path = self.path().join("config/journal.json");
            fs::create_dir_all(path.parent().expect("config parent")).unwrap();
            fs::write(path, serde_json::to_vec(config).unwrap()).unwrap();
        }

        fn write_fixture_key(&self) {
            let hex = &projection_fixture().hmac_key_hex;
            let key = (0..hex.len())
                .step_by(2)
                .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).unwrap())
                .collect::<Vec<_>>();
            let path = brain_fingerprint_key_path(self.path());
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, key).unwrap();
        }

        fn seed_record(&self, name: &str) {
            let record = projection_fixture()
                .records
                .get(name)
                .expect("fixture record");
            let path = brain_state_path(self.path());
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, serde_json::to_vec(record).unwrap()).unwrap();
        }

        fn seed_value(&self, record: &Value) {
            let path = brain_state_path(self.path());
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, serde_json::to_vec(record).unwrap()).unwrap();
        }
    }

    impl Drop for TestJournal {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn fixture_now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(&projection_fixture().now)
            .expect("fixture now")
            .with_timezone(&Utc)
    }

    fn ready_outcome() -> Value {
        projection_fixture()
            .records
            .get("lane_byo_cloud/ready")
            .expect("ready record")
            .get("evidence")
            .expect("evidence")
            .clone()
    }

    fn begin_cloud(journal: &TestJournal) -> BrainRefreshPermit {
        journal.write_config("lane_byo_cloud");
        begin_refresh(
            journal.path(),
            fixture_now(),
            Some("run".to_owned()),
            None,
            false,
            None,
        )
        .unwrap()
        .expect("refresh permit")
    }

    fn record_bytes(journal: &TestJournal) -> Vec<u8> {
        fs::read(brain_state_path(journal.path())).unwrap()
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn checked_finish_observes_the_held_record_lock() {
        let journal = TestJournal::new();
        let permit = begin_cloud(&journal);
        let before = record_bytes(&journal);
        let stopped = std::cell::Cell::new(false);
        let error = finish_refresh_checked(
            journal.path(),
            permit,
            ready_outcome(),
            fixture_now(),
            None,
            &|| {
                if solstone_core_journal_io::lock_is_held(brain_state_path(journal.path())).unwrap()
                {
                    stopped.set(true);
                }
                if stopped.get() {
                    Err("stopped".into())
                } else {
                    Ok(())
                }
            },
        )
        .unwrap_err();
        assert!(matches!(error, WriterError::Cancelled(message) if message == "stopped"));
        assert_eq!(record_bytes(&journal), before);
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn checked_finish_cancelled_while_blocked_does_not_publish() {
        use std::sync::{Arc, atomic::AtomicBool};
        use std::thread;

        let journal = TestJournal::new();
        let permit = begin_cloud(&journal);
        let before = record_bytes(&journal);
        let path = brain_state_path(journal.path());
        let holder = hold_record_lock(&path).unwrap();
        let started = Arc::new(AtomicBool::new(false));
        let cancel = Arc::new(AtomicBool::new(false));
        let started_flag = Arc::clone(&started);
        let cancel_flag = Arc::clone(&cancel);
        let journal_path = journal.path().to_path_buf();
        let outcome = ready_outcome();
        let now = fixture_now();
        let worker = thread::spawn(move || {
            started_flag.store(true, Ordering::Release);
            finish_refresh_checked(&journal_path, permit, outcome, now, None, &|| {
                if cancel_flag.load(Ordering::Acquire) {
                    Err("stopped".into())
                } else {
                    Ok(())
                }
            })
        });
        while !started.load(Ordering::Acquire) {
            thread::yield_now();
        }
        assert!(!cancel.load(Ordering::Acquire));
        cancel.store(true, Ordering::Release);
        drop(holder);
        let error = worker.join().unwrap().unwrap_err();
        assert!(matches!(error, WriterError::Cancelled(_)));
        assert_eq!(record_bytes(&journal), before);
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn begin_refresh_cancelled_after_key_staging_does_not_publish() {
        let journal = TestJournal::new();
        journal.write_config("lane_byo_cloud");
        let key_path = brain_fingerprint_key_path(journal.path());
        let cancelled = std::cell::Cell::new(false);
        let checkpoint = || {
            if fs::read_dir(key_path.parent().unwrap()).is_ok_and(|mut entries| {
                entries.any(|entry| {
                    entry.is_ok_and(|entry| {
                        solstone_core_journal_io::is_publication_candidate_name(&entry.file_name())
                    })
                })
            }) {
                cancelled.set(true);
            }
            if cancelled.get() {
                Err("stopped at key stage".to_owned())
            } else {
                Ok(())
            }
        };
        let error = begin_refresh_checked(
            journal.path(),
            fixture_now(),
            None,
            None,
            false,
            None,
            &checkpoint,
        )
        .unwrap_err();
        assert!(
            cancelled.get(),
            "the original key owner must reach its actual stage"
        );
        assert!(matches!(
            error,
            BeginRefreshError::Writer(WriterError::Cancelled(_))
        ));
        assert!(!key_path.exists());
        assert!(!brain_state_path(journal.path()).exists());
    }

    #[test]
    fn generated_timestamps_use_python_isoformat_utc_style() {
        assert!(iso(fixture_now()).ends_with("+00:00"));
        assert!(!iso(fixture_now()).ends_with('Z'));
    }

    #[test]
    fn begin_refresh_prefers_expected_fingerprint_changed_over_unavailable() {
        let journal = TestJournal::new();
        journal.write_config("lane_bundled");
        generate_fingerprint_key(journal.path()).unwrap();
        let expected = "a".repeat(64);
        let error = begin_refresh(
            journal.path(),
            fixture_now(),
            None,
            Some(&expected),
            false,
            None,
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "active brain fingerprint changed");
    }

    #[test]
    fn begin_refresh_absent_contract_reports_unavailable_for_failed_build() {
        let journal = TestJournal::new();
        journal.write_config("lane_bundled");
        let error =
            begin_refresh(journal.path(), fixture_now(), None, None, true, None).unwrap_err();
        assert_eq!(error.to_string(), "active brain fingerprint is unavailable");
    }

    #[test]
    fn reachable_writer_selector_has_the_documented_24_cases() {
        assert_eq!(REACHABLE_WRITE_CASES.len(), 24);
        assert!(REACHABLE_WRITE_CASES.iter().all(|name| {
            name.starts_with("lane_bundled/")
                || name.starts_with("lane_byo_cloud/")
                || name.starts_with("lane_byo_endpoint/")
                || name.starts_with("lane_spp/")
        }));
        assert!(REACHABLE_WRITE_CASES.iter().all(|name| {
            !name.contains("fingerprint_")
                && !name.contains("marker_")
                && projection_fixture()
                    .records
                    .get(*name)
                    .is_some_and(|record| record["checking"].is_null())
        }));
    }

    #[test]
    fn reachable_writer_cases_are_emitted_by_finish_refresh() {
        for name in REACHABLE_WRITE_CASES {
            let fixture = projection_fixture();
            let target = fixture.records.get(name).expect("reachable fixture record");
            let (lane, _) = name.split_once('/').expect("fixture case name");
            let now = DateTime::parse_from_rfc3339(
                target["updated_at"].as_str().expect("fixture updated_at"),
            )
            .expect("fixture timestamp")
            .with_timezone(&Utc);
            let journal = TestJournal::new();
            journal.write_config(lane);
            journal.write_fixture_key();

            let mut prior = fixture.records[&format!("{lane}/ready")].clone();
            prior["revision"] = json!(1);
            journal.seed_value(&prior);

            let bundled_runtime = (lane == "lane_bundled")
                .then(|| fixture.bundled_runtime_fingerprint_sha256.clone());
            let permit = begin_refresh(
                journal.path(),
                now,
                Some("writer-parity".to_owned()),
                None,
                false,
                bundled_runtime.clone(),
            )
            .expect("begin refresh")
            .expect("refresh permit");
            let written = finish_refresh(
                journal.path(),
                permit,
                target["evidence"].clone(),
                now,
                bundled_runtime,
            )
            .expect("finish refresh");
            assert_eq!(written, *target, "writer parity mismatch: {name}");
            let disk: Value = serde_json::from_slice(&record_bytes(&journal)).expect("record JSON");
            assert_eq!(disk, *target, "writer parity disk mismatch: {name}");
        }
    }

    #[test]
    fn composition_matches_every_projection_fixture_record() {
        for (name, record) in &projection_fixture().records {
            let now = DateTime::parse_from_rfc3339(
                record["updated_at"].as_str().expect("fixture updated_at"),
            )
            .unwrap()
            .with_timezone(&Utc);
            let composed = compose_direct_record(DirectRecord {
                revision: record["revision"].as_u64().expect("fixture revision"),
                reason_code: record["reason_code"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                active_lane: record["active_lane"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                active_provider: record["active_provider"].as_str().map(str::to_owned),
                active_model: record["active_model"].as_str().map(str::to_owned),
                fingerprint_sha256: record["fingerprint_sha256"].as_str().map(str::to_owned),
                evidence: record["evidence"].clone(),
                checking: record["checking"].clone(),
                marker: record["runtime_failure_marker"].clone(),
                diagnostic: record["diagnostic"].clone(),
                now,
            });
            assert_eq!(&composed, record, "fixture composition mismatch: {name}");
        }
    }

    #[test]
    fn none_lane_write_needs_no_refresh_lease_and_validates() {
        let journal = TestJournal::new();
        journal.write_config("lane_none");
        let permit = begin_refresh(
            journal.path(),
            fixture_now(),
            Some("run".to_owned()),
            None,
            false,
            None,
        )
        .unwrap();
        assert!(permit.is_none());
        assert!(!brain_refresh_lease_path(journal.path()).exists());
        let record = fs::read(brain_state_path(journal.path())).unwrap();
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(brain_state_path(journal.path()))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let value: Value = serde_json::from_slice(&record).unwrap();
        validate_brain_state_record(&value, fixture_now()).unwrap();
        assert_eq!(value["reason_code"], "thinking_engine_not_chosen");
        assert!(value["runtime_failure_marker"].is_null());
        let evidence = value["evidence"].as_object().expect("evidence object");
        assert_eq!(evidence.len(), 3);
        assert!(evidence["configuration"].is_object());
        for component in ["lane_prerequisites", "generate"] {
            assert!(evidence[component].is_null(), "{component} must be null");
        }
    }

    #[test]
    fn checking_record_validates_without_a_live_lease() {
        let journal = TestJournal::new();
        let permit = begin_cloud(&journal);
        drop(permit);
        let record: Value = serde_json::from_slice(&record_bytes(&journal)).unwrap();
        validate_brain_state_record(&record, fixture_now()).unwrap();
        assert_eq!(record["reason_code"], "brain_check_in_progress");
    }

    /// Whether a staged namespace replacement began. Windows refuses to rename a
    /// directory while a handle below it is open, and a live refresh holds one:
    /// the replacement cannot begin, which is the stronger outcome.
    fn replacement_began(staged: std::io::Result<()>) -> bool {
        match staged {
            Ok(()) => true,
            Err(error) if cfg!(windows) && error.kind() == std::io::ErrorKind::PermissionDenied => {
                false
            }
            Err(error) => panic!("stage the replacement: {error}"),
        }
    }

    fn assert_replaced_root_refused(abandon: bool) {
        let journal = TestJournal::new();
        let permit = begin_cloud(&journal);
        let original_record = record_bytes(&journal);
        let original_key = fs::read(brain_fingerprint_key_path(journal.path())).unwrap();
        let displaced = TestJournal::new();
        fs::remove_dir(displaced.path()).unwrap();
        if !replacement_began(fs::rename(journal.path(), displaced.path())) {
            assert!(!displaced.path().exists());
            assert_eq!(record_bytes(&journal), original_record);
            let result = if abandon {
                abandon_refresh(
                    journal.path(),
                    permit,
                    "provider_unavailable",
                    Map::new(),
                    fixture_now(),
                )
            } else {
                finish_refresh(journal.path(), permit, ready_outcome(), fixture_now(), None)
            };
            result.expect("the permit still names the only root");
            assert_ne!(record_bytes(&journal), original_record);
            return;
        }
        fs::create_dir(journal.path()).unwrap();
        journal.write_config("lane_byo_cloud");
        fs::create_dir(journal.path().join("health")).unwrap();
        fs::write(brain_state_path(journal.path()), &original_record).unwrap();
        fs::write(brain_fingerprint_key_path(journal.path()), original_key).unwrap();
        let result = if abandon {
            abandon_refresh(
                journal.path(),
                permit,
                "generate_failed",
                Map::new(),
                fixture_now(),
            )
        } else {
            finish_refresh(journal.path(), permit, ready_outcome(), fixture_now(), None)
        };
        assert!(result.is_err());
        assert_eq!(record_bytes(&journal), original_record);
        assert_eq!(record_bytes(&displaced), original_record);
        assert!(!brain_refresh_lease_path(journal.path()).exists());
    }

    #[test]
    fn finish_refresh_refuses_replacement_root_with_identical_config_and_checking_record() {
        assert_replaced_root_refused(false);
    }

    #[test]
    fn abandon_refresh_refuses_replacement_root_with_identical_config_and_checking_record() {
        assert_replaced_root_refused(true);
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn refresh_refuses_replaced_health_with_identical_key_and_record() {
        let journal = TestJournal::new();
        let permit = begin_cloud(&journal);
        let original_record = record_bytes(&journal);
        let original_key = fs::read(brain_fingerprint_key_path(journal.path())).unwrap();
        let original_health = journal.path().join("original-health");
        if !replacement_began(fs::rename(journal.path().join("health"), &original_health)) {
            assert!(!original_health.exists());
            assert_eq!(record_bytes(&journal), original_record);
            finish_refresh(journal.path(), permit, ready_outcome(), fixture_now(), None)
                .expect("the permit still names the only health directory");
            assert_ne!(record_bytes(&journal), original_record);
            return;
        }
        fs::create_dir(journal.path().join("health")).unwrap();
        fs::write(brain_state_path(journal.path()), &original_record).unwrap();
        fs::write(brain_fingerprint_key_path(journal.path()), original_key).unwrap();
        assert!(
            finish_refresh(journal.path(), permit, ready_outcome(), fixture_now(), None).is_err()
        );
        assert_eq!(record_bytes(&journal), original_record);
        assert_eq!(
            fs::read(original_health.join(record_name())).unwrap(),
            original_record
        );
        assert!(
            !journal
                .path()
                .join("health")
                .join(sidecar_name(record_name()))
                .exists()
        );
        assert!(!brain_refresh_lease_path(journal.path()).exists());
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn refresh_refuses_replaced_lease_without_unlocking_the_original() {
        let journal = TestJournal::new();
        let permit = begin_cloud(&journal);
        let original_record = record_bytes(&journal);
        let saved_lease = journal.path().join("health/original-lease");
        fs::rename(brain_refresh_lease_path(journal.path()), &saved_lease).unwrap();
        fs::write(brain_refresh_lease_path(journal.path()), []).unwrap();
        assert!(permit.checkpoint(journal.path()).is_err());
        assert!(
            solstone_core_journal_io::acquire_file_lease(&saved_lease, LeaseOptions::default())
                .unwrap()
                .is_none()
        );
        assert!(
            finish_refresh(journal.path(), permit, ready_outcome(), fixture_now(), None).is_err()
        );
        assert_eq!(record_bytes(&journal), original_record);
        assert!(
            solstone_core_journal_io::acquire_file_lease(&saved_lease, LeaseOptions::default())
                .unwrap()
                .is_some()
        );
    }

    #[cfg(all(unix, feature = "full-tests"))]
    #[test]
    fn final_rename_root_replacement_preserves_the_record_and_cleans_the_stage() {
        use solstone_core_journal_io::{
            BoundPublicationPrimitive, run_with_bound_publication_barrier,
        };
        let journal = TestJournal::new();
        let permit = begin_cloud(&journal);
        let before = record_bytes(&journal);
        let mut original_entries = fs::read_dir(journal.path().join("health"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        original_entries.sort();
        let displaced = TestJournal::new();
        fs::remove_dir(displaced.path()).unwrap();
        let original = journal.path().to_path_buf();
        let destination = displaced.path().to_path_buf();
        let (result, reached) = run_with_bound_publication_barrier(
            BoundPublicationPrimitive::Rename,
            1,
            move || {
                fs::rename(&original, &destination).unwrap();
                fs::create_dir(&original).unwrap();
                fs::write(original.join("replacement-sentinel"), b"untouched").unwrap();
            },
            || finish_refresh(journal.path(), permit, ready_outcome(), fixture_now(), None),
        );
        assert!(reached);
        assert!(matches!(result, Err(WriterError::Io(_))));
        assert_eq!(record_bytes(&displaced), before);
        let replacement_entries = fs::read_dir(journal.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        assert_eq!(
            replacement_entries,
            [OsString::from("replacement-sentinel")]
        );
        let mut retained_entries = fs::read_dir(displaced.path().join("health"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        retained_entries.sort();
        assert_eq!(retained_entries, original_entries);
    }

    #[cfg(all(unix, feature = "full-tests"))]
    #[test]
    fn final_rename_rechecks_the_current_model_and_protected_key() {
        use solstone_core_journal_io::{
            BoundPublicationPrimitive, run_with_bound_publication_barrier,
        };
        for replace_key in [false, true] {
            let journal = TestJournal::new();
            let permit = begin_cloud(&journal);
            let before = record_bytes(&journal);
            let root_path = journal.path().to_path_buf();
            let (result, reached) = run_with_bound_publication_barrier(
                BoundPublicationPrimitive::Rename,
                1,
                move || {
                    if replace_key {
                        fs::write(
                            brain_fingerprint_key_path(&root_path),
                            [0x77; FINGERPRINT_KEY_BYTES],
                        )
                        .unwrap();
                    } else {
                        let config_path = root_path.join("config/journal.json");
                        let mut config: Value =
                            serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
                        config["providers"]["active"]["model"] = json!("changed-after-staging");
                        fs::write(config_path, serde_json::to_vec(&config).unwrap()).unwrap();
                    }
                },
                || finish_refresh(journal.path(), permit, ready_outcome(), fixture_now(), None),
            );
            assert!(reached);
            assert!(result.is_err());
            assert_eq!(record_bytes(&journal), before);
            if replace_key {
                assert_eq!(
                    fs::read(brain_fingerprint_key_path(journal.path())).unwrap(),
                    [0x77; FINGERPRINT_KEY_BYTES]
                );
            } else {
                let config: Value = serde_json::from_slice(
                    &fs::read(journal.path().join("config/journal.json")).unwrap(),
                )
                .unwrap();
                assert_eq!(
                    config["providers"]["active"]["model"],
                    "changed-after-staging"
                );
            }
        }
    }

    #[cfg(all(unix, feature = "full-tests"))]
    #[test]
    fn key_publication_final_cancellation_preserves_absence_and_can_retry_normally() {
        use solstone_core_journal_io::{
            BoundPublicationPrimitive, run_with_bound_publication_barrier,
        };
        use std::sync::{Arc, atomic::AtomicBool};
        let journal = TestJournal::new();
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancel_at_rename = cancelled.clone();
        let checkpoint = || {
            if cancelled.load(Ordering::SeqCst) {
                Err("cancelled".to_owned())
            } else {
                Ok(())
            }
        };
        let (result, reached) = run_with_bound_publication_barrier(
            BoundPublicationPrimitive::Rename,
            1,
            move || cancel_at_rename.store(true, Ordering::SeqCst),
            || generate_fingerprint_key_checked(journal.path(), &checkpoint),
        );
        assert!(reached);
        assert!(result.is_err());
        assert!(!brain_fingerprint_key_path(journal.path()).exists());
        let health_entries = fs::read_dir(journal.path().join("health"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        assert_eq!(health_entries, [sidecar_name(key_name())]);
        let key = generate_fingerprint_key(journal.path()).unwrap();
        assert_eq!(
            fs::read(brain_fingerprint_key_path(journal.path())).unwrap(),
            key
        );
    }

    #[cfg(all(unix, feature = "full-tests"))]
    #[test]
    fn post_publication_sync_failure_reports_uncertainty_with_the_actual_record() {
        use solstone_core_journal_io::{
            BoundPublicationPrimitive, run_with_bound_publication_fault,
        };
        let journal = TestJournal::new();
        let permit = begin_cloud(&journal);
        let checking = record_bytes(&journal);
        let (result, reached) = run_with_bound_publication_fault(
            BoundPublicationPrimitive::ParentSync,
            1,
            nix::libc::EIO,
            || finish_refresh(journal.path(), permit, ready_outcome(), fixture_now(), None),
        );
        assert!(reached);
        assert!(matches!(result, Err(WriterError::PublicationUncertain(_))));
        let published = record_bytes(&journal);
        assert_ne!(published, checking);
        let record: Value = serde_json::from_slice(&published).unwrap();
        assert_eq!(record["aggregate_state"], "ready");
        assert!(record["reason_code"].is_null());
        validate_brain_state_record(&record, fixture_now()).unwrap();
    }

    fn assert_finish_refusal_preserves_bytes(
        mutator: impl FnOnce(&mut Value),
        finish_now: DateTime<Utc>,
    ) {
        let journal = TestJournal::new();
        let permit = begin_cloud(&journal);
        let path = brain_state_path(journal.path());
        let mut record: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        mutator(&mut record);
        fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
        let before = fs::read(&path).unwrap();
        assert!(finish_refresh(journal.path(), permit, ready_outcome(), finish_now, None).is_err());
        assert_eq!(fs::read(path).unwrap(), before);
    }

    #[test]
    fn finish_fence_expiry_refuses_without_writing() {
        let journal = TestJournal::new();
        let permit = begin_cloud(&journal);
        let before = record_bytes(&journal);
        assert!(
            finish_refresh(
                journal.path(),
                permit,
                ready_outcome(),
                fixture_now() + checking_ttl(),
                None
            )
            .is_err()
        );
        assert_eq!(record_bytes(&journal), before);
    }

    #[test]
    fn finish_fence_missing_checking_refuses_without_writing() {
        let journal = TestJournal::new();
        let permit = begin_cloud(&journal);
        journal.seed_record("lane_byo_cloud/ready");
        let before = record_bytes(&journal);
        assert!(
            finish_refresh(journal.path(), permit, ready_outcome(), fixture_now(), None,).is_err()
        );
        assert_eq!(record_bytes(&journal), before);
    }

    #[test]
    fn finish_fence_revision_refuses_without_writing() {
        assert_finish_refusal_preserves_bytes(
            |record| record["revision"] = json!(record["revision"].as_u64().unwrap() + 1),
            fixture_now(),
        );
    }

    #[test]
    fn finish_fence_run_id_refuses_without_writing() {
        assert_finish_refusal_preserves_bytes(
            |record| record["checking"]["run_id"] = json!("other-run"),
            fixture_now(),
        );
    }

    #[test]
    fn finish_fence_checking_revision_refuses_without_writing() {
        assert_finish_refusal_preserves_bytes(
            |record| {
                record["checking"]["checking_revision"] =
                    json!(record["checking"]["checking_revision"].as_u64().unwrap() + 1)
            },
            fixture_now(),
        );
    }

    #[test]
    fn finish_fence_marker_seen_refuses_without_writing() {
        assert_finish_refusal_preserves_bytes(
            |record| record["checking"]["runtime_failure_marker_seen"] = json!("new-marker"),
            fixture_now(),
        );
    }

    #[test]
    fn permit_lease_ownership_fence_is_type_enforced() {
        fn consumes_permit(_permit: BrainRefreshPermit) {}
        let journal = TestJournal::new();
        consumes_permit(begin_cloud(&journal));
        // BrainRefreshPermit has no Clone implementation and owns the private,
        // non-Clone FileLease. A caller cannot retain a usable permit after
        // releasing its lease, so Python's leading ownership assertion is
        // enforced by the Rust representation rather than a forgeable flag.
    }

    fn begin_spp_renewal(journal: &TestJournal) -> BrainRefreshPermit {
        journal.write_config("lane_spp");
        journal.write_fixture_key();
        journal.seed_record("lane_spp/ready");
        match begin_prerequisite_renewal(
            journal.path(),
            fixture_now(),
            Some("renew".to_owned()),
            None,
            None,
        ) {
            BeginPrerequisiteRenewal::Started(permit) => permit,
            result => panic!("expected renewal permit, got {result:?}"),
        }
    }

    #[test]
    fn accepted_paths_increment_revision_once() {
        let journal = TestJournal::new();
        let permit = begin_cloud(&journal);
        assert_eq!(permit.checking_revision, 1);
        let finished =
            finish_refresh(journal.path(), permit, ready_outcome(), fixture_now(), None).unwrap();
        assert_eq!(finished["revision"], 2);

        let journal = TestJournal::new();
        let first = begin_cloud(&journal);
        finish_refresh(journal.path(), first, ready_outcome(), fixture_now(), None).unwrap();
        let permit = begin_cloud(&journal);
        let abandoned = abandon_refresh(
            journal.path(),
            permit,
            "provider_unavailable",
            Map::new(),
            fixture_now(),
        )
        .unwrap();
        assert_eq!(abandoned["revision"], 4);

        let journal = TestJournal::new();
        let permit = begin_spp_renewal(&journal);
        assert_eq!(permit.checking_revision, 4);
        let component =
            projection_fixture().records["lane_spp/ready"]["evidence"]["lane_prerequisites"]
                .clone();
        let finished =
            finish_prerequisite_renewal(journal.path(), permit, component, fixture_now(), None)
                .unwrap();
        assert_eq!(finished["revision"], 5);

        let journal = TestJournal::new();
        let permit = begin_spp_renewal(&journal);
        let abandoned = abandon_prerequisite_renewal(
            journal.path(),
            permit,
            "provider_key_missing",
            Map::new(),
            fixture_now(),
            None,
        )
        .unwrap();
        assert_eq!(abandoned["revision"], 5);

        let journal = TestJournal::new();
        journal.write_config("lane_byo_cloud");
        let permit = begin_cloud(&journal);
        let result = record_runtime_failure(
            journal.path(),
            "provider_unavailable",
            "generate",
            &permit.fingerprint_sha256,
            Map::new(),
            fixture_now(),
            None,
        );
        assert!(result.accepted, "{result:?}");
        assert_eq!(result.record.unwrap()["revision"], 2);
        drop(permit);

        let journal = TestJournal::new();
        journal.write_config("lane_none");
        begin_refresh(journal.path(), fixture_now(), None, None, false, None).unwrap();
        let record: Value = serde_json::from_slice(&record_bytes(&journal)).unwrap();
        assert_eq!(record["revision"], 1);
    }

    #[test]
    fn abandon_and_runtime_failure_refusals_preserve_record_bytes() {
        let journal = TestJournal::new();
        let permit = begin_cloud(&journal);
        let before = record_bytes(&journal);
        assert!(
            abandon_refresh(
                journal.path(),
                permit,
                "not-a-reason",
                Map::new(),
                fixture_now(),
            )
            .is_err()
        );
        assert_eq!(record_bytes(&journal), before);

        let journal = TestJournal::new();
        let permit = begin_spp_renewal(&journal);
        let before = record_bytes(&journal);
        assert!(
            finish_prerequisite_renewal(
                journal.path(),
                permit,
                json!({"status": "not_attempted", "observed_at": iso(fixture_now())}),
                fixture_now(),
                None,
            )
            .is_err()
        );
        assert_eq!(record_bytes(&journal), before);

        let journal = TestJournal::new();
        let permit = begin_spp_renewal(&journal);
        let before = record_bytes(&journal);
        assert!(
            abandon_prerequisite_renewal(
                journal.path(),
                permit,
                "provider_unavailable",
                Map::new(),
                fixture_now(),
                None,
            )
            .is_err()
        );
        assert_eq!(record_bytes(&journal), before);

        let journal = TestJournal::new();
        let permit = begin_cloud(&journal);
        let before = record_bytes(&journal);
        let rejected = record_runtime_failure(
            journal.path(),
            "not-a-reason",
            "generate",
            &permit.fingerprint_sha256,
            Map::new(),
            fixture_now(),
            None,
        );
        assert_eq!(
            rejected.rejected_reason.as_deref(),
            Some("reason_not_recordable")
        );
        assert_eq!(record_bytes(&journal), before);
        drop(permit);
    }

    #[test]
    fn abandon_refresh_preserves_caller_diagnostic_at_record_top_level() {
        let journal = TestJournal::new();
        let permit = begin_cloud(&journal);
        let diagnostic = Map::from_iter([
            ("phase".to_owned(), Value::String("failed".to_owned())),
            (
                "runtime_reason".to_owned(),
                Value::String("gpu-unavailable".to_owned()),
            ),
        ]);
        let record = abandon_refresh(
            journal.path(),
            permit,
            "local_server_unhealthy",
            diagnostic.clone(),
            fixture_now(),
        )
        .expect("refresh abandon");
        assert_eq!(record["diagnostic"], Value::Object(diagnostic.clone()));
        let disk: Value = serde_json::from_slice(&record_bytes(&journal)).expect("record JSON");
        assert_eq!(disk["diagnostic"], Value::Object(diagnostic));
    }

    #[test]
    fn finish_preserves_caller_evidence_timestamp_spelling() {
        let journal = TestJournal::new();
        journal.write_config("lane_byo_cloud");
        let now = fixture_now();
        let permit = begin_refresh(
            journal.path(),
            now,
            Some("run".to_owned()),
            None,
            false,
            None,
        )
        .unwrap()
        .expect("refresh permit");
        let mut outcome = projection_fixture()
            .records
            .get("lane_byo_cloud/ready")
            .expect("ready record")
            .get("evidence")
            .expect("evidence")
            .clone();
        let components = outcome.as_object_mut().unwrap();
        for (index, component) in components
            .values_mut()
            .filter_map(Value::as_object_mut)
            .enumerate()
        {
            let observed = component["observed_at"].as_str().unwrap();
            component.insert(
                "observed_at".to_owned(),
                Value::String(if index == 0 {
                    observed.replace("+00:00", "Z")
                } else {
                    observed.replace('Z', "+00:00")
                }),
            );
        }
        finish_refresh(journal.path(), permit, outcome, now, None).unwrap();
        let text = String::from_utf8(fs::read(brain_state_path(journal.path())).unwrap()).unwrap();
        assert!(text.contains("\"observed_at\": \"2026-08-06T11:59:00Z\""));
        assert!(text.contains("\"observed_at\": \"2026-08-06T11:59:00+00:00\""));
        assert!(text.contains("\"updated_at\": \"2026-08-06T12:00:00+00:00\""));
    }

    #[test]
    fn bundled_refresh_converges_on_one_shared_unified_runtime_hash() {
        // backend_reason is no longer hashed, so same-backend flicker cannot
        // Conflict. One matching begin/finish hash is the bound; a real
        // cuda↔vulkan flip still Conflicts as drift protection.
        let hash = crate::fingerprint::bundled_runtime_desired_fingerprint(
            "vulkan",
            "local/qwen3.5-4b",
            &"c".repeat(64),
            Some("/var/tmp/journal/cache/providers/local/bin/x86_64-unknown-linux-gnu/b10068/llama-server"),
            "/var/tmp/journal/cache/providers/local/models/local__qwen3.5-4b/Qwen3.5-4B-Q4_K_M.gguf",
            Some("/var/tmp/journal/cache/providers/local/models/local__qwen3.5-4b/mmproj-F16.gguf"),
        )
        .unwrap()
        .sha256;
        let journal = TestJournal::new();
        journal.write_config("lane_bundled");
        journal.write_fixture_key();
        let now = fixture_now();
        let permit = begin_refresh(
            journal.path(),
            now,
            Some("unified".to_owned()),
            None,
            false,
            Some(hash.clone()),
        )
        .unwrap()
        .expect("permit");
        finish_refresh(
            journal.path(),
            permit,
            ready_outcome(),
            now,
            Some(hash.clone()),
        )
        .expect("matching unified hash must not Conflict");
        fs::create_dir_all(journal.path().join("health/providers/runtime")).unwrap();
        fs::write(
            journal.path().join("health/providers/runtime/local.json"),
            json!({"phase":"ready","desired_fingerprint_sha256":hash}).to_string(),
        )
        .unwrap();
        assert_eq!(
            crate::inspect::assess_bundled_runtime_prerequisite(journal.path(), Some(&hash))
                .reason_code,
            None
        );
    }

    #[test]
    fn fingerprint_key_is_reused_and_mode_is_private() {
        let journal = TestJournal::new();
        let first = generate_fingerprint_key(journal.path()).unwrap();
        let second = generate_fingerprint_key(journal.path()).unwrap();
        assert_eq!(first, second);
        let path = brain_fingerprint_key_path(journal.path());
        assert_eq!(fs::read(path).unwrap(), first);
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(brain_fingerprint_key_path(journal.path()))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    fn write_openai_binding(journal: &TestJournal, bound: bool) {
        let mut config = json!({
            "env": {"OPENAI_API_KEY": "sk-binding"},
            "providers": {"active": {"provider": "openai", "model": "gpt-binding"}}
        });
        if bound {
            config["providers"]["key_validation"] = json!({
                "openai": {
                    "valid": false,
                    "reason_code": "provider_key_invalid",
                    "binding_version": 1,
                    "credential_hmac": crate::managed_credential_binding_hmac(&[0x55; 32], "openai", "sk-binding")
                }
            });
        }
        let path = journal.path().join("config/journal.json");
        fs::create_dir_all(path.parent().expect("config parent")).unwrap();
        fs::write(path, serde_json::to_vec(&config).unwrap()).unwrap();
    }

    fn ordinary_refresh(
        journal: &TestJournal,
    ) -> Result<Option<BrainRefreshPermit>, BeginRefreshError> {
        begin_refresh(
            journal.path(),
            fixture_now(),
            Some("run".to_owned()),
            None,
            false,
            None,
        )
    }

    fn begin_binding_refresh(journal: &TestJournal) -> BrainRefreshPermit {
        write_openai_binding(journal, false);
        let key_path = brain_fingerprint_key_path(journal.path());
        fs::create_dir_all(key_path.parent().unwrap()).unwrap();
        fs::write(key_path, [0x55; 32]).unwrap();
        ordinary_refresh(journal).unwrap().unwrap()
    }

    fn publish_binding_validation(journal: &TestJournal, entry: Value) {
        let root = Arc::new(JournalRoot::open(journal.path()).unwrap());
        let outcome = solstone_core_journal_config_write::mutate_journal_config_at_root_checked(
            &root,
            LockOptions::default(),
            &|| Ok(()),
            |config| {
                config
                    .get_mut("providers")
                    .unwrap()
                    .as_object_mut()
                    .unwrap()
                    .insert("key_validation".to_owned(), json!({"openai":entry}));
                solstone_core_journal_config_write::JournalConfigMutation {
                    changed: true,
                    value: (),
                }
            },
        )
        .unwrap();
        assert!(matches!(
            outcome.outcome,
            solstone_core_journal_config_write::BoundConfigOutcome::Published(_)
        ));
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[cfg(unix)]
    fn positive_native_gap_preserves_newer_rejection(brain: bool) {
        use solstone_core_journal_io::{
            BoundPublicationPrimitive, run_with_two_bound_publication_barriers,
        };
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::{Mutex, mpsc};
        use std::time::Duration;

        struct RetainerThreadGuard(Arc<Mutex<Option<std::thread::JoinHandle<()>>>>);
        impl Drop for RetainerThreadGuard {
            fn drop(&mut self) {
                let join = self
                    .0
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take();
                if let Some(join) = join {
                    // The guard surrounds the publication call: its C/B
                    // guards unwind first, then this owned retainer completes
                    // within its existing budget, before fixture deletion.
                    let result = join.join();
                    if !std::thread::panicking() {
                        result.expect("owned retainer thread panicked");
                    }
                }
            }
        }

        let journal = TestJournal::new();
        drop(begin_binding_refresh(&journal));
        let root = Arc::new(JournalRoot::open(journal.path()).unwrap());
        let target = CredentialHoldTarget::Managed {
            provider: "openai",
            credential: "sk-binding",
        };
        let positive =
            capture_credential_validation_bound(root.clone(), journal.path(), target).unwrap();
        let negative =
            capture_credential_validation_bound(root.clone(), journal.path(), target).unwrap();
        let session = CredentialHoldSession::open_bound(root.clone(), target).unwrap();
        let positive_id = positive.observation_id.clone();
        let path = journal.path().to_owned();
        let observed_path = path.clone();
        let (registered_tx, registered_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let join = Arc::new(Mutex::new(None));
        let retainer = RetainerThreadGuard(join.clone());
        let join_for_barrier = join.clone();
        let published = Arc::new(AtomicBool::new(false));
        let published_at_native = published.clone();
        let (result, fired) = run_with_two_bound_publication_barriers(
            BoundPublicationPrimitive::AuthorizedRename,
            1,
            move || {
                let child = std::thread::spawn(move || {
                    let sent = AtomicBool::new(false);
                    let checkpoint = || {
                        if session.is_held().map_err(|error| error.to_string())?
                            && !sent.swap(true, Ordering::SeqCst)
                        {
                            registered_tx.send(()).map_err(|error| error.to_string())?;
                        }
                        Ok(())
                    };
                    let retained = retain_current_credential_rejection_bound(
                        &path,
                        target,
                        &negative,
                        &checkpoint,
                        &|config| {
                            config
                                .get("env")
                                .and_then(|env| env.get("OPENAI_API_KEY"))
                                .and_then(Value::as_str)
                                == Some("sk-binding")
                        },
                        None,
                    );
                    let result = retained.and_then(|receipt| {
                        let receipt = receipt.ok_or_else(|| {
                            CredentialRejectionFailure::Refused(
                                "unexpected target change".to_owned(),
                            )
                        })?;
                        receipt.checkpoint(&negative).map_err(|error| {
                            CredentialRejectionFailure::Refused(error.to_string())
                        })?;
                        Ok((receipt.carrier(), receipt.hold.clone()))
                    });
                    done_tx.send(result).unwrap();
                });
                *join_for_barrier.lock().unwrap() = Some(child);
                registered_rx
                    .recv_timeout(Duration::from_secs(5))
                    .expect("actual retainer must prepare P before its config-lock wait");
            },
            BoundPublicationPrimitive::ParentSync,
            1,
            move || {
                let file = if brain {
                    "health/brain.json"
                } else {
                    "config/journal.json"
                };
                let value: Value =
                    serde_json::from_slice(&fs::read(observed_path.join(file)).unwrap()).unwrap();
                let actual = if brain {
                    value["credential_observations"]["openai"]["observation_id"].as_str()
                } else {
                    value["providers"]["key_validation"]["openai"]["credential_observation_id"]
                        .as_str()
                };
                assert_eq!(
                    actual,
                    Some(positive_id.as_str()),
                    "the older positive really crossed the native rename"
                );
                published_at_native.store(true, Ordering::SeqCst);
            },
            || -> Result<(), String> {
                if brain {
                    positive
                        .retire_after_validation(&[0x55; 32], &|| Ok(()))
                        .map(|_| ())
                        .map_err(|error| error.to_string())
                } else {
                    let mut entry = json!({"valid":true,"binding_version":1,
                        "credential_hmac":crate::managed_credential_binding_hmac(&[0x55;32],"openai","sk-binding")});
                    positive.prepare_successful_validation(&mut entry).unwrap();
                    let transaction = solstone_core_journal_config_write::mutate_journal_config_at_root_interlocked(
                        &root, LockOptions::default(), &|| positive.checkpoint_with_binding_key(&[0x55;32]).map_err(|error| error.to_string()),
                        &|| positive.begin_positive_config_publication(&[0x55;32]).map_err(|error| error.to_string()),
                        |config| {
                            config.get_mut("providers").unwrap().as_object_mut().unwrap()
                                .insert("key_validation".to_owned(), json!({"openai":entry}));
                            solstone_core_journal_config_write::JournalConfigMutation { changed: true, value: () }
                        },
                    ).map_err(|error| error.to_string())?;
                    // The config receipt confirms native bytes/namespace. The
                    // credential owner's later settlement must reject newer P.
                    assert!(matches!(
                        transaction.outcome,
                        solstone_core_journal_config_write::BoundConfigOutcome::Published(_)
                    ));
                    positive
                        .retire_after_validation(&[0x55; 32], &|| Ok(()))
                        .map(|_| ())
                        .map_err(|error| error.to_string())
                }
            },
        );
        assert_eq!(fired, 2);
        assert!(published.load(Ordering::SeqCst));
        assert!(
            result.is_err(),
            "a known older partial publication cannot settle over newer P"
        );
        let retained = done_rx
            .recv_timeout(Duration::from_secs(15))
            .expect("actual negative owner must finish after C/B release");
        drop(retainer);
        let (carrier, hold) = retained.unwrap();
        assert_eq!(
            hold.credential_hmac,
            crate::managed_credential_binding_hmac(&[0x55; 32], "openai", "sk-binding")
        );
        // R may capture the config SHA before S's native rename and then
        // acknowledge its exact rejection through the existing Brain fallback.
        // Require the actual returned carrier's saved authority in either case.
        let (file, marker_field) = match carrier {
            CredentialRejectionCarrier::Config => ("config/journal.json", "rejection_marker_id"),
            CredentialRejectionCarrier::Brain => ("health/brain.json", "marker_id"),
        };
        let saved: Value =
            serde_json::from_slice(&fs::read(journal.path().join(file)).unwrap()).unwrap();
        let entry = match carrier {
            CredentialRejectionCarrier::Config => {
                let entry = &saved["providers"]["key_validation"]["openai"];
                assert_eq!(entry["valid"], false);
                entry
            }
            CredentialRejectionCarrier::Brain => &saved["credential_holds"]["openai"],
        };
        assert_eq!(entry["binding_version"], 1);
        assert_eq!(entry["reason_code"], "provider_key_invalid");
        assert_eq!(entry["credential_hmac"], hold.credential_hmac);
        assert_eq!(entry[marker_field], hold.marker_id);
        // The FnOnce publication callback consumed positive (including its
        // publication guard), so admission runs after that guard has dropped.
        assert_eq!(
            classify_credential_admission_bound(root, journal.path(), target, None).unwrap(),
            StoredCredentialBinding::Matches
        );
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[cfg(unix)]
    #[test]
    fn config_native_authorized_rename_keeps_newer_learned_rejection() {
        positive_native_gap_preserves_newer_rejection(false);
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[cfg(unix)]
    #[test]
    fn brain_native_authorized_rename_keeps_newer_rejection_and_exact_receipt() {
        positive_native_gap_preserves_newer_rejection(true);
    }

    #[test]
    fn refresh_terminal_refuses_a_new_config_negative_without_rewriting_brain() {
        let journal = TestJournal::new();
        let permit = begin_binding_refresh(&journal);
        let before = record_bytes(&journal);
        publish_binding_validation(
            &journal,
            json!({
                "valid":false,"reason_code":"provider_key_invalid","binding_version":1,
                "credential_hmac":crate::managed_credential_binding_hmac(&[0x55;32],"openai","sk-binding"),
                "rejection_marker_id":"a".repeat(64),
            }),
        );
        let error = finish_refresh(journal.path(), permit, ready_outcome(), fixture_now(), None)
            .unwrap_err();
        assert!(
            error.to_string().contains("credential observation changed"),
            "{error}"
        );
        assert_eq!(record_bytes(&journal), before);
    }

    #[test]
    fn refresh_terminal_refuses_new_same_key_config_success_identity() {
        let journal = TestJournal::new();
        let permit = begin_binding_refresh(&journal);
        let before = record_bytes(&journal);
        publish_binding_validation(
            &journal,
            json!({
                "valid":true,"binding_version":1,
                "credential_hmac":crate::managed_credential_binding_hmac(&[0x55;32],"openai","sk-binding"),
                "credential_observation_id":"b".repeat(64),
            }),
        );
        let error = finish_refresh(journal.path(), permit, ready_outcome(), fixture_now(), None)
            .unwrap_err();
        assert!(
            error.to_string().contains("credential observation changed"),
            "{error}"
        );
        assert_eq!(record_bytes(&journal), before);
    }

    #[test]
    fn refresh_terminal_refuses_a_new_brain_observation_without_config_writes() {
        let journal = TestJournal::new();
        let permit = begin_binding_refresh(&journal);
        let config_before = fs::read(journal.path().join("config/journal.json")).unwrap();
        let root = Arc::new(JournalRoot::open(journal.path()).unwrap());
        let witness = capture_credential_validation_bound(
            root,
            journal.path(),
            CredentialHoldTarget::Managed {
                provider: "openai",
                credential: "sk-binding",
            },
        )
        .unwrap();
        witness
            .retire_after_validation(&[0x55; 32], &|| Ok(()))
            .unwrap();
        let before = record_bytes(&journal);
        let error = finish_refresh(journal.path(), permit, ready_outcome(), fixture_now(), None)
            .unwrap_err();
        assert!(
            error.to_string().contains("credential observation changed"),
            "{error}"
        );
        assert_eq!(record_bytes(&journal), before);
        assert_eq!(
            fs::read(journal.path().join("config/journal.json")).unwrap(),
            config_before
        );
    }

    #[test]
    fn first_rejection_publishes_its_binding_with_initially_absent_key() {
        let journal = TestJournal::new();
        fs::create_dir(journal.path().join("config")).unwrap();
        fs::write(
            journal.path().join("config/journal.json"),
            br#"{"env":{"OPENAI_API_KEY":"sk-binding"}}"#,
        )
        .unwrap();
        let root = Arc::new(JournalRoot::open(journal.path()).unwrap());
        let target = CredentialHoldTarget::Managed {
            provider: "openai",
            credential: "sk-binding",
        };
        let witness = capture_credential_validation_bound(root, journal.path(), target).unwrap();
        let result = retain_current_credential_rejection_bound(
            journal.path(),
            target,
            &witness,
            &|| Ok(()),
            &|config| {
                config
                    .get("env")
                    .and_then(|env| env.get("OPENAI_API_KEY"))
                    .and_then(Value::as_str)
                    == Some("sk-binding")
            },
            None,
        )
        .map(|receipt| receipt.map(|receipt| receipt.carrier()));
        assert_eq!(result.unwrap(), Some(CredentialRejectionCarrier::Config));
        let saved: Value =
            serde_json::from_slice(&fs::read(journal.path().join("config/journal.json")).unwrap())
                .unwrap();
        assert_eq!(
            saved["providers"]["key_validation"]["openai"]["valid"],
            false
        );
        assert_eq!(
            saved["providers"]["key_validation"]["openai"]["reason_code"],
            "provider_key_invalid"
        );
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[cfg(unix)]
    fn rejection_owner_projection_journal_image(
        journal: &Path,
    ) -> BTreeMap<PathBuf, (u32, Option<Vec<u8>>)> {
        fn visit(root: &Path, path: &Path, image: &mut BTreeMap<PathBuf, (u32, Option<Vec<u8>>)>) {
            let metadata = fs::symlink_metadata(path).unwrap();
            assert!(!metadata.is_symlink());
            image.insert(
                path.strip_prefix(root).unwrap().to_path_buf(),
                (
                    metadata.permissions().mode(),
                    metadata.is_file().then(|| fs::read(path).unwrap()),
                ),
            );
            if metadata.is_dir() {
                for entry in fs::read_dir(path).unwrap() {
                    visit(root, &entry.unwrap().path(), image);
                }
            }
        }
        let mut image = BTreeMap::new();
        visit(journal, journal, &mut image);
        image
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[cfg(unix)]
    fn rejection_owner_projection_without_config_publication(all_writes_fail: bool) {
        let journal = TestJournal::new();
        let permit = begin_binding_refresh(&journal);
        finish_refresh(journal.path(), permit, ready_outcome(), fixture_now(), None).unwrap();
        let root = Arc::new(JournalRoot::open(journal.path()).unwrap());
        let current =
            config(&BrainWriteRoot::open_bound(root.clone(), journal.path()).unwrap()).unwrap();
        assert_eq!(
            inspect_brain_state(journal.path(), &current, fixture_now())
                .projection
                .aggregate_state,
            "ready",
            "known ready baseline"
        );
        let target = CredentialHoldTarget::Managed {
            provider: "openai",
            credential: "sk-binding",
        };
        let witness =
            capture_credential_validation_bound(root.clone(), journal.path(), target).unwrap();
        let session = CredentialHoldSession::open_bound(root, target).unwrap();
        let config_before = fs::read(journal.path().join("config/journal.json")).unwrap();
        let brain_before = record_bytes(&journal);
        let config_mode = fs::metadata(journal.path().join("config"))
            .unwrap()
            .permissions();
        let health_mode = fs::metadata(journal.path().join("health"))
            .unwrap()
            .permissions();
        fs::set_permissions(
            journal.path().join("config"),
            fs::Permissions::from_mode(0o555),
        )
        .unwrap();
        if all_writes_fail {
            fs::set_permissions(
                journal.path().join("health"),
                fs::Permissions::from_mode(0o555),
            )
            .unwrap();
        }
        let result = retain_current_credential_rejection_bound(
            journal.path(),
            target,
            &witness,
            &|| Ok(()),
            &|config| {
                config
                    .get("env")
                    .and_then(|env| env.get("OPENAI_API_KEY"))
                    .and_then(Value::as_str)
                    == Some("sk-binding")
            },
            None,
        )
        .map(|receipt| receipt.map(|receipt| receipt.carrier()));
        fs::set_permissions(journal.path().join("config"), config_mode).unwrap();
        fs::set_permissions(journal.path().join("health"), health_mode).unwrap();
        if all_writes_fail {
            assert_eq!(result, Err(CredentialRejectionFailure::Unpublished));
            assert_eq!(record_bytes(&journal), brain_before);
        } else {
            assert_eq!(result, Ok(Some(CredentialRejectionCarrier::Brain)));
        }
        assert_eq!(
            fs::read(journal.path().join("config/journal.json")).unwrap(),
            config_before
        );
        assert!(session.is_held().unwrap());
        let journal_before = rejection_owner_projection_journal_image(journal.path());
        let memory_before = session.memory_snapshot().unwrap();
        let projection = inspect_brain_state(journal.path(), &current, fixture_now()).projection;
        assert_eq!(
            rejection_owner_projection_journal_image(journal.path()),
            journal_before,
            "inspection must preserve exact files, modes and directory population"
        );
        assert!(memory_before == session.memory_snapshot().unwrap());
        assert_eq!(
            (
                projection.aggregate_state.as_str(),
                projection.reason_code.as_deref()
            ),
            ("unhealthy", Some("provider_key_invalid")),
            "all_writes_fail={all_writes_fail}: current owner state must reflect the retained rejection"
        );
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[cfg(unix)]
    #[test]
    fn brain_only_rejection_owner_projection_does_not_reuse_old_ready_evidence() {
        rejection_owner_projection_without_config_publication(false);
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[cfg(unix)]
    #[test]
    fn memory_only_rejection_owner_projection_does_not_reuse_old_ready_evidence() {
        rejection_owner_projection_without_config_publication(true);
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[cfg(unix)]
    fn rejection_owner_projection_does_not_adopt_a_replacement_directory_after_read(health: bool) {
        let journal = TestJournal::new();
        let permit = begin_binding_refresh(&journal);
        finish_refresh(journal.path(), permit, ready_outcome(), fixture_now(), None).unwrap();
        let root = Arc::new(JournalRoot::open(journal.path()).unwrap());
        let current = config(&BrainWriteRoot::open_bound(root, journal.path()).unwrap()).unwrap();
        assert_eq!(
            inspect_brain_state(journal.path(), &current, fixture_now())
                .projection
                .aggregate_state,
            "ready"
        );
        let directory = if health {
            journal.path().join("health")
        } else {
            journal.path().to_path_buf()
        };
        let original = directory.with_extension("original-inspection");
        let key = fs::read(brain_fingerprint_key_path(journal.path())).unwrap();
        let projection = crate::inspect_brain_state_with_clock(journal.path(), &current, || {
            fs::rename(&directory, &original).unwrap();
            if !health {
                fs::create_dir(journal.path()).unwrap();
            }
            fs::create_dir(journal.path().join("health")).unwrap();
            fs::write(brain_fingerprint_key_path(journal.path()), &key).unwrap();
            fixture_now()
        })
        .projection;
        let replacement_key = fs::read(brain_fingerprint_key_path(journal.path())).unwrap();
        let replacement_entries = fs::read_dir(journal.path().join("health"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        fs::remove_dir_all(&directory).unwrap();
        fs::rename(&original, &directory).unwrap();
        assert_eq!(replacement_key, key);
        assert_eq!(replacement_entries, vec![key_name().to_os_string()]);
        assert_eq!(
            (
                projection.aggregate_state.as_str(),
                projection.reason_code.as_deref()
            ),
            ("unknown", Some("brain_record_unavailable")),
            "a replacement directory cannot inherit the original directory's Ready record"
        );
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[cfg(unix)]
    #[test]
    fn rejection_owner_projection_does_not_adopt_a_replacement_root_after_read() {
        rejection_owner_projection_does_not_adopt_a_replacement_directory_after_read(false);
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[cfg(unix)]
    #[test]
    fn rejection_owner_projection_does_not_adopt_a_replacement_health_directory_after_read() {
        rejection_owner_projection_does_not_adopt_a_replacement_directory_after_read(true);
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[cfg(unix)]
    #[test]
    fn durable_rejection_owner_projection_needs_no_process_cell_and_is_read_only() {
        let source = TestJournal::new();
        let permit = begin_binding_refresh(&source);
        finish_refresh(source.path(), permit, ready_outcome(), fixture_now(), None).unwrap();
        let journal = TestJournal::new();
        for name in ["config", "health"] {
            fs::create_dir(journal.path().join(name)).unwrap();
        }
        for name in [
            "config/journal.json",
            "health/brain.json",
            "health/brain-fingerprint.key",
        ] {
            fs::copy(source.path().join(name), journal.path().join(name)).unwrap();
        }
        let root = Arc::new(JournalRoot::open(journal.path()).unwrap());
        let target = CredentialHoldTarget::Managed {
            provider: "openai",
            credential: "sk-binding",
        };
        let current =
            config(&BrainWriteRoot::open_bound(root.clone(), journal.path()).unwrap()).unwrap();
        assert!(
            CredentialHoldSession::existing_bound(root.clone(), target)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            inspect_brain_state(journal.path(), &current, fixture_now())
                .projection
                .aggregate_state,
            "ready"
        );
        let key = load_existing_fingerprint_key(journal.path()).unwrap();
        record_credential_hold_bound(
            root.clone(),
            journal.path(),
            "openai",
            &CredentialRejectionHold {
                credential_hmac: target.binding_hmac(&key).unwrap(),
                marker_id: "c".repeat(64),
            },
            &key,
            &|| Ok(()),
        )
        .unwrap();
        let before = rejection_owner_projection_journal_image(journal.path());
        let projection = inspect_brain_state(journal.path(), &current, fixture_now()).projection;
        assert_eq!(
            projection.reason_code.as_deref(),
            Some("provider_key_invalid")
        );
        assert_eq!(
            rejection_owner_projection_journal_image(journal.path()),
            before
        );
        assert!(
            CredentialHoldSession::existing_bound(root, target)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn temporary_unavailable_config_after_rejection_keeps_the_process_fact() {
        let journal = TestJournal::new();
        drop(begin_binding_refresh(&journal));
        let root = Arc::new(JournalRoot::open(journal.path()).unwrap());
        let target = CredentialHoldTarget::Managed {
            provider: "openai",
            credential: "sk-binding",
        };
        let witness =
            capture_credential_validation_bound(root.clone(), journal.path(), target).unwrap();
        let session = CredentialHoldSession::open_bound(root.clone(), target).unwrap();
        let config_path = journal.path().join("config/journal.json");
        let retained = config_path.with_extension("held-original");
        let original = fs::read(&config_path).unwrap();
        let key_before = fs::read(brain_fingerprint_key_path(journal.path())).unwrap();
        let brain_before = record_bytes(&journal);
        fs::rename(&config_path, &retained).unwrap();
        fs::create_dir(&config_path).unwrap();
        let result = retain_current_credential_rejection_bound(
            journal.path(),
            target,
            &witness,
            &|| Ok(()),
            &|config| {
                config
                    .get("env")
                    .and_then(|env| env.get("OPENAI_API_KEY"))
                    .and_then(Value::as_str)
                    == Some("sk-binding")
            },
            None,
        );
        let held_while_unreadable = session.is_held().unwrap();
        fs::remove_dir(&config_path).unwrap();
        fs::rename(&retained, &config_path).unwrap();
        drop(witness);
        assert!(matches!(
            result,
            Err(CredentialRejectionFailure::Unpublished)
        ));
        assert_eq!(fs::read(&config_path).unwrap(), original);
        assert_eq!(
            fs::read(brain_fingerprint_key_path(journal.path())).unwrap(),
            key_before
        );
        assert_eq!(record_bytes(&journal), brain_before);
        assert_eq!(
            (
                held_while_unreadable,
                session.is_held().unwrap(),
                classify_credential_admission_bound(root, journal.path(), target, None).unwrap(),
            ),
            (true, true, StoredCredentialBinding::Matches),
            "restoring original config readability must not forget the rejected actual target"
        );
    }

    #[test]
    fn temporary_unavailable_key_after_rejection_keeps_the_process_fact() {
        let journal = TestJournal::new();
        drop(begin_binding_refresh(&journal));
        let root = Arc::new(JournalRoot::open(journal.path()).unwrap());
        let target = CredentialHoldTarget::Managed {
            provider: "openai",
            credential: "sk-binding",
        };
        let witness =
            capture_credential_validation_bound(root.clone(), journal.path(), target).unwrap();
        let key = brain_fingerprint_key_path(journal.path());
        let retained = key.with_extension("held-original");
        let original = fs::read(&key).unwrap();
        fs::rename(&key, &retained).unwrap();
        fs::create_dir(&key).unwrap();
        let result = retain_current_credential_rejection_bound(
            journal.path(),
            target,
            &witness,
            &|| Ok(()),
            &|config| {
                config
                    .get("env")
                    .and_then(|env| env.get("OPENAI_API_KEY"))
                    .and_then(Value::as_str)
                    == Some("sk-binding")
            },
            None,
        );
        assert!(matches!(
            result,
            Err(CredentialRejectionFailure::Refused(_))
        ));
        fs::remove_dir(&key).unwrap();
        fs::rename(&retained, &key).unwrap();
        assert_eq!(fs::read(&key).unwrap(), original);
        assert!(
            CredentialHoldSession::open_bound(root.clone(), target)
                .unwrap()
                .is_held()
                .unwrap(),
            "restoring original key readability must not erase the learned rejection"
        );
        assert_eq!(
            classify_credential_admission_bound(root, journal.path(), target, None).unwrap(),
            StoredCredentialBinding::Matches
        );
    }

    #[test]
    fn positive_config_authority_survives_transient_metadata_but_legacy_has_no_work_id() {
        let positive = json!({"valid":true,"binding_version":1,"credential_hmac":"c".repeat(64),
            "credential_observation_id":"d".repeat(64),"timestamp":"first"});
        let mut transient = positive.clone();
        transient["valid"] = json!(false);
        transient["reason_code"] = json!("validation_unavailable");
        transient["timestamp"] = json!("later");
        assert_eq!(
            config_authority_projection(Some(&positive)),
            config_authority_projection(Some(&transient))
        );
        transient["reason_code"] = json!("provider_key_invalid");
        assert_ne!(
            config_authority_projection(Some(&positive)),
            config_authority_projection(Some(&transient))
        );
        let mut legacy = positive;
        legacy
            .as_object_mut()
            .unwrap()
            .remove("credential_observation_id");
        assert_eq!(config_authority_projection(Some(&legacy)), None);
    }

    #[test]
    fn brain_observation_authority_never_recreates_a_missing_protected_key() {
        for corrupt in [false, true] {
            let journal = TestJournal::new();
            let permit = begin_binding_refresh(&journal);
            let root = Arc::new(JournalRoot::open(journal.path()).unwrap());
            let target = CredentialHoldTarget::Managed {
                provider: "openai",
                credential: "sk-binding",
            };
            let witness =
                capture_credential_validation_bound(root.clone(), journal.path(), target).unwrap();
            witness
                .retire_after_validation(&[0x55; 32], &|| Ok(()))
                .unwrap();
            drop(permit);
            if corrupt {
                let mut record: Value = serde_json::from_slice(&record_bytes(&journal)).unwrap();
                record["credential_observations"]["openai"]["observation_id"] = json!("invalid");
                journal.seed_value(&record);
            }
            fs::remove_file(brain_fingerprint_key_path(journal.path())).unwrap();
            let before = record_bytes(&journal);
            assert!(generate_fingerprint_key(journal.path()).is_err());
            assert!(!brain_fingerprint_key_path(journal.path()).exists());
            assert!(
                classify_credential_admission_bound(root.clone(), journal.path(), target, None)
                    .is_err()
            );
            assert!(capture_credential_validation_bound(root, journal.path(), target).is_err());
            assert_eq!(record_bytes(&journal), before);
        }
    }

    #[test]
    fn credential_publication_budget_is_not_rearmed_by_later_owner_phases() {
        let journal = TestJournal::new();
        let permit = begin_binding_refresh(&journal);
        let before = record_bytes(&journal);
        let root = Arc::new(JournalRoot::open(journal.path()).unwrap());
        let witness = capture_credential_validation_bound(
            root,
            journal.path(),
            CredentialHoldTarget::Managed {
                provider: "openai",
                credential: "sk-binding",
            },
        )
        .unwrap();
        witness.publication_deadline.set(Some(
            std::time::Instant::now() - std::time::Duration::from_secs(1),
        ));
        witness.begin_publication_budget();
        let mut permit = permit;
        permit.share_credential_publication_budget(&witness);
        assert!(
            permit
                .checkpoint(journal.path())
                .unwrap_err()
                .contains("budget exhausted")
        );
        assert!(
            witness
                .retire_after_validation(&[0x55; 32], &|| Ok(()))
                .unwrap_err()
                .to_string()
                .contains("budget exhausted")
        );
        assert_eq!(record_bytes(&journal), before);
    }

    #[test]
    fn ordinary_refresh_keeps_unavailable_and_bound_absent_fingerprint_material() {
        let missing = TestJournal::new();
        write_openai_binding(&missing, true);
        let key_path = brain_fingerprint_key_path(missing.path());
        let error = ordinary_refresh(&missing).expect_err("bound absent key");
        assert!(matches!(
            error,
            BeginRefreshError::Writer(WriterError::Fingerprint(_))
        ));
        assert!(!key_path.exists());

        let short = TestJournal::new();
        write_openai_binding(&short, true);
        let short_path = brain_fingerprint_key_path(short.path());
        fs::create_dir_all(short_path.parent().expect("health")).unwrap();
        let original = b"not-32-bytes".to_vec();
        fs::write(&short_path, &original).unwrap();
        let error = ordinary_refresh(&short).expect_err("unavailable key");
        assert!(matches!(
            error,
            BeginRefreshError::Writer(WriterError::Fingerprint(_))
        ));
        assert_eq!(fs::read(&short_path).unwrap(), original);

        let fresh = TestJournal::new();
        fresh.write_config("lane_byo_cloud");
        let fresh_path = brain_fingerprint_key_path(fresh.path());
        assert!(!fresh_path.exists());
        ordinary_refresh(&fresh)
            .expect("unbound journal")
            .expect("first key");
        assert_eq!(
            fs::read(&fresh_path).unwrap().len(),
            crate::inspect::FINGERPRINT_KEY_BYTES
        );
    }

    #[test]
    fn genuine_credential_rejection_survives_protected_key_loss_and_refresh() {
        let journal = TestJournal::new();
        write_openai_binding(&journal, true);
        let key_path = brain_fingerprint_key_path(journal.path());
        fs::create_dir_all(key_path.parent().unwrap()).unwrap();
        fs::write(&key_path, [0x55; 32]).unwrap();
        let original_config = fs::read(journal.path().join("config/journal.json")).unwrap();
        let config: Map<String, Value> = serde_json::from_slice(&original_config).unwrap();
        assert_eq!(
            crate::inspect_brain_state(journal.path(), &config, fixture_now())
                .projection
                .reason_code
                .as_deref(),
            Some("provider_key_invalid")
        );
        fs::remove_file(&key_path).unwrap();
        assert!(ordinary_refresh(&journal).is_err());
        assert!(!key_path.exists());
        assert!(!brain_state_path(journal.path()).exists());
        assert_eq!(
            fs::read(journal.path().join("config/journal.json")).unwrap(),
            original_config
        );
        assert_eq!(
            crate::inspect_brain_state(journal.path(), &config, fixture_now())
                .projection
                .reason_code
                .as_deref(),
            Some("provider_key_invalid")
        );
    }

    fn seed_credential_hold(journal: &TestJournal) -> Value {
        let key = load_existing_fingerprint_key(journal.path()).unwrap();
        let holds = json!({ "openai": {
            "binding_version": 1,
            "credential_hmac": crate::managed_credential_binding_hmac(&key, "openai", "sk-binding"),
            "marker_id": "b".repeat(64),
            "reason_code": "provider_key_invalid",
        }});
        let mut record: Value = serde_json::from_slice(&record_bytes(journal)).unwrap();
        record["credential_holds"] = holds.clone();
        journal.seed_value(&record);
        holds
    }

    #[test]
    fn durable_brain_hold_prevents_rekey_when_config_has_no_rejection_binding() {
        let journal = TestJournal::new();
        write_openai_binding(&journal, false);
        drop(ordinary_refresh(&journal).unwrap().unwrap());
        seed_credential_hold(&journal);
        let before = record_bytes(&journal);
        let config_path = journal.path().join("config/journal.json");
        let config_before = fs::read(&config_path).unwrap();
        fs::remove_file(brain_fingerprint_key_path(journal.path())).unwrap();
        assert!(ordinary_refresh(&journal).is_err());
        assert!(!brain_fingerprint_key_path(journal.path()).exists());
        assert_eq!(record_bytes(&journal), before);
        assert_eq!(fs::read(config_path).unwrap(), config_before);
    }

    #[test]
    fn brain_hold_publisher_binds_actual_key_and_new_reader_without_changing_config() {
        let journal = TestJournal::new();
        write_openai_binding(&journal, false);
        let key = generate_fingerprint_key(journal.path()).unwrap();
        let config_path = journal.path().join("config/journal.json");
        let before = fs::read(&config_path).unwrap();
        let hold = CredentialRejectionHold {
            credential_hmac: crate::managed_credential_binding_hmac(&key, "openai", "sk-binding"),
            marker_id: "c".repeat(64),
        };
        record_credential_hold_bound(
            Arc::new(JournalRoot::open(journal.path()).unwrap()),
            journal.path(),
            "openai",
            &hold,
            &key,
            &|| Ok(()),
        )
        .unwrap();
        assert_eq!(fs::read(&config_path).unwrap(), before);
        let record: Value = serde_json::from_slice(&record_bytes(&journal)).unwrap();
        assert!(validate_brain_state_record(&record, fixture_now()).is_err());
        assert_eq!(record.as_object().unwrap().len(), 1);
        crate::record::parse_credential_holds(record.get("credential_holds")).unwrap();
        drop(ordinary_refresh(&journal).unwrap().unwrap());
        let refreshed: Value = serde_json::from_slice(&record_bytes(&journal)).unwrap();
        validate_brain_state_record(&refreshed, fixture_now()).unwrap();
        assert_eq!(refreshed["credential_holds"], record["credential_holds"]);
        for model in ["first-model", "changed-model"] {
            let mut config: Value =
                serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
            config["providers"]["active"]["model"] = json!(model);
            fs::write(&config_path, serde_json::to_vec(&config).unwrap()).unwrap();
            for (credential, expected) in [
                ("sk-binding", StoredCredentialBinding::Matches),
                ("corrected-key", StoredCredentialBinding::DoesNotReject),
            ] {
                assert_eq!(
                    classify_credential_admission_bound(
                        Arc::new(JournalRoot::open(journal.path()).unwrap()),
                        journal.path(),
                        CredentialHoldTarget::Managed {
                            provider: "openai",
                            credential
                        },
                        None,
                    )
                    .unwrap(),
                    expected
                );
            }
        }
    }

    #[test]
    fn bound_admission_consults_surviving_hold_without_creating_durable_state() {
        let journal = TestJournal::new();
        let root = Arc::new(JournalRoot::open(journal.path()).unwrap());
        let target = CredentialHoldTarget::Managed {
            provider: "openai",
            credential: "unacknowledged-key",
        };
        let session = CredentialHoldSession::open_bound(root, target).unwrap();
        session.record_rejection().unwrap();
        drop(session);
        assert_eq!(
            classify_credential_admission_bound(
                Arc::new(JournalRoot::open(journal.path()).unwrap()),
                journal.path(),
                target,
                None,
            )
            .unwrap(),
            StoredCredentialBinding::Matches
        );
        assert_eq!(
            classify_credential_admission_bound(
                Arc::new(JournalRoot::open(journal.path()).unwrap()),
                journal.path(),
                CredentialHoldTarget::Managed {
                    provider: "openai",
                    credential: "corrected-key"
                },
                None,
            )
            .unwrap(),
            StoredCredentialBinding::Unbound
        );
        assert!(fs::read_dir(journal.path()).unwrap().next().is_none());
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn a_rejection_receipt_cannot_authorize_successful_refresh_evidence() {
        let journal = TestJournal::new();
        write_openai_binding(&journal, false);
        let permit = ordinary_refresh(&journal).unwrap().unwrap();
        let target = CredentialHoldTarget::Managed {
            provider: "openai",
            credential: "sk-binding",
        };
        let witness =
            capture_credential_validation_bound(permit.admitted_root(), journal.path(), target)
                .unwrap();
        let receipt = retain_current_credential_rejection_bound(
            journal.path(),
            target,
            &witness,
            &|| Ok(()),
            &|_| true,
            None,
        )
        .unwrap()
        .unwrap();
        let before = record_bytes(&journal);
        assert!(
            finish_refresh_rejected(
                journal.path(),
                permit,
                ready_outcome(),
                fixture_now(),
                None,
                receipt
            )
            .is_err()
        );
        assert_eq!(record_bytes(&journal), before);
        assert_eq!(
            classify_credential_admission_bound(
                witness.admitted_root(),
                journal.path(),
                target,
                None
            )
            .unwrap(),
            StoredCredentialBinding::Matches
        );
    }

    #[test]
    fn brain_hold_publication_preserves_invalid_derived_evidence_for_normal_refresh() {
        let journal = TestJournal::new();
        write_openai_binding(&journal, false);
        drop(ordinary_refresh(&journal).unwrap().unwrap());
        let key = load_existing_fingerprint_key(journal.path()).unwrap();
        let mut raw: Value = serde_json::from_slice(&record_bytes(&journal)).unwrap();
        raw["evidence"]["retired-component"] = json!("invalid");
        raw["credential_holds"] = json!({"anthropic": {
            "binding_version": 1, "reason_code": "provider_key_invalid",
            "credential_hmac": crate::managed_credential_binding_hmac(&key, "anthropic", "other-key"),
            "marker_id": "d".repeat(64),
        }});
        let other = raw["credential_holds"]["anthropic"].clone();
        journal.seed_value(&raw);
        let hold = CredentialRejectionHold {
            credential_hmac: crate::managed_credential_binding_hmac(&key, "openai", "sk-binding"),
            marker_id: "e".repeat(64),
        };
        record_credential_hold_bound(
            Arc::new(JournalRoot::open(journal.path()).unwrap()),
            journal.path(),
            "openai",
            &hold,
            &key,
            &|| Ok(()),
        )
        .unwrap();
        let held: Value = serde_json::from_slice(&record_bytes(&journal)).unwrap();
        assert!(validate_brain_state_record(&held, fixture_now()).is_err());
        assert_eq!(held["evidence"], raw["evidence"]);
        assert_eq!(held["credential_holds"]["anthropic"], other);
        assert_eq!(
            held["credential_holds"]["openai"]["credential_hmac"],
            hold.credential_hmac
        );
        drop(ordinary_refresh(&journal).unwrap().unwrap());
        let repaired: Value = serde_json::from_slice(&record_bytes(&journal)).unwrap();
        validate_brain_state_record(&repaired, fixture_now()).unwrap();
        assert_eq!(repaired["credential_holds"], held["credential_holds"]);
    }

    #[cfg(all(unix, feature = "full-tests"))]
    #[test]
    fn brain_hold_final_key_change_refuses_before_creating_the_record() {
        use solstone_core_journal_io::{
            BoundPublicationPrimitive, run_with_bound_publication_barrier,
        };
        let journal = TestJournal::new();
        write_openai_binding(&journal, false);
        let key = generate_fingerprint_key(journal.path()).unwrap();
        let hold = CredentialRejectionHold {
            credential_hmac: crate::managed_credential_binding_hmac(&key, "openai", "sk-binding"),
            marker_id: "f".repeat(64),
        };
        let key_path = brain_fingerprint_key_path(journal.path());
        let (result, reached) = run_with_bound_publication_barrier(
            BoundPublicationPrimitive::Rename,
            1,
            move || fs::write(key_path, [0x88; FINGERPRINT_KEY_BYTES]).unwrap(),
            || {
                record_credential_hold_bound(
                    Arc::new(JournalRoot::open(journal.path()).unwrap()),
                    journal.path(),
                    "openai",
                    &hold,
                    &key,
                    &|| Ok(()),
                )
            },
        );
        assert!(reached);
        assert!(result.is_err());
        assert!(!brain_state_path(journal.path()).exists());
        assert_eq!(
            fs::read(brain_fingerprint_key_path(journal.path())).unwrap(),
            [0x88; FINGERPRINT_KEY_BYTES]
        );
    }

    #[test]
    fn invalid_derived_evidence_refresh_and_none_lane_preserve_valid_hold_authority() {
        let journal = TestJournal::new();
        write_openai_binding(&journal, false);
        drop(ordinary_refresh(&journal).unwrap().unwrap());
        let holds = seed_credential_hold(&journal);
        let mut invalid: Value = serde_json::from_slice(&record_bytes(&journal)).unwrap();
        invalid["evidence"]["retired_component"] = json!({"status": "invalid"});
        journal.seed_value(&invalid);
        drop(ordinary_refresh(&journal).unwrap().unwrap());
        let repaired: Value = serde_json::from_slice(&record_bytes(&journal)).unwrap();
        validate_brain_state_record(&repaired, fixture_now()).unwrap();
        assert_eq!(repaired["credential_holds"], holds);
        journal.write_config("lane_none");
        assert!(ordinary_refresh(&journal).unwrap().is_none());
        let none: Value = serde_json::from_slice(&record_bytes(&journal)).unwrap();
        assert_eq!(none["credential_holds"], holds);
    }

    #[test]
    fn transient_runtime_and_abandon_writes_preserve_holds_and_return_the_stored_record() {
        let journal = TestJournal::new();
        write_openai_binding(&journal, false);
        drop(ordinary_refresh(&journal).unwrap().unwrap());
        let holds = seed_credential_hold(&journal);
        let permit = ordinary_refresh(&journal).unwrap().unwrap();
        let abandoned = abandon_refresh(
            journal.path(),
            permit,
            "provider_unavailable",
            Map::new(),
            fixture_now(),
        )
        .unwrap();
        assert_eq!(abandoned["credential_holds"], holds);
        assert_eq!(
            serde_json::from_slice::<Value>(&record_bytes(&journal)).unwrap(),
            abandoned
        );
        let fingerprint = abandoned["fingerprint_sha256"].as_str().unwrap();
        let failure = record_runtime_failure(
            journal.path(),
            "provider_unavailable",
            "generate",
            fingerprint,
            Map::new(),
            fixture_now(),
            None,
        );
        assert!(failure.accepted, "{failure:?}");
        let written = failure.record.unwrap();
        assert_eq!(written["credential_holds"], holds);
        assert_eq!(
            serde_json::from_slice::<Value>(&record_bytes(&journal)).unwrap(),
            written
        );
    }

    #[test]
    fn newer_and_repeated_same_key_holds_defeat_an_older_refresh_success() {
        for existing_hold in [false, true] {
            let journal = TestJournal::new();
            write_openai_binding(&journal, false);
            drop(ordinary_refresh(&journal).unwrap().unwrap());
            if existing_hold {
                seed_credential_hold(&journal);
            }
            let permit = ordinary_refresh(&journal).unwrap().unwrap();
            let key = load_existing_fingerprint_key(journal.path()).unwrap();
            let hold = CredentialRejectionHold {
                credential_hmac: crate::managed_credential_binding_hmac(
                    &key,
                    "openai",
                    "sk-binding",
                ),
                marker_id: "a".repeat(64),
            };
            record_credential_hold_bound(
                Arc::new(JournalRoot::open(journal.path()).unwrap()),
                journal.path(),
                "openai",
                &hold,
                &key,
                &|| Ok(()),
            )
            .unwrap();
            let before = record_bytes(&journal);
            assert!(
                finish_refresh(journal.path(), permit, ready_outcome(), fixture_now(), None)
                    .is_err()
            );
            assert_eq!(record_bytes(&journal), before);
            let current = ordinary_refresh(&journal).unwrap().unwrap();
            let written = finish_refresh(
                journal.path(),
                current,
                ready_outcome(),
                fixture_now(),
                None,
            )
            .unwrap();
            assert_eq!(
                written["credential_holds"]["openai"]["marker_id"],
                hold.marker_id
            );
        }
    }

    #[test]
    fn corrupt_hold_authority_is_not_discarded_by_derived_record_repair() {
        let journal = TestJournal::new();
        write_openai_binding(&journal, false);
        drop(ordinary_refresh(&journal).unwrap().unwrap());
        seed_credential_hold(&journal);
        let mut record: Value = serde_json::from_slice(&record_bytes(&journal)).unwrap();
        record["credential_holds"]["openai"]["credential_hmac"] = json!("broken");
        journal.seed_value(&record);
        let original = record_bytes(&journal);
        assert!(ordinary_refresh(&journal).is_err());
        assert_eq!(record_bytes(&journal), original);
    }

    #[test]
    fn runtime_failure_publishes_a_valid_marker_record() {
        let journal = TestJournal::new();
        journal.write_config("lane_byo_cloud");
        let now = fixture_now();
        let permit = begin_refresh(
            journal.path(),
            now,
            Some("run".to_owned()),
            None,
            false,
            None,
        )
        .unwrap()
        .expect("refresh permit");
        let result = record_runtime_failure(
            journal.path(),
            "provider_unavailable",
            "generate",
            &permit.fingerprint_sha256,
            Map::new(),
            now,
            None,
        );
        assert!(result.accepted, "{result:?}");
        let record = result.record.expect("published record");
        validate_brain_state_record(&record, now).unwrap();
        assert!(record["checking"].is_null());
        assert_eq!(
            record["runtime_failure_marker"]["reason_code"],
            "provider_unavailable"
        );
        drop(permit);
    }

    #[test]
    fn runtime_failure_precedence_prefers_reason_over_component() {
        // There is no (1, 2) pair test: the native API receives `DateTime<Utc>`,
        // so Python's fallible now-normalization has already completed before this
        // function can run. Invalid timestamps are structurally unrepresentable here.
        let journal = TestJournal::new();
        let result = record_runtime_failure(
            journal.path(),
            "not-a-reason",
            "not-a-component",
            "not-a-sha256",
            Map::new(),
            fixture_now(),
            None,
        );
        assert_eq!(
            result.rejected_reason.as_deref(),
            Some("reason_not_recordable")
        );
    }

    #[test]
    fn runtime_failure_precedence_prefers_component_over_fingerprint() {
        let journal = TestJournal::new();
        let result = record_runtime_failure(
            journal.path(),
            "provider_unavailable",
            "configuration",
            "not-a-sha256",
            Map::new(),
            fixture_now(),
            None,
        );
        assert_eq!(
            result.rejected_reason.as_deref(),
            Some("component_reason_not_allowed")
        );
    }

    #[test]
    fn runtime_failure_precedence_prefers_component_over_invalid_diagnostic() {
        let journal = TestJournal::new();
        let mut diagnostic = Map::new();
        diagnostic.insert("unexpected".to_owned(), json!("value"));
        let result = record_runtime_failure(
            journal.path(),
            "provider_unavailable",
            "configuration",
            &"a".repeat(64),
            diagnostic,
            fixture_now(),
            None,
        );
        assert_eq!(
            result.rejected_reason.as_deref(),
            Some("component_reason_not_allowed")
        );
    }

    #[test]
    fn runtime_failure_precedence_prefers_invalid_diagnostic_over_bad_fingerprint() {
        let journal = TestJournal::new();
        let mut diagnostic = Map::new();
        diagnostic.insert("unexpected".to_owned(), json!("value"));
        let result = record_runtime_failure(
            journal.path(),
            "provider_unavailable",
            "generate",
            "not-a-sha256",
            diagnostic,
            fixture_now(),
            None,
        );
        assert_eq!(
            result.rejected_reason.as_deref(),
            Some("reason_not_recordable")
        );
    }

    #[test]
    fn runtime_failure_precedence_prefers_bad_fingerprint_over_unreadable_record() {
        let journal = TestJournal::new();
        journal.write_config("lane_byo_cloud");
        fs::create_dir_all(brain_state_path(journal.path())).unwrap();
        let result = record_runtime_failure(
            journal.path(),
            "provider_unavailable",
            "generate",
            "not-a-sha256",
            Map::new(),
            fixture_now(),
            None,
        );
        assert_eq!(
            result.rejected_reason.as_deref(),
            Some("fingerprint_mismatch")
        );
    }

    #[test]
    fn runtime_failure_precedence_prefers_unreadable_record_over_missing_fingerprint() {
        let journal = TestJournal::new();
        journal.write_config("lane_byo_cloud");
        fs::create_dir_all(brain_state_path(journal.path())).unwrap();
        let result = record_runtime_failure(
            journal.path(),
            "provider_unavailable",
            "generate",
            &"a".repeat(64),
            Map::new(),
            fixture_now(),
            None,
        );
        assert_eq!(result.rejected_reason.as_deref(), Some("state_unavailable"));
    }

    #[test]
    fn runtime_failure_precedence_prefers_missing_fingerprint_over_mismatch() {
        let journal = TestJournal::new();
        journal.write_config("lane_byo_cloud");
        let result = record_runtime_failure(
            journal.path(),
            "provider_unavailable",
            "generate",
            &"a".repeat(64),
            Map::new(),
            fixture_now(),
            None,
        );
        assert_eq!(
            result.rejected_reason.as_deref(),
            Some("fingerprint_not_available")
        );
    }

    fn ready_evidence_spp_for(now: DateTime<Utc>) -> Value {
        let component = json!({
            "status": "ok",
            "observed_at": iso(now),
            "expires_at": iso(now + Duration::days(1)),
        });
        json!({
            "configuration": component.clone(),
            "lane_prerequisites": component.clone(),
            "generate": component,
        })
    }

    #[test]
    fn attestation_refusal_ignores_non_spp_lane() {
        let journal = TestJournal::new();
        journal.write_config("lane_byo_cloud");
        let config = solstone_core_journal_config::read_journal_config(journal.path())
            .unwrap()
            .config
            .unwrap();
        let now = Utc::now();
        record_confidential_attestation_refusal(journal.path(), &config, "certificate_invalid");
        let inspection = inspect_brain_state(journal.path(), &config, now);
        assert_eq!(
            inspection.status,
            crate::inspect::InspectionStatus::Unavailable
        );
        assert!(inspection.record.is_none());
        assert!(!brain_fingerprint_key_path(journal.path()).exists());
    }

    #[test]
    fn attestation_refusal_ignores_missing_fingerprint_key() {
        let journal = TestJournal::new();
        journal.write_config("lane_spp");
        let config = solstone_core_journal_config::read_journal_config(journal.path())
            .unwrap()
            .config
            .unwrap();
        let now = Utc::now();
        record_confidential_attestation_refusal(journal.path(), &config, "certificate_invalid");
        let inspection = inspect_brain_state(journal.path(), &config, now);
        assert_eq!(
            inspection.status,
            crate::inspect::InspectionStatus::Unavailable
        );
        assert!(inspection.record.is_none());
        assert!(!brain_fingerprint_key_path(journal.path()).exists());
    }

    #[test]
    fn attestation_refusal_ignores_stale_fingerprint() {
        let journal = TestJournal::new();
        journal.write_config("lane_spp");
        journal.write_fixture_key();
        let mut stale_config = solstone_core_journal_config::read_journal_config(journal.path())
            .unwrap()
            .config
            .unwrap();
        stale_config["providers"]["local"]["endpoint_url"] = json!("https://other.example/v1");
        journal.seed_record("lane_spp/ready");
        let before_bytes = record_bytes(&journal);
        record_confidential_attestation_refusal(
            journal.path(),
            &stale_config,
            "certificate_invalid",
        );
        assert_eq!(record_bytes(&journal), before_bytes);
    }

    #[test]
    fn attestation_refusal_records_when_no_prior_record_exists() {
        let journal = TestJournal::new();
        journal.write_config("lane_spp");
        generate_fingerprint_key(journal.path()).unwrap();
        let config = solstone_core_journal_config::read_journal_config(journal.path())
            .unwrap()
            .config
            .unwrap();

        record_confidential_attestation_refusal(journal.path(), &config, "gateway_unreachable");

        let now = Utc::now() + chrono::Duration::seconds(1);
        let inspection = inspect_brain_state(journal.path(), &config, now);
        assert_eq!(inspection.projection.aggregate_state, "blocked");
        assert_eq!(
            inspection.projection.reason_code.as_deref(),
            Some("attestation_not_verified")
        );
        let record = inspection.record.expect("record exists");
        assert_eq!(
            record["evidence"]["lane_prerequisites"]["reason_code"],
            "attestation_not_verified"
        );
    }

    #[test]
    fn attestation_refusal_carries_the_failing_register_to_the_owner() {
        let journal = TestJournal::new();
        journal.write_config("lane_spp");
        generate_fingerprint_key(journal.path()).unwrap();
        let config = solstone_core_journal_config::read_journal_config(journal.path())
            .unwrap()
            .config
            .unwrap();
        let shown = |raw: &str| {
            record_confidential_attestation_refusal(journal.path(), &config, raw);
            let now = Utc::now() + chrono::Duration::seconds(1);
            let inspection = inspect_brain_state(journal.path(), &config, now);
            assert_eq!(
                inspection.projection.reason_code.as_deref(),
                Some("attestation_rejected")
            );
            let record = inspection.record.clone().expect("record exists");
            let view = crate::present_brain_inspection(&inspection, now);
            (record, view.reason_text)
        };

        let (record, register_9) = shown("pcr_9_mismatch");
        assert_eq!(
            record["evidence"]["lane_prerequisites"]["diagnostic"]["register"],
            "9"
        );
        assert!(register_9.contains('9'));

        // Another register on the same reason replaces the one shown.
        let (record, register_22) = shown("pcr_22_mismatch");
        assert_eq!(
            record["evidence"]["lane_prerequisites"]["diagnostic"]["register"],
            "22"
        );
        assert!(register_22.contains("22"));
        assert_ne!(register_9, register_22);

        // A refusal that names no register shows none.
        let (record, unnamed) = shown("certificate_invalid");
        assert!(
            record["evidence"]["lane_prerequisites"]["diagnostic"]
                .get("register")
                .is_none()
        );
        assert_eq!(
            unnamed,
            crate::brain_reason_text(Some("attestation_rejected"))
        );

        // Closed HTTP categories survive the real writer and record reader,
        // while the owner-facing reason and register behavior stay unchanged.
        for raw in [
            "proof_http_failed",
            "proof_http_timeout",
            "proof_http_eof",
            "proof_http_transport",
            "proof_http_framing",
            "proof_http_status",
            "proof_http_content_type",
            "proof_http_trailing",
            "proof_http_channel_state",
        ] {
            let (record, text) = shown(raw);
            assert_eq!(
                record["evidence"]["lane_prerequisites"]["diagnostic"],
                json!({"verification_reason": raw})
            );
            assert_eq!(text, unnamed);
            let mut unknown = record.clone();
            unknown["evidence"]["lane_prerequisites"]["diagnostic"]["verification_reason"] =
                json!("proof_http_unknown");
            assert!(validate_brain_state_record(&unknown, Utc::now()).is_err());
        }
        for raw in [
            "proof_http_unknown",
            "proof_http_timeout: private-path",
            "Bearer private-token",
        ] {
            let (record, _) = shown(raw);
            assert_eq!(
                record["evidence"]["lane_prerequisites"]["diagnostic"],
                json!({})
            );
        }
        let (record, _) = shown("pcr_9_mismatch");
        assert_eq!(
            record["evidence"]["lane_prerequisites"]["diagnostic"],
            json!({"register": "9"})
        );
    }

    #[test]
    fn attestation_refusal_settled_idempotence_and_different_reason_bump() {
        let journal = TestJournal::new();
        journal.write_config("lane_spp");
        generate_fingerprint_key(journal.path()).unwrap();
        let config = solstone_core_journal_config::read_journal_config(journal.path())
            .unwrap()
            .config
            .unwrap();

        record_confidential_attestation_refusal(journal.path(), &config, "gateway_unreachable");
        let now1 = Utc::now() + chrono::Duration::seconds(1);
        let insp1 = inspect_brain_state(journal.path(), &config, now1);
        let rev1 = insp1.record.as_ref().unwrap()["revision"].as_u64().unwrap();
        assert_eq!(insp1.projection.aggregate_state, "blocked");
        assert_eq!(
            insp1.projection.reason_code.as_deref(),
            Some("attestation_not_verified")
        );

        // Same reason does not bump revision
        record_confidential_attestation_refusal(journal.path(), &config, "gateway_unreachable");
        let now2 = Utc::now() + chrono::Duration::seconds(1);
        let insp2 = inspect_brain_state(journal.path(), &config, now2);
        assert_eq!(
            insp2.record.as_ref().unwrap()["revision"].as_u64().unwrap(),
            rev1
        );

        // Different raw reason (tls_handshake_failed -> attestation_rejected) bumps revision
        record_confidential_attestation_refusal(journal.path(), &config, "tls_handshake_failed");
        let now3 = Utc::now() + chrono::Duration::seconds(1);
        let insp3 = inspect_brain_state(journal.path(), &config, now3);
        assert_eq!(
            insp3.record.as_ref().unwrap()["revision"].as_u64().unwrap(),
            rev1 + 1
        );
        assert_eq!(insp3.projection.aggregate_state, "unhealthy");
        assert_eq!(
            insp3.projection.reason_code.as_deref(),
            Some("attestation_rejected")
        );
        assert_eq!(
            insp3.record.as_ref().unwrap()["evidence"]["lane_prerequisites"]["reason_code"],
            "attestation_rejected"
        );
    }

    #[test]
    fn attestation_refusal_nvattest_reasons_preserved() {
        let journal = TestJournal::new();
        journal.write_config("lane_spp");
        generate_fingerprint_key(journal.path()).unwrap();
        let config = solstone_core_journal_config::read_journal_config(journal.path())
            .unwrap()
            .config
            .unwrap();

        // nvattest_unavailable
        record_confidential_attestation_refusal(journal.path(), &config, "nvattest_unavailable");
        let now1 = Utc::now() + chrono::Duration::seconds(1);
        let insp1 = inspect_brain_state(journal.path(), &config, now1);
        let expected_agg = local_contract()
            .brain_state
            .reason_to_aggregate
            .get("nvattest_unavailable")
            .expect("aggregate mapping for nvattest_unavailable");
        assert_eq!(insp1.projection.aggregate_state, *expected_agg);
        assert_eq!(
            insp1.projection.reason_code.as_deref(),
            Some("nvattest_unavailable")
        );
        assert_eq!(
            insp1.record.as_ref().unwrap()["evidence"]["lane_prerequisites"]["reason_code"],
            "nvattest_unavailable"
        );

        // nvattest_integrity_failed
        record_confidential_attestation_refusal(
            journal.path(),
            &config,
            "nvattest_integrity_failed",
        );
        let now2 = Utc::now() + chrono::Duration::seconds(1);
        let insp2 = inspect_brain_state(journal.path(), &config, now2);
        assert_eq!(insp2.projection.aggregate_state, "unhealthy");
        assert_eq!(
            insp2.projection.reason_code.as_deref(),
            Some("nvattest_integrity_failed")
        );
        assert_eq!(
            insp2.record.as_ref().unwrap()["evidence"]["lane_prerequisites"]["reason_code"],
            "nvattest_integrity_failed"
        );

        // A persisted record with a retired code fails validation, becomes brain_record_invalid,
        // renders stale, and the owner text has no "nvattest".
        for retired in ["nvattest_install_failed", "nvattest_install_in_progress"] {
            let mut corrupt_record = insp2.record.clone().unwrap();
            corrupt_record["reason_code"] = serde_json::Value::String(retired.to_owned());
            corrupt_record["evidence"]["lane_prerequisites"]["reason_code"] =
                serde_json::Value::String(retired.to_owned());
            let state_path = brain_state_path(journal.path());
            std::fs::write(&state_path, serde_json::to_vec(&corrupt_record).unwrap()).unwrap();

            let inspected = inspect_brain_state(journal.path(), &config, Utc::now());
            assert_eq!(inspected.status, crate::inspect::InspectionStatus::Corrupt);
            assert_eq!(
                inspected.projection.reason_code.as_deref(),
                Some("brain_record_invalid")
            );
            assert!(inspected.record.is_none());
            let presentation =
                crate::presentation::present_brain_inspection(&inspected, Utc::now());
            assert!(!presentation.reason_text.contains("nvattest"));
            assert!(!presentation.headline.contains("nvattest"));
        }
    }

    #[test]
    fn attestation_refusal_clears_checking_conflicts_finish_and_subsequent_refresh_clears() {
        let journal = TestJournal::new();
        journal.write_config("lane_spp");
        generate_fingerprint_key(journal.path()).unwrap();
        let config = solstone_core_journal_config::read_journal_config(journal.path())
            .unwrap()
            .config
            .unwrap();
        let now = Utc::now();
        let permit = begin_refresh(
            journal.path(),
            now,
            Some("renew".to_owned()),
            None,
            false,
            None,
        )
        .unwrap()
        .unwrap();

        record_confidential_attestation_refusal(journal.path(), &config, "gateway_unreachable");

        let after_now = Utc::now() + chrono::Duration::seconds(1);
        let after = inspect_brain_state(journal.path(), &config, after_now);
        let record = after.record.expect("record exists");
        assert!(record["checking"].is_null());
        assert_eq!(after.projection.aggregate_state, "blocked");
        assert_eq!(
            after.projection.reason_code.as_deref(),
            Some("attestation_not_verified")
        );
        assert_eq!(
            record["evidence"]["lane_prerequisites"]["reason_code"],
            "attestation_not_verified"
        );

        // Subsequent finish on old permit is rejected with Conflict
        let ready = ready_evidence_spp_for(after_now);
        assert!(finish_refresh(journal.path(), permit, ready, after_now, None).is_err());

        // Still shows refusal after conflict
        let insp_after = inspect_brain_state(journal.path(), &config, after_now);
        assert_eq!(insp_after.projection.aggregate_state, "blocked");
        assert_eq!(
            insp_after.projection.reason_code.as_deref(),
            Some("attestation_not_verified")
        );

        // New refresh succeeds and clears refusal marker
        let new_permit = begin_refresh(journal.path(), after_now, None, None, false, None)
            .unwrap()
            .unwrap();
        let ready2 = ready_evidence_spp_for(after_now);
        let finished = finish_refresh(journal.path(), new_permit, ready2, after_now, None).unwrap();
        assert!(finished["runtime_failure_marker"].is_null());
        assert_eq!(finished["aggregate_state"], "ready");
        assert!(finished["reason_code"].is_null());

        let insp_cleared = inspect_brain_state(journal.path(), &config, after_now);
        assert_eq!(insp_cleared.projection.aggregate_state, "ready");
        assert_eq!(insp_cleared.projection.reason_code, None);
    }

    #[test]
    fn predecessor_with_cogitate_evidence_is_replaced_by_generate_refresh() {
        let journal = TestJournal::new();
        journal.write_config("lane_byo_cloud");
        let config_path = journal.path().join("config/journal.json");
        let mut config: Map<String, Value> =
            serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
        if let Some(active) = config
            .get_mut("providers")
            .and_then(Value::as_object_mut)
            .and_then(|p| p.get_mut("active"))
            .and_then(Value::as_object_mut)
        {
            active.insert("placeholder_cred".to_owned(), json!("secret-123"));
            active.insert("unrelated_literal".to_owned(), json!("preserved-value"));
        }
        fs::write(&config_path, serde_json::to_vec(&config).unwrap()).unwrap();

        let now = fixture_now();
        let key = generate_fingerprint_key(journal.path()).unwrap();
        let sha256 = build_active_brain_fingerprint(&config, &key, None, Some(journal.path()))
            .unwrap()
            .unwrap();

        let brain_file = brain_state_path(journal.path());
        let predecessor_json = json!({
            "schema_version": 1,
            "revision": 5,
            "updated_at": iso(now),
            "fingerprint_sha256": sha256,
            "active_lane": "byo-cloud",
            "active_provider": "anthropic",
            "active_model": "claude",
            "aggregate_state": "unhealthy",
            "reason_code": "cogitate_terminal_error",
            "checking": null,
            "runtime_failure_marker": null,
            "evidence": {
                "configuration": {
                    "status": "ok",
                    "observed_at": iso(now),
                    "expires_at": iso(now + Duration::days(1)),
                    "reason_code": null,
                    "diagnostic": {}
                },
                "lane_prerequisites": {
                    "status": "ok",
                    "observed_at": iso(now),
                    "expires_at": iso(now + Duration::days(1)),
                    "reason_code": null,
                    "diagnostic": {}
                },
                "generate": {
                    "status": "ok",
                    "observed_at": iso(now),
                    "expires_at": iso(now + Duration::days(1)),
                    "reason_code": null,
                    "diagnostic": {}
                },
                "cogitate": {
                    "status": "failed",
                    "observed_at": iso(now),
                    "expires_at": iso(now + Duration::days(1)),
                    "reason_code": "cogitate_terminal_error",
                    "diagnostic": {}
                }
            },
            "diagnostic": {}
        });
        fs::write(&brain_file, serde_json::to_vec(&predecessor_json).unwrap()).unwrap();

        // Predecessor inspects Corrupt / brain_record_invalid / aggregate unknown
        let inspection = inspect_brain_state(journal.path(), &config, now);
        assert_eq!(inspection.status, InspectionStatus::Corrupt);
        assert_eq!(
            inspection.projection.reason_code.as_deref(),
            Some("brain_record_invalid")
        );
        assert_eq!(inspection.projection.aggregate_state, "unknown");
        assert!(inspection.record.is_none());

        // begin_refresh replaces predecessor
        let permit = begin_refresh(journal.path(), now, None, None, false, None)
            .unwrap()
            .expect("refresh permit acquired over retired predecessor");

        // Finishing with a Generate refusal still blocks or degrades
        let failed_evidence = json!({
            "configuration": {
                "status": "ok",
                "observed_at": iso(now),
                "expires_at": iso(now + Duration::days(1))
            },
            "lane_prerequisites": {
                "status": "ok",
                "observed_at": iso(now),
                "expires_at": iso(now + Duration::days(1))
            },
            "generate": {
                "status": "failed",
                "reason_code": "provider_quota_exceeded",
                "observed_at": iso(now),
                "expires_at": iso(now + Duration::days(1))
            }
        });
        let refused_finished =
            finish_refresh(journal.path(), permit, failed_evidence, now, None).unwrap();
        assert_eq!(refused_finished["aggregate_state"], "unhealthy");
        assert_eq!(refused_finished["reason_code"], "provider_quota_exceeded");

        // A following successful Generate finish is ready and has no cogitate component
        let permit2 = begin_refresh(journal.path(), now, None, None, false, None)
            .unwrap()
            .expect("second refresh permit");
        let ok_evidence = json!({
            "configuration": {
                "status": "ok",
                "observed_at": iso(now),
                "expires_at": iso(now + Duration::days(1))
            },
            "lane_prerequisites": {
                "status": "ok",
                "observed_at": iso(now),
                "expires_at": iso(now + Duration::days(1))
            },
            "generate": {
                "status": "ok",
                "observed_at": iso(now),
                "expires_at": iso(now + Duration::days(1))
            }
        });
        let success_finished =
            finish_refresh(journal.path(), permit2, ok_evidence, now, None).unwrap();
        assert_eq!(success_finished["aggregate_state"], "ready");
        assert!(success_finished["reason_code"].is_null());
        assert!(
            !success_finished["evidence"]
                .as_object()
                .unwrap()
                .contains_key("cogitate")
        );

        let inspection2 = inspect_brain_state(journal.path(), &config, now);
        assert_eq!(inspection2.status, InspectionStatus::Ok);
        assert_eq!(inspection2.projection.aggregate_state, "ready");
        assert_eq!(
            inspection2.projection.active_lane.as_deref(),
            Some("byo-cloud")
        );
        assert_eq!(
            inspection2.projection.active_provider.as_deref(),
            Some("anthropic")
        );
        assert_eq!(
            inspection2.projection.active_model.as_deref(),
            Some("claude-sonnet-4-6")
        );

        // Verify config literals in providers.active survived unchanged
        let read_back_config = solstone_core_journal_config::read_journal_config(journal.path())
            .unwrap()
            .config
            .unwrap();
        let active = read_back_config["providers"]["active"].as_object().unwrap();
        assert_eq!(active["placeholder_cred"], "secret-123");
        assert_eq!(active["unrelated_literal"], "preserved-value");

        // Invalid derived state is recoverable regardless of its previous format.
        let malformed_json = json!({
            "schema_version": 1,
            "revision": 1,
            "updated_at": iso(now),
            "fingerprint_sha256": sha256,
            "active_lane": "byo-cloud",
            "active_provider": "anthropic",
            "active_model": "claude",
            "aggregate_state": "ready",
            "evidence": {
                "configuration": "not-an-object"
            }
        });
        fs::write(&brain_file, serde_json::to_vec(&malformed_json).unwrap()).unwrap();
        let permit = begin_refresh(journal.path(), now, None, None, false, None)
            .unwrap()
            .expect("refresh permit over malformed derived state");
        let checking: Value = serde_json::from_slice(&record_bytes(&journal)).unwrap();
        assert_eq!(checking["aggregate_state"], "checking");
        assert_ne!(checking["aggregate_state"], "ready");
        drop(permit);

        // Filesystem failures remain errors rather than being treated as absent evidence.
        fs::remove_file(&brain_file).unwrap();
        fs::create_dir(&brain_file).unwrap();
        assert!(matches!(
            begin_refresh(journal.path(), now, None, None, false, None),
            Err(BeginRefreshError::Writer(WriterError::Io(_)))
        ));
    }
}
