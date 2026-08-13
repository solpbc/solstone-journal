// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::error::Error;
use std::fmt;
use std::io;
use std::path::PathBuf;

#[derive(Debug)]
pub struct HealthDirectoryProbeError {
    pub path: PathBuf,
    pub source: io::Error,
}

impl fmt::Display for HealthDirectoryProbeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.path.display(), self.source)
    }
}

impl Error for HealthDirectoryProbeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.source)
    }
}

#[derive(Debug)]
pub enum OrdinaryTailError {
    InvalidUtf8 {
        path: PathBuf,
        source: std::string::FromUtf8Error,
    },
}

impl fmt::Display for OrdinaryTailError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUtf8 { path, source } => write!(formatter, "{}: {source}", path.display()),
        }
    }
}

impl Error for OrdinaryTailError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidUtf8 { source, .. } => Some(source),
        }
    }
}

#[derive(Debug)]
pub enum EnumerationError {
    Enumerate { path: PathBuf, source: io::Error },
}

impl fmt::Display for EnumerationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Enumerate { path, source } => write!(formatter, "{}: {source}", path.display()),
        }
    }
}

impl Error for EnumerationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Enumerate { source, .. } => Some(source),
        }
    }
}
