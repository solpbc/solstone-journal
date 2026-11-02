// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The owner notice for a paired device whose uploads the journal keeps
//! turning away.
//!
//! It rests only on the rejection streak the ledger already keeps for the
//! device and for each of its sources, read back through the same projection
//! the Health devices card shows. It is announced after every ingest outcome
//! the ledger records for the device, so a dropped line costs nothing: the
//! next upload says the same thing again.

use std::path::Path;

use serde_json::json;
use solstone_core_sol_link::client_status::{
    ClientActivityState, ClientAssessment, ClientInspection, inspect_clients_at,
};
use solstone_core_sol_link::ledger::IngestRejection;
use solstone_core_sol_link::{current_display_label, read_descriptions};

use crate::model::ReasonCode;

const FAMILY: &str = "ingest_rejected";

/// Where the Health page lists a device whose uploads are turned away.
const HEALTH_DEVICES: &str = "/app/health/#registeredClientsCard";

/// The record has no notion of time, only a streak that any accepted upload
/// ends. Two refusals with nothing accepted between them is the smallest
/// streak that shows the next upload did not fix the first.
const PERSISTENT_STREAK: u64 = 2;

const APP_TOO_OLD: [&str; 3] = [
    ReasonCode::ProtocolVersionLegacy.as_str(),
    ReasonCode::LegacyObserverField.as_str(),
    ReasonCode::LegacyStreamField.as_str(),
];
const JOURNAL_TOO_OLD: &str = ReasonCode::ProtocolVersionFuture.as_str();
const PAIRING_UNUSABLE: &str = ReasonCode::PairingIdentityUnavailable.as_str();

/// The episode key for one paired device.
pub fn notice_key(cid: &str) -> String {
    format!("{FAMILY}:{cid}")
}

/// What the owner can do about a refusal that will repeat until they act.
///
/// Every other reason stays quiet. The journal's own write, read, lock,
/// stream and notify failures are not the owner's to fix on the device; a
/// malformed, conflicting or removed upload is about that upload, not the
/// device; a missing or unreadable protocol version does not say which app
/// sent it. Health still lists every one of them.
fn owner_fix(reason_code: &str) -> Option<&'static str> {
    if APP_TOO_OLD.contains(&reason_code) {
        Some("its app is too old for this journal. update the app on it.")
    } else if reason_code == JOURNAL_TOO_OLD {
        Some("its app is newer than this journal. update your journal.")
    } else if reason_code == PAIRING_UNUSABLE {
        Some("its pairing can't be used anymore. pair it again.")
    } else {
        None
    }
}

/// The framed Callosum line to announce for `cid` after the ledger recorded
/// an ingest outcome for it, or `None` when there is nothing to say.
///
/// A raise needs a refusal the owner can fix, repeated without an accepted
/// upload between. A clear needs the device and every one of its sources to
/// hold no active refusal. A refusal the owner cannot fix leaves any earlier
/// episode as it is, and a record that cannot be read says nothing.
pub(crate) fn announcement(journal: &Path, cid: &str, now_ms: i64) -> Option<String> {
    let ClientInspection::Ready {
        clients,
        activity: ClientActivityState::Present,
    } = inspect_clients_at(journal, now_ms)
    else {
        return None;
    };
    let client = clients.iter().find(|client| client.cid == cid)?;
    let rejections = active_rejections(client);
    let label = device_label(journal, client);
    let row = if rejections.is_empty() {
        json!({
            "tract": "owner_notice",
            "event": "clear",
            "key": notice_key(cid),
            "title": format!("your journal is accepting uploads from {label} again"),
        })
    } else {
        let fix = rejections
            .iter()
            .filter(|rejection| rejection.active_count >= PERSISTENT_STREAK)
            .find_map(|rejection| owner_fix(&rejection.reason_code))?;
        json!({
            "tract": "owner_notice",
            "event": "raise",
            "key": notice_key(cid),
            "title": format!("your journal is turning away uploads from {label}"),
            "message": fix,
            "action": HEALTH_DEVICES,
        })
    };
    Some(format!("{row}\n"))
}

/// The device's own streak and each source's, as Health shows them.
fn active_rejections(client: &ClientAssessment) -> Vec<&IngestRejection> {
    client
        .ingest_rejection
        .iter()
        .chain(
            client
                .source_delivery
                .values()
                .filter_map(|row| row.ingest_rejection.as_ref()),
        )
        .collect()
}

fn device_label(journal: &Path, client: &ClientAssessment) -> String {
    let descriptions = read_descriptions(journal).unwrap_or_default();
    let label = current_display_label(&client.client_entry, descriptions.get(&client.cid));
    if label.trim().is_empty() {
        "one of your devices".to_owned()
    } else {
        label
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use serde_json::Value;
    use solstone_core_sol_link::ledger::ClientRole;
    use solstone_core_sol_link::ledger::{AcceptedSegment, AuthorizationLedger, ClientEntry};
    use tempfile::TempDir;

    use super::*;

    const CID: &str = "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
    const AT: &str = "2026-01-02T00:00:00Z";

    fn journal() -> TempDir {
        let journal = TempDir::new().unwrap();
        AuthorizationLedger::new(journal.path())
            .add(ClientEntry::new(
                CID,
                "kitchen tablet",
                "2026-01-01T00:00:00Z",
                "test-instance",
                ClientRole::Roleless,
            ))
            .unwrap();
        journal
    }

    fn reject(journal: &Path, code: &str, source: Option<&str>) {
        assert!(
            AuthorizationLedger::new(journal)
                .record_ingest_rejection(CID, AT, code, source)
                .unwrap()
        );
    }

    fn accept(journal: &Path, source: Option<&str>) {
        let segment = AcceptedSegment {
            day: "20260102".to_owned(),
            name: "120000_300".to_owned(),
        };
        assert!(
            AuthorizationLedger::new(journal)
                .record_accepted_ingest(CID, AT, segment, source)
                .unwrap()
        );
    }

    fn row(journal: &Path) -> Option<Value> {
        announcement(journal, CID, 0).map(|line| serde_json::from_str(&line).unwrap())
    }

    #[test]
    fn every_reason_the_owner_can_fix_raises_on_the_second_refusal() {
        for code in [
            ReasonCode::ProtocolVersionLegacy,
            ReasonCode::LegacyObserverField,
            ReasonCode::LegacyStreamField,
            ReasonCode::ProtocolVersionFuture,
            ReasonCode::PairingIdentityUnavailable,
        ] {
            let journal = journal();
            reject(journal.path(), code.as_str(), None);
            assert!(row(journal.path()).is_none(), "{code:?} once");
            reject(journal.path(), code.as_str(), None);
            let raised = row(journal.path()).expect("second refusal raises");
            assert_eq!(raised["tract"], "owner_notice");
            assert_eq!(raised["event"], "raise");
            assert_eq!(raised["key"], format!("ingest_rejected:{CID}"));
            assert_eq!(raised["action"], HEALTH_DEVICES);
            assert!(
                raised["title"].as_str().unwrap().contains("kitchen tablet"),
                "{code:?}"
            );
            assert!(!raised["message"].as_str().unwrap().is_empty(), "{code:?}");
        }
    }

    #[test]
    fn a_reason_the_owner_cannot_fix_says_nothing_however_long_it_lasts() {
        for code in [
            ReasonCode::JournalWriteFailed,
            ReasonCode::EventAppendFailed,
            ReasonCode::LocationLockUnavailable,
            ReasonCode::MultipartMalformed,
            ReasonCode::EnvelopeInvalid,
            ReasonCode::ContentConflict,
            ReasonCode::SegmentRemoved,
            ReasonCode::ForeignStreamBinding,
            ReasonCode::ProtocolVersionRequired,
            ReasonCode::ProtocolVersionMalformed,
        ] {
            let journal = journal();
            for _ in 0..5 {
                reject(journal.path(), code.as_str(), Some("audio"));
            }
            assert!(row(journal.path()).is_none(), "{code:?}");
        }
    }

    #[test]
    fn an_accepted_upload_clears_only_when_no_source_is_still_refused() {
        let journal = journal();
        reject(journal.path(), PAIRING_UNUSABLE, Some("location"));
        reject(journal.path(), PAIRING_UNUSABLE, Some("location"));
        assert_eq!(row(journal.path()).unwrap()["event"], "raise");

        // Audio lands, which ends the device-wide streak, but location is
        // still refused, so the episode still holds.
        accept(journal.path(), Some("audio"));
        assert_eq!(row(journal.path()).unwrap()["event"], "raise");

        accept(journal.path(), Some("location"));
        let cleared = row(journal.path()).expect("clear");
        assert_eq!(cleared["event"], "clear");
        assert_eq!(cleared["key"], format!("ingest_rejected:{CID}"));
        assert!(
            cleared["title"]
                .as_str()
                .unwrap()
                .contains("kitchen tablet")
        );
    }

    #[test]
    fn a_malformed_upload_that_the_next_one_fixes_ends_in_a_clear_and_no_raise() {
        let journal = journal();
        reject(
            journal.path(),
            ReasonCode::ProtocolVersionLegacy.as_str(),
            None,
        );
        assert!(row(journal.path()).is_none());
        accept(journal.path(), None);
        assert_eq!(row(journal.path()).unwrap()["event"], "clear");
    }

    #[test]
    fn an_unknown_device_or_unreadable_activity_says_nothing() {
        let journal = journal();
        assert!(announcement(journal.path(), "sha256:unknown", 0).is_none());
        std::fs::write(journal.path().join("link/devices.json"), b"{not json").unwrap();
        assert!(row(journal.path()).is_none());
    }

    #[test]
    fn the_owner_label_names_the_device() {
        let journal = journal();
        std::fs::write(
            journal.path().join("link/client-descriptions.json"),
            json!({CID: {"protocol_version": 1, "revision": 1, "reported": null,
                "owner_label": "garden phone", "updated_at": AT}})
            .to_string(),
        )
        .unwrap();
        reject(journal.path(), JOURNAL_TOO_OLD, None);
        reject(journal.path(), JOURNAL_TOO_OLD, None);
        let raised = row(journal.path()).unwrap();
        let title = raised["title"].as_str().unwrap();
        assert!(title.contains("garden phone"), "{title}");
        assert!(!title.contains("kitchen tablet"), "{title}");
    }
}
