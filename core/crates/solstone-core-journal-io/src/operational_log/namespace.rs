// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Chronicle/day/health chain for one operational-log day.

use std::error::Error;
use std::ffi::OsStr;
use std::fmt;
use std::io;

use crate::errors::FlatDirectoryError;
use crate::journal_root::JournalRoot;
use crate::paths::is_day_key;

#[cfg(unix)]
use crate::flat_directory::{FlatDirectory, create_or_open_flat_directory_bound};
#[cfg(windows)]
use crate::windows_sync_dir::{WindowsFlatDirectory, create_or_open_windows_flat_directory_bound};

const CHRONICLE_DIR: &str = "chronicle";
const HEALTH_DIR: &str = "health";
#[cfg(unix)]
const DIRECTORY_MODE: u32 = 0o700;

/// Ordered checkpoints along the chronicle → day → health admission chain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OplogNamespacePrimitive {
    /// After `chronicle/` is retained, before the day child lookup.
    AfterChronicle,
    /// After `chronicle/<day>/` is retained, before the health child lookup.
    AfterDay,
    /// After `chronicle/<day>/health/` is retained, before return to the caller.
    AfterHealth,
}

/// Admitted `chronicle/<day>/health` directory for one local day.
pub struct OplogDayHealth {
    day: String,
    root: JournalRoot,
    #[cfg(unix)]
    health: FlatDirectory,
    #[cfg(windows)]
    health: WindowsFlatDirectory,
}

impl fmt::Debug for OplogDayHealth {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OplogDayHealth")
            .field("day", &self.day)
            .finish_non_exhaustive()
    }
}

impl OplogDayHealth {
    /// Admitted YYYYMMDD day key.
    pub fn day(&self) -> &str {
        &self.day
    }

    /// Borrow the admitted day-health directory.
    #[cfg(unix)]
    pub fn health(&self) -> &FlatDirectory {
        &self.health
    }

    /// Borrow the admitted day-health directory.
    #[cfg(windows)]
    pub fn health(&self) -> &WindowsFlatDirectory {
        &self.health
    }

    /// Re-resolve `chronicle/<day>/health` from the retained journal root.
    ///
    /// This is a fresh name lookup, not `fstat` of the already-open health
    /// descriptor, so a path-level ancestor replacement is visible.
    pub fn revalidate_binding(&self) -> Result<(), OplogNamespaceError> {
        let fresh = admit_health_chain(&self.root, &self.day)?;
        if fresh.identity() != self.health.identity() {
            return Err(OplogNamespaceError::new(
                OplogNamespaceStage::Health,
                OplogNamespaceClass::IdentityChanged,
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OplogNamespaceStage {
    Chronicle,
    Day,
    Health,
}

impl OplogNamespaceStage {
    const fn token(self) -> &'static str {
        match self {
            Self::Chronicle => "chronicle",
            Self::Day => "day",
            Self::Health => "health",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OplogNamespaceClass {
    Unsafe,
    IdentityChanged,
    Io,
}

impl OplogNamespaceClass {
    const fn token(self) -> &'static str {
        match self {
            Self::Unsafe => "unsafe",
            Self::IdentityChanged => "identity_changed",
            Self::Io => "io",
        }
    }
}

/// Bounded failure while admitting chronicle/day/health.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct OplogNamespaceError {
    stage: OplogNamespaceStage,
    class: OplogNamespaceClass,
}

impl OplogNamespaceError {
    const fn new(stage: OplogNamespaceStage, class: OplogNamespaceClass) -> Self {
        Self { stage, class }
    }

    fn token(self) -> String {
        format!(
            "oplog_namespace_{}_{}",
            self.stage.token(),
            self.class.token()
        )
    }
}

impl fmt::Display for OplogNamespaceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.token())
    }
}

impl fmt::Debug for OplogNamespaceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl Error for OplogNamespaceError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        None
    }
}

/// Create or admit `chronicle/<day>/health` beneath `root`.
pub fn admit_day_health_directory(
    root: JournalRoot,
    day: &str,
) -> Result<OplogDayHealth, OplogNamespaceError> {
    if !is_day_key(day) {
        return Err(OplogNamespaceError::new(
            OplogNamespaceStage::Day,
            OplogNamespaceClass::Unsafe,
        ));
    }
    let health = admit_health_chain(&root, day)?;
    Ok(OplogDayHealth {
        day: day.to_owned(),
        root,
        health,
    })
}

#[cfg(unix)]
fn admit_health_chain(root: &JournalRoot, day: &str) -> Result<FlatDirectory, OplogNamespaceError> {
    let chronicle = create_or_open_flat_directory_bound(
        root,
        OsStr::new(CHRONICLE_DIR),
        DIRECTORY_MODE,
        root.canonical_path(),
    )
    .map_err(|error| map_flat_directory_error(OplogNamespaceStage::Chronicle, error))?;
    oplog_namespace_composition_checkpoint(OplogNamespacePrimitive::AfterChronicle)?;
    let day_dir = create_or_open_flat_directory_bound(
        &chronicle,
        OsStr::new(day),
        DIRECTORY_MODE,
        chronicle.diagnostic_path(),
    )
    .map_err(|error| map_flat_directory_error(OplogNamespaceStage::Day, error))?;
    oplog_namespace_composition_checkpoint(OplogNamespacePrimitive::AfterDay)?;
    let health = create_or_open_flat_directory_bound(
        &day_dir,
        OsStr::new(HEALTH_DIR),
        DIRECTORY_MODE,
        day_dir.diagnostic_path(),
    )
    .map_err(|error| map_flat_directory_error(OplogNamespaceStage::Health, error))?;
    oplog_namespace_composition_checkpoint(OplogNamespacePrimitive::AfterHealth)?;
    Ok(health)
}

#[cfg(windows)]
fn admit_health_chain(
    root: &JournalRoot,
    day: &str,
) -> Result<WindowsFlatDirectory, OplogNamespaceError> {
    let chronicle = create_or_open_windows_flat_directory_bound(
        root,
        OsStr::new(CHRONICLE_DIR),
        root.canonical_path(),
    )
    .map_err(|error| map_flat_directory_error(OplogNamespaceStage::Chronicle, error))?;
    oplog_namespace_composition_checkpoint(OplogNamespacePrimitive::AfterChronicle)?;
    let day_dir = create_or_open_windows_flat_directory_bound(
        &chronicle,
        OsStr::new(day),
        chronicle.diagnostic_path(),
    )
    .map_err(|error| map_flat_directory_error(OplogNamespaceStage::Day, error))?;
    oplog_namespace_composition_checkpoint(OplogNamespacePrimitive::AfterDay)?;
    let health = create_or_open_windows_flat_directory_bound(
        &day_dir,
        OsStr::new(HEALTH_DIR),
        day_dir.diagnostic_path(),
    )
    .map_err(|error| map_flat_directory_error(OplogNamespaceStage::Health, error))?;
    oplog_namespace_composition_checkpoint(OplogNamespacePrimitive::AfterHealth)?;
    Ok(health)
}

fn map_flat_directory_error(
    stage: OplogNamespaceStage,
    error: FlatDirectoryError,
) -> OplogNamespaceError {
    let class = match error {
        FlatDirectoryError::InvalidRelativePath { .. }
        | FlatDirectoryError::InvalidName { .. }
        | FlatDirectoryError::NotDirectory { .. }
        | FlatDirectoryError::SymlinkRefused { .. }
        | FlatDirectoryError::NotRegular { .. }
        | FlatDirectoryError::SizeLimitExceeded { .. } => OplogNamespaceClass::Unsafe,
        FlatDirectoryError::IdentityChanged { .. }
        | FlatDirectoryError::EnumerationChanged { .. } => OplogNamespaceClass::IdentityChanged,
        FlatDirectoryError::Io { source, .. } => match source.kind() {
            io::ErrorKind::AlreadyExists
            | io::ErrorKind::NotADirectory
            | io::ErrorKind::IsADirectory => OplogNamespaceClass::Unsafe,
            _ => OplogNamespaceClass::Io,
        },
    };
    OplogNamespaceError::new(stage, class)
}

fn oplog_namespace_composition_checkpoint(
    primitive: OplogNamespacePrimitive,
) -> Result<(), OplogNamespaceError> {
    #[cfg(any(test, feature = "test-hooks"))]
    {
        checkpoint_traced(primitive)
    }
    #[cfg(not(any(test, feature = "test-hooks")))]
    {
        let _ = primitive;
        Ok(())
    }
}

#[cfg(any(test, feature = "test-hooks"))]
struct OplogNamespaceTraceState {
    fault: Option<OplogNamespacePrimitive>,
    fault_consumed: bool,
    barriers: Vec<(OplogNamespacePrimitive, Box<dyn FnOnce()>)>,
}

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    static OPLOG_NAMESPACE_TRACE: std::cell::RefCell<Option<OplogNamespaceTraceState>> = const {
        std::cell::RefCell::new(None)
    };
}

#[cfg(any(test, feature = "test-hooks"))]
fn checkpoint_traced(primitive: OplogNamespacePrimitive) -> Result<(), OplogNamespaceError> {
    let (fault, callback) = OPLOG_NAMESPACE_TRACE.with(|trace| {
        let mut trace = trace.borrow_mut();
        let Some(state) = trace.as_mut() else {
            return (false, None);
        };
        if state.fault == Some(primitive) {
            state.fault = None;
            state.fault_consumed = true;
            (true, None)
        } else if let Some(index) = state
            .barriers
            .iter()
            .position(|(candidate, _)| *candidate == primitive)
        {
            let callback = state.barriers.remove(index).1;
            (false, Some(callback))
        } else {
            (false, None)
        }
    });
    if fault {
        let stage = match primitive {
            OplogNamespacePrimitive::AfterChronicle => OplogNamespaceStage::Chronicle,
            OplogNamespacePrimitive::AfterDay => OplogNamespaceStage::Day,
            OplogNamespacePrimitive::AfterHealth => OplogNamespaceStage::Health,
        };
        return Err(OplogNamespaceError::new(stage, OplogNamespaceClass::Io));
    }
    if let Some(callback) = callback {
        callback();
    }
    Ok(())
}

/// Run `operation` with one namespace-chain barrier callback.
#[cfg(any(test, feature = "test-hooks"))]
pub fn run_with_oplog_namespace_barrier<T>(
    primitive: OplogNamespacePrimitive,
    callback: impl FnOnce() + 'static,
    operation: impl FnOnce() -> T,
) -> T {
    with_namespace_trace(
        OplogNamespaceTraceState {
            fault: None,
            fault_consumed: false,
            barriers: vec![(primitive, Box::new(callback))],
        },
        operation,
        |_| true,
    )
    .0
}

/// Run `operation` with one injected I/O fault at `primitive`.
#[cfg(any(test, feature = "test-hooks"))]
pub fn run_with_oplog_namespace_fault<T>(
    primitive: OplogNamespacePrimitive,
    operation: impl FnOnce() -> T,
) -> (T, bool) {
    with_namespace_trace(
        OplogNamespaceTraceState {
            fault: Some(primitive),
            fault_consumed: false,
            barriers: Vec::new(),
        },
        operation,
        |state| state.fault_consumed,
    )
}

#[cfg(any(test, feature = "test-hooks"))]
fn with_namespace_trace<T>(
    state: OplogNamespaceTraceState,
    operation: impl FnOnce() -> T,
    consumed: impl FnOnce(&OplogNamespaceTraceState) -> bool,
) -> (T, bool) {
    OPLOG_NAMESPACE_TRACE.with(|trace| {
        assert!(
            trace.borrow().is_none(),
            "oplog namespace trace is already active"
        );
        *trace.borrow_mut() = Some(state);
    });
    let result = operation();
    let state = OPLOG_NAMESPACE_TRACE.with(|trace| {
        trace
            .borrow_mut()
            .take()
            .expect("oplog namespace trace remains active")
    });
    let flag = consumed(&state);
    (result, flag)
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::fs;
    use std::path::Path;

    use super::*;

    const DAY: &str = "20260901";

    fn temp() -> tempfile::TempDir {
        #[cfg(unix)]
        {
            tempfile::tempdir_in("/var/tmp").unwrap()
        }
        #[cfg(windows)]
        {
            tempfile::TempDir::new().unwrap()
        }
    }

    fn expect_token(error: OplogNamespaceError, token: &str) {
        assert_eq!(error.to_string(), token);
        assert_eq!(format!("{error:?}"), token);
        assert!(error.source().is_none());
    }

    fn chronicle_path(root: &Path) -> std::path::PathBuf {
        root.join(CHRONICLE_DIR)
    }

    fn day_path(root: &Path) -> std::path::PathBuf {
        chronicle_path(root).join(DAY)
    }

    fn health_path(root: &Path) -> std::path::PathBuf {
        day_path(root).join(HEALTH_DIR)
    }

    #[test]
    fn stable_directory_positive_control_admits_the_chain() {
        let temporary = temp();
        let root = temporary.path();
        let health = admit_day_health_directory(JournalRoot::open(root).unwrap(), DAY).unwrap();
        assert_eq!(health.day(), DAY);
        assert!(chronicle_path(root).is_dir());
        assert!(day_path(root).is_dir());
        assert!(health_path(root).is_dir());
        health.revalidate_binding().unwrap();
    }

    #[test]
    fn after_chronicle_wrong_kind_day_is_unsafe_without_health() {
        let temporary = temp();
        let root = temporary.path().to_path_buf();
        let error = run_with_oplog_namespace_barrier(
            OplogNamespacePrimitive::AfterChronicle,
            {
                let root = root.clone();
                move || {
                    fs::write(day_path(&root), b"not-a-directory").unwrap();
                }
            },
            || admit_day_health_directory(JournalRoot::open(&root).unwrap(), DAY),
        )
        .unwrap_err();
        expect_token(error, "oplog_namespace_day_unsafe");
        assert!(chronicle_path(&root).is_dir());
        assert!(day_path(&root).is_file());
        assert!(!health_path(&root).exists());
    }

    #[cfg(unix)]
    #[test]
    fn after_day_symlink_health_is_unsafe() {
        let temporary = temp();
        let root = temporary.path().to_path_buf();
        let error = run_with_oplog_namespace_barrier(
            OplogNamespacePrimitive::AfterDay,
            {
                let root = root.clone();
                move || {
                    std::os::unix::fs::symlink("outside", health_path(&root)).unwrap();
                }
            },
            || admit_day_health_directory(JournalRoot::open(&root).unwrap(), DAY),
        )
        .unwrap_err();
        expect_token(error, "oplog_namespace_health_unsafe");
        assert!(day_path(&root).is_dir());
        assert!(
            fs::symlink_metadata(health_path(&root))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(!root.join("outside").exists());
    }

    #[cfg(unix)]
    #[test]
    fn after_health_path_replacement_retains_the_original_directory() {
        let temporary = temp();
        let root = temporary.path().to_path_buf();
        let moved = root.join("health-moved");
        let health = run_with_oplog_namespace_barrier(
            OplogNamespacePrimitive::AfterHealth,
            {
                let root = root.clone();
                let moved = moved.clone();
                move || {
                    fs::rename(health_path(&root), &moved).unwrap();
                    fs::create_dir(health_path(&root)).unwrap();
                    fs::write(health_path(&root).join("replacement"), b"untouched").unwrap();
                }
            },
            || admit_day_health_directory(JournalRoot::open(&root).unwrap(), DAY),
        )
        .unwrap();
        assert!(moved.is_dir());
        assert_eq!(
            fs::read(health_path(&root).join("replacement")).unwrap(),
            b"untouched"
        );
        health.revalidate_binding().unwrap_err();
    }

    #[cfg(unix)]
    #[test]
    fn preexisting_symlink_chronicle_is_unsafe_and_does_not_create_the_target() {
        let temporary = temp();
        let root = temporary.path();
        std::os::unix::fs::symlink("elsewhere", chronicle_path(root)).unwrap();
        let error = admit_day_health_directory(JournalRoot::open(root).unwrap(), DAY).unwrap_err();
        expect_token(error, "oplog_namespace_chronicle_unsafe");
        assert!(!root.join("elsewhere").exists());
        assert!(
            fs::symlink_metadata(chronicle_path(root))
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn injected_fault_after_chronicle_is_io_and_leaves_no_day() {
        let temporary = temp();
        let root = temporary.path();
        let (result, consumed) =
            run_with_oplog_namespace_fault(OplogNamespacePrimitive::AfterChronicle, || {
                admit_day_health_directory(JournalRoot::open(root).unwrap(), DAY)
            });
        assert!(consumed);
        expect_token(result.unwrap_err(), "oplog_namespace_chronicle_io");
        assert!(chronicle_path(root).is_dir());
        assert!(!day_path(root).exists());
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use std::fs;
    use std::os::windows::fs::MetadataExt;
    use std::path::Path;
    use std::process::Command;

    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

    use super::*;

    const DAY: &str = "20260901";

    fn temp() -> tempfile::TempDir {
        tempfile::TempDir::new().unwrap()
    }

    fn create_junction(link: &Path, target: &Path) {
        let output = Command::new("cmd")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .expect("launch cmd.exe for native junction fixture");
        assert!(
            output.status.success(),
            "create junction fixture {} -> {}: status={} stdout={} stderr={}",
            link.display(),
            target.display(),
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    fn chronicle_path(root: &Path) -> std::path::PathBuf {
        root.join(CHRONICLE_DIR)
    }

    fn day_path(root: &Path) -> std::path::PathBuf {
        chronicle_path(root).join(DAY)
    }

    fn health_path(root: &Path) -> std::path::PathBuf {
        day_path(root).join(HEALTH_DIR)
    }

    fn assert_reparse(path: &Path) {
        assert_ne!(
            fs::symlink_metadata(path).unwrap().file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT,
            0
        );
    }

    #[test]
    fn after_chronicle_wrong_kind_day_is_unsafe_without_health() {
        let temporary = temp();
        let root = temporary.path().to_path_buf();
        let error = run_with_oplog_namespace_barrier(
            OplogNamespacePrimitive::AfterChronicle,
            {
                let root = root.clone();
                move || {
                    fs::write(day_path(&root), b"not-a-directory").unwrap();
                }
            },
            || admit_day_health_directory(JournalRoot::open(&root).unwrap(), DAY),
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "oplog_namespace_day_unsafe");
        assert!(chronicle_path(&root).is_dir());
        assert!(day_path(&root).is_file());
        assert!(!health_path(&root).exists());
    }

    #[test]
    fn after_day_junction_health_is_unsafe() {
        let temporary = temp();
        let root = temporary.path().to_path_buf();
        let target = root.join("junction-target");
        fs::create_dir(&target).unwrap();
        let error = run_with_oplog_namespace_barrier(
            OplogNamespacePrimitive::AfterDay,
            {
                let root = root.clone();
                let target = target.clone();
                move || {
                    create_junction(&health_path(&root), &target);
                }
            },
            || admit_day_health_directory(JournalRoot::open(&root).unwrap(), DAY),
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "oplog_namespace_health_unsafe");
        assert!(day_path(&root).is_dir());
        assert_reparse(&health_path(&root));
        assert!(
            fs::read_dir(&target).unwrap().next().is_none(),
            "admit must not create through the planted health junction"
        );
    }

    #[test]
    fn after_health_path_replacement_retains_the_original_directory() {
        let temporary = temp();
        let root = temporary.path().to_path_buf();
        let moved = root.join("health-moved");
        let health = run_with_oplog_namespace_barrier(
            OplogNamespacePrimitive::AfterHealth,
            {
                let root = root.clone();
                let moved = moved.clone();
                move || {
                    fs::rename(health_path(&root), &moved).unwrap();
                    fs::create_dir(health_path(&root)).unwrap();
                    fs::write(health_path(&root).join("replacement"), b"untouched").unwrap();
                }
            },
            || admit_day_health_directory(JournalRoot::open(&root).unwrap(), DAY),
        )
        .unwrap();
        assert!(moved.is_dir());
        assert_eq!(
            fs::read(health_path(&root).join("replacement")).unwrap(),
            b"untouched"
        );
        assert!(health.revalidate_binding().is_err());
    }

    #[test]
    fn preexisting_file_chronicle_is_unsafe() {
        let temporary = temp();
        let root = temporary.path();
        fs::write(chronicle_path(root), b"not-a-directory").unwrap();
        let error = admit_day_health_directory(JournalRoot::open(root).unwrap(), DAY).unwrap_err();
        assert_eq!(error.to_string(), "oplog_namespace_chronicle_unsafe");
        assert_eq!(fs::read(chronicle_path(root)).unwrap(), b"not-a-directory");
        assert!(!day_path(root).exists());
    }
}
