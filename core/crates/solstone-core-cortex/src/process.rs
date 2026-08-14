// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io::Write;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use serde_json::{Map, Value};
use solstone_core_generate_wire::{record_usage, usage_for_log};

use crate::state::{CortexState, Work};
use crate::storage::now_ms;

pub(crate) fn spawn_worker(
    state: CortexState,
    executable_dir: PathBuf,
    receiver: mpsc::Receiver<Work>,
) {
    while let Ok(work) = receiver.recv() {
        state.spawn_begin(&work.use_id);
        if !state.accepting() {
            state.abort(work, "Cortex stopped before spawn".into());
        } else {
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                spawn_one(state.clone(), executable_dir.clone(), work.clone())
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

fn spawn_one(state: CortexState, executable_dir: PathBuf, work: Work) -> Result<(), String> {
    // The child is Python. This is cortex's only interpreter-resolution site, so
    // the cortex verb is deliberately not registered in the native process table.
    let python = solstone_core_journal_cli::sibling_python_in_dir(&executable_dir)
        .map_err(|error| error.to_string())?;
    let mut command = Command::new(python);
    command
        .arg("-m")
        .arg("solstone.think.talents")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(facet) = work
        .request
        .get("facet")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        command.env("SOL_FACET", facet);
    }
    if let Some(day) = work
        .request
        .get("day")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        command.env("SOL_DAY", day);
    }
    if let Some(env) = work.request.get("env").and_then(Value::as_object) {
        for (key, value) in env {
            command.env(
                key,
                value
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| value.to_string()),
            );
        }
    }
    command.process_group(0);
    let request_line = serde_json::to_vec(&Value::Object(work.request.clone()))
        .map_err(|error| error.to_string())?;
    let mut child = command.spawn().map_err(|error| error.to_string())?;
    let pgid = i32::try_from(child.id()).map_err(|_| "child pid does not fit i32")?;
    let Some(mut stdin) = child.stdin.take() else {
        terminate_and_reap(&mut child, pgid);
        return Err("child stdin unavailable".into());
    };
    if let Err(error) = stdin
        .write_all(&request_line)
        .and_then(|_| stdin.write_all(b"\n"))
    {
        terminate_and_reap(&mut child, pgid);
        return Err(error.to_string());
    }
    drop(stdin);
    let stderr = Arc::new(Mutex::new(Vec::new()));
    state.spawn_started(&work, pgid, Arc::clone(&stderr));
    let Some(stdout) = child.stdout.take() else {
        return Err("child stdout unavailable".into());
    };
    let Some(stderr_pipe) = child.stderr.take() else {
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
    thread::spawn(move || {
        let code = child
            .wait()
            .ok()
            .and_then(|status| status.code())
            .unwrap_or(-1);
        let _ = done_rx.recv_timeout(Duration::from_millis(100));
        reaper_state.finish(&reaper_use_id, code);
    });
    let timeout = timeout_for(&work.request);
    let timeout_state = state;
    let timeout_id = work.use_id;
    thread::spawn(move || {
        thread::sleep(Duration::from_secs(timeout));
        if let Some(running) = timeout_state.timeout(&timeout_id, timeout) {
            stop_group(running.pgid);
        }
    });
    Ok(())
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

fn timeout_for(request: &Map<String, Value>) -> u64 {
    request
        .get("timeout_seconds")
        .and_then(Value::as_u64)
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

pub(crate) fn stop_group(pgid: i32) {
    let _ = killpg(Pid::from_raw(pgid), Signal::SIGTERM);
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        if !group_has_live_processes(pgid) {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
    let _ = killpg(Pid::from_raw(pgid), Signal::SIGKILL);
}

#[cfg(target_os = "linux")]
fn group_has_live_processes(pgid: i32) -> bool {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return killpg(Pid::from_raw(pgid), None).is_ok();
    };
    entries.flatten().any(|entry| {
        let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else {
            return false;
        };
        let Some((_, fields)) = stat.rsplit_once(") ") else {
            return false;
        };
        let mut fields = fields.split_whitespace();
        let state = fields.next();
        let _parent = fields.next();
        let group = fields.next().and_then(|value| value.parse::<i32>().ok());
        group == Some(pgid) && state != Some("Z")
    })
}

#[cfg(not(target_os = "linux"))]
fn group_has_live_processes(pgid: i32) -> bool {
    killpg(Pid::from_raw(pgid), None).is_ok()
}

fn terminate_and_reap(child: &mut Child, pgid: i32) {
    stop_group(pgid);
    let _ = child.wait();
}

pub(crate) fn cancel_worker(state: CortexState, receiver: mpsc::Receiver<(String, String)>) {
    while let Ok((use_id, reason)) = receiver.recv() {
        if let Some(running) = state.cancel_running(&use_id, &reason) {
            stop_group(running.pgid);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::process::CommandExt;
    use std::process::Command;
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;

    use crate::renewal::{BrainAdapter, RenewalBrain, RenewalHandle, write_valid_test_journal};
    use crate::storage::CortexStore;
    use tempfile::tempdir;

    #[test]
    fn context_for_matches_python_talent_key_shape() {
        assert_eq!(context_for("chat"), "talent.system.chat");
        assert_eq!(context_for("entities:observer"), "talent.entities.observer");
    }

    #[test]
    fn stdout_augmentation_fills_only_absent_values() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let request: Map<String, Value> = serde_json::from_value(
            serde_json::json!({"use_id":"one","name":"chat","day":"20260101","ts":10}),
        )
        .unwrap();
        let active = store.claim("chat", "one", &request).unwrap().unwrap();
        let (spawn_tx, _) = mpsc::channel();
        let (cancel_tx, _) = mpsc::channel();
        let (outbound_tx, outbound_rx) = mpsc::channel();
        let state = CortexState::new(store, spawn_tx, cancel_tx, outbound_tx);
        let work = Work {
            use_id: "one".into(),
            active: active.clone(),
            request,
        };
        handle_stdout(&state, &work, "{\"event\":\"thinking\"}".into());
        let absent = outbound_rx.recv().unwrap();
        assert_eq!(absent.fields["use_id"], "one");
        assert_eq!(absent.fields["name"], "chat");
        assert_eq!(absent.fields["day"], "20260101");
        handle_stdout(&state, &work, "{\"event\":\"thinking\",\"ts\":999,\"use_id\":\"other\",\"name\":\"other-name\",\"day\":\"other-day\"}".into());
        let present = outbound_rx.recv().unwrap();
        assert_eq!(present.fields["ts"], 999);
        assert_eq!(present.fields["use_id"], "other");
        assert_eq!(present.fields["name"], "other-name");
        assert_eq!(present.fields["day"], "other-day");
    }

    #[test]
    fn injected_interpreter_is_reached_only_by_deliberate_spawn() {
        let directory = tempdir().unwrap();
        let executable_dir = directory.path().join("bin");
        fs::create_dir(&executable_dir).unwrap();
        let marker = directory.path().join("marker");
        let argv = directory.path().join("argv");
        let environment = directory.path().join("environment");
        let python = executable_dir.join("python3");
        fs::write(&python, "#!/bin/sh\nprintf x >> \"$CORTEX_MARKER\"\nprintf '%s\\n' \"$@\" > \"$CORTEX_ARGV\"\nenv > \"$CORTEX_ENV\"\nprintf '%s\\n' '{\"event\":\"finish\"}'\n").unwrap();
        let mut permissions = fs::metadata(&python).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&python, permissions).unwrap();
        assert!(!marker.exists());
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let request: Map<String, Value> = serde_json::from_value(
            serde_json::json!({"use_id":"one","name":"chat","day":"20260101","facet":"top","env":{"CORTEX_MARKER":marker,"CORTEX_ARGV":argv,"CORTEX_ENV":environment,"SOL_FACET":"override"}}),
        )
        .unwrap();
        let active = store.claim("chat", "one", &request).unwrap().unwrap();
        let (spawn_tx, _) = mpsc::channel();
        let (cancel_tx, _) = mpsc::channel();
        let (outbound_tx, _) = mpsc::channel();
        let state = CortexState::new(store, spawn_tx, cancel_tx, outbound_tx);
        spawn_one(
            state,
            executable_dir,
            Work {
                use_id: "one".into(),
                active,
                request,
            },
        )
        .unwrap();
        thread::sleep(Duration::from_millis(150));
        assert_eq!(fs::read_to_string(marker).unwrap(), "x");
        assert_eq!(
            fs::read_to_string(argv).unwrap(),
            "-m\nsolstone.think.talents\n"
        );
        assert!(
            fs::read_to_string(environment)
                .unwrap()
                .contains("SOL_FACET=override")
        );
    }

    #[test]
    fn renewal_cycle_never_reaches_injected_interpreters_but_deliberate_spawn_does() {
        let directory = tempdir().unwrap();
        let executable_dir = directory.path().join("bin");
        fs::create_dir(&executable_dir).unwrap();
        let marker = directory.path().join("marker");
        for name in ["python", "python3", "pytest", "uv", "ruff"] {
            let shim = executable_dir.join(name);
            fs::write(&shim, "#!/bin/sh\nprintf x >> \"$CORTEX_MARKER\"\nprintf '%s\\n' '{\"event\":\"finish\"}'\n").unwrap();
            let mut permissions = fs::metadata(&shim).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&shim, permissions).unwrap();
        }

        write_valid_test_journal(directory.path());
        let cycle_now = chrono::Utc::now();
        let brain_path = directory.path().join("health/brain.json");
        let mut brain: Value = serde_json::from_slice(&fs::read(&brain_path).unwrap()).unwrap();
        let adapter = BrainAdapter::new(directory.path().to_path_buf());
        brain["fingerprint_sha256"] = Value::String(adapter.active_fingerprint().unwrap().unwrap());
        brain["evidence"]["lane_prerequisites"]["observed_at"] =
            Value::String((cycle_now - chrono::Duration::minutes(9)).to_rfc3339());
        brain["evidence"]["lane_prerequisites"]["expires_at"] =
            Value::String((cycle_now + chrono::Duration::seconds(30)).to_rfc3339());
        fs::write(&brain_path, serde_json::to_vec(&brain).unwrap()).unwrap();

        let (outbound, receiver) = mpsc::channel();
        let renewal = RenewalHandle::production(directory.path().to_path_buf(), outbound);
        let _ = renewal.step(cycle_now);
        let request = receiver.recv().unwrap();
        let reference = request.fields["ref"].as_str().unwrap().to_owned();
        renewal.handle_supervisor(
            cycle_now,
            "started",
            &Map::from_iter([("ref".into(), Value::String(reference.clone()))]),
        );
        brain["evidence"]["lane_prerequisites"]["observed_at"] =
            Value::String((cycle_now + chrono::Duration::seconds(1)).to_rfc3339());
        brain["evidence"]["lane_prerequisites"]["expires_at"] =
            Value::String((cycle_now + chrono::Duration::minutes(10)).to_rfc3339());
        fs::write(&brain_path, serde_json::to_vec(&brain).unwrap()).unwrap();
        renewal.handle_supervisor(
            cycle_now,
            "stopped",
            &Map::from_iter([
                ("ref".into(), Value::String(reference)),
                ("exit_code".into(), Value::from(0)),
            ]),
        );
        assert!(!marker.exists());

        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let request: Map<String, Value> = serde_json::from_value(serde_json::json!({
            "use_id":"one","name":"chat","day":"20260101","env":{"CORTEX_MARKER":marker}
        }))
        .unwrap();
        let active = store.claim("chat", "one", &request).unwrap().unwrap();
        let (spawn_tx, _) = mpsc::channel();
        let (cancel_tx, _) = mpsc::channel();
        let (outbound_tx, _) = mpsc::channel();
        let state = CortexState::new(store, spawn_tx, cancel_tx, outbound_tx);
        spawn_one(
            state,
            executable_dir,
            Work {
                use_id: "one".into(),
                active,
                request,
            },
        )
        .unwrap();
        thread::sleep(Duration::from_millis(150));
        assert_eq!(fs::read_to_string(marker).unwrap(), "x");
    }

    #[test]
    fn non_json_stdout_is_log_only_info_and_error_defaults_terminal() {
        let directory = tempdir().unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let request: Map<String, Value> = serde_json::from_value(
            serde_json::json!({"use_id":"one","name":"chat","day":"20260101"}),
        )
        .unwrap();
        let active = store.claim("chat", "one", &request).unwrap().unwrap();
        let (spawn_tx, _) = mpsc::channel();
        let (cancel_tx, _) = mpsc::channel();
        let (outbound_tx, outbound_rx) = mpsc::channel();
        let state = CortexState::new(store.clone(), spawn_tx, cancel_tx, outbound_tx);
        let work = Work {
            use_id: "one".into(),
            active: active.clone(),
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
        assert!(store.has_finish(&active));
        let active_false = store.claim("chat", "two", &work.request).unwrap().unwrap();
        let work_false = Work {
            use_id: "two".into(),
            active: active_false.clone(),
            request: work.request.clone(),
        };
        handle_stdout(
            &state,
            &work_false,
            "{\"event\":\"error\",\"terminal\":false}".into(),
        );
        assert!(!store.has_finish(&active_false));
    }

    #[test]
    fn synthesized_error_messages_and_timeout_default_are_exact() {
        assert_eq!(timeout_for(&Map::new()), 600);
        assert_eq!(
            timeout_for(&serde_json::from_value(serde_json::json!({"timeout_seconds":7})).unwrap()),
            7
        );
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
            "Talent cancelled by chat watchdog",
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
    fn terminal_usage_writes_cogitate_records_with_model_fallback_and_segment() {
        for (use_id, name, model_version, expected_model, expected_context) in [
            (
                "one",
                "apps:chat",
                Some("usage-model"),
                "usage-model",
                "talent.apps.chat",
            ),
            ("two", "chat", None, "request-model", "talent.system.chat"),
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
    fn captured_process_group_survives_direct_child_reap() {
        let directory = tempdir().unwrap();
        let child_pid = directory.path().join("descendant-pid");
        let script = directory.path().join("child.sh");
        fs::write(
            &script,
            format!(
                "#!/bin/sh\nsleep 30 &\necho $! > {}\nexit 0\n",
                child_pid.display()
            ),
        )
        .unwrap();
        let mut child = Command::new("/bin/sh")
            .arg(&script)
            .process_group(0)
            .spawn()
            .unwrap();
        let pgid = i32::try_from(child.id()).unwrap();
        child.wait().unwrap();
        let descendant = (0..100)
            .find_map(|_| fs::read_to_string(&child_pid).ok())
            .map(|value| value.trim().parse::<i32>().unwrap())
            .unwrap();
        // A lazy lookup of the reaped direct child fails, but the captured group
        // still owns the descendant and can terminate it.
        assert!(nix::unistd::getpgid(Some(Pid::from_raw(pgid))).is_err());
        assert!(nix::sys::signal::kill(Pid::from_raw(descendant), None).is_ok());
        stop_group(pgid);
        for _ in 0..100 {
            if nix::sys::signal::kill(Pid::from_raw(descendant), None).is_err() {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("captured process group did not terminate its descendant");
    }

    #[test]
    fn stop_group_waits_for_a_graceful_exit_within_ten_seconds() {
        let directory = tempdir().unwrap();
        let ready = directory.path().join("ready");
        let mut child = Command::new("/bin/sh")
            .arg("-c")
            .arg(format!("trap '' TERM; : > {}; sleep 0.2", ready.display()))
            .process_group(0)
            .spawn()
            .unwrap();
        let pgid = i32::try_from(child.id()).unwrap();
        let reaped = thread::spawn(move || child.wait().unwrap());
        for _ in 0..100 {
            if ready.exists() {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        assert!(ready.exists());
        let started = std::time::Instant::now();
        stop_group(pgid);
        assert!(started.elapsed() >= Duration::from_millis(150));
        assert!(started.elapsed() < Duration::from_secs(10));
        assert!(reaped.join().unwrap().success());
    }

    #[test]
    fn stdin_write_failure_terminates_and_reaps_spawned_child() {
        let directory = tempdir().unwrap();
        let executable_dir = directory.path().join("bin");
        fs::create_dir(&executable_dir).unwrap();
        let child_pid = directory.path().join("child-pid");
        let python = executable_dir.join("python3");
        fs::write(
            &python,
            "#!/bin/sh\necho $$ > \"$CORTEX_CHILD_PID\"\nexec 0<&-\nsleep 30\n",
        )
        .unwrap();
        let mut permissions = fs::metadata(&python).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&python, permissions).unwrap();
        let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
        let request: Map<String, Value> = serde_json::from_value(serde_json::json!({
            "use_id":"one",
            "name":"chat",
            "prompt":"x".repeat(1_048_576),
            "env":{"CORTEX_CHILD_PID":child_pid}
        }))
        .unwrap();
        let active = store.claim("chat", "one", &request).unwrap().unwrap();
        let (spawn_tx, _) = mpsc::channel();
        let (cancel_tx, _) = mpsc::channel();
        let (outbound_tx, _) = mpsc::channel();
        let state = CortexState::new(store, spawn_tx, cancel_tx, outbound_tx);
        assert!(
            spawn_one(
                state,
                executable_dir,
                Work {
                    use_id: "one".into(),
                    active,
                    request,
                },
            )
            .is_err()
        );
        let pid = fs::read_to_string(child_pid)
            .unwrap()
            .trim()
            .parse::<i32>()
            .unwrap();
        assert!(nix::sys::signal::kill(Pid::from_raw(pid), None).is_err());
    }
}
