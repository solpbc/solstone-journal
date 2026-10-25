// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native Generate transport and ordinary Settings key-owner composition.
//! These direct calls do not qualify the Sense/Think/Cortex processing pipeline.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;
use solstone_core_generate::{Cancellation, ContentPart, GenerateRequest};

const MARKER: &str = "SOLSTONE_BINDING_COMPOSITION_CHILD";
const JOURNAL_ENV: &str = "SOLSTONE_BINDING_JOURNAL";
const RECEIPT: &str = "SOLSTONE_BINDING_RECEIPT ";
const CANARY: &str = "composition-canary-bytes";
const SECRET: &str = "sk-composition-secret";
const CORRECTED: &str = "sk-composition-corrected";
const CASE: &str = "credential_binding_composition::bound_rejection_composition_posts_retained_bytes_after_correction";

#[test]
fn bound_rejection_composition_posts_retained_bytes_after_correction() {
    if std::env::var(MARKER).ok().as_deref() == Some(CASE) {
        let journal = PathBuf::from(std::env::var(JOURNAL_ENV).expect("journal"));
        let runtime = solstone_core_generate_wire::EndpointRuntime::default();
        let cancellation = Cancellation::never();
        let request = GenerateRequest {
            id: None,
            context: "segment.thought".to_owned(),
            contents: vec![ContentPart::Text {
                text: CANARY.to_owned(),
            }],
            system_instruction: None,
            temperature: 0.2,
            max_output_tokens: 32,
            timeout_s: Some(30.0),
            json_output: false,
            json_schema: None,
            attempt_index: 0,
            exclusive_admission: false,
            overflow_text: None,
            transport_retries: Some(0),
        };
        let result = solstone_core_generate_wire::respond(
            &request,
            &runtime,
            &journal,
            |_| None,
            &cancellation,
        );
        let reason = match result {
            Ok(solstone_core_generate::GenerateResponse::Refused(refusal)) => refusal
                .reason_code
                .as_ref()
                .map(|code| code.as_wire().to_owned())
                .unwrap_or_else(|| "refused".to_owned()),
            Ok(solstone_core_generate::GenerateResponse::Generated(_)) => "generated".to_owned(),
            Ok(solstone_core_generate::GenerateResponse::Cancelled) => "cancelled".to_owned(),
            Err(_) => "dispatch_error".to_owned(),
        };
        println!("{RECEIPT}{{\"reason\":\"{reason}\"}}");
        return;
    }

    let journal = std::env::temp_dir().join(format!(
        "solstone-binding-composition-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    std::fs::create_dir_all(journal.join("config")).expect("journal");
    std::fs::write(
        journal.join("config/journal.json"),
        serde_json::to_vec(&json!({
            "env": {"OPENAI_API_KEY": SECRET},
            "providers": {"active": {"provider": "openai", "model": "gpt-binding"}}
        }))
        .expect("config"),
    )
    .expect("config writes");
    let now = chrono::Utc::now() - chrono::Duration::seconds(30);
    let record_path = solstone_core_brain::brain_state_path(&journal);
    std::fs::create_dir_all(record_path.parent().expect("health")).expect("health");
    std::fs::write(
        &record_path,
        serde_json::to_vec(&json!({
            "schema_version": 1,
            "revision": 1,
            "updated_at": now.to_rfc3339(),
            "aggregate_state": "ready",
            "reason_code": null,
            "active_lane": "byo-cloud",
            "active_provider": "openai",
            "active_model": "gpt-binding",
            "fingerprint_sha256": "a".repeat(64),
            "diagnostic": {},
            "checking": null,
            "runtime_failure_marker": null,
            "evidence": {
                "configuration": {"status": "ok", "observed_at": now.to_rfc3339(), "expires_at": "2030-01-01T00:00:00Z"},
                "generate": {"status": "ok", "observed_at": now.to_rfc3339(), "expires_at": "2030-01-01T00:00:00Z"},
                "lane_prerequisites": {"status": "ok", "observed_at": now.to_rfc3339(), "expires_at": "2030-01-01T00:00:00Z"}
            }
        }))
        .expect("record"),
    )
    .expect("record writes");

    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let port = listener.local_addr().expect("port").port();
    let base = format!("http://127.0.0.1:{port}");
    let mut child = Command::new(std::env::current_exe().expect("test executable"));
    child
        .arg("--exact")
        .arg(CASE)
        .arg("--nocapture")
        .env_clear()
        .envs(platform_env())
        .env("PATH", "")
        .env(MARKER, CASE)
        .env(JOURNAL_ENV, &journal)
        .env(
            solstone_core_generate_wire::overrides::BASE_URL_OVERRIDE_ENV,
            &base,
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = OwnedChild::spawn(&mut child);
    let (mut stream, body) = accept_request(&listener);
    assert!(body.contains(CANARY), "first post carries the canary");
    assert!(!body.contains(SECRET));
    write_http(&mut stream, 401, "{}");
    let output = child.finish_output();
    assert!(output.status.success(), "child failed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("provider_key_invalid"));

    let config = solstone_core_thinking::read_config(&journal).expect("config");
    let stored = config["providers"]["key_validation"]["openai"].to_string();
    assert!(stored.contains("credential_hmac"));
    assert!(!stored.contains(SECRET));
    let inspection =
        solstone_core_brain::inspect_brain_state(&journal, &config, chrono::Utc::now());
    assert_eq!(
        inspection.projection.reason_code.as_deref(),
        Some("provider_key_invalid")
    );
    assert_eq!(inspection.projection.aggregate_state, "unhealthy");
    let entry = config["providers"]["key_validation"]["openai"].clone();
    let admitted =
        Arc::new(solstone_core_journal_io::JournalRoot::open(&journal).expect("journal root"));
    let cancellation = Cancellation::never();
    // This direct-call fixture is a unit control, not a fresh HTTP validation
    // or normal processing/composition acceptance proof.
    let witness = solstone_core_brain::capture_credential_validation_bound(
        admitted.clone(),
        &journal,
        solstone_core_brain::CredentialHoldTarget::Managed {
            provider: "openai",
            credential: CORRECTED,
        },
    )
    .expect("unit correction witness");
    solstone_core_thinking::providers::save_key(
        &journal,
        &admitted,
        &cancellation,
        "OPENAI_API_KEY",
        "openai",
        CORRECTED,
        Some(json!({"valid": true})),
        SECRET,
        Some(&entry),
        Some(witness),
    )
    .expect("ordinary key correction");

    let mut child = Command::new(std::env::current_exe().expect("test executable"));
    child
        .arg("--exact")
        .arg(CASE)
        .arg("--nocapture")
        .env_clear()
        .envs(platform_env())
        .env("PATH", "")
        .env(MARKER, CASE)
        .env(JOURNAL_ENV, &journal)
        .env(
            solstone_core_generate_wire::overrides::BASE_URL_OVERRIDE_ENV,
            &base,
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = OwnedChild::spawn(&mut child);
    let (mut stream, body) = accept_request(&listener);
    assert!(
        body.contains(CANARY),
        "corrected post carries the same bytes"
    );
    assert!(!body.contains(CORRECTED));
    write_http(&mut stream, 200, "{}");
    let _ = child.finish_output();
    assert!(!journal.join("health/work-retry").exists());
    let _ = std::fs::remove_dir_all(journal);
}

const OWNER_CASE: &str =
    "credential_binding_composition::owner_correction_keeps_the_unprocessed_segment";

#[test]
fn owner_correction_keeps_the_unprocessed_segment() {
    if std::env::var(MARKER).ok().as_deref() == Some(OWNER_CASE) {
        let journal = PathBuf::from(std::env::var(JOURNAL_ENV).expect("journal"));
        let phase = std::env::var("SOLSTONE_BINDING_PHASE").unwrap_or_default();
        let runtime = solstone_core_generate_wire::EndpointRuntime::default();
        let cancellation = Cancellation::never();
        if phase == "refresh" {
            use solstone_core_brain_owner::{
                OwnerRefreshOptions, OwnerRefreshOutcome, ProductionSppPrerequisite,
                RespondWithConfigProbe,
            };
            let outcome = solstone_core_brain_owner::refresh(
                &journal,
                &OwnerRefreshOptions {
                    expected_fingerprint: None,
                    expected_active_fingerprint: false,
                    expect_active_fingerprint_absent: false,
                },
                &RespondWithConfigProbe { runtime },
                &ProductionSppPrerequisite,
                &|| false,
            );
            let owner = match outcome {
                OwnerRefreshOutcome::PersistedReady { .. } => "ready",
                OwnerRefreshOutcome::LostFence { .. } => "lost_fence",
                other => panic!("unexpected owner outcome: {other:?}"),
            };
            println!("{RECEIPT}{}", json!({"owner":owner}));
            return;
        }
        if phase == "probe" {
            let request = GenerateRequest {
                id: None,
                context: "settings.cloud.validate_key".to_owned(),
                contents: vec![ContentPart::Text {
                    text: "Reply with the single word OK.".to_owned(),
                }],
                system_instruction: None,
                temperature: 0.0,
                max_output_tokens: 512,
                timeout_s: Some(30.0),
                json_output: false,
                json_schema: None,
                attempt_index: 0,
                exclusive_admission: false,
                overflow_text: None,
                transport_retries: Some(0),
            };
            let result = solstone_core_generate_wire::respond(
                &request,
                &runtime,
                &journal,
                |_| None,
                &cancellation,
            );
            let valid = matches!(
                result,
                Ok(solstone_core_generate::GenerateResponse::Generated(_))
            );
            println!("{RECEIPT}{{\"valid\":{valid}}}");
            return;
        }
        let source = PathBuf::from(std::env::var("SOLSTONE_BINDING_SOURCE").expect("source"));
        let text = String::from_utf8(std::fs::read(&source).expect("source bytes")).expect("text");
        let request = GenerateRequest {
            id: None,
            context: "segment.thought".to_owned(),
            contents: vec![ContentPart::Text { text }],
            system_instruction: None,
            temperature: 0.2,
            max_output_tokens: 32,
            timeout_s: Some(30.0),
            json_output: false,
            json_schema: None,
            attempt_index: 0,
            exclusive_admission: false,
            overflow_text: None,
            transport_retries: Some(0),
        };
        let result = solstone_core_generate_wire::respond(
            &request,
            &runtime,
            &journal,
            |_| None,
            &cancellation,
        );
        let reason = match result {
            Ok(solstone_core_generate::GenerateResponse::Refused(refusal)) => refusal
                .reason_code
                .as_ref()
                .map(|code| code.as_wire().to_owned())
                .unwrap_or_else(|| "refused".to_owned()),
            Ok(solstone_core_generate::GenerateResponse::Generated(_)) => "generated".to_owned(),
            Ok(solstone_core_generate::GenerateResponse::Cancelled) => "cancelled".to_owned(),
            Err(_) => "dispatch_error".to_owned(),
        };
        println!("{RECEIPT}{{\"reason\":\"{reason}\"}}");
        return;
    }

    let journal = std::env::temp_dir().join(format!(
        "solstone-binding-owner-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    let segment = journal.join("chronicle/20260812/capture/120000_1");
    std::fs::create_dir_all(journal.join("config")).expect("config");
    std::fs::create_dir_all(&segment).expect("segment");
    let source = segment.join("audio.flac");
    std::fs::write(&source, CANARY.as_bytes()).expect("source");
    std::fs::write(
        journal.join("config/journal.json"),
        serde_json::to_vec(&json!({
            "env": {"OPENAI_API_KEY": SECRET},
            "providers": {"active": {"provider": "openai", "model": "gpt-binding"}}
        }))
        .expect("config"),
    )
    .expect("config writes");
    let day = journal.join("chronicle/20260812");
    let identity = solstone_core_sense::batch::scan_unprocessed(&journal, &day, None, None, None)
        .expect("scan");
    assert_eq!(identity.len(), 1);
    let identity = identity.into_iter().next().expect("segment");
    assert_eq!(
        std::fs::read(&identity.path).expect("source bytes"),
        CANARY.as_bytes()
    );

    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let port = listener.local_addr().expect("port").port();
    let base = format!("http://127.0.0.1:{port}");
    let spawn = |phase: &str, extra: &[(&str, &str)]| {
        let mut child = Command::new(std::env::current_exe().expect("test executable"));
        child
            .arg("--exact")
            .arg(OWNER_CASE)
            .arg("--nocapture")
            .env_clear()
            .envs(platform_env())
            .env("PATH", "")
            .env(MARKER, OWNER_CASE)
            .env(JOURNAL_ENV, &journal)
            .env("SOLSTONE_BINDING_PHASE", phase)
            .env("SOLSTONE_BINDING_SOURCE", &source)
            .env(
                solstone_core_generate_wire::overrides::BASE_URL_OVERRIDE_ENV,
                &base,
            )
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in extra {
            child.env(key, value);
        }
        OwnedChild::spawn(&mut child)
    };
    let finish = |child: OwnedChild| {
        let output = child.finish_output();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{stdout}{stderr}");
        let line = stdout
            .lines()
            .find(|line| line.starts_with(RECEIPT))
            .expect("receipt");
        serde_json::from_str::<serde_json::Value>(&line[RECEIPT.len()..]).expect("receipt json")
    };
    let same_identity = || {
        let again = solstone_core_sense::batch::scan_unprocessed(&journal, &day, None, None, None)
            .expect("scan");
        assert_eq!(again, vec![identity.clone()]);
        assert_eq!(
            std::fs::read(&identity.path).expect("source bytes"),
            CANARY.as_bytes()
        );
    };

    let child = spawn("post", &[]);
    let (mut stream, body) = accept_request(&listener);
    assert!(body.contains(CANARY));
    assert!(!body.contains(SECRET));
    write_http(&mut stream, 401, "{}");
    let receipt = finish(child);
    assert_eq!(receipt["reason"], "provider_key_invalid");
    let config = solstone_core_thinking::read_config(&journal).expect("config");
    assert!(
        config["providers"]["key_validation"]["openai"]["credential_hmac"]
            .as_str()
            .is_some()
    );
    assert!(!solstone_core_brain::brain_state_path(&journal).exists());
    let inspection =
        solstone_core_brain::inspect_brain_state(&journal, &config, chrono::Utc::now());
    assert_eq!(
        inspection.projection.reason_code.as_deref(),
        Some("provider_key_invalid")
    );
    assert_eq!(inspection.projection.aggregate_state, "unhealthy");
    assert!(inspection.projection.fingerprint_sha256.is_none());
    same_identity();
    assert!(!journal.join("health/work-retry").exists());

    let mut edited: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(journal.join("config/journal.json")).expect("text"),
    )
    .expect("json");
    let hmac = edited["providers"]["key_validation"]["openai"]["credential_hmac"].clone();
    edited["providers"]["active"]["model"] = json!("gpt-binding-other");
    std::fs::write(
        journal.join("config/journal.json"),
        serde_json::to_vec(&edited).expect("bytes"),
    )
    .expect("model write");
    same_identity();
    struct Unavailable;
    impl solstone_core_thinking::providers::ManagedKeyValidator for Unavailable {
        fn validate(&self, _provider: &str, _key: &str) -> Result<serde_json::Value, String> {
            Ok(json!({"valid": false, "reason_code": "validation_unavailable"}))
        }
    }
    solstone_core_thinking::providers::persist_key_validations(&journal, &Unavailable)
        .expect("unavailable persist");
    let kept = solstone_core_thinking::read_config(&journal).expect("config");
    assert_eq!(
        kept["providers"]["key_validation"]["openai"]["reason_code"],
        "provider_key_invalid"
    );
    assert_eq!(
        kept["providers"]["key_validation"]["openai"]["credential_hmac"],
        hmac
    );
    same_identity();

    let admitted = Arc::new(solstone_core_journal_io::JournalRoot::open(&journal).expect("root"));
    let cancellation = Cancellation::never();
    let witness = solstone_core_brain::capture_credential_validation_bound(
        admitted.clone(),
        &journal,
        solstone_core_brain::CredentialHoldTarget::Managed {
            provider: "openai",
            credential: CORRECTED,
        },
    )
    .expect("pre-probe correction witness");
    let probe = spawn(
        "probe",
        &[(
            solstone_core_generate_wire::overrides::API_KEY_OVERRIDE_ENV,
            CORRECTED,
        )],
    );
    let (mut stream, body) = accept_request(&listener);
    assert!(body.contains("Reply with the single word OK."));
    write_http(
        &mut stream,
        200,
        r#"{"model":"gpt-binding","status":"completed","output":[{"content":[{"type":"output_text","text":"OK"}]}]}"#,
    );
    let probed = finish(probe);
    assert_eq!(probed["valid"], true);
    let validation = json!({"valid": probed["valid"]});
    let entry = solstone_core_thinking::read_config(&journal).expect("config")["providers"]
        ["key_validation"]["openai"]
        .clone();
    solstone_core_thinking::providers::save_key(
        &journal,
        &admitted,
        &cancellation,
        "OPENAI_API_KEY",
        "openai",
        CORRECTED,
        Some(validation),
        SECRET,
        Some(&entry),
        Some(witness),
    )
    .expect("correction from the canned probe");
    same_identity();

    let child = spawn("again", &[]);
    let (mut stream, body) = accept_request(&listener);
    assert!(body.contains(CANARY));
    assert!(!body.contains(CORRECTED));
    write_http(
        &mut stream,
        200,
        r#"{"model":"gpt-binding-other","status":"completed","output":[{"content":[{"type":"output_text","text":"OK"}]}]}"#,
    );
    let receipt = finish(child);
    assert_eq!(receipt["reason"], "generated");
    same_identity();
    assert_eq!(std::fs::read(&source).expect("source"), CANARY.as_bytes());
    assert!(!journal.join("health/work-retry").exists());
    let _ = std::fs::remove_dir_all(journal);
}

#[test]
fn late_child_rejection_cannot_overwrite_newer_same_key_settings_success() {
    let journal = TemporaryJournal::new("late-child-rejection");
    std::fs::create_dir_all(journal.0.join("config")).unwrap();
    std::fs::write(
        journal.0.join("config/journal.json"),
        serde_json::to_vec(&json!({
            "env":{"OPENAI_API_KEY":SECRET},
            "providers":{"active":{"provider":"openai","model":"gpt-binding"}}
        }))
        .unwrap(),
    )
    .unwrap();
    // Keep the protected binding key stable before either real HTTP request;
    // first-use key changes would otherwise mask the cross-process race.
    let key = solstone_core_brain::generate_fingerprint_key(&journal.0).unwrap();
    let key_before = std::fs::read(journal.0.join("health/brain-fingerprint.key")).unwrap();
    let root = Arc::new(solstone_core_journal_io::JournalRoot::open(&journal.0).unwrap());
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let spawn = |case: &str, phase: Option<&str>| {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .arg("--exact")
            .arg(case)
            .arg("--nocapture")
            .env_clear()
            .envs(platform_env())
            .env("PATH", "")
            .env(MARKER, case)
            .env(JOURNAL_ENV, &journal.0)
            .env(
                solstone_core_generate_wire::overrides::BASE_URL_OVERRIDE_ENV,
                &base,
            )
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(phase) = phase {
            command.env("SOLSTONE_BINDING_PHASE", phase);
        }
        OwnedChild::spawn(&mut command)
    };
    let old = spawn(CASE, None);
    let (mut old_stream, old_request) = accept_content_request(&listener);
    old_request.assert_openai(SECRET);
    assert!(old_request.body.contains(CANARY));
    assert!(!old_request.body.contains(SECRET));
    // The older child now waits for its real content-request response while
    // this host captures authority and runs a separate real canned probe.
    let witness = solstone_core_brain::capture_credential_validation_bound(
        root.clone(),
        &journal.0,
        solstone_core_brain::CredentialHoldTarget::Managed {
            provider: "openai",
            credential: SECRET,
        },
    )
    .unwrap();
    let probe = spawn(OWNER_CASE, Some("probe"));
    let (mut probe_stream, probe_request) = accept_content_request(&listener);
    probe_request.assert_openai(SECRET);
    assert!(
        probe_request
            .body
            .contains("Reply with the single word OK.")
    );
    assert!(!probe_request.body.contains(CANARY));
    write_http(
        &mut probe_stream,
        200,
        r#"{"model":"gpt-binding","status":"completed","output":[{"content":[{"type":"output_text","text":"OK"}]}]}"#,
    );
    let probed = probe.finish_receipt();
    assert_eq!(probed["valid"], true);
    solstone_core_thinking::providers::save_key(
        &journal.0,
        &root,
        &Cancellation::never(),
        "OPENAI_API_KEY",
        "openai",
        SECRET,
        Some(json!({"valid":probed["valid"]})),
        SECRET,
        None,
        Some(witness),
    )
    .unwrap();
    let fresh = solstone_core_thinking::read_config(&journal.0).unwrap();
    let fresh_entry = fresh["providers"]["key_validation"]["openai"].clone();
    assert_eq!(fresh_entry["valid"], true);
    assert_eq!(
        fresh_entry["credential_hmac"],
        solstone_core_brain::managed_credential_binding_hmac(&key, "openai", SECRET)
    );
    // Release the old content response only after ordinary Settings settled.
    write_http(
        &mut old_stream,
        401,
        r#"{"error":{"message":"invalid api key"}}"#,
    );
    let old_receipt = old.finish_receipt();
    assert_eq!(old_receipt["reason"], "provider_key_invalid");
    let after = solstone_core_thinking::read_config(&journal.0).unwrap();
    assert_eq!(
        std::fs::read(journal.0.join("health/brain-fingerprint.key")).unwrap(),
        key_before
    );
    assert_eq!(
        root.identity(),
        solstone_core_journal_io::JournalRoot::open(&journal.0)
            .unwrap()
            .identity()
    );
    assert_eq!(
        after["providers"]["key_validation"]["openai"], fresh_entry,
        "the older separate-process rejection must not overwrite newer same-key Settings success"
    );
}

/// Real callable-safe Brain owner plus configured BYO endpoint. Unlike the
/// managed-provider override control above, no ambient endpoint override is
/// needed or honored by RespondWithConfigProbe.
struct EndpointRace {
    journal: TemporaryJournal,
    listener: TcpListener,
    root: Arc<solstone_core_journal_io::JournalRoot>,
    key: [u8; 32],
    config_before: Vec<u8>,
}

impl EndpointRace {
    fn new(label: &str) -> Self {
        let journal = TemporaryJournal::new(label);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        std::fs::create_dir_all(journal.0.join("config")).unwrap();
        let config_before = serde_json::to_vec(&json!({
            "providers":{"active":{"provider":"local","model":"local/declared-endpoint"},
                "local":{"endpoint_url":base,"served_model_id":"gpt-binding","credential":SECRET}}
        }))
        .unwrap();
        std::fs::write(journal.0.join("config/journal.json"), &config_before).unwrap();
        let key = solstone_core_brain::generate_fingerprint_key(&journal.0).unwrap();
        let root = Arc::new(solstone_core_journal_io::JournalRoot::open(&journal.0).unwrap());
        Self {
            journal,
            listener,
            root,
            key,
            config_before,
        }
    }

    fn spawn(&self, refresh: bool) -> OwnedChild {
        let case = if refresh { OWNER_CASE } else { CASE };
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .arg("--exact")
            .arg(case)
            .arg("--nocapture")
            .env_clear()
            .envs(platform_env())
            .env("PATH", "")
            .env(MARKER, case)
            .env(JOURNAL_ENV, &self.journal.0)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if refresh {
            command.env("SOLSTONE_BINDING_PHASE", "refresh");
        }
        OwnedChild::spawn(&mut command)
    }

    fn request(&self, content: bool) -> TcpStream {
        let (stream, request) = accept_content_request(&self.listener);
        assert_eq!(request.method, "POST");
        assert_eq!(request.target, "/v1/chat/completions");
        let auth = request
            .headers
            .iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case("authorization"))
            .map(|(_, value)| value.as_str())
            .collect::<Vec<_>>();
        assert_eq!(auth, vec![format!("Bearer {SECRET}").as_str()]);
        serde_json::from_str::<serde_json::Value>(&request.body).unwrap();
        assert_eq!(request.body.contains(CANARY), content);
        if !content {
            assert!(request.body.contains("Reply with the single word OK."));
        }
        assert!(!request.body.contains(SECRET));
        stream
    }

    fn positive(stream: &mut TcpStream) {
        write_http(
            stream,
            200,
            r#"{"id":"endpoint-control","model":"gpt-binding","choices":[{"index":0,"message":{"role":"assistant","content":"OK"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}"#,
        );
    }

    fn negative(stream: &mut TcpStream) {
        write_http(stream, 401, r#"{"error":{"message":"invalid api key"}}"#);
    }

    fn assert_identity(&self) {
        assert_eq!(
            std::fs::read(self.journal.0.join("health/brain-fingerprint.key")).unwrap(),
            self.key
        );
        assert_eq!(
            solstone_core_journal_io::JournalRoot::open(&self.journal.0)
                .unwrap()
                .identity(),
            self.root.identity()
        );
    }
}

#[test]
fn late_child_rejection_cannot_overwrite_newer_unheld_refresh_success() {
    let race = EndpointRace::new("late-rejection-unheld-refresh");
    let old = race.spawn(false);
    let mut old_stream = race.request(true);
    let refresh = race.spawn(true);
    let mut refresh_stream = race.request(false);
    EndpointRace::positive(&mut refresh_stream);
    assert_eq!(refresh.finish_receipt()["owner"], "ready");
    let brain_before = std::fs::read(race.journal.0.join("health/brain.json")).unwrap();
    let brain: serde_json::Value = serde_json::from_slice(&brain_before).unwrap();
    assert!(
        brain["credential_observations"]["local"]["observation_id"]
            .as_str()
            .is_some_and(|id| id.len() == 64)
    );
    assert!(
        brain
            .get("credential_holds")
            .is_none_or(|holds| holds.as_object().unwrap().is_empty())
    );
    assert_eq!(
        std::fs::read(race.journal.0.join("config/journal.json")).unwrap(),
        race.config_before
    );
    EndpointRace::negative(&mut old_stream);
    assert_eq!(old.finish_receipt()["reason"], "provider_key_invalid");
    assert_eq!(
        std::fs::read(race.journal.0.join("health/brain.json")).unwrap(),
        brain_before
    );
    assert_eq!(
        std::fs::read(race.journal.0.join("config/journal.json")).unwrap(),
        race.config_before
    );
    race.assert_identity();
    let next = race.spawn(false);
    let mut next_stream = race.request(true);
    EndpointRace::positive(&mut next_stream);
    assert_eq!(next.finish_receipt()["reason"], "generated");
}

#[test]
fn stale_refresh_success_cannot_clear_newer_separate_process_rejection() {
    let race = EndpointRace::new("stale-refresh-new-rejection");
    let old_refresh = race.spawn(true);
    let mut refresh_stream = race.request(false);
    let rejecting = race.spawn(false);
    let mut content_stream = race.request(true);
    EndpointRace::negative(&mut content_stream);
    assert_eq!(rejecting.finish_receipt()["reason"], "provider_key_invalid");
    let config_before = std::fs::read(race.journal.0.join("config/journal.json")).unwrap();
    let config: serde_json::Value = serde_json::from_slice(&config_before).unwrap();
    assert_eq!(
        config["providers"]["key_validation"]["local"]["valid"],
        false
    );
    assert_eq!(
        config["providers"]["key_validation"]["local"]["reason_code"],
        "provider_key_invalid"
    );
    let brain_before = std::fs::read(race.journal.0.join("health/brain.json")).unwrap();
    EndpointRace::positive(&mut refresh_stream);
    assert_eq!(old_refresh.finish_receipt()["owner"], "lost_fence");
    assert_eq!(
        std::fs::read(race.journal.0.join("config/journal.json")).unwrap(),
        config_before
    );
    assert_eq!(
        std::fs::read(race.journal.0.join("health/brain.json")).unwrap(),
        brain_before
    );
    race.assert_identity();
    // A fresh EndpointRuntime may fetch model metadata before admission.
    // Service/count that route separately; require zero owner-content POSTs.
    let metadata = RefusalMetadataServer::start(race.listener.try_clone().unwrap());
    let retry = race.spawn(false);
    assert_eq!(retry.finish_receipt()["reason"], "provider_key_invalid");
    assert_eq!(
        metadata.finish(),
        1,
        "one actual metadata GET, zero content POSTs"
    );
}

struct RefusalMetadataServer {
    stop: Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<usize>>,
}

impl RefusalMetadataServer {
    fn start(listener: TcpListener) -> Self {
        use std::sync::atomic::{AtomicBool, Ordering};
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let thread = std::thread::spawn(move || {
            listener.set_nonblocking(true).unwrap();
            let deadline = Instant::now() + Duration::from_secs(40);
            let mut gets = 0;
            loop {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let request = read_request(&mut stream);
                        assert_eq!(
                            request.method, "GET",
                            "known rejected retry sent owner content"
                        );
                        assert_eq!(request.target, "/v1/models");
                        assert!(request.body.is_empty());
                        let auth = request
                            .headers
                            .iter()
                            .filter(|(name, _)| name.eq_ignore_ascii_case("authorization"))
                            .map(|(_, value)| value.as_str())
                            .collect::<Vec<_>>();
                        assert_eq!(auth, vec![format!("Bearer {SECRET}").as_str()]);
                        gets += 1;
                        write_http(
                            &mut stream,
                            200,
                            r#"{"data":[{"id":"gpt-binding","context_window":4096}]}"#,
                        );
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        if stopped.load(Ordering::Acquire) {
                            break;
                        }
                        assert!(
                            Instant::now() < deadline,
                            "metadata control exceeded its bound"
                        );
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    Err(error) => panic!("metadata listener: {error}"),
                }
            }
            gets
        });
        Self {
            stop,
            thread: Some(thread),
        }
    }

    fn finish(mut self) -> usize {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        self.thread
            .take()
            .unwrap()
            .join()
            .expect("metadata-only control")
    }
}

impl Drop for RefusalMetadataServer {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct TemporaryJournal(PathBuf);

impl TemporaryJournal {
    fn new(label: &str) -> Self {
        Self(std::env::temp_dir().join(format!("solstone-{label}-{}-{}",
            std::process::id(), std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos())))
    }
}

impl Drop for TemporaryJournal {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct OwnedChild(Option<Child>);

impl OwnedChild {
    fn spawn(command: &mut Command) -> Self {
        Self(Some(command.spawn().expect("native child")))
    }

    fn finish_output(mut self) -> Output {
        let deadline = Instant::now() + Duration::from_secs(35);
        loop {
            if self
                .0
                .as_mut()
                .unwrap()
                .try_wait()
                .expect("child status")
                .is_some()
            {
                break;
            }
            assert!(Instant::now() < deadline, "native child exceeded its bound");
            std::thread::sleep(Duration::from_millis(20));
        }
        let output = self
            .0
            .take()
            .unwrap()
            .wait_with_output()
            .expect("child output");
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    fn finish_receipt(self) -> serde_json::Value {
        let output = self.finish_output();
        let stdout = String::from_utf8(output.stdout).unwrap();
        let receipts = stdout
            .lines()
            .filter_map(|line| line.strip_prefix(RECEIPT))
            .collect::<Vec<_>>();
        assert_eq!(receipts.len(), 1, "one actual child receipt");
        serde_json::from_str(receipts[0]).unwrap()
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// What a child with a cleared environment still needs from its host. Windows
/// loads its socket providers through `SystemRoot`; without it every socket
/// call in the child fails before a request can leave.
fn platform_env() -> Vec<(&'static str, std::ffi::OsString)> {
    #[cfg(windows)]
    {
        std::env::var_os("SystemRoot")
            .map(|root| vec![("SystemRoot", root)])
            .unwrap_or_default()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

struct WireRequest {
    method: String,
    target: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl WireRequest {
    fn assert_openai(&self, credential: &str) {
        assert_eq!(self.method, "POST");
        assert_eq!(self.target, "/v1/responses");
        let auth = self
            .headers
            .iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case("authorization"))
            .map(|(_, value)| value.as_str())
            .collect::<Vec<_>>();
        assert_eq!(auth, vec![format!("Bearer {credential}").as_str()]);
        serde_json::from_str::<serde_json::Value>(&self.body).expect("actual JSON content body");
    }
}

fn accept_request(listener: &TcpListener) -> (TcpStream, String) {
    let (stream, request) = accept_content_request(listener);
    (stream, request.body)
}

fn accept_content_request(listener: &TcpListener) -> (TcpStream, WireRequest) {
    listener
        .set_nonblocking(true)
        .expect("nonblocking listener");
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "no native request within bound");
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(error) => panic!("native listener: {error}"),
            }
        };
        let request = read_request(&mut stream);
        if request.method == "GET" && request.target == "/v1/models" {
            assert!(request.body.is_empty(), "metadata must not carry content");
            write_http(&mut stream, 200, r#"{"data":[{"id":"gpt-binding"}]}"#);
            continue;
        }
        return (stream, request);
    }
}

fn read_request(stream: &mut TcpStream) -> WireRequest {
    // Windows hands an accepted socket the listener's non-blocking mode, so a
    // read that beats the client's bytes fails with WouldBlock instead of
    // waiting for them. Read blocking, bounded by the timeout below.
    stream
        .set_nonblocking(false)
        .expect("blocking accepted stream");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("timeout");
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .expect("write timeout");
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 2048];
    loop {
        let count = stream
            .read(&mut chunk)
            .expect("complete HTTP frame within bound");
        assert_ne!(count, 0, "truncated HTTP frame");
        buffer.extend_from_slice(&chunk[..count]);
        assert!(buffer.len() <= 1024 * 1024, "bounded native HTTP frame");
        if let Some(split) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            let head = std::str::from_utf8(&buffer[..split]).unwrap();
            let mut lines = head.split("\r\n");
            let request = lines.next().unwrap().split_whitespace().collect::<Vec<_>>();
            assert_eq!(request.len(), 3);
            assert_eq!(request[2], "HTTP/1.1");
            let headers = lines
                .map(|line| {
                    let (name, value) = line.split_once(':').expect("HTTP header");
                    (name.to_owned(), value.trim().to_owned())
                })
                .collect::<Vec<_>>();
            assert!(
                !headers
                    .iter()
                    .any(|(name, _)| name.eq_ignore_ascii_case("transfer-encoding"))
            );
            let lengths = headers
                .iter()
                .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .map(|(_, value)| value.parse::<usize>().unwrap())
                .collect::<Vec<_>>();
            assert!(lengths.len() <= 1, "duplicate content length");
            let length = lengths.first().copied().unwrap_or(0);
            assert!(length <= 1024 * 1024);
            let end = split + 4 + length;
            if buffer.len() < end {
                continue;
            }
            assert_eq!(buffer.len(), end, "one exact HTTP frame");
            return WireRequest {
                method: request[0].to_owned(),
                target: request[1].to_owned(),
                headers,
                body: String::from_utf8(buffer[split + 4..end].to_vec()).unwrap(),
            };
        }
    }
}

fn write_http(stream: &mut TcpStream, status: u16, body: &str) {
    let message = format!(
        "HTTP/1.1 {status} Binding\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(message.as_bytes())
        .expect("native response");
}
