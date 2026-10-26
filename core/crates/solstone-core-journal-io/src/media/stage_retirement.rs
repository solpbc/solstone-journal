// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! An ended producer's redundant stage, retained across the caller's unwind boundary.

use super::*;
use crate::activity::ActivityLease;

/// What the native removal call established, independently of name absence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StageRemovalEffect {
    NotAttempted,
    /// The call started but has not established whether removal took effect.
    Uncertain,
    /// The native call accepted removal. Absence still requires observation.
    Requested,
}

#[derive(Debug)]
pub struct StageRetirementReceipt {
    pub existed: Option<bool>,
    pub leaf_effect: StageRemovalEffect,
    pub leaf_absence: Confirmation,
    pub directory_effect: StageRemovalEffect,
    pub directory_absence: Confirmation,
    pub parent_durability: Durability,
    pub failure: Option<io::Error>,
}

enum ExpectedLeaf {
    Recorded(OsString, MediaIdentity),
    Empty,
}

/// Construct before entering a fallible actor, then mutate by borrowing this
/// owner. Drop closes retained handles and never retries cleanup.
#[must_use = "Retain the actual retirement owner until its facts are handled"]
pub struct StageRetirement {
    activity: Arc<ActivityLease>,
    parent: Arc<MediaDirectory>,
    name: OsString,
    directory: MediaIdentity,
    expected: ExpectedLeaf,
    stage: Option<platform::Directory>,
    file: Option<File>,
    attempted: bool,
    complete: bool,
    receipt: StageRetirementReceipt,
}

impl StageRetirement {
    /// The caller must establish that this ended producer's exact stage copy
    /// is redundant. Persisted identities alone grant no retirement authority.
    pub fn recorded(
        activity: Arc<ActivityLease>,
        parent: Arc<MediaDirectory>,
        name: OsString,
        directory: MediaIdentity,
        leaf_name: OsString,
        leaf: MediaIdentity,
    ) -> Self {
        let mut owner = Self::empty(activity, parent, name, directory);
        owner.expected = ExpectedLeaf::Recorded(leaf_name, leaf);
        owner
    }

    /// Retire only an empty, freshly identity-bound stage in an admitted
    /// producer namespace. This never accepts a media-bearing orphan.
    pub fn empty(
        activity: Arc<ActivityLease>,
        parent: Arc<MediaDirectory>,
        name: OsString,
        directory: MediaIdentity,
    ) -> Self {
        Self {
            activity,
            parent,
            name,
            directory,
            expected: ExpectedLeaf::Empty,
            stage: None,
            file: None,
            attempted: false,
            complete: false,
            receipt: StageRetirementReceipt {
                existed: None,
                leaf_effect: StageRemovalEffect::NotAttempted,
                leaf_absence: Confirmation::Unverified,
                directory_effect: StageRemovalEffect::NotAttempted,
                directory_absence: Confirmation::Unverified,
                parent_durability: Durability::Unproven,
                failure: None,
            },
        }
    }

    pub fn receipt(&self) -> &StageRetirementReceipt {
        &self.receipt
    }

    pub fn completed(&self) -> bool {
        self.complete
    }

    /// One attempt only. A cancellation or unwind after an effect does not
    /// discard it or authorize another cleanup attempt.
    pub fn retire(&mut self, checkpoint: &dyn Fn() -> io::Result<()>) {
        let expected = match &self.expected {
            ExpectedLeaf::Recorded(name, identity) => Some((name.clone(), *identity)),
            ExpectedLeaf::Empty => None,
        };
        let leaf = match &expected {
            Some((name, identity)) => Leaf::Exact(name, Some(*identity)),
            None => Leaf::Empty,
        };
        self.attempt(leaf, checkpoint);
    }

    pub fn into_completed_receipt(self) -> Result<StageRetirementReceipt, Box<Self>> {
        if self.complete {
            Ok(self.receipt)
        } else {
            Err(Box::new(self))
        }
    }

    fn attempt(&mut self, leaf: Leaf<'_>, checkpoint: &dyn Fn() -> io::Result<()>) {
        if self.attempted {
            return;
        }
        self.attempted = true;
        if let Err(error) = self.perform(leaf, checkpoint) {
            self.receipt.failure = Some(error);
        }
    }

    fn check(&self, checkpoint: &dyn Fn() -> io::Result<()>) -> io::Result<()> {
        checkpoint()?;
        self.activity
            .revalidate()
            .map_err(crate::activity::ActivityAdmissionError::into_io)?;
        self.parent.revalidate()
    }

    #[cfg(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "ios",
        target_os = "android",
        windows
    ))]
    fn perform(
        &mut self,
        leaf: Leaf<'_>,
        checkpoint: &dyn Fn() -> io::Result<()>,
    ) -> io::Result<()> {
        plain_name(&self.name)?;
        if let Leaf::Exact(leaf_name, _) = leaf {
            plain_name(leaf_name)?;
        }
        if !crate::activity_obligations::is_stage_namespace_name(&self.name)
            || !Arc::ptr_eq(
                self.parent.inner.journal_root(),
                self.activity.journal_root(),
            )
        {
            return Err(invalid("not a stage of this journal"));
        }
        self.check(checkpoint)?;
        if !exclusive_local_activity(&self.activity)? {
            return Err(super::activity_carrier::shared_storage());
        }
        self.stage = platform::open_removal_directory(&self.parent.inner, &self.name)?;
        let Some(stage) = &self.stage else {
            self.receipt.existed = Some(false);
            self.receipt.leaf_absence = Confirmation::Confirmed;
            self.receipt.directory_absence = Confirmation::Confirmed;
            self.complete = true;
            return Ok(());
        };
        self.receipt.existed = Some(true);
        if stage.identity() != self.directory {
            return Err(changed());
        }
        let entries = platform::entries(stage, 2)?.ok_or_else(changed)?;
        match entries.as_slice() {
            [] => self.receipt.leaf_absence = Confirmation::Confirmed,
            [entry]
                if entry.kind == crate::journal_root::JournalEntryKind::RegularFile
                    && match leaf {
                        Leaf::Exact(name, recorded) => {
                            entry.name == name && recorded.is_none_or(|id| entry.identity == id)
                        }
                        Leaf::Accepted(accept) => {
                            plain_name(&entry.name).is_ok() && accept(&entry.name)
                        }
                        Leaf::Empty => false,
                    } =>
            {
                self.check(checkpoint)?;
                let stage = self.stage.as_ref().ok_or_else(changed)?;
                self.file = platform::open_removal(stage, &entry.name)?;
                let file = self.file.as_ref().ok_or_else(changed)?;
                if platform::identity(file)? != entry.identity {
                    return Err(changed());
                }
                platform::check_leaf(stage, &entry.name, entry.identity)?;
                self.receipt.leaf_effect = StageRemovalEffect::Uncertain;
                platform::remove_leaf(stage, &entry.name, file)?;
                self.receipt.leaf_effect = StageRemovalEffect::Requested;
                // Windows requires the actual delete handle to close before
                // absence can be observed. Keep its separate native facts.
                self.file.take();
                platform::ensure_absent(stage, &entry.name)?;
                self.receipt.leaf_absence = Confirmation::Confirmed;
            }
            _ => return Err(changed()),
        }
        self.check(checkpoint)?;
        let stage = self.stage.as_ref().ok_or_else(changed)?;
        self.receipt.directory_effect = StageRemovalEffect::Uncertain;
        platform::remove_directory(&self.parent.inner, &self.name, stage)?;
        self.receipt.directory_effect = StageRemovalEffect::Requested;
        self.stage.take();
        platform::ensure_absent(&self.parent.inner, &self.name)?;
        self.receipt.directory_absence = Confirmation::Confirmed;
        self.check(checkpoint)?;
        if cfg!(not(windows)) {
            self.parent.sync()?;
            self.receipt.parent_durability = Durability::Synced;
        }
        self.complete = true;
        Ok(())
    }

    #[cfg(not(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "ios",
        target_os = "android",
        windows
    )))]
    fn perform(
        &mut self,
        _leaf: Leaf<'_>,
        _checkpoint: &dyn Fn() -> io::Result<()>,
    ) -> io::Result<()> {
        Err(super::activity_carrier::shared_storage())
    }
}

pub(super) enum Leaf<'a> {
    Exact(&'a OsStr, Option<MediaIdentity>),
    Accepted(&'a dyn Fn(&OsStr) -> bool),
    Empty,
}

pub(super) fn retire_stage(
    activity: &Arc<ActivityLease>,
    parent: &Arc<MediaDirectory>,
    name: &OsStr,
    directory: MediaIdentity,
    leaf: Leaf<'_>,
) -> io::Result<bool> {
    let mut owner = StageRetirement::empty(
        Arc::clone(activity),
        Arc::clone(parent),
        name.to_owned(),
        directory,
    );
    owner.attempt(leaf, &|| Ok(()));
    match owner.receipt.failure.take() {
        Some(error) => Err(error),
        None => Ok(owner.receipt.existed == Some(true)),
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;
    use std::fs;

    struct Fixture {
        temp: tempfile::TempDir,
        root: Arc<JournalRoot>,
        owner: StageRetirement,
        path: PathBuf,
    }

    fn fixture() -> Fixture {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir(temp.path().join("segment")).unwrap();
        let root = Arc::new(JournalRoot::open(temp.path()).unwrap());
        let activity = ActivityLease::try_acquire(Arc::clone(&root)).unwrap();
        let parent = MediaDirectory::open(Arc::clone(&root), Path::new("segment")).unwrap();
        let stage = MediaStage::create(Arc::clone(&parent), Arc::clone(&activity))
            .unwrap_or_else(|failure| panic!("{}", failure.error));
        let name = stage.name.clone();
        let path = temp.path().join("segment").join(&name);
        fs::write(path.join("copy"), b"redundant").unwrap();
        let directory = parent.open_child(&name).unwrap().unwrap();
        let leaf = directory.observe_file(OsStr::new("copy")).unwrap().unwrap();
        let owner = StageRetirement::recorded(
            activity,
            parent,
            name,
            directory.identity(),
            "copy".into(),
            leaf.identity,
        );
        drop(stage);
        Fixture {
            temp,
            root,
            owner,
            path,
        }
    }

    #[test]
    fn directory_failure_retains_actual_leaf_removal_and_exclusion() {
        let mut f = fixture();
        let (_, consumed) = run_with_media_hook(
            MediaPrimitive::NativeDirectoryRemoveCut,
            || Err(io::Error::other("directory retirement refused")),
            || f.owner.retire(&|| Ok(())),
        );
        assert!(consumed);
        assert!(!f.owner.completed());
        assert_eq!(f.owner.receipt().leaf_effect, StageRemovalEffect::Requested);
        assert_eq!(f.owner.receipt().leaf_absence, Confirmation::Confirmed);
        assert_eq!(
            f.owner.receipt().directory_effect,
            StageRemovalEffect::Uncertain
        );
        assert_eq!(
            f.owner.receipt().directory_absence,
            Confirmation::Unverified
        );
        assert!(f.owner.receipt().failure.is_some());
        assert!(f.owner.stage.is_some());
        assert!(f.path.is_dir());
        assert!(!f.path.join("copy").exists());
        assert!(ActivityLease::try_acquire(Arc::clone(&f.root)).is_err());
        f.owner.retire(&|| Ok(()));
        assert!(
            f.path.exists(),
            "an incomplete owner must not retry cleanup"
        );
        drop(f.owner);
        assert!(f.path.exists(), "Drop closes handles without cleanup");
        assert!(ActivityLease::try_acquire(Arc::clone(&f.root)).is_ok());
        drop(f.temp);
    }

    #[test]
    fn cancellation_after_leaf_and_unwind_after_directory_keep_separate_facts() {
        let mut cancelled = fixture();
        let path = cancelled.path.clone();
        cancelled.owner.retire(&|| {
            if !path.join("copy").exists() {
                Err(io::ErrorKind::Interrupted.into())
            } else {
                Ok(())
            }
        });
        assert_eq!(
            cancelled.owner.receipt().leaf_absence,
            Confirmation::Confirmed
        );
        assert_eq!(
            cancelled.owner.receipt().directory_effect,
            StageRemovalEffect::NotAttempted
        );
        assert!(cancelled.path.is_dir());
        assert_eq!(
            cancelled.owner.receipt().failure.as_ref().unwrap().kind(),
            io::ErrorKind::Interrupted
        );

        let mut unwound = fixture();
        let path = unwound.path.clone();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            unwound.owner.retire(&|| {
                assert!(path.exists(), "after actual directory retirement");
                Ok(())
            });
        }));
        assert!(result.is_err());
        assert!(!unwound.owner.completed());
        assert_eq!(
            unwound.owner.receipt().directory_effect,
            StageRemovalEffect::Requested
        );
        assert_eq!(
            unwound.owner.receipt().directory_absence,
            Confirmation::Confirmed
        );
        assert_eq!(
            unwound.owner.receipt().parent_durability,
            Durability::Unproven
        );
        assert!(unwound.owner.receipt().failure.is_none());
        assert!(ActivityLease::try_acquire(Arc::clone(&unwound.root)).is_err());
        assert!(!unwound.path.exists());
    }

    #[test]
    fn empty_only_refuses_payload_and_completed_receipt_closes_its_lease() {
        let mut f = fixture();
        f.owner.expected = ExpectedLeaf::Empty;
        f.owner.retire(&|| Ok(()));
        assert!(!f.owner.completed());
        assert_eq!(fs::read(f.path.join("copy")).unwrap(), b"redundant");
        assert_eq!(
            f.owner.receipt().leaf_effect,
            StageRemovalEffect::NotAttempted
        );
        drop(f.owner);

        let mut f = fixture();
        f.owner.retire(&|| Ok(()));
        assert!(f.owner.completed());
        let receipt = f
            .owner
            .into_completed_receipt()
            .unwrap_or_else(|_| panic!("incomplete"));
        assert_eq!(receipt.directory_absence, Confirmation::Confirmed);
        assert!(!f.path.exists());
        assert!(ActivityLease::try_acquire(Arc::clone(&f.root)).is_ok());
    }
}
