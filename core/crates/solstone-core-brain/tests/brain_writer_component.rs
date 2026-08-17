// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde_json::{Map, Value};
use solstone_core_brain::{
    BeginPrerequisiteRenewal, begin_prerequisite_renewal, begin_refresh,
    brain_fingerprint_key_path, brain_state_path, generate_fingerprint_key, hold_record_lock,
    inspect_brain_state, record_runtime_failure, validate_brain_state_record,
};

const BEGIN_PAUSE_JOURNAL: &str = "BRAIN_WRITER_BEGIN_PAUSE_JOURNAL";
const RENEWAL_PAUSE_JOURNAL: &str = "BRAIN_WRITER_RENEWAL_PAUSE_JOURNAL";
const RECORD_LOCK_PAUSE_JOURNAL: &str = "BRAIN_WRITER_RECORD_LOCK_PAUSE_JOURNAL";
const KEY_RACE_JOURNAL: &str = "BRAIN_WRITER_KEY_RACE_JOURNAL";

fn fixture() -> &'static Value {
    static FIXTURE: OnceLock<Value> = OnceLock::new();
    FIXTURE.get_or_init(|| {
        serde_json::from_str(include_str!("../../../fixtures/brain_projection.json"))
            .expect("brain projection fixture")
    })
}

fn fixture_now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(fixture()["now"].as_str().expect("fixture now"))
        .expect("fixture now")
        .with_timezone(&Utc)
}

struct TestJournal(PathBuf);

impl TestJournal {
    fn new() -> Self {
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "solstone-core-brain-writer-component-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("journal directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn write_config(&self, name: &str) {
        let config = fixture()["configs"].get(name).expect("fixture config");
        let path = self.path().join("config/journal.json");
        fs::create_dir_all(path.parent().expect("config parent")).unwrap();
        fs::write(path, serde_json::to_vec(config).unwrap()).unwrap();
    }

    fn write_fixture_key(&self) {
        let hex = fixture()["hmac_key_hex"].as_str().expect("hmac_key_hex");
        let key = (0..hex.len())
            .step_by(2)
            .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).unwrap())
            .collect::<Vec<_>>();
        let path = brain_fingerprint_key_path(self.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, key).unwrap();
    }

    fn seed_record(&self, name: &str) {
        let record = fixture()["records"].get(name).expect("fixture record");
        let path = brain_state_path(self.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, serde_json::to_vec(record).unwrap()).unwrap();
    }
}

impl Drop for TestJournal {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct ReapOnDrop(Option<Child>);

impl ReapOnDrop {
    fn spawn(name: &str, env_name: &str, journal: &Path) -> Self {
        let child = Command::new(std::env::current_exe().unwrap())
            .args(["--ignored", "--exact", name, "--nocapture"])
            .env(env_name, journal)
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn helper");
        Self(Some(child))
    }

    fn child(&mut self) -> &mut Child {
        self.0.as_mut().expect("child still held")
    }

    fn wait_ready(&mut self) {
        let mut line = String::new();
        let read = {
            let stdout = self.child().stdout.as_mut().expect("piped stdout");
            BufReader::new(stdout)
                .read_line(&mut line)
                .expect("read ready line")
        };
        if read == 0 {
            panic!("helper exited before ready: {:?}", self.child().try_wait());
        }
    }

    fn kill_wait(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    fn wait_with_output(mut self) -> std::process::Output {
        self.0
            .take()
            .expect("child still held")
            .wait_with_output()
            .unwrap()
    }
}

impl Drop for ReapOnDrop {
    fn drop(&mut self) {
        self.kill_wait();
    }
}

fn begin_cloud(journal: &TestJournal) -> solstone_core_brain::BrainRefreshPermit {
    journal.write_config("lane_byo_cloud");
    begin_refresh(
        journal.path(),
        fixture_now(),
        Some("run".to_owned()),
        None,
        false,
        None,
    )
    .unwrap()
    .expect("refresh permit")
}

fn record_bytes(journal: &TestJournal) -> Vec<u8> {
    fs::read(brain_state_path(journal.path())).unwrap()
}

fn signal_ready() {
    let mut stdout = std::io::stdout();
    writeln!(stdout, "ready").unwrap();
    stdout.flush().unwrap();
}

#[test]
#[ignore = "subprocess fixture for crash_after_begin_keeps_checking_record"]
fn begin_pause_helper() {
    let Ok(journal) = std::env::var(BEGIN_PAUSE_JOURNAL) else {
        return;
    };
    let permit = begin_refresh(
        Path::new(&journal),
        fixture_now(),
        Some("crash-run".to_owned()),
        None,
        false,
        None,
    )
    .unwrap()
    .expect("permit");
    signal_ready();
    let _permit = permit;
    thread::park();
}

#[test]
#[ignore = "subprocess fixture for refresh_contention_returns_no_permit_while_renewal_returns_busy"]
fn renewal_pause_helper() {
    let Ok(journal) = std::env::var(RENEWAL_PAUSE_JOURNAL) else {
        return;
    };
    let permit = match begin_prerequisite_renewal(
        Path::new(&journal),
        fixture_now(),
        Some("renewal-run".to_owned()),
        None,
        None,
    ) {
        BeginPrerequisiteRenewal::Started(permit) => permit,
        result => panic!("expected renewal permit, got {result:?}"),
    };
    signal_ready();
    let _permit = permit;
    thread::park();
}

#[test]
#[ignore = "subprocess fixture for record_writes_serialize_behind_a_real_process_lock"]
fn record_lock_pause_helper() {
    let Ok(journal) = std::env::var(RECORD_LOCK_PAUSE_JOURNAL) else {
        return;
    };
    let path = brain_state_path(Path::new(&journal));
    let _lock = hold_record_lock(&path).unwrap();
    signal_ready();
    thread::park();
}

#[test]
#[ignore = "subprocess fixture for key_generation_race_keeps_one_key"]
fn key_race_helper() {
    let Ok(journal) = std::env::var(KEY_RACE_JOURNAL) else {
        return;
    };
    let key = generate_fingerprint_key(Path::new(&journal)).unwrap();
    println!(
        "{}",
        key.iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
}

#[test]
fn crash_after_begin_keeps_checking_record() {
    let journal = TestJournal::new();
    journal.write_config("lane_byo_cloud");
    let mut child = ReapOnDrop::spawn("begin_pause_helper", BEGIN_PAUSE_JOURNAL, journal.path());
    child.wait_ready();
    child.kill_wait();

    let raw: Value = serde_json::from_slice(&record_bytes(&journal)).unwrap();
    assert!(
        raw["evidence"]
            .as_object()
            .unwrap()
            .values()
            .all(Value::is_null)
    );
    assert!(raw["checking"].is_object());
    validate_brain_state_record(&raw, fixture_now()).unwrap();
    let config = fixture()["configs"]["lane_byo_cloud"].as_object().unwrap();
    let inspection = inspect_brain_state(journal.path(), config, fixture_now());
    assert_eq!(
        inspection.projection.reason_code.as_deref(),
        Some("brain_check_interrupted")
    );
}

#[test]
fn refresh_contention_returns_no_permit_while_renewal_returns_busy() {
    let journal = TestJournal::new();
    journal.write_config("lane_byo_cloud");
    let mut child = ReapOnDrop::spawn("begin_pause_helper", BEGIN_PAUSE_JOURNAL, journal.path());
    child.wait_ready();
    assert!(
        begin_refresh(journal.path(), fixture_now(), None, None, false, None)
            .unwrap()
            .is_none()
    );
    child.kill_wait();

    let journal = TestJournal::new();
    journal.write_config("lane_spp");
    journal.write_fixture_key();
    journal.seed_record("lane_spp/ready");
    let mut child = ReapOnDrop::spawn(
        "renewal_pause_helper",
        RENEWAL_PAUSE_JOURNAL,
        journal.path(),
    );
    child.wait_ready();
    assert!(matches!(
        begin_prerequisite_renewal(journal.path(), fixture_now(), None, None, None),
        BeginPrerequisiteRenewal::Busy { .. }
    ));
    child.kill_wait();
}

#[test]
fn record_writes_serialize_behind_a_real_process_lock() {
    let journal = TestJournal::new();
    let permit = begin_cloud(&journal);
    let expected_fingerprint = permit.fingerprint_sha256.clone();
    drop(permit);

    let mut holder = ReapOnDrop::spawn(
        "record_lock_pause_helper",
        RECORD_LOCK_PAUSE_JOURNAL,
        journal.path(),
    );
    holder.wait_ready();

    let journal_path = journal.path().to_path_buf();
    let (sender, receiver) = mpsc::channel();
    let writer = thread::spawn(move || {
        let result = record_runtime_failure(
            &journal_path,
            "provider_unavailable",
            "generate",
            &expected_fingerprint,
            Map::new(),
            fixture_now(),
            None,
        );
        sender.send(result).unwrap();
    });

    // Mutual exclusion of a blocking flock is a liveness property; this deadline is the honest bound.
    assert_eq!(
        receiver.recv_timeout(Duration::from_secs(2)),
        Err(mpsc::RecvTimeoutError::Timeout)
    );
    holder.kill_wait();

    let result = receiver
        .recv()
        .expect("writer completes after record lock holder dies");
    writer.join().unwrap();
    assert!(result.accepted, "{result:?}");
    let record: Value = serde_json::from_slice(&record_bytes(&journal)).unwrap();
    validate_brain_state_record(&record, fixture_now()).unwrap();
    assert!(
        brain_state_path(journal.path())
            .with_extension("json.lock")
            .exists()
    );
}

#[test]
fn key_generation_race_keeps_one_key() {
    let journal = TestJournal::new();
    let left = ReapOnDrop::spawn("key_race_helper", KEY_RACE_JOURNAL, journal.path());
    let right = ReapOnDrop::spawn("key_race_helper", KEY_RACE_JOURNAL, journal.path());
    let left_out = left.wait_with_output();
    let right_out = right.wait_with_output();
    assert!(left_out.status.success(), "{left_out:?}");
    assert!(right_out.status.success(), "{right_out:?}");
    let disk = fs::read(brain_fingerprint_key_path(journal.path())).unwrap();
    let key = generate_fingerprint_key(journal.path()).unwrap();
    assert_eq!(disk.len(), key.len());
    assert_eq!(disk.len(), 32);
    let expected = disk
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    assert_eq!(String::from_utf8_lossy(&left_out.stdout).trim(), expected);
    assert_eq!(String::from_utf8_lossy(&right_out.stdout).trim(), expected);
}
