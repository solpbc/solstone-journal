// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

#[cfg(unix)]
use nix::errno::Errno;
#[cfg(unix)]
use nix::sys::signal::{Signal, killpg};
#[cfg(unix)]
use nix::unistd::Pid;
use serde_json::{Map, Value};
use solstone_core_generate_wire::{record_usage, usage_for_log};
use solstone_core_system::lifecycle::HostedServiceParentRuntime;
use solstone_core_system::process::{
    self, CommandLaunchRequest, Disposition, LaunchAuthority, LaunchError,
};

use crate::state::{CortexState, ResolvedTalent, Work};
use crate::storage::now_ms;

pub(crate) fn spawn_worker(
    state: CortexState,
    executable_dir: PathBuf,
    talent_root: PathBuf,
    apps_root: PathBuf,
    templates_dir: PathBuf,
    receiver: mpsc::Receiver<Work>,
    hosted_parent: Option<Arc<HostedServiceParentRuntime>>,
) {
    while let Ok(work) = receiver.recv() {
        state.spawn_begin(&work.use_id);
        if !state.accepting() {
            state.abort(work, "Cortex stopped before spawn".into());
        } else {
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                spawn_one(
                    state.clone(),
                    executable_dir.clone(),
                    &talent_root,
                    &apps_root,
                    &templates_dir,
                    work.clone(),
                    hosted_parent.clone(),
                )
            })) {
                Ok(Ok(())) => {}
                Ok(Err(error)) => state.abort(work, format!("Failed to spawn talent: {error}")),
                Err(payload) => state.abort(
                    work,
                    format!("Spawn worker error: {}", panic_message(payload)),
                ),
            }
        }
        state.spawn_finished();
    }
}

pub fn spawn_one(
    state: CortexState,
    executable_dir: PathBuf,
    talent_root: &Path,
    apps_root: &Path,
    templates_dir: &Path,
    work: Work,
    hosted_parent: Option<Arc<HostedServiceParentRuntime>>,
) -> Result<(), String> {
    let name = work
        .request
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .ok_or("talent request missing name")?;
    let resolved = solstone_core_talent_cli::resolve_execution_facts(
        name,
        talent_root,
        apps_root,
        state.journal(),
        templates_dir,
        None,
    )
    .map_err(|error| format!("failed to resolve talent {name}: {error}"))?
    .map(|facts| ResolvedTalent {
        talent_type: facts.talent_type,
        declared_cwd: facts.declared_cwd,
        timeout_seconds: facts.timeout_seconds,
    });
    if let Some(resolved) = resolved.as_ref() {
        state.update_resolved_talent(&work.use_id, resolved.clone());
    }
    // Cortex owns this request/response lifecycle and spawns the native worker
    // directly; the journal boundary also dispatches the service verb natively.
    let timeout = timeout_for(
        &work.request,
        resolved.as_ref().and_then(|facts| facts.timeout_seconds),
    );
    let command = build_talent_worker_command(
        &executable_dir,
        state.journal(),
        resolved.as_ref(),
        &work.request,
    )?;
    let request_line = serde_json::to_vec(&Value::Object(work.request.clone()))
        .map_err(|error| error.to_string())?;
    let disposition = Disposition::IndependentBoundedHelper {
        timeout: Duration::from_secs(timeout),
    };
    #[cfg(unix)]
    let terminate = Box::new(|child: &mut std::process::Child, _timeout| {
        let pgid = i32::try_from(child.id()).map_err(|_| {
            LaunchError::Terminate(std::io::Error::other("child pid does not fit i32"))
        })?;
        stop_group(pgid);
        Ok(())
    });
    #[cfg(not(unix))]
    let terminate = Box::new(|child: &mut std::process::Child, _timeout| {
        child.kill().map_err(LaunchError::Terminate)
    });
    let authority = match hosted_parent {
        Some(parent) => process::launch_command_hosted(
            disposition,
            command,
            parent.child_launch_provenance(format!("cortex-talent-{}", work.use_id)),
            terminate,
        ),
        None => process::launch_command(disposition, command, terminate),
    }
    .map_err(|error| error.to_string())?;
    let authority: Arc<Mutex<LaunchAuthority>> = Arc::new(Mutex::new(authority));
    let Some(mut stdin) = authority
        .lock()
        .expect("cortex authority lock poisoned")
        .take_stdin()
    else {
        let _ = authority
            .lock()
            .expect("cortex authority lock poisoned")
            .terminate(Duration::from_secs(10));
        return Err("child stdin unavailable".into());
    };
    if let Err(error) = stdin
        .write_all(&request_line)
        .and_then(|_| stdin.write_all(b"\n"))
    {
        let _ = authority
            .lock()
            .expect("cortex authority lock poisoned")
            .terminate(Duration::from_secs(10));
        return Err(error.to_string());
    }
    drop(stdin);
    let stderr = Arc::new(Mutex::new(Vec::new()));
    state.spawn_started(&work, Arc::clone(&authority), Arc::clone(&stderr));
    let Some(stdout) = authority
        .lock()
        .expect("cortex authority lock poisoned")
        .take_stdout()
    else {
        return Err("child stdout unavailable".into());
    };
    let Some(stderr_pipe) = authority
        .lock()
        .expect("cortex authority lock poisoned")
        .take_stderr()
    else {
        return Err("child stderr unavailable".into());
    };
    let (stdout_done, done_rx) = mpsc::channel();
    let stdout_state = state.clone();
    let stdout_work = work.clone();
    thread::spawn(move || {
        let reader = std::io::BufReader::new(stdout);
        for line in std::io::BufRead::lines(reader) {
            let Ok(line) = line else { break };
            handle_stdout(&stdout_state, &stdout_work, line);
        }
        let _ = stdout_done.send(());
    });
    let stderr_lines = Arc::clone(&stderr);
    let stderr_id = work.use_id.clone();
    thread::spawn(move || {
        let reader = std::io::BufReader::new(stderr_pipe);
        for line in std::io::BufRead::lines(reader) {
            let Ok(line) = line else { break };
            let line = line.trim().to_owned();
            if !line.is_empty() {
                stderr_lines
                    .lock()
                    .expect("stderr lock poisoned")
                    .push(line.clone());
                eprintln!("[talent:{stderr_id}:stderr] {line}");
            }
        }
    });
    let reaper_state = state.clone();
    let reaper_use_id = work.use_id.clone();
    let authority_for_reaper = Arc::clone(&authority);
    thread::spawn(move || {
        // Poll rather than LaunchAuthority::wait(): a blocking wait would hold
        // the shared mutex for the whole talent run, and the timeout / cancel /
        // immediate-stop threads need that same lock to terminate().
        let raw_code = loop {
            let polled = authority_for_reaper
                .lock()
                .expect("cortex authority lock poisoned")
                .poll();
            match polled {
                Ok(Some(code)) => break code,
                Ok(None) => thread::sleep(Duration::from_millis(20)),
                Err(_) => break -1,
            }
        };
        let code = if raw_code >= 0 { raw_code } else { -1 };
        let _ = done_rx.recv_timeout(Duration::from_millis(100));
        reaper_state.finish(&reaper_use_id, code);
    });
    let timeout_state = state;
    let timeout_id = work.use_id;
    thread::spawn(move || {
        thread::sleep(Duration::from_secs(timeout));
        if let Some(running) = timeout_state.timeout(&timeout_id, timeout) {
            let _ = running
                .authority
                .lock()
                .expect("cortex authority lock poisoned")
                .terminate(Duration::from_secs(10));
        }
    });
    Ok(())
}

pub(crate) fn build_talent_worker_command(
    executable_dir: &Path,
    journal: &Path,
    resolved: Option<&ResolvedTalent>,
    request: &Map<String, Value>,
) -> Result<CommandLaunchRequest, String> {
    let worker = solstone_core_journal_cli::sibling_native_in_dir(executable_dir, "solstone-core")
        .map_err(|error| error.to_string())?;
    let current_dir = resolved
        .is_some_and(|facts| {
            facts.talent_type.as_deref() == Some("cogitate")
                && facts.declared_cwd.as_deref() == Some("journal")
        })
        .then(|| journal.to_path_buf());
    let mut environment = BTreeMap::new();
    if let Some(facet) = request
        .get("facet")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        environment.insert(OsString::from("SOL_FACET"), OsString::from(facet));
    }
    if let Some(day) = request
        .get("day")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        environment.insert(OsString::from("SOL_DAY"), OsString::from(day));
    }
    if let Some(env) = request.get("env").and_then(Value::as_object) {
        for (key, value) in env {
            environment.insert(
                OsString::from(key),
                value
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| value.to_string())
                    .into(),
            );
        }
    }
    Ok(CommandLaunchRequest {
        program: worker.into_os_string(),
        arguments: vec![OsString::from("__talent-worker")],
        environment,
        current_dir,
        process_group: true,
        stdin_piped: true,
        stdout_piped: true,
        stderr_piped: true,
    })
}

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<String>() {
        return message.clone();
    }
    if let Some(message) = payload.downcast_ref::<&str>() {
        return (*message).to_owned();
    }
    "panic".to_owned()
}

fn timeout_for(request: &Map<String, Value>, resolved_timeout_seconds: Option<u64>) -> u64 {
    request
        .get("timeout_seconds")
        .and_then(Value::as_u64)
        .or(resolved_timeout_seconds)
        .unwrap_or(600)
}

fn handle_stdout(state: &CortexState, work: &Work, line: String) {
    let line = line.trim();
    if line.is_empty() {
        return;
    }
    let Ok(Value::Object(mut event)) = serde_json::from_str::<Value>(line) else {
        let mut info = Map::new();
        info.insert("event".into(), Value::String("info".into()));
        info.insert("ts".into(), Value::from(now_ms()));
        info.insert("message".into(), Value::String(line.into()));
        info.insert("use_id".into(), Value::String(work.use_id.clone()));
        let _ = state.append_and_relay_log_only(&work.active, info);
        return;
    };
    let request = state
        .request_for(&work.use_id)
        .unwrap_or_else(|| work.request.clone());
    for (key, value) in [
        ("ts", Value::from(now_ms())),
        ("use_id", Value::String(work.use_id.clone())),
        (
            "name",
            request
                .get("name")
                .cloned()
                .unwrap_or(Value::String(String::new())),
        ),
        (
            "day",
            request
                .get("day")
                .cloned()
                .unwrap_or(Value::String(String::new())),
        ),
    ] {
        event.entry(key).or_insert(value);
    }
    state.append_and_relay(&work.use_id, &work.active, event.clone());
    if event.get("event").and_then(Value::as_str) == Some("start") {
        state.update_start(&work.use_id, &event);
    }
    let terminal = event.get("event").and_then(Value::as_str) == Some("finish")
        || (event.get("event").and_then(Value::as_str) == Some("error")
            && event
                .get("terminal")
                .and_then(Value::as_bool)
                .unwrap_or(true));
    if terminal {
        record_terminal_usage(state, &work.use_id, &event);
    }
}

fn record_terminal_usage(state: &CortexState, use_id: &str, event: &Map<String, Value>) {
    if state
        .resolved_talent(use_id)
        .and_then(|facts| facts.talent_type)
        .as_deref()
        != Some("cogitate")
    {
        return;
    }
    let Some(usage) = event.get("usage") else {
        return;
    };
    let Some(request) = state.request_for(use_id) else {
        return;
    };
    let model = usage
        .get("model_version")
        .and_then(Value::as_str)
        .or_else(|| request.get("model").and_then(Value::as_str))
        .unwrap_or("unknown");
    let name = request
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let context = context_for(name);
    let segment = request
        .get("env")
        .and_then(Value::as_object)
        .and_then(|env| env.get("SOL_SEGMENT"))
        .and_then(Value::as_str);
    if let Err(error) = record_usage(
        state.journal(),
        model,
        &context,
        &usage_for_log(usage),
        "cogitate",
        segment,
        None,
    ) {
        eprintln!("cortex: failed to log token usage for talent {use_id}: {error}");
    }
}

fn context_for(name: &str) -> String {
    if let Some((app, talent)) = name.split_once(':') {
        format!("talent.{app}.{talent}")
    } else {
        format!("talent.system.{name}")
    }
}

#[cfg(unix)]
pub(crate) fn stop_group(pgid: i32) {
    stop_group_with_grace(pgid, Duration::from_secs(10));
}

#[cfg(unix)]
pub fn stop_group_with_grace(pgid: i32, grace: Duration) {
    let _ = killpg(Pid::from_raw(pgid), Signal::SIGTERM);
    let deadline = std::time::Instant::now() + grace;
    while std::time::Instant::now() < deadline {
        if !group_has_live_processes(pgid) {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
    let _ = killpg(Pid::from_raw(pgid), Signal::SIGKILL);
}

#[cfg(unix)]
fn group_has_live_processes(pgid: i32) -> bool {
    !matches!(killpg(Pid::from_raw(pgid), None), Err(Errno::ESRCH))
}

pub(crate) fn cancel_worker(state: CortexState, receiver: mpsc::Receiver<(String, String)>) {
    while let Ok((use_id, reason)) = receiver.recv() {
        if let Some(running) = state.cancel_running(&use_id, &reason) {
            let _ = running
                .authority
                .lock()
                .expect("cortex authority lock poisoned")
                .terminate(Duration::from_secs(10));
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::mpsc;

    use super::*;

    use crate::renewal::{BrainAdapter, RenewalBrain, RenewalHandle, write_valid_test_journal};
    use crate::storage::CortexStore;
    use tempfile::tempdir;

    fn write_sibling_stub(executable_dir: &Path) {
        let native = executable_dir.join("solstone-core");
        fs::write(&native, "#!/bin/sh\n").expect("native stub");
        let mut permissions = fs::metadata(&native).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&native, permissions).unwrap();
    }

    fn captured_command(
        executable_dir: &Path,
        journal: &Path,
        resolved: Option<&ResolvedTalent>,
        request: &Map<String, Value>,
    ) -> CommandLaunchRequest {
        let mut captured = Vec::new();
        captured
            .push(build_talent_worker_command(executable_dir, journal, resolved, request).unwrap());
        assert_eq!(captured.len(), 1);
        captured.remove(0)
    }

    #[test]
    fn context_for_matches_python_talent_key_shape() {
        assert_eq!(context_for("conversation"), "talent.system.conversation");
        assert_eq!(context_for("entities:observer"), "talent.entities.observer");
    }

    #[test]
    fn stdout_augmentation_fills_only_absent_values() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let request: Map<String, Value> = serde_json::from_value(
            serde_json::json!({"use_id":"one","name":"conversation","day":"20260101","ts":10}),
        )
        .unwrap();
        let (active, identity) = store
            .claim("conversation", "one", &request)
            .unwrap()
            .unwrap();
        let (spawn_tx, _) = mpsc::channel();
        let (cancel_tx, _) = mpsc::channel();
        let (outbound_tx, outbound_rx) = mpsc::channel();
        let state = CortexState::new(store, spawn_tx, cancel_tx, outbound_tx);
        let work = Work {
            use_id: "one".into(),
            talent_name: "conversation".into(),
            active: active.clone(),
            identity,
            request,
        };
        handle_stdout(&state, &work, "{\"event\":\"thinking\"}".into());
        let absent = outbound_rx.recv().unwrap();
        assert_eq!(absent.fields["use_id"], "one");
        assert_eq!(absent.fields["name"], "conversation");
        assert_eq!(absent.fields["day"], "20260101");
        handle_stdout(&state, &work, "{\"event\":\"thinking\",\"ts\":999,\"use_id\":\"other\",\"name\":\"other-name\",\"day\":\"other-day\"}".into());
        let present = outbound_rx.recv().unwrap();
        assert_eq!(present.fields["ts"], 999);
        assert_eq!(present.fields["use_id"], "other");
        assert_eq!(present.fields["name"], "other-name");
        assert_eq!(present.fields["day"], "other-day");
    }

    #[test]
    fn deliberate_spawn_emits_exactly_one_native_sibling_command() {
        let directory = tempdir().unwrap();
        let executable_dir = directory.path().join("bin");
        fs::create_dir(&executable_dir).unwrap();
        for name in ["python", "python3", "pytest", "uv", "ruff"] {
            let poison = executable_dir.join(name);
            fs::write(&poison, "#!/bin/sh\n").unwrap();
            let mut permissions = fs::metadata(&poison).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&poison, permissions).unwrap();
        }
        write_sibling_stub(&executable_dir);
        let request: Map<String, Value> = serde_json::from_value(serde_json::json!({
            "use_id":"one",
            "name":"conversation",
            "day":"20260101",
            "facet":"top",
            "env":{
                "SOL_FACET":"override",
                "PATH":format!("{}:/usr/bin", executable_dir.display())
            }
        }))
        .unwrap();
        let command = captured_command(&executable_dir, directory.path(), None, &request);
        let program = PathBuf::from(&command.program);
        assert_eq!(program, executable_dir.join("solstone-core"));
        let args: Vec<String> = command
            .arguments
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args, ["__talent-worker"]);
        let facet = command
            .environment
            .get(&OsString::from("SOL_FACET"))
            .map(|value| value.to_string_lossy().into_owned());
        assert_eq!(facet.as_deref(), Some("override"));
        assert!(command.current_dir.is_none());
    }

    #[test]
    fn renewal_cycle_never_reaches_injected_interpreters_but_deliberate_spawn_does() {
        let directory = tempdir().unwrap();
        let executable_dir = directory.path().join("bin");
        fs::create_dir(&executable_dir).unwrap();
        for name in ["python", "python3", "pytest", "uv", "ruff"] {
            let shim = executable_dir.join(name);
            fs::write(&shim, "#!/bin/sh\n").unwrap();
            let mut permissions = fs::metadata(&shim).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&shim, permissions).unwrap();
        }
        write_sibling_stub(&executable_dir);
        let marker = directory.path().join("marker");

        write_valid_test_journal(directory.path());
        let cycle_now = chrono::Utc.with_ymd_and_hms(2026, 8, 6, 12, 0, 0).unwrap();
        let brain_path = directory.path().join("health/brain.json");
        let mut brain: Value = serde_json::from_slice(&fs::read(&brain_path).unwrap()).unwrap();
        let adapter = BrainAdapter::new(directory.path().to_path_buf());
        brain["fingerprint_sha256"] = Value::String(adapter.active_fingerprint().unwrap().unwrap());
        brain["updated_at"] = Value::String(cycle_now.to_rfc3339());
        for component in [
            "cogitate",
            "configuration",
            "generate",
            "lane_prerequisites",
        ] {
            brain["evidence"][component]["observed_at"] =
                Value::String((cycle_now - chrono::Duration::minutes(9)).to_rfc3339());
            brain["evidence"][component]["expires_at"] =
                Value::String((cycle_now + chrono::Duration::seconds(30)).to_rfc3339());
        }
        fs::write(&brain_path, serde_json::to_vec(&brain).unwrap()).unwrap();

        let (outbound, receiver) = mpsc::channel();
        let renewal = RenewalHandle::production(
            directory.path().to_path_buf(),
            outbound,
            Arc::new(move || cycle_now),
        );
        let _ = renewal.step(cycle_now);
        let request = receiver.recv().unwrap();
        let reference = request.fields["ref"].as_str().unwrap().to_owned();
        renewal.handle_supervisor(
            "started",
            &Map::from_iter([("ref".into(), Value::String(reference.clone()))]),
        );
        for component in [
            "cogitate",
            "configuration",
            "generate",
            "lane_prerequisites",
        ] {
            brain["evidence"][component]["observed_at"] = Value::String(cycle_now.to_rfc3339());
            brain["evidence"][component]["expires_at"] =
                Value::String((cycle_now + chrono::Duration::minutes(10)).to_rfc3339());
        }
        fs::write(&brain_path, serde_json::to_vec(&brain).unwrap()).unwrap();
        renewal.handle_supervisor(
            "stopped",
            &Map::from_iter([
                ("ref".into(), Value::String(reference)),
                ("exit_code".into(), Value::from(0)),
            ]),
        );
        assert_eq!(renewal.snapshot().retry_index, 0);
        assert!(!marker.exists());

        let request: Map<String, Value> = serde_json::from_value(serde_json::json!({
            "use_id":"one","name":"conversation","day":"20260101","env":{"CORTEX_MARKER":marker}
        }))
        .unwrap();
        let command = captured_command(&executable_dir, directory.path(), None, &request);
        let program = PathBuf::from(&command.program);
        assert_eq!(program, executable_dir.join("solstone-core"));
        let args: Vec<String> = command
            .arguments
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args, ["__talent-worker"]);
        assert!(!marker.exists());
    }

    #[test]
    fn non_json_stdout_is_log_only_info_and_error_defaults_terminal() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let request: Map<String, Value> = serde_json::from_value(
            serde_json::json!({"use_id":"one","name":"conversation","day":"20260101"}),
        )
        .unwrap();
        let (active, identity) = store
            .claim("conversation", "one", &request)
            .unwrap()
            .unwrap();
        let (spawn_tx, _) = mpsc::channel();
        let (cancel_tx, _) = mpsc::channel();
        let (outbound_tx, outbound_rx) = mpsc::channel();
        let state = CortexState::new(store, spawn_tx, cancel_tx, outbound_tx);
        let store2 = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let work = Work {
            use_id: "one".into(),
            talent_name: "conversation".into(),
            active: active.clone(),
            identity,
            request,
        };
        handle_stdout(&state, &work, "plain text".into());
        assert!(outbound_rx.try_recv().is_err());
        assert!(
            fs::read_to_string(&active)
                .unwrap()
                .contains("\"event\":\"info\"")
        );
        handle_stdout(&state, &work, "{\"event\":\"error\"}".into());
        assert!(store2.has_finish(&active));
        let (active_false, identity_false) = store2
            .claim("conversation", "two", &work.request)
            .unwrap()
            .unwrap();
        let work_false = Work {
            use_id: "two".into(),
            talent_name: "conversation".into(),
            active: active_false.clone(),
            identity: identity_false,
            request: work.request.clone(),
        };
        handle_stdout(
            &state,
            &work_false,
            "{\"event\":\"error\",\"terminal\":false}".into(),
        );
        assert!(!store2.has_finish(&active_false));
    }

    #[test]
    fn timeout_prefers_request_then_resolved_talent_then_default() {
        assert_eq!(timeout_for(&Map::new(), Some(9)), 9);
        assert_eq!(
            timeout_for(
                &serde_json::from_value(serde_json::json!({"timeout_seconds":7})).unwrap(),
                Some(9),
            ),
            7
        );
        assert_eq!(timeout_for(&Map::new(), None), 600);
        assert_eq!(
            panic_message(Box::new(String::from("worker failed"))),
            "worker failed"
        );
        for message in [
            "Failed to spawn talent: unavailable",
            "Spawn worker error: worker failed",
            "Cortex stopped before spawn",
            "Recovered: Cortex restarted while talent was running",
            "Talent timed out after 7 seconds",
            "Talent cancelled by watchdog",
            "Talent exited with code 9 without finish event",
        ] {
            let event = crate::storage::synthesized_error("one", message);
            assert_eq!(event["error"], message);
            assert!(!event.contains_key("trace"));
            assert!(!event.contains_key("name"));
            assert!(!event.contains_key("day"));
        }
    }

    #[test]
    fn cogitate_terminal_usage_writes_record() {
        for (use_id, name, model_version, expected_model, expected_context) in [
            (
                "one",
                "apps:timeline",
                Some("usage-model"),
                "usage-model",
                "talent.apps.timeline",
            ),
            (
                "two",
                "conversation",
                None,
                "request-model",
                "talent.system.conversation",
            ),
        ] {
            let directory = tempdir().unwrap();
            let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
            let (spawn_tx, _spawn_rx) = mpsc::channel();
            let (cancel_tx, _) = mpsc::channel();
            let (outbound_tx, _) = mpsc::channel();
            let state = CortexState::new(store, spawn_tx, cancel_tx, outbound_tx);
            state.request(
                serde_json::from_value(serde_json::json!({
                    "use_id": use_id,
                    "name": name,
                    "model": "request-model",
                    "env": {"SOL_SEGMENT": "segment-a"}
                }))
                .unwrap(),
            );
            state.update_resolved_talent(
                use_id,
                ResolvedTalent {
                    talent_type: Some("cogitate".into()),
                    declared_cwd: None,
                    timeout_seconds: None,
                },
            );
            let mut usage = serde_json::json!({"input_tokens": 3});
            if let Some(model_version) = model_version {
                usage["model_version"] = Value::String(model_version.into());
            }
            let terminal = serde_json::from_value(serde_json::json!({"usage": usage})).unwrap();
            record_terminal_usage(&state, use_id, &terminal);
            let token_file = fs::read_dir(directory.path().join("tokens"))
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path();
            let record: Value =
                serde_json::from_str(fs::read_to_string(token_file).unwrap().trim()).unwrap();
            assert_eq!(record["model"], expected_model);
            assert_eq!(record["context"], expected_context);
            assert_eq!(record["segment"], "segment-a");
            assert_eq!(record["type"], "cogitate");
        }
    }

    #[test]
    fn generate_terminal_usage_does_not_write_record() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let (spawn_tx, _spawn_rx) = mpsc::channel();
        let (cancel_tx, _) = mpsc::channel();
        let (outbound_tx, _) = mpsc::channel();
        let state = CortexState::new(store, spawn_tx, cancel_tx, outbound_tx);
        state.request(
            serde_json::from_value(
                serde_json::json!({"use_id":"one","name":"conversation","model":"model"}),
            )
            .unwrap(),
        );
        state.update_resolved_talent(
            "one",
            ResolvedTalent {
                talent_type: Some("generate".into()),
                declared_cwd: None,
                timeout_seconds: None,
            },
        );
        record_terminal_usage(
            &state,
            "one",
            &serde_json::from_value(serde_json::json!({"usage":{"input_tokens":3}})).unwrap(),
        );
        assert!(!directory.path().join("tokens").exists());
    }
}
