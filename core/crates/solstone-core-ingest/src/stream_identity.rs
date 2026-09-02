// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use axum::http::StatusCode;
use solstone_core_segment::{
    BoundStream, PairedStreamBase, SegmentError, StreamAllocationBase, StreamHints,
    bind_named_stream, bind_paired_stream, lookup_stream,
};
use solstone_core_sol_link::ledger::AuthorizationLedger;
use solstone_core_sol_link::{ClientLabelState, PairingIdentity, PlatformState};

use crate::model::ReasonCode;

/// Bind the stream this device should write, without advancing the chain.
pub(crate) fn bind_ingest_stream(
    journal: &Path,
    day: &str,
    segment: &str,
    cid: &str,
    source: &str,
    hints: &StreamHints,
) -> Result<BoundStream, (ReasonCode, StatusCode, String)> {
    match lookup_stream(journal, cid, source) {
        Ok(Some(name)) => {
            return map_named_bind(bind_named_stream(
                journal, day, segment, &name, cid, source, hints,
            ));
        }
        Ok(None) => {}
        Err(_) => {
            return Err((
                ReasonCode::JournalReadFailed,
                StatusCode::INTERNAL_SERVER_ERROR,
                "cannot resolve journal stream".to_owned(),
            ));
        }
    }
    bind_first_paired_stream(journal, day, segment, cid, source, hints)
}

fn bind_first_paired_stream(
    journal: &Path,
    day: &str,
    segment: &str,
    cid: &str,
    source: &str,
    hints: &StreamHints,
) -> Result<BoundStream, (ReasonCode, StatusCode, String)> {
    let mut ledger = AuthorizationLedger::new(journal);
    match ledger.get_pairing_identity_fields(cid) {
        Err(_) => Err((
            ReasonCode::JournalReadFailed,
            StatusCode::INTERNAL_SERVER_ERROR,
            "cannot resolve pairing identity".to_owned(),
        )),
        Ok(None) => pairing_identity_unavailable(),
        Ok(Some(fields)) => match fields.projection() {
            PairingIdentity::Unavailable => pairing_identity_unavailable(),
            PairingIdentity::Available {
                client_label,
                platform,
            } => match (&client_label, platform) {
                (ClientLabelState::Valid(label), _) => map_named_bind(bind_paired_stream(
                    journal,
                    day,
                    segment,
                    &PairedStreamBase {
                        origin: StreamAllocationBase::ClientLabel,
                        input: label,
                    },
                    cid,
                    source,
                    hints,
                )),
                (_, PlatformState::Valid(platform)) => map_named_bind(bind_paired_stream(
                    journal,
                    day,
                    segment,
                    &PairedStreamBase {
                        origin: StreamAllocationBase::Platform,
                        input: platform.as_wire(),
                    },
                    cid,
                    source,
                    hints,
                )),
                _ => map_named_bind(bind_paired_stream(
                    journal,
                    day,
                    segment,
                    &PairedStreamBase {
                        origin: StreamAllocationBase::Device,
                        input: "device",
                    },
                    cid,
                    source,
                    hints,
                )),
            },
        },
    }
}

fn pairing_identity_unavailable() -> Result<BoundStream, (ReasonCode, StatusCode, String)> {
    Err((
        ReasonCode::PairingIdentityUnavailable,
        StatusCode::CONFLICT,
        "pairing identity is unavailable".to_owned(),
    ))
}

pub(crate) fn map_named_bind(
    result: Result<BoundStream, SegmentError>,
) -> Result<BoundStream, (ReasonCode, StatusCode, String)> {
    match result {
        Ok(bound) => Ok(bound),
        Err(SegmentError::StreamBindingConflict { name }) => Err((
            ReasonCode::ForeignStreamBinding,
            StatusCode::CONFLICT,
            format!("stream {name} is bound to another device"),
        )),
        Err(SegmentError::StreamInput(_)) => Err((
            ReasonCode::MalformedEvidenceRow,
            StatusCode::CONFLICT,
            "stream binding has an unsupported shape".to_owned(),
        )),
        Err(SegmentError::StreamAllocationExhausted { .. }) => Err((
            ReasonCode::SegmentAllocationFailed,
            StatusCode::INTERNAL_SERVER_ERROR,
            "stream allocation attempts exhausted".to_owned(),
        )),
        Err(_) => Err((
            ReasonCode::JournalWriteFailed,
            StatusCode::INTERNAL_SERVER_ERROR,
            "cannot resolve journal stream".to_owned(),
        )),
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use std::fs;
    use std::path::Path;

    use axum::http::StatusCode;
    use serde_json::{Value, json};
    use solstone_core_segment::{Kind, StreamHints, StreamRecord};
    use solstone_core_sol_link::Platform;
    use solstone_core_sol_link::ledger::{AuthorizationLedger, ClientEntry, ClientRole};

    use super::bind_ingest_stream;
    use crate::model::ReasonCode;

    const CID: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const DAY: &str = "20260804";
    const SEGMENT: &str = "120000_1";

    fn hints() -> StreamHints {
        StreamHints {
            kind: Some(Kind::Observed),
            host: None,
            platform: None,
        }
    }

    fn journal() -> tempfile::TempDir {
        tempfile::TempDir::new_in("/var/tmp").expect("journal root")
    }

    fn bind(
        journal: &Path,
    ) -> Result<solstone_core_segment::BoundStream, (ReasonCode, StatusCode, String)> {
        bind_ingest_stream(journal, DAY, SEGMENT, CID, "", &hints())
    }

    fn stream_stems(journal: &Path) -> Vec<String> {
        let directory = journal.join("streams");
        match fs::read_dir(&directory) {
            Ok(entries) => entries
                .map(Result::unwrap)
                .map(|entry| entry.path())
                .filter(|path| {
                    path.extension()
                        .is_some_and(|extension| extension == "json")
                })
                .map(|path| path.file_stem().unwrap().to_str().unwrap().to_owned())
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    fn load_record(journal: &Path, name: &str) -> StreamRecord {
        serde_json::from_slice(
            &fs::read(journal.join("streams").join(format!("{name}.json"))).unwrap(),
        )
        .unwrap()
    }

    fn write_clients(journal: &Path, rows: Value) {
        let path = journal.join("link/authorized_clients.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, rows.to_string()).unwrap();
    }

    fn client_object(extra: Value) -> Value {
        let mut object = json!({
            "fingerprint": CID,
            "device_label": "phone",
            "paired_at": "2026-01-01T00:00:00Z",
            "instance_id": "i",
        });
        let extra = extra.as_object().unwrap();
        object.as_object_mut().unwrap().extend(extra.clone());
        object
    }

    #[test]
    fn valid_client_label_becomes_the_paired_base() {
        let temporary = journal();
        write_clients(
            temporary.path(),
            json!([client_object(json!({"client_label": "Desk.01"}))]),
        );
        let bound = bind(temporary.path()).unwrap();
        assert_eq!(bound.stream, "desk_01");
        let record = load_record(temporary.path(), "desk_01");
        let allocation = record.allocation.unwrap();
        assert_eq!(allocation.base_input, "Desk.01");
        assert_eq!(allocation.source, "");
        assert_eq!(allocation.collision, None);
        assert!(matches!(
            allocation.base,
            solstone_core_segment::StreamAllocationBase::ClientLabel
        ));
    }

    #[test]
    fn absent_label_with_valid_platform_uses_the_wire_token() {
        let temporary = journal();
        write_clients(
            temporary.path(),
            json!([client_object(json!({"platform": "linux"}))]),
        );
        let bound = bind(temporary.path()).unwrap();
        assert_eq!(bound.stream, "linux");
        let allocation = load_record(temporary.path(), "linux").allocation.unwrap();
        assert_eq!(allocation.base_input, Platform::Linux.as_wire());
        assert!(matches!(
            allocation.base,
            solstone_core_segment::StreamAllocationBase::Platform
        ));
    }

    #[test]
    fn empty_and_unprojectable_labels_fall_through_to_platform_then_device() {
        let temporary = journal();
        write_clients(
            temporary.path(),
            json!([client_object(
                json!({"client_label": "", "platform": "ios"})
            )]),
        );
        let bound = bind(temporary.path()).unwrap();
        assert_eq!(bound.stream, "ios");
        assert!(matches!(
            load_record(temporary.path(), "ios")
                .allocation
                .unwrap()
                .base,
            solstone_core_segment::StreamAllocationBase::Platform
        ));

        let temporary = journal();
        write_clients(
            temporary.path(),
            json!([client_object(json!({"client_label": "a".repeat(254)}))]),
        );
        let bound = bind(temporary.path()).unwrap();
        assert_eq!(bound.stream, "device");
        assert!(matches!(
            load_record(temporary.path(), "device")
                .allocation
                .unwrap()
                .base,
            solstone_core_segment::StreamAllocationBase::Device
        ));
    }

    #[test]
    fn malformed_pairing_identity_refuses_without_writing() {
        for extra in [
            json!({"client_label": 1}),
            json!({"platform": "plan9"}),
            json!({"platform": true}),
        ] {
            let temporary = journal();
            write_clients(temporary.path(), json!([client_object(extra)]));
            let error = bind(temporary.path()).unwrap_err();
            assert_eq!(error.0, ReasonCode::PairingIdentityUnavailable);
            assert_eq!(error.1, StatusCode::CONFLICT);
            assert!(stream_stems(temporary.path()).is_empty());
        }
    }

    #[test]
    fn missing_ledger_row_refuses_without_writing() {
        let temporary = journal();
        write_clients(temporary.path(), json!([]));
        let error = bind(temporary.path()).unwrap_err();
        assert_eq!(error.0, ReasonCode::PairingIdentityUnavailable);
        assert_eq!(error.1, StatusCode::CONFLICT);
        assert!(stream_stems(temporary.path()).is_empty());
    }

    #[test]
    fn unreadable_ledger_is_a_journal_read_failure() {
        let temporary = journal();
        let path = temporary.path().join("link/authorized_clients.json");
        fs::create_dir_all(&path).unwrap();
        let error = bind(temporary.path()).unwrap_err();
        assert_eq!(error.0, ReasonCode::JournalReadFailed);
        assert_eq!(error.1, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(stream_stems(temporary.path()).is_empty());
    }

    #[test]
    fn existing_binding_reuses_the_name_without_reading_the_ledger() {
        let temporary = journal();
        let record = json!({
            "name": "desk_01",
            "kind": "observer",
            "host": null,
            "platform": null,
            "created_at": 1,
            "last_day": null,
            "last_segment": null,
            "seq": 0,
            "cid": CID,
            "source": "",
        });
        let path = temporary.path().join("streams/desk_01.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, record.to_string()).unwrap();
        let bound = bind(temporary.path()).expect("hit path does not consult the ledger");
        assert_eq!(bound.stream, "desk_01");
        assert!(!temporary.path().join("link").exists());
    }

    #[test]
    fn ledger_add_round_trip_still_projects_a_valid_label() {
        let temporary = journal();
        let mut entry = ClientEntry::new(
            CID,
            "phone",
            "2026-01-01T00:00:00Z",
            "i",
            ClientRole::Roleless,
        );
        entry.client_label = "studio-mac".to_owned();
        AuthorizationLedger::new(temporary.path())
            .add(entry)
            .unwrap();
        let bound = bind(temporary.path()).unwrap();
        assert_eq!(bound.stream, "studio-mac");
    }
}
