// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! "Remove older backups" against real restic on a loopback S3 bucket, with a
//! synthetic journal. Run explicitly:
//!
//! SOLSTONE_NATIVE_RESTIC=<restic> SOLSTONE_NATIVE_RCLONE=<rclone> \
//!   cargo test -p solstone-core-offload --test older_backups_native -- --ignored --test-threads 1

use std::cell::Cell;
use std::fs;
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use chrono::{Datelike, Utc};
use serde_json::{Value, json};
use solstone_core_backup::{
    Destination, generate_and_store_keys, get_backup_config, set_destination, set_enabled,
    set_recovery_key_confirmed,
};
use solstone_core_backup_runtime::{
    BackupServices, Clock, NativeJournalMaintenance, SystemToolRunner, ToolOutput, ToolRequest,
    ToolRunner, UreqHttpTransport, init_repository, run_backup,
};
use solstone_core_offload::older_backups::{Refusal, plan, remove, view};
use solstone_core_removals::{Action, Event, OperationId, Policy, Removal, Surface, Unit};
use solstone_core_retention::archive_catalog::OffloadFile;

const DAY: &str = "20261101";
const ERASED: &str = "090000_300";
const KEPT: &str = "091000_300";
const ITEM: &str = "1f3c2a9e-trash-item";
const FAR: &str = "9999-12-31T00:00:00Z";

struct WallClock;
impl Clock for WallClock {
    fn now_unix(&self) -> i64 {
        Utc::now().timestamp()
    }
    fn iso_week(&self) -> u8 {
        Utc::now().iso_week().week() as u8
    }
}

struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Fails the `nth` forget it sees, once; everything else runs for real.
struct FailingForget {
    nth: usize,
    seen: Cell<usize>,
}
impl ToolRunner for FailingForget {
    fn run(&self, request: &ToolRequest<'_>) -> std::io::Result<ToolOutput> {
        if request.argv.first().and_then(|arg| arg.to_str()) == Some("forget") {
            let seen = self.seen.get() + 1;
            self.seen.set(seen);
            if seen == self.nth {
                return Ok(ToolOutput {
                    returncode: 11,
                    stdout: vec![],
                    stderr: b"repository is already locked".to_vec(),
                });
            }
        }
        SystemToolRunner.run(request)
    }
}

fn required(name: &str) -> PathBuf {
    PathBuf::from(std::env::var_os(name).unwrap_or_else(|| panic!("{name} is required")))
}

fn serve_s3(rclone: &Path, directory: &Path) -> (OwnedChild, String) {
    fs::create_dir_all(directory.join("fixture-bucket")).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let child = Command::new(rclone)
        .args(["serve", "s3"])
        .arg(directory)
        .args([
            "--no-cleanup",
            "--addr",
            &address.to_string(),
            "--config",
            "/dev/null",
        ])
        .env("RCLONE_AUTH_KEY", "\"fixture-access,fixture-secret\"")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("fixture S3 server");
    let owned = OwnedChild(child);
    let deadline = Instant::now() + Duration::from_secs(15);
    while TcpStream::connect_timeout(&address, Duration::from_millis(100)).is_err() {
        assert!(Instant::now() < deadline, "S3 fixture readiness");
        std::thread::sleep(Duration::from_millis(50));
    }
    (owned, format!("http://{address}"))
}

struct Fixture {
    _server: OwnedChild,
    _tmp: tempfile::TempDir,
    journal: PathBuf,
    restic: PathBuf,
    repository: String,
    password: String,
}

impl Fixture {
    fn new() -> Self {
        let restic = required("SOLSTONE_NATIVE_RESTIC");
        let rclone = required("SOLSTONE_NATIVE_RCLONE");
        let tmp = tempfile::Builder::new()
            .prefix("older-backups-")
            .tempdir_in(
                std::env::var_os("SOLSTONE_NATIVE_TMP")
                    .map_or_else(std::env::temp_dir, PathBuf::from),
            )
            .unwrap();
        let (server, endpoint) = serve_s3(&rclone, &tmp.path().join("s3"));
        let repository = format!("s3:{endpoint}/fixture-bucket/byo");
        let destination = Destination {
            repository: repository.clone(),
            backend: "s3".into(),
            credentials:
                json!({"access_key_id": "fixture-access", "secret_access_key": "fixture-secret"})
                    .as_object()
                    .unwrap()
                    .clone(),
        };
        let journal = tmp.path().join("journal");
        fs::create_dir_all(journal.join("config")).unwrap();
        for key in [ERASED, KEPT] {
            let dir = segment(&journal, key);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("audio.m4a"), format!("synthetic audio {key}")).unwrap();
            fs::write(
                dir.join("audio.jsonl"),
                format!("{{\"text\":\"synthetic words {key}\"}}\n"),
            )
            .unwrap();
        }
        set_destination(&journal, &destination).unwrap();
        let keys = generate_and_store_keys(&journal).unwrap();
        init_repository(
            &SystemToolRunner,
            &destination,
            &keys.daily_key,
            &keys.recovery_key,
            &restic,
            Some(Duration::from_secs(60)),
        )
        .expect("init");
        set_enabled(&journal, true).unwrap();
        set_recovery_key_confirmed(&journal, true).unwrap();
        Self {
            _server: server,
            _tmp: tmp,
            journal,
            restic,
            repository,
            password: keys.daily_key,
        }
    }

    fn services<'a>(&'a self, runner: &'a dyn ToolRunner) -> BackupServices<'a> {
        BackupServices {
            runner,
            http: &UreqHttpTransport,
            clock: &WallClock,
            restic: Some((&self.restic).into()),
            rclone_path: None,
            version: "older-backups-fixture",
            journal_maintenance: &NativeJournalMaintenance,
        }
    }

    fn backup(&self) -> String {
        std::thread::sleep(Duration::from_millis(1100));
        let result = run_backup(&self.journal, &self.services(&SystemToolRunner));
        assert_eq!(result.status, "ok", "{result:?}");
        result.snapshot_id.unwrap()
    }

    fn restic(&self, args: &[&str]) -> String {
        let output = Command::new(&self.restic)
            .args(args)
            .env("RESTIC_REPOSITORY", &self.repository)
            .env("RESTIC_PASSWORD", &self.password)
            .env("AWS_ACCESS_KEY_ID", "fixture-access")
            .env("AWS_SECRET_ACCESS_KEY", "fixture-secret")
            .env("RESTIC_CACHE_DIR", self._tmp.path().join("cache"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    /// An older backup of the journal as it is now, stamped `hours_ago`, with
    /// `tags`.
    fn older(&self, hours_ago: i64, tags: &[&str]) {
        let time = (chrono::Local::now() - chrono::Duration::hours(hours_ago))
            .format("%Y-%m-%d %H:%M:%S")
            .to_string();
        let path = self.journal.to_string_lossy().to_string();
        let mut args = vec!["backup", path.as_str(), "--time", time.as_str(), "-q"];
        for tag in tags {
            args.extend(["--tag", tag]);
        }
        self.restic(&args);
    }

    fn snapshots(&self) -> Vec<Value> {
        serde_json::from_str(&self.restic(&["snapshots", "--json"])).unwrap()
    }

    /// How many backups hold `rel` (a file under the journal).
    fn holders(&self, rel: &str) -> usize {
        let pattern = self.journal.join(rel).to_string_lossy().to_string();
        let out = self.restic(&["find", "--json", pattern.as_str()]);
        if out.trim().is_empty() {
            return 0;
        }
        serde_json::from_str::<Vec<Value>>(&out).unwrap().len()
    }

    /// The recording goes to the trash: its files move into `.trash/<id>`,
    /// a tombstone stays where they were.
    fn trash(&self) {
        let from = segment(&self.journal, ERASED);
        let to = self
            .journal
            .join(".trash")
            .join(ITEM)
            .join("files")
            .join("chronicle")
            .join(DAY)
            .join("macbook")
            .join(ERASED);
        fs::create_dir_all(to.parent().unwrap()).unwrap();
        fs::rename(&from, &to).unwrap();
        fs::create_dir_all(&from).unwrap();
        fs::write(
            from.join("tombstone.json"),
            serde_json::to_vec(&json!({"deleted_at": Utc::now().to_rfc3339(), "cid": ITEM, "reason": "owner_segment_delete",
                "manifest": [format!("chronicle/{DAY}/macbook/{ERASED}/audio.m4a"), format!("chronicle/{DAY}/macbook/{ERASED}/audio.jsonl")]}))
            .unwrap(),
        )
        .unwrap();
    }

    /// The trash erases it: the files go, then the record is written.
    fn erase(&self, unlink: bool) {
        if unlink {
            fs::remove_dir_all(self.journal.join(".trash").join(ITEM)).unwrap();
        }
        Removal::with_id(
            &self.journal,
            OperationId::durable(Utc::now(), ITEM),
            Action::Erase,
            Surface::Policy {
                policy: Policy::Trash,
            },
        )
        .record_event(
            Event::Erased,
            Unit::Segment {
                day: DAY.into(),
                stream: "macbook".into(),
                segment: ERASED.into(),
            },
        );
    }

    fn state(&self) -> Value {
        view(&self.journal, Utc::now().timestamp(), false)
    }

    fn after_backup(&self) {
        solstone_core_offload::older_backups::survey_if_pending(
            &self.journal,
            &self.services(&SystemToolRunner),
        );
    }
}

fn segment(journal: &Path, key: &str) -> PathBuf {
    journal
        .join("chronicle")
        .join(DAY)
        .join("macbook")
        .join(key)
}

fn erased_rel() -> String {
    format!("chronicle/{DAY}/macbook/{ERASED}/audio.m4a")
}

fn trash_rel() -> String {
    format!(".trash/{ITEM}/files/chronicle/{DAY}/macbook/{ERASED}/audio.m4a")
}

fn kept_rel() -> String {
    format!("chronicle/{DAY}/macbook/{KEPT}/audio.m4a")
}

fn web() -> Surface {
    Surface::Web {
        page: solstone_core_removals::Page::Backup,
    }
}

#[test]
#[ignore = "requires native restic and rclone; see the module doc"]
fn erase_back_up_remove_older_then_nothing_holds_it_at_either_path() {
    let fixture = Fixture::new();
    fixture.older(72, &[]);
    fixture.older(48, &[]);
    fixture.backup();
    fixture.trash();
    fixture.backup();
    assert!(
        fixture.holders(&erased_rel()) >= 3,
        "control: older backups hold it where it was"
    );
    assert_eq!(
        fixture.holders(&trash_rel()),
        1,
        "control: one backup holds it in the trash"
    );
    fixture.erase(true);
    assert_eq!(fixture.state()["state"], "waiting");
    let newest = fixture.backup();
    fixture.after_backup();
    let ready = fixture.state();
    assert_eq!(ready["state"], "ready", "{ready}");
    assert_eq!(ready["older"], 4);

    let done = remove(
        &fixture.journal,
        &fixture.services(&SystemToolRunner),
        false,
        None,
        web(),
        &|_, _| {},
    )
    .unwrap();
    assert_eq!(done.removed, 4);
    let left = fixture.snapshots();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0]["id"], newest);
    assert_eq!(fixture.holders(&erased_rel()), 0);
    assert_eq!(fixture.holders(&trash_rel()), 0);
    assert_eq!(
        fixture.holders(&kept_rel()),
        1,
        "what the owner still has stays backed up"
    );
    let shown = fixture.state();
    assert_eq!(shown["state"], "done", "{shown}");
    let history = solstone_core_removals::history(&fixture.journal, 10).unwrap();
    assert!(
        history
            .entries
            .iter()
            .any(|entry| entry.action == Action::OlderBackupsRemove)
    );
    assert_eq!(
        remove(
            &fixture.journal,
            &fixture.services(&SystemToolRunner),
            false,
            None,
            web(),
            &|_, _| {}
        )
        .err(),
        Some(Refusal::Waiting),
        "settled: nothing is offered again"
    );
}

#[test]
#[ignore = "requires native restic and rclone; see the module doc"]
fn a_newest_backup_that_still_holds_it_is_refused_and_nothing_is_removed() {
    let fixture = Fixture::new();
    fixture.older(48, &[]);
    fixture.trash();
    fixture.backup();
    // The record says erased, but the files are still in the trash folder
    // when the next backup runs: the newest holds it at its trash path.
    fixture.erase(false);
    fixture.backup();
    fixture.after_backup();
    // The erase then finishes here: the newest backup holds what's gone.
    fs::remove_dir_all(fixture.journal.join(".trash").join(ITEM)).unwrap();
    let before = fixture.snapshots().len();
    let refused = remove(
        &fixture.journal,
        &fixture.services(&SystemToolRunner),
        true,
        Some(FAR),
        web(),
        &|_, _| {},
    );
    assert_eq!(refused.err(), Some(Refusal::NewestHoldsErased));
    assert_eq!(fixture.snapshots().len(), before, "nothing forgotten");
}

#[test]
#[ignore = "requires native restic and rclone; see the module doc"]
fn backups_kept_from_before_a_restore_go_only_when_ticked() {
    let fixture = Fixture::new();
    fixture.older(400, &["solstone-kept-before-restore"]);
    fixture.older(300, &["solstone-kept-before-restore"]);
    fixture.older(48, &[]);
    fixture.trash();
    fixture.backup();
    fixture.erase(true);
    fixture.backup();
    fixture.after_backup();
    assert_eq!(fixture.state()["kept_before_restore"], 2);
    let services = fixture.services(&SystemToolRunner);
    let unticked = remove(&fixture.journal, &services, false, None, web(), &|_, _| {}).unwrap();
    assert_eq!(unticked.kept_before_restore_left, 2);
    let kept: Vec<Value> = fixture
        .snapshots()
        .into_iter()
        .filter(|snapshot| {
            snapshot["tags"]
                .as_array()
                .is_some_and(|tags| tags.contains(&json!("solstone-kept-before-restore")))
        })
        .collect();
    assert_eq!(
        kept.len(),
        2,
        "unticked: the kept-from-restore backups stay"
    );
    let shown = fixture.state();
    assert_eq!(
        (
            shown["state"].as_str(),
            shown["kept_before_restore"].as_u64()
        ),
        (Some("partly_done"), Some(2))
    );
    // Partly done's "remove them" takes them, with no new erase.
    let ticked = remove(
        &fixture.journal,
        &services,
        true,
        Some(FAR),
        web(),
        &|_, _| {},
    )
    .unwrap();
    assert_eq!(ticked.kept_before_restore_left, 0);
    assert_eq!(
        fixture.snapshots().len(),
        1,
        "ticked: only the newest is left"
    );
    assert_eq!(fixture.state()["state"], "done");
}

#[test]
#[ignore = "requires native restic and rclone; see the module doc"]
fn a_stop_part_way_reports_what_it_removed_and_the_next_press_removes_the_rest() {
    let fixture = Fixture::new();
    for hours in 20..34 {
        fixture.older(hours, &[]);
    }
    fixture.trash();
    fixture.backup();
    fixture.erase(true);
    let newest = fixture.backup();
    fixture.after_backup();
    let failing = FailingForget {
        nth: 2,
        seen: Cell::new(0),
    };
    let stopped = remove(
        &fixture.journal,
        &fixture.services(&failing),
        false,
        None,
        web(),
        &|_, _| {},
    );
    let Some(Refusal::Stopped { removed, total, .. }) = stopped.err() else {
        panic!("expected a stop part way");
    };
    assert_eq!((removed, total), (10, 15));
    let shown = fixture.state();
    assert_eq!(
        (shown["state"].as_str(), shown["removed"].as_u64()),
        (Some("stopped"), Some(10))
    );
    assert_eq!(fixture.snapshots().len(), 6);
    let done = remove(
        &fixture.journal,
        &fixture.services(&SystemToolRunner),
        false,
        None,
        web(),
        &|_, _| {},
    )
    .unwrap();
    assert_eq!(done.removed, 15, "the first attempt's 10 count too");
    let left = fixture.snapshots();
    assert_eq!(
        (left.len(), left[0]["id"].as_str()),
        (1, Some(newest.as_str()))
    );
    assert_eq!(fixture.holders(&erased_rel()), 0);
}

#[test]
#[ignore = "requires native restic and rclone; see the module doc"]
fn an_archive_of_only_erased_media_goes_and_one_with_kept_media_stays() {
    let fixture = Fixture::new();
    // Two offload archives, one per recording, each witnessed in the ledger.
    let stage = fixture._tmp.path().join("stage");
    let mut archive_ids = Vec::new();
    for key in [ERASED, KEPT] {
        let dir = stage.join(key);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("raw.flac"), format!("synthetic original {key}")).unwrap();
        let out = fixture.restic(&[
            "backup",
            dir.to_str().unwrap(),
            "--tag",
            "spb-offload-archive",
            "--json",
        ]);
        let id = out
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .find_map(|line| line["snapshot_id"].as_str().map(str::to_owned))
            .unwrap();
        solstone_core_offload::ledger::append_offload_event(
            &fixture.journal,
            DAY,
            "macbook",
            key,
            &id,
            &[OffloadFile {
                name: "raw.flac".into(),
                bytes: 20,
                sha256: "a".repeat(64),
            }],
            1,
        )
        .unwrap();
        archive_ids.push(id);
    }
    fixture.older(48, &[]);
    fixture.trash();
    fixture.backup();
    fixture.erase(true);
    fixture.backup();
    fixture.after_backup();
    let ready = fixture.state();
    assert_eq!(
        (
            ready["archives_removable"].as_u64(),
            ready["archives_kept"].as_u64()
        ),
        (Some(1), Some(1)),
        "{ready}"
    );
    let plan = plan(
        &fixture.journal,
        &fixture.services(&SystemToolRunner),
        false,
        None,
    )
    .unwrap();
    assert_eq!(plan.archives_removable[0].id, archive_ids[0]);
    let done = remove(
        &fixture.journal,
        &fixture.services(&SystemToolRunner),
        false,
        None,
        web(),
        &|_, _| {},
    )
    .unwrap();
    assert_eq!(done.archives_removed, 1);
    let left: Vec<String> = fixture
        .snapshots()
        .iter()
        .map(|s| s["id"].as_str().unwrap().to_owned())
        .collect();
    assert!(
        !left.contains(&archive_ids[0]),
        "the erased recording's archive went"
    );
    assert!(
        left.contains(&archive_ids[1]),
        "the kept recording's archive stayed"
    );
    let backup = get_backup_config(&fixture.journal).unwrap();
    assert!(backup.get("older_backups").is_some());
}
