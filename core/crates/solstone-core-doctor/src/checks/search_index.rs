// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Reports the search index health value. Read-only: the index is measured
//! against the disk, never opened for writing.

use solstone_core_cli_copy as copy;

use solstone_core_system_health::{
    IndexFailure, IndexHealthState, evaluate_index_health_with_in, read_indexing_observations,
};

use crate::context::CheckContext;
use crate::vocabulary::{Check, RunnerResult, Status, make_result};

pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    let health = evaluate_index_health_with_in(
        &context.journal_path,
        &read_indexing_observations(&context.journal_path, context.now),
        solstone_core_window_catalog::cli_locale(),
    );
    let (status, fix) = match (health.state, health.failure) {
        (IndexHealthState::Failing, Some(failure)) => (Status::Warn, fix_for(failure)),
        (IndexHealthState::Failing, None) => (Status::Warn, None),
        // Rows for removed entries stay until a full rescan; nothing else removes
        // them. Warn only when that is what the detail line says.
        (IndexHealthState::Behind, None) if health.retained > 0 && health.pending == 0 => (
            Status::Warn,
            Some(copy::text("journal.cli.doctor.index.fix.full")),
        ),
        // The daily check has run since it last finished, two days ago or
        // more, without finishing: its own errors say why.
        (IndexHealthState::Behind, None) if health.check_overdue => (
            Status::Warn,
            Some(copy::text("journal.cli.doctor.index.fix.errors")),
        ),
        _ => (Status::Ok, None),
    };
    Ok(make_result(check, status, health.text, fix))
}

fn fix_for(failure: IndexFailure) -> Option<String> {
    match failure {
        IndexFailure::MembershipsMissing => Some(copy::text("journal.cli.doctor.index.fix.reset")),
        IndexFailure::StampUnreadable => Some(copy::text("journal.cli.doctor.index.fix.rebuild")),
        IndexFailure::DamageCheck => Some(copy::text("journal.cli.doctor.index.fix.checking")),
        IndexFailure::ClassificationStalled => {
            Some(copy::text("journal.cli.doctor.index.fix.warnings"))
        }
        IndexFailure::FailedFiles => Some(copy::text("journal.cli.doctor.index.fix.failed.files")),
        IndexFailure::FailedWork => Some(copy::text("journal.cli.doctor.index.fix.failed.work")),
        IndexFailure::FailedDeletions => Some(copy::text("journal.cli.doctor.index.fix.deletions")),
        IndexFailure::Unreadable => Some(copy::text("journal.cli.doctor.index.fix.rebuild")),
        IndexFailure::NewerGeneration => Some(copy::text("journal.cli.doctor.index.fix.upgrade")),
        IndexFailure::ScanFailed => Some(copy::text("journal.cli.doctor.index.fix.errors")),
    }
}
