// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;
use solstone_core_journal_io::{JournalRoot, activity::ActivityLease};
use std::fs;
use std::os::unix::fs::MetadataExt;

const ROOT_ENV: &str = "SOLSTONE_RESTORE_LOCK_TEST_ROOT";
const FD_ENV: &str = "SOLSTONE_RESTORE_LOCK_TEST_FD";
const HELPER: &str = "bounded_child::tests::cooperation::payload_helper";
const COORDINATOR: &str = "bounded_child::tests::cooperation::coordinator_helper";

#[test]
#[ignore = "private child of the actual coordinator-loss control"]
fn payload_helper() {
    let root = std::path::PathBuf::from(std::env::var_os(ROOT_ENV).unwrap());
    let fd = std::env::var(FD_ENV).unwrap();
    let inherited = fs::metadata(format!("/proc/self/fd/{fd}")).unwrap();
    let named = fs::metadata(root.join("backup-activity.lock")).unwrap();
    assert_eq!(
        (inherited.dev(), inherited.ino()),
        (named.dev(), named.ino())
    );
    let ready = root.join("helper.ready.tmp");
    fs::write(&ready, std::process::id().to_string()).unwrap();
    fs::rename(ready, root.join("helper.ready")).unwrap();
    loop {
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
#[ignore = "private coordinator of the actual coordinator-loss control"]
fn coordinator_helper() {
    let path = std::path::PathBuf::from(std::env::var_os(ROOT_ENV).unwrap());
    let root = Arc::new(JournalRoot::open(&path).unwrap());
    let lease = ActivityLease::try_acquire(root).unwrap();
    let descriptor = lease.duplicate_descriptor_for_child().unwrap();
    let request = Request {
        program: std::env::current_exe().unwrap().into_os_string(),
        argv: ["--exact", HELPER, "--ignored", "--nocapture"]
            .map(OsString::from)
            .into(),
        env: BTreeMap::from([
            (ROOT_ENV.into(), path.into_os_string()),
            (FD_ENV.into(), descriptor.as_raw_fd().to_string().into()),
        ]),
        timeout: Some(Duration::from_secs(10)),
        pass_fds: vec![descriptor.as_fd()],
    };
    let output = run(&request).unwrap();
    panic!(
        "coordinator should have been killed while its child was alive: {}",
        output.returncode
    );
}

struct Coordinator(Child);
impl Drop for Coordinator {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn helper_stopped(pid: u32) -> bool {
    match fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => stat.rsplit_once(") ").unwrap().1.starts_with('Z'),
        Err(error)
            if error.kind() == io::ErrorKind::NotFound
                || error.raw_os_error() == Some(nix::libc::ESRCH) =>
        {
            true
        }
        Err(error) => panic!("read helper state: {error}"),
    }
}

#[test]
fn coordinator_sigkill_keeps_native_exclusion_until_actual_child_stops() {
    let temp = tempfile::tempdir().unwrap();
    let mut coordinator = Coordinator(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", COORDINATOR, "--ignored", "--nocapture"])
            .env(ROOT_ENV, temp.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    let pid = loop {
        if let Ok(pid) = fs::read_to_string(temp.path().join("helper.ready")) {
            break pid.parse::<u32>().unwrap();
        }
        assert!(
            coordinator.0.try_wait().unwrap().is_none(),
            "coordinator exited before ready"
        );
        assert!(
            Instant::now() < deadline,
            "actual helper did not become ready"
        );
        thread::sleep(Duration::from_millis(5));
    };
    assert!(!helper_stopped(pid));
    let root = Arc::new(JournalRoot::open(temp.path()).unwrap());
    assert!(matches!(
        ActivityLease::try_acquire(root.clone()),
        Err(solstone_core_journal_io::activity::ActivityAdmissionError::Busy)
    ));
    coordinator.0.kill().unwrap();
    let status = coordinator.0.wait().unwrap();
    assert_eq!(status.signal(), Some(nix::libc::SIGKILL));
    while !helper_stopped(pid) {
        assert!(
            Instant::now() < deadline,
            "helper remained live after coordinator loss"
        );
        thread::sleep(Duration::from_millis(5));
    }
    // The leader reads as a zombie before the helper's other threads drop the
    // inherited descriptor, so the kernel frees the lock a moment later.
    let lease = loop {
        match ActivityLease::try_acquire(root.clone()) {
            Ok(lease) => break lease,
            Err(solstone_core_journal_io::activity::ActivityAdmissionError::Busy) => {
                assert!(
                    Instant::now() < deadline,
                    "native exclusion remained after the helper stopped"
                );
                thread::sleep(Duration::from_millis(5));
            }
            Err(error) => panic!("activity admission after helper stop: {error:?}"),
        }
    };
    lease.revalidate().unwrap();
}
