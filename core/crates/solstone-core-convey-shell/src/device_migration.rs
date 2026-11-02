// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Private takeover sequencer joining the link, segment, and push owners.

use std::path::Path;

use serde_json::Value;
use solstone_core_segment::{TakeoverPlan, with_takeover_stream_boundary};
use solstone_core_sol_link::device_migration::{
    DecisionApply, DecisionRequest, DecisionResponse, MigrationError, MigrationReasonCode, decide,
    pending_push_cid, record_push_result,
};

pub(crate) fn decide_and_resume(
    journal: &Path,
    caller_cid: &str,
    raw_body: &[u8],
) -> Result<DecisionResponse, MigrationError> {
    let mut boundary = |adopted: &str,
                        retired: &str,
                        stored_plan: Option<&Value>,
                        apply: &mut DecisionApply<'_>| {
        with_takeover_stream_boundary(journal, adopted, retired, |guard| {
            let plan = match stored_plan {
                Some(value) => serde_json::from_value::<TakeoverPlan>(value.clone())
                    .map_err(|_| segment_error())?,
                None => guard.plan(adopted, retired),
            };
            let plan_value = serde_json::to_value(&plan).map_err(|_| segment_error())?;
            let mut publish = |value: &Value| {
                let plan = serde_json::from_value::<TakeoverPlan>(value.clone())
                    .map_err(|_| unavailable())?;
                guard.publish(&plan).map_err(|_| unavailable())
            };
            Ok(apply(plan_value, &mut publish))
        })
        .map_err(|_| unavailable())?
    };

    let view = decide(journal, caller_cid, raw_body, &mut boundary)?;
    let request: DecisionRequest = serde_json::from_slice(raw_body).map_err(|_| unavailable())?;
    let Some(retired_cid) = pending_push_cid(journal, caller_cid, &request.operation_id)? else {
        return Ok(view);
    };
    crate::paired_device::end_device_notices(journal, &retired_cid);
    if solstone_core_push::remove_cid_registrations(journal, &retired_cid).is_err() {
        return record_push_result(journal, caller_cid, &request.operation_id, false);
    }
    record_push_result(journal, caller_cid, &request.operation_id, true)
}

fn unavailable() -> MigrationError {
    MigrationError {
        reason: MigrationReasonCode::MigrationStateUnavailable,
    }
}

fn segment_error() -> solstone_core_segment::SegmentError {
    solstone_core_segment::SegmentError::StreamInput("invalid stored takeover plan")
}

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use std::fs;

    use serde_json::{Value, json};
    use solstone_core_sol_link::device_migration::{Choice, DecisionRequest};
    use solstone_core_sol_link::ledger::{
        AuthorizationLedger, AuthorizedClientsRead, ClientEntry, ClientRole,
    };

    use super::decide_and_resume;

    const OLD: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const ADOPTED: &str = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const OPERATION: &str = "123e4567-e89b-42d3-a456-426614174000";
    const DECISION: &str = "123e4567-e89b-42d3-a456-426614174010";

    fn prepare(journal: &std::path::Path) -> Vec<u8> {
        let mut ledger = AuthorizationLedger::new(journal);
        ledger
            .add(ClientEntry::new(
                OLD,
                "Old phone",
                "2026-09-01T00:00:00Z",
                "fixture-instance",
                ClientRole::Roleless,
            ))
            .unwrap();
        ledger
            .add(ClientEntry::new(
                ADOPTED,
                "New phone",
                "2026-09-01T00:00:00Z",
                "fixture-instance",
                ClientRole::Roleless,
            ))
            .unwrap();
        let operation_dir = journal.join("link/device-migrations/operations");
        fs::create_dir_all(&operation_dir).unwrap();
        fs::write(
            operation_dir.join(format!("{OPERATION}.json")),
            serde_json::to_vec(&json!({
                "caller_cid": OLD,
                "operation_id": OPERATION,
                "raw_body": [],
                "issued_pem": "stored-certificate",
                "issued_cid": ADOPTED,
                "previous_cid": OLD,
                "pairing": {"client_cert": "stored-certificate"},
                "checkpoint": "issued",
                "generation": 1,
                "order": 1
            }))
            .unwrap(),
        )
        .unwrap();
        serde_json::to_vec(&DecisionRequest {
            protocol_version: 1,
            operation_id: DECISION.to_owned(),
            choice: Choice::SameDevice,
            replaces_cid: solstone_core_sol_link::device_migration::ReplacesCid::Missing,
        })
        .unwrap()
    }

    #[test]
    fn migration_push_removal_failure_stays_awaiting_push() {
        let journal = tempfile::TempDir::new().unwrap();
        let body = prepare(journal.path());
        fs::create_dir_all(journal.path().join("config")).unwrap();
        let push_path = journal.path().join("config/push-registry.json");
        fs::write(&push_path, b"{ malformed").unwrap();
        let before = fs::read(&push_path).unwrap();

        let error = decide_and_resume(journal.path(), ADOPTED, &body).unwrap_err();

        assert_eq!(error.reason_code(), "migration_state_unavailable");
        assert_eq!(fs::read(&push_path).unwrap(), before);
        let decision_path = journal
            .path()
            .join(format!("link/device-migrations/decisions/{DECISION}.json"));
        let decision: Value = serde_json::from_slice(&fs::read(&decision_path).unwrap()).unwrap();
        assert_eq!(decision["checkpoint"], "awaiting_push");
        assert_eq!(decision["replaced_cid"], OLD);

        fs::write(&push_path, br#"{"version":2,"devices":[]}"#).unwrap();
        let resumed = decide_and_resume(journal.path(), ADOPTED, &body).unwrap();
        assert_eq!(resumed.state.as_wire(), "same_device");
        let decision: Value = serde_json::from_slice(&fs::read(&decision_path).unwrap()).unwrap();
        assert_eq!(decision["checkpoint"], "complete");
    }

    #[test]
    fn same_device_resume_retires_only_the_stored_previous_cid() {
        let journal = tempfile::TempDir::new().unwrap();
        let body = prepare(journal.path());
        let push_path = journal.path().join("config/push-registry.json");
        fs::create_dir_all(push_path.parent().unwrap()).unwrap();
        fs::write(&push_path, br#"{"version":2,"devices":[]}"#).unwrap();

        let resumed = decide_and_resume(journal.path(), ADOPTED, &body).unwrap();

        assert_eq!(resumed.state.as_wire(), "same_device");
        let entries = match AuthorizationLedger::new(journal.path()).read_state() {
            AuthorizedClientsRead::Present(entries) => entries,
            other => panic!("authorization ledger is not present: {other:?}"),
        };
        let operation_path = journal.path().join(format!(
            "link/device-migrations/operations/{OPERATION}.json"
        ));
        let operation: Value = serde_json::from_slice(&fs::read(operation_path).unwrap()).unwrap();
        let previous_cid = operation["previous_cid"]
            .as_str()
            .expect("stored previous CID");
        assert_eq!(previous_cid, OLD);
        assert!(
            !entries
                .iter()
                .any(|entry| entry.fingerprint == previous_cid)
        );
        assert!(entries.iter().any(|entry| entry.fingerprint == ADOPTED));
        let decision_path = journal
            .path()
            .join(format!("link/device-migrations/decisions/{DECISION}.json"));
        let decision: Value = serde_json::from_slice(&fs::read(decision_path).unwrap()).unwrap();
        assert_eq!(decision["replaced_cid"], previous_cid);
        assert_eq!(decision["checkpoint"], "complete");
    }

    #[cfg(all(test, feature = "full-tests"))]
    mod replacement_proof_tests {
        use std::fs;

        use axum::body::{Body, to_bytes};
        use axum::http::{Request, StatusCode, header};
        use serde_json::{Value, json};
        use sha2::{Digest, Sha256};
        use solstone_core_callosum::CallosumSocketServer;
        use solstone_core_convey_http::identity::{AccessBasis, Carrier, LinkedDeviceCid};
        use solstone_core_segment::list_stream_records_tolerant;
        use solstone_core_sol_link::device_migration::{Choice, DecisionRequest, ReplacesCid};
        use solstone_core_sol_link::ledger::{AuthorizationLedger, ClientEntry, ClientRole};
        use tower::ServiceExt;

        use super::decide_and_resume;

        const CID_A: &str =
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        const CID_B: &str =
            "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

        fn basis(cid: &str) -> AccessBasis {
            AccessBasis::LinkedDevice {
                carrier: Carrier::Direct,
                cid: LinkedDeviceCid::try_from(cid).expect("valid test cid"),
                leaf_spki: vec![0x30, 0x00],
            }
        }

        fn multipart_one(envelope: Value, name: &str, bytes: &[u8]) -> (String, Vec<u8>) {
            let boundary = "device-migration-test-boundary";
            let mut body = Vec::new();
            body.extend_from_slice(
                format!(
                    "--{boundary}\r\nContent-Disposition: form-data; name=\"envelope\"\r\n\r\n{envelope}\r\n"
                )
                .as_bytes(),
            );
            body.extend_from_slice(
                format!(
                    "--{boundary}\r\nContent-Disposition: form-data; name=\"files\"; filename=\"{name}\"\r\n\r\n"
                )
                .as_bytes(),
            );
            body.extend_from_slice(bytes);
            body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
            (format!("multipart/form-data; boundary={boundary}"), body)
        }

        fn multipart_two(
            envelope: Value,
            name1: &str,
            bytes1: &[u8],
            name2: &str,
            bytes2: &[u8],
        ) -> (String, Vec<u8>) {
            let boundary = "device-migration-test-boundary";
            let mut body = Vec::new();
            body.extend_from_slice(
                format!(
                    "--{boundary}\r\nContent-Disposition: form-data; name=\"envelope\"\r\n\r\n{envelope}\r\n"
                )
                .as_bytes(),
            );
            body.extend_from_slice(
                format!(
                    "--{boundary}\r\nContent-Disposition: form-data; name=\"files\"; filename=\"{name1}\"\r\n\r\n"
                )
                .as_bytes(),
            );
            body.extend_from_slice(bytes1);
            body.extend_from_slice(b"\r\n");
            body.extend_from_slice(
                format!(
                    "--{boundary}\r\nContent-Disposition: form-data; name=\"files\"; filename=\"{name2}\"\r\n\r\n"
                )
                .as_bytes(),
            );
            body.extend_from_slice(bytes2);
            body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
            (format!("multipart/form-data; boundary={boundary}"), body)
        }

        async fn request(
            app: &axum::Router,
            method: &str,
            uri: &str,
            cid: &str,
            body: Vec<u8>,
            content_type: Option<String>,
        ) -> (StatusCode, Value) {
            let mut req = Request::builder()
                .method(method)
                .uri(uri)
                .body(Body::from(body))
                .expect("request");
            req.headers_mut().insert(
                "X-Solstone-Protocol-Version",
                header::HeaderValue::from_static("3"),
            );
            if let Some(content_type) = content_type {
                req.headers_mut().insert(
                    header::CONTENT_TYPE,
                    content_type.parse().expect("content type"),
                );
            }
            req.extensions_mut().insert(basis(cid));
            let response = app.clone().oneshot(req).await.expect("response");
            let status = response.status();
            let body = to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body");
            (
                status,
                serde_json::from_slice(&body).expect("JSON response"),
            )
        }

        fn digest(bytes: &[u8]) -> String {
            format!("{:x}", Sha256::digest(bytes))
        }

        #[tokio::test]
        async fn explicit_replace_lists_ancestor_and_keeps_custody() {
            let journal = tempfile::TempDir::new().unwrap();
            let root = journal.path();
            let mut ledger = AuthorizationLedger::new(root);
            ledger
                .add(ClientEntry::new(
                    CID_A,
                    "Device A",
                    "2026-09-01T00:00:00Z",
                    "fixture-instance",
                    ClientRole::Roleless,
                ))
                .unwrap();
            ledger
                .add(ClientEntry::new(
                    CID_B,
                    "Device B",
                    "2026-09-01T00:00:00Z",
                    "fixture-instance",
                    ClientRole::Roleless,
                ))
                .unwrap();

            let push_path = root.join("config/push-registry.json");
            fs::create_dir_all(push_path.parent().unwrap()).unwrap();
            fs::write(&push_path, br#"{"version":2,"devices":[]}"#).unwrap();

            fs::create_dir_all(root.join("health")).unwrap();
            let socket = root.join("health/callosum.sock");
            #[cfg(windows)]
            let _namespace =
                solstone_core_callosum::test_fixture::WindowsPipeNamespace::register(&socket);
            let _callosum = CallosumSocketServer::bind(&socket)
                .await
                .expect("Callosum server");
            let app = solstone_core_ingest::api_router(root);

            let day = "20260805";
            let seg_a = "150000_60";
            let source = "mic";
            let audio_bytes = b"distinct audio flac content";
            let screen_bytes = b"distinct screen mp4 content";
            let (ct, body) = multipart_two(
                json!({
                    "day": day,
                    "segment": seg_a,
                    "source": source,
                    "files": [{"submitted": "audio.flac"}, {"submitted": "screen.mp4"}],
                }),
                "audio.flac",
                audio_bytes,
                "screen.mp4",
                screen_bytes,
            );
            let (status, upload_resp) =
                request(&app, "POST", "/app/devices/ingest", CID_A, body, Some(ct)).await;
            assert_eq!(status, StatusCode::OK, "{upload_resp}");

            let mut seg_dir: Option<std::path::PathBuf> = None;
            for stream_entry in fs::read_dir(root.join("chronicle").join(day)).unwrap() {
                let p = stream_entry.unwrap().path().join(seg_a);
                if p.is_dir() {
                    seg_dir = Some(p);
                    break;
                }
            }
            let seg_dir = seg_dir.expect("segment directory exists");
            let screen_path = seg_dir.join("screen.mp4");
            assert!(screen_path.is_file());
            fs::remove_file(&screen_path).unwrap();
            let sidecar_content = format!(
                "{}\n",
                json!({
                    "_solstone_processing": {
                        "schema": "solstone.processing.v1",
                        "state": "analyzed",
                        "handler": "describe",
                        "input_size": screen_bytes.len(),
                    }
                })
            );
            fs::write(seg_dir.join("screen.jsonl"), sidecar_content).unwrap();

            let (status, listing_a) = request(
                &app,
                "GET",
                &format!("/app/devices/ingest/segments/{day}?source={source}"),
                CID_A,
                Vec::new(),
                None,
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            let items_a = listing_a["items"].as_array().expect("items array");
            assert_eq!(items_a.len(), 1);
            assert_eq!(items_a[0]["key"], seg_a);
            let files_a = items_a[0]["files"].as_array().expect("files array");
            let audio_file_a = files_a.iter().find(|f| f["name"] == "audio.flac").unwrap();
            let screen_file_a = files_a.iter().find(|f| f["name"] == "screen.mp4").unwrap();

            assert_eq!(audio_file_a["status"], "present");
            assert_eq!(audio_file_a["size"], audio_bytes.len());
            assert_eq!(audio_file_a["sha256"], digest(audio_bytes));
            assert_eq!(fs::read(seg_dir.join("audio.flac")).unwrap(), audio_bytes);

            assert_eq!(screen_file_a["status"], "processed");
            assert_eq!(screen_file_a["size"], screen_bytes.len());
            assert_eq!(screen_file_a["sha256"], digest(screen_bytes));
            assert!(!screen_path.exists());

            let body_b = serde_json::to_vec(&DecisionRequest {
                protocol_version: 1,
                operation_id: "123e4567-e89b-42d3-a456-426614174099".to_owned(),
                choice: Choice::ReplaceDevice,
                replaces_cid: ReplacesCid::Cid(CID_A.to_owned()),
            })
            .unwrap();
            let decision_resp = decide_and_resume(root, CID_B, &body_b).unwrap();
            assert_eq!(decision_resp.state.as_wire(), "replaced_device");

            let (status, listing_b_pre) = request(
                &app,
                "GET",
                &format!("/app/devices/ingest/segments/{day}?source={source}"),
                CID_B,
                Vec::new(),
                None,
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            let items_b_pre = listing_b_pre["items"].as_array().expect("items array");
            assert_eq!(items_b_pre.len(), 1);
            assert_eq!(items_b_pre[0]["key"], seg_a);
            let files_b_pre = items_b_pre[0]["files"].as_array().expect("files array");
            let audio_b_pre = files_b_pre
                .iter()
                .find(|f| f["name"] == "audio.flac")
                .unwrap();
            let screen_b_pre = files_b_pre
                .iter()
                .find(|f| f["name"] == "screen.mp4")
                .unwrap();
            assert_eq!(audio_b_pre["status"], "present");
            assert_eq!(audio_b_pre["size"], audio_bytes.len());
            assert_eq!(audio_b_pre["sha256"], digest(audio_bytes));
            assert_eq!(screen_b_pre["status"], "processed");
            assert_eq!(screen_b_pre["size"], screen_bytes.len());
            assert_eq!(screen_b_pre["sha256"], digest(screen_bytes));

            let seg_b1 = "150100_60";
            let b_bytes = b"b segment content";
            let (ct, body) = multipart_one(
                json!({
                    "day": day,
                    "segment": seg_b1,
                    "source": source,
                    "files": [{"submitted": "audio.flac"}],
                }),
                "audio.flac",
                b_bytes,
            );
            let (status, upload_b) = request(
                &app,
                "POST",
                "/app/devices/ingest",
                CID_B,
                body.clone(),
                Some(ct.clone()),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{upload_b}");

            let a_stream = fs::read_to_string(seg_dir.join("stream.json")).unwrap();
            let a_marker: Value = serde_json::from_str(&a_stream).unwrap();
            let a_stream_name = a_marker["stream"].as_str().unwrap();

            let b_seg_dir = root
                .join("chronicle")
                .join(day)
                .join(a_stream_name)
                .join(seg_b1);
            assert!(b_seg_dir.join("stream.json").is_file());
            let b_marker_before: Value =
                serde_json::from_str(&fs::read_to_string(b_seg_dir.join("stream.json")).unwrap())
                    .unwrap();
            let marker_seq_before = b_marker_before["seq"].as_u64().unwrap();
            assert_eq!(marker_seq_before, 2);

            let records_before = list_stream_records_tolerant(root).unwrap();
            assert!(records_before.anomalies.is_empty());
            assert_eq!(records_before.records.len(), 1);
            assert_eq!(records_before.records[0].0, a_stream_name);
            let record_before = &records_before.records[0].1;
            let record_seq_before = record_before["seq"].as_u64().unwrap();
            assert_eq!(record_seq_before, 2);

            let (status, retry_b) =
                request(&app, "POST", "/app/devices/ingest", CID_B, body, Some(ct)).await;
            assert_eq!(status, StatusCode::OK, "{retry_b}");
            assert_eq!(retry_b["status"], "duplicate");

            let b_marker_after: Value =
                serde_json::from_str(&fs::read_to_string(b_seg_dir.join("stream.json")).unwrap())
                    .unwrap();
            assert_eq!(b_marker_after["seq"], marker_seq_before);
            assert_eq!(b_marker_after["stream"], a_stream_name);

            let records_after = list_stream_records_tolerant(root).unwrap();
            assert!(records_after.anomalies.is_empty());
            assert_eq!(records_after.records.len(), 1);
            assert_eq!(records_after.records[0].0, a_stream_name);
            let record_after = &records_after.records[0].1;
            assert_eq!(record_after["seq"], record_seq_before);

            let seg_b2 = "160000_60";
            let (ct, body) = multipart_one(
                json!({
                    "day": day,
                    "segment": seg_b2,
                    "source": source,
                    "files": [{"submitted": "screen.mp4"}],
                }),
                "screen.mp4",
                screen_bytes,
            );
            let (status, upload_b2) =
                request(&app, "POST", "/app/devices/ingest", CID_B, body, Some(ct)).await;
            assert_eq!(status, StatusCode::OK, "{upload_b2}");

            assert!(!screen_path.exists());
            assert_eq!(fs::read(seg_dir.join("audio.flac")).unwrap(), audio_bytes);

            let (status, listing_after) = request(
                &app,
                "GET",
                &format!("/app/devices/ingest/segments/{day}?source={source}"),
                CID_B,
                Vec::new(),
                None,
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            let items_after = listing_after["items"].as_array().expect("items array");
            let item_150000 = items_after.iter().find(|it| it["key"] == seg_a).unwrap();
            let files_150000 = item_150000["files"].as_array().expect("files");
            let screen_150000 = files_150000
                .iter()
                .find(|f| f["name"] == "screen.mp4")
                .unwrap();
            assert_eq!(screen_150000["status"], "processed");
            assert_eq!(screen_150000["size"], screen_bytes.len());
            assert_eq!(screen_150000["sha256"], digest(screen_bytes));
            let audio_150000 = files_150000
                .iter()
                .find(|f| f["name"] == "audio.flac")
                .unwrap();
            assert_eq!(audio_150000["status"], "present");
            assert_eq!(audio_150000["size"], audio_bytes.len());
            assert_eq!(audio_150000["sha256"], digest(audio_bytes));
        }

        #[tokio::test]
        async fn new_device_keeps_identical_bytes_on_separate_streams() {
            let journal = tempfile::TempDir::new().unwrap();
            let root = journal.path();
            let mut ledger = AuthorizationLedger::new(root);
            ledger
                .add(ClientEntry::new(
                    CID_A,
                    "Device A",
                    "2026-09-01T00:00:00Z",
                    "fixture-instance",
                    ClientRole::Roleless,
                ))
                .unwrap();
            ledger
                .add(ClientEntry::new(
                    CID_B,
                    "Device B",
                    "2026-09-01T00:00:00Z",
                    "fixture-instance",
                    ClientRole::Roleless,
                ))
                .unwrap();

            let push_path = root.join("config/push-registry.json");
            fs::create_dir_all(push_path.parent().unwrap()).unwrap();
            fs::write(&push_path, br#"{"version":2,"devices":[]}"#).unwrap();

            let body_b = serde_json::to_vec(&DecisionRequest {
                protocol_version: 1,
                operation_id: "123e4567-e89b-42d3-a456-426614174098".to_owned(),
                choice: Choice::NewDevice,
                replaces_cid: ReplacesCid::Missing,
            })
            .unwrap();
            let decision_resp = decide_and_resume(root, CID_B, &body_b).unwrap();
            assert_eq!(decision_resp.state.as_wire(), "new_device");

            fs::create_dir_all(root.join("health")).unwrap();
            let socket = root.join("health/callosum.sock");
            #[cfg(windows)]
            let _namespace =
                solstone_core_callosum::test_fixture::WindowsPipeNamespace::register(&socket);
            let _callosum = CallosumSocketServer::bind(&socket)
                .await
                .expect("Callosum server");
            let app = solstone_core_ingest::api_router(root);

            let day = "20260805";
            let source = "mic";
            let identical_bytes = b"identical sound bytes";

            let (ct_a, body_a) = multipart_one(
                json!({
                    "day": day,
                    "segment": "120000_60",
                    "source": source,
                    "files": [{"submitted": "audio.flac"}],
                }),
                "audio.flac",
                identical_bytes,
            );
            let (status, upload_a) = request(
                &app,
                "POST",
                "/app/devices/ingest",
                CID_A,
                body_a,
                Some(ct_a),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{upload_a}");

            let (ct_b, body_b) = multipart_one(
                json!({
                    "day": day,
                    "segment": "120000_60",
                    "source": source,
                    "files": [{"submitted": "audio.flac"}],
                }),
                "audio.flac",
                identical_bytes,
            );
            let (status, upload_b) = request(
                &app,
                "POST",
                "/app/devices/ingest",
                CID_B,
                body_b,
                Some(ct_b),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{upload_b}");

            let records = list_stream_records_tolerant(root).unwrap();
            assert!(records.anomalies.is_empty());
            assert_eq!(records.records.len(), 2);
            let mut physical_paths = Vec::new();
            for (name, _) in &records.records {
                let path = root
                    .join("chronicle")
                    .join(day)
                    .join(name)
                    .join("120000_60");
                assert_eq!(fs::read(path.join("audio.flac")).unwrap(), identical_bytes);
                physical_paths.push(path);
            }
            assert_ne!(physical_paths[0], physical_paths[1]);

            let (status, listing_a) = request(
                &app,
                "GET",
                &format!("/app/devices/ingest/segments/{day}?source={source}"),
                CID_A,
                Vec::new(),
                None,
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            let items_a = listing_a["items"].as_array().expect("items array");
            assert_eq!(items_a.len(), 1);
            assert_eq!(items_a[0]["key"], "120000_60");
            let files_a = items_a[0]["files"].as_array().unwrap();
            assert_eq!(files_a.len(), 1);
            assert_eq!(files_a[0]["name"], "audio.flac");
            assert_eq!(files_a[0]["size"], identical_bytes.len());
            assert_eq!(files_a[0]["sha256"], digest(identical_bytes));
            assert_eq!(files_a[0]["status"], "present");

            let (status, listing_b) = request(
                &app,
                "GET",
                &format!("/app/devices/ingest/segments/{day}?source={source}"),
                CID_B,
                Vec::new(),
                None,
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            let items_b = listing_b["items"].as_array().expect("items array");
            assert_eq!(items_b.len(), 1);
            assert_eq!(items_b[0]["key"], "120000_60");
            let files_b = items_b[0]["files"].as_array().unwrap();
            assert_eq!(files_b.len(), 1);
            assert_eq!(files_b[0]["name"], "audio.flac");
            assert_eq!(files_b[0]["size"], identical_bytes.len());
            assert_eq!(files_b[0]["sha256"], digest(identical_bytes));
            assert_eq!(files_b[0]["status"], "present");
        }
    }
}
