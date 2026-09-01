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

/// Admitted `chronicle/<day>/health` directory for one local day.
pub struct OplogDayHealth {
    day: String,
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
    root: &JournalRoot,
    day: &str,
) -> Result<OplogDayHealth, OplogNamespaceError> {
    if !is_day_key(day) {
        return Err(OplogNamespaceError::new(
            OplogNamespaceStage::Day,
            OplogNamespaceClass::Unsafe,
        ));
    }

    #[cfg(unix)]
    {
        let chronicle = create_or_open_flat_directory_bound(
            root,
            OsStr::new(CHRONICLE_DIR),
            DIRECTORY_MODE,
            root.canonical_path(),
        )
        .map_err(|error| map_flat_directory_error(OplogNamespaceStage::Chronicle, error))?;
        let day_dir = create_or_open_flat_directory_bound(
            &chronicle,
            OsStr::new(day),
            DIRECTORY_MODE,
            chronicle.diagnostic_path(),
        )
        .map_err(|error| map_flat_directory_error(OplogNamespaceStage::Day, error))?;
        let health = create_or_open_flat_directory_bound(
            &day_dir,
            OsStr::new(HEALTH_DIR),
            DIRECTORY_MODE,
            day_dir.diagnostic_path(),
        )
        .map_err(|error| map_flat_directory_error(OplogNamespaceStage::Health, error))?;
        Ok(OplogDayHealth {
            day: day.to_owned(),
            health,
        })
    }
    #[cfg(windows)]
    {
        let chronicle = create_or_open_windows_flat_directory_bound(
            root,
            OsStr::new(CHRONICLE_DIR),
            root.canonical_path(),
        )
        .map_err(|error| map_flat_directory_error(OplogNamespaceStage::Chronicle, error))?;
        let day_dir = create_or_open_windows_flat_directory_bound(
            &chronicle,
            OsStr::new(day),
            chronicle.diagnostic_path(),
        )
        .map_err(|error| map_flat_directory_error(OplogNamespaceStage::Day, error))?;
        let health = create_or_open_windows_flat_directory_bound(
            &day_dir,
            OsStr::new(HEALTH_DIR),
            day_dir.diagnostic_path(),
        )
        .map_err(|error| map_flat_directory_error(OplogNamespaceStage::Health, error))?;
        Ok(OplogDayHealth {
            day: day.to_owned(),
            health,
        })
    }
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
