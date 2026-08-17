// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use chrono::TimeZone;
use solstone_core_doctor::{
    args::DoctorArgs,
    checks::{service_running, task_pace},
    context::CheckContext,
    registry::{self, Battery},
    run,
    vocabulary::{Check, Platform, Severity, Status, results_failed},
};
use std::{
    fs,
    io::Write,
    os::unix::{
        fs::{PermissionsExt, symlink},
        net::{UnixDatagram, UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

static NEXT_CONTEXT: AtomicUsize = AtomicUsize::new(0);

const POISON_INTERPRETER: &str = r#"#!/bin/sh
printf '%s\n' "$0" > "$POISON_MARKER"
exit 97
"#;

const ACCEPT_BOUND: Duration = Duration::from_millis(250);
const COMPLETE_STATUS_TIMEOUT: Duration = Duration::from_millis(250);
const WIRE_STATUS_TIMEOUT: Duration = Duration::from_millis(50);
const HEALTHY_TASK_PACE_FRAME: &[u8] =
    br#"{"event":"status","tasks":[{"name":"index","slow":false}],"tract":"supervisor"}
"#;

fn context() -> CheckContext {
    let root = std::env::temp_dir().join(format!(
        "solstone-doctor-service-check-{}-{}",
        std::process::id(),
        NEXT_CONTEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&root).expect("create staged test root");
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
        python_env_root: None,
        port: 5015,
        service_status_timeout: Duration::from_millis(10),
        service_status_command_override: None,
        parakeet_server_probe_override: None,
        speakers_analyze_resolvers: None,
    }
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

fn site_packages(context: &CheckContext, python: &str) -> PathBuf {
    let prefix = context
        .install_bin_dir
        .parent()
        .expect("staged install bin has a prefix");
    let site_packages = prefix.join("lib").join(python).join("site-packages");
    fs::create_dir_all(site_packages.join("solstone")).expect("create staged solstone package");
    fs::write(site_packages.join("solstone/__init__.py"), "").expect("write package marker");
    site_packages
}

fn metadata(
    site_packages: &Path,
    directory: &str,
    name: &str,
    version: &str,
    requires_python: Option<&str>,
) {
    let dist_info = site_packages.join(directory);
    fs::create_dir_all(&dist_info).expect("create staged dist-info");
    let requires_python = requires_python
        .map(|value| format!("Requires-Python: {value}\n"))
        .unwrap_or_default();
    fs::write(
        dist_info.join("METADATA"),
        format!("Name: {name}\nVersion: {version}\n{requires_python}\n"),
    )
    .expect("write staged metadata");
}

fn stage_ac9_batteries(context: &CheckContext) {
    fs::create_dir_all(&context.home_dir).expect("create staged home");
    fs::create_dir_all(&context.journal_path).expect("create staged journal");
    fs::create_dir_all(&context.install_bin_dir).expect("create staged install bin");
    let site_packages = site_packages(context, "python3.12");
    metadata(
        &site_packages,
        "solstone-1.2.3.dist-info",
        "solstone",
        "1.2.3",
        Some(">=3.12"),
    );
    metadata(
        &site_packages,
        "solstone_journal-1.2.3.dist-info",
        "solstone-journal",
        "1.2.3",
        None,
    );
    for module in ["frontmatter", "flask", "onnxruntime"] {
        fs::create_dir(site_packages.join(module)).expect("create host dependency module");
    }
    fs::write(
        context
            .install_bin_dir
            .parent()
            .expect("install prefix")
            .join("pyvenv.cfg"),
        "version = 3.12.0\n",
    )
    .expect("write staged pyvenv config");
    for binary in ["sol", "journal", "python"] {
        let path = context.install_bin_dir.join(binary);
        fs::write(&path, POISON_INTERPRETER).expect("write staged executable");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
            .expect("make staged executable");
    }
    let aliases = context.home_dir.join(".local/bin");
    fs::create_dir_all(&aliases).expect("create staged aliases");
    symlink(context.install_bin_dir.join("sol"), aliases.join("sol")).expect("link staged sol");
    symlink(
        context.install_bin_dir.join("journal"),
        aliases.join("journal"),
    )
    .expect("link staged journal");
    let unit = context
        .home_dir
        .join(".config/systemd/user/solstone.service");
    fs::create_dir_all(unit.parent().expect("unit parent")).expect("create unit parent");
    fs::write(
        unit,
        format!(
            "ExecStart={} start 5015\n",
            context.install_bin_dir.join("journal").display()
        ),
    )
    .expect("write staged service unit");
}

fn run_ac9_child(root: PathBuf) {
    let context = CheckContext {
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
        python_env_root: None,
        port: 5015,
        service_status_timeout: Duration::from_millis(10),
        service_status_command_override: None,
        parakeet_server_probe_override: None,
        speakers_analyze_resolvers: None,
    };
    for readiness in [false, true] {
        let results = run(
            &DoctorArgs {
                verbose: false,
                json: false,
                jsonl: false,
                port: 5015,
                feature: None,
                readiness,
            },
            &context,
        );
        assert_eq!(
            results.len(),
            registry::entries(if readiness {
                Battery::JournalReadiness
            } else {
                Battery::Journal
            })
            .len(),
            "entire battery must run"
        );
        assert!(
            results
                .iter()
                .all(|result| result.execution_error.is_none()),
            "a check failed before the battery completed: {results:?}"
        );
    }
}

#[test]
fn callosum_complete_frame_status_consumed() {
    let mut context = context();
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
    let mut context = context();
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
    let mut context = context();
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
    let mut context = context();
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
    let mut context = context();
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
    let mut context = context();
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
    let mut context = context();
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
    let mut context = context();
    install_linux_unit(&context);
    context.service_status_timeout = WIRE_STATUS_TIMEOUT;

    let ready_path = context.home_dir.join("exit-descendant-ready.sock");
    let ready_fifo = context.home_dir.join("exit-descendant-ready.fifo");
    let lifetime_path = context.home_dir.join("exit-descendant-lifetime.sock");
    let release_path = context.home_dir.join("exit-descendant-release.sock");
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
    let mut context = context();
    install_linux_unit(&context);
    context.service_status_timeout = WIRE_STATUS_TIMEOUT;

    let ready_path = context.home_dir.join("timeout-descendant-ready.sock");
    let lifetime_path = context.home_dir.join("timeout-descendant-lifetime.sock");
    let release_path = context.home_dir.join("timeout-descendant-release.sock");
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
        run_ac9_child(PathBuf::from(root));
        return;
    }

    let staged = context();
    fs::create_dir_all(&staged.home_dir).expect("create staged home");
    fs::create_dir_all(&staged.journal_path).expect("create staged journal");
    fs::create_dir_all(&staged.install_bin_dir).expect("create staged install bin");
    let root = staged
        .install_bin_dir
        .parent()
        .and_then(Path::parent)
        .expect("staged root")
        .to_path_buf();
    let poison_dir = root.join("poison");
    let marker = root.join("poison-marker");
    fs::create_dir_all(&poison_dir).expect("create poison directory");
    for name in ["python", "python3", "pip", "uv"] {
        let shim = poison_dir.join(name);
        fs::write(&shim, POISON_INTERPRETER).expect("write poison PATH shim");
        fs::set_permissions(&shim, fs::Permissions::from_mode(0o755))
            .expect("make poison PATH shim executable");
    }
    stage_ac9_batteries(&staged);

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

    let output = Command::new(std::env::current_exe().expect("test executable"))
        .args([
            "--exact",
            "ac9_full_batteries_never_invoke_poisoned_interpreters",
        ])
        .env("SOLSTONE_DOCTOR_AC9_ROOT", &root)
        .env("POISON_MARKER", &marker)
        .env("PATH", &poison_dir)
        .output()
        .expect("run isolated poison-interpreter child");
    assert!(
        output.status.success(),
        "child test failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !marker.exists(),
        "native doctor invoked a poison interpreter: {}",
        fs::read_to_string(&marker).unwrap_or_default()
    );
}
