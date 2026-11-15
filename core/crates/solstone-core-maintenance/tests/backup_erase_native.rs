// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! An erase that also removes the thing from every backup, end to end: a real
//! recording deleted and erased through the trash's own kinds, backups written
//! by the journal's own backup run to a loopback S3 bucket, the backup half
//! drained by the maintenance routine through the journal's restic helper, and
//! stock restic as the outside check. Synthetic journal only. Run explicitly:
//!
//! SOLSTONE_NATIVE_RESTIC=<restic> SOLSTONE_NATIVE_RCLONE=<rclone> \
//!   cargo test -p solstone-core-maintenance --test backup_erase_native -- --ignored --test-threads 1
//!
//! The helper, `restic-erase`, sits beside that restic, as in a payload.

use std::fs;
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use chrono::{Datelike, Utc};
use serde_json::{Value, json};
use solstone_core_backup::archive_admission::ArchiveCancellation;
use solstone_core_backup::{
    Destination, generate_and_store_keys, set_destination, set_enabled, set_recovery_key_confirmed,
};
use solstone_core_backup_runtime::hosted_runtime::{
    HttpError, HttpRequest, HttpResponse, HttpTransport,
};
use solstone_core_backup_runtime::{
    BackupServices, Clock, NativeJournalMaintenance, SystemToolRunner, UreqHttpTransport,
    init_repository, run_backup,
};
use solstone_core_removals::{Action, Page, Surface, Unit};

const DAY: &str = "20261101";
const STREAM: &str = "macbook";
const ERASED: &str = "090000_300";
const KEPT: &str = "091000_300";

struct WallClock;
impl Clock for WallClock {
    fn now_unix(&self) -> i64 {
        Utc::now().timestamp()
    }
    fn iso_week(&self) -> u8 {
        Utc::now().iso_week().week() as u8
    }
}

/// The loopback bucket can't answer S3's versioning and object-lock queries.
/// This answers them as a bucket configured `versioning`, read-only, and sends
/// everything else on.
struct BucketSays {
    versioning: &'static str,
}
impl HttpTransport for BucketSays {
    fn execute(&self, request: &HttpRequest) -> Result<HttpResponse, HttpError> {
        self.execute_cancellable(request, &ArchiveCancellation::new())
    }
    fn execute_cancellable(
        &self,
        request: &HttpRequest,
        cancellation: &ArchiveCancellation,
    ) -> Result<HttpResponse, HttpError> {
        let answer = |status: u16, body: String| {
            Ok(HttpResponse {
                status,
                headers: vec![],
                body: body.into_bytes(),
            })
        };
        if request.url.ends_with("?object-lock=") {
            return answer(
                404,
                "<Error><Code>ObjectLockConfigurationNotFoundError</Code></Error>".into(),
            );
        }
        if request.url.ends_with("?versioning=") {
            return answer(
                200,
                format!(
                    "<VersioningConfiguration><Status>{}</Status></VersioningConfiguration>",
                    self.versioning
                ),
            );
        }
        UreqHttpTransport.execute_cancellable(request, cancellation)
    }
}

struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
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
    tmp: tempfile::TempDir,
    journal: PathBuf,
    restic: PathBuf,
    repository: String,
    password: String,
}

fn segment_rel(key: &str) -> String {
    format!("chronicle/{DAY}/{STREAM}/{key}")
}

impl Fixture {
    fn new() -> Self {
        let restic = required("SOLSTONE_NATIVE_RESTIC");
        assert!(
            restic.with_file_name("restic-erase").is_file(),
            "restic-erase must sit beside {}",
            restic.display()
        );
        let rclone = required("SOLSTONE_NATIVE_RCLONE");
        let tmp = tempfile::Builder::new()
            .prefix("backup-erase-")
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
            let dir = journal.join(segment_rel(key));
            fs::create_dir_all(&dir).unwrap();
            fs::write(
                dir.join("audio.m4a"),
                format!("synthetic audio {key} ").repeat(4000),
            )
            .unwrap();
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
            tmp,
            journal,
            restic,
            repository,
            password: keys.daily_key,
        }
    }

    fn services(&self) -> BackupServices<'_> {
        self.services_with(&UreqHttpTransport)
    }

    fn services_with<'a>(&'a self, http: &'a dyn HttpTransport) -> BackupServices<'a> {
        BackupServices {
            runner: &SystemToolRunner,
            http,
            clock: &WallClock,
            restic: Some((&self.restic).into()),
            rclone_path: None,
            version: "backup-erase-fixture",
            journal_maintenance: &NativeJournalMaintenance,
        }
    }

    fn backup(&self) -> String {
        std::thread::sleep(Duration::from_millis(1100));
        let result = run_backup(&self.journal, &self.services());
        assert_eq!(result.status, "ok", "{result:?}");
        result.snapshot_id.unwrap()
    }

    fn restic_with(&self, program: &Path, args: &[&str]) -> std::process::Output {
        Command::new(program)
            .args(args)
            .env("RESTIC_REPOSITORY", &self.repository)
            .env("RESTIC_PASSWORD", &self.password)
            .env("AWS_ACCESS_KEY_ID", "fixture-access")
            .env("AWS_SECRET_ACCESS_KEY", "fixture-secret")
            .env("RESTIC_CACHE_DIR", self.tmp.path().join("cache"))
            .output()
            .unwrap()
    }

    fn restic(&self, args: &[&str]) -> String {
        let output = self.restic_with(&self.restic, args);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    fn snapshots(&self) -> Vec<Value> {
        serde_json::from_str(&self.restic(&["snapshots", "--json"])).unwrap()
    }

    /// Every data piece under `rel` in `snapshot`, read with stock restic.
    fn pieces(&self, snapshot: &str, rel: &str) -> Vec<String> {
        let root = self.journal.canonicalize().unwrap().join(rel);
        let tree: Value = serde_json::from_str(&self.restic(&[
            "cat",
            "tree",
            &format!("{snapshot}:{}", root.display()),
        ]))
        .unwrap();
        tree["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|node| node["content"].as_array().cloned().unwrap_or_default())
            .map(|id| id.as_str().unwrap().to_owned())
            .collect()
    }

    /// Stock restic lists `rel` in any snapshot.
    fn listed_anywhere(&self, rel: &str) -> bool {
        let root = self.journal.canonicalize().unwrap();
        self.snapshots().iter().any(|sn| {
            self.restic(&["ls", "--json", sn["id"].as_str().unwrap()])
                .lines()
                .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                .filter_map(|node| node["path"].as_str().map(str::to_owned))
                .any(|path| path == root.join(rel).to_string_lossy())
        })
    }

    /// The physical scan, through the helper's read-only command, of a list
    /// built independently above.
    fn stored_copies(&self, pieces: &[String]) -> u64 {
        let list = self.tmp.path().join("control.json");
        let targets: Vec<String> = pieces.iter().map(|id| format!("data:{id}")).collect();
        fs::write(&list, json!({"schema": 1, "targets": targets}).to_string()).unwrap();
        let helper = self.restic.with_file_name("restic-erase");
        let output = self.restic_with(
            &helper,
            &[
                "--no-cache",
                "scan-blobs",
                "--state",
                list.to_str().unwrap(),
            ],
        );
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["occurrences"]
            .as_u64()
            .unwrap()
    }
}

fn web() -> Surface {
    Surface::Web {
        page: Page::Settings,
    }
}

fn segment_unit(key: &str) -> Unit {
    Unit::Segment {
        day: DAY.into(),
        stream: STREAM.into(),
        segment: key.into(),
    }
}

/// Delete the recording to the trash through its own kind; returns the item id.
fn delete(journal: &Path, key: &str) -> String {
    solstone_core_import::trash_kinds::with_kinds(journal, |kinds| {
        let unit = segment_unit(key);
        let kind = kinds
            .iter()
            .find(|kind| kind.handles(&unit))
            .expect("a kind for recordings");
        solstone_core_trash::detach(
            journal,
            *kind,
            unit,
            Action::SegmentDelete,
            web(),
            vec![segment_rel(key)],
        )
        .expect("delete to the trash")
        .id
    })
}

fn erase(journal: &Path, id: &str, backups: bool) {
    solstone_core_import::trash_kinds::with_kinds(journal, |kinds| {
        solstone_core_trash::erase_with(journal, kinds, id, Action::Erase, web(), None, backups)
            .expect("erase")
    });
}

#[test]
#[ignore = "requires native restic, its erase helper and rclone; see the module doc"]
fn erase_with_backups_removes_it_from_every_backup_and_keeps_everything_else() {
    let fixture = Fixture::new();
    let journal = &fixture.journal;
    let before_delete = fixture.backup();
    let id = delete(journal, ERASED);
    assert!(journal.join(".trash").join(&id).exists());
    let in_trash = fixture.backup();
    let erased_pieces = {
        let mut pieces = fixture.pieces(&before_delete, &segment_rel(ERASED));
        pieces.extend(fixture.pieces(
            &in_trash,
            &format!(".trash/{id}/files/{}", segment_rel(ERASED)),
        ));
        pieces
    };
    let kept_pieces = fixture.pieces(&in_trash, &segment_rel(KEPT));
    assert!(
        !erased_pieces.is_empty() && fixture.stored_copies(&erased_pieces) > 0,
        "positive control"
    );

    // Erase now, with the choice ticked: the local erase is done at once, and
    // the entry is written for the backup half.
    erase(journal, &id, true);
    assert!(
        !journal.join(".trash").join(&id).exists(),
        "the local erase waits for nothing"
    );
    let (entries, _) = solstone_core_trash::backup_erase::pending(journal);
    assert_eq!(entries.len(), 1, "one entry for the item the act erased");
    assert_eq!(entries[0].id, id);

    let removes = BucketSays { versioning: "" };
    let report =
        solstone_core_offload::backup_erase::run(journal, &fixture.services_with(&removes));
    assert_eq!(report.state, "done", "{report:?}");
    assert_eq!(
        (report.removed, report.shared, report.staying),
        (1, 0, 0),
        "{report:?}"
    );
    assert!(
        solstone_core_trash::backup_erase::pending(journal)
            .0
            .is_empty()
    );

    // Stock restic: healthy, every backup kept, the erased recording at neither
    // path, the kept one restoring byte for byte.
    fixture.restic(&["check", "--read-data", "-q"]);
    assert_eq!(fixture.snapshots().len(), 2, "no backup was lost");
    assert!(!fixture.listed_anywhere(&format!("{}/audio.m4a", segment_rel(ERASED))));
    assert!(!fixture.listed_anywhere(&format!(".trash/{id}")));
    assert!(
        fixture.listed_anywhere(&format!("{}/tombstone.json", segment_rel(ERASED))),
        "the tombstone stays"
    );
    assert_eq!(
        fixture.stored_copies(&erased_pieces),
        0,
        "its stored pieces are gone"
    );
    assert!(fixture.stored_copies(&kept_pieces) > 0);
    let restored = fixture.tmp.path().join("restored");
    for sn in fixture.snapshots() {
        let target = restored.join(sn["id"].as_str().unwrap());
        fixture.restic(&[
            "restore",
            sn["id"].as_str().unwrap(),
            "--target",
            target.to_str().unwrap(),
            "-q",
        ]);
        let kept = target
            .join(journal.canonicalize().unwrap().strip_prefix("/").unwrap())
            .join(segment_rel(KEPT))
            .join("audio.m4a");
        assert_eq!(
            fs::read(&kept).unwrap(),
            fs::read(journal.join(segment_rel(KEPT)).join("audio.m4a")).unwrap()
        );
    }

    // The removal history says so.
    let history = solstone_core_removals::history(journal, 50).unwrap();
    assert!(
        format!("{history:?}").contains("BackupErase"),
        "the history holds the backup half"
    );
}

#[test]
#[ignore = "requires native restic, its erase helper and rclone; see the module doc"]
fn storage_that_keeps_versions_or_cant_say_is_never_called_done() {
    for (versioning, reason) in [(Some("Enabled"), "versions"), (None, "unknown")] {
        let fixture = Fixture::new();
        let journal = &fixture.journal;
        fixture.backup();
        let id = delete(journal, ERASED);
        fixture.backup();
        erase(journal, &id, true);
        let says = versioning.map(|versioning| BucketSays { versioning });
        let services = match &says {
            Some(says) => fixture.services_with(says),
            None => fixture.services(),
        };
        let report = solstone_core_offload::backup_erase::run(journal, &services);
        assert_eq!(
            (report.state.as_str(), report.reason.as_deref()),
            ("stays", Some(reason)),
            "{report:?}"
        );
        assert_eq!(report.staying, 1);
        // No backup can restore it all the same.
        assert!(!fixture.listed_anywhere(&format!(".trash/{id}")));
        let status = solstone_core_offload::backup_erase::status(journal);
        assert_eq!(
            (status.state.as_str(), status.reason.as_deref()),
            ("stays", Some(reason))
        );
        let history = format!(
            "{:?}",
            solstone_core_removals::history(journal, 50).unwrap()
        );
        assert!(history.contains("BackupEraseStays"), "{history}");
    }
}

#[test]
#[ignore = "requires native restic, its erase helper and rclone; see the module doc"]
fn erase_without_backups_writes_no_entry_and_the_backups_keep_it() {
    let fixture = Fixture::new();
    let journal = &fixture.journal;
    fixture.backup();
    let id = delete(journal, ERASED);
    fixture.backup();
    erase(journal, &id, false);
    assert!(
        solstone_core_trash::backup_erase::pending(journal)
            .0
            .is_empty()
    );
    let report = solstone_core_offload::backup_erase::run(journal, &fixture.services());
    assert_eq!(report.state, "nothing");
    assert!(
        fixture.listed_anywhere(&format!(".trash/{id}")),
        "unticked leaves the backups as they were"
    );
}

#[test]
#[ignore = "requires native restic, its erase helper and rclone; see the module doc"]
fn a_timed_erase_follows_the_setting_and_backup_off_writes_no_entry() {
    let fixture = Fixture::new();
    let journal = &fixture.journal;
    fixture.backup();
    let id = delete(journal, ERASED);
    fixture.backup();
    // The setting on: what the timer erases leaves the backups too.
    let config = journal.join("config/journal.json");
    let mut value: Value = serde_json::from_slice(&fs::read(&config).unwrap()).unwrap();
    value["trash"] = json!({"erase_from_backups": true, "erase_after_days": 1});
    fs::write(&config, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    let later = Utc::now() + chrono::Duration::days(31);
    solstone_core_import::trash_kinds::sweep(journal, later);
    let (entries, _) = solstone_core_trash::backup_erase::pending(journal);
    assert_eq!(
        entries.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
        [id.as_str()]
    );
    // Backup turned off: an erase writes nothing, and what was pending for the
    // destination that is gone is retired unreached.
    set_enabled(journal, false).unwrap();
    let other = delete(journal, KEPT);
    erase(journal, &other, true);
    assert_eq!(
        solstone_core_trash::backup_erase::pending(journal).0.len(),
        1,
        "no entry while backup is off"
    );
}

#[test]
#[ignore = "requires native restic, its erase helper and rclone; see the module doc"]
fn each_entry_stands_alone_and_nothing_unreadable_is_dropped() {
    let fixture = Fixture::new();
    let journal = &fixture.journal;
    fixture.backup();
    let first = delete(journal, ERASED);
    let second = delete(journal, KEPT);
    fixture.backup();
    erase(journal, &first, true);
    erase(journal, &second, true);
    let dir = journal.join("health/backup-erase");
    // The second waits on its own, not yet due; a torn entry sits beside them.
    let later = (Utc::now() + chrono::Duration::hours(3)).to_rfc3339();
    fs::write(
        dir.join("status.json"),
        serde_json::to_vec(&json!({
            "state": "failed", "pending": 2, "updated": Utc::now().to_rfc3339(),
            "held": {second.clone(): {"reason": "ledger", "attempts": 1, "next_try": later}},
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(dir.join("torn.json"), b"{").unwrap();

    let removes = BucketSays { versioning: "" };
    let report =
        solstone_core_offload::backup_erase::run(journal, &fixture.services_with(&removes));
    // The fixture's recordings share a small piece, so the first is removed
    // or removed-but-shared: either way it ran.
    assert_eq!(
        report.removed + report.shared,
        1,
        "the first ran while the second sat out: {report:?}"
    );
    let (left, torn) = solstone_core_trash::backup_erase::pending(journal);
    assert_eq!(
        left.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
        [second.as_str()]
    );
    assert_eq!(torn, ["torn.json"], "an entry that can't be read is kept");
    let status = solstone_core_offload::backup_erase::status(journal);
    assert_eq!(status.pending, 2, "{status:?}");
    assert!(status.held.contains_key(&second), "{status:?}");
    assert!(!fixture.listed_anywhere(&format!("{}/audio.m4a", segment_rel(ERASED))));
    assert!(
        fixture.listed_anywhere(&format!("{}/audio.m4a", segment_rel(KEPT))),
        "a held entry's thing is untouched"
    );

    // A destination other than the one set up now: retired unreached, with no
    // claim; the torn entry still waits and says so.
    let path = dir.join(format!("{second}.json"));
    let mut entry: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    entry["destination"] = json!("byo:s3:somewhere-else");
    fs::write(&path, serde_json::to_vec(&entry).unwrap()).unwrap();
    let report =
        solstone_core_offload::backup_erase::run(journal, &fixture.services_with(&removes));
    assert_eq!(report.unreached, 1, "{report:?}");
    assert!(
        solstone_core_trash::backup_erase::pending(journal)
            .0
            .is_empty()
    );
    let status = solstone_core_offload::backup_erase::status(journal);
    assert_eq!(
        (status.state.as_str(), status.reason.as_deref()),
        ("failed", Some("unreadable")),
        "{status:?}"
    );
    let results = fs::read_to_string(dir.join("results.jsonl")).unwrap();
    assert!(results.contains("not_reached"), "{results}");
    let history = format!(
        "{:?}",
        solstone_core_removals::history(journal, 50).unwrap()
    );
    assert_eq!(
        history.matches("BackupErase").count(),
        1,
        "no claim for the unreached one"
    );
}

/// Back up a copy of the journal from another place, as an earlier copy of it
/// would have, and record that place as where it was restored from.
fn earlier_copy(fixture: &Fixture, name: &str) -> PathBuf {
    let old = fixture.tmp.path().join(name);
    let status = Command::new("cp")
        .args(["-a"])
        .arg(&fixture.journal)
        .arg(&old)
        .status()
        .unwrap();
    assert!(status.success());
    let old = old.canonicalize().unwrap();
    fixture.restic(&["backup", old.to_str().unwrap(), "-q"]);
    std::thread::sleep(Duration::from_millis(1100));
    let at = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    std::thread::sleep(Duration::from_millis(1100));
    solstone_core_backup_runtime::erase_paths::record_origin(
        &fixture.journal,
        &"a".repeat(64),
        old.to_str().unwrap(),
        &at,
    )
    .unwrap();
    old
}

#[test]
#[ignore = "requires native restic, its erase helper and rclone; see the module doc"]
fn a_restored_journals_old_place_is_reached_only_when_gone_and_quiet() {
    let removes = BucketSays { versioning: "" };
    let old_holds = |fixture: &Fixture, old: &Path, key: &str| {
        let wanted = format!("{}/{}/audio.m4a", old.display(), segment_rel(key));
        fixture.snapshots().iter().any(|sn| {
            fixture
                .restic(&["ls", sn["id"].as_str().unwrap()])
                .lines()
                .any(|l| l == wanted)
        })
    };

    // Still on this device: left alone, and the entry says older backups hold it.
    let fixture = Fixture::new();
    let journal = &fixture.journal;
    let old = earlier_copy(&fixture, "old-journal");
    fixture.backup();
    let first = delete(journal, ERASED);
    fixture.backup();
    erase(journal, &first, true);
    let report =
        solstone_core_offload::backup_erase::run(journal, &fixture.services_with(&removes));
    assert_eq!(report.staying, 1, "{report:?}");
    assert!(
        old_holds(&fixture, &old, ERASED),
        "the old place is left alone"
    );
    let results = fs::read_to_string(journal.join("health/backup-erase/results.jsonl")).unwrap();
    assert!(results.contains("still_on_this_device"), "{results}");
    assert!(
        format!(
            "{:?}",
            solstone_core_removals::history(journal, 50).unwrap()
        )
        .contains("BackupErasePartly")
    );

    // Gone from this device and quiet since the restore: reached.
    fs::remove_dir_all(&old).unwrap();
    let second = delete(journal, KEPT);
    fixture.backup();
    erase(journal, &second, true);
    let report =
        solstone_core_offload::backup_erase::run(journal, &fixture.services_with(&removes));
    assert_eq!(report.removed + report.shared, 1, "{report:?}");
    assert!(
        !old_holds(&fixture, &old, KEPT),
        "the old place's backups are rewritten"
    );
    fixture.restic(&["check", "-q"]);

    // Backed up again since the restore: left alone.
    let fixture = Fixture::new();
    let journal = &fixture.journal;
    let old = earlier_copy(&fixture, "old-journal");
    fixture.restic(&["backup", old.to_str().unwrap(), "-q"]);
    fs::remove_dir_all(&old).unwrap();
    fixture.backup();
    let id = delete(journal, ERASED);
    fixture.backup();
    erase(journal, &id, true);
    let report =
        solstone_core_offload::backup_erase::run(journal, &fixture.services_with(&removes));
    assert_eq!(report.staying, 1, "{report:?}");
    assert!(old_holds(&fixture, &old, ERASED));
    let results = fs::read_to_string(journal.join("health/backup-erase/results.jsonl")).unwrap();
    assert!(results.contains("backed_up_since_the_restore"), "{results}");
}
