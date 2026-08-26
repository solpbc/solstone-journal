// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use chrono::TimeZone;
use solstone_core_doctor::{
    args::DoctorArgs,
    checks::{service_running, task_pace},
    context::CheckContext,
    run,
    vocabulary::{Check, CheckResult, Platform, Severity, Status, results_failed},
};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    os::unix::{
        fs::PermissionsExt,
        net::{UnixDatagram, UnixListener, UnixStream},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

static NEXT_CONTEXT: AtomicUsize = AtomicUsize::new(0);

const ACCEPT_BOUND: Duration = Duration::from_millis(250);
const COMPLETE_STATUS_TIMEOUT: Duration = Duration::from_millis(250);
const WIRE_STATUS_TIMEOUT: Duration = Duration::from_millis(50);
const ISOLATED_BATTERY_BOUND: Duration = Duration::from_secs(10);
const HEALTHY_TASK_PACE_FRAME: &[u8] =
    br#"{"event":"status","tasks":[{"name":"index","slow":false}],"tract":"supervisor"}
"#;

struct TestRoot(PathBuf);

impl Drop for TestRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn context() -> (CheckContext, TestRoot) {
    let root = std::env::temp_dir().join(format!(
        "solstone-doctor-service-check-{}-{}",
        std::process::id(),
        NEXT_CONTEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&root).expect("create staged test root");
    (
        CheckContext {
            home_dir: root.join("home"),
            install_bin_dir: root.join("install/bin"),
            journal_path: root.join("journal"),
            callosum_socket_path: root.join("journal/health/callosum.sock"),
            platform: Platform::Linux,
            now: chrono::Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            host_arch: "x86_64".into(),
            hostname: "test-host".into(),
            machine_id: Some("test-machine".into()),
            checkout_root: None,
            payload_root: None,
            port: 5015,
            service_status_timeout: Duration::from_millis(10),
            service_status_command_override: None,
            parakeet_server_probe_override: None,
            speakers_analyze_resolvers: None,
            vad_runtime_probe: None,
            free_space_bytes_override: None,
        },
        TestRoot(root),
    )
}

fn task_pace_check() -> Check {
    Check {
        name: "task_pace",
        severity: Severity::Advisory,
        platforms: &[Platform::Linux],
    }
}

fn service_running_check() -> Check {
    Check {
        name: "service_running",
        severity: Severity::Blocker,
        platforms: &[Platform::Linux],
    }
}

fn bind_listener(path: &Path) -> UnixListener {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create socket parent");
    }
    if path.exists() {
        fs::remove_file(path).expect("remove stale socket");
    }
    UnixListener::bind(path).expect("bind callosum socket")
}

fn spawn_result<T, F>(work: F) -> mpsc::Receiver<T>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let (tx, rx) = mpsc::sync_channel(0);
    thread::spawn(move || {
        let _ = tx.send(work());
    });
    rx
}

fn accept_stream(listener: UnixListener) -> UnixStream {
    let (ready_tx, ready_rx) = mpsc::sync_channel(0);
    thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accept doctor client");
        let _ = ready_tx.send(stream);
    });
    ready_rx
        .recv_timeout(ACCEPT_BOUND)
        .expect("receipted accept")
}

fn write_exact(stream: &mut UnixStream, payload: &[u8]) {
    stream.write_all(payload).expect("write frame bytes");
    stream.flush().ok();
}

fn recv_result<T>(rx: mpsc::Receiver<T>, bound: Duration) -> T {
    rx.recv_timeout(bound).expect("check finished")
}

fn install_linux_unit(context: &CheckContext) {
    let unit = context
        .home_dir
        .join(".config/systemd/user/solstone.service");
    fs::create_dir_all(unit.parent().expect("unit parent")).expect("create unit parent");
    fs::write(&unit, b"x").expect("write service unit");
}

fn write_script(path: &Path, body: &str) {
    fs::write(path, body).expect("write override script");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("chmod override script");
}

struct BoundedProcessGroup {
    child: Option<Child>,
    group: nix::unistd::Pid,
}

impl BoundedProcessGroup {
    fn spawn(mut command: Command) -> Self {
        command.process_group(0);
        let child = command.spawn().expect("spawn isolated doctor battery");
        let group = nix::unistd::Pid::from_raw(
            i32::try_from(child.id()).expect("doctor battery PID fits process-group ID"),
        );
        Self {
            child: Some(child),
            group,
        }
    }

    fn wait_bounded(&mut self, timeout: Duration) -> ExitStatus {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self
                .child
                .as_mut()
                .expect("owned doctor battery")
                .try_wait()
                .expect("poll isolated doctor battery")
            {
                self.terminate_group();
                self.child.take();
                self.assert_group_gone();
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "isolated doctor battery exceeded {timeout:?}"
            );
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn terminate_group(&self) {
        match nix::sys::signal::killpg(self.group, nix::sys::signal::Signal::SIGKILL) {
            Ok(()) | Err(nix::errno::Errno::ESRCH) => {}
            Err(error) => panic!("terminate isolated doctor battery group: {error}"),
        }
    }

    fn assert_group_gone(&self) {
        let deadline = Instant::now() + Duration::from_millis(500);
        loop {
            match nix::sys::signal::killpg(self.group, None::<nix::sys::signal::Signal>) {
                Err(nix::errno::Errno::ESRCH) => return,
                Ok(()) | Err(nix::errno::Errno::EPERM) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(5));
                }
                Ok(()) | Err(nix::errno::Errno::EPERM) => {
                    panic!("isolated doctor battery group survived cleanup")
                }
                Err(error) => panic!("inspect isolated doctor battery group: {error}"),
            }
        }
    }
}

impl Drop for BoundedProcessGroup {
    fn drop(&mut self) {
        if self.child.is_some() {
            self.terminate_group();
        }
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.assert_group_gone();
    }
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\"'\"'"))
}

struct DescendantRelease {
    release_socket: PathBuf,
}

impl DescendantRelease {
    fn release(&self) {
        let Ok(socket) = UnixDatagram::unbound() else {
            return;
        };
        if socket.connect(&self.release_socket).is_ok() {
            let _ = socket.send(b"release");
        }
    }
}

impl Drop for DescendantRelease {
    fn drop(&mut self) {
        self.release();
    }
}

fn descendant_survives_cleanup_bound(lifetime_socket: &Path) -> bool {
    let deadline = Instant::now() + Duration::from_millis(300);
    loop {
        if UnixStream::connect(lifetime_socket).is_err() {
            return false;
        }
        if Instant::now() >= deadline {
            return true;
        }
        thread::sleep(Duration::from_millis(5));
    }
}

fn parakeet_unreachable_probe(_: &Path, _: Duration) -> Result<(), String> {
    Err("fixture unreachable".into())
}

fn speakers_binary_missing() -> Result<PathBuf, String> {
    Err("fixture helper missing".into())
}

fn speakers_model_ready(_: &str) -> Result<PathBuf, solstone_core_transcribe::TranscribeError> {
    Ok("/fixture/model.onnx".into())
}

fn poison_battery_context(root: &Path) -> CheckContext {
    fs::create_dir_all(root.join("journal")).expect("create poison-battery journal");
    CheckContext {
        home_dir: root.join("home"),
        install_bin_dir: root.join("install/bin"),
        journal_path: root.join("journal"),
        callosum_socket_path: root.join("journal/health/callosum.sock"),
        platform: Platform::Linux,
        now: chrono::Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
        host_arch: "x86_64".into(),
        hostname: "fixture-host".into(),
        machine_id: Some("fixture-machine".into()),
        checkout_root: None,
        payload_root: None,
        port: 5015,
        service_status_timeout: Duration::from_millis(1),
        service_status_command_override: None,
        parakeet_server_probe_override: Some(parakeet_unreachable_probe),
        speakers_analyze_resolvers: Some((speakers_binary_missing, speakers_model_ready)),
        vad_runtime_probe: None,
        free_space_bytes_override: None,
    }
}

fn verdicts(rows: &[CheckResult]) -> BTreeMap<&str, Status> {
    rows.iter().map(|row| (row.name, row.status)).collect()
}

fn run_poison_battery_child(root: &Path) {
    let context = poison_battery_context(root);
    let journal = run(
        &DoctorArgs {
            verbose: false,
            json: false,
            jsonl: false,
            port: 5015,
            readiness: false,
        },
        &context,
    );
    let readiness = run(
        &DoctorArgs {
            readiness: true,
            ..DoctorArgs {
                verbose: false,
                json: false,
                jsonl: false,
                port: 5015,
                readiness: false,
            }
        },
        &context,
    );
    assert!(
        journal
            .iter()
            .chain(&readiness)
            .all(|row| row.execution_error.is_none()),
        "poison battery must complete every native check without an execution error"
    );
    assert_eq!(
        verdicts(&journal),
        BTreeMap::from([
            ("disk_space", Status::Skip),
            ("config_dir_readable", Status::Ok),
            ("journal_dir_writable", Status::Ok),
            ("supervisor_conflict", Status::Skip),
            ("service_identity", Status::Skip),
            ("service_running", Status::Skip),
            ("journal_sync", Status::Ok),
            ("journal_caught_up", Status::Ok),
            ("task_pace", Status::Skip),
            ("brain", Status::Warn),
            ("capture_health", Status::Skip),
            ("client_binding", Status::Ok),
            ("client_delivery_stall", Status::Skip),
            ("client_ingest_health", Status::Skip),
            ("orphan_segment_pdf", Status::Skip),
            ("launchd_stale_plist", Status::Skip),
            ("default_stt_ready", Status::Warn),
            ("parakeet_cpp_stt_ready", Status::Skip),
            ("speakers_analyze_installation", Status::Fail),
            ("vad_runtime_ready", Status::Fail),
            ("skill_state", Status::Skip),
        ])
    );
    assert_eq!(
        verdicts(&readiness),
        BTreeMap::from([
            ("local_bin_solstone_reachable", Status::Warn),
            ("disk_space", Status::Skip),
            ("journal_dir_writable", Status::Ok),
            ("default_stt_ready", Status::Warn),
            ("parakeet_cpp_stt_ready", Status::Skip),
            ("speakers_analyze_installation", Status::Fail),
            ("vad_runtime_ready", Status::Fail),
        ])
    );
}

fn service_running_from_status(crashed: serde_json::Value) -> CheckResult {
    let (mut context, _root) = context();
    install_linux_unit(&context);
    context.service_status_timeout = COMPLETE_STATUS_TIMEOUT;
    let listener = bind_listener(&context.callosum_socket_path);
    let socket_path = context.callosum_socket_path.clone();
    let result_rx = spawn_result(move || {
        service_running::run(&context, service_running_check()).expect("service check")
    });
    let mut stream = accept_stream(listener);
    let mut frame = serde_json::json!({
        "tract": "supervisor",
        "event": "status",
        "crashed": crashed,
    })
    .to_string()
    .into_bytes();
    frame.push(b'\n');
    write_exact(&mut stream, &frame);
    let row = recv_result(result_rx, COMPLETE_STATUS_TIMEOUT + ACCEPT_BOUND);
    drop(stream);
    fs::remove_file(socket_path).expect("remove callosum fixture socket");
    row
}

#[test]
fn ac15_service_running_receives_ok_and_crash_status_over_callosum() {
    let ok = service_running_from_status(serde_json::json!([]));
    assert_eq!(ok.status, Status::Ok);
    assert_eq!(ok.detail, "journal service is running");

    let crash = service_running_from_status(serde_json::json!([{
        "name": "foo",
        "restart_attempts": 3,
    }]));
    assert_eq!(crash.status, Status::Fail);
    assert!(
        crash
            .detail
            .contains("crash-loop: foo (3 restart attempts)")
    );
    assert_eq!(crash.fix.as_deref(), Some("run journal service logs"));
}

#[test]
fn callosum_complete_frame_status_consumed() {
    let (mut context, _root) = context();
    context.service_status_timeout = COMPLETE_STATUS_TIMEOUT;
    let listener = bind_listener(&context.callosum_socket_path);
    let started = Instant::now();
    let result_rx = spawn_result(move || task_pace::run(&context, task_pace_check()).unwrap());
    let mut stream = accept_stream(listener);
    write_exact(&mut stream, HEALTHY_TASK_PACE_FRAME);
    let row = recv_result(result_rx, COMPLETE_STATUS_TIMEOUT + ACCEPT_BOUND);
    drop(stream);
    assert!(started.elapsed() < COMPLETE_STATUS_TIMEOUT + ACCEPT_BOUND);
    assert_eq!(row.status, Status::Ok);
    assert_eq!(row.detail, "tasks on pace");
}

#[test]
fn callosum_malformed_frame_does_not_count_as_status() {
    let payload = b"not-json\n";
    let (mut context, _root) = context();
    context.service_status_timeout = WIRE_STATUS_TIMEOUT;
    let listener = bind_listener(&context.callosum_socket_path);
    let result_rx = spawn_result(move || task_pace::run(&context, task_pace_check()).unwrap());
    let mut stream = accept_stream(listener);
    write_exact(&mut stream, payload);
    let row = recv_result(result_rx, WIRE_STATUS_TIMEOUT + ACCEPT_BOUND);
    drop(stream);
    assert_eq!(row.status, Status::Skip);
    assert_eq!(row.detail, "supervisor status unavailable");
}

#[test]
fn callosum_partial_frame_does_not_count_as_status() {
    let payload = br#"{"tract":"supervisor","event":"status""#;
    let (mut context, _root) = context();
    context.service_status_timeout = WIRE_STATUS_TIMEOUT;
    let listener = bind_listener(&context.callosum_socket_path);
    let result_rx = spawn_result(move || task_pace::run(&context, task_pace_check()).unwrap());
    let mut stream = accept_stream(listener);
    write_exact(&mut stream, payload);
    let row = recv_result(result_rx, WIRE_STATUS_TIMEOUT + ACCEPT_BOUND);
    drop(stream);
    assert_eq!(row.status, Status::Skip);
    assert_eq!(row.detail, "supervisor status unavailable");
}

#[test]
fn callosum_accepted_silent_times_out() {
    let (mut context, _root) = context();
    context.service_status_timeout = WIRE_STATUS_TIMEOUT;
    let listener = bind_listener(&context.callosum_socket_path);
    let waited_from = Instant::now();
    let result_rx = spawn_result(move || task_pace::run(&context, task_pace_check()).unwrap());
    let stream = accept_stream(listener);
    let row = recv_result(result_rx, WIRE_STATUS_TIMEOUT + ACCEPT_BOUND);
    let elapsed = waited_from.elapsed();
    drop(stream);
    assert!(
        elapsed >= WIRE_STATUS_TIMEOUT,
        "silent peer must wait out service_status_timeout, elapsed={elapsed:?}"
    );
    assert_eq!(row.status, Status::Skip);
    assert_eq!(row.detail, "supervisor status unavailable");
}

#[test]
fn callosum_accepted_then_eof_without_status() {
    // ReadFrame::Eof becomes a reconnect gap; next_message keeps waiting, so fetch
    // still ends on service_status_timeout. Distinguishability is the close: this
    // test drops the peer immediately. The silent case must hold the stream open
    // for the whole wait — an immediate close cannot satisfy that case.
    let (mut context, _root) = context();
    context.service_status_timeout = WIRE_STATUS_TIMEOUT;
    let listener = bind_listener(&context.callosum_socket_path);
    let result_rx = spawn_result(move || task_pace::run(&context, task_pace_check()).unwrap());
    let stream = accept_stream(listener);
    drop(stream);
    let row = recv_result(result_rx, WIRE_STATUS_TIMEOUT + ACCEPT_BOUND);
    assert_eq!(row.status, Status::Skip);
    assert_eq!(row.detail, "supervisor status unavailable");
}

#[test]
fn service_running_accepted_silent_warns() {
    let (mut context, _root) = context();
    install_linux_unit(&context);
    let script = context.home_dir.join("not-failed.sh");
    write_script(&script, "#!/bin/sh\necho active\nexit 1\n");
    context.service_status_command_override = Some((script, Vec::new()));
    context.service_status_timeout = WIRE_STATUS_TIMEOUT;
    let listener = bind_listener(&context.callosum_socket_path);
    let waited_from = Instant::now();
    let result_rx =
        spawn_result(move || service_running::run(&context, service_running_check()).unwrap());
    let stream = accept_stream(listener);
    let row = recv_result(result_rx, WIRE_STATUS_TIMEOUT + ACCEPT_BOUND);
    let elapsed = waited_from.elapsed();
    drop(stream);
    assert!(
        elapsed >= WIRE_STATUS_TIMEOUT,
        "silent service_running must wait out fetch, elapsed={elapsed:?}"
    );
    assert_eq!(row.status, Status::Warn);
    assert_eq!(row.detail, "service installed but not running");
    assert!(row.execution_error.is_none());
    assert!(!results_failed(&[row]));
}

#[test]
fn service_running_failed_service_command_fails() {
    let (mut context, _root) = context();
    install_linux_unit(&context);
    let script = context.home_dir.join("failed-service.sh");
    write_script(&script, "#!/bin/sh\necho failed\nexit 0\n");
    context.service_status_command_override = Some((script, Vec::new()));
    context.service_status_timeout = WIRE_STATUS_TIMEOUT;
    let listener = bind_listener(&context.callosum_socket_path);
    let result_rx =
        spawn_result(move || service_running::run(&context, service_running_check()).unwrap());
    let stream = accept_stream(listener);
    let row = recv_result(result_rx, WIRE_STATUS_TIMEOUT + ACCEPT_BOUND);
    drop(stream);
    assert_eq!(row.status, Status::Fail);
    assert_eq!(row.detail, "journal service unit is failed");
    assert_eq!(
        row.fix.as_deref(),
        Some("run journal service restart; if it persists, run journal service logs")
    );
    assert!(row.execution_error.is_none());
}

#[test]
fn service_timeout_descendant() {
    if std::env::var_os("SOLSTONE_DOCTOR_TIMEOUT_DESCENDANT").is_none() {
        return;
    }
    let lifetime_socket = PathBuf::from(
        std::env::var_os("SOLSTONE_DOCTOR_TIMEOUT_LIFETIME_SOCKET")
            .expect("descendant lifetime socket"),
    );
    let release_socket = PathBuf::from(
        std::env::var_os("SOLSTONE_DOCTOR_TIMEOUT_RELEASE_SOCKET")
            .expect("descendant release socket"),
    );
    let ready_socket = PathBuf::from(
        std::env::var_os("SOLSTONE_DOCTOR_TIMEOUT_READY_SOCKET")
            .expect("descendant readiness socket"),
    );
    let lifetime = UnixListener::bind(lifetime_socket).expect("bind descendant lifetime socket");
    thread::spawn(move || while lifetime.accept().is_ok() {});
    let release = UnixDatagram::bind(release_socket).expect("bind descendant release socket");
    release
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("bound descendant cleanup watchdog");
    let ready = UnixDatagram::unbound().expect("create descendant readiness sender");
    ready
        .connect(ready_socket)
        .expect("connect descendant readiness sender");
    ready.send(b"ready").expect("signal descendant readiness");
    if let Some(ready_fifo) = std::env::var_os("SOLSTONE_DOCTOR_TIMEOUT_READY_FIFO") {
        let mut ready_fifo = fs::OpenOptions::new()
            .write(true)
            .open(ready_fifo)
            .expect("open descendant readiness FIFO");
        ready_fifo
            .write_all(b"ready\n")
            .expect("signal shell readiness");
    }
    let mut message = [0_u8; 16];
    let _ = release.recv(&mut message);
}

#[test]
fn service_command_exit_still_terminates_descendants_holding_output_pipes() {
    let (mut context, _root) = context();
    install_linux_unit(&context);
    context.service_status_timeout = WIRE_STATUS_TIMEOUT;

    // Keep Unix-domain paths short even when the checkout or TMPDIR is deeply nested.
    let ready_path = context.home_dir.join("er");
    let ready_fifo = context.home_dir.join("ef");
    let lifetime_path = context.home_dir.join("el");
    let release_path = context.home_dir.join("ex");
    nix::unistd::mkfifo(
        &ready_fifo,
        nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
    )
    .expect("create descendant readiness FIFO");
    let ready = UnixDatagram::bind(&ready_path).expect("bind descendant readiness socket");
    ready
        .set_read_timeout(Some(Duration::from_secs(1)))
        .expect("bound readiness watchdog");
    let cleanup = DescendantRelease {
        release_socket: release_path.clone(),
    };

    let script = context.home_dir.join("exit-with-descendant.sh");
    let executable = std::env::current_exe().expect("test executable");
    write_script(
        &script,
        &format!(
            "#!/bin/sh\nSOLSTONE_DOCTOR_TIMEOUT_DESCENDANT=1 \\\nSOLSTONE_DOCTOR_TIMEOUT_LIFETIME_SOCKET={} \\\nSOLSTONE_DOCTOR_TIMEOUT_RELEASE_SOCKET={} \\\nSOLSTONE_DOCTOR_TIMEOUT_READY_SOCKET={} \\\nSOLSTONE_DOCTOR_TIMEOUT_READY_FIFO={} \\\n{} --exact service_timeout_descendant --nocapture &\nIFS= read -r descendant_ready < {}\n",
            shell_quote(&lifetime_path),
            shell_quote(&release_path),
            shell_quote(&ready_path),
            shell_quote(&ready_fifo),
            shell_quote(&executable),
            shell_quote(&ready_fifo),
        ),
    );
    context.service_status_command_override = Some((script, Vec::new()));

    let listener = bind_listener(&context.callosum_socket_path);
    let result_rx =
        spawn_result(move || service_running::run(&context, service_running_check()).unwrap());
    let stream = accept_stream(listener);
    let mut message = [0_u8; 16];
    let length = ready
        .recv(&mut message)
        .expect("descendant signaled readiness");
    assert_eq!(&message[..length], b"ready");

    let row = result_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("exited service command completed without pipe hang");
    drop(stream);
    let survived = descendant_survives_cleanup_bound(&lifetime_path);
    cleanup.release();
    assert!(
        !survived,
        "service command descendant held output pipes after its parent exited"
    );
    assert_eq!(row.status, Status::Warn);
    assert_eq!(row.detail, "service installed but not running");
    assert!(row.execution_error.is_none());
}

#[test]
fn service_timeout_terminates_the_owned_descendant_group() {
    let (mut context, _root) = context();
    install_linux_unit(&context);
    context.service_status_timeout = WIRE_STATUS_TIMEOUT;

    // Keep Unix-domain paths short even when the checkout or TMPDIR is deeply nested.
    let ready_path = context.home_dir.join("tr");
    let lifetime_path = context.home_dir.join("tl");
    let release_path = context.home_dir.join("tx");
    let ready = UnixDatagram::bind(&ready_path).expect("bind descendant readiness socket");
    ready
        .set_read_timeout(Some(Duration::from_secs(1)))
        .expect("bound readiness watchdog");
    let cleanup = DescendantRelease {
        release_socket: release_path.clone(),
    };

    let script = context.home_dir.join("timeout-with-descendant.sh");
    let executable = std::env::current_exe().expect("test executable");
    write_script(
        &script,
        &format!(
            "#!/bin/sh\nSOLSTONE_DOCTOR_TIMEOUT_DESCENDANT=1 \\\nSOLSTONE_DOCTOR_TIMEOUT_LIFETIME_SOCKET={} \\\nSOLSTONE_DOCTOR_TIMEOUT_RELEASE_SOCKET={} \\\nSOLSTONE_DOCTOR_TIMEOUT_READY_SOCKET={} \\\n{} --exact service_timeout_descendant --nocapture >/dev/null 2>&1 &\nwait \"$!\"\n",
            shell_quote(&lifetime_path),
            shell_quote(&release_path),
            shell_quote(&ready_path),
            shell_quote(&executable),
        ),
    );
    context.service_status_command_override = Some((script, Vec::new()));

    let listener = bind_listener(&context.callosum_socket_path);
    let result_rx =
        spawn_result(move || service_running::run(&context, service_running_check()).unwrap());
    let stream = accept_stream(listener);
    let mut message = [0_u8; 16];
    let length = ready
        .recv(&mut message)
        .expect("descendant signaled readiness");
    assert_eq!(&message[..length], b"ready");
    assert!(
        UnixStream::connect(&lifetime_path).is_ok(),
        "positive control must observe the live descendant"
    );

    let row = result_rx
        .recv_timeout(Duration::from_secs(3))
        .expect("timed-out service probe completed");
    drop(stream);
    let survived = descendant_survives_cleanup_bound(&lifetime_path);
    cleanup.release();
    assert!(
        !survived,
        "service probe descendant survived its owned process-group timeout"
    );
    assert_eq!(row.status, Status::Warn);
    assert_eq!(row.detail, "service installed but not running");
    assert!(row.execution_error.is_none());
}

#[test]
fn ac9_full_batteries_never_invoke_poisoned_interpreters() {
    if let Some(root) = std::env::var_os("SOLSTONE_DOCTOR_AC9_ROOT") {
        run_poison_battery_child(&PathBuf::from(root));
        return;
    }

    let (staged, _root) = context();
    let root = staged.home_dir.parent().expect("staged root").to_path_buf();
    fs::create_dir_all(&staged.journal_path).expect("create staged journal");
    let poison_dir = root.join("poison");
    let marker = root.join("poison-marker");
    fs::create_dir_all(&poison_dir).expect("create poison directory");
    let poison_script = "#!/bin/sh\nprintf '%s\\n' \"$0\" > \"$POISON_MARKER\"\nexit 97\n";
    for name in ["python", "python3", "pytest", "ruff", "uv", "pip"] {
        let shim = poison_dir.join(name);
        fs::write(&shim, poison_script).expect("write poison PATH shim");
        fs::set_permissions(&shim, fs::Permissions::from_mode(0o755))
            .expect("make poison PATH shim executable");
    }
    let aliases = staged.home_dir.join(".local/bin");
    fs::create_dir_all(&aliases).expect("create staged aliases");
    for name in ["journal", "solstone"] {
        fs::write(aliases.join(name), "fixture alias").expect("write staged alias");
    }
    let python_env = root.join("venv/bin");
    fs::create_dir_all(&python_env).expect("create staged Python environment");
    fs::write(python_env.join("python"), poison_script).expect("write staged Python poison");
    fs::set_permissions(python_env.join("python"), fs::Permissions::from_mode(0o755))
        .expect("make staged Python poison executable");

    let positive = Command::new(poison_dir.join("python"))
        .env("POISON_MARKER", &marker)
        .status()
        .expect("invoke poison shim as positive control");
    assert_eq!(positive.code(), Some(97));
    assert!(
        marker.exists(),
        "positive control must create the poison marker"
    );
    fs::remove_file(&marker).expect("clear positive-control marker");

    let stdout_path = root.join("doctor-battery.stdout");
    let stderr_path = root.join("doctor-battery.stderr");
    let stdout = fs::File::create(&stdout_path).expect("create doctor battery stdout");
    let stderr = fs::File::create(&stderr_path).expect("create doctor battery stderr");
    let mut command = Command::new(std::env::current_exe().expect("test executable"));
    command
        .args([
            "--exact",
            "ac9_full_batteries_never_invoke_poisoned_interpreters",
        ])
        .env_clear()
        .env("SOLSTONE_DOCTOR_AC9_ROOT", &root)
        .env("POISON_MARKER", &marker)
        .env("PATH", &poison_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    let mut child = BoundedProcessGroup::spawn(command);
    let status = child.wait_bounded(ISOLATED_BATTERY_BOUND);
    let stdout = fs::read(&stdout_path).expect("read doctor battery stdout");
    let stderr = fs::read(&stderr_path).expect("read doctor battery stderr");
    assert!(
        status.success(),
        "child test failed:\n{}\n{}",
        String::from_utf8_lossy(&stdout),
        String::from_utf8_lossy(&stderr)
    );
    assert!(
        !marker.exists(),
        "native doctor invoked a poison interpreter: {}",
        fs::read_to_string(&marker).unwrap_or_default()
    );
}
