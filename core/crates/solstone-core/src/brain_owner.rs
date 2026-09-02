// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Owner-facing `journal brain` inspection and fenced refresh orchestration.

use std::collections::BTreeMap;
use std::env;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration as StdDuration, Instant, SystemTime};

use chrono::{DateTime, Duration, Utc};
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use serde_json::{Map, Value, json};
use solstone_core_cli::{JournalBrainOwnerCommand, JournalBrainRefreshOptions};
use solstone_core_generate::{
    ClientError, ContentPart, GenerateRequest, GenerateResponse, GeneratedResponse, OneShotClient,
    sibling_executable,
};

use crate::{EXIT_UNAVAILABLE, resolve_journal_config_path};

const COMPONENTS: [&str; 4] = [
    "configuration",
    "lane_prerequisites",
    "generate",
    "cogitate",
];

#[derive(Clone)]
struct View {
    aggregate_state: String,
    reason_code: Option<String>,
    lane: Option<String>,
    provider: Option<String>,
    model: Option<String>,
    fingerprint_sha256: Option<String>,
    failing_component: Option<String>,
    observed_at: Option<String>,
    expires_at: Option<String>,
    path: Option<String>,
    checked_age: Option<String>,
    transient: Option<&'static str>,
}

pub(crate) fn run(command: JournalBrainOwnerCommand) -> ExitCode {
    match command {
        JournalBrainOwnerCommand::Status { json } => run_status(json),
        JournalBrainOwnerCommand::Refresh(options) => run_owner_refresh(&options),
        JournalBrainOwnerCommand::RenewPrerequisites {
            json,
            expected_fingerprint,
        } => run_owner_renew_prerequisites(json, expected_fingerprint.as_deref()),
        _ => ExitCode::from(2),
    }
}

fn journal() -> Result<(PathBuf, Map<String, Value>), ExitCode> {
    let line = resolve_journal_config_path(None).map_err(|error| {
        crate::eprint_journal_path_error(error);
        ExitCode::from(EXIT_UNAVAILABLE)
    })?;
    let config = solstone_core_brain::read_journal_config(&line.path)
        .map_err(|error| {
            eprintln!("brain failed: could not read journal config: {error}");
            ExitCode::from(EXIT_UNAVAILABLE)
        })?
        .config
        .unwrap_or_default();
    Ok((line.path, config))
}

fn run_status(json_output: bool) -> ExitCode {
    let Ok((journal, config)) = journal() else {
        return ExitCode::from(EXIT_UNAVAILABLE);
    };
    let view = view(&journal, &config, Utc::now());
    render(&view, json_output);
    brain_exit_code(&view)
}

fn current_bundled_runtime_fingerprint(
    journal: &Path,
    config: &Map<String, Value>,
    nvidia_probe: Option<solstone_core_local::NvidiaProbe>,
) -> Option<String> {
    let configured = config
        .get("providers")
        .and_then(Value::as_object)
        .and_then(|providers| providers.get("active"))
        .and_then(Value::as_object)
        .and_then(|active| active.get("model"))
        .and_then(Value::as_str)
        .filter(|model| !model.trim().is_empty())
        .unwrap_or("local/qwen3.5-4b");
    let model_id = solstone_core_local::install::resolve_bundled_model_id(
        configured,
        cfg!(target_os = "macos"),
    );
    let readiness = bundled_runtime_readiness(journal, &model_id, nvidia_probe)?;
    let artifacts = readiness.get("artifacts")?.as_object()?;
    let model_path = artifacts.get("model_path")?.as_str()?;
    let backend = readiness
        .get("host")
        .and_then(Value::as_object)
        .and_then(|host| host.get("backend"))
        .and_then(Value::as_str)
        .unwrap_or("metal");
    let artifact_target = readiness
        .get("target")
        .and_then(Value::as_object)
        .and_then(|target| target.get("target_fingerprint_sha256"))
        .and_then(Value::as_str)
        .unwrap_or("");
    solstone_core_brain::bundled_runtime_desired_fingerprint(
        backend,
        &model_id,
        artifact_target,
        artifacts.get("binary_path").and_then(Value::as_str),
        model_path,
        artifacts.get("projector_path").and_then(Value::as_str),
    )
    .ok()
    .map(|desired| desired.sha256)
}

fn bundled_runtime_readiness(
    journal: &Path,
    model_id: &str,
    nvidia_probe: Option<solstone_core_local::NvidiaProbe>,
) -> Option<Value> {
    let journal = journal.display().to_string();
    #[cfg(target_os = "macos")]
    {
        let _ = nvidia_probe;
        solstone_core_local::install::metal_candidate::inspect(&Map::from_iter([
            ("journal".into(), Value::String(journal)),
            ("model_id".into(), Value::String(model_id.to_owned())),
            ("backend".into(), Value::String("metal".into())),
        ]))
        .ok()
    }
    #[cfg(not(target_os = "macos"))]
    {
        let mut input = Map::from_iter([
            ("journal".into(), Value::String(journal)),
            ("model_id".into(), Value::String(model_id.to_owned())),
        ]);
        if let Some(probe) = nvidia_probe {
            input.insert("nvidia_probe".into(), serde_json::to_value(probe).ok()?);
        }
        Some(solstone_core_local::install::readiness::inspect_local(
            input,
        ))
    }
}

/// The one full refresh path.  Unsafe prerequisite renewal delegates here.
fn run_owner_refresh(options: &JournalBrainRefreshOptions) -> ExitCode {
    let now = Utc::now();
    if options.expected_fingerprint.is_some() && options.expect_active_fingerprint_absent {
        return render_transient("stale_expected_fingerprint", options.json);
    }
    let Ok((journal, config)) = journal() else {
        return ExitCode::from(EXIT_UNAVAILABLE);
    };
    let before = view(&journal, &config, now);
    let initial_resolution = solstone_core_brain::derive_active_brain_lane(&config);
    // The Python writer wrapper always supplies the current bundled target,
    // even when another lane is active. The writer ignores it for non-bundled
    // lanes, while having it already captured closes a config-change race into
    // bundled between this read and `begin_refresh`.
    let bundled_runtime_fingerprint = current_bundled_runtime_fingerprint(&journal, &config, None);
    if let Some(expected) = options.expected_fingerprint.as_deref() {
        let actual = if options.expected_active_fingerprint {
            before.fingerprint_sha256.as_deref()
        } else {
            (initial_resolution.lane.as_deref() == Some("bundled"))
                .then_some(bundled_runtime_fingerprint.as_deref())
                .flatten()
        };
        if actual != Some(expected) {
            return render_transient("stale_expected_fingerprint", options.json);
        }
    }
    if before.reason_code.as_deref() == Some("configuration_invalid") {
        render(&before, options.json);
        return brain_exit_code(&before);
    }
    if options.expected_fingerprint.is_some()
        && !options.expected_active_fingerprint
        && before.aggregate_state == "ready"
    {
        render(&before, options.json);
        return brain_exit_code(&before);
    }
    let permit = match solstone_core_brain::begin_refresh(
        &journal,
        now,
        None,
        if options.expected_active_fingerprint {
            options.expected_fingerprint.as_deref()
        } else {
            None
        },
        options.expect_active_fingerprint_absent,
        bundled_runtime_fingerprint.clone(),
    ) {
        Ok(Some(permit)) => permit,
        Ok(None) => {
            // A failed lease probe must not manufacture a busy result: the
            // persisted state is authoritative unless a held lease proves it.
            if solstone_core_brain::probe_file_lease_held(
                &solstone_core_brain::brain_refresh_lease_path(&journal),
            )
            .unwrap_or(false)
            {
                let busy = transient_view("busy", &before);
                render(&busy, options.json);
                return brain_exit_code(&busy);
            }
            let current = view(&journal, &config, Utc::now());
            render(&current, options.json);
            return brain_exit_code(&current);
        }
        Err(solstone_core_brain::BeginRefreshError::ExpectedFingerprintStale(_)) => {
            return render_transient("stale_expected_fingerprint", options.json);
        }
        Err(error) => {
            eprintln!("brain refresh failed: {error}");
            return ExitCode::from(EXIT_UNAVAILABLE);
        }
    };

    let checking_config = match solstone_core_brain::read_journal_config(&journal) {
        Ok(read) => read.config.unwrap_or_default(),
        Err(_) => {
            return abandon_probe_failure(&journal, &config, permit, options.json, Utc::now());
        }
    };
    let checking_view = view(&journal, &checking_config, Utc::now());
    let (Some(lane), Some(_provider), Some(_model)) = (
        checking_view.lane.as_deref(),
        checking_view.provider.as_deref(),
        checking_view.model.as_deref(),
    ) else {
        return abandon_probe_failure(&journal, &checking_config, permit, options.json, Utc::now());
    };
    let outcome = probe_outcome(
        &journal,
        &checking_config,
        lane,
        bundled_runtime_fingerprint.as_deref(),
        now,
    );
    let finish_bundled_runtime_fingerprint = (lane == "bundled")
        .then(|| current_bundled_runtime_fingerprint(&journal, &checking_config, None))
        .flatten();
    if solstone_core_brain::finish_refresh(
        &journal,
        permit,
        outcome,
        Utc::now(),
        finish_bundled_runtime_fingerprint,
    )
    .is_err()
    {
        return render_transient("lost_fence", options.json);
    }
    let committed = view(&journal, &checking_config, Utc::now());
    render(&committed, options.json);
    brain_exit_code(&committed)
}

fn abandon_probe_failure(
    journal: &Path,
    fallback_config: &Map<String, Value>,
    permit: solstone_core_brain::BrainRefreshPermit,
    json_output: bool,
    now: DateTime<Utc>,
) -> ExitCode {
    if solstone_core_brain::abandon_refresh(
        journal,
        permit,
        "probe_internal_error",
        Map::new(),
        now,
    )
    .is_err()
    {
        return render_transient("lost_fence", json_output);
    }
    let config = solstone_core_brain::read_journal_config(journal)
        .ok()
        .and_then(|read| read.config)
        .unwrap_or_else(|| fallback_config.clone());
    let committed = view(journal, &config, Utc::now());
    render(&committed, json_output);
    brain_exit_code(&committed)
}

fn run_owner_renew_prerequisites(json_output: bool, expected: Option<&str>) -> ExitCode {
    let now = Utc::now();
    let Ok((journal, config)) = journal() else {
        return ExitCode::from(EXIT_UNAVAILABLE);
    };
    let current = view(&journal, &config, now);
    if expected.is_some() && current.fingerprint_sha256.as_deref() != expected {
        return render_transient("stale_expected_fingerprint", json_output);
    }
    match solstone_core_brain::begin_prerequisite_renewal(&journal, now, None, expected, None) {
        solstone_core_brain::BeginPrerequisiteRenewal::Started(permit) => {
            let component = spp_prerequisite(&journal, &config, now);
            if solstone_core_brain::finish_prerequisite_renewal(
                &journal,
                permit,
                component,
                Utc::now(),
                None,
            )
            .is_err()
            {
                return render_transient("lost_fence", json_output);
            }
            let committed = view(&journal, &config, Utc::now());
            render(&committed, json_output);
            brain_exit_code(&committed)
        }
        solstone_core_brain::BeginPrerequisiteRenewal::Busy { .. } => {
            let busy = transient_view("busy", &current);
            render(&busy, json_output);
            brain_exit_code(&busy)
        }
        solstone_core_brain::BeginPrerequisiteRenewal::Unsafe { .. } => {
            run_owner_refresh(&JournalBrainRefreshOptions {
                json: json_output,
                expected_fingerprint: expected.map(ToOwned::to_owned),
                expected_active_fingerprint: expected.is_some(),
                expect_active_fingerprint_absent: false,
            })
        }
    }
}

fn probe_outcome(
    journal: &Path,
    config: &Map<String, Value>,
    lane: &str,
    bundled_runtime_fingerprint: Option<&str>,
    now: DateTime<Utc>,
) -> Value {
    let lane_prerequisites =
        lane_prerequisite(journal, config, lane, bundled_runtime_fingerprint, now);
    if let Some(reason) = lane_prerequisites
        .get("reason_code")
        .and_then(Value::as_str)
    {
        return json!({
            "configuration": component_ok(now),
            "lane_prerequisites": lane_prerequisites,
            "generate": component_not_attempted(reason, now),
            "cogitate": component_not_attempted(reason, now),
        });
    }
    json!({
        "configuration": component_ok(now),
        "lane_prerequisites": lane_prerequisites,
        "generate": generate_component(now),
        "cogitate": cogitate_component(journal, config, now),
    })
}

fn lane_prerequisite(
    journal: &Path,
    config: &Map<String, Value>,
    lane: &str,
    bundled_runtime_fingerprint: Option<&str>,
    now: DateTime<Utc>,
) -> Value {
    match lane {
        "bundled" => {
            let assessment = solstone_core_brain::assess_bundled_runtime_prerequisite(
                journal,
                bundled_runtime_fingerprint,
            );
            assessment.reason_code.map_or_else(
                || component_ok(now),
                |reason| {
                    let diagnostic = runtime_diagnostic(
                        &reason,
                        assessment.phase.as_deref(),
                        assessment.runtime_reason.as_deref(),
                    );
                    component_for_reason("lane_prerequisites", &reason, diagnostic, now)
                },
            )
        }
        "byo-cloud" => {
            let provider = solstone_core_brain::derive_active_brain_lane(config).provider;
            let key_name = match provider.as_str() {
                "google" => "GOOGLE_API_KEY",
                "openai" => "OPENAI_API_KEY",
                "anthropic" => "ANTHROPIC_API_KEY",
                _ => return component_ok(now),
            };
            let configured = config
                .get("env")
                .and_then(Value::as_object)
                .and_then(|env| env.get(key_name))
                .and_then(Value::as_str);
            let present = configured.is_some_and(|key| !key.trim().is_empty())
                || env::var(key_name).is_ok_and(|key| !key.trim().is_empty());
            if present {
                component_ok(now)
            } else {
                component_for_reason(
                    "lane_prerequisites",
                    "provider_key_missing",
                    Map::new(),
                    now,
                )
            }
        }
        "spp" => spp_prerequisite(journal, config, now),
        _ => component_ok(now),
    }
}

fn spp_prerequisite(journal: &Path, config: &Map<String, Value>, now: DateTime<Utc>) -> Value {
    let endpoint = match solstone_core_local::resolve_local_endpoint(config) {
        solstone_core_local::LocalEndpointResolution::Byo(endpoint) if endpoint.is_confidential => {
            endpoint
        }
        _ => {
            return component_for_reason(
                "lane_prerequisites",
                "attestation_not_verified",
                Map::new(),
                now,
            );
        }
    };
    let nvattest_dir = config
        .get("services")
        .and_then(Value::as_object)
        .and_then(|services| services.get("confidential"))
        .and_then(Value::as_object)
        .and_then(|value| value.get("nvattest_dir"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| env::var_os("SPP_NVATTEST_DIR").map(PathBuf::from))
        .unwrap_or_else(|| journal.join("cache/providers/nvattest"));
    let state = solstone_core_spp_ratls::AttestationStateStore::new();
    match solstone_core_spp_ratls::perform_fresh_reattest(
        &state,
        &endpoint.base_url,
        &nvattest_dir,
        StdDuration::from_secs(120),
        solstone_core_spp_ratls::ensure_nvattest_installed,
    ) {
        Ok(channel) if channel.session.status(SystemTime::now()) == "verified" => {
            spp_component_ok(now, &channel.session)
        }
        Ok(_) => component_for_reason("lane_prerequisites", "attestation_expired", Map::new(), now),
        Err(failure) => {
            let reason = valid_spp_reason(failure.reason_code);
            component_for_reason("lane_prerequisites", reason, Map::new(), now)
        }
    }
}

fn valid_spp_reason(raw: &str) -> &'static str {
    let reason = spp_reason(raw);
    if solstone_core_brain::is_valid_evidence_reason("lane_prerequisites", reason) {
        reason
    } else {
        "attestation_rejected"
    }
}

fn spp_reason(raw: &str) -> &'static str {
    match raw {
        "gateway_unreachable" => "attestation_not_verified",
        "nvattest_install_in_progress" => "nvattest_install_in_progress",
        "nvattest_platform_unsupported" => "nvattest_platform_unsupported",
        "nvattest_unavailable" => "nvattest_unavailable",
        "nvattest_install_failed" => "nvattest_install_failed",
        "nvattest_integrity_failed" => "nvattest_integrity_failed",
        "tls_handshake_failed"
        | "proof_http_failed"
        | "attestation_failed"
        | "certificate_invalid"
        | "certificate_extension_missing"
        | "certificate_extension_not_critical"
        | "certificate_extension_invalid"
        | "certificate_evidence_invalid"
        | "nonce_mismatch"
        | "pcr_pin_mismatch"
        | "spki_mismatch"
        | "cpu_verification_failed"
        | "gpu_nonce_mismatch"
        | "gpu_appraisal_failed"
        | "composite_appraisal_failed"
        | "exporter_proof_invalid"
        | "exporter_mismatch"
        | "exporter_quote_failed"
        | "endpoint_invalid"
        | "unexpected_error" => "attestation_rejected",
        _ => "attestation_rejected",
    }
}

fn generate_component(now: DateTime<Utc>) -> Value {
    let request = GenerateRequest {
        id: None,
        context: "health.brain.generate".to_owned(),
        contents: vec![ContentPart::Text {
            text: "Reply with the single word OK.".to_owned(),
        }],
        system_instruction: None,
        temperature: 0.0,
        max_output_tokens: 512,
        thinking_budget: Some(0),
        timeout_s: Some(30.0),
        json_output: false,
        json_schema: None,
        enforce_responsiveness: false,
        attempt_index: 0,
        exclusive_admission: false,
        transport_retries: Some(0),
    };
    let result = OneShotClient::sibling().and_then(|client| client.execute(&request));
    let reason = match result {
        Ok(GenerateResponse::Generated(response)) => {
            let Some(reason) = classify_canned_generate(&response) else {
                return component_ok(now);
            };
            reason.to_owned()
        }
        Ok(GenerateResponse::Refused(response)) => map_provider_reason(
            "generate",
            response.reason_code.as_ref().map(|value| value.as_wire()),
        ),
        Err(ClientError::Protocol(failure)) => {
            map_provider_reason("generate", Some(&failure.error.reason))
        }
        Err(_) => "probe_internal_error".to_owned(),
    };
    component_for_reason("generate", &reason, Map::new(), now)
}

fn classify_canned_generate(response: &GeneratedResponse) -> Option<&'static str> {
    if response.finish_reason == "max_tokens" {
        return Some("probe_output_starved");
    }
    if !response.text.trim().is_empty() {
        return None;
    }
    if generated_response_has_reasoning(response) || response.finish_reason != "stop" {
        Some("probe_output_starved")
    } else {
        Some("provider_response_invalid")
    }
}

fn generated_response_has_reasoning(response: &GeneratedResponse) -> bool {
    response
        .thinking
        .as_ref()
        .and_then(Value::as_array)
        .is_some_and(|thinking| !thinking.is_empty())
        || response.usage.as_object().is_some_and(|usage| {
            usage.iter().any(|(name, value)| {
                name.contains("reasoning")
                    && !value.is_boolean()
                    && value.as_f64().is_some_and(|count| count > 0.0)
            })
        })
}

fn cogitate_component(journal: &Path, config: &Map<String, Value>, now: DateTime<Utc>) -> Value {
    let model = solstone_core_brain::derive_active_brain_lane(config)
        .model
        .unwrap_or_default();
    let request = json!({
        "schema": solstone_core_cogitate_wire::REQUEST_SCHEMA,
        "access_tier": "diagnostic", "outbound_approval": null, "diagnostic": true,
        "talent_instruction": null, "sol_tool_name": null, "read_scope": [], "output_path": null,
        "schedule": null, "max_turns": 2, "cost_cap_usd": 0.05, "context_window": null,
        "timeout_ms": 60000_u64, "read_call_budget": 1_i64, "model": model,
        "correlation_id": "health.brain.cogitate", "initial_prompt": "Call the emit_final tool exactly once with the content OK. Do not reply with plain text and do not call any other tool.",
        "journal_root": journal, "dry_run": false,
    });
    let Ok(input) = serde_json::to_vec(&request) else {
        return component_for_reason("cogitate", "probe_internal_error", Map::new(), now);
    };
    let Ok(executable) = sibling_executable() else {
        return component_for_reason("cogitate", "probe_internal_error", Map::new(), now);
    };
    let reason = match run_cogitate_with_outer_timeout(executable, input) {
        Ok(output) => terminal_cogitate_reason(&output.stdout).unwrap_or_else(|| {
            if output.status.success() {
                "cogitate_terminal_error".to_owned()
            } else {
                "probe_internal_error".to_owned()
            }
        }),
        Err(error) => cogitate_run_error_reason(error).to_owned(),
    };
    if reason == "ok" {
        component_ok(now)
    } else {
        component_for_reason("cogitate", &reason, Map::new(), now)
    }
}

fn run_cogitate_with_outer_timeout(
    executable: PathBuf,
    input: Vec<u8>,
) -> Result<std::process::Output, CogitateRunError> {
    let deadline = Instant::now() + StdDuration::from_secs(60);
    let (pid_sender, pid_receiver) = mpsc::sync_channel(1);
    let (result_sender, result_receiver) = mpsc::sync_channel(1);
    let child_pid = Arc::new(AtomicI32::new(0));
    let worker_pid = Arc::clone(&child_pid);
    thread::spawn(move || {
        let mut child = match Command::new(executable)
            .arg("cogitate")
            .arg("--one-shot")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(child) => child,
            Err(error) => {
                let _ = pid_sender.send(Err(error));
                return;
            }
        };
        worker_pid.store(child.id() as i32, Ordering::Release);
        let _ = pid_sender.send(Ok(child.id()));
        let result = (|| {
            child
                .stdin
                .as_mut()
                .ok_or_else(|| std::io::Error::other("missing stdin"))?
                .write_all(&input)?;
            child.wait_with_output()
        })();
        let _ = result_sender.send(result);
    });
    let pid = match pid_receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        Ok(Ok(pid)) => pid,
        Ok(Err(_)) | Err(mpsc::RecvTimeoutError::Disconnected) => {
            kill_cogitate_child(&child_pid);
            return Err(CogitateRunError::Io);
        }
        Err(mpsc::RecvTimeoutError::Timeout) => {
            kill_cogitate_child(&child_pid);
            return Err(CogitateRunError::Timeout);
        }
    };
    match result_receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(_)) => Err(CogitateRunError::Io),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            let _ = kill(Pid::from_raw(pid as i32), Signal::SIGKILL);
            Err(CogitateRunError::Timeout)
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(CogitateRunError::Io),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CogitateRunError {
    Timeout,
    Io,
}

fn cogitate_run_error_reason(error: CogitateRunError) -> &'static str {
    match error {
        CogitateRunError::Timeout => "brain_refresh_timeout",
        CogitateRunError::Io => "probe_internal_error",
    }
}

fn kill_cogitate_child(child_pid: &AtomicI32) {
    let pid = child_pid.load(Ordering::Acquire);
    if pid > 0 {
        let _ = kill(Pid::from_raw(pid), Signal::SIGKILL);
    }
}

fn terminal_cogitate_reason(stdout: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(stdout).ok()?;
    let terminal = text
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|value| {
            matches!(
                value.get("event").and_then(Value::as_str),
                Some("finish" | "error")
            )
        })?;
    if terminal.get("event").and_then(Value::as_str) == Some("finish")
        && terminal
            .get("result")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.trim().is_empty())
    {
        return Some("ok".to_owned());
    }
    Some(map_provider_reason(
        "cogitate",
        terminal.get("reason_code").and_then(Value::as_str),
    ))
}

fn map_provider_reason(component: &str, reason: Option<&str>) -> String {
    match reason {
        Some(reason) if solstone_core_brain::is_valid_evidence_reason(component, reason) => {
            reason.to_owned()
        }
        _ => "probe_internal_error".to_owned(),
    }
}

fn component_ok(now: DateTime<Utc>) -> Value {
    json!({"status":"ok", "observed_at": now.to_rfc3339(), "expires_at": (now + Duration::hours(26)).to_rfc3339()})
}

fn spp_component_ok(
    now: DateTime<Utc>,
    session: &solstone_core_spp_ratls::AttestationSession,
) -> Value {
    let expires_at = [
        session.tpm_heartbeat_due_at(),
        session.gpu_reattest_due_at(),
        session.session_cap_at(),
    ]
    .into_iter()
    .min()
    .expect("attestation session has deadlines");
    json!({
        "status": "ok",
        "observed_at": now.to_rfc3339(),
        "expires_at": DateTime::<Utc>::from(expires_at).to_rfc3339(),
    })
}

fn component_not_attempted(reason: &str, now: DateTime<Utc>) -> Value {
    json!({"status":"not_attempted", "observed_at":now.to_rfc3339(), "reason_code":reason})
}

fn runtime_diagnostic(
    reason: &str,
    phase: Option<&str>,
    runtime_reason: Option<&str>,
) -> Map<String, Value> {
    let mut diagnostic = Map::new();
    if let Some(phase) = phase {
        diagnostic.insert("phase".to_owned(), Value::String(phase.to_owned()));
    }
    if reason != "local_runtime_fingerprint_mismatch"
        && let Some(runtime_reason) = runtime_reason
    {
        diagnostic.insert(
            "runtime_reason".to_owned(),
            Value::String(runtime_reason.to_owned()),
        );
    }
    diagnostic
}

fn component_for_reason(
    component: &str,
    reason: &str,
    diagnostic: Map<String, Value>,
    now: DateTime<Utc>,
) -> Value {
    solstone_core_brain::evidence_component_for_reason(component, reason, diagnostic, now)
        .expect("owner evidence reason is admitted by the shared contract")
}

fn view(journal: &Path, config: &Map<String, Value>, now: DateTime<Utc>) -> View {
    let inspection = solstone_core_brain::inspect_brain_state(journal, config, now);
    let (failing_component, observed_at, expires_at) = evidence_view(inspection.record.as_ref());
    let checked_age = observed_at
        .as_deref()
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| age(now, value.with_timezone(&Utc)));
    View {
        aggregate_state: inspection.projection.aggregate_state,
        reason_code: inspection.projection.reason_code,
        lane: inspection.projection.active_lane,
        provider: inspection.projection.active_provider,
        model: inspection.projection.active_model,
        fingerprint_sha256: inspection.projection.fingerprint_sha256,
        failing_component,
        observed_at,
        expires_at,
        path: Some(
            solstone_core_brain::brain_state_path(journal)
                .display()
                .to_string(),
        ),
        checked_age,
        transient: None,
    }
}

fn evidence_view(record: Option<&Value>) -> (Option<String>, Option<String>, Option<String>) {
    let Some(evidence) = record
        .and_then(|value| value.get("evidence"))
        .and_then(Value::as_object)
    else {
        return (None, None, None);
    };
    let mut ready = (None, None);
    for name in COMPONENTS {
        let Some(component) = evidence.get(name).and_then(Value::as_object) else {
            continue;
        };
        let observed = component
            .get("observed_at")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        let expires = component
            .get("expires_at")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        if component.get("status").and_then(Value::as_str) != Some("ok") {
            return (Some(name.to_owned()), observed, expires);
        }
        if ready.0.is_none() {
            ready = (observed, expires);
        }
    }
    (None, ready.0, ready.1)
}

fn render_transient(reason: &'static str, json_output: bool) -> ExitCode {
    let view = transient_view(reason, &empty_view());
    render(&view, json_output);
    brain_exit_code(&view)
}

fn empty_view() -> View {
    View {
        aggregate_state: "unknown".to_owned(),
        reason_code: None,
        lane: None,
        provider: None,
        model: None,
        fingerprint_sha256: None,
        failing_component: None,
        observed_at: None,
        expires_at: None,
        path: None,
        checked_age: None,
        transient: None,
    }
}

fn transient_view(reason: &'static str, previous: &View) -> View {
    View {
        aggregate_state: if reason == "busy" {
            "checking"
        } else {
            "unknown"
        }
        .to_owned(),
        reason_code: Some(reason.to_owned()),
        lane: previous.lane.clone(),
        provider: previous.provider.clone(),
        model: previous.model.clone(),
        fingerprint_sha256: previous.fingerprint_sha256.clone(),
        failing_component: None,
        observed_at: None,
        expires_at: None,
        path: previous.path.clone(),
        checked_age: None,
        transient: Some(reason),
    }
}

fn render(view: &View, json_output: bool) {
    if json_output {
        let object = BTreeMap::from([
            ("aggregate_state", json!(view.aggregate_state)),
            ("expires_at", json!(view.expires_at)),
            ("failing_component", json!(view.failing_component)),
            ("fingerprint_sha256", json!(view.fingerprint_sha256)),
            ("lane", json!(view.lane)),
            ("model", json!(view.model)),
            ("observed_at", json!(view.observed_at)),
            ("path", json!(view.path)),
            ("provider", json!(view.provider)),
            ("reason_code", json!(view.reason_code)),
        ]);
        println!(
            "{}",
            serde_json::to_string(&object).expect("owner view serializes")
        );
        return;
    }
    println!("{}", human_text(view));
}

fn human_text(view: &View) -> String {
    if view.transient == Some("busy") {
        return "Brain busy: check already running".to_owned();
    }
    if view.aggregate_state == "ready" {
        let identity = match (&view.provider, &view.model, &view.lane) {
            (Some(provider), Some(model), lane) => format!(
                "{} {provider}/{model}",
                lane.as_deref().unwrap_or("unknown")
            ),
            (_, _, Some(lane)) => lane.clone(),
            _ => "unknown".to_owned(),
        };
        let age = view
            .checked_age
            .as_ref()
            .map(|value| format!(", checked {value} ago"))
            .unwrap_or_default();
        return format!("Brain ready: {identity}{age}");
    }
    let reason = reason_text(view.reason_code.as_deref());
    let component = view
        .failing_component
        .as_ref()
        .map(|value| format!(" ({value})"))
        .unwrap_or_default();
    format!("Brain {}: {reason}{component}", view.aggregate_state)
}

fn reason_text(reason: Option<&str>) -> String {
    match reason {
        None => "ok".to_owned(),
        Some("thinking_engine_not_chosen") => "no thinking engine chosen".to_owned(),
        Some("configuration_invalid") => "configuration invalid".to_owned(),
        Some("stale_expected_fingerprint") => "stale expected fingerprint".to_owned(),
        Some("lost_fence") => "refresh fence lost".to_owned(),
        Some("busy") => "check already running".to_owned(),
        Some(value) => value.replace('_', " "),
    }
}

fn brain_exit_code(view: &View) -> ExitCode {
    if matches!(
        view.transient,
        Some("busy" | "stale_expected_fingerprint" | "lost_fence")
    ) {
        return ExitCode::from(3);
    }
    ExitCode::from(match view.aggregate_state.as_str() {
        "ready" => 0,
        "blocked" | "unhealthy" => 1,
        _ => 2,
    })
}

fn age(now: DateTime<Utc>, observed: DateTime<Utc>) -> String {
    let seconds = (now - observed).num_seconds().max(0);
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m", seconds / 60)
    } else if seconds < 172800 {
        format!("{}h", seconds / 3600)
    } else {
        format!("{}d", seconds / 86400)
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn owner_view(state: &str, reason: Option<&str>, component: Option<&str>) -> View {
        View {
            aggregate_state: state.to_owned(),
            reason_code: reason.map(str::to_owned),
            lane: None,
            provider: None,
            model: None,
            fingerprint_sha256: None,
            failing_component: component.map(str::to_owned),
            observed_at: None,
            expires_at: None,
            path: None,
            checked_age: None,
            transient: None,
        }
    }

    #[test]
    fn owner_exit_code_table_and_transient_precedence_are_exact() {
        let cases = [
            ("ready", None, 0),
            ("blocked", Some("provider_key_missing"), 1),
            ("unhealthy", Some("provider_response_invalid"), 1),
            ("unknown", Some("brain_record_missing"), 2),
            ("checking", Some("brain_check_in_progress"), 2),
        ];
        for (state, reason, expected) in cases {
            assert_eq!(
                brain_exit_code(&owner_view(state, reason, None)),
                ExitCode::from(expected),
                "{state}"
            );
        }
        for transient in ["busy", "stale_expected_fingerprint", "lost_fence"] {
            assert_eq!(
                brain_exit_code(&transient_view(
                    transient,
                    &owner_view("checking", None, None)
                )),
                ExitCode::from(3),
                "{transient}"
            );
        }
    }

    #[test]
    fn owner_human_text_covers_identity_age_and_generic_forms() {
        let mut ready = owner_view("ready", None, None);
        ready.lane = Some("byo-cloud".to_owned());
        ready.provider = Some("openai".to_owned());
        ready.model = Some("gpt".to_owned());
        ready.checked_age = Some("59s".to_owned());
        assert_eq!(
            human_text(&ready),
            "Brain ready: byo-cloud openai/gpt, checked 59s ago"
        );

        ready.provider = None;
        ready.model = None;
        ready.checked_age = None;
        assert_eq!(human_text(&ready), "Brain ready: byo-cloud");
        ready.lane = None;
        assert_eq!(human_text(&ready), "Brain ready: unknown");

        assert_eq!(
            human_text(&owner_view(
                "unhealthy",
                Some("provider_response_invalid"),
                Some("generate"),
            )),
            "Brain unhealthy: provider response invalid (generate)"
        );
        assert_eq!(
            human_text(&owner_view("unknown", None, None)),
            "Brain unknown: ok"
        );
        assert_eq!(
            human_text(&owner_view(
                "checking",
                Some("brain_check_in_progress"),
                None
            )),
            "Brain checking: brain check in progress"
        );
        assert_eq!(
            human_text(&transient_view("busy", &owner_view("checking", None, None))),
            "Brain busy: check already running"
        );
    }

    #[test]
    fn owner_checked_age_boundaries_match_the_oracle() {
        let now = Utc.with_ymd_and_hms(2026, 1, 2, 0, 0, 0).unwrap();
        for (seconds, expected) in [
            (0, "0s"),
            (59, "59s"),
            (60, "1m"),
            (3599, "59m"),
            (3600, "1h"),
            (172799, "47h"),
            (172800, "2d"),
        ] {
            assert_eq!(age(now, now - Duration::seconds(seconds)), expected);
        }
    }

    #[test]
    fn owner_provider_reasons_are_component_scoped_and_fail_closed() {
        assert_eq!(
            map_provider_reason("generate", Some("probe_output_starved")),
            "probe_output_starved"
        );
        assert_eq!(
            map_provider_reason("cogitate", Some("probe_output_starved")),
            "probe_internal_error"
        );
        assert_eq!(
            map_provider_reason("generate", Some("not-in-contract")),
            "probe_internal_error"
        );
    }

    fn generated_response(
        text: &str,
        finish_reason: &str,
        usage: Value,
        thinking: Option<Value>,
    ) -> GeneratedResponse {
        GeneratedResponse {
            id: None,
            text: text.to_owned(),
            model: "test-model".to_owned(),
            usage,
            finish_reason: finish_reason.to_owned(),
            thinking,
            schema_validation: None,
            input_budget: None,
            request_budget: None,
            inference: None,
            hints_applied: Vec::new(),
        }
    }

    #[test]
    fn owner_generate_classification_matches_the_canned_oracle() {
        let cases = [
            (
                "OK",
                "max_tokens",
                json!({}),
                None,
                Some("probe_output_starved"),
            ),
            ("OK", "stop", json!({}), None, None),
            (
                "",
                "stop",
                json!({}),
                None,
                Some("provider_response_invalid"),
            ),
            (
                "",
                "stop",
                json!({"reasoning_tokens": 1}),
                None,
                Some("probe_output_starved"),
            ),
            (
                "",
                "stop",
                json!({}),
                Some(json!([{"summary":"reasoned"}])),
                Some("probe_output_starved"),
            ),
            ("", "unknown", json!({}), None, Some("probe_output_starved")),
        ];
        for (text, finish_reason, usage, thinking, expected) in cases {
            let response = generated_response(text, finish_reason, usage, thinking);
            assert_eq!(classify_canned_generate(&response), expected);
        }
    }

    #[test]
    fn owner_cogitate_outer_timeout_is_distinct_from_process_failure() {
        assert_eq!(
            cogitate_run_error_reason(CogitateRunError::Timeout),
            "brain_refresh_timeout"
        );
        assert_eq!(
            cogitate_run_error_reason(CogitateRunError::Io),
            "probe_internal_error"
        );
    }

    #[test]
    fn owner_cogitate_resolution_failure_keeps_probe_internal_error() {
        assert!(
            solstone_core_generate::sibling_executable().is_err(),
            "cargo test must run without a sibling solstone-core binary"
        );
        let now = Utc.with_ymd_and_hms(2026, 1, 2, 0, 0, 0).unwrap();
        let config = Map::new();

        assert_eq!(
            cogitate_component(Path::new("unused-journal"), &config, now),
            component_for_reason("cogitate", "probe_internal_error", Map::new(), now)
        );
    }

    #[test]
    fn owner_probe_reason_vocabulary_and_cogitate_terminals_are_closed() {
        for reason in [
            "brain_refresh_timeout",
            "endpoint_contract_failed",
            "endpoint_unreachable",
            "local_server_unhealthy",
            "model_not_found",
            "network_unreachable",
            "provider_key_invalid",
            "provider_quota_exceeded",
            "provider_request_rejected",
            "provider_response_invalid",
            "provider_unavailable",
        ] {
            assert_eq!(map_provider_reason("generate", Some(reason)), reason);
            assert_eq!(map_provider_reason("cogitate", Some(reason)), reason);
        }
        assert_eq!(
            map_provider_reason("generate", Some("probe_output_starved")),
            "probe_output_starved"
        );
        assert_eq!(
            terminal_cogitate_reason(br#"{"event":"finish","result":"OK","reason_code":null}"#)
                .as_deref(),
            Some("ok")
        );
        assert_eq!(
            terminal_cogitate_reason(br#"{"event":"error","reason_code":"endpoint_unreachable"}"#)
                .as_deref(),
            Some("endpoint_unreachable")
        );
        assert_eq!(
            terminal_cogitate_reason(br#"{"event":"error","reason_code":"unrecognized"}"#)
                .as_deref(),
            Some("probe_internal_error")
        );
    }

    #[test]
    fn owner_cogitate_multiline_wire_terminals_are_classified() {
        let success = br#"{"event":"text_delta","ts":1,"correlation_id":"health.brain.cogitate","delta":"O","model":"model"}
{"event":"finish","ts":2,"correlation_id":"health.brain.cogitate","terminal":true,"usage":{"input_tokens":1,"output_tokens":1,"cached_tokens":0,"cache_creation_tokens":0,"reasoning_tokens":0,"requests":1},"result":"OK"}"#;
        assert_eq!(terminal_cogitate_reason(success), Some("ok".to_owned()));

        let error = br#"{"event":"text_delta","ts":1,"correlation_id":"health.brain.cogitate","delta":"E","model":"model"}
{"event":"error","ts":2,"correlation_id":"health.brain.cogitate","terminal":true,"usage":{"input_tokens":1,"output_tokens":1,"cached_tokens":0,"cache_creation_tokens":0,"reasoning_tokens":0,"requests":1},"error":"endpoint unreachable","reason_code":"endpoint_unreachable"}"#;
        assert_eq!(
            terminal_cogitate_reason(error),
            Some("endpoint_unreachable".to_owned())
        );
    }

    #[test]
    fn owner_cogitate_unparseable_terminal_stream_is_none() {
        let stdout = br#"{"event":"text_delta","ts":1,"correlation_id":"health.brain.cogitate","delta":"O","model":"model"}
garbage"#;
        assert_eq!(terminal_cogitate_reason(stdout), None);
    }

    #[test]
    fn owner_spp_reason_mapping_preserves_the_oracle_fallbacks() {
        assert_eq!(
            spp_reason("gateway_unreachable"),
            "attestation_not_verified"
        );
        assert_eq!(
            spp_reason("nvattest_platform_unsupported"),
            "nvattest_platform_unsupported"
        );
        assert_eq!(spp_reason("nvattest_unavailable"), "nvattest_unavailable");
        assert_eq!(
            spp_reason("nvattest_integrity_failed"),
            "nvattest_integrity_failed"
        );
        assert_eq!(
            spp_reason("nvattest_install_failed"),
            "nvattest_install_failed"
        );
        assert_eq!(spp_reason("certificate_invalid"), "attestation_rejected");
        assert_eq!(
            spp_reason("unrecognized-provider-reason"),
            "attestation_rejected"
        );
        assert_eq!(
            valid_spp_reason("unrecognized-provider-reason"),
            "attestation_rejected"
        );
    }

    #[test]
    fn bundled_runtime_diagnostics_only_carry_contract_fields() {
        let diagnostic = runtime_diagnostic(
            "local_runtime_not_ready",
            Some("starting"),
            Some("runtime_starting"),
        );
        assert_eq!(diagnostic["phase"], "starting");
        assert_eq!(diagnostic["runtime_reason"], "runtime_starting");

        let mismatch = runtime_diagnostic(
            "local_runtime_fingerprint_mismatch",
            Some("ready"),
            Some("runtime_ready"),
        );
        assert_eq!(mismatch["phase"], "ready");
        assert!(!mismatch.contains_key("runtime_reason"));
    }
}
