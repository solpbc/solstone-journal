// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Windows explicit archive-output selection.
//!
//! Publication itself is create-only and revalidates the destination through
//! journal-io's Windows path capability. This module intentionally keeps
//! acquisition lexical: canonicalizing a caller-controlled parent would follow
//! a reparse point before that capability can refuse it.

use std::fmt::{Display, Formatter};
use std::path::{Component, Path, PathBuf};

/// The caller-supplied explicit archive output path and its injected cwd.
pub struct ExplicitArchiveOutputRequest {
    output: PathBuf,
    cwd: PathBuf,
}

impl ExplicitArchiveOutputRequest {
    /// Construct an explicit output selection from caller-provided path state.
    pub fn new(output: PathBuf, cwd: PathBuf) -> Self {
        Self { output, cwd }
    }
}

/// A create-only archive output target. The destination is rechecked just
/// before publication so a caller cannot win a check-then-use race.
pub struct ArchiveOutputTarget {
    final_path: PathBuf,
}

impl ArchiveOutputTarget {
    /// Return the normalized absolute output path.
    pub fn final_path(&self) -> &Path {
        &self.final_path
    }

    /// Confirm that the final archive name is still unoccupied.
    pub fn revalidate(&self) -> Result<(), ExplicitTargetError> {
        match std::fs::symlink_metadata(&self.final_path) {
            Ok(_) => Err(ExplicitTargetError::Collision {
                path: self.final_path.clone(),
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(ExplicitTargetError::TargetIo {
                path: self.final_path.clone(),
                source,
            }),
        }
    }
}

/// Failure while selecting or revalidating an explicit archive output target.
#[derive(Debug)]
pub enum ExplicitTargetError {
    InvalidTarget {
        path: PathBuf,
        reason: &'static str,
    },
    UnsafeTarget {
        path: PathBuf,
        reason: &'static str,
    },
    Collision {
        path: PathBuf,
    },
    TargetIo {
        path: PathBuf,
        source: std::io::Error,
    },
    TargetChanged {
        path: PathBuf,
    },
}

impl Display for ExplicitTargetError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidTarget { path, reason } => {
                write!(
                    formatter,
                    "invalid archive output {}: {reason}",
                    path.display()
                )
            }
            Self::UnsafeTarget { path, reason } => {
                write!(
                    formatter,
                    "unsafe archive output {}: {reason}",
                    path.display()
                )
            }
            Self::Collision { path } => {
                write!(
                    formatter,
                    "archive output already exists: {}",
                    path.display()
                )
            }
            Self::TargetIo { path, source } => {
                write!(formatter, "archive output {}: {source}", path.display())
            }
            Self::TargetChanged { path } => {
                write!(formatter, "archive output changed: {}", path.display())
            }
        }
    }
}

impl std::error::Error for ExplicitTargetError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::TargetIo { source, .. } => Some(source),
            Self::InvalidTarget { .. }
            | Self::UnsafeTarget { .. }
            | Self::Collision { .. }
            | Self::TargetChanged { .. } => None,
        }
    }
}

/// Select a Windows create-only archive output target.
pub fn acquire_explicit_output_target(
    request: &ExplicitArchiveOutputRequest,
) -> Result<ArchiveOutputTarget, ExplicitTargetError> {
    let final_path = if request.output.is_absolute() {
        request.output.clone()
    } else {
        request.cwd.join(&request.output)
    };
    if !final_path.is_absolute() {
        return Err(ExplicitTargetError::InvalidTarget {
            path: final_path,
            reason: "must resolve to an absolute path",
        });
    }
    let Some(parent) = final_path.parent() else {
        return Err(ExplicitTargetError::InvalidTarget {
            path: final_path,
            reason: "has no parent directory",
        });
    };
    if parent.as_os_str().is_empty() || !normal_leaf(&final_path) {
        return Err(ExplicitTargetError::InvalidTarget {
            path: final_path,
            reason: "must name one normal file leaf",
        });
    }
    let target = ArchiveOutputTarget { final_path };
    target.revalidate()?;
    Ok(target)
}

fn normal_leaf(path: &Path) -> bool {
    matches!(path.components().next_back(), Some(Component::Normal(_)))
}
