// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(target_os = "linux")]

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use solstone_core_installation_identity::{
    ArtifactBindingEvidence, JournalToken, LegacyManifestEvidence, OwnerBase, PlatformTag,
    RootToken, SetupAdmissionRequest, admit_installation_binding, admit_setup,
};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);
const ROOT_TOKEN: &str = "/installation/cross-process";
const FIRST_JOURNAL: &str = "/journal/cross-process-one";
const SECOND_JOURNAL: &str = "/journal/cross-process-two";

struct Fixture {
    root: PathBuf,
    home: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = env::temp_dir().join(format!(
            "solstone-installation-admission-process-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).expect("create fixture root");
        let home = root.join("home");
        fs::create_dir(&home).expect("create fixture home");
        let request = request(&home, FIRST_JOURNAL);
        drop(admit_setup(request).expect("seed adopted binding"));
        Self { root, home }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn request(home: &Path, journal: &str) -> SetupAdmissionRequest {
    SetupAdmissionRequest {
        owner: OwnerBase::at_home(home.to_path_buf(), PlatformTag::Linux).expect("owner"),
        root_token: RootToken::from_raw_absolute(ROOT_TOKEN.as_bytes().to_vec())
            .expect("root token"),
        journal_token: JournalToken::from_raw_absolute(journal.as_bytes().to_vec())
            .expect("journal token"),
        journal_is_explicit: true,
        legacy_manifest: LegacyManifestEvidence::Absent,
        artifacts: ArtifactBindingEvidence::Fresh,
    }
}

struct RoleProcess {
    child: Child,
    started: PathBuf,
    ready: PathBuf,
    release: PathBuf,
}

impl RoleProcess {
    fn spawn(fixture: &Fixture, label: &str, role: &str) -> Self {
        let started = fixture.path(&format!("{label}.started"));
        let ready = fixture.path(&format!("{label}.ready"));
        let release = fixture.path(&format!("{label}.release"));
        let child = Command::new(env::current_exe().expect("current test executable"))
            .args([
                "--exact",
                "linux_admission_child",
                "--ignored",
                "--nocapture",
            ])
            .env("SOLSTONE_ADMISSION_CHILD_ROLE", role)
            .env("SOLSTONE_ADMISSION_CHILD_HOME", &fixture.home)
            .env("SOLSTONE_ADMISSION_CHILD_STARTED", &started)
            .env("SOLSTONE_ADMISSION_CHILD_READY", &ready)
            .env("SOLSTONE_ADMISSION_CHILD_RELEASE", &release)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn role process");
        Self {
            child,
            started,
            ready,
            release,
        }
    }

    fn wait_started(&mut self) {
        wait_for_path(&mut self.child, &self.started, "child start");
    }

    fn wait_ready(&mut self) {
        wait_for_path(&mut self.child, &self.ready, "child admission");
    }

    fn assert_not_ready(&mut self) {
        let deadline = Instant::now() + Duration::from_millis(150);
        while Instant::now() < deadline {
            assert!(
                !self.ready.exists(),
                "child unexpectedly acquired admission"
            );
            assert!(
                self.child.try_wait().expect("inspect child").is_none(),
                "child exited before acquiring admission"
            );
            thread::yield_now();
        }
    }

    fn release_and_wait(mut self) {
        fs::write(&self.release, b"release\n").expect("release child");
        let status = self.child.wait().expect("wait for child");
        assert!(status.success(), "child failed: {status}");
    }
}

fn wait_for_path(child: &mut Child, path: &Path, label: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !path.exists() {
        assert!(Instant::now() < deadline, "timed out waiting for {label}");
        assert!(
            child.try_wait().expect("inspect child").is_none(),
            "child exited while waiting for {label}"
        );
        thread::yield_now();
    }
}

#[test]
fn cross_process_readers_coexist_and_writers_are_reciprocally_exclusive() {
    let fixture = Fixture::new();
    let mut provider_entries_before = fs::read_dir(
        OwnerBase::at_home(fixture.home.clone(), PlatformTag::Linux)
            .expect("owner")
            .path(),
    )
    .expect("read provider")
    .map(|entry| entry.expect("provider entry").file_name())
    .collect::<Vec<_>>();
    provider_entries_before.sort();

    let mut first_reader = RoleProcess::spawn(&fixture, "reader-one", "reader");
    first_reader.wait_started();
    first_reader.wait_ready();
    let mut second_reader = RoleProcess::spawn(&fixture, "reader-two", "reader");
    second_reader.wait_started();
    second_reader.wait_ready();

    let mut writer = RoleProcess::spawn(&fixture, "writer", "writer");
    writer.wait_started();
    writer.assert_not_ready();
    first_reader.release_and_wait();
    writer.assert_not_ready();
    second_reader.release_and_wait();
    writer.wait_ready();

    let mut late_reader = RoleProcess::spawn(&fixture, "reader-late", "reader");
    late_reader.wait_started();
    late_reader.assert_not_ready();
    writer.release_and_wait();
    late_reader.wait_ready();
    late_reader.release_and_wait();

    let mut provider_entries_after = fs::read_dir(
        OwnerBase::at_home(fixture.home.clone(), PlatformTag::Linux)
            .expect("owner")
            .path(),
    )
    .expect("read provider")
    .map(|entry| entry.expect("provider entry").file_name())
    .collect::<Vec<_>>();
    provider_entries_after.sort();
    assert_eq!(provider_entries_after, provider_entries_before);
}

#[test]
#[ignore = "subprocess helper"]
fn linux_admission_child() {
    let Ok(role) = env::var("SOLSTONE_ADMISSION_CHILD_ROLE") else {
        return;
    };
    let home = PathBuf::from(env::var_os("SOLSTONE_ADMISSION_CHILD_HOME").expect("child home"));
    let started =
        PathBuf::from(env::var_os("SOLSTONE_ADMISSION_CHILD_STARTED").expect("child started path"));
    let ready =
        PathBuf::from(env::var_os("SOLSTONE_ADMISSION_CHILD_READY").expect("child ready path"));
    let release =
        PathBuf::from(env::var_os("SOLSTONE_ADMISSION_CHILD_RELEASE").expect("child release path"));
    fs::write(&started, b"started\n").expect("publish child start");

    let owner = OwnerBase::at_home(home.clone(), PlatformTag::Linux).expect("child owner");
    let root =
        RootToken::from_raw_absolute(ROOT_TOKEN.as_bytes().to_vec()).expect("child root token");
    match role.as_str() {
        "reader" => {
            let admission =
                admit_installation_binding(&owner, &root).expect("child reader admission");
            fs::write(&ready, b"ready\n").expect("publish reader admission");
            wait_for_release(&release);
            drop(admission);
        }
        "writer" => {
            let admission =
                admit_setup(request(&home, SECOND_JOURNAL)).expect("child writer admission");
            fs::write(&ready, b"ready\n").expect("publish writer admission");
            wait_for_release(&release);
            drop(admission);
        }
        other => panic!("unknown child role {other}"),
    }
}

fn wait_for_release(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !path.exists() {
        assert!(Instant::now() < deadline, "timed out waiting for release");
        thread::yield_now();
    }
}
