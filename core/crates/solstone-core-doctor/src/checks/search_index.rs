// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Reports the search index health value. Read-only: the index is measured
//! against the disk, never opened for writing.

use solstone_core_system_health::{IndexFailure, IndexHealthState, evaluate_index_health};

use crate::context::CheckContext;
use crate::vocabulary::{Check, RunnerResult, Status, make_result};

pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    let health = evaluate_index_health(&context.journal_path, context.now);
    let (status, fix) = match (health.state, health.failure) {
        (IndexHealthState::Failing, Some(failure)) => (Status::Warn, fix_for(failure)),
        (IndexHealthState::Failing, None) => (Status::Warn, None),
        _ => (Status::Ok, None),
    };
    Ok(make_result(check, status, health.text, fix))
}

fn fix_for(failure: IndexFailure) -> Option<&'static str> {
    match failure {
        IndexFailure::MembershipsMissing => {
            Some("run solstone journal indexer --reset --rescan-full")
        }
        IndexFailure::ClassificationStalled => {
            Some("run solstone journal indexer --rescan and check its warnings")
        }
        IndexFailure::FailedFiles => Some("run solstone journal indexer --rescan"),
        IndexFailure::Unreadable => Some(
            "run solstone journal indexer status for details; if it persists, run solstone journal indexer --reset --rescan-full",
        ),
        IndexFailure::NewerGeneration => Some("update solstone to the newest version"),
    }
}
