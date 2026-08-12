// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Oura save-mode consent routing and presentation values.

use std::path::PathBuf;

use serde_json::{Map, Value};
use solstone_core_body_ingest::{BodyIngestError, OURA_CHECKLIST, OURA_PATH, oura_approval};

/// The process code a caller uses for a blocked Oura save gate.
pub const CONSENT_GATE_EXIT_CODE: i32 = 2;

/// Inputs forwarded exactly to the body-owned approval check.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConsentGateRequest {
    pub journal_root: PathBuf,
    pub confirmed: bool,
    pub scheduled: bool,
}

/// The named save-gate result; the library does not print or exit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConsentGateOutcome {
    Allowed,
    Blocked(GateFailure),
}

/// Presentation data derived from a body-owned refusal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GateFailure {
    error: BodyIngestError,
    journal_root: PathBuf,
    scheduled: bool,
}

impl GateFailure {
    #[must_use]
    pub fn reason(&self) -> &'static str {
        self.error.stage()
    }

    /// Render owner-facing remediation without performing output.
    #[must_use]
    pub fn format_text(&self) -> String {
        let flow = if self.scheduled {
            "scheduled"
        } else {
            "interactive"
        };
        format!(
            "Oura body sync was blocked before writing an import directory.\n\
Importer: oura\nTarget journal: {}\nReason: {}\n\
Approval artifact: {}\n\
1. Review the Oura raw-data retention choice.\n\
2. Record the approval artifact for this journal.\n\
3. Retry with confirmation for an interactive run.\n\
4. Record scheduled_sync consent for --scheduled runs.\n\
Flow: {flow}",
            self.journal_root.display(),
            self.reason(),
            self.journal_root.join(OURA_PATH).display(),
        )
    }

    /// The reference-compatible gate payload shape.
    #[must_use]
    pub fn to_python_payload(&self) -> Value {
        let mut payload = Map::new();
        payload.insert("skipped".to_owned(), Value::Bool(true));
        payload.insert(
            "reason".to_owned(),
            Value::String("consent_gate".to_owned()),
        );
        payload.insert(
            "gate_reason".to_owned(),
            Value::String(self.reason().to_owned()),
        );
        payload.insert("importer".to_owned(), Value::String("oura".to_owned()));
        payload.insert(
            "flow".to_owned(),
            Value::String(
                if self.scheduled {
                    "scheduled"
                } else {
                    "interactive"
                }
                .to_owned(),
            ),
        );
        payload.insert(
            "approval_path".to_owned(),
            Value::String(self.journal_root.join(OURA_PATH).display().to_string()),
        );
        payload.insert(
            "target_journal".to_owned(),
            Value::String(self.journal_root.display().to_string()),
        );
        // BodyIngestError has a stage but not field diagnostics; never invent them.
        payload.insert("missing_fields".to_owned(), Value::Array(Vec::new()));
        payload.insert("invalid_fields".to_owned(), Value::Array(Vec::new()));
        payload.insert(
            "checklist_version".to_owned(),
            Value::String(OURA_CHECKLIST.to_owned()),
        );
        Value::Object(payload)
    }
}

/// Check approval before any save-side body operation.
///
/// Unlike sync state, unreadable *consent* state refuses by returning `Blocked`.
#[must_use]
pub fn check_oura_sync_save(request: &ConsentGateRequest) -> ConsentGateOutcome {
    match oura_approval(&request.journal_root, request.confirmed, request.scheduled) {
        Ok(_) => ConsentGateOutcome::Allowed,
        Err(error) => ConsentGateOutcome::Blocked(GateFailure {
            error,
            journal_root: request.journal_root.clone(),
            scheduled: request.scheduled,
        }),
    }
}
