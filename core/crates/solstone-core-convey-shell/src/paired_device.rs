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
    if outcome.authorized_removed {
        end_device_notices(journal_root, cid);
    }
    Ok(outcome)
}

/// A device that is no longer paired can no longer have its uploads turned
/// away, so its notice ends without a word: the owner just removed it.
pub(crate) fn end_device_notices(journal_root: &Path, cid: &str) {
    solstone_core_push::owner_notice::clear(
        journal_root,
        "",
        &solstone_core_ingest::ingest_rejected_notice_key(cid),
        None,
        time::OffsetDateTime::now_utc(),
    );
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use solstone_core_callosum::CallosumEnvelope;
    use solstone_core_sol_link::ledger::{ClientEntry, ClientRole};
    use time::OffsetDateTime;

    use super::*;

    const CID: &str = "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";

    fn raise(journal: &Path) -> bool {
        let envelope = CallosumEnvelope {
            tract: "owner_notice".to_owned(),
            event: "raise".to_owned(),
            ts: None,
            extra: json!({
                "key": solstone_core_ingest::ingest_rejected_notice_key(CID),
                "title": "uploads turned away",
            })
            .as_object()
            .cloned()
            .unwrap(),
        };
        crate::owner_notices::handle(journal, "", &envelope, OffsetDateTime::UNIX_EPOCH).is_some()
    }

    #[test]
    fn unpairing_a_device_ends_its_upload_notice() {
        let journal = tempfile::tempdir().unwrap();
        AuthorizationLedger::new(journal.path())
            .add(ClientEntry::new(
                CID,
                "Test device",
                "2026-01-01T00:00:00Z",
                "test-instance",
                ClientRole::Roleless,
            ))
            .unwrap();
        assert!(raise(journal.path()));
        assert!(!raise(journal.path()), "still active");

        assert!(
            remove_paired_device(journal.path(), CID)
                .unwrap()
                .authorized_removed
        );
        assert!(raise(journal.path()), "the episode ended with the pairing");
    }
}
