// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Oura consent adaptation owned by the native body-ingest crate.

use std::path::Path;

use solstone_core_body_ingest::{
    BodyIngestError, BodyIngestErrorKind, oura_scheduled_sync_guidance,
};

use crate::ImportError;

/// Read-only standing scheduled Oura guidance for a later renderer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScheduledSyncGuidance {
    pub cadence: String,
    pub valid_until: String,
}

pub fn read_oura_scheduled_sync_guidance(
    journal: &Path,
) -> Result<Option<ScheduledSyncGuidance>, ImportError> {
    oura_scheduled_sync_guidance(journal)
        .map(|guidance| {
            guidance.map(|guidance| ScheduledSyncGuidance {
                cadence: guidance.cadence().to_owned(),
                valid_until: guidance.valid_until().to_owned(),
            })
        })
        .map_err(|error| body_error_to_import(error, true))
}

/// Preserve native error classes while giving the importer a uniform error type.
pub fn body_error_to_import(error: BodyIngestError, scheduled: bool) -> ImportError {
    let exit_code = match error.kind() {
        BodyIngestErrorKind::Gate => 2,
        BodyIngestErrorKind::Source | BodyIngestErrorKind::Normalize => 65,
        BodyIngestErrorKind::Publication | BodyIngestErrorKind::Rebuild => 74,
    };
    let message = match error.stage() {
        "per_run_confirmation_missing" if scheduled => {
            "Oura scheduled save requires standing scheduled_sync consent".to_owned()
        }
        "per_run_confirmation_missing" => {
            "Oura save requires --confirm-body-save; use --scheduled only with standing scheduled_sync consent".to_owned()
        }
        "scheduled_sync_consent_missing" | "scheduled_sync_not_approved" => {
            "Oura scheduled save requires standing scheduled_sync consent".to_owned()
        }
        "scheduled_sync_cadence_invalid"
        | "scheduled_sync_valid_until_missing"
        | "scheduled_sync_valid_until_invalid"
        | "scheduled_sync_consent_expired" => {
            "Oura scheduled save requires valid, unexpired scheduled_sync consent".to_owned()
        }
        stage => format!("Oura body operation failed: {stage}"),
    };
    ImportError::Refusal {
        kind: error.stage(),
        exit_code,
        message,
    }
}

pub fn oura_save_refusal(error: BodyIngestError) -> ImportError {
    body_error_to_import(error, false)
}
