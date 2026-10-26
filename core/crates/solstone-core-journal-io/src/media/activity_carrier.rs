// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;
use crate::activity::{ActivityLease, TeardownLease};

/// Independent facts about preparation. Observations grant no cleanup authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ActivityCarrierPreparationReceipt {
    stage_durability: Durability,
    root_durability: Durability,
    confirmation: Confirmation,
}

impl ActivityCarrierPreparationReceipt {
    pub fn stage_durability(&self) -> Durability {
        self.stage_durability
    }

    pub fn root_durability(&self) -> Durability {
        self.root_durability
    }

    pub fn confirmation(&self) -> Confirmation {
        self.confirmation
    }
}

struct CarrierOwner {
    activity: StageLease,
    stage: Option<MediaStage>,
}

impl CarrierOwner {
    fn prepare(activity: StageLease) -> (Self, ActivityCarrierPreparationReceipt, io::Result<()>) {
        let mut owner = CarrierOwner {
            activity,
            stage: None,
        };
        let mut receipt = ActivityCarrierPreparationReceipt {
            stage_durability: Durability::Unproven,
            root_durability: Durability::Unproven,
            confirmation: Confirmation::Unverified,
        };
        let result = (|| {
            owner
                .activity
                .revalidate()
                .map_err(crate::activity::ActivityAdmissionError::into_io)?;
            let root = MediaDirectory::open(owner.activity.journal_root().clone(), Path::new(""))?;
            match MediaStage::create_with_lease(root, owner.activity.clone()) {
                Ok(stage) => owner.stage = Some(stage),
                Err(failure) => {
                    owner.stage = failure.owned;
                    return Err(failure.error);
                }
            }
            owner.revalidate_empty()?;
            if owner.flushes_directories() {
                step(MediaPrimitive::CarrierStageSync)?;
                owner.revalidate_empty()?;
                owner
                    .stage
                    .as_ref()
                    .and_then(|stage| stage.directory.as_ref())
                    .expect("validated directory retained")
                    .inner
                    .sync()?;
                receipt.stage_durability = Durability::Synced;
                owner.revalidate_empty()?;
                step(MediaPrimitive::CarrierRootSync)?;
                owner.revalidate_empty()?;
                owner
                    .stage
                    .as_ref()
                    .expect("validated stage retained")
                    .parent
                    .inner
                    .sync()?;
                receipt.root_durability = Durability::Synced;
            }
            step(MediaPrimitive::CarrierConfirmation)?;
            owner.revalidate_empty()?;
            receipt.confirmation = Confirmation::Confirmed;
            Ok(())
        })();
        (owner, receipt, result)
    }

    /// Windows has no directory flush. Activity and teardown carriers there
    /// are confirmed but durability stays `Unproven`.
    fn flushes_directories(&self) -> bool {
        cfg!(not(windows))
    }

    fn activity(&self) -> &Arc<ActivityLease> {
        match &self.activity {
            StageLease::Activity(lease) => lease,
            StageLease::Teardown(_) => {
                unreachable!("private activity wrapper retains activity role")
            }
        }
    }

    fn teardown(&self) -> &Arc<TeardownLease> {
        match &self.activity {
            StageLease::Teardown(lease) => lease,
            StageLease::Activity(_) => {
                unreachable!("private teardown wrapper retains teardown role")
            }
        }
    }

    fn revalidate_empty(&self) -> io::Result<()> {
        self.activity
            .revalidate()
            .map_err(crate::activity::ActivityAdmissionError::into_io)?;
        let stage = self.stage.as_ref().ok_or_else(changed)?;
        if stage.removed {
            return Err(changed());
        }
        stage.parent.revalidate()?;
        let directory = stage.directory.as_ref().ok_or_else(changed)?;
        directory.revalidate()?;
        if directory.entries(0)?.is_none() {
            return Err(invalid("activity reservation is not empty"));
        }
        stage.parent.revalidate()?;
        self.activity
            .revalidate()
            .map_err(crate::activity::ActivityAdmissionError::into_io)
    }

    fn retire(&mut self) -> ActivityCarrierRetirementReceipt {
        let mut receipt = ActivityCarrierRetirementReceipt {
            effect: ActivityCarrierRetirementEffect::NotAttempted,
            root_durability: Durability::Unproven,
            confirmation: Confirmation::Unverified,
            failures: Vec::new(),
        };
        // An uncertain creation (stage without an admitted directory) is held.
        if let Err(error) = self.revalidate_empty() {
            receipt.failures.push(error);
            return receipt;
        }
        let stage = self.stage.as_mut().expect("validated stage retained");
        receipt.effect = ActivityCarrierRetirementEffect::Attempted;
        if let Err(error) = stage.cleanup_exact() {
            receipt.failures.push(error);
            return receipt;
        }
        receipt.effect = ActivityCarrierRetirementEffect::Removed;
        if self.flushes_directories() {
            let stage = self.stage.as_mut().expect("validated stage retained");
            let flush = step(MediaPrimitive::CarrierRetirementRootSync)
                .and_then(|()| stage.parent.revalidate())
                .and_then(|()| stage.parent.inner.sync());
            match flush {
                Ok(()) => receipt.root_durability = Durability::Synced,
                Err(error) => receipt.failures.push(error),
            }
        }
        let stage = self.stage.as_mut().expect("validated stage retained");
        // A flush error does not erase an independently confirmable removal.
        let confirmation = (|| {
            step(MediaPrimitive::CarrierRetirementConfirmation)?;
            stage.revalidate_activity()?;
            stage.parent.revalidate()?;
            platform::ensure_absent(&stage.parent.inner, &stage.name)?;
            stage.parent.revalidate()?;
            stage.revalidate_activity()
        })();
        match confirmation {
            Ok(()) => receipt.confirmation = Confirmation::Confirmed,
            Err(error) => receipt.failures.push(error),
        }
        receipt
    }
}

/// An empty root reservation made durable before journal-bound remote effects.
/// Drop closes handles only. Discovered names and saved identities cannot mint one.
pub struct RootActivityCarrier {
    owner: CarrierOwner,
    preparation: ActivityCarrierPreparationReceipt,
}

impl RootActivityCarrier {
    /// Observes this actual empty reservation and persistent lock. The result is
    /// immutable data; it never certifies recovery, cooperation or settlement.
    pub fn observe_recovery_binding(
        &self,
        origin: &crate::archive_origin::LocalArchiveOrigin,
        repository_id: &str,
    ) -> Result<
        crate::archive_recovery::ArchiveRecoveryBinding,
        crate::archive_recovery::RecoveryBindingObservationError,
    > {
        use crate::archive_recovery::RecoveryBindingObservationError as E;
        #[cfg(target_os = "linux")]
        {
            if !Arc::ptr_eq(origin.root(), self.journal_root()) {
                return Err(E::Native(invalid(
                    "origin belongs to another original root",
                )));
            }
            origin.revalidate().map_err(E::Origin)?;
            self.activity().revalidate().map_err(E::Activity)?;
            let matched = self
                .observation()
                .validate(self.journal_root())
                .map_err(E::Native)?;
            let lock = self
                .activity()
                .recovery_lock_observation()
                .map_err(E::Activity)?;
            let name = matched
                .name
                .to_str()
                .ok_or_else(|| E::Native(invalid("invalid reservation name")))?;
            let binding = crate::archive_recovery::ArchiveRecoveryBinding::observed(
                origin.evidence().clone(),
                name.into(),
                matched.identity,
                lock.clone(),
                repository_id,
            )
            .map_err(E::Native)?;
            origin.revalidate().map_err(E::Origin)?;
            if self
                .observation()
                .validate(self.journal_root())
                .map_err(E::Native)?
                != matched
                || self
                    .activity()
                    .recovery_lock_observation()
                    .map_err(E::Activity)?
                    != lock
            {
                return Err(E::Native(changed()));
            }
            self.activity().revalidate().map_err(E::Activity)?;
            Ok(binding)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (origin, repository_id);
            Err(E::Unsupported)
        }
    }

    /// Caller must freshly refuse existing unresolved obligations under this
    /// original activity lease before preparing a new operation. This native
    /// leaf cannot certify the owning catalog/intent/config admission.
    pub fn prepare(
        activity: Arc<ActivityLease>,
    ) -> Result<Self, ActivityCarrierPreparationFailure> {
        let (owner, receipt, result) = CarrierOwner::prepare(StageLease::Activity(activity));
        match result {
            Ok(()) => Ok(Self {
                owner,
                preparation: receipt,
            }),
            Err(error) => Err(ActivityCarrierPreparationFailure {
                error,
                receipt,
                owner,
            }),
        }
    }

    pub fn activity(&self) -> &Arc<ActivityLease> {
        self.owner.activity()
    }

    pub fn journal_root(&self) -> &Arc<JournalRoot> {
        self.owner.activity.journal_root()
    }

    pub fn preparation(&self) -> &ActivityCarrierPreparationReceipt {
        &self.preparation
    }

    pub fn diagnostic_path(&self) -> PathBuf {
        self.owner
            .stage
            .as_ref()
            .expect("prepared stage")
            .diagnostic_path()
    }

    pub fn revalidate(&self) -> io::Result<()> {
        self.owner.revalidate_empty()
    }

    pub fn observation(&self) -> RootCarrierObservation<'_> {
        RootCarrierObservation {
            owner: ObservationOwner::Live(&self.owner),
        }
    }

    /// Requires actual settlement of every child and HTTP writer and qualified
    /// cooperation/quiescence of every stage-namespace actor. Unknown holds this
    /// call. No native receipt proves those owning-runtime preconditions.
    ///
    /// Unix final check-to-rmdir can remove an uncooperating empty replacement;
    /// this method is not unconditional protection against that namespace race.
    pub fn retire_after_settlement(mut self) -> ActivityCarrierRetirement<Self> {
        let receipt = self.owner.retire();
        ActivityCarrierRetirement {
            owned: self,
            receipt,
        }
    }
}

/// Preparation failure retains partial reservations and the original lease.
pub struct ActivityCarrierPreparationFailure {
    error: io::Error,
    receipt: ActivityCarrierPreparationReceipt,
    owner: CarrierOwner,
}

impl ActivityCarrierPreparationFailure {
    pub fn error(&self) -> &io::Error {
        &self.error
    }

    pub fn receipt(&self) -> &ActivityCarrierPreparationReceipt {
        &self.receipt
    }

    pub fn activity(&self) -> &Arc<ActivityLease> {
        self.owner.activity()
    }

    pub fn diagnostic_path(&self) -> Option<PathBuf> {
        self.owner.stage.as_ref().map(MediaStage::diagnostic_path)
    }

    /// Requires the same actual settlement and qualified namespace cooperation
    /// as RootActivityCarrier::retire_after_settlement, including prelaunch
    /// failures. Unknown cooperation holds cleanup; Drop never unlinks.
    pub fn retire_after_settlement(mut self) -> ActivityCarrierRetirement<Self> {
        let receipt = self.owner.retire();
        ActivityCarrierRetirement {
            owned: self,
            receipt,
        }
    }
}

impl std::fmt::Debug for ActivityCarrierPreparationFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ActivityCarrierPreparationFailure")
            .field("error", &self.error)
            .field("receipt", &self.receipt)
            .finish_non_exhaustive()
    }
}

/// Empty durable root reservation retaining the original teardown admission.
/// It exposes no writable media stage. Drop closes only; it never retires.
pub struct RootTeardownCarrier {
    owner: CarrierOwner,
    preparation: ActivityCarrierPreparationReceipt,
}

impl RootTeardownCarrier {
    /// Existing obligations must be refused freshly under this original lease.
    /// This leaf certifies no owning config, catalog or settlement policy.
    pub fn prepare(
        teardown: Arc<TeardownLease>,
    ) -> Result<Self, TeardownCarrierPreparationFailure> {
        let (owner, receipt, result) = CarrierOwner::prepare(StageLease::Teardown(teardown));
        match result {
            Ok(()) => Ok(Self {
                owner,
                preparation: receipt,
            }),
            Err(error) => Err(TeardownCarrierPreparationFailure {
                error,
                receipt,
                owner,
            }),
        }
    }

    /// Persistable observations of this actual empty reservation. They grant no
    /// recovery authority; a later invocation must acquire its own real lease.
    pub fn recovery_observation(&self) -> io::Result<StageObservation> {
        self.owner.revalidate_empty()?;
        let observed = self
            .owner
            .stage
            .as_ref()
            .ok_or_else(changed)?
            .observation()?;
        self.owner.revalidate_empty()?;
        Ok(observed)
    }

    /// Reacquire only the exact original empty root child under a newly acquired
    /// teardown lease. Never creates a stage or sweeps another reservation.
    /// The caller still owns remote settlement and namespace cooperation.
    pub fn reacquire(
        teardown: Arc<TeardownLease>,
        expected: &StageObservation,
    ) -> io::Result<Self> {
        teardown
            .revalidate()
            .map_err(crate::activity::ActivityAdmissionError::into_io)?;
        let root = teardown.journal_root();
        let suffix = expected
            .directory_name
            .strip_prefix(".tmp-offload-")
            .ok_or_else(changed)?;
        if !expected.parent_relative.is_empty()
            || expected.journal_observation != root.identity().observation_id()
            || suffix.len() != 32
            || !suffix
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(changed());
        }
        let parent = MediaDirectory::open(root.clone(), Path::new(""))?;
        if parent.identity() != expected.parent_identity {
            return Err(changed());
        }
        let name = OsString::from(&expected.directory_name);
        let directory = parent.open_child(&name)?.ok_or_else(changed)?;
        if directory.identity() != expected.directory_identity {
            return Err(changed());
        }
        let stage = MediaStage {
            parent,
            activity: StageLease::Teardown(teardown.clone()),
            name,
            directory: Some(directory),
            leaf: None,
            removed: false,
            #[cfg(windows)]
            published: false,
            #[cfg(windows)]
            payload_delete_pending: None,
            #[cfg(windows)]
            directory_delete_pending: false,
        };
        let owner = CarrierOwner {
            activity: StageLease::Teardown(teardown),
            stage: Some(stage),
        };
        owner.revalidate_empty()?;
        if owner.stage.as_ref().ok_or_else(changed)?.observation()? != *expected {
            return Err(changed());
        }
        let mut preparation = ActivityCarrierPreparationReceipt {
            stage_durability: Durability::Unproven,
            root_durability: Durability::Unproven,
            confirmation: Confirmation::Unverified,
        };
        if owner.flushes_directories() {
            let stage = owner.stage.as_ref().ok_or_else(changed)?;
            stage.directory.as_ref().ok_or_else(changed)?.inner.sync()?;
            owner.revalidate_empty()?;
            stage.parent.inner.sync()?;
            preparation.stage_durability = Durability::Synced;
            preparation.root_durability = Durability::Synced;
        }
        owner.revalidate_empty()?;
        preparation.confirmation = Confirmation::Confirmed;
        Ok(Self { owner, preparation })
    }

    pub fn teardown(&self) -> &Arc<TeardownLease> {
        self.owner.teardown()
    }
    pub fn journal_root(&self) -> &Arc<JournalRoot> {
        self.owner.activity.journal_root()
    }
    pub fn preparation(&self) -> &ActivityCarrierPreparationReceipt {
        &self.preparation
    }
    pub fn diagnostic_path(&self) -> PathBuf {
        self.owner
            .stage
            .as_ref()
            .expect("prepared stage")
            .diagnostic_path()
    }
    pub fn revalidate(&self) -> io::Result<()> {
        self.owner.revalidate_empty()
    }
    pub fn observation(&self) -> RootCarrierObservation<'_> {
        RootCarrierObservation {
            owner: ObservationOwner::Live(&self.owner),
        }
    }

    /// Requires actual child/HTTP settlement and qualified namespace cooperation,
    /// including the documented Unix final check-to-rmdir limitation. Unknown
    /// cooperation holds this call; the receipt cannot establish it.
    pub fn retire_after_settlement(mut self) -> ActivityCarrierRetirement<Self> {
        let receipt = self.owner.retire();
        ActivityCarrierRetirement {
            owned: self,
            receipt,
        }
    }
}

/// Retains original teardown ownership and every partial reservation on failure.
pub struct TeardownCarrierPreparationFailure {
    error: io::Error,
    receipt: ActivityCarrierPreparationReceipt,
    owner: CarrierOwner,
}

impl TeardownCarrierPreparationFailure {
    pub fn error(&self) -> &io::Error {
        &self.error
    }
    pub fn receipt(&self) -> &ActivityCarrierPreparationReceipt {
        &self.receipt
    }
    pub fn teardown(&self) -> &Arc<TeardownLease> {
        self.owner.teardown()
    }
    pub fn journal_root(&self) -> &Arc<JournalRoot> {
        self.owner.activity.journal_root()
    }
    pub fn diagnostic_path(&self) -> Option<PathBuf> {
        self.owner.stage.as_ref().map(MediaStage::diagnostic_path)
    }

    /// Same actual settlement/cooperation preconditions as the prepared carrier,
    /// even for prelaunch failure. Unknown holds; Drop does not unlink.
    pub fn retire_after_settlement(mut self) -> ActivityCarrierRetirement<Self> {
        let receipt = self.owner.retire();
        ActivityCarrierRetirement {
            owned: self,
            receipt,
        }
    }
}

impl std::fmt::Debug for TeardownCarrierPreparationFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TeardownCarrierPreparationFailure")
            .field("error", &self.error)
            .field("receipt", &self.receipt)
            .finish_non_exhaustive()
    }
}

/// Borrow of one actual prepared root carrier. No saved name or identity can
/// construct it; the owner and original role remain borrowed throughout census.
pub struct RootCarrierObservation<'a> {
    owner: ObservationOwner<'a>,
}

enum ObservationOwner<'a> {
    Live(&'a CarrierOwner),
    Recovered(&'a crate::archive_recovery::RecoveredArchiveReservation),
}

#[derive(Eq, PartialEq)]
pub(crate) struct CarrierMatch {
    pub(crate) name: OsString,
    pub(crate) identity: MediaIdentity,
}

impl RootCarrierObservation<'_> {
    pub(crate) fn recovered(
        owner: &crate::archive_recovery::RecoveredArchiveReservation,
    ) -> RootCarrierObservation<'_> {
        RootCarrierObservation {
            owner: ObservationOwner::Recovered(owner),
        }
    }

    pub(crate) fn validate(&self, root: &Arc<JournalRoot>) -> io::Result<CarrierMatch> {
        let owner = match self.owner {
            ObservationOwner::Recovered(owner) => {
                let (name, identity) = owner.match_root(root)?;
                return Ok(CarrierMatch { name, identity });
            }
            ObservationOwner::Live(owner) => owner,
        };
        if !Arc::ptr_eq(root, owner.activity.journal_root()) {
            return Err(invalid("carrier belongs to another original root"));
        }
        owner.revalidate_empty()?;
        let stage = owner.stage.as_ref().ok_or_else(changed)?;
        if stage.parent.object_identity() != root.identity() {
            return Err(invalid("carrier is not a root reservation"));
        }
        let matched = CarrierMatch {
            name: stage.name.clone(),
            identity: stage.directory.as_ref().ok_or_else(changed)?.identity(),
        };
        owner.revalidate_empty()?;
        Ok(matched)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActivityCarrierRetirementEffect {
    NotAttempted,
    Attempted,
    /// Removal returned success under the owning cooperation precondition.
    /// This fact does not establish that precondition or settle any writer.
    Removed,
}

/// Actual removal, durability and confirmation remain separate facts.
#[derive(Debug)]
pub struct ActivityCarrierRetirementReceipt {
    effect: ActivityCarrierRetirementEffect,
    root_durability: Durability,
    confirmation: Confirmation,
    failures: Vec<io::Error>,
}

impl ActivityCarrierRetirementReceipt {
    pub(crate) fn not_attempted() -> Self {
        Self {
            effect: ActivityCarrierRetirementEffect::NotAttempted,
            root_durability: Durability::Unproven,
            confirmation: Confirmation::Unverified,
            failures: Vec::new(),
        }
    }

    pub fn effect(&self) -> ActivityCarrierRetirementEffect {
        self.effect
    }

    pub fn root_durability(&self) -> Durability {
        self.root_durability
    }

    pub fn confirmation(&self) -> Confirmation {
        self.confirmation
    }

    pub fn failures(&self) -> &[io::Error] {
        &self.failures
    }
}

// Finite adapter for the already physically retained recovered owner. No saved
// name/identity or public MediaDirectory method can enter this mutation.
#[cfg(target_os = "linux")]
pub(crate) fn retire_recovered(
    owner: &crate::archive_recovery::RecoveredArchiveReservation,
) -> ActivityCarrierRetirementReceipt {
    let mut receipt = ActivityCarrierRetirementReceipt::not_attempted();
    if let Err(error) = owner.revalidate() {
        receipt.failures.push(io::Error::other(error));
        return receipt;
    }
    let (parent, directory, name) = owner.retirement_directories();
    receipt.effect = ActivityCarrierRetirementEffect::Attempted;
    if let Err(error) = platform::remove_directory(&parent.inner, name, &directory.inner) {
        receipt.failures.push(error);
        return receipt;
    }
    receipt.effect = ActivityCarrierRetirementEffect::Removed;
    let flush = (|| {
        step(MediaPrimitive::CarrierRetirementRootSync)?;
        owner.revalidate_after_removal().map_err(io::Error::other)?;
        parent.inner.sync()
    })();
    match flush {
        Ok(()) => receipt.root_durability = Durability::Synced,
        Err(error) => receipt.failures.push(error),
    }
    let confirmation = (|| {
        step(MediaPrimitive::CarrierRetirementConfirmation)?;
        owner.revalidate_after_removal().map_err(io::Error::other)?;
        platform::ensure_absent(&parent.inner, name)?;
        owner.revalidate_after_removal().map_err(io::Error::other)
    })();
    match confirmation {
        Ok(()) => receipt.confirmation = Confirmation::Confirmed,
        Err(error) => receipt.failures.push(error),
    }
    receipt
}

/// Retire empty root reservations left by owners that no longer exist, such as
/// after a crash or restart. Call only with a freshly acquired lease, before
/// creating anything: holding it excludes every cooperating owner, but only on
/// local storage, so other filesystems retire nothing. Any restore intent holds
/// every reservation, since an interrupted restore may name one. Removal is
/// rmdir, so a reservation holding anything stays an unresolved obligation.
pub fn retire_orphan_root_carriers(activity: &Arc<ActivityLease>) -> io::Result<usize> {
    retire_orphans_under(&StageLease::Activity(Arc::clone(activity)))
}

/// The same retirement under a teardown's own lease, before it observes
/// anything, so a teardown that ended mid-run doesn't hold the next one.
pub fn retire_orphan_root_carriers_for_teardown(lease: &Arc<TeardownLease>) -> io::Result<usize> {
    retire_orphans_under(&StageLease::Teardown(Arc::clone(lease)))
}

fn retire_orphans_under(lease: &StageLease) -> io::Result<usize> {
    let mut retired = 0;
    match retire_orphans(lease, &mut retired) {
        Ok(()) => Ok(retired),
        // Desktop observation failure still retires nothing and reports the
        // count so far. A phone container that is not qualified must not look
        // like that empty success.
        #[cfg(any(target_os = "ios", target_os = "android"))]
        Err(error) => Err(error),
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        Err(_) => Ok(retired),
    }
}

fn retire_orphans(lease: &StageLease, retired: &mut usize) -> io::Result<()> {
    use crate::activity_obligations::{NativeObligation, observe_obligations};
    use crate::journal_root::JournalEntryKind;

    let revalidate = || {
        lease
            .revalidate()
            .map_err(crate::activity::ActivityAdmissionError::into_io)
    };
    revalidate()?;
    if !exclusive_local(lease.journal_root(), &revalidate)? {
        return Ok(());
    }
    let root = MediaDirectory::open(lease.journal_root().clone(), Path::new(""))?;
    let census = observe_obligations(lease.journal_root()).map_err(io::Error::other)?;
    let entries = census.entries();
    if entries
        .iter()
        .any(|entry| matches!(entry, NativeObligation::Intent { .. }))
    {
        return Ok(());
    }
    for obligation in entries {
        let NativeObligation::Stage { relative, entry } = obligation else {
            continue;
        };
        if relative != Path::new(&entry.name) || entry.kind != JournalEntryKind::Directory {
            continue;
        }
        revalidate()?;
        if matches!(
            platform::remove_empty_child(&root.inner, &entry.name, entry.identity),
            Ok(true)
        ) {
            *retired += 1;
        }
    }
    // Windows has no directory flush; the removal itself was confirmed.
    #[cfg(not(windows))]
    if *retired > 0 {
        root.sync()?;
    }
    Ok(())
}

/// Whether holding this lease excludes every writer of its journal: Linux and
/// macOS on local storage only, Windows on an admitted fixed volume, and a
/// qualified private app container on Android and iOS. An unqualified phone
/// container is `Err`, never a successful `Ok(false)`.
pub fn exclusive_local_activity(activity: &Arc<ActivityLease>) -> io::Result<bool> {
    exclusive_local(activity.journal_root(), &|| {
        activity
            .revalidate()
            .map_err(crate::activity::ActivityAdmissionError::into_io)
    })
}

fn exclusive_local(
    journal_root: &Arc<JournalRoot>,
    revalidate: &dyn Fn() -> io::Result<()>,
) -> io::Result<bool> {
    #[cfg(target_os = "linux")]
    {
        revalidate()?;
        let root = MediaDirectory::open(journal_root.clone(), Path::new(""))?;
        Ok(local_storage(
            nix::sys::statfs::fstatfs(&root.inner)?.filesystem_type(),
        ))
    }
    // APFS and HFS+ are the local macOS filesystems; network and FUSE mounts
    // (smbfs, nfs, afpfs, webdav, macfuse) can hold another host's writers.
    #[cfg(target_os = "macos")]
    {
        revalidate()?;
        // A synced folder is APFS here, but another Mac may write it too.
        let canonical = journal_root.canonical_path().to_string_lossy();
        if canonical.contains("/Library/CloudStorage/")
            || canonical.contains("/Library/Mobile Documents/")
        {
            return Ok(false);
        }
        let root = MediaDirectory::open(journal_root.clone(), Path::new(""))?;
        let stat = nix::sys::statfs::fstatfs(&root.inner)?;
        Ok(matches!(stat.filesystem_type_name(), "apfs" | "hfs"))
    }
    #[cfg(windows)]
    {
        revalidate()?;
        journal_root.revalidate().map_err(io::Error::other)?;
        Ok(true)
    }
    #[cfg(target_os = "android")]
    {
        revalidate()?;
        journal_root
            .revalidate_canonical_binding()
            .map_err(io::Error::other)?;
        let root = MediaDirectory::open(journal_root.clone(), Path::new(""))?;
        let magic = nix::sys::statfs::fstatfs(&root.inner)?.filesystem_type().0 as u64;
        let process = android_process_name()?;
        let canonical = journal_root.canonical_path().to_str().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "journal path is not unicode")
        })?;
        if qualify_android_container(magic, canonical, &process) {
            Ok(true)
        } else {
            Err(shared_storage())
        }
    }
    #[cfg(target_os = "ios")]
    {
        revalidate()?;
        journal_root
            .revalidate_canonical_binding()
            .map_err(io::Error::other)?;
        let canonical = journal_root.canonical_path().to_str().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "journal path is not unicode")
        })?;
        let root = MediaDirectory::open(journal_root.clone(), Path::new(""))?;
        let stat = nix::sys::statfs::fstatfs(&root.inner)?;
        let name = stat.filesystem_type_name();
        if qualify_ios_container(name, canonical) {
            Ok(true)
        } else {
            Err(shared_storage())
        }
    }
    #[cfg(not(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "ios",
        target_os = "android",
        windows
    )))]
    {
        let _ = (journal_root, revalidate);
        Ok(false)
    }
}

/// Android private-app-container check. `magic` is the statfs type. `process`
/// is the name from `/proc/self/cmdline`, never an environment variable.
/// ext4 and f2fs under this process's `/data/data` or `/data/user/<id>` tree
/// qualify. Shared, fused, and other-package paths do not.
pub fn qualify_android_container(magic: u64, canonical_path: &str, process: &str) -> bool {
    const EXT4: u64 = 0xEF53;
    const F2FS: u64 = 0xF2F5_2010;
    if magic != EXT4 && magic != F2FS {
        return false;
    }
    let Some(process) = android_process_label(process) else {
        return false;
    };
    android_private_path(canonical_path, process)
}

fn android_process_label(process: &str) -> Option<&str> {
    let process = process.split('\0').next().unwrap_or("");
    let process = process.rsplit('/').next().unwrap_or("");
    if process.is_empty()
        || process.contains('/')
        || process.contains('\0')
        || process == "."
        || process == ".."
    {
        None
    } else {
        Some(process)
    }
}

fn android_private_path(path: &str, process: &str) -> bool {
    if path.contains('\0') || path.split('/').any(|part| part == "." || part == "..") {
        return false;
    }
    let rest = if let Some(rest) = path.strip_prefix("/data/data/") {
        rest
    } else if let Some(rest) = path.strip_prefix("/data/user/") {
        let Some((user, rest)) = rest.split_once('/') else {
            return false;
        };
        if user.is_empty() || !user.bytes().all(|byte| byte.is_ascii_digit()) {
            return false;
        }
        rest
    } else {
        return false;
    };
    let (owner, tail) = rest.split_once('/').unwrap_or((rest, ""));
    owner == process && !owner.is_empty() && !tail.starts_with('/')
}

/// iOS private-app-container check. Only APFS under an application, plugin, or
/// app-group container qualifies, and a cloud-synced path never does.
pub fn qualify_ios_container(fs_type_name: &str, canonical_path: &str) -> bool {
    if fs_type_name != "apfs" {
        return false;
    }
    if canonical_path.contains("Mobile Documents") || canonical_path.contains("CloudStorage") {
        return false;
    }
    if canonical_path
        .split('/')
        .any(|part| part == "." || part == "..")
    {
        return false;
    }
    const ROOTS: [&str; 3] = [
        "/Containers/Data/Application/",
        "/Containers/Data/PluginKitPlugin/",
        "/Containers/Shared/AppGroup/",
    ];
    ROOTS.iter().any(|root| {
        let Some(index) = canonical_path.find(root) else {
            return false;
        };
        let after = &canonical_path[index + root.len()..];
        let Some(identifier) = after.split('/').next() else {
            return false;
        };
        !identifier.is_empty()
    })
}

#[cfg(target_os = "android")]
pub(crate) fn android_process_name() -> io::Result<String> {
    let bytes = std::fs::read("/proc/self/cmdline")?;
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    let text = std::str::from_utf8(&bytes[..end])
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    android_process_label(text)
        .map(str::to_owned)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "android process name is empty"))
}

/// Remove one recorded stage whose owner ended: `name` under `parent`, holding
/// nothing but `leaf_name` with exactly the recorded identity. Its caller must
/// know that nothing was published from it. Returns false when it is already
/// absent; anything unexpected refuses and leaves the stage in place.
pub fn retire_recorded_stage(
    activity: &Arc<ActivityLease>,
    parent: &Arc<MediaDirectory>,
    name: &OsStr,
    directory: MediaIdentity,
    leaf_name: &OsStr,
    leaf: MediaIdentity,
) -> io::Result<bool> {
    retire_stage(
        activity,
        parent,
        name,
        directory,
        Leaf::Exact(leaf_name, Some(leaf)),
    )
}

/// Remove one stage nothing records, which is empty or holds exactly one
/// regular file named `leaf_name`: the observed file, then the directory. Its
/// caller must know that name is made only by a producer whose unpublished
/// copies are disposable. Anything else refuses.
pub fn retire_unrecorded_stage(
    activity: &Arc<ActivityLease>,
    parent: &Arc<MediaDirectory>,
    name: &OsStr,
    directory: MediaIdentity,
    leaf_name: &OsStr,
) -> io::Result<bool> {
    retire_stage(
        activity,
        parent,
        name,
        directory,
        Leaf::Exact(leaf_name, None),
    )
}

/// Remove one stage nothing records, which is empty or holds exactly one
/// regular file whose name `accept` allows. Its caller must know that every
/// such stage there is made by a producer whose unpublished copies are
/// disposable. Anything else refuses.
pub fn retire_unrecorded_stage_where(
    activity: &Arc<ActivityLease>,
    parent: &Arc<MediaDirectory>,
    name: &OsStr,
    directory: MediaIdentity,
    accept: &dyn Fn(&OsStr) -> bool,
) -> io::Result<bool> {
    retire_stage(activity, parent, name, directory, Leaf::Accepted(accept))
}

enum Leaf<'a> {
    #[cfg_attr(
        not(any(
            target_os = "linux",
            target_os = "macos",
            target_os = "ios",
            target_os = "android",
            windows
        )),
        allow(dead_code)
    )]
    Exact(&'a OsStr, Option<MediaIdentity>),
    #[cfg_attr(
        not(any(
            target_os = "linux",
            target_os = "macos",
            target_os = "ios",
            target_os = "android",
            windows
        )),
        allow(dead_code)
    )]
    Accepted(&'a dyn Fn(&OsStr) -> bool),
}

fn retire_stage(
    activity: &Arc<ActivityLease>,
    parent: &Arc<MediaDirectory>,
    name: &OsStr,
    directory: MediaIdentity,
    leaf: Leaf<'_>,
) -> io::Result<bool> {
    #[cfg(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "ios",
        target_os = "android",
        windows
    ))]
    {
        plain_name(name)?;
        if let Leaf::Exact(leaf_name, _) = leaf {
            plain_name(leaf_name)?;
        }
        if !crate::activity_obligations::is_stage_namespace_name(name)
            || !Arc::ptr_eq(parent.inner.journal_root(), activity.journal_root())
        {
            return Err(invalid("not a stage of this journal"));
        }
        if !exclusive_local_activity(activity)? {
            return Err(shared_storage());
        }
        let Some(stage) = platform::open_removal_directory(&parent.inner, name)? else {
            return Ok(false);
        };
        if stage.identity() != directory {
            return Err(changed());
        }
        let entries = platform::entries(&stage, 2)?.ok_or_else(changed)?;
        match entries.as_slice() {
            // An empty directory holds no data.
            [] => {}
            [entry]
                if entry.kind == crate::journal_root::JournalEntryKind::RegularFile
                    && match leaf {
                        Leaf::Exact(leaf_name, recorded) => {
                            entry.name == leaf_name
                                && recorded.is_none_or(|leaf| entry.identity == leaf)
                        }
                        Leaf::Accepted(accept) => {
                            plain_name(&entry.name).is_ok() && accept(&entry.name)
                        }
                    } =>
            {
                let leaf_name = entry.name.as_os_str();
                let leaf = entry.identity;
                activity
                    .revalidate()
                    .map_err(crate::activity::ActivityAdmissionError::into_io)?;
                let file = platform::open_removal(&stage, leaf_name)?.ok_or_else(changed)?;
                if platform::identity(&file)? != leaf {
                    return Err(changed());
                }
                platform::check_leaf(&stage, leaf_name, leaf)?;
                platform::remove_leaf(&stage, leaf_name, &file)?;
                drop(file);
                platform::ensure_absent(&stage, leaf_name)?;
            }
            _ => return Err(changed()),
        }
        activity
            .revalidate()
            .map_err(crate::activity::ActivityAdmissionError::into_io)?;
        platform::remove_directory(&parent.inner, name, &stage)?;
        drop(stage);
        platform::ensure_absent(&parent.inner, name)?;
        if cfg!(not(windows)) {
            parent.sync()?;
        }
        Ok(true)
    }
    #[cfg(not(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "ios",
        target_os = "android",
        windows
    )))]
    {
        let _ = (activity, parent, name, directory, leaf);
        Err(shared_storage())
    }
}

pub(super) fn shared_storage() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "the activity lease does not exclude every writer here",
    )
}

/// Filesystems where this host's lock excludes every writer. Anything else,
/// including network, FUSE and virtual-machine shares, is refused.
#[cfg(target_os = "linux")]
fn local_storage(kind: nix::sys::statfs::FsType) -> bool {
    use nix::sys::statfs::{
        BTRFS_SUPER_MAGIC, EXT4_SUPER_MAGIC, F2FS_SUPER_MAGIC, FsType, OVERLAYFS_SUPER_MAGIC,
        TMPFS_MAGIC,
    };
    // nix gates XFS_SUPER_MAGIC out on musl, which the release builds use.
    const XFS_SUPER_MAGIC: FsType = FsType(0x5846_5342);
    const ZFS_SUPER_MAGIC: FsType = FsType(0x2fc1_2fc1);
    [
        EXT4_SUPER_MAGIC,
        XFS_SUPER_MAGIC,
        BTRFS_SUPER_MAGIC,
        F2FS_SUPER_MAGIC,
        ZFS_SUPER_MAGIC,
        TMPFS_MAGIC,
        OVERLAYFS_SUPER_MAGIC,
    ]
    .contains(&kind)
}

/// Keeps the original capability and lease even after a known removal.
#[must_use = "Handle retirement facts and retained ownership independently"]
pub struct ActivityCarrierRetirement<T> {
    pub owned: T,
    pub receipt: ActivityCarrierRetirementReceipt,
}

#[cfg(test)]
mod qualification {
    use super::{qualify_android_container, qualify_ios_container};

    const EXT4: u64 = 0xEF53;
    const F2FS: u64 = 0xF2F5_2010;
    const PROCESS: &str = "journal.app";

    #[test]
    fn android_private_ext4_and_f2fs_qualify() {
        assert!(qualify_android_container(
            EXT4,
            "/data/data/journal.app/files/journal",
            PROCESS
        ));
        assert!(qualify_android_container(
            F2FS,
            "/data/user/0/journal.app/files/journal",
            PROCESS
        ));
        assert!(qualify_android_container(
            EXT4,
            "/data/user/10/journal.app",
            PROCESS
        ));
    }

    #[test]
    fn android_shared_wrong_package_and_other_magic_refuse() {
        assert!(!qualify_android_container(
            EXT4,
            "/data/data/other.app/files/journal",
            PROCESS
        ));
        assert!(!qualify_android_container(
            EXT4,
            "/storage/emulated/0/journal",
            PROCESS
        ));
        assert!(!qualify_android_container(EXT4, "/sdcard/journal", PROCESS));
        assert!(!qualify_android_container(
            EXT4,
            "/data/user/0/journal.app.extra/files",
            PROCESS
        ));
        assert!(!qualify_android_container(
            0x0102_1994,
            "/data/data/journal.app/files",
            PROCESS
        ));
        assert!(!qualify_android_container(
            0x0102_1997,
            "/data/data/journal.app/files",
            PROCESS
        ));
        assert!(!qualify_android_container(
            EXT4,
            "/data/data/journal.app/files",
            ""
        ));
        assert!(!qualify_android_container(
            EXT4,
            "/data/user/u0/journal.app/files",
            PROCESS
        ));
        assert!(!qualify_android_container(
            EXT4,
            "/data/data/journal.app/../other.app/files",
            PROCESS
        ));
    }

    #[test]
    fn ios_apfs_app_containers_qualify() {
        for path in [
            "/private/var/mobile/Containers/Data/Application/8C5E/Documents/journal",
            "/var/mobile/Containers/Data/PluginKitPlugin/8C5E/journal",
            "/private/var/mobile/Containers/Shared/AppGroup/8C5E/journal",
        ] {
            assert!(qualify_ios_container("apfs", path), "{path}");
        }
    }

    #[test]
    fn ios_cloud_non_apfs_and_outside_paths_refuse() {
        let app =
            "/private/var/mobile/Containers/Data/Application/8C5E/Library/Mobile Documents/journal";
        assert!(!qualify_ios_container("apfs", app));
        assert!(!qualify_ios_container(
            "apfs",
            "/private/var/mobile/Library/CloudStorage/journal"
        ));
        assert!(!qualify_ios_container(
            "hfs",
            "/private/var/mobile/Containers/Data/Application/8C5E/journal"
        ));
        assert!(!qualify_ios_container(
            "apfs",
            "/var/mobile/Containers/Data/Application/"
        ));
        assert!(!qualify_ios_container("apfs", "/tmp/journal"));
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod tests;

#[cfg(all(test, target_os = "linux"))]
mod storage_tests {
    use nix::sys::statfs::{
        EXT4_SUPER_MAGIC, FUSE_SUPER_MAGIC, FsType, NFS_SUPER_MAGIC, SMB_SUPER_MAGIC, TMPFS_MAGIC,
        XFS_SUPER_MAGIC,
    };

    #[test]
    fn shared_and_network_filesystems_refuse_orphan_retirement() {
        for local in [EXT4_SUPER_MAGIC, XFS_SUPER_MAGIC, TMPFS_MAGIC] {
            assert!(super::local_storage(local));
        }
        // CIFS, SMB2, 9p, Ceph and an unknown type; virtiofs reports FUSE.
        for shared in [
            NFS_SUPER_MAGIC,
            SMB_SUPER_MAGIC,
            FUSE_SUPER_MAGIC,
            FsType(0xff53_4d42),
            FsType(0xfe53_4d42),
            FsType(0x0102_1997),
            FsType(0x00c3_6400),
            FsType(0x1234_5678),
        ] {
            assert!(!super::local_storage(shared));
        }
    }
}
