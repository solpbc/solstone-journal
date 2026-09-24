// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use solstone_core_sol_link::ledger::{
    AuthorizationLedger, AuthorizedClientsMutationError, RemoveOutcome,
};

pub(crate) fn remove_paired_device(
    journal_root: &Path,
    cid: &str,
) -> Result<RemoveOutcome, AuthorizedClientsMutationError> {
    let outcome = AuthorizationLedger::new(journal_root).remove(cid)?;
    if let Err(_error) = solstone_core_push::remove_cid_registrations(journal_root, cid) {
        log::error!("push registrations for an unpaired device could not be removed");
    }
    Ok(outcome)
}
