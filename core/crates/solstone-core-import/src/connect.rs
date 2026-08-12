// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Oura-only connection routing for the import library.

use std::path::Path;

use solstone_core_body_ingest::{OuraConnectOptions, connect_oura};

use crate::consent_gate::body_error_to_import;
use crate::{ImportError, SyncBackend};

/// Owner-facing result of a successful native backend connection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConnectOutcome {
    pub backend: SyncBackend,
    pub scopes: Vec<String>,
}

/// Connect the sole owner-present backend supported by this wave.
pub fn connect_backend(journal: &Path, backend: &str) -> Result<ConnectOutcome, ImportError> {
    if backend != "oura" {
        return Err(ImportError::Refusal {
            kind: "unsupported_connect_backend",
            exit_code: 2,
            message: format!("connect is only available for oura, not {backend}"),
        });
    }
    connect_oura(journal, &OuraConnectOptions::default())
        .map(|report| outcome_from_scopes(report.scopes()))
        .map_err(|error| body_error_to_import(error, false))
}

fn outcome_from_scopes(scopes: &[String]) -> ConnectOutcome {
    ConnectOutcome {
        backend: SyncBackend::Oura,
        scopes: scopes.to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthetic_scopes_map_to_connect_outcome() {
        let outcome = outcome_from_scopes(&["daily".to_owned(), "heartrate".to_owned()]);

        assert_eq!(outcome.backend, SyncBackend::Oura);
        assert_eq!(outcome.scopes, ["daily", "heartrate"]);
    }
}
