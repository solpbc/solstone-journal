// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    #![allow(
        clippy::unwrap_used,
        reason = "synthetic journals own all fixture bytes"
    )]

    use std::fs;
    use std::path::Path;

    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use serde_json::{Value, json};
    use solstone_core_journal_io::JournalRoot;
    use solstone_core_retention::archive_proof::{
        AcceptedProcessingObservation, ArchiveFileProof, OffloadOffer, OffloadProof,
    };
    use solstone_core_retention::marks::{
        Failure, Mark, MarkId, MarkState, Proposal, Register, RemovalClass,
    };
    use solstone_core_retention::receipt::Target;
    use solstone_core_retention::review_context::ReviewContext;
    use tower::ServiceExt;

    fn journal() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("config")).unwrap();
        fs::write(root.path().join("config/journal.json"), br#"{"setup":{"completed_at":1700000000000},"retention":{"raw_media":"days","raw_media_days":1}}"#).unwrap();
        fs::create_dir(root.path().join("chronicle")).unwrap();
        fs::write(root.path().join("chronicle/raw.flac"), b"original bytes").unwrap();
        root
    }

    fn mark(root: &Path, name: &str, bound: bool) -> Mark {
        let class = if bound {
            RemovalClass::OffloadRawRelease
        } else {
            RemovalClass::PolicyRawRelease
        };
        let target = Target {
            day: "20261009".into(),
            stream: "camera".into(),
            dir: "120000_60_suffix".into(),
        };
        let proposal = Proposal {
            bytes: 9_007_199_254_740_993,
            reason: "archive".into(),
            names: vec![name.into()],
        };
        let offload = bound.then(|| {
            OffloadOffer::new(OffloadProof {
                journal_id: JournalRoot::open(root).unwrap().identity().observation_id(),
                manifest_id: "manifest".into(),
                target: target.clone(),
                layout: solstone_core_journal_io::paths::SegmentLayout::Named,
                repository_id: "a".repeat(64),
                snapshot_id: "b".repeat(64),
                files: vec![ArchiveFileProof {
                    witness_id: "witness".into(),
                    name: name.into(),
                    bytes: proposal.bytes,
                    sha256: "c".repeat(64),
                    archive_path: format!("/stage/{name}"),
                    processing: AcceptedProcessingObservation {
                        record: Some(json!({
                            "schema": "solstone.processing.v1", "handler": "transcribe",
                            "state": "analyzed", "input_size": proposal.bytes,
                            "reason_code": "ok", "attempted_at": "2026-10-09T00:00:00Z",
                            "large": 9_007_199_254_740_993_u64,
                        })),
                        has_analysis_row: true,
                    },
                }],
            })
            .unwrap()
        });
        Mark {
            id: MarkId::derive(class, &target, &proposal.names),
            class,
            target,
            proposal,
            offload,
            marked_at: "2026-10-10T00:00:00Z".into(),
            state: MarkState::Marked,
        }
    }

    fn save(root: &Path, marks: &[Mark]) -> Vec<u8> {
        let mut register = Register::empty();
        if marks.iter().any(|mark| mark.offload.is_some()) {
            register.version = 2;
        }
        register.marks = marks
            .iter()
            .map(|mark| (mark.id.clone(), mark.clone()))
            .collect();
        let bytes = serde_json::to_vec(&register).unwrap();
        fs::create_dir_all(root.join("health")).unwrap();
        fs::write(root.join("health/retention-marks.json"), &bytes).unwrap();
        bytes
    }

    async fn call(root: &Path, method: &str, endpoint: &str, body: String) -> (StatusCode, Value) {
        let router = solstone_core_home_web::routes(
            root.to_path_buf(),
            solstone_core_home_web::Clock::fixed(
                jiff::tz::Offset::UTC
                    .to_timestamp(jiff::civil::date(2026, 10, 10).at(12, 0, 0, 0))
                    .unwrap(),
            ),
            |_| false,
        );
        let response = router
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(endpoint)
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 4 * 1024 * 1024)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    async fn list(root: &Path) -> Value {
        call(root, "GET", "/app/home/api/removals", String::new())
            .await
            .1
    }

    #[tokio::test]
    async fn current_review_carries_full_marks_and_same_root_restart_preserves_context() {
        let root = journal();
        let first = mark(root.path(), "first.flac", false);
        let second = mark(root.path(), "second.flac", true);
        let mut failed = mark(root.path(), "failed.flac", false);
        failed.state = MarkState::Failed(Failure {
            at: "now".into(),
            reason: "unfinished".into(),
            staged: None,
        });
        let before = save(root.path(), &[first.clone(), second.clone(), failed]);
        let response = list(root.path()).await;
        assert_eq!(response["state"], "list.ready");
        let context = response["review_context"].as_str().unwrap();
        let decoded: ReviewContext = serde_json::from_str(context).unwrap();
        assert_eq!(decoded.marks().len(), 3);
        assert!(decoded.marks().contains(&first));
        assert!(decoded.marks().contains(&second));
        assert!(context.contains("9007199254740993"));
        assert_eq!(response["removals"].as_array().unwrap().len(), 3);
        assert_eq!(list(root.path()).await["review_context"], context);
        assert_eq!(
            fs::read(root.path().join("health/retention-marks.json")).unwrap(),
            before
        );
    }

    #[tokio::test]
    async fn copied_policy_context_changes_and_copied_bound_offer_refuses() {
        let original = journal();
        let copied = journal();
        let policy = mark(original.path(), "policy.flac", false);
        let bytes = save(original.path(), &[policy]);
        fs::create_dir(copied.path().join("health")).unwrap();
        fs::write(copied.path().join("health/retention-marks.json"), &bytes).unwrap();
        assert_ne!(
            list(original.path()).await["review_context"],
            list(copied.path()).await["review_context"]
        );
        let bound = mark(original.path(), "bound.flac", true);
        let bytes = save(original.path(), &[bound]);
        fs::write(copied.path().join("health/retention-marks.json"), &bytes).unwrap();
        let response = list(copied.path()).await;
        assert_eq!(response["state"], "list.register_unavailable");
        assert!(response["review_context"].is_null());
        assert_eq!(
            fs::read(copied.path().join("health/retention-marks.json")).unwrap(),
            bytes
        );
    }

    #[tokio::test]
    async fn empty_invalid_and_oversized_lists_remain_distinct() {
        let root = journal();
        let empty = list(root.path()).await;
        assert_eq!(empty["state"], "list.empty");
        assert!(empty["review_context"].is_string());
        fs::create_dir(root.path().join("health")).unwrap();
        for bytes in [
            b"{".as_slice(),
            br#"{"version":3,"marks":{}}"#,
            br#"{"version":2,"marks":{},"extra":true}"#,
        ] {
            fs::write(root.path().join("health/retention-marks.json"), bytes).unwrap();
            let unavailable = list(root.path()).await;
            assert_eq!(unavailable["state"], "list.register_unavailable");
            assert!(unavailable["review_context"].is_null());
        }
        let mut large = mark(root.path(), "large.flac", false);
        large.proposal.reason = "x".repeat(512 * 1024);
        let before = save(root.path(), &[large]);
        let response = list(root.path()).await;
        assert_eq!(response["state"], "list.review_too_large");
        assert!(response["review_context"].is_null());
        assert_eq!(
            fs::read(root.path().join("health/retention-marks.json")).unwrap(),
            before
        );
    }

    #[tokio::test]
    async fn approval_stays_held_and_legacy_action_ids_refuse_without_file_effects() {
        let root = journal();
        let row = mark(root.path(), "a.flac", false);
        let before = save(root.path(), std::slice::from_ref(&row));
        let config = fs::read(root.path().join("config/journal.json")).unwrap();
        let context = list(root.path()).await["review_context"].clone();
        for (endpoint, state) in [("/app/home/api/approve", "approve.refused_before_start")] {
            let (status, response) = call(
                root.path(),
                "POST",
                endpoint,
                json!({"review_context": context, "mark_ids": [row.id]}).to_string(),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(response["state"], state);
            assert_eq!(response["requested_count"], 1);
            assert_eq!(
                response
                    .get("removed_count")
                    .or_else(|| response.get("declined_count")),
                Some(&json!(0))
            );
            let (status, response) = call(
                root.path(),
                "POST",
                endpoint,
                json!({"mark_ids": [row.id]}).to_string(),
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert_eq!(response["state"], "request.invalid");
        }
        let (status, response) = call(
            root.path(),
            "POST",
            "/app/home/api/decline",
            json!({"mark_ids": [row.id]}).to_string(),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(response["state"], "request.invalid");
        assert_eq!(
            fs::read(root.path().join("health/retention-marks.json")).unwrap(),
            before
        );
        assert_eq!(
            fs::read(root.path().join("config/journal.json")).unwrap(),
            config
        );
        assert_eq!(
            fs::read(root.path().join("chronicle/raw.flac")).unwrap(),
            b"original bytes"
        );
        assert!(
            !root
                .path()
                .join("health/retention-marks.json.lock")
                .exists()
        );
        assert!(!root.path().join("config/actions").exists());
    }

    #[tokio::test]
    async fn typed_approval_stays_prelaunch_held_after_reoffer_and_copied_context() {
        let original = journal();
        let copied = journal();
        let mut typed = mark(original.path(), "typed.flac", true);
        save(original.path(), std::slice::from_ref(&typed));
        let rendered = list(original.path()).await["review_context"].clone();
        let old = typed.offload.as_ref().unwrap();
        typed.offload = Some(OffloadOffer::new(old.proof.clone()).unwrap());
        let before = save(original.path(), std::slice::from_ref(&typed));
        fs::create_dir_all(copied.path().join("health")).unwrap();
        fs::write(copied.path().join("health/retention-marks.json"), &before).unwrap();
        for root in [original.path(), copied.path()] {
            let config = fs::read(root.join("config/journal.json")).unwrap();
            let (_, response) = call(
                root,
                "POST",
                "/app/home/api/approve",
                json!({"review_context":rendered,"mark_ids":[typed.id]}).to_string(),
            )
            .await;
            assert_eq!(response["state"], "approve.refused_before_start");
            assert_eq!(response["removed_count"], 0);
            assert_eq!(
                fs::read(root.join("health/retention-marks.json")).unwrap(),
                before
            );
            assert_eq!(fs::read(root.join("config/journal.json")).unwrap(), config);
            assert_eq!(
                fs::read(root.join("chronicle/raw.flac")).unwrap(),
                b"original bytes"
            );
            assert!(!root.join("backup-activity.lock").exists());
            assert!(!root.join("health/retention-marks.json.lock").exists());
        }
        // This route is a prelaunch refusal even when a current root cannot be opened.
        let moved = original.path().join("detached");
        fs::rename(original.path().join("health"), &moved).unwrap();
        fs::write(original.path().join("config/journal.json"), b"{broken").unwrap();
        let (_, response) = call(
            original.path(),
            "POST",
            "/app/home/api/approve",
            json!({"review_context":rendered,"mark_ids":[typed.id]}).to_string(),
        )
        .await;
        assert_eq!(response["state"], "approve.refused_before_start");
        assert_eq!(
            fs::read(moved.join("retention-marks.json")).unwrap(),
            before
        );
        assert!(!original.path().join("health").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn retained_decline_removes_mixed_selected_rows_and_keeps_failed_unselected_row() {
        let root = journal();
        let policy = mark(root.path(), "policy.flac", false);
        let typed = mark(root.path(), "typed.flac", true);
        let mut failed = mark(root.path(), "failed.flac", false);
        failed.state = MarkState::Failed(Failure {
            at: "now".into(),
            reason: "unfinished".into(),
            staged: None,
        });
        save(
            root.path(),
            &[policy.clone(), typed.clone(), failed.clone()],
        );
        let config = fs::read(root.path().join("config/journal.json")).unwrap();
        let context = list(root.path()).await["review_context"].clone();
        let body =
            json!({"review_context": context, "mark_ids": [policy.id, typed.id]}).to_string();
        let (status, response) =
            call(root.path(), "POST", "/app/home/api/decline", body.clone()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(response["state"], "declined.done");
        assert_eq!(response["requested_count"], 2);
        assert_eq!(response["declined_count"], 2);
        assert_eq!(response["refused_count"], 0);
        assert_eq!(response["unknown_count"], 0);
        assert_eq!(response["mutation"]["publication"], "published");
        assert_eq!(response["mutation"]["status"], "applied");
        let after = fs::read(root.path().join("health/retention-marks.json")).unwrap();
        let register: Register = serde_json::from_slice(&after).unwrap();
        assert_eq!(register.version, 2);
        assert_eq!(register.marks.len(), 1);
        assert_eq!(register.marks.get(&failed.id), Some(&failed));
        let (status, response) = call(root.path(), "POST", "/app/home/api/decline", body).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(response["state"], "declined.refused");
        assert_eq!(response["mutation"]["error"]["reason"], "missing");
        assert_eq!(
            fs::read(root.path().join("health/retention-marks.json")).unwrap(),
            after
        );
        assert_eq!(
            fs::read(root.path().join("config/journal.json")).unwrap(),
            config
        );
        assert_eq!(
            fs::read(root.path().join("chronicle/raw.flac")).unwrap(),
            b"original bytes"
        );
        assert!(!root.path().join("config/actions").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn retained_decline_refuses_whole_drifted_batch_and_copied_root_contexts() {
        for reoffer in [false, true] {
            let root = journal();
            let first = mark(root.path(), "first.flac", false);
            let mut second = mark(root.path(), "second.flac", true);
            save(root.path(), &[first.clone(), second.clone()]);
            let context = list(root.path()).await["review_context"].clone();
            if reoffer {
                let previous = second.offload.as_ref().unwrap();
                let replacement = OffloadOffer::new(previous.proof.clone()).unwrap();
                assert_ne!(previous.nonce, replacement.nonce);
                assert_ne!(previous.revision, replacement.revision);
                second.offload = Some(replacement);
            } else {
                second.proposal.reason = "changed proposal".into();
            }
            let before = save(root.path(), &[first.clone(), second.clone()]);
            let (_, response) = call(
                root.path(),
                "POST",
                "/app/home/api/decline",
                json!({"review_context": context, "mark_ids": [first.id, second.id]}).to_string(),
            )
            .await;
            assert_eq!(response["state"], "declined.refused");
            assert_eq!(response["declined_count"], 0);
            assert_eq!(response["refused_count"], 2);
            assert_eq!(response["mutation"]["error"]["reason"], "stale");
            assert_eq!(
                fs::read(root.path().join("health/retention-marks.json")).unwrap(),
                before
            );
            assert!(
                !root
                    .path()
                    .join("health/retention-marks.json.lock")
                    .exists()
            );
        }
        for bound in [false, true] {
            let original = journal();
            let copied = journal();
            let row = mark(original.path(), "copied.flac", bound);
            let before = save(original.path(), std::slice::from_ref(&row));
            save(copied.path(), std::slice::from_ref(&row));
            let context = list(original.path()).await["review_context"].clone();
            let (_, response) = call(
                copied.path(),
                "POST",
                "/app/home/api/decline",
                json!({"review_context": context, "mark_ids": [row.id]}).to_string(),
            )
            .await;
            assert_eq!(response["state"], "declined.refused");
            assert_eq!(response["declined_count"], 0);
            assert_eq!(response["mutation"]["error"]["reason"], "root_changed");
            assert_eq!(
                fs::read(copied.path().join("health/retention-marks.json")).unwrap(),
                before
            );
            assert!(
                !copied
                    .path()
                    .join("health/retention-marks.json.lock")
                    .exists()
            );
        }
    }

    #[tokio::test]
    async fn retained_decline_preserves_outer_session_gate() {
        for (config, expected_status) in [
            (b"{}".as_slice(), StatusCode::FOUND),
            (b"{".as_slice(), StatusCode::INTERNAL_SERVER_ERROR),
        ] {
            let root = journal();
            let row = mark(root.path(), "gated.flac", false);
            let before = save(root.path(), std::slice::from_ref(&row));
            let context = list(root.path()).await["review_context"].clone();
            fs::write(root.path().join("config/journal.json"), config).unwrap();
            let response = solstone_core_convey_shell::router(root.path().to_path_buf())
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/app/home/api/decline")
                        .header("content-type", "application/json")
                        .body(Body::from(
                            json!({"review_context": context, "mark_ids": [row.id]}).to_string(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), expected_status);
            if expected_status == StatusCode::FOUND {
                assert_eq!(response.headers().get("location").unwrap(), "/init");
            }
            assert_eq!(
                fs::read(root.path().join("health/retention-marks.json")).unwrap(),
                before
            );
            assert_eq!(
                fs::read(root.path().join("config/journal.json")).unwrap(),
                config
            );
            assert_eq!(
                fs::read(root.path().join("chronicle/raw.flac")).unwrap(),
                b"original bytes"
            );
            assert!(
                !root
                    .path()
                    .join("health/retention-marks.json.lock")
                    .exists()
            );
        }
    }

    #[tokio::test]
    async fn context_and_extractor_size_limits_return_specific_refusal_without_effects() {
        let root = journal();
        let row = mark(root.path(), "a.flac", false);
        let before = save(root.path(), std::slice::from_ref(&row));
        for context_bytes in [512 * 1024 + 1, 2 * 1024 * 1024 + 1] {
            for endpoint in ["/app/home/api/approve", "/app/home/api/decline"] {
                let (status, response) = call(
                    root.path(),
                    "POST",
                    endpoint,
                    json!({"review_context": "x".repeat(context_bytes), "mark_ids": [row.id]})
                        .to_string(),
                )
                .await;
                assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
                assert_eq!(response["state"], "request.too_large");
            }
        }
        assert_eq!(
            fs::read(root.path().join("health/retention-marks.json")).unwrap(),
            before
        );
        assert!(!root.path().join("config/actions").exists());
    }

    #[tokio::test]
    async fn approval_receiving_routes_refuse_missing_stale_and_poisoned_owners_without_cache() {
        use solstone_core_backup_web::{BackupWebDeps, archive_operation};
        async fn receive(
            router: axum::Router,
            method: &str,
            url: &str,
            body: &str,
        ) -> (StatusCode, Value) {
            let response = router
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(url)
                        .header("content-type", "application/json")
                        .body(Body::from(body.to_owned()))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.headers()["cache-control"], "no-store");
            let status = response.status();
            let value =
                serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap())
                    .unwrap();
            (status, value)
        }
        let root = journal();
        let standalone = || {
            solstone_core_home_web::routes(
                root.path().to_path_buf(),
                solstone_core_home_web::Clock::fixed(jiff::Timestamp::now()),
                |_| false,
            )
        };
        for (method, url, body) in [
            ("GET", "/app/home/api/approve/1", ""),
            ("POST", "/app/home/api/approve/1/cancel", "{}"),
        ] {
            let (status, value) = receive(standalone(), method, url, body).await;
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(value["state"], "approve.unavailable");
            assert!(value.get("removed_count").is_none());
        }
        let deps = BackupWebDeps::new(root.path().to_path_buf());
        let (generation, budget) =
            archive_operation::begin(&deps.operations, "other_archive", deps.clock.as_ref())
                .unwrap()
                .into_parts();
        let router = || {
            solstone_core_home_web::routes_with_backup_deps(
                deps.clone(),
                solstone_core_home_web::Clock::fixed(jiff::Timestamp::now()),
                |_| false,
            )
        };
        for body in ["", "[]", "null", "{", "{\"extra\":true}"] {
            assert_eq!(
                receive(
                    router(),
                    "POST",
                    &format!("/app/home/api/approve/{generation}/cancel"),
                    body
                )
                .await
                .0,
                StatusCode::BAD_REQUEST
            );
            assert!(!budget.cancellation().unwrap().is_cancelled());
        }
        let oversized = " ".repeat(1025);
        assert_eq!(
            receive(
                router(),
                "POST",
                &format!("/app/home/api/approve/{generation}/cancel"),
                &oversized
            )
            .await
            .0,
            StatusCode::PAYLOAD_TOO_LARGE
        );
        for id in [
            generation.to_string(),
            "01".into(),
            "+1".into(),
            "18446744073709551616".into(),
        ] {
            for (method, tail, body) in [("GET", "", ""), ("POST", "/cancel", "{}")] {
                let (_, value) = receive(
                    router(),
                    method,
                    &format!("/app/home/api/approve/{id}{tail}"),
                    body,
                )
                .await;
                assert_eq!(value["state"], "approve.stale");
                assert!(value.get("cancellation_requested").is_none());
                assert!(!budget.cancellation().unwrap().is_cancelled());
            }
        }
        let slot = deps.operations.clone();
        assert!(
            std::thread::spawn(move || {
                let _guard = slot.lock().unwrap();
                panic!("poison original receive mutex");
            })
            .join()
            .is_err()
        );
        for (method, tail, body) in [("GET", "", ""), ("POST", "/cancel", "{}")] {
            let (_, value) = receive(
                router(),
                method,
                &format!("/app/home/api/approve/{generation}{tail}"),
                body,
            )
            .await;
            assert_eq!(value["state"], "approve.unavailable");
            assert!(value.get("cancellation_requested").is_none());
        }
        assert!(!budget.cancellation().unwrap().is_cancelled());
        assert!(archive_operation::begin(&deps.operations, "backup", deps.clock.as_ref()).is_err());
    }

    #[tokio::test]
    async fn approval_receiving_preserves_real_journal_session_gate() {
        use solstone_core_backup_web::{BackupWebDeps, archive_operation};
        for (config, expected) in [
            (
                br#"{"setup":{"completed_at":1700000000000}}"#.as_slice(),
                StatusCode::CONFLICT,
            ),
            (b"{}".as_slice(), StatusCode::FOUND),
            (b"{".as_slice(), StatusCode::INTERNAL_SERVER_ERROR),
        ] {
            let root = journal();
            let row = mark(root.path(), "busy.flac", true);
            save(root.path(), std::slice::from_ref(&row));
            let context = solstone_core_retention::review_context::read_review_context(
                &JournalRoot::open(root.path()).unwrap(),
                std::slice::from_ref(&row.id),
            )
            .unwrap();
            let start_body = json!({"review_context":serde_json::to_string(&context).unwrap(),"mark_ids":[row.id.as_str()]}).to_string();
            let deps = BackupWebDeps::new(root.path().to_path_buf());
            let (generation, budget) =
                archive_operation::begin(&deps.operations, "other_archive", deps.clock.as_ref())
                    .unwrap()
                    .into_parts();
            let router = solstone_core_home_web::routes_with_backup_deps(
                deps.clone(),
                solstone_core_home_web::Clock::fixed(jiff::Timestamp::now()),
                |_| false,
            );
            let router = solstone_core_convey_shell::session_gate::apply_layer(
                router,
                root.path().to_path_buf(),
            );
            fs::write(root.path().join("config/journal.json"), config).unwrap();
            for (method, tail, body) in [
                ("GET", "", ""),
                ("POST", "/cancel", "{}"),
                ("POST", "/start", start_body.as_str()),
            ] {
                let response = router
                    .clone()
                    .oneshot(
                        Request::builder()
                            .method(method)
                            .uri(if tail == "/start" {
                                "/app/home/api/approve".into()
                            } else {
                                format!("/app/home/api/approve/{generation}{tail}")
                            })
                            .header("content-type", "application/json")
                            .body(Body::from(body.to_owned()))
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                let start_refused = tail == "/start" && expected == StatusCode::CONFLICT;
                assert_eq!(
                    response.status(),
                    if start_refused {
                        StatusCode::OK
                    } else {
                        expected
                    }
                );
                if start_refused {
                    let value: Value = serde_json::from_slice(
                        &to_bytes(response.into_body(), 65536).await.unwrap(),
                    )
                    .unwrap();
                    assert_eq!(value["state"], "approve.refused_before_start");
                }
                assert!(!budget.cancellation().unwrap().is_cancelled());
                assert!(
                    archive_operation::begin(&deps.operations, "backup", deps.clock.as_ref())
                        .is_err()
                );
            }
        }
    }
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[tokio::test]
    async fn public_approval_refuses_invalid_current_selection_before_admission() {
        for cut in [
            "legacy",
            "stale",
            "reoffer",
            "copied",
            "failed",
            "malformed",
            "oversized",
        ] {
            let root = journal();
            let mut row = mark(root.path(), "selected.flac", cut != "legacy");
            if cut == "legacy" {
                // An offload row from before archive offers: no backup proof.
                row.class = RemovalClass::OffloadRawRelease;
                row.id = MarkId::derive(row.class, &row.target, &row.proposal.names);
            }
            save(root.path(), &[row.clone()]);
            let listed = list(root.path()).await;
            let context = listed["review_context"].as_str().unwrap().to_owned();
            let mut body =
                json!({"review_context":context,"mark_ids":[row.id.as_str()]}).to_string();
            match cut {
                "stale" => {
                    row.proposal.reason = "changed".into();
                    save(root.path(), &[row.clone()]);
                }
                "reoffer" => {
                    row.offload = Some(
                        OffloadOffer::new(row.offload.as_ref().unwrap().proof.clone()).unwrap(),
                    );
                    save(root.path(), &[row.clone()]);
                }
                "failed" => {
                    row.state = MarkState::Failed(Failure {
                        at: "now".into(),
                        reason: "held".into(),
                        staged: None,
                    });
                    save(root.path(), &[row.clone()]);
                }
                "malformed" => body = "{".into(),
                "oversized" => body = " ".repeat(2 * 1024 * 1024 + 1),
                _ => {}
            }
            let before = fs::read(root.path().join("health/retention-marks.json")).unwrap();
            let copied = journal();
            let executing = if cut == "copied" {
                save(copied.path(), &[row.clone()]);
                copied.path()
            } else {
                root.path()
            };
            let mut deps = solstone_core_backup_web::BackupWebDeps::new(executing.to_path_buf());
            deps.executable = root.path().join("missing-package/bin/solstone");
            let router = solstone_core_home_web::routes_with_backup_deps(
                deps.clone(),
                solstone_core_home_web::Clock::fixed(
                    jiff::tz::Offset::UTC
                        .to_timestamp(jiff::civil::date(2026, 10, 11).at(12, 0, 0, 0))
                        .unwrap(),
                ),
                |_| false,
            );
            let response = router
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/app/home/api/approve")
                        .header("content-type", "application/json")
                        .body(Body::from(body))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.headers()["cache-control"], "no-store");
            let value: Value =
                serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap())
                    .unwrap();
            assert_ne!(value["state"], "approve.started", "{cut}");
            assert!(
                solstone_core_backup_web::archive_operation::begin(
                    &deps.operations,
                    "probe",
                    deps.clock.as_ref()
                )
                .is_ok()
            );
            assert!(!root.path().join("backup-activity.lock").exists());
            assert_eq!(
                fs::read(root.path().join("health/retention-marks.json")).unwrap(),
                before
            );
            assert_eq!(
                fs::read(root.path().join("chronicle/raw.flac")).unwrap(),
                b"original bytes"
            );
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[tokio::test]
    async fn public_policy_only_approval_starts_and_settles_locally() {
        let root = journal();
        let row = mark(root.path(), "selected.flac", false);
        let segment = root
            .path()
            .join(solstone_core_retention::layout::segment_rel(
                &row.target.day,
                &row.target.stream,
                &row.target.dir,
            ));
        fs::create_dir_all(&segment).unwrap();
        fs::create_dir_all(root.path().join("chronicle/20261009/health")).unwrap();
        fs::write(segment.join("selected.flac"), b"policy").unwrap();
        fs::write(
            segment.join("selected.jsonl"),
            "{\"_solstone_processing\":{\"schema\":\"solstone.processing.v1\",\"state\":\"analyzed\",\"reason_code\":\"ok\",\"handler\":\"transcribe\",\"input_size\":6,\"attempted_at\":\"2026-10-09T12:00:00Z\"}}\n{\"start\":0,\"text\":\"x\"}\n",
        )
        .unwrap();
        save(root.path(), std::slice::from_ref(&row));
        let context = list(root.path()).await["review_context"].clone();
        let mut deps = solstone_core_backup_web::BackupWebDeps::new(root.path().to_path_buf());
        deps.executable = root.path().join("missing-package/bin/solstone");
        let router = solstone_core_home_web::routes_with_backup_deps(
            deps.clone(),
            solstone_core_home_web::Clock::fixed(
                jiff::tz::Offset::UTC
                    .to_timestamp(jiff::civil::date(2026, 10, 11).at(12, 0, 0, 0))
                    .unwrap(),
            ),
            |_| false,
        );
        let request = |method: &str, uri: String, body: String| {
            Request::builder()
                .method(method)
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap()
        };
        let response = router
            .clone()
            .oneshot(request(
                "POST",
                "/app/home/api/approve".into(),
                json!({"review_context":context,"mark_ids":[row.id.as_str()]}).to_string(),
            ))
            .await
            .unwrap();
        let started: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
        assert_eq!(started["state"], "approve.started");
        let generation = started["generation"].as_str().unwrap().to_owned();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let observed = loop {
            let response = router
                .clone()
                .oneshot(request(
                    "GET",
                    format!("/app/home/api/approve/{generation}"),
                    String::new(),
                ))
                .await
                .unwrap();
            assert!(response.status().is_success());
            let observed: Value =
                serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap())
                    .unwrap();
            if observed["worker_settled"] == json!(true) {
                break observed;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "worker did not settle"
            );
            std::thread::park_timeout(std::time::Duration::from_millis(10));
        };
        assert_eq!(observed["state"], "completed");
        assert_eq!(observed["report"]["settlement"]["state"], "settled");
        assert!(!segment.join("selected.flac").exists());
        assert!(
            solstone_core_retention::review_context::load_at_root(
                &JournalRoot::open(root.path()).unwrap()
            )
            .unwrap()
            .marks
            .is_empty()
        );
        assert_eq!(
            fs::read(root.path().join("chronicle/raw.flac")).unwrap(),
            b"original bytes"
        );
        assert!(
            solstone_core_backup_web::archive_operation::begin(
                &deps.operations,
                "probe",
                deps.clock.as_ref()
            )
            .is_ok()
        );
    }
}

#[cfg(all(test, feature = "full-tests"))]
#[allow(clippy::disallowed_methods, reason = "the harness drives node")]
#[path = "review_transport/backup_observation.rs"]
mod backup_observation;
