// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::PathBuf;

use thiserror::Error;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GrabFailureClass {
    Usage,
    Runtime,
}

#[derive(Debug, Error)]
pub enum GrabFailure {
    #[error("{0}")]
    Usage(String),
    #[error("{0}")]
    Runtime(String),
    #[error("failed to access {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl GrabFailure {
    pub fn class(&self) -> GrabFailureClass {
        match self {
            Self::Usage(_) => GrabFailureClass::Usage,
            Self::Runtime(_) | Self::Io { .. } => GrabFailureClass::Runtime,
        }
    }

    pub(crate) fn runtime(message: impl Into<String>) -> Self {
        Self::Runtime(message.into())
    }
}
