// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;

/// An original stage creation retained before mutation. Drop only closes
/// handles, including when the native operation or its caller unwinds.
#[must_use = "Retain actual creation custody on failure or unwind"]
pub struct MediaStageCreation {
    parent: Arc<MediaDirectory>,
    stage: Option<MediaStage>,
    error: Option<io::Error>,
    attempted: bool,
    complete: bool,
    // Even an incomplete native stage closes before its admission ends.
    activity: StageLease,
}

impl MediaStageCreation {
    pub fn new(parent: Arc<MediaDirectory>, activity: Arc<crate::activity::ActivityLease>) -> Self {
        Self::with_lease(parent, StageLease::Activity(activity))
    }

    pub(super) fn with_lease(parent: Arc<MediaDirectory>, activity: StageLease) -> Self {
        Self {
            parent,
            activity,
            stage: None,
            error: None,
            attempted: false,
            complete: false,
        }
    }

    pub fn error(&self) -> Option<&io::Error> {
        self.error.as_ref()
    }

    pub fn create(&mut self) {
        if self.attempted {
            return;
        }
        self.attempted = true;
        match self.perform() {
            Ok(()) => self.complete = true,
            Err(error) => self.error = Some(error),
        }
    }

    pub fn take_created(&mut self) -> Option<MediaStage> {
        if self.complete {
            self.stage.take()
        } else {
            None
        }
    }

    pub(super) fn into_result(
        mut self,
    ) -> Result<MediaStage, OwnedMediaFailure<Option<MediaStage>>> {
        if let Some(stage) = self.take_created() {
            return Ok(stage);
        }
        Err(OwnedMediaFailure {
            error: self.error.take().unwrap_or_else(changed),
            owned: self.stage.take(),
        })
    }

    fn perform(&mut self) -> io::Result<()> {
        platform::supported_mutation()?;
        if !Arc::ptr_eq(
            self.parent.inner.journal_root(),
            self.activity.journal_root(),
        ) {
            return Err(invalid("stage belongs to another activity root"));
        }
        self.activity
            .revalidate()
            .map_err(crate::activity::ActivityAdmissionError::into_io)?;
        self.parent.revalidate()?;
        let mut random = [0u8; 16];
        getrandom::fill(&mut random).map_err(|error| io::Error::other(error.to_string()))?;
        self.stage = Some(MediaStage {
            parent: Arc::clone(&self.parent),
            activity: self.activity.clone(),
            name: OsString::from(format!(".tmp-offload-{:032x}", u128::from_le_bytes(random))),
            directory: None,
            leaf: None,
            removed: false,
            #[cfg(windows)]
            published: false,
            #[cfg(windows)]
            payload_delete_pending: None,
            #[cfg(windows)]
            directory_delete_pending: false,
        });
        let stage = self.stage.as_mut().ok_or_else(changed)?;
        stage.revalidate_activity()?;
        let inner = platform::create_directory(&stage.parent.inner, &stage.name)?;
        stage.directory = Some(Arc::new(MediaDirectory { inner }));
        step(MediaPrimitive::StageCreated)?;
        stage.directory.as_ref().ok_or_else(changed)?.revalidate()?;
        stage.revalidate_activity()
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;

    #[test]
    fn original_creation_survives_unwind_after_native_directory_creation() {
        let temp = tempfile::tempdir().unwrap();
        let root = Arc::new(JournalRoot::open(temp.path()).unwrap());
        let activity = crate::activity::ActivityLease::try_acquire(Arc::clone(&root)).unwrap();
        let parent = MediaDirectory::open(Arc::clone(&root), Path::new("")).unwrap();
        let mut creation = MediaStageCreation::new(parent, activity);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_with_media_hook(
                MediaPrimitive::StageCreated,
                || panic!("after actual stage creation"),
                || creation.create(),
            );
        }));
        assert!(result.is_err());
        assert!(!creation.complete);
        let stage = creation.stage.as_ref().unwrap();
        let path = stage.diagnostic_path();
        assert!(stage.directory.is_some());
        assert!(path.is_dir());
        assert!(creation.take_created().is_none());
        assert!(crate::activity::ActivityLease::try_acquire(Arc::clone(&root)).is_err());
        drop(creation);
        assert!(
            path.is_dir(),
            "Drop must not silently clean an unknown creation"
        );
        assert!(crate::activity::ActivityLease::try_acquire(root).is_ok());
    }
}
