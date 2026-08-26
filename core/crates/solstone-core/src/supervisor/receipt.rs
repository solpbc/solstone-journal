// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Typed terminal receipts for the hosted-supervisor fixture boundary.

use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use solstone_core_journal_io::{AtomicWriteError, JsonWriteOptions, write_json};

use super::SupervisorHostOutcome;

pub const HOSTED_SUPERVISOR_RECEIPT_SCHEMA_V1: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostedSupervisorReceipt {
    pub schema: u32,
    pub nonce: String,
    pub outcome: SupervisorHostOutcome,
}

#[derive(Debug)]
pub enum HostedSupervisorReceiptReadError {
    Missing {
        path: PathBuf,
    },
    Read {
        path: PathBuf,
        source: io::Error,
    },
    Malformed {
        path: PathBuf,
        source: serde_json::Error,
    },
    SchemaMismatch {
        expected: u32,
        found: u32,
    },
}

impl fmt::Display for HostedSupervisorReceiptReadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing { path } => {
                write!(
                    formatter,
                    "hosted supervisor receipt is missing at {}",
                    path.display()
                )
            }
            Self::Read { path, source } => {
                write!(
                    formatter,
                    "could not read hosted supervisor receipt at {}: {source}",
                    path.display()
                )
            }
            Self::Malformed { path, source } => {
                write!(
                    formatter,
                    "malformed hosted supervisor receipt at {}: {source}",
                    path.display()
                )
            }
            Self::SchemaMismatch { expected, found } => {
                write!(formatter, "expected schema {expected}, found {found}")
            }
        }
    }
}

impl Error for HostedSupervisorReceiptReadError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            Self::Malformed { source, .. } => Some(source),
            Self::Missing { .. } | Self::SchemaMismatch { .. } => None,
        }
    }
}

pub fn write_hosted_supervisor_receipt(
    path: &Path,
    nonce: &str,
    outcome: &SupervisorHostOutcome,
) -> Result<(), AtomicWriteError> {
    let receipt = HostedSupervisorReceipt {
        schema: HOSTED_SUPERVISOR_RECEIPT_SCHEMA_V1,
        nonce: nonce.to_owned(),
        outcome: outcome.clone(),
    };
    write_json(
        path,
        &receipt,
        JsonWriteOptions {
            mode: Some(0o600),
            indent: Some(2),
            sort_keys: true,
        },
    )
}

pub fn read_hosted_supervisor_receipt(
    path: &Path,
) -> Result<HostedSupervisorReceipt, HostedSupervisorReceiptReadError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(source) if source.kind() == io::ErrorKind::NotFound => {
            return Err(HostedSupervisorReceiptReadError::Missing {
                path: path.to_path_buf(),
            });
        }
        Err(source) => {
            return Err(HostedSupervisorReceiptReadError::Read {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    let receipt = serde_json::from_slice::<HostedSupervisorReceipt>(&bytes).map_err(|source| {
        HostedSupervisorReceiptReadError::Malformed {
            path: path.to_path_buf(),
            source,
        }
    })?;
    if receipt.schema != HOSTED_SUPERVISOR_RECEIPT_SCHEMA_V1 {
        return Err(HostedSupervisorReceiptReadError::SchemaMismatch {
            expected: HOSTED_SUPERVISOR_RECEIPT_SCHEMA_V1,
            found: receipt.schema,
        });
    }
    Ok(receipt)
}
