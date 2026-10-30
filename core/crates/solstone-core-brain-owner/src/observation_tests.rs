// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::tests::*;
use super::*;
use solstone_core_brain::{RuntimeRefusalCaller, record_confidential_attestation_refusal_observed};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

fn sink() -> (BrainObservationSink, impl FnMut() -> Vec<Value>) {
    let (emitter, mut receiver) = solstone_core_callosum::test_support::diagnostic_fixture(128);
    (BrainObservationSink::new(emitter), move || {
        let mut rows = Vec::new();
        while let Ok(row) = receiver.try_recv() {
            rows.push(serde_json::to_value(row).unwrap());
        }
        rows
    })
}

fn probe() -> StaticProbe {
    StaticProbe {
        generate: Mutex::new(Box::new(|| {
            Ok(GenerateResponse::Generated(Box::new(generated_response(
                "OK",
                "stop",
                json!({}),
                None,
            ))))
        })),
        after_permit: None,
    }
}

fn spp_journal() -> tempfile::TempDir {
    let temp = cloud_journal();
    std::fs::write(temp.path().join("config/journal.json"), serde_json::to_vec(&json!({
        "providers":{"active":{"provider":"local","model":"local/qwen3.5-4b"},
            "local":{"endpoint_url":"https://example.invalid/v1","served_model_id":"model","credential":"secret"}},
        "services":{"confidential":{"endpoint_url":"https://example.invalid/v1","served_model_id":"model",
            "credential_fingerprint_sha256":"2bb80d537b1da3e38bd30361aa855686bde0eacd7162fef6a25fe97bf527a25b"}}
    })).unwrap()).unwrap();
    temp
}

struct ReadySpp;
impl OwnerSppPrerequisite for ReadySpp {
    fn assess(&self, _: &Path, _: &Map<String, Value>, now: DateTime<Utc>) -> Value {
        json!({"status":"ok","observed_at":now.to_rfc3339(),"expires_at":(now+Duration::days(1)).to_rfc3339()})
    }
}

fn runtime_refusal(
    journal: &Path,
    sink: Option<&BrainObservationSink>,
    caller: RuntimeRefusalCaller,
) {
    let config = read_journal_config(journal).unwrap().config.unwrap();
    record_confidential_attestation_refusal_observed(
        journal,
        &config,
        "proof_http_eof",
        sink,
        caller,
        ObservationPhase::RuntimeEstablish,
        None,
    );
}

fn assert_released(journal: &Path) {
    assert!(!probe_file_lease_held(&brain_refresh_lease_path(journal)).unwrap());
}

#[test]
fn exact_publication_survives_later_independent_publication() {
    for observed in [false, true] {
        let temp = spp_journal();
        let config_before = std::fs::read(temp.path().join("config/journal.json")).unwrap();
        let (sink, mut rows) = sink();
        let observation = sink.begin(ObservationCaller::MobileRefresh).unwrap();
        let published = Arc::new(Mutex::new(Value::Null));
        let checking = Arc::new(Mutex::new(Value::Null));
        let saved = Arc::clone(&published);
        let path = temp.path().to_path_buf();
        let runtime_sink = observed.then_some(sink.clone());
        AFTER_PUBLICATION.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                *saved.lock().unwrap() = brain_record(&path);
                runtime_refusal(&path, runtime_sink.as_ref(), RuntimeRefusalCaller::Generate);
            }))
        });
        let checked = Arc::clone(&checking);
        let mut probe = probe();
        probe.after_permit = Some(Box::new(move |journal| {
            *checked.lock().unwrap() = brain_record(journal)
        }));
        let outcome = refresh_observed(
            temp.path(),
            &refresh_options(),
            &probe,
            &ReadySpp,
            &|| false,
            observed.then_some(&observation),
        );
        assert!(matches!(
            outcome,
            OwnerRefreshOutcome::PersistedNonReady { .. }
        ));
        assert_eq!(
            std::fs::read(temp.path().join("config/journal.json")).unwrap(),
            config_before
        );
        assert_released(temp.path());
        let final_record = brain_record(temp.path());
        let published = published.lock().unwrap();
        assert!(
            final_record["revision"].as_u64().unwrap() > published["revision"].as_u64().unwrap()
        );
        let rows = rows();
        if !observed {
            assert!(rows.is_empty());
            continue;
        }
        let permit = rows.iter().find(|r| r["phase"] == "permit").unwrap();
        assert_eq!(
            permit["run_id"],
            checking.lock().unwrap()["checking"]["run_id"]
        );
        assert_eq!(
            permit["checking_revision"],
            checking.lock().unwrap()["revision"]
        );
        let own = rows
            .iter()
            .find(|r| r["phase"] == "finish" && r["outcome"] == "published")
            .unwrap();
        assert_eq!(own["observation_id"], observation.id());
        assert_eq!(own["revision"], published["revision"]);
        assert!(own.get("runtime_failure_marker").is_none());
        let independent = rows.iter().find(|r| r["caller"] == "generate").unwrap();
        assert_ne!(independent["observation_id"], own["observation_id"]);
        assert_eq!(independent["revision"], final_record["revision"]);
        assert_eq!(
            independent["runtime_failure_marker"],
            final_record["runtime_failure_marker"]["marker_id"]
        );
        assert_eq!(independent["verification_reason"], "proof_http_eof");
        assert_eq!(rows.last().unwrap()["outcome"], "non_ready");
    }
}

#[test]
fn publication_and_later_settlement_failure_are_separate() {
    let temp = cloud_journal();
    let (sink, mut rows) = sink();
    let observation = sink.begin(ObservationCaller::MobileRefresh).unwrap();
    let path = temp.path().join("config/journal.json");
    AFTER_PUBLICATION.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || std::fs::write(path, b"{").unwrap()))
    });
    let outcome = refresh_observed(
        temp.path(),
        &refresh_options(),
        &probe(),
        &NoSpp,
        &|| false,
        Some(&observation),
    );
    assert!(matches!(
        outcome,
        OwnerRefreshOutcome::OperationalFailure { .. }
    ));
    assert_released(temp.path());
    let record = brain_record(temp.path());
    let rows = rows();
    let publication = rows.iter().find(|r| r["outcome"] == "published").unwrap();
    assert_eq!(publication["revision"], record["revision"]);
    assert_eq!(rows.last().unwrap()["outcome"], "operational_failure");
    assert!(rows.iter().any(|r| r["phase"] == "credential_settlement"
        && r["failure_category"] == "credential_settlement_failed"));
}

#[test]
fn competing_runtime_publication_during_refresh_keeps_its_own_identity() {
    for observed in [false, true] {
        let temp = spp_journal();
        let config = std::fs::read(temp.path().join("config/journal.json")).unwrap();
        let (sink, mut rows) = sink();
        let observation = sink.begin(ObservationCaller::MobileRefresh).unwrap();
        let runtime_sink = observed.then_some(sink.clone());
        let mut probe = probe();
        probe.after_permit = Some(Box::new(move |journal| {
            runtime_refusal(
                journal,
                runtime_sink.as_ref(),
                RuntimeRefusalCaller::Transcription,
            )
        }));
        let outcome = refresh_observed(
            temp.path(),
            &refresh_options(),
            &probe,
            &ReadySpp,
            &|| false,
            observed.then_some(&observation),
        );
        assert!(
            matches!(outcome,OwnerRefreshOutcome::LostFence { ref view } if owner_exit_code(view)==3)
        );
        assert_released(temp.path());
        assert_eq!(
            std::fs::read(temp.path().join("config/journal.json")).unwrap(),
            config
        );
        let record = brain_record(temp.path());
        let rows = rows();
        if !observed {
            assert!(rows.is_empty());
            continue;
        }
        assert!(
            rows.iter()
                .any(|r| r["phase"] == "permit" && r["observation_id"] == observation.id())
        );
        let runtime = rows
            .iter()
            .find(|r| r["caller"] == "transcription")
            .unwrap();
        assert_ne!(runtime["observation_id"], observation.id());
        assert_eq!(runtime["revision"], record["revision"]);
        assert_eq!(
            runtime["runtime_failure_marker"],
            record["runtime_failure_marker"]["marker_id"]
        );
        assert!(
            !rows
                .iter()
                .any(|r| r["caller"] == "mobile_refresh" && r["outcome"] == "published")
        );
        assert!(
            rows.iter()
                .any(|r| r["failure_category"] == "checking_absent")
        );
        assert_eq!(rows.last().unwrap()["outcome"], "lost_fence");
        let before = std::fs::read(brain_state_path(temp.path())).unwrap();
        runtime_refusal(temp.path(), Some(&sink), RuntimeRefusalCaller::Generate);
        assert_eq!(
            std::fs::read(brain_state_path(temp.path())).unwrap(),
            before
        );
    }
}

#[test]
fn cancelled_busy_stale_and_closed_sink_preserve_decisions_and_bytes() {
    for closed in [false, true] {
        let temp = cloud_journal();
        let (emitter, mut receiver) = solstone_core_callosum::test_support::diagnostic_fixture(128);
        if closed {
            receiver.close();
        }
        let sink = BrainObservationSink::new(emitter);
        let observation = sink.begin(ObservationCaller::MobileRefresh).unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&cancelled);
        let checking = Arc::new(Mutex::new(Vec::new()));
        let saved = Arc::clone(&checking);
        let mut probe = probe();
        probe.after_permit = Some(Box::new(move |journal| {
            *saved.lock().unwrap() = std::fs::read(brain_state_path(journal)).unwrap();
            flag.store(true, Ordering::Release);
        }));
        let outcome = refresh_observed(
            temp.path(),
            &refresh_options(),
            &probe,
            &NoSpp,
            &|| cancelled.load(Ordering::Acquire),
            Some(&observation),
        );
        assert!(matches!(outcome, OwnerRefreshOutcome::Cancelled));
        assert_eq!(
            std::fs::read(brain_state_path(temp.path())).unwrap(),
            *checking.lock().unwrap()
        );
        assert_released(temp.path());
        let stale = refresh_observed(
            temp.path(),
            &OwnerRefreshOptions {
                expected_fingerprint: Some("invalid".into()),
                expected_active_fingerprint: false,
                expect_active_fingerprint_absent: true,
            },
            &probe,
            &NoSpp,
            &|| false,
            Some(&observation),
        );
        assert!(matches!(
            stale,
            OwnerRefreshOutcome::StaleExpectedFingerprint { .. }
        ));
        if closed {
            assert!(receiver.try_recv().is_err());
        } else {
            let mut rows = Vec::new();
            while let Ok(row) = receiver.try_recv() {
                rows.push(serde_json::to_value(row).unwrap());
            }
            assert!(rows.iter().any(|r| r["outcome"] == "cancelled"));
            assert!(rows.iter().all(|r| r["outcome"] != "published"));
        }
    }
}

#[test]
fn renewal_fallback_and_safe_renewal_retain_the_explicit_context() {
    let temp = spp_journal();
    let (sink, mut rows) = sink();
    let observation = sink.begin(ObservationCaller::MobileRenewal).unwrap();
    assert!(matches!(
        renew_prerequisites_observed(
            temp.path(),
            None,
            &probe(),
            &ReadySpp,
            &|| false,
            Some(&observation)
        ),
        OwnerRefreshOutcome::PersistedReady { .. }
    ));
    assert_released(temp.path());
    let first = rows();
    assert_eq!(first.iter().filter(|r| r["phase"] == "terminal").count(), 1);
    assert!(first.iter().any(|r| r["outcome"] == "published"));
    assert!(
        first
            .iter()
            .all(|r| r["observation_id"] == observation.id())
    );
    let mut no_generate = probe();
    no_generate.generate = Mutex::new(Box::new(|| {
        panic!("safe renewal must reuse Generate evidence")
    }));
    let renewed = sink.begin(ObservationCaller::MobileRenewal).unwrap();
    assert!(matches!(
        renew_prerequisites_observed(
            temp.path(),
            None,
            &no_generate,
            &ReadySpp,
            &|| false,
            Some(&renewed)
        ),
        OwnerRefreshOutcome::PersistedReady { .. }
    ));
    assert_released(temp.path());
    let next = rows();
    assert!(
        next.iter()
            .any(|r| r["phase"] == "renewal" && r["outcome"] == "published")
    );
    assert!(next.iter().all(|r| r["observation_id"] == renewed.id()));
}

#[test]
fn observed_busy_has_no_invented_permit_or_publication() {
    let temp = cloud_journal();
    let held = solstone_core_brain::begin_refresh(temp.path(), Utc::now(), None, None, false, None)
        .unwrap()
        .unwrap();
    let before = std::fs::read(brain_state_path(temp.path())).unwrap();
    let (sink, mut rows) = sink();
    let observation = sink.begin(ObservationCaller::MobileRefresh).unwrap();
    let outcome = refresh_observed(
        temp.path(),
        &refresh_options(),
        &probe(),
        &NoSpp,
        &|| false,
        Some(&observation),
    );
    assert!(matches!(outcome,OwnerRefreshOutcome::Busy { ref view } if owner_exit_code(view)==3));
    assert_eq!(
        std::fs::read(brain_state_path(temp.path())).unwrap(),
        before
    );
    drop(held);
    assert_released(temp.path());
    let rows = rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["outcome"], "busy");
    assert!(rows[0].get("run_id").is_none());
    assert!(rows[0].get("revision").is_none());
}

#[cfg(unix)]
#[test]
fn uncertain_publication_is_never_reported_definitely_unpublished() {
    use solstone_core_journal_io::{BoundPublicationPrimitive, run_with_bound_publication_fault};
    let temp = cloud_journal();
    solstone_core_brain::generate_fingerprint_key(temp.path()).unwrap();
    let config = std::fs::read(temp.path().join("config/journal.json")).unwrap();
    let (sink, mut rows) = sink();
    let observation = sink.begin(ObservationCaller::MobileRefresh).unwrap();
    let (outcome, reached) =
        run_with_bound_publication_fault(BoundPublicationPrimitive::ParentSync, 3, 5, || {
            refresh_observed(
                temp.path(),
                &refresh_options(),
                &probe(),
                &NoSpp,
                &|| false,
                Some(&observation),
            )
        });
    assert!(reached);
    assert!(
        matches!(outcome,OwnerRefreshOutcome::LostFence { ref view } if owner_exit_code(view)==3),
        "actual outcome: {outcome:?}"
    );
    assert_released(temp.path());
    assert_eq!(
        std::fs::read(temp.path().join("config/journal.json")).unwrap(),
        config
    );
    assert_eq!(brain_record(temp.path())["aggregate_state"], "ready");
    let rows = rows();
    assert!(
        rows.iter()
            .any(|r| r["phase"] == "finish" && r["outcome"] == "publication_uncertain")
    );
    assert!(!rows.iter().any(|r| r["outcome"] == "published"));
}
