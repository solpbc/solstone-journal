// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Private intent persistence and transaction-owned retirement. Serialized
//! evidence never becomes a native capability; Drop releases handles only.

use std::sync::Arc;

use solstone_core_journal_io::activity::ActivityLease;
use solstone_core_journal_io::media::{Durability, MediaRead};

use solstone_core_retention::restore_intent::{RepairHold, RestoreIntent};

mod adoption;
pub(crate) mod recovered;
pub(crate) mod transaction;

#[derive(Debug, PartialEq, Eq)]
enum StoreProblem {
    Unsupported,
    Busy,
    Changed,
    Invalid(RepairHold),
    Io(String),
}

fn io_problem(error: impl std::fmt::Display) -> StoreProblem {
    StoreProblem::Io(error.to_string())
}

#[derive(Debug)]
struct PreparationReceipt {
    // An attempted create/open may have created a directory even on failure.
    attempted: Vec<&'static str>,
    durability: [Durability; 4],
    confirmed: bool,
    failures: Vec<StoreProblem>,
}

impl Default for PreparationReceipt {
    fn default() -> Self {
        Self {
            attempted: Vec::new(),
            durability: [Durability::Unproven; 4],
            confirmed: false,
            failures: Vec::new(),
        }
    }
}

struct RestoreIntentStore {
    activity: Arc<ActivityLease>,
    anchor: Arc<()>,
    preparation: PreparationReceipt,
    #[cfg(any(unix, windows))]
    chain: Vec<native::Binding>,
    #[cfg(any(unix, windows))]
    lock: Option<solstone_core_journal_io::locking::BoundParentLock>,
}

struct StoreAdmissionFailure {
    // Includes every retained partial parent and the original activity lease.
    owner: Box<RestoreIntentStore>,
    problem: StoreProblem,
}

struct ObservedIntent {
    anchor: Arc<()>,
    #[cfg(any(unix, windows))]
    read: MediaRead,
    intent: RestoreIntent,
}

#[must_use = "retain the original owner and every independent publication fact"]
struct StoreWrite {
    owner: RestoreIntentStore,
    candidate: RestoreIntent,
    receipt: StoreWriteReceipt,
}

#[derive(Default)]
struct StoreWriteReceipt {
    #[cfg(any(unix, windows))]
    native: Option<native::NativePublication>,
    #[cfg(any(unix, windows))]
    confirmation: Option<MediaRead>,
    durability: [Option<Durability>; 4],
    failures: Vec<StoreProblem>,
}

impl RestoreIntentStore {
    fn unprepared(activity: Arc<ActivityLease>) -> Self {
        Self {
            activity,
            anchor: Arc::new(()),
            preparation: PreparationReceipt::default(),
            #[cfg(any(unix, windows))]
            chain: Vec::new(),
            #[cfg(any(unix, windows))]
            lock: None,
        }
    }

    fn create(activity: Arc<ActivityLease>) -> Result<Self, StoreAdmissionFailure> {
        let owner = Self::unprepared(activity);
        #[cfg(any(unix, windows))]
        let mut owner = owner;
        #[cfg(any(unix, windows))]
        let result = owner.prepare(&mut native::NativeHooks);
        #[cfg(not(any(unix, windows)))]
        let result = Err(StoreProblem::Unsupported);
        match result {
            Ok(()) => Ok(owner),
            Err(problem) => Err(StoreAdmissionFailure {
                owner: Box::new(owner),
                problem,
            }),
        }
    }

    fn open_existing(activity: Arc<ActivityLease>) -> Result<Self, StoreAdmissionFailure> {
        let owner = Self::unprepared(activity);
        #[cfg(any(unix, windows))]
        let mut owner = owner;
        #[cfg(any(unix, windows))]
        let result = owner.prepare_existing(&mut native::NativeHooks);
        #[cfg(not(any(unix, windows)))]
        let result = Err(StoreProblem::Unsupported);
        match result {
            Ok(()) => Ok(owner),
            Err(problem) => Err(StoreAdmissionFailure {
                owner: Box::new(owner),
                problem,
            }),
        }
    }

    pub(super) fn preparation_durability(&self) -> [Durability; 4] {
        self.preparation.durability
    }

    fn read(&self, operation: &str) -> Result<Option<ObservedIntent>, StoreProblem> {
        #[cfg(any(unix, windows))]
        {
            self.read_bound(operation)
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = operation;
            Err(StoreProblem::Unsupported)
        }
    }

    fn create_intent(self, candidate: RestoreIntent) -> StoreWrite {
        self.write(candidate, None)
    }

    fn update(self, observed: &ObservedIntent, candidate: RestoreIntent) -> StoreWrite {
        self.write(candidate, Some(observed))
    }

    fn write(self, candidate: RestoreIntent, expected: Option<&ObservedIntent>) -> StoreWrite {
        #[cfg(any(unix, windows))]
        {
            self.write_bound(candidate, expected, &mut native::NativeHooks)
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = expected;
            StoreWrite {
                owner: self,
                candidate,
                receipt: StoreWriteReceipt {
                    failures: vec![StoreProblem::Unsupported],
                    ..Default::default()
                },
            }
        }
    }
}

#[cfg(any(unix, windows))]
#[allow(clippy::indexing_slicing)] // Fixed three-parent chain, checked before use.
mod native {
    use super::*;
    use std::ffi::OsStr;
    #[cfg(unix)]
    use std::os::fd::AsFd;
    #[cfg(windows)]
    use std::os::windows::io::AsHandle;
    use std::path::Path;
    use std::time::Duration;

    #[cfg(windows)]
    use solstone_core_journal_io::atomic::{
        AtomicWriteOptions, DetailedAtomicOutcome, StageCleanup, atomic_replace_detailed,
        write_bytes_exclusive_detailed,
    };
    #[cfg(unix)]
    use solstone_core_journal_io::atomic::{
        BoundAtomicOutcome, atomic_replace_bound, write_bytes_exclusive_bound_detailed,
    };
    use solstone_core_journal_io::atomic::{
        DetailedAtomicError, ExclusivePublication, FinalNameConfirmation, MetadataDurability,
    };
    use solstone_core_journal_io::errors::ExistingParentLockError;
    #[cfg(unix)]
    use solstone_core_journal_io::flat_directory::{
        FlatDirectory, create_or_open_flat_directory_bound, open_flat_directory_bound,
    };
    use solstone_core_journal_io::locking::{
        acquire_existing_parent_lock_bound, open_existing_parent_lock_bound,
    };
    use solstone_core_journal_io::media::MediaDirectory;
    use solstone_core_journal_io::observation::FileObservation;
    #[cfg(windows)]
    use solstone_core_journal_io::windows_sync_dir::{
        WindowsFlatDirectory as FlatDirectory, create_or_open_windows_flat_directory_bound,
        open_windows_flat_directory_bound,
    };

    use solstone_core_journal_io::activity_obligations::{MAX_INTENT_BYTES, intent_filename};

    const LOCK: &str = "restore-intents.lock";
    const PARENTS: [&str; 3] = ["health", "health/offload", "health/offload/restore-intents"];

    pub(super) struct Binding {
        flat: FlatDirectory,
        pub(super) media: Option<Arc<MediaDirectory>>,
    }

    pub(super) enum NativePublication {
        Create(Result<ExclusivePublication, DetailedAtomicError>),
        #[cfg(unix)]
        Update(Result<BoundAtomicOutcome, DetailedAtomicError>),
        #[cfg(windows)]
        Update(Result<DetailedAtomicOutcome, DetailedAtomicError>),
    }

    impl NativePublication {
        fn observation(&self) -> Option<&FileObservation> {
            match self {
                Self::Create(Ok(ExclusivePublication {
                    final_name: FinalNameConfirmation::Confirmed { observation },
                    ..
                })) => Some(observation),
                #[cfg(unix)]
                Self::Update(Ok(
                    BoundAtomicOutcome::Published { observation }
                    | BoundAtomicOutcome::PublishedDurabilityUncertain { observation, .. }
                    | BoundAtomicOutcome::PublishedObservationUncertain { observation, .. },
                )) => Some(observation),
                _ => None,
            }
        }

        fn confirms(&self, read: &MediaRead, bytes: &[u8]) -> bool {
            #[cfg(windows)]
            if matches!(self, Self::Update(Ok(DetailedAtomicOutcome::Published))) {
                return read.bytes == bytes;
            }
            self.observation().is_some_and(|published| {
                read.bytes == bytes
                    && published.bytes == bytes
                    && read.entry.identity.matches_flat_entry(&published.entry)
                    && published.entry.size == read.entry.size
            })
        }

        fn fully_confirmed(&self) -> bool {
            match self {
                #[cfg(unix)]
                Self::Create(Ok(receipt)) => receipt.is_fully_confirmed(),
                #[cfg(windows)]
                Self::Create(Ok(receipt)) => {
                    matches!(receipt.cleanup, StageCleanup::Removed)
                        && matches!(receipt.final_name, FinalNameConfirmation::Confirmed { .. })
                        && matches!(
                            receipt.durability,
                            MetadataDurability::Unproven { source: None }
                        )
                }
                #[cfg(unix)]
                Self::Update(Ok(BoundAtomicOutcome::Published { .. })) => true,
                #[cfg(windows)]
                Self::Update(Ok(DetailedAtomicOutcome::Published)) => true,
                _ => false,
            }
        }

        fn directory_durability(&self) -> Durability {
            #[cfg(unix)]
            match self {
                Self::Create(Ok(receipt))
                    if matches!(receipt.durability, MetadataDurability::Confirmed) =>
                {
                    Durability::Synced
                }
                Self::Update(Ok(BoundAtomicOutcome::Published { .. })) => Durability::Synced,
                _ => Durability::Unproven,
            }
            #[cfg(windows)]
            {
                Durability::Unproven
            }
        }
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub(super) enum Point {
        BeforeWrite,
        AfterWrite,
        BeforeConfirmation,
    }

    pub(super) trait Hooks {
        fn checkpoint(&mut self, _point: Point) {}
        #[cfg(unix)]
        fn sync(&mut self, directory: &impl AsFd, _index: usize) -> std::io::Result<()> {
            nix::unistd::fsync(directory).map_err(std::io::Error::from)
        }
    }
    pub(super) struct NativeHooks;
    impl Hooks for NativeHooks {}

    fn name(operation: &str) -> Result<std::ffi::OsString, StoreProblem> {
        intent_filename(operation).ok_or(StoreProblem::Invalid(RepairHold::InvalidIntent))
    }

    #[derive(Clone, Copy)]
    enum StoreMode {
        Create,
        Existing,
    }

    #[cfg(unix)]
    fn open_parent(
        parent: &impl AsFd,
        leaf: &OsStr,
        diagnostic: &Path,
        mode: StoreMode,
    ) -> Result<FlatDirectory, StoreProblem> {
        match mode {
            StoreMode::Create => {
                create_or_open_flat_directory_bound(parent, leaf, 0o700, diagnostic)
                    .map_err(io_problem)
            }
            StoreMode::Existing => open_flat_directory_bound(parent, leaf, diagnostic)
                .map_err(io_problem)?
                .ok_or(StoreProblem::Changed),
        }
    }

    #[cfg(windows)]
    fn open_parent(
        parent: &impl AsHandle,
        leaf: &OsStr,
        diagnostic: &Path,
        mode: StoreMode,
    ) -> Result<FlatDirectory, StoreProblem> {
        match mode {
            StoreMode::Create => {
                create_or_open_windows_flat_directory_bound(parent, leaf, diagnostic)
                    .map_err(io_problem)
            }
            StoreMode::Existing => open_windows_flat_directory_bound(parent, leaf, diagnostic)
                .map_err(io_problem)?
                .ok_or(StoreProblem::Changed),
        }
    }

    fn flat_identity(
        flat: &FlatDirectory,
    ) -> solstone_core_journal_io::journal_root::ObjectIdentity {
        #[cfg(unix)]
        {
            flat.identity()
        }
        #[cfg(windows)]
        {
            flat.object_identity()
        }
    }

    impl RestoreIntentStore {
        pub(super) fn prepare(&mut self, hooks: &mut impl Hooks) -> Result<(), StoreProblem> {
            self.prepare_mode(hooks, StoreMode::Create)
        }

        pub(super) fn prepare_existing(
            &mut self,
            hooks: &mut impl Hooks,
        ) -> Result<(), StoreProblem> {
            self.prepare_mode(hooks, StoreMode::Existing)
        }

        fn prepare_mode(
            &mut self,
            hooks: &mut impl Hooks,
            mode: StoreMode,
        ) -> Result<(), StoreProblem> {
            self.activity.revalidate().map_err(io_problem)?;
            for (index, relative) in PARENTS.into_iter().enumerate() {
                self.activity.revalidate().map_err(io_problem)?;
                for binding in &self.chain {
                    binding
                        .media
                        .as_ref()
                        .ok_or(StoreProblem::Changed)?
                        .revalidate()
                        .map_err(io_problem)?;
                }
                self.preparation.attempted.push(relative);
                let leaf = Path::new(relative)
                    .file_name()
                    .ok_or(StoreProblem::Changed)?;
                let parent = Path::new(relative).parent().ok_or(StoreProblem::Changed)?;
                let diagnostic = self.activity.journal_root().canonical_path().join(parent);
                let flat = if index == 0 {
                    open_parent(
                        self.activity.journal_root().as_ref(),
                        leaf,
                        &diagnostic,
                        mode,
                    )
                } else {
                    open_parent(&self.chain[index - 1].flat, leaf, &diagnostic, mode)
                }?;
                // Preserve the just-opened parent even if its independent named
                // admission fails; the preparation result owns all prior effects.
                let media = match MediaDirectory::open(
                    Arc::clone(self.activity.journal_root()),
                    Path::new(relative),
                ) {
                    Ok(media) => media,
                    Err(error) => {
                        // Keep the actual opened descriptor in the failure carrier.
                        self.chain.push(Binding { flat, media: None });
                        return Err(io_problem(error));
                    }
                };
                let equal = flat_identity(&flat) == media.object_identity();
                self.chain.push(Binding {
                    flat,
                    media: Some(media),
                });
                if !equal {
                    return Err(StoreProblem::Changed);
                }
            }
            let lock = match mode {
                StoreMode::Create => acquire_existing_parent_lock_bound(
                    &self.chain[1].flat,
                    OsStr::new(LOCK),
                    Duration::ZERO,
                    Duration::ZERO,
                ),
                StoreMode::Existing => open_existing_parent_lock_bound(
                    &self.chain[1].flat,
                    OsStr::new(LOCK),
                    Duration::ZERO,
                    Duration::ZERO,
                )
                .and_then(|lock| {
                    lock.ok_or_else(|| ExistingParentLockError::NamespaceChanged {
                        path: self
                            .activity
                            .journal_root()
                            .canonical_path()
                            .join("health/offload")
                            .join(LOCK),
                    })
                }),
            }
            .map_err(|error| match error {
                ExistingParentLockError::Timeout(_) => StoreProblem::Busy,
                other => io_problem(other),
            })?;
            self.lock = Some(lock);
            // Preparation records independent failures rather than an all-or-none
            // boolean. A refused preparation never authorizes a publication.
            let (facts, failures) = self.sync_facts(hooks);
            self.preparation.durability = facts;
            self.preparation.failures = failures;
            match self.revalidate() {
                Ok(()) => self.preparation.confirmed = true,
                Err(error) => self.preparation.failures.push(error),
            }
            if !self.preparation.failures.is_empty() {
                return Err(StoreProblem::Changed);
            }
            Ok(())
        }

        pub(super) fn revalidate(&self) -> Result<(), StoreProblem> {
            self.activity.revalidate().map_err(io_problem)?;
            if self.chain.len() != 3 {
                return Err(StoreProblem::Changed);
            }
            for binding in &self.chain {
                #[cfg(unix)]
                binding.flat.revalidate().map_err(io_problem)?;
                #[cfg(windows)]
                binding.flat.revalidate_bound().map_err(io_problem)?;
                let media = binding.media.as_ref().ok_or(StoreProblem::Changed)?;
                media.revalidate().map_err(io_problem)?;
                if flat_identity(&binding.flat) != media.object_identity() {
                    return Err(StoreProblem::Changed);
                }
            }
            self.lock
                .as_ref()
                .ok_or(StoreProblem::Changed)?
                .revalidate_bound(&self.chain[1].flat, OsStr::new(LOCK))
                .map_err(io_problem)?;
            self.activity.revalidate().map_err(io_problem)
        }

        pub(super) fn sync_facts(
            &self,
            hooks: &mut impl Hooks,
        ) -> ([Durability; 4], Vec<StoreProblem>) {
            #[cfg(windows)]
            {
                let _ = hooks;
                ([Durability::Unproven; 4], Vec::new())
            }
            #[cfg(unix)]
            {
                let mut facts = [Durability::Unproven; 4];
                let mut failures = Vec::new();
                for (index, binding) in self.chain.iter().rev().enumerate() {
                    match hooks.sync(&binding.flat, index) {
                        Ok(()) => facts[index] = Durability::Synced,
                        Err(error) => failures.push(io_problem(error)),
                    }
                }
                match hooks.sync(self.activity.journal_root().as_ref(), 3) {
                    Ok(()) => facts[3] = Durability::Synced,
                    Err(error) => failures.push(io_problem(error)),
                }
                (facts, failures)
            }
        }

        pub(super) fn read_bound(
            &self,
            operation: &str,
        ) -> Result<Option<ObservedIntent>, StoreProblem> {
            let name = name(operation)?;
            self.revalidate()?;
            let read = self.chain[2]
                .media
                .as_ref()
                .ok_or(StoreProblem::Changed)?
                .read_file_bounded(OsStr::new(&name), MAX_INTENT_BYTES)
                .map_err(io_problem)?;
            self.revalidate()?;
            let Some(read) = read else {
                return Ok(None);
            };
            if !read.complete {
                return Err(StoreProblem::Changed);
            }
            let intent = RestoreIntent::decode(Ok(&read.bytes)).map_err(StoreProblem::Invalid)?;
            if intent.event.operation_id != operation {
                return Err(StoreProblem::Invalid(RepairHold::InvalidIntent));
            }
            Ok(Some(ObservedIntent {
                anchor: Arc::clone(&self.anchor),
                read,
                intent,
            }))
        }

        pub(super) fn write_bound(
            self,
            candidate: RestoreIntent,
            expected: Option<&ObservedIntent>,
            hooks: &mut impl Hooks,
        ) -> StoreWrite {
            let mut result = StoreWrite {
                owner: self,
                candidate,
                receipt: StoreWriteReceipt::default(),
            };
            let preflight = (|| {
                let bytes = result.candidate.encode().map_err(StoreProblem::Invalid)?;
                if result.candidate.destination.root
                    != result
                        .owner
                        .activity
                        .journal_root()
                        .identity()
                        .observation_id()
                {
                    return Err(StoreProblem::Changed);
                }
                let operation = &result.candidate.event.operation_id;
                let name = name(operation)?;
                if let Some(expected) = expected {
                    if !Arc::ptr_eq(&result.owner.anchor, &expected.anchor) {
                        return Err(StoreProblem::Changed);
                    }
                    expected
                        .intent
                        .validate_successor(&result.candidate)
                        .map_err(StoreProblem::Invalid)?;
                }
                hooks.checkpoint(Point::BeforeWrite);
                result.owner.revalidate()?;
                if let Some(expected) = expected {
                    let current = result
                        .owner
                        .read_bound(operation)?
                        .ok_or(StoreProblem::Changed)?;
                    if current.read != expected.read || current.intent != expected.intent {
                        return Err(StoreProblem::Changed);
                    }
                }
                result.owner.revalidate()?;
                Ok((name, bytes))
            })();
            let (name, bytes) = match preflight {
                Ok(value) => value,
                Err(error) => {
                    result.receipt.failures.push(error);
                    return result;
                }
            };
            #[cfg(unix)]
            {
                let directory = &result.owner.chain[2].flat;
                result.receipt.native = Some(if expected.is_some() {
                    NativePublication::Update(atomic_replace_bound(
                        directory,
                        OsStr::new(&name),
                        &bytes,
                        0o600,
                    ))
                } else {
                    NativePublication::Create(write_bytes_exclusive_bound_detailed(
                        directory,
                        OsStr::new(&name),
                        &bytes,
                        0o600,
                    ))
                });
            }
            #[cfg(windows)]
            {
                // The shared binding holds every admitted ancestor against
                // replacement; native publication retains its own parent too.
                let path = result
                    .owner
                    .activity
                    .journal_root()
                    .canonical_path()
                    .join("health/offload/restore-intents")
                    .join(&name);
                result.receipt.native = Some(if expected.is_some() {
                    NativePublication::Update(atomic_replace_detailed(&path, &bytes, 0o600))
                } else {
                    NativePublication::Create(write_bytes_exclusive_detailed(
                        &path,
                        &bytes,
                        AtomicWriteOptions { mode: Some(0o600) },
                    ))
                });
            }
            hooks.checkpoint(Point::AfterWrite);
            // Keep all native errors and every independent subsequent failure.
            if let Err(error) = result.owner.revalidate() {
                result.receipt.failures.push(error);
            }
            let intent_durability = result
                .receipt
                .native
                .as_ref()
                .map(NativePublication::directory_durability)
                .unwrap_or(Durability::Unproven);
            let durability = [
                intent_durability,
                result.owner.preparation.durability[1],
                result.owner.preparation.durability[2],
                result.owner.preparation.durability[3],
            ];
            result.receipt.durability = durability.map(Some);
            hooks.checkpoint(Point::BeforeConfirmation);
            let confirmation = (|| {
                result.owner.revalidate()?;
                let observed = result
                    .owner
                    .read_bound(&result.candidate.event.operation_id)?
                    .ok_or(StoreProblem::Changed)?;
                if !result
                    .receipt
                    .native
                    .as_ref()
                    .is_some_and(|native| native.confirms(&observed.read, &bytes))
                {
                    return Err(StoreProblem::Changed);
                }
                result.owner.revalidate()?;
                Ok(observed.read)
            })();
            match confirmation {
                Ok(read) => result.receipt.confirmation = Some(read),
                Err(error) => result.receipt.failures.push(error),
            }
            result
        }
    }

    impl StoreWriteReceipt {
        pub(super) fn fully_confirmed(&self) -> bool {
            self.native
                .as_ref()
                .is_some_and(NativePublication::fully_confirmed)
                && self.confirmation.is_some()
                && self.durability
                    == [Some(if cfg!(windows) {
                        Durability::Unproven
                    } else {
                        Durability::Synced
                    }); 4]
                && self.failures.is_empty()
        }
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod tests;
