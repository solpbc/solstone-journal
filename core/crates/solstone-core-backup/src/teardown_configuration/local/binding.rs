// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Exact local finalization. No remote, lease or carrier completion is inferred.

use super::*;
#[cfg(unix)]
use solstone_core_journal_io::ClaimName;
use solstone_core_journal_io::{
    BoundParentLock, ClaimRemovalError, ClaimRemovalOutcome, ExistingParentLockError,
    FlatDirectoryError, acquire_existing_parent_lock_bound, open_existing_parent_lock_bound,
};
use std::io;
use std::time::Duration;

/// The bound native parent each platform publishes and removes through.
#[cfg(unix)]
mod native {
    use super::*;
    pub(super) use solstone_core_journal_io::FlatDirectory as Parent;
    use solstone_core_journal_io::{
        FileObservation, claim_and_remove_observed_bounded, read_observed_file_bounded,
    };

    pub(super) fn open(
        root: &Arc<JournalRoot>,
        relative: &[&str],
    ) -> Result<Parent, FlatDirectoryError> {
        Parent::open(root, &relative.iter().collect::<std::path::PathBuf>())
    }
    pub(super) fn identity(parent: &Parent) -> solstone_core_journal_io::ObjectIdentity {
        parent.identity()
    }
    pub(super) fn revalidate(parent: &Parent) -> Result<(), FlatDirectoryError> {
        parent.revalidate()
    }
    pub(super) fn read(
        parent: &Parent,
        name: &OsStr,
        limit: usize,
    ) -> Result<Option<FileObservation>, FlatDirectoryError> {
        read_observed_file_bounded(parent, name, limit)
    }
    pub(super) fn remove(
        parent: &Parent,
        name: &OsStr,
        prior: &FileObservation,
        claim: &ClaimName,
        limit: usize,
    ) -> Result<ClaimRemovalOutcome, ClaimRemovalError> {
        claim_and_remove_observed_bounded(parent, name, prior, claim, limit)
    }
}

#[cfg(windows)]
mod native {
    use super::*;
    pub(super) use solstone_core_journal_io::WindowsFlatDirectory as Parent;
    use solstone_core_journal_io::{
        FileObservation, open_windows_flat_directory_bound, read_windows_observed_file_bounded,
        remove_windows_observed_file_bounded,
    };

    pub(super) fn open(
        root: &Arc<JournalRoot>,
        relative: &[&str],
    ) -> Result<Parent, FlatDirectoryError> {
        let mut diagnostic = root.canonical_path().to_path_buf();
        let mut current: Option<Parent> = None;
        for name in relative {
            let next = match &current {
                Some(parent) => {
                    open_windows_flat_directory_bound(parent, OsStr::new(name), &diagnostic)?
                }
                None => open_windows_flat_directory_bound(&**root, OsStr::new(name), &diagnostic)?,
            };
            diagnostic.push(name);
            current = Some(next.ok_or_else(|| FlatDirectoryError::NotDirectory {
                path: diagnostic.clone(),
            })?);
        }
        current.ok_or(FlatDirectoryError::NotDirectory { path: diagnostic })
    }
    pub(super) fn identity(parent: &Parent) -> solstone_core_journal_io::ObjectIdentity {
        parent.object_identity()
    }
    pub(super) fn revalidate(parent: &Parent) -> Result<(), FlatDirectoryError> {
        parent.revalidate_bound()
    }
    pub(super) fn read(
        parent: &Parent,
        name: &OsStr,
        limit: usize,
    ) -> Result<Option<FileObservation>, FlatDirectoryError> {
        read_windows_observed_file_bounded(parent, name, limit)
    }
    /// Windows deletes through the verified handle, so no claim name is used.
    pub(super) fn remove(
        parent: &Parent,
        name: &OsStr,
        prior: &FileObservation,
        limit: usize,
    ) -> Result<ClaimRemovalOutcome, ClaimRemovalError> {
        remove_windows_observed_file_bounded(parent, name, prior, limit)
    }
}

const CONFIG_LOCK: &str = "journal.json.lock";
const BINDING_LOCK: &str = "binding.json.lock";
const BINDING: &str = "binding.json";

pub enum BindingFinalizationRefusal {
    Configuration(Refusal),
    PublicationUncertain,
    MissingOriginalBinding,
    ParentChanged,
    BindingChanged,
    RemovalUnconfirmed,
    Native(io::Error),
    Flat(FlatDirectoryError),
    Lock(ExistingParentLockError),
}

impl std::fmt::Debug for BindingFinalizationRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Configuration(_) => "Configuration",
            Self::PublicationUncertain => "PublicationUncertain",
            Self::MissingOriginalBinding => "MissingOriginalBinding",
            Self::ParentChanged => "ParentChanged",
            Self::BindingChanged => "BindingChanged",
            Self::RemovalUnconfirmed => "RemovalUnconfirmed",
            Self::Native(_) => "Native",
            Self::Flat(_) => "Flat",
            Self::Lock(_) => "Lock",
        })
    }
}

/// Actual one-shot removal facts. Neither decoding nor cloning a handle to
/// these facts can recreate the retained guards or permit carrier retirement.
pub struct BindingRemovalReceipt {
    attempted: bool,
    native: Option<Result<ClaimRemovalOutcome, ClaimRemovalError>>,
    confirmation: Result<(), BindingFinalizationRefusal>,
}

impl BindingRemovalReceipt {
    pub fn attempted(&self) -> bool {
        self.attempted
    }
    pub fn native(&self) -> Option<&Result<ClaimRemovalOutcome, ClaimRemovalError>> {
        self.native.as_ref()
    }
    pub fn confirmation(&self) -> &Result<(), BindingFinalizationRefusal> {
        &self.confirmation
    }
    /// The exact binding is gone and confirmed absent. Windows reports
    /// `RemovedUnflushed`: it has no directory flush.
    pub fn complete(&self) -> bool {
        matches!(
            self.native,
            Some(Ok(
                ClaimRemovalOutcome::Removed | ClaimRemovalOutcome::RemovedUnflushed
            ))
        ) && self.confirmation.is_ok()
    }
}

/// Keeps config→binding exclusion and the actual original parent chains until
/// explicitly consumed. Dropping it releases guards without cleaning entries.
#[must_use]
pub struct BindingFinalization {
    root: Arc<JournalRoot>,
    config: Arc<MediaDirectory>,
    config_parent: native::Parent,
    config_lock: BoundParentLock,
    binding: Arc<MediaDirectory>,
    binding_parent: native::Parent,
    binding_lock: BoundParentLock,
    expected: MediaRead,
    candidate_sha256: String,
    published_identity: MediaIdentity,
    published_size: u64,
    #[cfg(unix)]
    claim: ClaimName,
    receipt: Option<Arc<BindingRemovalReceipt>>,
}

pub(in crate::teardown_configuration) fn begin(
    captured: &TeardownConfiguration,
    publication: &BoundConfigPublication,
) -> Result<BindingFinalization, BindingFinalizationRefusal> {
    use BindingFinalizationRefusal as Failure;
    if publication.durability_uncertainty.is_some()
        || publication.observation_uncertainty.is_some()
        || publication.confirmation_refusal.is_some()
    {
        return Err(Failure::PublicationUncertain);
    }
    if captured.expected_clear_sha256.as_deref() != Some(publication.candidate_sha256.as_str()) {
        return Err(Failure::Configuration(Refusal::ConfigurationChanged));
    }
    captured
        .revalidate_published_clear(publication)
        .map_err(Failure::Configuration)?;
    let original = captured
        .binding_observation
        .as_ref()
        .ok_or(Failure::MissingOriginalBinding)?;
    let config_parent = native::open(&captured.root, &["config"]).map_err(Failure::Flat)?;
    let binding_parent =
        native::open(&captured.root, &["backup", "hosted"]).map_err(Failure::Flat)?;
    if native::identity(&config_parent) != captured.directory.object_identity()
        || native::identity(&binding_parent) != original.directory.object_identity()
    {
        return Err(Failure::ParentChanged);
    }
    captured
        .revalidate_published_clear(publication)
        .map_err(Failure::Configuration)?;
    // Clear has returned and released its own lock. Never nest an acquisition.
    let config_lock = lock(&config_parent, CONFIG_LOCK)?;
    captured
        .revalidate_published_clear(publication)
        .map_err(Failure::Configuration)?;
    let binding_lock = lock(&binding_parent, BINDING_LOCK)?;
    captured
        .revalidate_published_clear(publication)
        .map_err(Failure::Configuration)?;
    #[cfg(unix)]
    let claim = {
        let mut entropy = [0u8; 8];
        getrandom::fill(&mut entropy)
            .map_err(|_| Failure::Native(io::Error::other("binding claim entropy unavailable")))?;
        ClaimName::parse(&format!(
            "!solstone-claim-{:08x}-{:016x}",
            std::process::id(),
            u64::from_le_bytes(entropy)
        ))
        .map_err(|_| Failure::Native(io::Error::other("binding claim name refused")))?
    };
    let held = BindingFinalization {
        root: captured.root.clone(),
        config: captured.directory.clone(),
        config_parent,
        config_lock,
        binding: original.directory.clone(),
        binding_parent,
        binding_lock,
        expected: original.read.clone(),
        candidate_sha256: publication.candidate_sha256.clone(),
        published_identity: publication
            .published_identity
            .ok_or(Failure::PublicationUncertain)?,
        published_size: publication
            .published_file
            .as_ref()
            .ok_or(Failure::PublicationUncertain)?
            .size,
        #[cfg(unix)]
        claim,
        receipt: None,
    };
    held.validate_original()?;
    Ok(held)
}

fn lock(
    parent: &native::Parent,
    name: &str,
) -> Result<BoundParentLock, BindingFinalizationRefusal> {
    let name = OsStr::new(name);
    match open_existing_parent_lock_bound(parent, name, Duration::ZERO, Duration::ZERO)
        .map_err(BindingFinalizationRefusal::Lock)?
    {
        Some(guard) => Ok(guard),
        None => acquire_existing_parent_lock_bound(parent, name, Duration::ZERO, Duration::ZERO)
            .map_err(BindingFinalizationRefusal::Lock),
    }
}

impl BindingFinalization {
    fn validate_guards(&self) -> Result<(), BindingFinalizationRefusal> {
        use BindingFinalizationRefusal as Failure;
        self.root
            .revalidate_canonical_binding()
            .map_err(|_| Failure::Configuration(Refusal::RootChanged))?;
        self.config.revalidate().map_err(Failure::Native)?;
        self.binding.revalidate().map_err(Failure::Native)?;
        native::revalidate(&self.config_parent).map_err(Failure::Flat)?;
        native::revalidate(&self.binding_parent).map_err(Failure::Flat)?;
        if native::identity(&self.config_parent) != self.config.object_identity()
            || native::identity(&self.binding_parent) != self.binding.object_identity()
        {
            return Err(Failure::ParentChanged);
        }
        self.config_lock
            .revalidate_bound(&self.config_parent, OsStr::new(CONFIG_LOCK))
            .map_err(Failure::Lock)?;
        self.binding_lock
            .revalidate_bound(&self.binding_parent, OsStr::new(BINDING_LOCK))
            .map_err(Failure::Lock)?;
        let (read, entry) =
            read_journal_config_observed_in_directory(&self.config, MAX_BOUND_CONFIG_BYTES)
                .map_err(|_| Failure::Configuration(Refusal::ConfigurationUnavailable))?;
        let entry = entry.ok_or(Failure::Configuration(Refusal::ConfigurationChanged))?;
        if read.sha256.as_deref() != Some(self.candidate_sha256.as_str())
            || entry.name != OsStr::new("journal.json")
            || entry.kind != JournalEntryKind::RegularFile
            || entry.identity != self.published_identity
            || entry.size != self.published_size
        {
            return Err(Failure::Configuration(Refusal::ConfigurationChanged));
        }
        self.config.revalidate().map_err(Failure::Native)?;
        self.binding.revalidate().map_err(Failure::Native)?;
        self.root
            .revalidate_canonical_binding()
            .map_err(|_| Failure::Configuration(Refusal::RootChanged))
    }

    fn validate_original(&self) -> Result<(), BindingFinalizationRefusal> {
        self.validate_guards()?;
        if read_binding(&self.binding).map_err(BindingFinalizationRefusal::Configuration)?
            != self.expected
        {
            return Err(BindingFinalizationRefusal::BindingChanged);
        }
        self.validate_guards()
    }

    fn validate_absence(&self) -> Result<(), BindingFinalizationRefusal> {
        self.validate_guards()?;
        #[cfg(unix)]
        let names = [OsStr::new(BINDING), self.claim.as_os_str()];
        #[cfg(windows)]
        let names = [OsStr::new(BINDING)];
        for name in names {
            if self
                .binding
                .read_file_bounded(name, MAX_BINDING_BYTES)
                .map_err(BindingFinalizationRefusal::Native)?
                .is_some()
            {
                return Err(BindingFinalizationRefusal::RemovalUnconfirmed);
            }
        }
        self.validate_guards()
    }

    /// Attempt at most once; later calls borrow the exact original receipt.
    pub fn remove_binding(&mut self) -> &Arc<BindingRemovalReceipt> {
        self.remove_binding_with(|held, prior| {
            #[cfg(unix)]
            let result = native::remove(
                &held.binding_parent,
                OsStr::new(BINDING),
                prior,
                &held.claim,
                MAX_BINDING_BYTES,
            );
            #[cfg(windows)]
            let result = native::remove(
                &held.binding_parent,
                OsStr::new(BINDING),
                prior,
                MAX_BINDING_BYTES,
            );
            result
        })
    }

    /// Android removal requires the original teardown admission's retained lease.
    #[cfg(target_os = "android")]
    pub fn remove_binding_for_teardown(
        &mut self,
        lease: &Arc<solstone_core_journal_io::activity::TeardownLease>,
    ) -> &Arc<BindingRemovalReceipt> {
        if self.receipt.is_none() {
            let admission = if Arc::ptr_eq(&self.root, lease.journal_root()) {
                lease
                    .revalidate()
                    .map_err(|error| BindingFinalizationRefusal::Native(io::Error::other(error)))
            } else {
                Err(BindingFinalizationRefusal::Configuration(
                    Refusal::RootChanged,
                ))
            };
            if let Err(refusal) = admission {
                self.receipt = Some(Arc::new(BindingRemovalReceipt {
                    attempted: false,
                    native: None,
                    confirmation: Err(refusal),
                }));
            }
        }
        self.remove_binding_with(|held, prior| {
            solstone_core_journal_io::claim_and_remove_android_teardown_observed_bounded(
                lease,
                &held.binding_parent,
                std::path::Path::new("backup/hosted"),
                OsStr::new(BINDING),
                prior,
                &held.claim,
                MAX_BINDING_BYTES,
            )
        })
    }

    fn remove_binding_with(
        &mut self,
        remove: impl FnOnce(
            &Self,
            &solstone_core_journal_io::FileObservation,
        ) -> Result<ClaimRemovalOutcome, ClaimRemovalError>,
    ) -> &Arc<BindingRemovalReceipt> {
        if self.receipt.is_none() {
            let prior = (|| {
                self.validate_original()?;
                let prior =
                    native::read(&self.binding_parent, OsStr::new(BINDING), MAX_BINDING_BYTES)
                        .map_err(BindingFinalizationRefusal::Flat)?
                        .ok_or(BindingFinalizationRefusal::BindingChanged)?;
                self.validate_original()?;
                let expected = &self.expected;
                if prior.entry.name != expected.entry.name
                    || prior.entry.kind != expected.entry.kind
                    || prior.entry.size != expected.entry.size
                    || prior.bytes != expected.bytes
                    || !expected.entry.identity.matches_flat_entry(&prior.entry)
                {
                    return Err(BindingFinalizationRefusal::BindingChanged);
                }
                Ok(prior)
            })();
            let receipt = match prior {
                Err(refusal) => BindingRemovalReceipt {
                    attempted: false,
                    native: None,
                    confirmation: Err(refusal),
                },
                Ok(prior) => {
                    let native = remove(self, &prior);
                    let confirmation = self.validate_absence();
                    BindingRemovalReceipt {
                        attempted: true,
                        native: Some(native),
                        confirmation,
                    }
                }
            };
            self.receipt = Some(Arc::new(receipt));
        }
        self.receipt
            .as_ref()
            .expect("one-shot receipt stored above")
    }

    pub fn receipt(&self) -> Option<&Arc<BindingRemovalReceipt>> {
        self.receipt.as_ref()
    }

    /// Fresh local checks while both original locks remain held.
    pub fn revalidate_removed(&self) -> Result<(), BindingFinalizationRefusal> {
        if !self
            .receipt
            .as_ref()
            .is_some_and(|receipt| receipt.complete())
        {
            return Err(BindingFinalizationRefusal::RemovalUnconfirmed);
        }
        self.validate_absence()
    }

    /// Release local handles only. This never removes or repairs an entry.
    pub fn into_receipt(self) -> Option<Arc<BindingRemovalReceipt>> {
        self.receipt
    }
}

#[cfg(all(test, feature = "full-tests"))]
#[cfg(unix)]
mod tests;
