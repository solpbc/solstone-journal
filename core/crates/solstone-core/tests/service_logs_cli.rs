// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::fs;
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
use std::io::Write as _;
use std::os::unix::ffi::OsStringExt as _;
use std::os::unix::fs::{PermissionsExt as _, symlink};
use std::path::{Path, PathBuf};
#[cfg(target_os = "linux")]
use std::process::{Child, ExitStatus};
use std::process::{Command, Output, Stdio};
#[cfg(target_os = "linux")]
use std::thread;
#[cfg(target_os = "linux")]
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
use nix::sys::signal::{Signal, kill};
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
use nix::sys::stat::Mode;
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
use nix::sys::{ptrace, wait::WaitPidFlag, wait::WaitStatus, wait::waitpid};
#[cfg(target_os = "linux")]
use nix::unistd::Pid;
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
use nix::unistd::mkfifo;

const BINARY: &str = env!("CARGO_BIN_EXE_solstone-core");
const POISON_INTERPRETER: &str = r#"#!/bin/sh
printf '%s:%s\n' "${POISON_ROUTE:-reached}" "${0##*/}" >> "$POISON_MARKER"
exit 97
"#;

struct TestJournal(tempfile::TempDir);

impl TestJournal {
    fn new() -> Self {
        Self(tempfile::tempdir().expect("create journal tempdir"))
    }

    fn path(&self) -> &Path {
        self.0.path()
    }

    fn service_log(&self) -> PathBuf {
        self.path().join("health/service.log")
    }

    fn write(&self, bytes: &[u8]) {
        fs::create_dir_all(self.path().join("health")).expect("create health directory");
        fs::write(self.service_log(), bytes).expect("write service log");
    }
}

fn run(journal: &OsStr, arguments: &[OsString]) -> Output {
    Command::new(BINARY)
        .args(arguments)
        .env("SOLSTONE_JOURNAL", journal)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("execute real solstone-core")
}

fn args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}

struct PoisonHarness {
    core: PathBuf,
    bin: PathBuf,
    marker: PathBuf,
}

impl PoisonHarness {
    fn new(root: &Path) -> Self {
        let bin = root.join("bin");
        fs::create_dir(&bin).unwrap();
        let core = bin.join("solstone-core");
        fs::copy(BINARY, &core).unwrap();
        make_executable(&core);
        for name in ["python", "python3", "pytest", "uv", "ruff"] {
            let path = bin.join(name);
            fs::write(&path, POISON_INTERPRETER).unwrap();
            make_executable(&path);
        }
        Self {
            core,
            bin,
            marker: root.join("python-invoked"),
        }
    }

    fn prove_live(&self) {
        let _ = fs::remove_file(&self.marker);
        for name in ["python", "python3", "pytest", "uv", "ruff"] {
            let sibling = Command::new(self.bin.join(name))
                .env("POISON_MARKER", &self.marker)
                .env("POISON_ROUTE", "sibling")
                .status()
                .unwrap();
            assert_eq!(sibling.code(), Some(97));
            let path = Command::new(name)
                .env("PATH", &self.bin)
                .env("POISON_MARKER", &self.marker)
                .env("POISON_ROUTE", "path")
                .status()
                .unwrap();
            assert_eq!(path.code(), Some(97));
        }
        let rows = fs::read_to_string(&self.marker).unwrap();
        assert_eq!(rows.lines().count(), 10);
        let expected = ["sibling", "path"]
            .into_iter()
            .flat_map(|route| {
                ["python", "python3", "pytest", "uv", "ruff"]
                    .into_iter()
                    .map(move |name| format!("{route}:{name}"))
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(
            rows.lines().map(str::to_owned).collect::<BTreeSet<_>>(),
            expected
        );
        fs::remove_file(&self.marker).unwrap();
    }
}

fn make_executable(path: &Path) {
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn missing_readable_and_help_like_tokens_reach_the_real_body() {
    let journal = TestJournal::new();
    for arguments in [
        args(&["service", "logs"]),
        args(&["service", "logs", "--help", "ignored"]),
    ] {
        let output = run(journal.path().as_os_str(), &arguments);
        assert_eq!(output.status.code(), Some(0));
        assert_eq!(output.stdout, b"=== service.log === (not found)\n");
        assert!(output.stderr.is_empty());
    }

    journal.write(b"first\r\nsecond\rlast\xff");
    let output = run(journal.path().as_os_str(), &args(&["service", "logs"]));
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        output.stdout,
        "=== service.log ===\nfirst\nsecond\nlast�\n".as_bytes()
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn missing_journal_is_observed_read_only() {
    let outer = tempfile::tempdir().unwrap();
    let journal = outer.path().join("missing-journal");
    let output = run(journal.as_os_str(), &args(&["service", "logs"]));
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stdout, b"=== service.log === (not found)\n");
    assert!(output.stderr.is_empty());
    assert!(
        !journal.exists(),
        "service logs must not create the journal"
    );
}

#[test]
fn service_logs_slices_after_replacement_by_unicode_codepoint() {
    let journal = TestJournal::new();
    let mut bytes = "prefix".repeat(2_000).into_bytes();
    bytes.extend_from_slice("界".repeat(9_999).as_bytes());
    bytes.push(0xff);
    bytes.extend_from_slice("🙂".as_bytes());
    journal.write(&bytes);

    let output = run(journal.path().as_os_str(), &args(&["service", "logs"]));
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty());
    let rendered = String::from_utf8(output.stdout).expect("valid output");
    let payload = rendered
        .strip_prefix("=== service.log ===\n")
        .expect("fixed header")
        .strip_suffix('\n')
        .expect("retained print newline");
    assert_eq!(payload.chars().count(), 10_000);
    assert!(payload.starts_with('界'));
    assert!(payload.ends_with("�🙂"));
}

#[test]
fn one_shot_exact_byte_oracle_closes_decode_newline_cut_and_final_lf() {
    let mut boundary_input = b"Z".to_vec();
    boundary_input.extend(std::iter::repeat_n(b'a', 9_998));
    boundary_input.push(0xff);
    boundary_input.push(b'B');
    let mut boundary_expected = b"=== service.log ===\n".to_vec();
    boundary_expected.extend(std::iter::repeat_n(b'a', 9_998));
    boundary_expected.extend_from_slice("�B\n".as_bytes());

    let cases = [
        (
            b"left\r\nmid\rright\xff".to_vec(),
            "=== service.log ===\nleft\nmid\nright�\n"
                .as_bytes()
                .to_vec(),
        ),
        (
            vec![b'a'; 10_001],
            [
                b"=== service.log ===\n".as_slice(),
                &vec![b'a'; 10_000],
                b"\n",
            ]
            .concat(),
        ),
        (boundary_input, boundary_expected),
    ];
    for (input, expected) in cases {
        let journal = TestJournal::new();
        journal.write(&input);
        let output = run(journal.path().as_os_str(), &args(&["service", "logs"]));
        assert_eq!(output.status.code(), Some(0));
        assert_eq!(output.stdout, expected);
        assert!(output.stderr.is_empty());
    }
}

#[test]
fn readable_one_shot_stays_native_under_the_live_poison_denominator() {
    let root = tempfile::tempdir().unwrap();
    let poison = PoisonHarness::new(root.path());
    poison.prove_live();
    let journal = root.path().join("journal");
    fs::create_dir_all(journal.join("health")).unwrap();
    fs::write(journal.join("health/service.log"), b"native-readable\n").unwrap();
    let output = Command::new(&poison.core)
        .args(["service", "logs"])
        .env("SOLSTONE_JOURNAL", &journal)
        .env("PATH", &poison.bin)
        .env("POISON_MARKER", &poison.marker)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stdout, b"=== service.log ===\nnative-readable\n\n");
    assert!(output.stderr.is_empty());
    assert!(!poison.marker.exists());
}

#[test]
fn readable_and_dangling_symlinks_have_opposite_outcomes() {
    let journal = TestJournal::new();
    fs::create_dir_all(journal.path().join("health")).unwrap();
    let target = journal.path().join("target.log");
    fs::write(&target, "target\n").unwrap();
    symlink(&target, journal.service_log()).unwrap();

    let readable = run(journal.path().as_os_str(), &args(&["service", "logs"]));
    assert_eq!(readable.status.code(), Some(0));
    assert_eq!(readable.stdout, b"=== service.log ===\ntarget\n\n");

    fs::remove_file(journal.service_log()).unwrap();
    symlink("missing-target", journal.service_log()).unwrap();
    let dangling = run(journal.path().as_os_str(), &args(&["service", "logs"]));
    assert_eq!(dangling.status.code(), Some(0));
    assert_eq!(dangling.stdout, b"=== service.log === (not found)\n");
    assert!(dangling.stderr.is_empty());
}

#[test]
fn symlink_loop_is_metadata_uncertainty_not_missing() {
    let journal = TestJournal::new();
    fs::create_dir_all(journal.path().join("health")).unwrap();
    symlink("service.log", journal.service_log()).unwrap();

    let output = run(journal.path().as_os_str(), &args(&["service", "logs"]));
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.starts_with("service logs: metadata failed for "));
    assert_eq!(stderr.lines().count(), 1);
}

#[test]
fn one_shot_preserves_non_utf8_journal_identity_in_safe_diagnostics() {
    let outer = tempfile::tempdir().unwrap();
    #[cfg(target_os = "linux")]
    let journal = outer
        .path()
        .join(OsString::from_vec(b"journal-\\-\xff-\n-\x1b".to_vec()));
    #[cfg(target_os = "linux")]
    fs::create_dir_all(journal.join("health/service.log")).unwrap();
    #[cfg(target_os = "macos")]
    let blocked = outer.path().join("blocked");
    #[cfg(target_os = "macos")]
    fs::create_dir(&blocked).unwrap();
    #[cfg(target_os = "macos")]
    fs::set_permissions(&blocked, fs::Permissions::from_mode(0o000)).unwrap();
    #[cfg(target_os = "macos")]
    let journal = blocked.join(OsString::from_vec(b"journal-\\-\xff-\n-\x1b".to_vec()));

    let output = run(journal.as_os_str(), &args(&["service", "logs"]));
    #[cfg(target_os = "macos")]
    fs::set_permissions(&blocked, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(stderr.lines().count(), 1);
    #[cfg(target_os = "linux")]
    assert!(stderr.starts_with("service logs: read failed for "));
    #[cfg(target_os = "macos")]
    assert!(stderr.starts_with("service logs: metadata failed for "));
    assert!(stderr.contains("journal-\\\\-\\xff-\\n-\\x1b/health/service.log"));
    assert!(!stderr.contains('\u{1b}'));
}

#[test]
fn missing_unsafe_follow_path_takes_not_found_before_the_safety_gate() {
    let outer = tempfile::tempdir().unwrap();
    let journal = outer
        .path()
        .join(OsString::from_vec(b"missing-\\-\xff-\n-\x1b".to_vec()));
    let output = run(journal.as_os_str(), &args(&["service", "logs", "--follow"]));
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(output.stderr, b"No service log file found\n");
    assert!(!journal.exists());
}

#[test]
fn route_selectors_reject_non_utf8_at_their_owned_layers_but_trailing_bytes_run_one_shot() {
    let journal = TestJournal::new();
    let opaque = OsString::from_vec(vec![0xff]);
    let output = run(
        journal.path().as_os_str(),
        &[
            OsString::from("service"),
            OsString::from("logs"),
            opaque.clone(),
        ],
    );
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stdout, b"=== service.log === (not found)\n");

    let output = run(
        journal.path().as_os_str(),
        &[opaque.clone(), OsString::from("logs")],
    );
    assert_eq!(output.status.code(), Some(64));
    assert!(output.stdout.is_empty());
    assert!(output.stderr.starts_with(b"Usage:\n"));

    let output = run(
        journal.path().as_os_str(),
        &[OsString::from("service"), opaque],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(
        output.stderr,
        b"Unknown subcommand: \\xff; Available: install, uninstall, start, stop, restart, status, logs\n"
    );
}

#[test]
fn adjacent_service_lifecycle_routes_keep_their_native_contract() {
    let journal = TestJournal::new();
    let home = tempfile::tempdir().unwrap();
    for (arguments, expected_code, expected_stdout) in [
        (
            &["service"][..],
            1,
            solstone_core_cli::SERVICE_USAGE.as_bytes(),
        ),
        (
            &["service", "--help"][..],
            0,
            solstone_core_cli::SERVICE_USAGE.as_bytes(),
        ),
        (
            &["service", "status"][..],
            1,
            b"service: not installed\nrun 'journal setup' or 'journal service install' to install it.\n"
                .as_slice(),
        ),
    ] {
        let output = Command::new(BINARY)
            .args(arguments)
            .env("SOLSTONE_JOURNAL", journal.path())
            .env("HOME", home.path())
            .env("PATH", "/definitely-not-a-service-manager")
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(expected_code), "{arguments:?}");
        assert_eq!(output.stdout, expected_stdout, "{arguments:?}");
        assert!(output.stderr.is_empty(), "{arguments:?}");
    }
}

#[test]
fn resolver_failure_precedes_metadata_and_output() {
    let home = tempfile::tempdir().unwrap();
    let config = home.path().join(".config/solstone/config.toml");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(&config, b"\xff").unwrap();
    let output = Command::new(BINARY)
        .args(["service", "logs"])
        .env_remove("SOLSTONE_JOURNAL")
        .env("HOME", home.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(75));
    assert!(output.stdout.is_empty());
    assert_eq!(
        output.stderr,
        b"journal-path failed: config is not valid UTF-8\n"
    );
}

#[cfg(target_os = "linux")]
struct ChildGuard {
    child: Option<Child>,
    deadline: Instant,
}

#[cfg(target_os = "linux")]
impl ChildGuard {
    fn spawn(mut command: Command, deadline: Instant) -> Self {
        Self {
            child: Some(command.spawn().expect("spawn exact Core child")),
            deadline,
        }
    }

    fn pid(&self) -> u32 {
        self.child.as_ref().expect("live guard").id()
    }

    fn try_wait(&mut self) -> Option<ExitStatus> {
        self.child
            .as_mut()
            .expect("live guard")
            .try_wait()
            .expect("query child status")
    }

    fn disarm(mut self) {
        self.child.take();
    }

    fn cleanup(&mut self) -> Option<ExitStatus> {
        let child = self.child.as_mut()?;
        if let Some(status) = child.try_wait().expect("query child before cleanup") {
            return Some(status);
        }
        let pid = Pid::from_raw(i32::try_from(child.id()).expect("child pid fits i32"));
        let _ = kill(pid, Signal::SIGTERM);
        let term_deadline = (Instant::now() + Duration::from_millis(250)).min(self.deadline);
        while Instant::now() < term_deadline {
            if let Some(status) = child.try_wait().expect("query child after SIGTERM") {
                return Some(status);
            }
            thread::yield_now();
        }
        let _ = kill(pid, Signal::SIGKILL);
        while Instant::now() < self.deadline {
            if let Some(status) = child.try_wait().expect("query child after SIGKILL") {
                return Some(status);
            }
            thread::yield_now();
        }
        None
    }
}

#[cfg(target_os = "linux")]
impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.child.is_none() {
            return;
        }
        assert!(
            self.cleanup().is_some(),
            "exact child remained unreaped at the supplied cleanup deadline"
        );
    }
}

#[cfg(target_os = "linux")]
fn wait_until(deadline: Instant, mut condition: impl FnMut() -> bool) {
    while Instant::now() < deadline {
        if condition() {
            return;
        }
        thread::yield_now();
    }
    panic!("condition did not become true before the shared deadline");
}

#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
struct TraceGuard {
    pid: Pid,
    attached: bool,
}

#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
impl TraceGuard {
    fn detach(&mut self) {
        ptrace::detach(self.pid, None).expect("detach exact traced Core/tail PID");
        self.attached = false;
    }
}

#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
impl Drop for TraceGuard {
    fn drop(&mut self) {
        if self.attached {
            let _ = ptrace::detach(self.pid, None);
        }
    }
}

#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
fn wait_traced(pid: Pid, deadline: Instant) -> WaitStatus {
    loop {
        match waitpid(pid, Some(WaitPidFlag::WNOHANG | WaitPidFlag::WUNTRACED)) {
            Ok(WaitStatus::StillAlive) => {}
            Ok(status) => return status,
            Err(nix::errno::Errno::EINTR) => {}
            Err(error) => panic!("wait for traced PID {pid} failed: {error}"),
        }
        assert!(
            Instant::now() < deadline,
            "traced Core/tail PID did not reach the next event before the shared deadline"
        );
        thread::yield_now();
    }
}

#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
fn read_traced_c_string(pid: Pid, address: u64) -> Vec<u8> {
    let mut bytes = Vec::new();
    let word_size = std::mem::size_of::<nix::libc::c_long>();
    while bytes.len() <= 65_536 {
        let offset = u64::try_from(bytes.len()).expect("path length fits u64");
        let word = ptrace::read(pid, (address + offset) as ptrace::AddressType)
            .expect("read traced syscall path");
        for byte in word.to_ne_bytes() {
            if byte == 0 {
                return bytes;
            }
            bytes.push(byte);
        }
        debug_assert_eq!(bytes.len() % word_size, 0);
    }
    panic!("traced syscall path exceeded the bounded Linux path read");
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn traced_syscall(registers: &nix::libc::user_regs_struct) -> (u64, u64, u64) {
    (registers.orig_rax, registers.rsi, registers.rax)
}

#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
fn traced_syscall(registers: &nix::libc::user_regs_struct) -> (u64, u64, u64) {
    (registers.regs[8], registers.regs[1], registers.regs[0])
}

#[cfg(target_os = "linux")]
#[test]
fn follow_exec_replaces_the_real_core_pid_with_fixed_tail_and_has_no_child() {
    use std::os::unix::process::ExitStatusExt as _;

    let capture = tempfile::tempdir().unwrap();
    let journal = capture.path().join("-journal");
    let service_log = journal.join("health/service.log");
    fs::create_dir_all(service_log.parent().unwrap()).unwrap();
    fs::write(&service_log, b"service-tail-sentinel\n").unwrap();
    let poison = PoisonHarness::new(capture.path());
    poison.prove_live();
    let stdin_path = capture.path().join("stdin");
    let stdout_path = capture.path().join("stdout");
    let stderr_path = capture.path().join("stderr");
    fs::write(&stdin_path, b"inherited-stdin\n").unwrap();
    let stdin = fs::File::open(&stdin_path).unwrap();
    let stdout = fs::File::create(&stdout_path).unwrap();
    let stderr = fs::File::create(&stderr_path).unwrap();
    let mut command = Command::new(&poison.core);
    command
        .args(["service", "logs", "-f"])
        .current_dir(capture.path())
        .env("SOLSTONE_JOURNAL", "-journal")
        .env("LC_ALL", "C")
        .env("PATH", &poison.bin)
        .env("POISON_MARKER", &poison.marker)
        .stdin(stdin)
        .stdout(stdout)
        .stderr(stderr);
    let deadline = Instant::now() + Duration::from_secs(5);
    let work_deadline = deadline - Duration::from_secs(1);
    let mut guard = ChildGuard::spawn(command, deadline);
    let pid = guard.pid();
    let expected_tail = fs::canonicalize("/usr/bin/tail").unwrap();

    wait_until(work_deadline, || {
        fs::read_link(format!("/proc/{pid}/exe")).is_ok_and(|actual| actual == expected_tail)
    });
    wait_until(work_deadline, || {
        fs::read(&stdout_path).is_ok_and(|bytes| bytes == b"service-tail-sentinel\n")
    });
    let children = fs::read_to_string(format!("/proc/{pid}/task/{pid}/children")).unwrap();
    assert!(
        children.trim().is_empty(),
        "tail must have no retained child"
    );
    for (descriptor, expected) in [(0, &stdin_path), (1, &stdout_path), (2, &stderr_path)] {
        assert_eq!(
            fs::read_link(format!("/proc/{pid}/fd/{descriptor}")).unwrap(),
            *expected,
            "tail must inherit descriptor {descriptor} unchanged"
        );
    }
    assert!(guard.try_wait().is_none(), "tail must still be following");

    kill(Pid::from_raw(i32::try_from(pid).unwrap()), Signal::SIGTERM).unwrap();
    let status = loop {
        if let Some(status) = guard.try_wait() {
            break status;
        }
        assert!(
            Instant::now() < work_deadline,
            "SIGTERM termination timed out"
        );
        thread::yield_now();
    };
    assert_eq!(status.signal(), Some(15));
    assert!(!Path::new(&format!("/proc/{pid}")).exists());
    assert!(fs::read(&stderr_path).unwrap().is_empty());
    assert!(
        !poison.marker.exists(),
        "follow body invoked an interpreter"
    );
    guard.disarm();
}

#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
#[test]
fn metadata_then_missing_open_uses_the_real_exec_pid_and_fixed_tail_shape() {
    let journal = TestJournal::new();
    journal.write(b"present-before-race\n");
    let service_log = journal.service_log();
    let expected_path = service_log.as_os_str().as_encoded_bytes().to_vec();
    let capture = tempfile::tempdir().unwrap();
    let poison = PoisonHarness::new(capture.path());
    poison.prove_live();
    let stdout_path = capture.path().join("race-stdout");
    let stderr_path = capture.path().join("race-stderr");
    let gate_path = capture.path().join("trace-gate");
    let ready_path = capture.path().join("trace-ready");
    let launcher_path = capture.path().join("trace-launcher");
    mkfifo(&gate_path, Mode::S_IRUSR | Mode::S_IWUSR).unwrap();
    let mut gate = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&gate_path)
        .unwrap();
    fs::write(
        &launcher_path,
        b"#!/bin/sh\n: > \"$TRACE_READY\"\nIFS= read -r _ < \"$TRACE_GATE\"\nexec \"$TRACE_CORE\" service logs -f\n",
    )
    .unwrap();
    make_executable(&launcher_path);
    let stdout = fs::File::create(&stdout_path).unwrap();
    let stderr = fs::File::create(&stderr_path).unwrap();
    let mut command = Command::new(&launcher_path);
    command
        .env("SOLSTONE_JOURNAL", journal.path())
        .env("LC_ALL", "C")
        .env("PATH", &poison.bin)
        .env("POISON_MARKER", &poison.marker)
        .env("TRACE_CORE", &poison.core)
        .env("TRACE_GATE", &gate_path)
        .env("TRACE_READY", &ready_path)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr);
    let deadline = Instant::now() + Duration::from_secs(5);
    let work_deadline = deadline - Duration::from_secs(1);
    let mut child = ChildGuard::spawn(command, deadline);
    let pid = Pid::from_raw(i32::try_from(child.pid()).expect("child pid fits i32"));
    wait_until(work_deadline, || ready_path.exists());
    ptrace::attach(pid).unwrap();
    let mut trace = TraceGuard {
        pid,
        attached: true,
    };

    match wait_traced(pid, work_deadline) {
        WaitStatus::Stopped(actual, Signal::SIGSTOP) => assert_eq!(actual, pid),
        status => panic!("expected traced launcher stop, got {status:?}"),
    }
    ptrace::setoptions(
        pid,
        ptrace::Options::PTRACE_O_TRACESYSGOOD | ptrace::Options::PTRACE_O_TRACEEXEC,
    )
    .unwrap();
    gate.write_all(b"go\n").unwrap();
    ptrace::cont(pid, None).unwrap();

    match wait_traced(pid, work_deadline) {
        WaitStatus::PtraceEvent(actual, Signal::SIGTRAP, event) => {
            assert_eq!(actual, pid);
            assert_eq!(event, nix::libc::PTRACE_EVENT_EXEC);
        }
        status => panic!("expected exact Core exec event, got {status:?}"),
    }
    assert_eq!(
        fs::read_link(format!("/proc/{pid}/exe")).unwrap(),
        fs::canonicalize(&poison.core).unwrap()
    );
    // PTRACE_EVENT_EXEC precedes the execve syscall-exit stop. Consume that
    // one exit so the following alternating stops begin at a syscall entry on
    // both x86_64 and aarch64.
    ptrace::syscall(pid, None).unwrap();
    match wait_traced(pid, work_deadline) {
        WaitStatus::PtraceSyscall(actual) => assert_eq!(actual, pid),
        status => panic!("expected Core execve syscall exit, got {status:?}"),
    }
    ptrace::syscall(pid, None).unwrap();

    let mut target_stat_exit = false;
    let mut at_entry = true;
    loop {
        match wait_traced(pid, work_deadline) {
            WaitStatus::PtraceSyscall(actual) => {
                assert_eq!(actual, pid);
                let registers = ptrace::getregs(pid).unwrap();
                let (number, path_address, result) = traced_syscall(&registers);
                if at_entry
                    && number == nix::libc::SYS_statx as u64
                    && read_traced_c_string(pid, path_address) == expected_path
                {
                    target_stat_exit = true;
                } else if !at_entry && target_stat_exit {
                    assert_eq!(result, 0, "service-log metadata probe must succeed");
                    fs::remove_file(&service_log)
                        .expect("remove service log after successful production metadata probe");
                    ptrace::cont(pid, None).unwrap();
                    break;
                }
                at_entry = !at_entry;
                ptrace::syscall(pid, None).unwrap();
            }
            status => panic!("unexpected event while finding service-log statx: {status:?}"),
        }
    }

    match wait_traced(pid, work_deadline) {
        WaitStatus::PtraceEvent(actual, Signal::SIGTRAP, event) => {
            assert_eq!(actual, pid);
            assert_eq!(event, nix::libc::PTRACE_EVENT_EXEC);
        }
        status => panic!("expected fixed-tail exec event, got {status:?}"),
    }
    let expected_tail = fs::canonicalize("/usr/bin/tail").unwrap();
    assert_eq!(
        fs::read_link(format!("/proc/{pid}/exe")).unwrap(),
        expected_tail
    );
    let children = fs::read_to_string(format!("/proc/{pid}/task/{pid}/children")).unwrap();
    assert!(
        children.trim().is_empty(),
        "replaced tail must have no child"
    );
    assert!(!poison.marker.exists(), "race body invoked an interpreter");
    trace.detach();

    let status = loop {
        if let Some(status) = child.try_wait() {
            break status;
        }
        assert!(
            Instant::now() < work_deadline,
            "fixed tail did not exit before the shared deadline"
        );
        thread::yield_now();
    };
    assert_eq!(status.code(), Some(1));
    assert!(fs::read(&stdout_path).unwrap().is_empty());
    // `service_logs.rs` invokes `TAIL` by absolute path on purpose (no PATH lookup),
    // and GNU coreutils prefixes its diagnostics with argv[0] as given -- so on Linux
    // this reads "/usr/bin/tail: ..." while BSD tail, which uses getprogname(), reads
    // "tail: ...". Pinning the bare spelling made this test pass only on macOS.
    // Assert the part that is actually the contract: the two diagnostic lines and the
    // path they name.
    let stderr_text = String::from_utf8(fs::read(&stderr_path).unwrap()).expect("utf8 stderr");
    let lines: Vec<&str> = stderr_text.lines().collect();
    assert_eq!(lines.len(), 2, "stderr: {stderr_text}");
    assert!(
        lines[0].ends_with(&format!(
            "tail: cannot open '{}' for reading: No such file or directory",
            service_log.display()
        )),
        "stderr: {stderr_text}"
    );
    assert!(
        lines[1].ends_with("tail: no files remaining"),
        "stderr: {stderr_text}"
    );
    assert!(!Path::new(&format!("/proc/{pid}")).exists());
    assert!(!poison.marker.exists());
    child.disarm();
}

#[cfg(target_os = "linux")]
#[test]
fn cleanup_guard_drop_reaps_exited_term_and_kill_wrong_executables() {
    let deadline = Instant::now() + Duration::from_secs(5);
    let work_deadline = deadline - Duration::from_secs(1);
    let mut command = Command::new("/bin/true");
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let guard = ChildGuard::spawn(command, deadline);
    let pid = guard.pid();
    wait_until(work_deadline, || {
        fs::read_to_string(format!("/proc/{pid}/stat"))
            .is_ok_and(|stat| stat.split_ascii_whitespace().nth(2) == Some("Z"))
    });
    drop(guard);
    assert!(!Path::new(&format!("/proc/{pid}")).exists());

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut command = Command::new("/bin/sleep");
    command
        .arg("60")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let guard = ChildGuard::spawn(command, deadline);
    let pid = guard.pid();
    assert!(Path::new(&format!("/proc/{pid}")).exists());
    drop(guard);
    assert!(!Path::new(&format!("/proc/{pid}")).exists());

    let deadline = Instant::now() + Duration::from_secs(5);
    let work_deadline = deadline - Duration::from_secs(1);
    let ready = tempfile::NamedTempFile::new().unwrap();
    let ready_path = ready.path().to_owned();
    drop(ready);
    let mut command = Command::new("/bin/sh");
    command
        .args([
            "-c",
            "trap '' TERM; : > \"$KILL_READY\"; while :; do :; done",
        ])
        .env("KILL_READY", &ready_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let guard = ChildGuard::spawn(command, deadline);
    let pid = guard.pid();
    wait_until(work_deadline, || ready_path.exists());
    drop(guard);
    assert!(!Path::new(&format!("/proc/{pid}")).exists());
}
