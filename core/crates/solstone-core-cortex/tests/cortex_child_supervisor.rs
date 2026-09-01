// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use nix::sys::signal::Signal;
use nix::unistd::Pid;
use serde_json::{Map, Value};
use solstone_core_cortex::test_hooks::{
    CortexState, CortexStore, Work, new_state, spawn_one, stop_group_with_grace,
};
use solstone_core_system::process::{self, Disposition, LaunchAuthority, LaunchError};
use tempfile::tempdir;

// Inverts LaunchAuthority's signal-aware i32 (non-negative = exit, negative = -signal)
// back into the raw Unix wait-status encoding ExitStatus::from_raw expects.
fn exit_status_from_code(code: i32) -> ExitStatus {
    if (0..=255).contains(&code) {
        ExitStatus::from_raw(code << 8)
    } else if code < 0 {
        ExitStatus::from_raw(-code)
    } else {
        ExitStatus::from_raw(code)
    }
}

struct ChildGuard {
    authority: Arc<Mutex<LaunchAuthority>>,
    pgid: i32,
    armed: bool,
}

impl ChildGuard {
    fn spawn(mut command: Command) -> Self {
        command.process_group(0);
        let authority = process::launch(
            Disposition::IndependentBoundedHelper {
                timeout: Duration::from_secs(30),
            },
            || command.spawn(),
            Box::new(|child, _timeout| {
                let pgid = i32::try_from(child.id()).map_err(|_| {
                    LaunchError::Terminate(std::io::Error::other("child pid does not fit i32"))
                })?;
                stop_group_with_grace(pgid, Duration::from_millis(200));
                Ok(())
            }),
        )
        .expect("fixture child");
        let pgid = i32::try_from(authority.pid()).expect("pid");
        Self {
            authority: Arc::new(Mutex::new(authority)),
            pgid,
            armed: true,
        }
    }

    fn try_wait(&mut self) -> Option<ExitStatus> {
        match self
            .authority
            .lock()
            .expect("cortex authority lock poisoned")
            .poll()
        {
            Ok(Some(code)) => Some(exit_status_from_code(code)),
            Ok(None) => None,
            Err(_) => Some(exit_status_from_code(-1)),
        }
    }

    fn wait_bounded(&mut self, timeout: Duration) -> ExitStatus {
        let started = Instant::now();
        loop {
            if let Some(status) = self.try_wait() {
                return status;
            }
            assert!(started.elapsed() < timeout, "fixture child did not exit");
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn reap_in_background(&mut self) -> mpsc::Receiver<ExitStatus> {
        let authority = Arc::clone(&self.authority);
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let code = loop {
                let polled = authority
                    .lock()
                    .expect("cortex authority lock poisoned")
                    .poll();
                match polled {
                    Ok(Some(code)) => break code,
                    Ok(None) => thread::sleep(Duration::from_millis(10)),
                    Err(_) => break -1,
                }
            };
            let _ = sender.send(exit_status_from_code(code));
        });
        receiver
    }

    fn disarm(&mut self) {
        self.armed = false;
    }

    fn terminate(&mut self) {
        if !self.armed {
            return;
        }
        self.armed = false;
        let _ = self
            .authority
            .lock()
            .expect("cortex authority lock poisoned")
            .terminate(Duration::from_millis(200));
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        self.terminate();
    }
}

struct RunningUsesGuard {
    state: CortexState,
}

impl Drop for RunningUsesGuard {
    fn drop(&mut self) {
        for running in self.state.running() {
            let _ = running
                .authority
                .lock()
                .expect("cortex authority lock poisoned")
                .terminate(Duration::from_millis(200));
        }
    }
}

fn package_roots(root: &Path) -> (PathBuf, PathBuf, PathBuf) {
    fs::write(root.join("pyproject.toml"), "[project]\nname = \"test\"\n").expect("pyproject");
    fs::create_dir_all(root.join(".git")).expect("git marker");
    let talent_root = root.join("solstone/talent");
    let apps_root = root.join("solstone/apps");
    let templates_dir = root.join("solstone/think/templates");
    fs::create_dir_all(&talent_root).expect("talent root");
    fs::create_dir_all(&apps_root).expect("apps root");
    fs::create_dir_all(&templates_dir).expect("templates root");
    (talent_root, apps_root, templates_dir)
}

fn install_worker(executable_dir: &Path) {
    fs::create_dir_all(executable_dir).expect("executable dir");
    let dest = executable_dir.join("solstone-core");
    if dest.exists() {
        let _ = fs::remove_file(&dest);
    }
    let src = PathBuf::from(env!("CARGO_BIN_EXE_solstone-cortex-worker"));
    if fs::hard_link(&src, &dest).is_err() {
        fs::copy(&src, &dest).expect("copy worker");
        let mut permissions = fs::metadata(&dest).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&dest, permissions).unwrap();
    }
}

fn write_poison(executable_dir: &Path, marker: &Path) {
    for name in ["python", "python3", "pytest", "uv", "ruff"] {
        let shim = executable_dir.join(name);
        fs::write(
            &shim,
            format!("#!/bin/sh\nprintf poison >> {}\n", marker.display()),
        )
        .unwrap();
        let mut permissions = fs::metadata(&shim).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&shim, permissions).unwrap();
    }
}

fn poll_file(path: &Path, timeout: Duration) -> String {
    let started = Instant::now();
    while started.elapsed() < timeout {
        if let Ok(text) = fs::read_to_string(path)
            && !text.is_empty()
        {
            return text;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("receipt {} was not written", path.display());
}

fn worker_command(mode: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_solstone-cortex-worker"));
    command.env("CORTEX_WORKER_MODE", mode);
    command
}

fn claim_work(store: &CortexStore, request: &Map<String, Value>) -> Work {
    let use_id = request["use_id"].as_str().unwrap().to_owned();
    let name = request["name"].as_str().unwrap();
    let (active, identity) = store.claim(name, &use_id, request).unwrap().unwrap();
    Work {
        use_id,
        talent_name: name.to_owned(),
        active,
        identity,
        request: request.clone(),
    }
}

#[test]
fn native_sibling_is_selected_and_lands_finish() {
    let directory = tempdir().unwrap();
    let executable_dir = directory.path().join("bin");
    install_worker(&executable_dir);
    let (talent_root, apps_root, templates_dir) = package_roots(directory.path());
    let marker = directory.path().join("marker");
    let poison_marker = directory.path().join("poison-marker");
    write_poison(&executable_dir, &poison_marker);
    let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
    let request: Map<String, Value> = serde_json::from_value(serde_json::json!({
        "use_id":"one",
        "name":"conversation",
        "day":"20260101",
        "timeout_seconds": 2,
        "facet":"top",
        "env":{
            "CORTEX_WORKER_MODE":"finish",
            "CORTEX_MARKER":marker,
            "SOL_FACET":"override",
            "PATH":format!("{}:/usr/bin", executable_dir.display()),
            "CORTEX_POISON_MARKER":poison_marker,
        }
    }))
    .unwrap();
    let work = claim_work(&store, &request);
    let active = work.active.clone();
    let state = new_state(store);
    let _guard = RunningUsesGuard {
        state: state.clone(),
    };
    spawn_one(
        state,
        executable_dir,
        &talent_root,
        &apps_root,
        &templates_dir,
        work,
        None,
    )
    .unwrap();
    assert_eq!(poll_file(&marker, Duration::from_secs(2)).trim(), "x");
    assert!(!poison_marker.exists());
    let completed = poll_file(&active.with_file_name("one.jsonl"), Duration::from_secs(2));
    assert!(completed.contains("\"event\":\"finish\""));
}

fn run_cwd_case(name: &str, frontmatter: &str) -> (PathBuf, PathBuf, String) {
    let directory = tempdir().unwrap();
    let root = directory.path();
    let fixture = root.join("fixture");
    fs::create_dir(&fixture).unwrap();
    let journal = fixture.join("journal");
    let executable_dir = root.join("bin");
    install_worker(&executable_dir);
    let (talent_root, apps_root, templates_dir) = package_roots(root);
    fs::write(talent_root.join(format!("{name}.md")), frontmatter).unwrap();
    let cwd_receipt = root.join("child-cwd");
    let request_path = root.join("request.json");
    let status_path = root.join("status");
    let request = serde_json::json!({
        "use_id":"one",
        "name":name,
        "timeout_seconds": 2,
        "env":{
            "CORTEX_WORKER_MODE":"pwd",
            "CORTEX_CWD":cwd_receipt,
        }
    });
    fs::write(&request_path, request.to_string()).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_solstone-cortex-controller"));
    command
        .current_dir(&fixture)
        .env("CORTEX_EXECUTABLE_DIR", &executable_dir)
        .env("CORTEX_TALENT_ROOT", &talent_root)
        .env("CORTEX_APPS_ROOT", &apps_root)
        .env("CORTEX_TEMPLATES_DIR", &templates_dir)
        .env("CORTEX_JOURNAL", &journal)
        .env("CORTEX_REQUEST_PATH", &request_path)
        .env("CORTEX_STATUS_PATH", &status_path)
        .env("CORTEX_RECEIPT_PATH", &cwd_receipt);
    let mut controller = ChildGuard::spawn(command);
    let cwd = poll_file(&cwd_receipt, Duration::from_secs(2));
    let status = poll_file(&status_path, Duration::from_secs(2));
    assert_eq!(status.trim(), "ok");
    let _ = controller.wait_bounded(Duration::from_secs(2));
    controller.disarm();
    (fixture, journal, cwd.trim().to_owned())
}

#[test]
fn generate_inherits_controller_fixture_directory() {
    let (fixture, journal, cwd) = run_cwd_case(
        "generate",
        "{\n\"type\": \"generate\",\n\"output\": \"json\"\n}\nbody\n",
    );
    assert_eq!(PathBuf::from(cwd), fixture);
    assert_ne!(fixture, journal);
}

#[test]
fn undeclared_cogitate_inherits_controller_fixture_directory() {
    let (fixture, journal, cwd) = run_cwd_case("defaulted", "{\n\"type\": \"cogitate\"\n}\nbody\n");
    assert_eq!(PathBuf::from(cwd), fixture);
    assert_ne!(fixture, journal);
}

#[test]
fn declared_cogitate_runs_in_journal_root() {
    let (fixture, journal, cwd) = run_cwd_case(
        "declared",
        "{\n\"type\": \"cogitate\",\n\"cwd\": \"journal\"\n}\nbody\n",
    );
    assert_eq!(PathBuf::from(cwd), journal);
    assert_ne!(fixture, journal);
}

#[test]
fn stdin_write_failure_terminates_and_reaps_spawned_child() {
    let directory = tempdir().unwrap();
    let executable_dir = directory.path().join("bin");
    install_worker(&executable_dir);
    let (talent_root, apps_root, templates_dir) = package_roots(directory.path());
    let child_pid = directory.path().join("child-pid");
    let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
    let request: Map<String, Value> = serde_json::from_value(serde_json::json!({
        "use_id":"one",
        "name":"conversation",
        "timeout_seconds": 2,
        "prompt":"x".repeat(1_048_576),
        "env":{
            "CORTEX_WORKER_MODE":"stdin-fail",
            "CORTEX_CHILD_PID":child_pid
        }
    }))
    .unwrap();
    let work = claim_work(&store, &request);
    let state = new_state(store);
    let _guard = RunningUsesGuard {
        state: state.clone(),
    };
    assert!(
        spawn_one(
            state,
            executable_dir,
            &talent_root,
            &apps_root,
            &templates_dir,
            work,
            None,
        )
        .is_err()
    );
    let pid = poll_file(&child_pid, Duration::from_secs(2))
        .trim()
        .parse::<i32>()
        .unwrap();
    assert!(nix::sys::signal::kill(Pid::from_raw(pid), None).is_err());
}

#[test]
fn captured_process_group_survives_direct_child_reap() {
    let directory = tempdir().unwrap();
    let ready = directory.path().join("descendant-ready");
    let mut command = worker_command("process-group");
    command.env("CORTEX_DESCENDANT_READY", &ready);
    let mut child = ChildGuard::spawn(command);
    let descendant = poll_file(&ready, Duration::from_secs(2))
        .trim()
        .parse::<i32>()
        .unwrap();
    let status = child.wait_bounded(Duration::from_secs(2));
    assert!(status.success());
    assert!(nix::unistd::getpgid(Some(Pid::from_raw(child.pgid))).is_err());
    assert!(nix::sys::signal::kill(Pid::from_raw(descendant), None).is_ok());
    stop_group_with_grace(child.pgid, Duration::from_millis(400));
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(2) {
        if nix::sys::signal::kill(Pid::from_raw(descendant), None).is_err() {
            child.disarm();
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("captured process group did not terminate its descendant");
}

#[test]
fn stop_group_records_term_and_does_not_kill_a_responsive_child() {
    let directory = tempdir().unwrap();
    let ready = directory.path().join("ready");
    let signals = directory.path().join("signals");
    let mut command = worker_command("responsive-stop");
    command.env("CORTEX_READY", &ready);
    command.env("CORTEX_SIGNALS", &signals);
    let mut child = ChildGuard::spawn(command);
    let _ = poll_file(&ready, Duration::from_secs(2));
    let status = child.reap_in_background();
    let grace = Duration::from_secs(2);
    let started = Instant::now();
    stop_group_with_grace(child.pgid, grace);
    assert!(started.elapsed() < grace, "responsive stop reached SIGKILL");
    let status = status.recv_timeout(grace).expect("bounded child reap");
    child.disarm();
    assert_eq!(poll_file(&signals, Duration::from_secs(1)).trim(), "TERM");
    assert_ne!(status.signal(), Some(Signal::SIGKILL as i32));
    assert!(status.success() || status.signal() == Some(Signal::SIGTERM as i32));
}

#[test]
fn stop_group_records_term_then_kills_an_ignoring_child() {
    let directory = tempdir().unwrap();
    let ready = directory.path().join("ready");
    let signals = directory.path().join("signals");
    let mut command = worker_command("ignore-term");
    command.env("CORTEX_READY", &ready);
    command.env("CORTEX_SIGNALS", &signals);
    let mut child = ChildGuard::spawn(command);
    let _ = poll_file(&ready, Duration::from_secs(2));
    stop_group_with_grace(child.pgid, Duration::from_millis(100));
    let status = child.wait_bounded(Duration::from_secs(2));
    child.disarm();
    assert_eq!(poll_file(&signals, Duration::from_secs(1)).trim(), "TERM");
    assert_eq!(status.signal(), Some(Signal::SIGKILL as i32));
}

#[test]
fn drain_keeps_running_use_alive_until_its_own_exit() {
    let directory = tempdir().unwrap();
    let ready = directory.path().join("ready");
    let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
    let request: Map<String, Value> =
        serde_json::from_value(serde_json::json!({"use_id":"one","name":"conversation"})).unwrap();
    let work = claim_work(&store, &request);
    let state = new_state(store);
    let mut command = worker_command("sleep-exit");
    command.env("CORTEX_READY", &ready);
    let mut child = ChildGuard::spawn(command);
    let _ = poll_file(&ready, Duration::from_secs(2));
    state.spawn_begin("one");
    state.spawn_started(
        &work,
        Arc::clone(&child.authority),
        Arc::new(Mutex::new(Vec::new())),
    );
    state.stop_accepting();
    assert!(child.try_wait().is_none());
    let status = child.wait_bounded(Duration::from_secs(2));
    child.disarm();
    assert!(status.success());
    state.finish("one", 0);
    state.spawn_finished();
    assert!(state.is_idle());
}

#[test]
fn immediate_stop_signals_the_running_group() {
    let directory = tempdir().unwrap();
    let ready = directory.path().join("ready");
    let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
    let request: Map<String, Value> =
        serde_json::from_value(serde_json::json!({"use_id":"one","name":"conversation"})).unwrap();
    let work = claim_work(&store, &request);
    let state = new_state(store);
    let mut command = worker_command("sleep-long");
    command.env("CORTEX_READY", &ready);
    let mut child = ChildGuard::spawn(command);
    let _ = poll_file(&ready, Duration::from_secs(2));
    state.spawn_begin("one");
    state.spawn_started(
        &work,
        Arc::clone(&child.authority),
        Arc::new(Mutex::new(Vec::new())),
    );
    for running in state.stop_immediately() {
        let _ = running
            .authority
            .lock()
            .expect("cortex authority lock poisoned")
            .terminate(Duration::from_millis(200));
    }
    let status = child.wait_bounded(Duration::from_secs(2));
    child.disarm();
    assert!(!status.success());
}

fn fixture_authority() -> Arc<Mutex<LaunchAuthority>> {
    let authority = process::launch(
        Disposition::IndependentBoundedHelper {
            timeout: Duration::from_secs(30),
        },
        || Command::new("/bin/sleep").arg("30").spawn(),
        Box::new(|child, _timeout| child.kill().map_err(LaunchError::Terminate)),
    )
    .expect("fixture launch");
    Arc::new(Mutex::new(authority))
}

fn running_state() -> (tempfile::TempDir, CortexState) {
    let directory = tempdir().unwrap();
    let store = CortexStore::new(directory.path().to_path_buf()).unwrap();
    let request: Map<String, Value> =
        serde_json::from_value(serde_json::json!({"use_id":"one","name":"conversation"})).unwrap();
    let work = claim_work(&store, &request);
    let state = new_state(store);
    state.spawn_begin("one");
    state.spawn_started(&work, fixture_authority(), Arc::new(Mutex::new(Vec::new())));
    (directory, state)
}

#[test]
fn drain_becomes_idle_after_finish() {
    let (_directory, state) = running_state();
    state.stop_accepting();
    assert!(!state.is_idle());
    state.finish("one", 0);
    state.spawn_finished();
    assert!(state.is_idle());
}

#[test]
fn immediate_stop_returns_running_uses_without_signaling() {
    let (_directory, state) = running_state();
    let running = state.stop_immediately();
    assert_eq!(running.len(), 1);
    assert_eq!(state.running().len(), 1);
}
