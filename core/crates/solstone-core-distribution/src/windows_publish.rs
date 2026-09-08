// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Producer-local directory publication. The shared journal publisher is unchanged.

#[cfg(any(windows, test))]
use std::fs;
use std::path::{Path, PathBuf};

#[cfg(windows)]
#[allow(unsafe_code)]
mod native;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedWindowsPayload {
    pub destination: PathBuf,
    /// Move success is observable; no directory crash-durability proof is made.
    pub durability_proven: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublicationState {
    NotPublished,
    /// The native operation returned success but final observation failed.
    /// Keep both names for reconciliation; never clean up either by inference.
    Unconfirmed,
}

#[derive(Debug)]
pub struct WindowsPublishFailure {
    pub state: PublicationState,
    pub destination: PathBuf,
    pub retained_stage: Option<PathBuf>,
    pub detail: String,
}

impl std::fmt::Display for WindowsPublishFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Windows payload publication {:?}: {}",
            self.state, self.detail
        )?;
        if let Some(stage) = &self.retained_stage {
            write!(f, "; staging retained at {}", stage.display())?;
        }
        Ok(())
    }
}
impl std::error::Error for WindowsPublishFailure {}

/// Populate a fresh sibling and publish through the local no-replace operation.
/// This function never invokes signing or changes the installed owner tree.
pub fn assemble_windows_payload<T>(
    destination: &Path,
    build: impl FnOnce(&Path) -> Result<T, String>,
) -> Result<(T, PublishedWindowsPayload), WindowsPublishFailure> {
    #[cfg(windows)]
    {
        assemble_with(destination, build, native::move_no_replace)
    }
    #[cfg(not(windows))]
    {
        let _ = build;
        Err(failure(
            destination,
            "Windows payload publication requires a Windows host",
        ))
    }
}

fn failure(destination: &Path, detail: impl Into<String>) -> WindowsPublishFailure {
    WindowsPublishFailure {
        state: PublicationState::NotPublished,
        destination: destination.to_owned(),
        retained_stage: None,
        detail: detail.into(),
    }
}

#[cfg(any(windows, test))]
fn assemble_with<T>(
    destination: &Path,
    build: impl FnOnce(&Path) -> Result<T, String>,
    publish: impl FnOnce(&Path, &Path) -> Result<(), String>,
) -> Result<(T, PublishedWindowsPayload), WindowsPublishFailure> {
    let parent = destination
        .parent()
        .filter(|p| p.is_absolute())
        .ok_or_else(|| failure(destination, "destination must have an absolute parent"))?;
    let leaf = destination
        .file_name()
        .ok_or_else(|| failure(destination, "destination must name a directory"))?;
    let canonical_parent = parent
        .canonicalize()
        .map_err(|e| failure(destination, format!("resolve destination parent: {e}")))?;
    let destination = canonical_parent.join(leaf);
    match fs::symlink_metadata(&destination) {
        Ok(_) => return Err(failure(&destination, "destination already exists")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(failure(&destination, format!("inspect destination: {e}"))),
    }
    let stage = tempfile::Builder::new()
        .prefix(".solstone-payload-")
        .tempdir_in(&canonical_parent)
        .map_err(|e| failure(&destination, format!("create staging directory: {e}")))?;
    let value = match build(stage.path()) {
        Ok(value) => value,
        Err(detail) => return Err(cleanup_failure(stage, &destination, detail)),
    };
    // The injected callback is private and only used by host-independent tests.
    // Production always executes MoveFileExW(..., 0), which preserves late peers.
    if let Err(detail) = publish(stage.path(), &destination) {
        return Err(cleanup_failure(stage, &destination, detail));
    }
    let stage_name = stage.keep();
    let confirmed = match (
        fs::symlink_metadata(&stage_name),
        fs::symlink_metadata(&destination),
    ) {
        (Err(e), Ok(metadata)) if e.kind() == std::io::ErrorKind::NotFound => {
            plain_directory(&metadata)
        }
        _ => false,
    };
    if !confirmed {
        return Err(WindowsPublishFailure {
            state: PublicationState::Unconfirmed,
            destination,
            retained_stage: Some(stage_name),
            detail: "native move returned success; final directory observations are unconfirmed"
                .into(),
        });
    }
    Ok((
        value,
        PublishedWindowsPayload {
            destination,
            durability_proven: false,
        },
    ))
}

#[cfg(any(windows, test))]
fn plain_directory(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.is_dir() && metadata.file_attributes() & 0x400 == 0
    }
    #[cfg(not(windows))]
    {
        metadata.is_dir() && !metadata.file_type().is_symlink()
    }
}

#[cfg(any(windows, test))]
fn cleanup_failure(
    stage: tempfile::TempDir,
    destination: &Path,
    detail: String,
) -> WindowsPublishFailure {
    let stage_name = stage.path().to_owned();
    match stage.close() {
        Ok(()) => failure(destination, detail),
        Err(cleanup) => WindowsPublishFailure {
            state: PublicationState::NotPublished,
            destination: destination.to_owned(),
            retained_stage: Some(stage_name),
            detail: format!("{detail}; staging cleanup failed: {cleanup}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preparation_failure_preserves_original_error_and_never_publishes() {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("payload");
        let error = assemble_with::<()>(
            &destination,
            |stage| {
                fs::write(stage.join("partial"), b"partial").unwrap();
                Err("native admission refused".into())
            },
            |_, _| panic!("must not publish"),
        )
        .unwrap_err();
        assert_eq!(error.state, PublicationState::NotPublished);
        assert_eq!(error.detail, "native admission refused");
        assert!(!destination.exists());
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    }

    #[test]
    fn preexisting_destination_is_preserved_before_build() {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("payload");
        fs::create_dir(&destination).unwrap();
        let error = assemble_with::<()>(
            &destination,
            |_| panic!("must not build"),
            |_, _| panic!("must not publish"),
        )
        .unwrap_err();
        assert_eq!(error.state, PublicationState::NotPublished);
        assert!(destination.is_dir());
    }

    #[test]
    fn late_competitor_survives_a_failed_publication() {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("payload");
        let error = assemble_with(
            &destination,
            |_| Ok(()),
            |_, dest| {
                fs::create_dir(dest).unwrap();
                Err("native no-replace refused".into())
            },
        )
        .unwrap_err();
        assert!(destination.is_dir());
        assert_eq!(error.state, PublicationState::NotPublished);
        assert_eq!(error.detail, "native no-replace refused");
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }

    #[test]
    fn unconfirmed_move_keeps_stage_and_destination_for_reconciliation() {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("payload");
        let error = assemble_with(&destination, |_| Ok(()), |_, _| Ok(())).unwrap_err();
        assert_eq!(error.state, PublicationState::Unconfirmed);
        assert!(error.retained_stage.unwrap().is_dir());
    }

    #[cfg(windows)]
    #[test]
    fn native_late_empty_destination_is_never_replaced() {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("payload");
        let error = assemble_with(
            &destination,
            |stage| {
                fs::write(stage.join("file"), b"new").unwrap();
                Ok(())
            },
            |stage, dest| {
                fs::create_dir(dest).unwrap();
                native::move_no_replace(stage, dest)
            },
        )
        .unwrap_err();
        assert_eq!(error.state, PublicationState::NotPublished);
        assert!(destination.is_dir());
        assert_eq!(fs::read_dir(&destination).unwrap().count(), 0);
    }

    #[cfg(windows)]
    #[test]
    fn native_publication_preserves_bytes_without_a_durability_claim() {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("payload");
        let (_, published) = assemble_windows_payload(&destination, |stage| {
            fs::write(stage.join("file"), b"payload").map_err(|e| e.to_string())
        })
        .unwrap();
        assert!(!published.durability_proven);
        assert_eq!(
            fs::read(published.destination.join("file")).unwrap(),
            b"payload"
        );
    }
}
