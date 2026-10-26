// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;

pub(super) enum ClaimFailure {
    Occupied(io::Error),
    Uncertain(io::Error),
}

impl ClaimFailure {
    pub(super) fn native(error: io::Error) -> Self {
        if error.kind() == io::ErrorKind::AlreadyExists {
            Self::Occupied(error)
        } else {
            Self::Uncertain(error)
        }
    }
}

impl From<io::Error> for ClaimFailure {
    fn from(error: io::Error) -> Self {
        Self::Uncertain(error)
    }
}

/// A removal stage holds its durable record from before the original moves,
/// and the original only under this dedicated name: no other stage producer
/// uses either, so recovery never mistakes another stage for a removal.
pub const REMOVAL_RECORD_LEAF: &str = "removal-record";
pub const REMOVAL_ORIGINAL_LEAF: &str = "removal-original";

/// What an interrupted removal needs to put its original back, written and
/// synced before the original moves. It grants no authority on its own.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InterruptedRemovalRecord {
    pub version: u32,
    /// The original's directory, a sibling of the stage.
    pub source: String,
    /// The original's name in that directory.
    pub name: String,
    pub digest: MediaDigest,
}

/// Persistable observations, never capabilities for replay or cleanup.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StageObservation {
    pub journal_observation: String,
    pub parent_relative: Vec<String>,
    pub parent_identity: MediaIdentity,
    pub directory_name: String,
    pub directory_identity: MediaIdentity,
}

impl MediaStage {
    pub fn observation(&self) -> io::Result<StageObservation> {
        self.parent.revalidate()?;
        let directory = self.directory.as_ref().ok_or_else(changed)?;
        directory.revalidate()?;
        let root = self.parent.inner.journal_root();
        let relative = self
            .parent
            .diagnostic_path()
            .strip_prefix(root.canonical_path())
            .map_err(|_| changed())?;
        let parent_relative = components(relative)?
            .into_iter()
            .map(|name| name.into_string().map_err(|_| changed()))
            .collect::<io::Result<Vec<_>>>()?;
        Ok(StageObservation {
            journal_observation: root.identity().observation_id(),
            parent_relative,
            parent_identity: self.parent.identity(),
            directory_name: self.name.to_str().ok_or_else(changed)?.to_owned(),
            directory_identity: directory.identity(),
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemovalEffect {
    NotClaimed,
    ClaimUncertain,
    Claimed,
    DeletionUncertain,
    DeletePending,
    Deleted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OriginalName {
    Unverified,
    Absent,
    Occupied,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RemovalPhase {
    Mutation,
    FileSync,
    SourceSync,
    ClaimSync,
    StageParentSync,
    Cleanup,
    Completion,
}

#[derive(Debug)]
pub struct RemovalFailure {
    pub phase: RemovalPhase,
    pub error: io::Error,
}

/// Facts are independent. Neither pending nor uncertain effects earn deletion credit.
#[derive(Debug)]
pub struct RemovalReceipt {
    pub effect: RemovalEffect,
    pub original_name: OriginalName,
    pub claim_confirmation: Confirmation,
    pub file_durability: Durability,
    pub source_durability: Durability,
    pub claim_durability: Durability,
    pub stage_parent_durability: Durability,
    pub cleanup: Cleanup,
    pub claimed_identity: Option<MediaIdentity>,
    /// This leaf never attempts restoration or restart replay.
    pub restoration_attempted: bool,
    pub failures: Vec<RemovalFailure>,
}

/// Owns every retained claim. Drop only closes handles; no stage extraction exists.
/// Production use requires an owning durable preclaim obligation and stage census.
pub struct MediaRemoval {
    source: Arc<MediaDirectory>,
    name: OsString,
    expected: MediaObservation,
    stage: MediaStage,
    source_file: Option<File>,
    claim_file: Option<File>,
    stage_observation: Option<StageObservation>,
    receipt: RemovalReceipt,
}

impl MediaDirectory {
    /// Neutral effect leaf; policy admission and exclusion belong to the caller.
    pub fn remove_observed(
        self: &Arc<Self>,
        name: &OsStr,
        expected: &MediaObservation,
        stage: MediaStage,
    ) -> MediaRemoval {
        let mut removal = MediaRemoval {
            source: Arc::clone(self),
            name: name.to_owned(),
            expected: expected.clone(),
            stage,
            source_file: None,
            claim_file: None,
            stage_observation: None,
            receipt: RemovalReceipt {
                effect: RemovalEffect::NotClaimed,
                original_name: OriginalName::Unverified,
                claim_confirmation: Confirmation::Unverified,
                file_durability: Durability::Unproven,
                source_durability: Durability::Unproven,
                claim_durability: Durability::Unproven,
                stage_parent_durability: Durability::Unproven,
                cleanup: Cleanup::Pending,
                claimed_identity: None,
                restoration_attempted: false,
                failures: Vec::new(),
            },
        };
        if let Err(error) = removal.perform() {
            removal.receipt.claim_confirmation = Confirmation::Unverified;
            removal.failure(RemovalPhase::Mutation, error);
        }
        removal.finish();
        removal
    }
}

impl MediaRemoval {
    pub fn receipt(&self) -> &RemovalReceipt {
        &self.receipt
    }

    /// Move the original facts and close handles. This performs no cleanup or mutation.
    pub fn into_receipt(self) -> RemovalReceipt {
        self.receipt
    }

    pub fn stage_observation(&self) -> Option<&StageObservation> {
        self.stage_observation.as_ref()
    }

    fn failure(&mut self, phase: RemovalPhase, error: io::Error) {
        self.receipt.failures.push(RemovalFailure { phase, error });
    }

    fn validate_context(&self) -> io::Result<()> {
        self.stage.revalidate_activity()?;
        self.source.revalidate()?;
        self.stage.parent.revalidate()?;
        let directory = self.stage.directory.as_ref().ok_or_else(changed)?;
        directory.revalidate()?;
        let source_root = self.source.inner.journal_root();
        let stage_root = self.stage.parent.inner.journal_root();
        if !Arc::ptr_eq(source_root, self.stage.activity.journal_root())
            || source_root.identity() != stage_root.identity()
            || source_root.canonical_path() != stage_root.canonical_path()
            || self.source.diagnostic_path().parent() != Some(self.stage.parent.diagnostic_path())
            || !self.source.identity().same_volume(directory.identity())
        {
            return Err(invalid("removal stage is not a same-volume sibling"));
        }
        let source_name = self
            .source
            .diagnostic_path()
            .file_name()
            .ok_or_else(changed)?;
        let child = self
            .stage
            .parent
            .open_child(source_name)?
            .ok_or_else(changed)?;
        if child.identity() != self.source.identity() {
            return Err(changed());
        }
        self.source.revalidate()
    }

    fn check_file(
        directory: &MediaDirectory,
        name: &OsStr,
        file: &mut File,
        expected: &MediaObservation,
    ) -> io::Result<()> {
        directory.revalidate()?;
        if platform::identity(file)? != expected.identity {
            return Err(changed());
        }
        let stamp = platform::stamp(file)?;
        file.seek(SeekFrom::Start(0))?;
        let digest = stream(file, &mut io::sink(), Some(expected.digest.size))?;
        if digest != expected.digest
            || platform::stamp(file)? != stamp
            || platform::identity(file)? != expected.identity
        {
            return Err(changed());
        }
        platform::check_leaf(&directory.inner, name, expected.identity)?;
        directory.revalidate()
    }

    fn perform(&mut self) -> io::Result<()> {
        plain_name(&self.name)?;
        platform::supported_mutation()?;
        if self.stage.leaf.is_some() || self.stage.removed {
            return Err(invalid("removal requires an unused empty stage"));
        }
        self.validate_context()?;
        let directory = self.stage.directory.as_ref().ok_or_else(changed)?;
        if directory.entries(0)? != Some(Vec::new()) {
            return Err(invalid("removal stage is occupied"));
        }
        self.stage_observation = Some(self.stage.observation()?);
        self.source_file = platform::open_removal(&self.source.inner, &self.name)?;
        Self::check_file(
            &self.source,
            &self.name,
            self.source_file.as_mut().ok_or_else(changed)?,
            &self.expected,
        )?;
        step(MediaPrimitive::BeforeRemovalClaim)?;
        self.validate_context()?;
        Self::check_file(
            &self.source,
            &self.name,
            self.source_file.as_mut().ok_or_else(changed)?,
            &self.expected,
        )?;
        self.write_record()?;
        let directory = self.stage.directory.as_ref().ok_or_else(changed)?;
        let leaf = OsStr::new(REMOVAL_ORIGINAL_LEAF);
        // Record uncertainty before a syscall, including applied-but-error outcomes.
        self.receipt.effect = RemovalEffect::ClaimUncertain;
        match platform::claim(
            &self.source.inner,
            &self.name,
            self.source_file.as_ref().ok_or_else(changed)?,
            &directory.inner,
            leaf,
        ) {
            Ok(()) => self.receipt.effect = RemovalEffect::Claimed,
            Err(ClaimFailure::Occupied(error)) => {
                self.receipt.effect = RemovalEffect::NotClaimed;
                return Err(error);
            }
            Err(ClaimFailure::Uncertain(error)) => return Err(error),
        }
        step(MediaPrimitive::AfterRemovalClaim)?;
        step(MediaPrimitive::BeforeClaimRead)?;
        self.validate_context()?;
        self.claim_file = platform::open_plain(&directory.inner, leaf)?;
        let claim = self.claim_file.as_mut().ok_or_else(changed)?;
        self.receipt.claimed_identity = Some(platform::identity(claim)?);
        Self::check_file(directory, leaf, claim, &self.expected)?;
        self.receipt.claim_confirmation = Confirmation::Confirmed;
        step(MediaPrimitive::BeforeClaimDelete)?;
        self.validate_context()?;
        let directory = self.stage.directory.as_ref().ok_or_else(changed)?;
        Self::check_file(
            directory,
            leaf,
            self.claim_file.as_mut().ok_or_else(changed)?,
            &self.expected,
        )?;
        // Also retain and verify the original actual file through the final cut.
        if platform::identity(self.source_file.as_ref().ok_or_else(changed)?)?
            != self.expected.identity
        {
            return Err(changed());
        }
        self.validate_context()?;
        self.receipt.effect = RemovalEffect::DeletionUncertain;
        platform::delete_claim(
            &directory.inner,
            leaf,
            self.source_file.as_ref().ok_or_else(changed)?,
        )?;
        self.receipt.effect = RemovalEffect::DeletePending;
        self.receipt.claim_confirmation = Confirmation::Unverified;
        step(MediaPrimitive::AfterClaimDelete)?;
        // Windows delete-pending acceptance is not absence while handles remain.
        self.source_file = None;
        self.claim_file = None;
        platform::ensure_absent(&directory.inner, leaf)?;
        self.validate_context()?;
        step(MediaPrimitive::RemovalCompletion)?;
        platform::ensure_absent(&directory.inner, leaf)?;
        self.validate_context()?;
        self.receipt.effect = RemovalEffect::Deleted;
        self.receipt.claim_confirmation = Confirmation::Confirmed;
        Ok(())
    }

    /// Durably record the original's own coordinates in the stage before it
    /// moves. The record is the stage's only owned leaf until cleanup.
    fn write_record(&mut self) -> io::Result<()> {
        let source = self
            .source
            .diagnostic_path()
            .file_name()
            .and_then(OsStr::to_str)
            .ok_or_else(changed)?
            .to_owned();
        let record = InterruptedRemovalRecord {
            version: 1,
            source,
            name: self.name.to_str().ok_or_else(changed)?.to_owned(),
            digest: self.expected.digest.clone(),
        };
        let bytes = serde_json::to_vec(&record).map_err(io::Error::other)?;
        let directory = Arc::clone(self.stage.directory.as_ref().ok_or_else(changed)?);
        let name = OsStr::new(REMOVAL_RECORD_LEAF);
        let file = platform::create_file(&directory.inner, name)?;
        let identity = platform::identity(&file)?;
        self.stage.leaf = Some(Box::new(Leaf {
            name: name.to_owned(),
            file,
            identity: Some(identity),
            #[cfg(windows)]
            mutation_access: true,
        }));
        let leaf = self.stage.leaf.as_mut().ok_or_else(changed)?;
        leaf.file.write_all(&bytes)?;
        leaf.file.sync_all()?;
        platform::check_leaf(&directory.inner, name, identity)?;
        if let Err(error) = directory.inner.sync()
            && !(cfg!(windows) && error.kind() == io::ErrorKind::Unsupported)
        {
            return Err(error);
        }
        directory.revalidate()
    }

    fn sync(directory: &MediaDirectory, primitive: MediaPrimitive) -> io::Result<()> {
        directory.revalidate()?;
        step(primitive)?;
        directory.inner.sync()?;
        directory.revalidate()
    }

    fn finish(&mut self) {
        if self.receipt.effect != RemovalEffect::NotClaimed {
            match Self::sync(&self.source, MediaPrimitive::RemovalSourceSync) {
                Ok(()) => self.receipt.source_durability = Durability::Synced,
                Err(error) => {
                    if !(cfg!(windows) && error.kind() == io::ErrorKind::Unsupported) {
                        self.failure(RemovalPhase::SourceSync, error);
                    }
                }
            }
            if let Some(directory) = &self.stage.directory {
                match Self::sync(directory, MediaPrimitive::RemovalClaimSync) {
                    Ok(()) => self.receipt.claim_durability = Durability::Synced,
                    Err(error) => {
                        if !(cfg!(windows) && error.kind() == io::ErrorKind::Unsupported) {
                            self.failure(RemovalPhase::ClaimSync, error);
                        }
                    }
                }
            }
        }
        self.cleanup();
        if self.stage.removed {
            match Self::sync(&self.stage.parent, MediaPrimitive::RemovalParentSync) {
                Ok(()) => self.receipt.stage_parent_durability = Durability::Synced,
                Err(error) => {
                    if !(cfg!(windows) && error.kind() == io::ErrorKind::Unsupported) {
                        self.failure(RemovalPhase::StageParentSync, error);
                    }
                }
            }
        }
        match plain_name(&self.name)
            .and_then(|()| self.source.revalidate())
            .and_then(|()| platform::entry(&self.source.inner, &self.name))
        {
            Ok(None) => self.receipt.original_name = OriginalName::Absent,
            Ok(Some(_)) => self.receipt.original_name = OriginalName::Occupied,
            Err(error) => {
                self.receipt.original_name = OriginalName::Unverified;
                self.failure(RemovalPhase::Completion, error);
            }
        }
        if let Err(error) = self.stage.parent.revalidate() {
            if error.kind() == io::ErrorKind::InvalidData
                || !(self.receipt.effect == RemovalEffect::Deleted
                    && self.receipt.claim_confirmation == Confirmation::Confirmed)
            {
                self.receipt.claim_confirmation = Confirmation::Unverified;
            }
            self.failure(RemovalPhase::Completion, error);
        }
        let stage_confirmation = (|| {
            self.stage.revalidate_activity()?;
            self.stage.parent.revalidate()?;
            if self.stage.removed {
                platform::ensure_absent(&self.stage.parent.inner, &self.stage.name)
            } else {
                let directory = self.stage.directory.as_ref().ok_or_else(changed)?;
                directory.revalidate()?;
                if self.receipt.effect == RemovalEffect::Deleted {
                    platform::ensure_absent(&directory.inner, OsStr::new(REMOVAL_ORIGINAL_LEAF))?;
                }
                Ok(())
            }
        })();
        if let Err(error) = stage_confirmation {
            if error.kind() == io::ErrorKind::InvalidData
                || !(self.receipt.effect == RemovalEffect::Deleted
                    && self.receipt.claim_confirmation == Confirmation::Confirmed)
            {
                self.receipt.claim_confirmation = Confirmation::Unverified;
            }
            self.receipt.cleanup = Cleanup::Refused;
            self.failure(RemovalPhase::Completion, error);
        }
    }

    /// Only known empty stages may be retired. Never retries a claim deletion.
    pub fn cleanup(&mut self) -> Cleanup {
        if let Err(error) = self.stage.revalidate_activity() {
            self.receipt.cleanup = Cleanup::Refused;
            if error.kind() == io::ErrorKind::InvalidData
                || !(self.receipt.effect == RemovalEffect::Deleted
                    && self.receipt.claim_confirmation == Confirmation::Confirmed)
            {
                self.receipt.claim_confirmation = Confirmation::Unverified;
            }
            self.failure(RemovalPhase::Cleanup, error);
            return Cleanup::Refused;
        }
        if !matches!(
            self.receipt.effect,
            RemovalEffect::NotClaimed | RemovalEffect::Deleted
        ) {
            self.receipt.cleanup = Cleanup::Refused;
            return Cleanup::Refused;
        }
        if self.stage.removed {
            match self
                .stage
                .parent
                .revalidate()
                .and_then(|()| platform::ensure_absent(&self.stage.parent.inner, &self.stage.name))
            {
                Ok(()) => {
                    self.receipt.cleanup = Cleanup::Complete;
                    return Cleanup::Complete;
                }
                Err(error) => {
                    if error.kind() == io::ErrorKind::InvalidData
                        || !(self.receipt.effect == RemovalEffect::Deleted
                            && self.receipt.claim_confirmation == Confirmation::Confirmed)
                    {
                        self.receipt.claim_confirmation = Confirmation::Unverified;
                    }
                    self.receipt.cleanup = Cleanup::Refused;
                    self.failure(RemovalPhase::Cleanup, error);
                    return Cleanup::Refused;
                }
            }
        }
        if let Some(directory) = &self.stage.directory {
            // The record is the stage's own leaf; stage cleanup removes it.
            match directory.entries(2) {
                Ok(Some(entries))
                    if entries.iter().all(|entry| {
                        self.stage.leaf.is_some() && entry.name == OsStr::new(REMOVAL_RECORD_LEAF)
                    }) => {}
                Ok(_) => {
                    self.receipt.cleanup = Cleanup::Refused;
                    return Cleanup::Refused;
                }
                Err(error) => {
                    self.failure(RemovalPhase::Cleanup, error);
                    self.receipt.cleanup = Cleanup::Refused;
                    return Cleanup::Refused;
                }
            }
        }
        self.receipt.stage_parent_durability = Durability::Unproven;
        match self.stage.cleanup_exact() {
            Ok(()) => {
                self.receipt.cleanup = Cleanup::Complete;
                Cleanup::Complete
            }
            Err(error) => {
                let cleanup = if error.kind() == io::ErrorKind::InvalidData {
                    Cleanup::Refused
                } else {
                    Cleanup::Pending
                };
                self.receipt.cleanup = cleanup;
                self.failure(RemovalPhase::Cleanup, error);
                cleanup
            }
        }
    }
}

/// How an interrupted removal was resolved.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InterruptedRemovalResolution {
    /// The original never moved, or its deletion had already completed.
    Discarded,
    /// The original went back under its own name in its own directory.
    Returned,
    /// The owner deleted the original's directory, so its deletion finished.
    Finished,
}

/// Resolve one removal stage whose owner ended, under a freshly held lease on
/// local storage. `None` means the stage is not a removal's and is untouched.
/// A moved original goes back under its recorded name when that directory is
/// live and the name is free; `deleted_by_owner` says whether the owner deleted
/// that directory, in which case its deletion finishes instead. The original
/// must still match its record exactly. Anything else refuses, leaving the
/// stage in place.
pub fn resolve_interrupted_removal(
    activity: &Arc<crate::activity::ActivityLease>,
    parent: &Arc<MediaDirectory>,
    name: &OsStr,
    directory: MediaIdentity,
    deleted_by_owner: impl FnOnce(&OsStr) -> io::Result<bool>,
) -> io::Result<Option<InterruptedRemovalResolution>> {
    #[cfg(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "ios",
        target_os = "android",
        target_os = "windows"
    ))]
    {
        use crate::journal_root::JournalEntryKind;
        const REMOVAL_RECORD_MAX: usize = 4096;
        plain_name(name)?;
        if !crate::activity_obligations::is_stage_namespace_name(name)
            || !Arc::ptr_eq(parent.inner.journal_root(), activity.journal_root())
        {
            return Err(invalid("not a stage of this journal"));
        }
        if !exclusive_local_activity(activity)? {
            return Err(activity_carrier::shared_storage());
        }
        let revalidate = || {
            activity
                .revalidate()
                .map_err(crate::activity::ActivityAdmissionError::into_io)
        };
        let sync_dir = |dir: &MediaDirectory| -> io::Result<()> {
            match dir.sync() {
                Ok(()) => Ok(()),
                Err(err) if cfg!(windows) && err.kind() == io::ErrorKind::Unsupported => Ok(()),
                Err(err) => Err(err),
            }
        };
        let Some(inner) = platform::open_removal_directory(&parent.inner, name)? else {
            return Ok(None);
        };
        let stage = Arc::new(MediaDirectory { inner });
        if stage.identity() != directory {
            return Err(changed());
        }
        let record_leaf = OsStr::new(REMOVAL_RECORD_LEAF);
        let original_leaf = OsStr::new(REMOVAL_ORIGINAL_LEAF);
        let Some(entries) = stage.entries(2)? else {
            return Ok(None);
        };
        let ours = entries.iter().all(|entry| {
            (entry.name == record_leaf || entry.name == original_leaf)
                && entry.kind == JournalEntryKind::RegularFile
        });
        let Some(record_entry) = entries.iter().find(|entry| entry.name == record_leaf) else {
            return Ok(None);
        };
        if !ours {
            return Ok(None);
        }
        let original = entries.iter().find(|entry| entry.name == original_leaf);
        let read = stage
            .read_file_bounded(record_leaf, REMOVAL_RECORD_MAX)?
            .ok_or_else(changed)?;
        if read.entry.identity != record_entry.identity {
            return Err(changed());
        }
        // The record is synced before the original moves, so an unreadable one
        // can only stand beside an original that never moved.
        let record = serde_json::from_slice::<InterruptedRemovalRecord>(&read.bytes)
            .ok()
            .filter(|record| {
                record.version == 1
                    && plain_name(OsStr::new(&record.source)).is_ok()
                    && plain_name(OsStr::new(&record.name)).is_ok()
                    && !crate::activity_obligations::is_stage_namespace_name(OsStr::new(
                        &record.source,
                    ))
            });
        let resolution = match (original, record) {
            (None, _) => InterruptedRemovalResolution::Discarded,
            (Some(_), None) => return Err(invalid("an unreadable record holds its original")),
            (Some(entry), Some(record)) => {
                let observed = stage.observe_file(original_leaf)?.ok_or_else(changed)?;
                if observed.identity != entry.identity || observed.digest != record.digest {
                    return Err(changed());
                }
                revalidate()?;
                let file =
                    platform::open_removal(&stage.inner, original_leaf)?.ok_or_else(changed)?;
                if platform::identity(&file)? != observed.identity {
                    return Err(changed());
                }
                if deleted_by_owner(OsStr::new(&record.source))? {
                    revalidate()?;
                    platform::check_leaf(&stage.inner, original_leaf, observed.identity)?;
                    platform::remove_leaf(&stage.inner, original_leaf, &file)?;
                    drop(file);
                    platform::ensure_absent(&stage.inner, original_leaf)?;
                    InterruptedRemovalResolution::Finished
                } else {
                    let source = parent
                        .open_child(OsStr::new(&record.source))?
                        .ok_or_else(changed)?;
                    let original_name = OsStr::new(&record.name);
                    if platform::entry(&source.inner, original_name)?.is_some() {
                        return Err(changed());
                    }
                    revalidate()?;
                    platform::check_leaf(&stage.inner, original_leaf, observed.identity)?;
                    platform::claim(
                        &stage.inner,
                        original_leaf,
                        &file,
                        &source.inner,
                        original_name,
                    )
                    .map_err(|failure| match failure {
                        ClaimFailure::Occupied(error) | ClaimFailure::Uncertain(error) => error,
                    })?;
                    platform::check_leaf(&source.inner, original_name, observed.identity)?;
                    sync_dir(&source)?;
                    InterruptedRemovalResolution::Returned
                }
            }
        };
        sync_dir(&stage)?;
        revalidate()?;
        let file = platform::open_removal(&stage.inner, record_leaf)?.ok_or_else(changed)?;
        if platform::identity(&file)? != record_entry.identity {
            return Err(changed());
        }
        platform::check_leaf(&stage.inner, record_leaf, record_entry.identity)?;
        platform::remove_leaf(&stage.inner, record_leaf, &file)?;
        drop(file);
        platform::ensure_absent(&stage.inner, record_leaf)?;
        revalidate()?;
        platform::remove_directory(&parent.inner, name, &stage.inner)?;
        drop(stage);
        platform::ensure_absent(&parent.inner, name)?;
        sync_dir(parent)?;
        Ok(Some(resolution))
    }
    #[cfg(not(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "ios",
        target_os = "android",
        target_os = "windows"
    )))]
    {
        let _ = (activity, parent, name, directory, deleted_by_owner);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "the activity lease does not exclude every writer here",
        ))
    }
}
