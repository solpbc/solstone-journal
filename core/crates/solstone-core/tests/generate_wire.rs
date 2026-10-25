// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::net::TcpListener;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Sender, TryRecvError};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use solstone_core_generate::contract;
use solstone_core_generate_wire::{LaneOutcome, refusal_for, resolve_lane};

static NEXT_ROOT: AtomicUsize = AtomicUsize::new(0);

#[cfg(all(unix, feature = "full-tests"))]
mod applicable_credential_targets {
    use std::collections::BTreeMap;
    use std::fs;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::process::CommandExt;
    use std::path::Path;
    use std::process::{Child, Command, Stdio};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use serde_json::{Value, json};
    use solstone_core_brain::{CredentialHoldSession, CredentialHoldTarget};
    use solstone_core_generate::{Cancellation, ContentPart, GenerateRequest, GenerateResponse};
    use solstone_core_generate_wire::overrides::{API_KEY_OVERRIDE_ENV, BASE_URL_OVERRIDE_ENV};
    use solstone_core_thinking::providers::{ManagedKeyValidator, OneShotKeyValidator, save_key};

    const CHILD: &str = "SOLSTONE_APPLICABLE_TARGET_CHILD";
    const A: &str = "sk-applicable-override-a";
    const B: &str = "sk-applicable-configured-b";
    const CANARY: &str = "retained-applicable-target-input";

    struct OwnedChild(Option<Child>);

    impl OwnedChild {
        fn finish(&mut self, status: std::process::ExitStatus) -> Result<(), &'static str> {
            if !status.success() {
                return Err("actual caller leader failed");
            }
            let child = self.0.as_ref().unwrap();
            let group = nix::unistd::Pid::from_raw(child.id().try_into().unwrap());
            if nix::sys::signal::killpg(group, None) != Err(nix::errno::Errno::ESRCH) {
                return Err("caller process group is still occupied");
            }
            self.0.take();
            Ok(())
        }
    }

    impl Drop for OwnedChild {
        fn drop(&mut self) {
            // Include the real OneShot probe child on every parent unwind.
            if let Some(child) = self.0.as_mut() {
                let pid = nix::unistd::Pid::from_raw(child.id().try_into().expect("child pid"));
                let _ = nix::sys::signal::killpg(pid, nix::sys::signal::Signal::SIGKILL);
                let _ = child.wait();
            }
        }
    }

    fn request() -> GenerateRequest {
        GenerateRequest {
            id: None,
            context: "segment.thought".into(),
            contents: vec![ContentPart::Text {
                text: CANARY.into(),
            }],
            system_instruction: None,
            temperature: 0.2,
            max_output_tokens: 32,
            timeout_s: Some(30.0),
            json_output: false,
            json_schema: None,
            attempt_index: 0,
            exclusive_admission: false,
            transport_retries: Some(0),
        }
    }

    fn assert_a_refused(journal: &Path) {
        let result = solstone_core_generate_wire::respond(
            &request(),
            &solstone_core_generate_wire::EndpointRuntime::default(),
            journal,
            |_| None,
            &Cancellation::never(),
        )
        .expect("actual A dispatch");
        let GenerateResponse::Refused(refusal) = result else {
            panic!("rejected A must refuse content");
        };
        assert_eq!(
            refusal.reason_code.unwrap().as_wire(),
            "provider_key_invalid"
        );
    }

    fn hold(journal: &Path, key: &str) -> CredentialHoldSession {
        CredentialHoldSession::open(
            journal,
            CredentialHoldTarget::Managed {
                provider: "openai",
                credential: key,
            },
        )
        .unwrap()
    }

    fn capture(journal: &Path, key: &str) -> solstone_core_brain::CredentialValidationWitness {
        solstone_core_brain::capture_credential_validation_bound(
            Arc::new(solstone_core_journal_io::JournalRoot::open(journal).unwrap()),
            journal,
            CredentialHoldTarget::Managed {
                provider: "openai",
                credential: key,
            },
        )
        .expect("ordinary pre-probe witness")
    }

    fn settings_probe(journal: &Path, validator: &OneShotKeyValidator, key: &str, valid: bool) {
        let before = solstone_core_thinking::read_config(journal).unwrap();
        let prior_key = before["env"]["OPENAI_API_KEY"].as_str().unwrap();
        let prior_entry = before["providers"]["key_validation"]["openai"]
            .as_object()
            .map(|_| before["providers"]["key_validation"]["openai"].clone());
        let witness = capture(journal, key);
        let validation = validator
            .validate("openai", key)
            .expect("actual native canned probe");
        assert_eq!(validation["valid"], valid);
        if !valid {
            assert_eq!(validation["reason_code"], "provider_key_invalid");
        }
        let root = solstone_core_journal_io::JournalRoot::open(journal).unwrap();
        save_key(
            journal,
            &root,
            &Cancellation::never(),
            "OPENAI_API_KEY",
            "openai",
            key,
            Some(validation),
            prior_key,
            prior_entry.as_ref(),
            Some(witness),
        )
        .expect("ordinary Settings publication/settlement");
    }

    struct JoinedThread<T> {
        thread: Option<std::thread::JoinHandle<T>>,
        release: Arc<std::sync::atomic::AtomicBool>,
    }

    impl<T> JoinedThread<T> {
        fn finish(mut self) -> T {
            self.thread
                .take()
                .unwrap()
                .join()
                .expect("owned overlapping caller")
        }
    }

    impl<T> Drop for JoinedThread<T> {
        fn drop(&mut self) {
            self.release
                .store(false, std::sync::atomic::Ordering::Release);
            if let Some(thread) = self.thread.take() {
                // Publication locks unwind before this outer guard is dropped.
                // Actual provider and retainer calls keep their existing bounds.
                let _ = thread.join();
            }
        }
    }

    fn run_overlapping_child(journal: &Path) {
        use solstone_core_journal_io::{
            BoundPublicationPrimitive, run_with_two_bound_publication_barriers,
        };
        use std::sync::atomic::{AtomicBool, Ordering};
        struct Release(Arc<AtomicBool>);
        impl Drop for Release {
            fn drop(&mut self) {
                self.0.store(false, Ordering::Release);
            }
        }
        let held_a = Arc::new(hold(journal, A));
        let held_b = Arc::new(hold(journal, B));
        let blocked = Arc::new(AtomicBool::new(true));
        let path_a = journal.to_path_buf();
        let a_held = held_a.clone();
        let a_blocked = blocked.clone();
        let a_thread = JoinedThread {
            release: blocked.clone(),
            thread: Some(std::thread::spawn(move || {
                let deadline = Instant::now() + Duration::from_secs(30);
                // Pause only after the actual retainer has learned P_A. This keeps
                // its deferred carrier attempt from racing the positive's settle,
                // while P_A must protect the actual caller throughout publication.
                let check = || {
                    if a_held.is_held().unwrap() {
                        while a_blocked.load(Ordering::Acquire) {
                            assert!(
                                Instant::now() < deadline,
                                "P_A continuation exceeded fixture bound"
                            );
                            std::thread::sleep(Duration::from_millis(5));
                        }
                    }
                    false
                };
                solstone_core_generate_wire::respond(
                    &request(),
                    &solstone_core_generate_wire::EndpointRuntime::default(),
                    &path_a,
                    |_| None,
                    &Cancellation::from_fn(&check),
                )
            })),
        };
        let path_b = journal.to_path_buf();
        let b_thread = JoinedThread {
            release: blocked.clone(),
            thread: Some(std::thread::spawn(move || {
                let before = solstone_core_thinking::read_config(&path_b).unwrap();
                let witness = capture(&path_b, B);
                let validation = OneShotKeyValidator::at_path(path_b.join("generate-wrapper"))
                    .validate("openai", B)
                    .unwrap();
                assert_eq!(validation["reason_code"], "provider_key_invalid");
                let root = solstone_core_journal_io::JournalRoot::open(&path_b).unwrap();
                save_key(
                    &path_b,
                    &root,
                    &Cancellation::never(),
                    "OPENAI_API_KEY",
                    "openai",
                    B,
                    Some(validation),
                    B,
                    before["providers"]["key_validation"]["openai"]
                        .as_object()
                        .map(|_| &before["providers"]["key_validation"]["openai"]),
                    Some(witness),
                )
            })),
        };
        // Declared after both threads: release their barrier before outer thread
        // custody joins on any unwind, after the publisher's C/B locks unwind.
        let release = Release(blocked);
        let deadline = Instant::now() + Duration::from_secs(20);
        while !journal.join("actual-requests-pending").exists() {
            assert!(
                Instant::now() < deadline,
                "two actual requests were not pending"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        let validator = OneShotKeyValidator::at_path(journal.join("generate-wrapper"));
        let before = solstone_core_thinking::read_config(journal).unwrap();
        let witness = capture(journal, B);
        let validation = validator.validate("openai", B).unwrap();
        assert_eq!(validation["valid"], true);
        let authorized = journal.join("positive-native-authorized");
        let both_a = held_a.clone();
        let both_b = held_b.clone();
        let native_path = journal.to_path_buf();
        let native_seen = Arc::new(std::sync::Mutex::new(None));
        let observed = native_seen.clone();
        let root = solstone_core_journal_io::JournalRoot::open(journal).unwrap();
        let (saved, barriers) = run_with_two_bound_publication_barriers(
            BoundPublicationPrimitive::AuthorizedRename,
            1,
            move || {
                fs::write(authorized, b"release the declared original HTTP responses").unwrap();
                let deadline = Instant::now() + Duration::from_secs(5);
                while !both_a.is_held().unwrap() || !both_b.is_held().unwrap() {
                    assert!(
                        Instant::now() < deadline,
                        "both actual retainer facts must precede native rename"
                    );
                    std::thread::sleep(Duration::from_millis(5));
                }
            },
            BoundPublicationPrimitive::ParentSync,
            1,
            move || {
                let published = solstone_core_thinking::read_config(&native_path).unwrap();
                let entry = &published["providers"]["key_validation"]["openai"];
                assert_eq!(entry["valid"], true);
                *observed.lock().unwrap() = Some(
                    entry["credential_observation_id"]
                        .as_str()
                        .unwrap()
                        .to_owned(),
                );
            },
            || {
                save_key(
                    journal,
                    &root,
                    &Cancellation::never(),
                    "OPENAI_API_KEY",
                    "openai",
                    B,
                    Some(validation),
                    B,
                    before["providers"]["key_validation"]["openai"]
                        .as_object()
                        .map(|_| &before["providers"]["key_validation"]["openai"]),
                    Some(witness),
                )
            },
        );
        assert_eq!(
            barriers, 2,
            "both actual native publication stages executed"
        );
        saved.expect("newer B Settings success must settle");
        assert!(held_a.is_held().unwrap());
        assert!(!held_b.is_held().unwrap());
        drop(release);
        let old_b = b_thread.finish();
        assert!(matches!(
            old_b,
            Err(solstone_core_thinking::MutationError::StaleProbe)
        ));
        let old_a = a_thread.finish();
        // A's deferred publication may truthfully refuse after B changed the
        // durable carrier; its already learned process fact must still survive.
        assert!(matches!(old_a, Err(_) | Ok(GenerateResponse::Refused(_))));
        assert!(held_a.is_held().unwrap());
        assert!(!held_b.is_held().unwrap());
        assert_a_refused(journal);
        let corrected =
            solstone_core_generate::OneShotClient::at_path(journal.join("generate-wrapper"))
                .with_env(API_KEY_OVERRIDE_ENV, B)
                .execute(&request())
                .unwrap();
        assert!(matches!(corrected, GenerateResponse::Generated(_)));
        println!(
            "overlapping-native-config-observation {}",
            native_seen.lock().unwrap().as_ref().unwrap()
        );
    }

    fn run_child(journal: &Path, case: &str) {
        if case == "overlapping_rejections_survive_configured_b_native_publication" {
            run_overlapping_child(journal);
            assert_eq!(
                fs::read(journal.join("retained-input.txt")).unwrap(),
                CANARY.as_bytes()
            );
            return;
        }
        let validator = OneShotKeyValidator::at_path(journal.join("generate-wrapper"));
        // A is the existing dedicated process override, while Settings selects B.
        // The first call sends real content; later calls must never resend it.
        assert_a_refused(journal);
        assert!(hold(journal, A).is_held().unwrap());
        if case == "sequential_corrections_retire_only_unselectable_targets" {
            for index in 0..24 {
                let key = format!("sk-applicable-sequential-{index}");
                settings_probe(journal, &validator, &key, true);
                settings_probe(journal, &validator, &key, false);
                assert!(hold(journal, &key).is_held().unwrap());
                assert_a_refused(journal);
            }
        } else {
            settings_probe(journal, &validator, B, false);
            assert!(hold(journal, A).is_held().unwrap());
            assert!(hold(journal, B).is_held().unwrap());
            let mut pending = Vec::new();
            if case == "capacity_refusal_preserves_live_callers_and_existing_key_recovery" {
                for index in 0..6 {
                    let key = format!("sk-applicable-pending-{index}");
                    let witness = capture(journal, &key);
                    let validation = validator.validate("openai", &key).unwrap();
                    assert_eq!(validation["reason_code"], "provider_key_invalid");
                    // Keep real pre-work authority/results alive: their caller
                    // can still return. No config switch can retire these cells.
                    pending.push((witness, validation));
                }
                let before = fs::read(journal.join("config/journal.json")).unwrap();
                let root = Arc::new(solstone_core_journal_io::JournalRoot::open(journal).unwrap());
                let refusal = solstone_core_brain::capture_credential_validation_bound(
                    root,
                    journal,
                    CredentialHoldTarget::Managed {
                        provider: "openai",
                        credential: "sk-applicable-ninth",
                    },
                )
                .unwrap_err();
                assert!(
                    matches!(&refusal, solstone_core_brain::WriterError::Conflict(detail)
                    if detail == &solstone_core_brain::CredentialHoldError::Capacity.to_string())
                );
                println!("actual-caller-capacity-refusal {refusal}");
                assert_eq!(
                    fs::read(journal.join("config/journal.json")).unwrap(),
                    before
                );
                assert!(hold(journal, A).is_held().unwrap());
                assert!(hold(journal, B).is_held().unwrap());
            }
            settings_probe(journal, &validator, B, true);
            assert!(!hold(journal, B).is_held().unwrap());
            assert!(hold(journal, A).is_held().unwrap());
            assert_a_refused(journal);
            let response =
                solstone_core_generate::OneShotClient::at_path(journal.join("generate-wrapper"))
                    .with_env(API_KEY_OVERRIDE_ENV, B)
                    .execute(&request())
                    .expect("actual corrected B content");
            assert!(matches!(response, GenerateResponse::Generated(_)));
            drop(pending);
            drop(capture(journal, "sk-applicable-after-pending"));
        }
        assert_eq!(
            fs::read(journal.join("retained-input.txt")).unwrap(),
            CANARY.as_bytes()
        );
        let config = solstone_core_thinking::read_config(journal).unwrap();
        assert_eq!(config["providers"]["active"]["provider"], "openai");
        assert_eq!(config["providers"]["active"]["model"], "gpt-applicable");
    }

    struct WireRequest {
        key: String,
        text: String,
    }

    fn decode_request(bytes: &[u8]) -> Result<Option<WireRequest>, &'static str> {
        if bytes.len() > 65_536 {
            return Err("oversized frame");
        }
        let Some(split) = bytes.windows(4).position(|value| value == b"\r\n\r\n") else {
            return Ok(None);
        };
        let mut lines = std::str::from_utf8(&bytes[..split])
            .map_err(|_| "invalid headers")?
            .split("\r\n");
        if lines.next() != Some("POST /v1/responses HTTP/1.1") {
            return Err("unexpected request line");
        }
        let mut headers = Vec::new();
        for line in lines {
            let (name, value) = line.split_once(':').ok_or("invalid header")?;
            if name.is_empty()
                || !name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                || value.contains(['\r', '\n'])
            {
                return Err("invalid header syntax");
            }
            headers.push((name, value.trim()));
        }
        if headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("transfer-encoding"))
        {
            return Err("transfer encoding is not an exact frame");
        }
        let lengths = headers
            .iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            .map(|(_, value)| *value)
            .collect::<Vec<_>>();
        if lengths.len() != 1 || !lengths[0].bytes().all(|byte| byte.is_ascii_digit()) {
            return Err("missing or ambiguous length");
        }
        let length = lengths[0].parse::<usize>().map_err(|_| "invalid length")?;
        if length > 65_536 - (split + 4) {
            return Err("oversized body");
        }
        let end = split + 4 + length;
        if bytes.len() < end {
            return Ok(None);
        }
        if bytes.len() != end {
            return Err("extra frame bytes");
        }
        let auth = headers
            .iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case("authorization"))
            .map(|(_, value)| *value)
            .collect::<Vec<_>>();
        if auth.len() != 1 {
            return Err("missing or ambiguous authorization");
        }
        let key = auth[0]
            .strip_prefix("Bearer ")
            .filter(|key| !key.is_empty())
            .ok_or("invalid authorization")?;
        let body =
            std::str::from_utf8(&bytes[split + 4..end]).map_err(|_| "invalid body encoding")?;
        let value: Value = serde_json::from_str(body).map_err(|_| "invalid JSON body")?;
        let text = value["input"][0]["content"][0]["text"]
            .as_str()
            .ok_or("missing input text")?;
        if ![CANARY, "Reply with the single word OK."].contains(&text)
            || value["input"]
                != json!([{"role":"user","content":[{"type":"input_text","text":text}]}])
            || value["store"] != false
            || value["model"]
                != if text == CANARY {
                    "gpt-applicable"
                } else {
                    "solstone-key-check"
                }
            || body.contains(key)
        {
            return Err("unexpected actual provider input");
        }
        Ok(Some(WireRequest {
            key: key.into(),
            text: text.into(),
        }))
    }

    fn read_request(stream: &mut TcpStream) -> WireRequest {
        stream
            .set_nonblocking(false)
            .expect("make actual caller request stream blocking");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut bytes = Vec::new();
        let mut chunk = [0; 4096];
        loop {
            let count = stream.read(&mut chunk).unwrap();
            assert!(count > 0 && bytes.len() + count <= 65_536);
            bytes.extend_from_slice(&chunk[..count]);
            if let Some(request) = decode_request(&bytes).expect("strict actual HTTP frame") {
                return request;
            }
        }
    }

    fn run(case: &str) {
        if std::env::var(CHILD).ok().as_deref() == Some(case) {
            run_child(Path::new(&std::env::var("SOLSTONE_JOURNAL").unwrap()), case);
            println!("applicable-target-control-complete {case}");
            return;
        }
        let temporary = tempfile::tempdir().unwrap();
        let journal = temporary.path();
        super::write_config(
            journal,
            json!({
                "env": {"OPENAI_API_KEY": B},
                "providers": {"active": {"provider": "openai", "model": "gpt-applicable"}}
            }),
        );
        solstone_core_brain::generate_fingerprint_key(journal).unwrap();
        fs::write(journal.join("retained-input.txt"), CANARY).unwrap();
        let wrapper = journal.join("generate-wrapper");
        let binary = env!("CARGO_BIN_EXE_solstone-core").replace('\'', "'\\''");
        fs::write(
            &wrapper,
            format!("#!/bin/sh\nexec '{binary}' generate \"$@\"\n"),
        )
        .unwrap();
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let stdout = journal.join("child.stdout");
        let stderr = journal.join("child.stderr");
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .arg("--exact")
            .arg(format!(
                "{}::{case}",
                module_path!().split_once("::").unwrap().1
            ))
            .arg("--nocapture")
            .env_clear()
            .env("PATH", "")
            .env(CHILD, case)
            .env("SOLSTONE_JOURNAL", journal)
            .env(API_KEY_OVERRIDE_ENV, A)
            .env(
                BASE_URL_OVERRIDE_ENV,
                format!("http://{}", listener.local_addr().unwrap()),
            )
            .stdin(Stdio::null())
            .stdout(fs::File::create(&stdout).unwrap())
            .stderr(fs::File::create(&stderr).unwrap())
            .process_group(0);
        let mut child = OwnedChild(Some(command.spawn().unwrap()));
        let deadline = Instant::now() + Duration::from_secs(180);
        let mut posts = BTreeMap::<String, usize>::new();
        let mut content = BTreeMap::<String, usize>::new();
        let overlap = case == "overlapping_rejections_survive_configured_b_native_publication";
        let mut pending = Vec::<TcpStream>::new();
        loop {
            assert!(
                Instant::now() < deadline,
                "bounded actual caller control timed out"
            );
            if pending.len() == 2 && journal.join("positive-native-authorized").exists() {
                for mut stream in pending.drain(..) {
                    stream
                        .write_all(super::http_response(401, "{}").as_bytes())
                        .unwrap();
                }
            }
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let request = read_request(&mut stream);
                    let key = request.key.as_str();
                    let count = posts.entry(key.into()).or_default();
                    *count += 1;
                    let is_content = request.text == CANARY;
                    if overlap
                        && ((is_content && key == A) || (!is_content && key == B && *count == 1))
                    {
                        if is_content {
                            *content.entry(key.into()).or_default() += 1;
                        }
                        pending.push(stream);
                        if pending.len() == 2 {
                            fs::write(
                                journal.join("actual-requests-pending"),
                                b"both original real requests are pending",
                            )
                            .unwrap();
                        }
                        continue;
                    }
                    let status = if is_content {
                        *content.entry(key.into()).or_default() += 1;
                        if key == A {
                            401
                        } else {
                            assert_eq!(key, B);
                            200
                        }
                    } else {
                        assert_eq!(request.text, "Reply with the single word OK.");
                        if key.starts_with("sk-applicable-sequential-") {
                            if *count == 1 {
                                200
                            } else {
                                assert_eq!(*count, 2);
                                401
                            }
                        } else if key.starts_with("sk-applicable-pending-") {
                            assert_eq!(*count, 1);
                            401
                        } else {
                            assert_eq!(key, B);
                            if *count == 1 {
                                401
                            } else {
                                assert_eq!(*count, 2);
                                200
                            }
                        }
                    };
                    let response = if status == 200 {
                        r#"{"model":"gpt-applicable","status":"completed","output":[{"content":[{"type":"output_text","text":"OK"}]}]}"#
                    } else {
                        "{}"
                    };
                    stream
                        .write_all(super::http_response(status, response).as_bytes())
                        .unwrap();
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => panic!("actual caller HTTP accept: {error}"),
            }
            if let Some(status) = child.0.as_mut().unwrap().try_wait().unwrap() {
                assert!(
                    status.success(),
                    "actual child failed:\n{}\n{}",
                    fs::read_to_string(&stdout).unwrap(),
                    fs::read_to_string(&stderr).unwrap()
                );
                assert!(
                    fs::read_to_string(&stdout)
                        .unwrap()
                        .contains(&format!("applicable-target-control-complete {case}")),
                    "the child must execute its selected control"
                );
                child
                    .finish(status)
                    .expect("successful leader must leave no child group");
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            content.get(A),
            Some(&1),
            "no repeated A content after definitive rejection"
        );
        assert!(
            pending.is_empty(),
            "original response streams must be released"
        );
        if case == "sequential_corrections_retire_only_unselectable_targets" {
            assert_eq!(posts.len(), 25);
            assert_eq!(content.len(), 1);
            for index in 0..24 {
                assert_eq!(posts[&format!("sk-applicable-sequential-{index}")], 2);
            }
        } else {
            assert_eq!(content.get(B), Some(&1));
            assert_eq!(posts.get(B), Some(&3));
            assert!(!posts.contains_key("sk-applicable-ninth"));
            assert_eq!(
                posts.len(),
                if case == "capacity_refusal_preserves_live_callers_and_existing_key_recovery" {
                    8
                } else {
                    2
                }
            );
            if case == "capacity_refusal_preserves_live_callers_and_existing_key_recovery" {
                for index in 0..6 {
                    assert_eq!(posts[&format!("sk-applicable-pending-{index}")], 1);
                }
                assert!(
                    fs::read_to_string(&stdout)
                        .unwrap()
                        .contains("actual-caller-capacity-refusal")
                );
            }
        }
        assert_eq!(
            fs::read(journal.join("retained-input.txt")).unwrap(),
            CANARY.as_bytes()
        );
        println!(
            "actual-native-caller-receipt {}",
            json!({"case":case,"posts_by_fixture_credential":posts,"content_posts_by_fixture_credential":content,"child_stdout":fs::read_to_string(&stdout).unwrap()})
        );
    }

    #[test]
    fn overlapping_rejections_survive_configured_b_native_publication() {
        run("overlapping_rejections_survive_configured_b_native_publication");
    }

    #[test]
    fn configured_b_success_keeps_rejected_override_a() {
        run("configured_b_success_keeps_rejected_override_a");
    }

    #[test]
    fn capacity_refusal_preserves_live_callers_and_existing_key_recovery() {
        run("capacity_refusal_preserves_live_callers_and_existing_key_recovery");
    }

    #[test]
    fn sequential_corrections_retire_only_unselectable_targets() {
        run("sequential_corrections_retire_only_unselectable_targets");
    }

    #[test]
    fn actual_caller_frame_instrument_rejects_ambiguous_or_mutated_evidence() {
        let body = json!({"model":"gpt-applicable", "store":false,
            "input":[{"role":"user","content":[{"type":"input_text","text":CANARY}]}]});
        let frame = |auth: &str, extra: &str, body: &str| {
            format!(
                "POST /v1/responses HTTP/1.1\r\n{auth}Content-Length: {}\r\n{extra}\r\n{body}",
                body.len()
            )
        };
        let auth = format!("Authorization: Bearer {A}\r\n");
        let body = body.to_string();
        let positive = frame(&auth, "", &body);
        let accepts_a = |bytes: &[u8]| {
            decode_request(bytes)
                .ok()
                .flatten()
                .is_some_and(|request| request.key == A && request.text == CANARY)
        };
        assert!(accepts_a(positive.as_bytes()));
        let mut wrong_input: Value = serde_json::from_str(&body).unwrap();
        wrong_input["input"][0]["content"][0]["text"] = json!("invented-input");
        let mut extra_input: Value = serde_json::from_str(&body).unwrap();
        extra_input["input"]
            .as_array_mut()
            .unwrap()
            .push(json!({"role":"user","content":[]}));
        let mut exposed_key: Value = serde_json::from_str(&body).unwrap();
        exposed_key["unexpected_key"] = json!(A);
        for (name, mutated) in [
            ("missing auth", frame("", "", &body)),
            (
                "wrong auth",
                frame(&format!("Authorization: Bearer {B}\r\n"), "", &body),
            ),
            (
                "duplicate auth",
                frame(&auth, &format!("aUtHoRiZaTiOn: Bearer {B}\r\n"), &body),
            ),
            (
                "duplicate length",
                frame(&auth, &format!("cOnTeNt-LeNgTh: {}\r\n", body.len()), &body),
            ),
            (
                "transfer encoding",
                frame(&auth, "Transfer-Encoding: chunked\r\n", &body),
            ),
            ("truncated", positive[..positive.len() - 1].into()),
            ("extra bytes", format!("{positive}x")),
            ("invalid JSON", frame(&auth, "", "{")),
            ("wrong input", frame(&auth, "", &wrong_input.to_string())),
            ("extra input", frame(&auth, "", &extra_input.to_string())),
            ("key in body", frame(&auth, "", &exposed_key.to_string())),
        ] {
            assert!(!accepts_a(mutated.as_bytes()), "instrument accepted {name}");
        }
    }

    #[test]
    fn failed_leader_keeps_process_group_custody() {
        // Both group members are direct children so this adverse guard control
        // can reap them itself; no orphan or detached fixture is needed.
        struct Peer(Option<Child>);
        impl Drop for Peer {
            fn drop(&mut self) {
                if let Some(child) = self.0.as_mut() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
        }
        let mut leader = OwnedChild(Some(
            Command::new("/bin/sleep")
                .arg("60")
                .process_group(0)
                .spawn()
                .unwrap(),
        ));
        let group = leader.0.as_ref().unwrap().id();
        let mut peer = Peer(Some(
            Command::new("/bin/sleep")
                .arg("60")
                .process_group(group.try_into().unwrap())
                .spawn()
                .unwrap(),
        ));
        leader.0.as_mut().unwrap().kill().unwrap();
        let status = leader.0.as_mut().unwrap().wait().unwrap();
        assert_eq!(leader.finish(status), Err("actual caller leader failed"));
        drop(leader);
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(status) = peer.0.as_mut().unwrap().try_wait().unwrap() {
                use std::os::unix::process::ExitStatusExt;
                assert_eq!(status.signal(), Some(nix::libc::SIGKILL));
                peer.0.take();
                break;
            }
            assert!(
                Instant::now() < deadline,
                "failed leader lost its live group member"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

fn root(name: &str) -> PathBuf {
    let suffix = NEXT_ROOT.fetch_add(1, Ordering::Relaxed);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let root = Path::new("/var/tmp").join(format!(
        "solstone-core-generate-wire-{name}-{stamp}-{suffix}"
    ));
    std::fs::create_dir_all(root.join("health")).expect("create health directory");
    root
}

fn fixture_vector(id: &str) -> &'static Value {
    contract()["conformance_vectors"]
        .as_array()
        .expect("vectors")
        .iter()
        .find(|vector| vector["id"] == id)
        .expect("fixture vector")
}

fn http_response(status: u16, body: &str) -> String {
    format!(
        "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn serve(completion: &str) -> (u16, thread::JoinHandle<usize>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind stub server");
    let port = listener.local_addr().expect("stub address").port();
    let completion = completion.to_owned();
    let handle = thread::spawn(move || {
        let mut requests = 0;
        for _ in 0..6 {
            let (mut stream, _) = listener.accept().expect("accept request");
            let mut bytes = [0_u8; 8192];
            let read = stream.read(&mut bytes).expect("read request");
            let request = String::from_utf8_lossy(&bytes[..read]);
            let path = request
                .split_whitespace()
                .nth(1)
                .expect("HTTP request path");
            let body = match path {
                "/health" => r#"{"loaded_model":"served"}"#,
                "/props" => r#"{"n_ctx":16384,"total_slots":1}"#,
                "/tokenize" => r#"{"tokens":[1]}"#,
                "/v1/chat/completions/input_tokens" => {
                    r#"{"object":"response.input_tokens","input_tokens":1}"#
                }
                "/v1/chat/completions" => &completion,
                other => panic!("unexpected local HTTP path: {other}"),
            };
            stream
                .write_all(http_response(200, body).as_bytes())
                .expect("write response");
            requests += 1;
        }
        requests
    });
    (port, handle)
}

#[derive(Debug)]
struct RecordedRequest {
    path: String,
    body: String,
}

struct RecordingServer {
    stop: Sender<()>,
    handle: thread::JoinHandle<Vec<RecordedRequest>>,
}

impl RecordingServer {
    fn finish(self) -> Vec<RecordedRequest> {
        // A disconnected receiver means the server panicked; join preserves that failure.
        let _ = self.stop.send(());
        self.handle.join().expect("join recording server")
    }
}

/// Record requests until the client has finished, independent of scheduling gaps
/// or the number of capacity probes. Dropping the sender also stops the server
/// when a test fails before it can join.
fn serve_recording(completions: Vec<(u16, String)>) -> (u16, RecordingServer) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind recording server");
    listener
        .set_nonblocking(true)
        .expect("make recording server nonblocking");
    let port = listener
        .local_addr()
        .expect("recording server address")
        .port();
    let (stop, stopped) = mpsc::channel();
    let handle = thread::spawn(move || {
        let mut requests = Vec::new();
        let mut completion_index = 0;
        loop {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream
                        .set_nonblocking(false)
                        .expect("make recording request stream blocking");
                    stream
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .expect("set request read timeout");
                    let request = read_recorded_request(&mut stream);
                    let (status, body) =
                        match request.path.as_str() {
                            "/health" => (200, r#"{"loaded_model":"served"}"#.to_owned()),
                            "/props" => (200, r#"{"n_ctx":16384,"total_slots":1}"#.to_owned()),
                            "/tokenize" => (200, r#"{"tokens":[1]}"#.to_owned()),
                            "/v1/chat/completions/input_tokens" => (
                                200,
                                r#"{"object":"response.input_tokens","input_tokens":1}"#.to_owned(),
                            ),
                            "/v1/chat/completions" => {
                                let response =
                                    completions.get(completion_index).cloned().unwrap_or_else(
                                        || (500, r#"{"error":"unexpected completion"}"#.into()),
                                    );
                                completion_index += 1;
                                response
                            }
                            _ => (404, r#"{"error":"unexpected path"}"#.to_owned()),
                        };
                    stream
                        .write_all(http_response(status, &body).as_bytes())
                        .expect("write recording response");
                    requests.push(request);
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if !matches!(stopped.try_recv(), Err(TryRecvError::Empty)) {
                        return requests;
                    }
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("accept recording request: {error}"),
            }
        }
    });
    (port, RecordingServer { stop, handle })
}

fn read_recorded_request(stream: &mut std::net::TcpStream) -> RecordedRequest {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 4096];
    let header_end = loop {
        let read = stream.read(&mut chunk).expect("read recorded request");
        assert!(read > 0, "request ended before HTTP headers");
        bytes.extend_from_slice(&chunk[..read]);
        if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break end + 4;
        }
    };
    let header = std::str::from_utf8(&bytes[..header_end]).expect("ASCII HTTP headers");
    let path = header
        .split_whitespace()
        .nth(1)
        .expect("HTTP request path")
        .to_owned();
    let content_length = header
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim())
        })
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    while bytes.len() < header_end + content_length {
        let read = stream.read(&mut chunk).expect("read recorded request body");
        assert!(read > 0, "request ended before HTTP body");
        bytes.extend_from_slice(&chunk[..read]);
    }
    RecordedRequest {
        path,
        body: String::from_utf8(bytes[header_end..header_end + content_length].to_vec())
            .expect("UTF-8 request body"),
    }
}

#[test]
fn recording_server_survives_late_start_and_idle_client() {
    let (port, server) = serve_recording(Vec::new());
    // Cross both old shutdown windows: five seconds before the first request,
    // then 150 ms of idle time between requests.
    for delay in [Duration::from_millis(5500), Duration::from_millis(200)] {
        thread::sleep(delay);
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", port))
            .expect("recording server remains available until the client finishes");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set response read timeout");
        stream
            .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .expect("write delayed health request");
        let mut response = String::new();
        stream
            .read_to_string(&mut response)
            .expect("read health response");
        assert_eq!(response, http_response(200, r#"{"loaded_model":"served"}"#));
    }
    let requests = server.finish();
    assert_eq!(requests.len(), 2);
    assert!(requests.iter().all(|request| request.path == "/health"));
}

fn serve_endpoint() -> (u16, thread::JoinHandle<usize>) {
    serve_endpoint_with_completion(generated_body().to_owned())
}

fn serve_endpoint_with_completion(completion: String) -> (u16, thread::JoinHandle<usize>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind endpoint stub server");
    let port = listener.local_addr().expect("endpoint stub address").port();
    let handle = thread::spawn(move || {
        let mut completions = 0;
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().expect("accept endpoint request");
            let mut bytes = [0_u8; 8192];
            let read = stream.read(&mut bytes).expect("read endpoint request");
            let path = String::from_utf8_lossy(&bytes[..read])
                .split_whitespace()
                .nth(1)
                .expect("endpoint HTTP request path")
                .to_owned();
            let body = match path.as_str() {
                "/v1/models" => r#"{"data":[{"id":"served","max_model_len":4096}]}"#,
                "/v1/chat/completions" => {
                    completions += 1;
                    &completion
                }
                other => panic!("unexpected endpoint HTTP path: {other}"),
            };
            stream
                .write_all(http_response(200, body).as_bytes())
                .expect("write endpoint response");
        }
        completions
    });
    (port, handle)
}

fn write_config(root: &Path, config: Value) {
    std::fs::create_dir_all(root.join("config")).expect("create config directory");
    std::fs::write(root.join("config/journal.json"), config.to_string()).expect("write config");
}

fn bundled_config(confidential: bool) -> Value {
    if confidential {
        json!({"providers": {"active": {"provider": "local"}}, "services": {"confidential": {}}})
    } else {
        json!({"providers": {"active": {"provider": "local"}}})
    }
}

fn run(root: &Path, args: &[&str], input: Option<&Value>) -> Output {
    let input = input.map(Value::to_string);
    run_bytes(root, args, input.as_deref().map(str::as_bytes))
}

fn run_bytes(root: &Path, args: &[&str], input: Option<&[u8]>) -> Output {
    run_bytes_with_path(root, args, input, None)
}

fn run_bytes_with_path(
    root: &Path,
    args: &[&str],
    input: Option<&[u8]>,
    path: Option<&Path>,
) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_solstone-core"));
    command
        .args(args)
        .env("SOLSTONE_JOURNAL", root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(path) = path {
        command.env("PATH", path);
    }
    let mut child = command.spawn().expect("start solstone-core generate");
    if let Some(input) = input {
        child
            .stdin
            .as_mut()
            .expect("generate stdin")
            .write_all(input)
            .expect("write generate input");
    }
    child.wait_with_output().expect("wait for generate")
}

#[cfg(unix)]
fn poison_mlx_commands(root: &Path) -> (PathBuf, PathBuf) {
    let bin = root.join("poison-bin");
    let marker = root.join("mlx-poisoned");
    std::fs::create_dir_all(&bin).expect("create poison bin");
    for command in ["python", "python3", "mlx-vlm-server"] {
        let path = bin.join(command);
        std::fs::write(
            &path,
            format!("#!/bin/sh\nprintf x >> {}\n", marker.display()),
        )
        .expect("write poison command");
        let mut permissions = std::fs::metadata(&path)
            .expect("poison command metadata")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&path, permissions).expect("make poison command executable");
    }
    (bin, marker)
}

fn one_shot(root: &Path, input: &Value) -> Output {
    run(root, &["generate", "--one-shot"], Some(input))
}

fn stdout_json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).expect("one JSON stdout record")
}

fn stderr_protocol_error(output: &Output) -> Value {
    assert!(output.stdout.is_empty(), "stdout: {:?}", output.stdout);
    serde_json::from_slice(&output.stderr).expect("one protocol error stderr record")
}

fn generated_body() -> &'static str {
    r#"{"choices":[{"message":{"content":"OK"},"finish_reason":"stop"}],"usage":{"prompt_tokens":2,"completion_tokens":1,"total_tokens":3}}"#
}

fn completion_body(text: &str, finish_reason: &str, usage: Option<Value>) -> String {
    let mut body = json!({
        "choices": [{"message": {"content": text}, "finish_reason": finish_reason}],
    });
    if let Some(usage) = usage {
        body["usage"] = usage;
    }
    body.to_string()
}

fn token_entries(journal: &Path) -> Vec<Value> {
    let zone = solstone_core_journal_config::owner_zone(journal);
    let day = chrono::Utc::now()
        .with_timezone(&zone)
        .format("%Y%m%d")
        .to_string();
    let path = journal.join("tokens").join(format!("{day}.jsonl"));
    std::fs::read_to_string(path)
        .expect("usage log")
        .lines()
        .map(|line| serde_json::from_str(line).expect("usage JSON"))
        .collect()
}

fn assert_only_local_requests(requests: &[RecordedRequest]) {
    assert!(!requests.is_empty(), "loopback server observed no requests");
    for request in requests {
        assert!(
            matches!(
                request.path.as_str(),
                "/health"
                    | "/props"
                    | "/tokenize"
                    | "/v1/chat/completions/input_tokens"
                    | "/v1/chat/completions"
            ),
            "unaccounted loopback request: {}",
            request.path
        );
    }
}

#[test]
fn contract_is_the_compiled_fixture_bytes() {
    let journal = root("contract");
    let output = run(&journal, &["generate", "--contract"], None);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stderr, b"");
    assert_eq!(
        output.stdout,
        include_bytes!("../../../fixtures/generate_contract.json")
    );
    let _ = std::fs::remove_dir_all(journal);
}

#[test]
fn bundled_generation_emits_generated_response_and_hints() {
    for (name, attempt_index, expected_hints) in [
        ("plain", 0, None),
        ("attempt", 1, Some(json!(["attempt_index"]))),
    ] {
        let journal = root(name);
        write_config(&journal, bundled_config(false));
        let (port, server) = serve(generated_body());
        std::fs::write(journal.join("health/local.port"), port.to_string()).expect("write port");
        let mut request = fixture_vector("generated")["request"].clone();
        request["attempt_index"] = json!(attempt_index);
        let output = one_shot(&journal, &request);
        assert_eq!(
            output.status.code(),
            Some(fixture_vector("generated")["exit_code"].as_i64().unwrap() as i32)
        );
        assert_eq!(output.stderr, b"");
        let response = stdout_json(&output);
        let expected = &fixture_vector("generated")["response"];
        assert_eq!(response["schema"], expected["schema"]);
        assert_eq!(response["id"], expected["id"]);
        assert_eq!(response["outcome"], expected["outcome"]);
        assert_eq!(response["text"], expected["text"]);
        assert_eq!(response["usage"], expected["usage"]);
        assert_eq!(response["finish_reason"], expected["finish_reason"]);
        assert_eq!(response.get("hints_applied"), expected_hints.as_ref());
        assert_eq!(server.join().expect("join server"), 6);
        let _ = std::fs::remove_dir_all(journal);
    }
}

#[test]
fn bundled_native_loopback_enforces_schema_output_and_sends_multimodal_response_format() {
    let schema = json!({
        "type": "object",
        "properties": {"answer": {"type": "string"}},
        "required": ["answer"],
    });
    let cases = [
        (
            "valid",
            completion_body(
                r#"{"answer":"OK"}"#,
                "stop",
                Some(json!({"prompt_tokens": 2, "completion_tokens": 1})),
            ),
            "generated",
        ),
        ("invalid", r#"{"choices":"not an array"}"#.into(), "refused"),
        (
            "empty",
            completion_body(
                "",
                "stop",
                Some(json!({"prompt_tokens": 2, "completion_tokens": 1})),
            ),
            "refused",
        ),
    ];
    for (name, completion, expected_outcome) in cases {
        let journal = root(&format!("native-schema-{name}"));
        write_config(&journal, bundled_config(false));
        let (port, server) = serve_recording(vec![(200, completion)]);
        std::fs::write(journal.join("health/local.port"), port.to_string()).expect("write port");
        let mut request = fixture_vector("generated")["request"].clone();
        request["contents"] = json!([
            {"type": "text", "text": "describe this image"},
            {"type": "image", "mime_type": "image/png", "data": "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4AWP4z8AAAAMBAQDbfS68AAAAAElFTkSuQmCC"},
        ]);
        request["json_output"] = json!(true);
        request["json_schema"] = schema.clone();

        let response = stdout_json(&one_shot(&journal, &request));
        let requests = server.finish();
        assert_eq!(response["outcome"], expected_outcome, "{name}: {response}");
        if expected_outcome == "generated" {
            assert_eq!(response["schema_validation"]["valid"], true);
            assert_eq!(response["text"], r#"{"answer":"OK"}"#);
        } else {
            assert_eq!(response["provider"], "local");
        }
        assert_only_local_requests(&requests);
        let chat = requests
            .iter()
            .find(|request| request.path == "/v1/chat/completions")
            .expect("chat completion request");
        let body: Value = serde_json::from_str(&chat.body).expect("chat request JSON");
        assert_eq!(body["response_format"]["type"], "json_schema");
        assert_eq!(
            body["response_format"]["json_schema"]["schema"], schema,
            "{name} response format"
        );
        assert_eq!(body["messages"][0]["content"][1]["type"], "image_url");
        let _ = std::fs::remove_dir_all(journal);
    }
}

#[test]
fn bundled_native_schema_validation_is_advisory() {
    let journal = root("native-schema-advisory");
    write_config(&journal, bundled_config(false));
    let (port, server) = serve_recording(vec![(
        200,
        completion_body("{}", "stop", Some(json!({"prompt_tokens": 2}))),
    )]);
    std::fs::write(journal.join("health/local.port"), port.to_string()).expect("write port");
    let mut request = fixture_vector("generated")["request"].clone();
    request["json_output"] = json!(true);
    request["json_schema"] = json!({
        "type": "object",
        "properties": {"answer": {"type": "string"}},
        "required": ["answer"],
    });

    let response = stdout_json(&one_shot(&journal, &request));
    let requests = server.finish();
    assert_eq!(
        response["outcome"], "generated",
        "full response: {response}"
    );
    assert_eq!(response["schema_validation"]["valid"], false);
    assert_eq!(
        response["schema_validation"]["errors"][0]["constraint"],
        "required"
    );
    assert_only_local_requests(&requests);
    let _ = std::fs::remove_dir_all(journal);
}

#[cfg(unix)]
#[test]
fn refusing_native_endpoint_stays_local_and_never_spawns_mlx() {
    let journal = root("native-refusing");
    write_config(&journal, bundled_config(false));
    let config = bundled_config(false)
        .as_object()
        .expect("bundled config")
        .clone();
    assert_eq!(
        resolve_lane(&config),
        ("local".into(), LaneOutcome::BundledLocal)
    );
    let (port, server) = serve_recording(vec![(200, r#"{"choices":"native refused"}"#.into())]);
    std::fs::write(journal.join("health/local.port"), port.to_string()).expect("write port");
    let (poison_bin, marker) = poison_mlx_commands(&journal);
    let input = fixture_vector("generated")["request"].to_string();
    let output = run_bytes_with_path(
        &journal,
        &["generate", "--one-shot"],
        Some(input.as_bytes()),
        Some(&poison_bin),
    );
    let requests = server.finish();
    assert_eq!(output.status.code(), Some(0));
    let response = stdout_json(&output);
    assert_eq!(response["outcome"], "refused");
    assert_eq!(response["provider"], "local");
    assert!(response["reason_code"].is_string(), "explicit local error");
    assert_only_local_requests(&requests);
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.path == "/v1/chat/completions")
            .count(),
        1,
        "the loopback fake accounted for the only completion request"
    );
    assert!(
        !marker.exists()
            || std::fs::read(&marker)
                .expect("read poison marker")
                .is_empty(),
        "native request spawned an MLX command"
    );
    let _ = std::fs::remove_dir_all(journal);
}

#[test]
fn malformed_requests_surface_as_protocol_errors() {
    let journal = root("malformed");
    let mut wrong_schema = fixture_vector("generated")["request"].clone();
    wrong_schema["schema"] = json!("wrong");
    let mut unknown_top_level = fixture_vector("generated")["request"].clone();
    unknown_top_level["unexpected"] = json!(true);
    let mut invalid_content = fixture_vector("generated")["request"].clone();
    invalid_content["contents"] = json!([{"type": "text", "text": "OK", "extra": true}]);
    for request in [&wrong_schema, &unknown_top_level, &invalid_content] {
        let output = one_shot(&journal, request);
        assert_eq!(
            output.status.code(),
            Some(
                contract()["exit_codes"]["malformed_request"]
                    .as_i64()
                    .unwrap() as i32
            )
        );
        let error = stderr_protocol_error(&output);
        assert_eq!(error["schema"], contract()["schema_identifiers"]["error"]);
        assert_eq!(error["reason"], "malformed-request");
        assert!(error["id"].is_null());
    }
    let error = stderr_protocol_error(&one_shot(&journal, &unknown_top_level));
    assert!(error["detail"].as_str().unwrap().contains("unexpected"));
    let _ = std::fs::remove_dir_all(journal);
}

#[test]
fn invalid_utf8_stdin_is_a_malformed_request() {
    let journal = root("invalid-utf8");
    let output = run_bytes(&journal, &["generate", "--one-shot"], Some(&[0xff]));
    assert_eq!(
        output.status.code(),
        Some(
            contract()["exit_codes"]["malformed_request"]
                .as_i64()
                .unwrap() as i32
        )
    );
    let error = stderr_protocol_error(&output);
    assert_eq!(error["reason"], "malformed-request");
    assert!(error["id"].is_null());
    let _ = std::fs::remove_dir_all(journal);
}

#[test]
fn lane_refusals_use_fixture_fields_without_network_calls() {
    let cases = [
        ("none", json!({}), "refused-no-engine-configured", true),
        (
            "google-missing-key",
            json!({"providers": {"active": {"provider": "google"}}}),
            "refused-provider-response-invalid",
            false,
        ),
        (
            "openai-missing-key",
            json!({"providers": {"active": {"provider": "openai"}}}),
            "refused-provider-response-invalid",
            false,
        ),
        (
            "unimplemented-provider",
            json!({"providers": {"active": {"provider": "not-a-real-provider"}}}),
            "refused-provider-response-invalid",
            false,
        ),
    ];
    for (name, config, vector_id, exact_reason_code) in cases {
        let journal = root(name);
        if name != "none" {
            write_config(&journal, config);
        }
        let output = one_shot(&journal, &fixture_vector("generated")["request"]);
        assert_eq!(output.status.code(), Some(0));
        assert_eq!(output.stderr, b"");
        let response = stdout_json(&output);
        let expected = &fixture_vector(vector_id)["response"];
        assert_eq!(response["reason"], expected["reason"], "{vector_id} reason");
        if name == "google-missing-key" || name == "openai-missing-key" {
            assert_eq!(
                response["detail"], "the configured provider could not produce a usable response",
                "{vector_id} detail"
            );
        } else if name == "unimplemented-provider" {
            assert_eq!(
                response["detail"],
                "no generate implementation exists for the resolved provider lane",
                "{vector_id} detail"
            );
        } else {
            assert_eq!(response["detail"], expected["detail"], "{vector_id} detail");
        }
        if name == "google-missing-key" {
            assert_eq!(response["provider"], "google");
            assert_eq!(response["reason_code"], "provider_key_missing");
            assert_eq!(response["retryable"], true);
            assert_eq!(response["blocking"], true);
        } else if name == "openai-missing-key" {
            assert_eq!(response["provider"], "openai");
            assert_eq!(response["reason_code"], "provider_key_missing");
            assert_eq!(response["retryable"], true);
            assert_eq!(response["blocking"], true);
        } else if name == "unimplemented-provider" {
            assert_eq!(response["provider"], "not-a-real-provider");
        } else {
            assert_eq!(
                response["provider"], expected["provider"],
                "{vector_id} provider"
            );
        }
        if exact_reason_code {
            for name in ["reason_code", "retryable", "blocking"] {
                assert_eq!(response[name], expected[name], "{vector_id} {name}");
            }
        }
        assert!(!journal.join("tokens").exists());
        let _ = std::fs::remove_dir_all(journal);
    }
}

#[test]
fn confidential_lane_refusal_matches_not_verified_fixture_without_network() {
    let config = json!({"providers": {"active": {"provider": "local"}, "local": {"endpoint_url": "https://endpoint", "served_model_id": "served"}}, "services": {"confidential": {}}});
    let journal = root("attestation");
    write_config(&journal, config);
    let output = one_shot(&journal, &fixture_vector("generated")["request"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stderr, b"");
    let response = stdout_json(&output);
    let vector_id = "refused-attestation-not-verified";
    let expected = &fixture_vector(vector_id)["response"];
    assert_eq!(response["reason"], expected["reason"], "{vector_id} reason");
    assert_eq!(response["detail"], "nvattest_unavailable");
    assert_eq!(
        response["provider"], expected["provider"],
        "{vector_id} provider"
    );
    for name in ["reason_code", "retryable", "blocking"] {
        assert_eq!(response[name], expected[name], "{vector_id} {name}");
    }
    assert!(!journal.join("tokens").exists());
    let _ = std::fs::remove_dir_all(journal);
}

#[test]
fn unreachable_byo_endpoint_uses_the_endpoint_unreachable_code() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("reserve endpoint port");
    let port = listener.local_addr().expect("endpoint address").port();
    drop(listener);
    let journal = root("endpoint-unreachable");
    write_config(
        &journal,
        json!({"providers": {"active": {"provider": "local"}, "local": {
            "endpoint_url": format!("http://127.0.0.1:{port}"),
            "served_model_id": "served",
        }}}),
    );
    let output = one_shot(&journal, &fixture_vector("generated")["request"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stderr, b"");
    let response = stdout_json(&output);
    let expected = &fixture_vector("refused-provider-response-invalid")["response"];
    for name in ["reason", "provider"] {
        assert_eq!(response[name], expected[name], "{name}");
    }
    assert_eq!(
        response["detail"],
        "the configured provider could not produce a usable response"
    );
    assert_eq!(response["reason_code"], "local_endpoint_unreachable");
    assert_eq!(response["retryable"], true);
    assert_eq!(response["blocking"], true);
    let _ = std::fs::remove_dir_all(journal);
}

#[test]
fn bundled_failure_uses_provider_response_invalid_reason() {
    let journal = root("bundled-failure");
    write_config(&journal, bundled_config(false));
    let (port, server) = serve(r#"{"choices":"bad"}"#);
    std::fs::write(journal.join("health/local.port"), port.to_string()).expect("write port");
    let output = one_shot(&journal, &fixture_vector("generated")["request"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stderr, b"");
    let response = stdout_json(&output);
    let expected = &fixture_vector("refused-provider-response-invalid")["response"];
    for name in ["reason", "provider"] {
        assert_eq!(response[name], expected[name], "{name}");
    }
    assert_eq!(
        response["detail"],
        "the configured provider could not produce a usable response"
    );
    assert_eq!(response["reason_code"], "response_invalid");
    assert_eq!(response["retryable"], false);
    assert_eq!(response["blocking"], true);
    assert_eq!(server.join().expect("join server"), 6);
    let _ = std::fs::remove_dir_all(journal);
}

#[test]
fn bundled_confidential_is_generated_and_logs_usage_while_refusals_do_not_log() {
    let journal = root("usage");
    write_config(&journal, bundled_config(true));
    let (port, server) = serve(generated_body());
    std::fs::write(journal.join("health/local.port"), port.to_string()).expect("write port");
    let output = one_shot(&journal, &fixture_vector("generated")["request"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(server.join().expect("join server"), 6);
    let zone = solstone_core_journal_config::owner_zone(&journal);
    let day = chrono::Utc::now()
        .with_timezone(&zone)
        .format("%Y%m%d")
        .to_string();
    let token_path = journal.join("tokens").join(format!("{day}.jsonl"));
    let text = std::fs::read_to_string(&token_path).expect("usage log");
    let lines = text.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 1);
    let value: Value = serde_json::from_str(lines[0]).expect("usage JSON");
    let keys = value
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    assert_eq!(
        keys,
        BTreeSet::from([
            "context".into(),
            "model".into(),
            "timestamp".into(),
            "type".into(),
            "usage".into()
        ])
    );

    let refusal_journal = root("no-usage-refusal");
    let refusal = one_shot(&refusal_journal, &fixture_vector("generated")["request"]);
    assert_eq!(refusal.status.code(), Some(0));
    assert!(!refusal_journal.join("tokens").exists());
    let _ = std::fs::remove_dir_all(journal);
    let _ = std::fs::remove_dir_all(refusal_journal);
}

#[test]
fn malformed_generate_arguments_use_protocol_errors() {
    let journal = root("arguments");
    let minimum = contract()["framing"]["session"]["concurrency"]["minimum"]
        .as_u64()
        .unwrap();
    let below_minimum = minimum.saturating_sub(1).to_string();
    for args in [
        &["generate", "--session"][..],
        &["generate", "--session", "--max-in-flight", &below_minimum][..],
        &["generate", "--session", "--max-in-flight", "-1"][..],
        &["generate", "--session", "--max-in-flight", "not-an-integer"][..],
        &["generate", "--session", "wrong-flag", "3"][..],
        &["generate"][..],
        &["generate", "--bogus"][..],
        &["generate", "--contract", "extra-arg"][..],
    ] {
        let output = run(&journal, args, None);
        assert_eq!(
            output.status.code(),
            Some(
                contract()["exit_codes"]["malformed_request"]
                    .as_i64()
                    .unwrap() as i32
            )
        );
        let error = stderr_protocol_error(&output);
        assert_eq!(error["reason"], "malformed-request");
    }
    let _ = std::fs::remove_dir_all(journal);
}

#[test]
fn n3_boundary_does_not_emit_incomplete_text_or_schema_validation_failed() {
    let journal = root("truncated-text");
    write_config(&journal, bundled_config(false));
    let body = completion_body(
        "partial answer",
        "length",
        Some(json!({"prompt_tokens": 2, "completion_tokens": 1})),
    );
    let (port, server) = serve(&body);
    std::fs::write(journal.join("health/local.port"), port.to_string()).expect("write port");
    let output = one_shot(&journal, &fixture_vector("generated")["request"]);
    let response = stdout_json(&output);
    assert_eq!(response["outcome"], "generated");
    assert_eq!(response["finish_reason"], "max_tokens");
    assert_ne!(
        response.get("reason").and_then(Value::as_str),
        Some("incomplete-text")
    );
    assert_eq!(server.join().expect("join server"), 6);

    let schema_journal = root("schema-advisory");
    write_config(&schema_journal, bundled_config(false));
    let body = completion_body("{}", "stop", Some(json!({"prompt_tokens": 2})));
    let (port, server) = serve(&body);
    std::fs::write(schema_journal.join("health/local.port"), port.to_string()).expect("write port");
    let mut request = fixture_vector("generated")["request"].clone();
    request["json_schema"] = json!({"type": "object", "required": ["answer"]});
    request["json_output"] = json!(true);
    let response = stdout_json(&one_shot(&schema_journal, &request));
    assert_eq!(response["outcome"], "generated");
    assert_ne!(
        response.get("reason").and_then(Value::as_str),
        Some("schema-validation-failed")
    );
    assert_eq!(server.join().expect("join server"), 6);
    let _ = std::fs::remove_dir_all(journal);
    let _ = std::fs::remove_dir_all(schema_journal);
}

#[test]
fn schema_validation_is_advisory_for_bundled_and_byo_lanes() {
    let schema = json!({
        "type": "object",
        "properties": {"answer": {"type": "string"}},
        "required": ["answer"],
    });
    let cases = [
        ("invalid", "{}", false),
        ("valid", r#"{"answer":"OK"}"#, true),
    ];

    for (name, text, expected_valid) in cases {
        let journal = root(&format!("schema-bundled-{name}"));
        write_config(&journal, bundled_config(false));
        let body = completion_body(text, "stop", Some(json!({"prompt_tokens": 2})));
        let (port, server) = serve(&body);
        std::fs::write(journal.join("health/local.port"), port.to_string()).expect("write port");
        let mut request = fixture_vector("generated")["request"].clone();
        request["json_schema"] = schema.clone();
        request["json_output"] = json!(true);
        let response = stdout_json(&one_shot(&journal, &request));
        assert_eq!(response["outcome"], "generated");
        assert_eq!(response["schema_validation"]["valid"], expected_valid);
        if expected_valid {
            assert_eq!(response["schema_validation"]["errors"], json!([]));
        } else {
            assert_eq!(
                response["schema_validation"]["errors"][0]["constraint"],
                "required"
            );
        }
        assert_eq!(server.join().expect("join bundled server"), 6);
        let _ = std::fs::remove_dir_all(journal);
    }

    for (name, text, expected_valid) in cases {
        let journal = root(&format!("schema-endpoint-{name}"));
        let body = completion_body(text, "stop", Some(json!({"prompt_tokens": 2})));
        let (port, server) = serve_endpoint_with_completion(body);
        write_config(
            &journal,
            json!({
                "providers": {"active": {"provider": "local"}, "local": {
                    "endpoint_url": format!("http://127.0.0.1:{port}"),
                    "served_model_id": "served",
                }},
            }),
        );
        let mut request = fixture_vector("generated")["request"].clone();
        request["json_schema"] = schema.clone();
        request["json_output"] = json!(true);
        let response = stdout_json(&one_shot(&journal, &request));
        assert_eq!(response["outcome"], "generated");
        assert_eq!(response["schema_validation"]["valid"], expected_valid);
        if expected_valid {
            assert_eq!(response["schema_validation"]["errors"], json!([]));
        } else {
            assert_eq!(
                response["schema_validation"]["errors"][0]["constraint"],
                "required"
            );
        }
        assert_eq!(server.join().expect("join endpoint server"), 1);
        let _ = std::fs::remove_dir_all(journal);
    }
}

#[test]
fn completed_provider_results_log_before_strict_validation() {
    let cases = [
        (
            "blank-with-usage",
            "",
            "stop",
            Some(json!({"prompt_tokens": 2, "completion_tokens": 1})),
            true,
            "provider-response-invalid",
        ),
        (
            "blank-without-usage",
            "",
            "stop",
            None,
            false,
            "provider-response-invalid",
        ),
        (
            "json-truncated",
            "partial JSON",
            "length",
            Some(json!({"prompt_tokens": 2, "completion_tokens": 1})),
            true,
            "incomplete-json",
        ),
    ];
    for (name, text, finish_reason, usage, writes_usage, reason) in cases {
        let journal = root(name);
        write_config(&journal, bundled_config(false));
        let body = completion_body(text, finish_reason, usage);
        let (port, server) = serve(&body);
        std::fs::write(journal.join("health/local.port"), port.to_string()).expect("write port");
        let mut request = fixture_vector("generated")["request"].clone();
        if reason == "incomplete-json" {
            request["json_output"] = json!(true);
        }
        let response = stdout_json(&one_shot(&journal, &request));
        assert_eq!(response["outcome"], "refused");
        assert_eq!(response["reason"], reason);
        if reason == "incomplete-json" {
            assert_eq!(response["reason_code"], "incomplete_json_length");
        } else {
            assert_eq!(response["reason_code"], "provider_response_invalid");
        }
        assert_eq!(journal.join("tokens").exists(), writes_usage);
        if writes_usage {
            assert_eq!(token_entries(&journal).len(), 1);
        }
        assert_eq!(server.join().expect("join server"), 6);
        let _ = std::fs::remove_dir_all(journal);
    }
}

#[test]
fn attestation_stale_refusal_matches_fixture_vector_fields() {
    // The one-shot binary owns a fresh process-local state store; producing a
    // stale session would require waiting for the real cadence deadline.
    let expected = &fixture_vector("refused-attestation-stale")["response"];
    let refusal = refusal_for(
        &LaneOutcome::AttestationStale,
        "local",
        Some("fixture-request".into()),
    );
    assert_eq!(
        refusal.reason.as_str(),
        expected["reason"].as_str().expect("fixture reason")
    );
    assert_eq!(
        refusal.reason_code.as_ref().map(|code| code.as_wire()),
        expected["reason_code"].as_str()
    );
    assert_eq!(refusal.retryable, expected["retryable"]);
    assert_eq!(refusal.blocking, expected["blocking"]);
    assert_eq!(refusal.detail, expected["detail"]);
    assert_eq!(refusal.provider.as_deref(), expected["provider"].as_str());
}

fn create_test_package(root: &Path) -> solstone_core_installed_payload::InstalledPackage {
    use solstone_core_installed_payload::{
        COMPILED_VERSION, INSTALLED_PAYLOAD_MANIFEST, InstalledPackage, PRODUCT, compiled_target,
        render_installed_payload,
    };
    let bin = root.join("lib/solstone-nvattest/bin/nvattest");
    #[cfg(target_os = "macos")]
    let lib = root.join("lib/solstone-nvattest/lib/libnvat.1.dylib");
    #[cfg(not(target_os = "macos"))]
    let lib = root.join("lib/solstone-nvattest/lib/libnvat.so.1");
    let ca = root.join("lib/solstone-nvattest/share/ca/ca-bundle.pem");
    std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
    std::fs::create_dir_all(lib.parent().unwrap()).unwrap();
    std::fs::create_dir_all(ca.parent().unwrap()).unwrap();
    std::fs::write(&bin, b"binary placeholder").unwrap();
    std::fs::write(&lib, b"library placeholder").unwrap();
    std::fs::write(&ca, b"ca placeholder").unwrap();
    let manifest =
        render_installed_payload(root, PRODUCT, COMPILED_VERSION, compiled_target(), "commit")
            .unwrap();
    let manifest_path = root.join(INSTALLED_PAYLOAD_MANIFEST);
    std::fs::create_dir_all(manifest_path.parent().unwrap()).unwrap();
    std::fs::write(manifest_path, manifest).unwrap();
    InstalledPackage::admit(root, COMPILED_VERSION, compiled_target()).unwrap()
}

fn plant_marker_dir(dir: &Path) {
    let bin = dir.join("bin/nvattest");
    let lib = dir.join("lib");
    let ca = dir.join("share/ca/ca-bundle.pem");
    std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
    std::fs::create_dir_all(&lib).unwrap();
    std::fs::create_dir_all(ca.parent().unwrap()).unwrap();
    let marker_file = dir.join("bin/marker.txt");
    let script = format!("#!/bin/sh\ntouch \"{}\"\n", marker_file.display());
    std::fs::write(&bin, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    std::fs::write(lib.join("placeholder.txt"), b"lib placeholder").unwrap();
    std::fs::write(&ca, b"ca placeholder").unwrap();
}

fn dir_snapshot(path: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut files = Vec::new();
    fn walk(base: &Path, dir: &Path, out: &mut Vec<(PathBuf, Vec<u8>)>) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            let mut entries: Vec<_> = entries.filter_map(|e| e.ok()).collect();
            entries.sort_by_key(|e| e.path());
            for entry in entries {
                let p = entry.path();
                if p.is_dir() {
                    walk(base, &p, out);
                } else if p.is_file() {
                    let rel = p.strip_prefix(base).unwrap().to_path_buf();
                    let bytes = std::fs::read(&p).unwrap();
                    out.push((rel, bytes));
                }
            }
        }
    }
    walk(path, path, &mut files);
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files
}

#[test]
fn confidential_channel_failure_uses_attestation_failed_vector() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind attestation stub");
    let port = listener.local_addr().expect("attestation address").port();
    let dropped = thread::spawn(move || {
        let (_stream, _) = listener.accept().expect("accept attestation connection");
    });
    let journal = root("confidential-attestation-failed");
    let pkg_root = journal.join("pkg");
    let package = create_test_package(&pkg_root);

    let request_str = fixture_vector("generated")["request"].to_string();
    let request =
        solstone_core_generate_wire::parse_one_shot_request(&request_str).expect("parse request");
    let endpoint = solstone_core_local::ByoEndpoint {
        base_url: format!("http://127.0.0.1:{port}"),
        served_model_id: "served".into(),
        credential: Some("token".into()),
        parallel_slots: None,
        is_bundled: false,
        is_confidential: true,
    };
    let config = serde_json::Map::new();
    let runtime = solstone_core_generate_wire::EndpointRuntime::default();

    let result = solstone_core_generate_wire::confidential_generate_in_package(
        &request, &journal, &endpoint, &config, &runtime, &package,
    );

    let solstone_core_generate_wire::ConfidentialResult::AttestationFailed(detail) = result else {
        panic!("expected AttestationFailed");
    };

    let refusal = refusal_for(
        &LaneOutcome::AttestationFailed(detail),
        "local",
        request.id.clone(),
    );
    let expected = &fixture_vector("refused-attestation-failed")["response"];
    assert_eq!(
        refusal.reason.as_str(),
        expected["reason"].as_str().unwrap()
    );
    assert_eq!(
        refusal.reason_code.as_ref().map(|code| code.as_wire()),
        expected["reason_code"].as_str()
    );
    assert_eq!(refusal.retryable, expected["retryable"].as_bool().unwrap());
    assert_eq!(refusal.blocking, expected["blocking"].as_bool().unwrap());
    assert_eq!(refusal.provider.as_deref(), expected["provider"].as_str());
    assert_eq!(detail, "gateway_unreachable");

    dropped.join().expect("join attestation stub");
    let _ = std::fs::remove_dir_all(journal);
}

#[allow(unsafe_code)]
#[test]
fn planted_nvattest_locations_are_never_executed_or_mutated() {
    let journal = root("planted-locations");
    let dir1 = journal.join("cache/providers/nvattest");
    let dir2 = journal.join("config_nvattest");
    let dir3 = journal.join("env_nvattest");

    plant_marker_dir(&dir1);
    plant_marker_dir(&dir2);
    plant_marker_dir(&dir3);

    let snap1 = dir_snapshot(&dir1);
    let snap2 = dir_snapshot(&dir2);
    let snap3 = dir_snapshot(&dir3);

    let mut config_map = serde_json::Map::new();
    let mut services = serde_json::Map::new();
    let mut confidential = serde_json::Map::new();
    confidential.insert(
        "nvattest_dir".to_string(),
        json!(dir2.to_string_lossy().to_string()),
    );
    services.insert("confidential".to_string(), Value::Object(confidential));
    config_map.insert("services".to_string(), Value::Object(services));

    unsafe {
        std::env::set_var("SPP_NVATTEST_DIR", &dir3);
    }

    struct EnvGuard;
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            unsafe {
                std::env::remove_var("SPP_NVATTEST_DIR");
            }
        }
    }
    let _guard = EnvGuard;

    // Part A: Valid package
    {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let port = listener.local_addr().expect("port").port();
        let dropped = thread::spawn(move || {
            let (_stream, _) = listener.accept().expect("accept connection");
        });
        let pkg_root = journal.join("valid_pkg");
        let package = create_test_package(&pkg_root);

        let request_str = fixture_vector("generated")["request"].to_string();
        let request = solstone_core_generate_wire::parse_one_shot_request(&request_str)
            .expect("parse request");
        let endpoint = solstone_core_local::ByoEndpoint {
            base_url: format!("http://127.0.0.1:{port}"),
            served_model_id: "served".into(),
            credential: Some("token".into()),
            parallel_slots: None,
            is_bundled: false,
            is_confidential: true,
        };
        let runtime = solstone_core_generate_wire::EndpointRuntime::default();

        let result = solstone_core_generate_wire::confidential_generate_in_package(
            &request,
            &journal,
            &endpoint,
            &config_map,
            &runtime,
            &package,
        );
        assert!(matches!(
            result,
            solstone_core_generate_wire::ConfidentialResult::AttestationFailed(_)
        ));

        assert!(!dir1.join("bin/marker.txt").exists());
        assert!(!dir2.join("bin/marker.txt").exists());
        assert!(!dir3.join("bin/marker.txt").exists());
        assert_eq!(dir_snapshot(&dir1), snap1);
        assert_eq!(dir_snapshot(&dir2), snap2);
        assert_eq!(dir_snapshot(&dir3), snap3);

        dropped.join().expect("join stub");
    }

    // Part B: Missing member (without lib/solstone-nvattest/bin/nvattest)
    {
        use solstone_core_installed_payload::{
            COMPILED_VERSION, INSTALLED_PAYLOAD_MANIFEST, InstalledPackage, PRODUCT,
            compiled_target, render_installed_payload,
        };
        let counting_listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        counting_listener.set_nonblocking(true).unwrap();
        let port = counting_listener.local_addr().expect("port").port();

        let pkg_root = journal.join("missing_bin_pkg");
        #[cfg(target_os = "macos")]
        let lib = pkg_root.join("lib/solstone-nvattest/lib/libnvat.1.dylib");
        #[cfg(not(target_os = "macos"))]
        let lib = pkg_root.join("lib/solstone-nvattest/lib/libnvat.so.1");
        let ca = pkg_root.join("lib/solstone-nvattest/share/ca/ca-bundle.pem");
        std::fs::create_dir_all(lib.parent().unwrap()).unwrap();
        std::fs::create_dir_all(ca.parent().unwrap()).unwrap();
        std::fs::write(&lib, b"library placeholder").unwrap();
        std::fs::write(&ca, b"ca placeholder").unwrap();
        let manifest = render_installed_payload(
            &pkg_root,
            PRODUCT,
            COMPILED_VERSION,
            compiled_target(),
            "commit",
        )
        .unwrap();
        let manifest_path = pkg_root.join(INSTALLED_PAYLOAD_MANIFEST);
        std::fs::create_dir_all(manifest_path.parent().unwrap()).unwrap();
        std::fs::write(manifest_path, manifest).unwrap();
        let package =
            InstalledPackage::admit(&pkg_root, COMPILED_VERSION, compiled_target()).unwrap();

        let request_str = fixture_vector("generated")["request"].to_string();
        let request = solstone_core_generate_wire::parse_one_shot_request(&request_str)
            .expect("parse request");
        let endpoint = solstone_core_local::ByoEndpoint {
            base_url: format!("http://127.0.0.1:{port}"),
            served_model_id: "served".into(),
            credential: Some("token".into()),
            parallel_slots: None,
            is_bundled: false,
            is_confidential: true,
        };
        let runtime = solstone_core_generate_wire::EndpointRuntime::default();

        let result = solstone_core_generate_wire::confidential_generate_in_package(
            &request,
            &journal,
            &endpoint,
            &config_map,
            &runtime,
            &package,
        );

        let solstone_core_generate_wire::ConfidentialResult::AttestationNotVerified(detail) =
            result
        else {
            panic!("expected AttestationNotVerified");
        };
        assert_eq!(detail, "nvattest_unavailable");

        assert!(!dir1.join("bin/marker.txt").exists());
        assert!(!dir2.join("bin/marker.txt").exists());
        assert!(!dir3.join("bin/marker.txt").exists());
        assert_eq!(dir_snapshot(&dir1), snap1);
        assert_eq!(dir_snapshot(&dir2), snap2);
        assert_eq!(dir_snapshot(&dir3), snap3);

        assert!(
            counting_listener.accept().is_err(),
            "stub accept count must stay 0"
        );
    }

    drop(_guard);
    let _ = std::fs::remove_dir_all(journal);
}

#[test]
fn byo_endpoint_generates_without_confidential_downgrade() {
    let (port, server) = serve_endpoint();
    let endpoint_url = format!("http://127.0.0.1:{port}");
    let config = json!({
        "providers": {"active": {"provider": "local"}, "local": {
            "endpoint_url": endpoint_url,
            "served_model_id": "served",
        }},
    });
    let journal = root("byo-endpoint");
    write_config(&journal, config.clone());
    let output = one_shot(&journal, &fixture_vector("generated")["request"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(stdout_json(&output)["outcome"], "generated");
    assert_eq!(server.join().expect("join endpoint server"), 1);

    let confidential_journal = root("byo-confidential");
    let mut confidential = config;
    confidential["services"] = json!({"confidential": {}});
    write_config(&confidential_journal, confidential);
    let output = one_shot(
        &confidential_journal,
        &fixture_vector("generated")["request"],
    );
    assert_eq!(output.status.code(), Some(0));
    let response = stdout_json(&output);
    assert_eq!(response["outcome"], "refused");
    assert_eq!(
        response["reason"],
        fixture_vector("refused-attestation-not-verified")["response"]["reason"]
    );
    let detail_or_reason = format!("{:?} {:?}", response.get("detail"), response.get("reason"));
    assert!(
        detail_or_reason.contains("nvattest_unavailable"),
        "expected nvattest_unavailable in {detail_or_reason}"
    );
    let _ = std::fs::remove_dir_all(journal);
    let _ = std::fs::remove_dir_all(confidential_journal);
}

#[test]
fn byo_endpoint_reports_only_honored_exclusive_admission_hint() {
    for exclusive_admission in [false, true] {
        let (port, server) = serve_endpoint();
        let journal = root("endpoint-hints");
        write_config(
            &journal,
            json!({
                "providers": {"active": {"provider": "local"}, "local": {
                    "endpoint_url": format!("http://127.0.0.1:{port}"),
                    "served_model_id": "served",
                }},
            }),
        );
        let mut request = fixture_vector("generated")["request"].clone();
        request["exclusive_admission"] = json!(exclusive_admission);
        let output = one_shot(&journal, &request);
        assert_eq!(output.status.code(), Some(0));
        let response = stdout_json(&output);
        if exclusive_admission {
            assert_eq!(response["hints_applied"], json!(["exclusive_admission"]));
        } else {
            assert!(response.get("hints_applied").is_none());
        }
        assert_eq!(server.join().expect("join endpoint server"), 1);
        let _ = std::fs::remove_dir_all(journal);
    }
}

#[test]
fn chatgpt_lane_without_credentials_refuses_with_sign_in_required() {
    let journal = root("chatgpt-no-cred");
    write_config(
        &journal,
        json!({
            "providers": {
                "active": {
                    "provider": "chatgpt",
                    "model": "gpt-test"
                }
            }
        }),
    );
    let output = one_shot(&journal, &fixture_vector("generated")["request"]);
    assert_eq!(output.status.code(), Some(0));
    let response = stdout_json(&output);
    assert_eq!(response["outcome"], "refused");
    assert_eq!(response["reason_code"], "chatgpt_sign_in_required");
    let _ = std::fs::remove_dir_all(journal);
}
