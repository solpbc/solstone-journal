// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Real Linux C-archive effects through the same finite reader. This isolated
//! feature qualifies host ownership/effects, never iOS startup/container authority.

use sha2::{Digest, Sha256};
use solstone_core_backup::{
    Destination,
    archive_activity::{ArchivePreparationCustody, OwnedArchivePreparationInput},
    archive_admission::{ArchiveBudget, ArchiveCancellation, Clock},
    archive_readback::{
        HistoricalArchiveFile, OwnedReadOperation, ReadbackError, ios::IosReadContext,
    },
    ios_native::{Call, Options},
};
use solstone_core_journal_io::{
    JournalRoot,
    media::{Cleanup, MediaDirectory, MediaStage, OwnedMediaTarget},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    os::fd::AsFd,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

struct FixedClock;
impl Clock for FixedClock {
    fn now_unix(&self) -> i64 {
        100
    }
    fn iso_week(&self) -> u8 {
        1
    }
}
fn options() -> Options {
    Options {
        version: 1,
        check_read_mode: 0,
        retry_lock_ms: 0,
        group: 0,
        groups: 0,
        reserved1: 0,
        reserved2: 0,
    }
}
fn completed(call: Result<Call, i32>) -> solstone_core_backup::ios_native::Completed {
    let result = call
        .unwrap_or_else(|status| panic!("native entry {status}"))
        .wait(Some(Duration::from_secs(30)), None, true);
    assert_eq!(result.fields.status, 0, "actual native completion");
    result
}
fn id(bytes: &[u8; 65]) -> String {
    assert_eq!(bytes[64], 0);
    std::str::from_utf8(&bytes[..64]).unwrap().to_owned()
}
struct Fixture {
    _dir: tempfile::TempDir,
    root: Arc<JournalRoot>,
    repo: PathBuf,
    destination: Destination,
    payload: Vec<u8>,
    snapshots: Vec<String>,
    cancel_in_wait: Option<(ArchiveCancellation, Arc<AtomicBool>)>,
}
impl Fixture {
    fn new(count: usize) -> Self {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("config")).unwrap();
        let repo = dir.path().join("repository");
        fs::create_dir(&repo).unwrap();
        let source = dir.path().join("source");
        fs::create_dir(&source).unwrap();
        let payload = b"synthetic retained archive bytes".to_vec();
        fs::write(source.join("clip.m4a"), &payload).unwrap();
        completed(Call::local(
            1,
            b"daily",
            &[],
            repo.to_str().unwrap(),
            None,
            None,
            &[],
            &options(),
        ));
        let held = fs::File::open(&source).unwrap();
        let mut snapshots = Vec::new();
        for _ in 0..count {
            let result = completed(Call::archive_local(
                b"daily",
                repo.to_str().unwrap(),
                "archive",
                held.as_fd(),
                br#"["clip.m4a"]"#,
            ));
            snapshots.push(id(&result.fields.snapshot_id));
        }
        assert_eq!(snapshots.iter().collect::<BTreeSet<_>>().len(), count);
        let destination = Destination {
            repository: repo.to_str().unwrap().into(),
            backend: "s3".into(),
            credentials: serde_json::Map::new(),
        };
        let config = serde_json::json!({"backup":{"enabled":true,"mode":"byo","daily_key":"daily","recovery_key":"0123456789ABCDEFGHJKMNPQRSTVWXYZ0123456789ABCDEFGHJKMNPQRSTVWXYZ","destination":{"repository":destination.repository,"backend":"s3","credentials":{}}}});
        fs::write(
            dir.path().join("config/journal.json"),
            serde_json::to_vec(&config).unwrap(),
        )
        .unwrap();
        let root = Arc::new(JournalRoot::open(dir.path()).unwrap());
        Self {
            _dir: dir,
            root,
            repo,
            destination,
            payload,
            snapshots,
            cancel_in_wait: None,
        }
    }
    fn read(
        &self,
        expected: Option<String>,
        password: &str,
    ) -> Result<(OwnedReadOperation, ArchivePreparationCustody), ReadbackError> {
        let clock: Arc<dyn Clock + Send + Sync> = Arc::new(FixedClock);
        let budget = match &self.cancel_in_wait {
            Some((cancel, _)) => ArchiveBudget::new_cancellable(clock.as_ref(), cancel.clone()),
            None => ArchiveBudget::new(clock.as_ref()),
        }
        .unwrap();
        let (input, custody) =
            OwnedArchivePreparationInput::capture(Arc::clone(&self.root), budget, clock)
                .unwrap_or_else(|_| panic!("capture"));
        let prepared = input.prepare().unwrap_or_else(|_| panic!("prepare"));
        let check_root = Arc::clone(&self.root);
        let cancel_in_wait = self.cancel_in_wait.clone();
        let context = Arc::new(
            IosReadContext::new(
                Arc::clone(&self.root),
                1,
                self.destination.clone(),
                password.into(),
                BTreeMap::new(),
                None,
                Arc::new(move || {
                    if std::thread::current().name() == Some("backup-cancel")
                        && let Some((cancel, observed)) = &cancel_in_wait
                    {
                        observed.store(true, Ordering::Release);
                        cancel.cancel();
                    }
                    check_root
                        .revalidate_canonical_binding()
                        .map_err(|_| ReadbackError::TargetRootChanged)
                }),
            )
            .unwrap(),
        );
        let input = match expected {
            Some(id) => prepared.bind_read(
                self.destination.clone(),
                password.into(),
                id,
                PathBuf::new(),
            ),
            None => prepared.capture_current_read(
                self.destination.clone(),
                password.into(),
                PathBuf::new(),
            ),
        };
        input
            .with_ios_context(context)
            .admit()
            .map(|read| (read, custody))
            .map_err(|failure| failure.reason())
    }
    fn file<'a>(&self, sha: &'a str) -> HistoricalArchiveFile<'a> {
        HistoricalArchiveFile {
            name: "clip.m4a",
            bytes: self.payload.len() as u64,
            sha256: sha,
        }
    }
    fn target(&self, read: &OwnedReadOperation) -> OwnedMediaTarget {
        let parent = MediaDirectory::open(Arc::clone(&self.root), Path::new("")).unwrap();
        MediaStage::create(parent, Arc::clone(read.activity()))
            .unwrap()
            .create_target()
            .unwrap()
    }
}
fn close(read: OwnedReadOperation, custody: ArchivePreparationCustody) {
    let settled = read
        .settle_direct()
        .unwrap_or_else(|_| panic!("normal original read settlement"));
    drop(settled.release());
    drop(custody);
}

#[test]
fn real_native_fresh_read_maps_paths_resolves_short_ids_verifies_bytes_and_settles() {
    let fixture = Fixture::new(2);
    let digest = format!("{:x}", Sha256::digest(&fixture.payload));
    let (mut read, custody) = fixture.read(None, "daily").unwrap();
    let short = &fixture.snapshots[0][..16];
    assert_eq!(
        fixture
            .snapshots
            .iter()
            .filter(|id| id.starts_with(short))
            .count(),
        1
    );
    let found = read
        .discover_historical_snapshot(short, &[fixture.file(&digest)])
        .unwrap();
    assert_eq!(found.snapshot_id, fixture.snapshots[0]);
    assert_eq!(found.files[0].archive_path, "/archive/clip.m4a");
    let target = fixture.target(&read);
    let mut bytes = read
        .read_exact(&found.files[0], target)
        .unwrap_or_else(|error| panic!("actual op22: {error}"));
    assert_eq!(bytes.digest().size, fixture.payload.len() as u64);
    assert_eq!(bytes.digest().sha256, digest);
    assert_eq!(bytes.cleanup(), Cleanup::Complete);
    drop(bytes);
    close(read, custody);
    // Fresh admission has no creator history from which to inherit qualification.
    assert!(fixture.read(Some("0".repeat(64)), "daily").is_err());
    let path = fixture.root.canonical_path().join("config/journal.json");
    let original = fs::read(&path).unwrap();
    let mut config: serde_json::Value = serde_json::from_slice(&original).unwrap();
    config["backup"]["daily_key"] = "wrong-key".into();
    fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
    assert!(fixture.read(None, "wrong-key").is_err());
    fs::write(path, original).unwrap();
}

#[test]
fn real_native_catalog_refuses_ambiguous_and_absent_short_selectors() {
    // Seventeen real snapshots force a shared first hex digit without a random
    // search loop. Source bytes/metadata stay unchanged, keeping pack/index reuse.
    let fixture = Fixture::new(17);
    let digest = format!("{:x}", Sha256::digest(&fixture.payload));
    let prefix = fixture
        .snapshots
        .iter()
        .map(|id| &id[..1])
        .find(|p| {
            fixture
                .snapshots
                .iter()
                .filter(|id| id.starts_with(*p))
                .count()
                > 1
        })
        .unwrap();
    let (mut read, _custody) = fixture.read(None, "daily").unwrap();
    assert!(
        read.discover_historical_snapshot(prefix, &[fixture.file(&digest)])
            .is_err()
    );
    drop(read);
    drop(_custody);
    let absent = (0..18)
        .map(|n| format!("{n:063x}"))
        .find(|p| !fixture.snapshots.iter().any(|id| id.starts_with(p)))
        .unwrap();
    let (mut read, _custody) = fixture.read(None, "daily").unwrap();
    assert!(
        read.discover_historical_snapshot(&absent, &[fixture.file(&digest)])
            .is_err()
    );
}

#[test]
fn native_reader_refuses_config_and_root_drift_before_target_writes() {
    let fixture = Fixture::new(1);
    let digest = format!("{:x}", Sha256::digest(&fixture.payload));
    let (mut read, _custody) = fixture.read(None, "daily").unwrap();
    let found = read
        .discover_historical_snapshot(&fixture.snapshots[0], &[fixture.file(&digest)])
        .unwrap();
    let target = fixture.target(&read);
    let config = fixture.root.canonical_path().join("config/journal.json");
    let before = fs::read(&config).unwrap();
    fs::write(&config, b"{\"backup\":{\"enabled\":false}}").unwrap();
    let mut refused = read.read_exact(&found.files[0], target).unwrap_err();
    assert_eq!(refused.cleanup(), Cleanup::Complete);
    fs::write(&config, before).unwrap();
    drop(refused);
    drop(read);
    drop(_custody);
    let (mut read, _custody) = fixture.read(None, "daily").unwrap();
    let original = fixture.root.canonical_path().to_path_buf();
    let moved = original.with_extension("retained-root");
    fs::rename(&original, &moved).unwrap();
    fs::create_dir(&original).unwrap();
    assert!(
        read.discover_historical_snapshot(&fixture.snapshots[0], &[fixture.file(&digest)])
            .is_err()
    );
    assert!(fs::read_dir(&original).unwrap().next().is_none());
    fs::remove_dir(&original).unwrap();
    fs::rename(&moved, &original).unwrap();
}

#[test]
fn native_read_same_size_and_mtime_still_checks_the_actual_bytes() {
    let fixture = Fixture::new(1);
    let source = fixture.root.canonical_path().join("source");
    let clip = source.join("clip.m4a");
    let metadata = fs::metadata(&clip).unwrap();
    let changed = vec![b'Z'; fixture.payload.len()];
    fs::write(&clip, &changed).unwrap();
    fs::File::options()
        .write(true)
        .open(&clip)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(metadata.modified().unwrap()))
        .unwrap();
    let held = fs::File::open(&source).unwrap();
    let result = completed(Call::archive_local(
        b"daily",
        fixture.repo.to_str().unwrap(),
        "archive",
        held.as_fd(),
        br#"["clip.m4a"]"#,
    ));
    let snapshot = id(&result.fields.snapshot_id);
    let digest = format!("{:x}", Sha256::digest(&changed));
    let (mut read, custody) = fixture.read(None, "daily").unwrap();
    let found = read
        .discover_historical_snapshot(&snapshot, &[fixture.file(&digest)])
        .unwrap();
    let mut actual = read
        .read_exact(&found.files[0], fixture.target(&read))
        .unwrap();
    assert_eq!(actual.digest().sha256, digest);
    assert_eq!(actual.cleanup(), Cleanup::Complete);
    drop(actual);
    close(read, custody);
    let old_digest = format!("{:x}", Sha256::digest(&fixture.payload));
    let (mut read, _custody) = fixture.read(None, "daily").unwrap();
    let found = read
        .discover_historical_snapshot(&snapshot, &[fixture.file(&old_digest)])
        .unwrap();
    let mut refused = read
        .read_exact(&found.files[0], fixture.target(&read))
        .unwrap_err();
    assert_eq!(refused.cleanup(), Cleanup::Complete);
}

#[test]
fn native_wait_observes_original_cancellation_and_same_owner_can_retry() {
    let mut fixture = Fixture::new(1);
    let observed = Arc::new(AtomicBool::new(false));
    fixture.cancel_in_wait = Some((ArchiveCancellation::default(), Arc::clone(&observed)));
    assert!(fixture.read(None, "daily").is_err());
    assert!(
        observed.load(Ordering::Acquire),
        "original cancellation reached the actual native wait monitor"
    );
    fixture.cancel_in_wait = None;
    let digest = format!("{:x}", Sha256::digest(&fixture.payload));
    let (mut read, custody) = fixture.read(None, "daily").unwrap();
    let found = read
        .discover_historical_snapshot(&fixture.snapshots[0], &[fixture.file(&digest)])
        .unwrap();
    let mut bytes = read
        .read_exact(&found.files[0], fixture.target(&read))
        .unwrap();
    assert_eq!(bytes.digest().sha256, digest);
    assert_eq!(bytes.cleanup(), Cleanup::Complete);
    drop(bytes);
    close(read, custody);
}

#[test]
fn actual_incomplete_native_listing_never_selects_partial_candidates() {
    let fixture = Fixture::new(1);
    let source = fixture.root.canonical_path().join("large-source");
    fs::create_dir(&source).unwrap();
    let mut names = Vec::new();
    for index in 0..2200 {
        let name = format!("f{index:04}-{}", "x".repeat(230));
        fs::write(source.join(&name), []).unwrap();
        names.push(name);
    }
    let held = fs::File::open(&source).unwrap();
    let result = completed(Call::archive_local(
        b"daily",
        fixture.repo.to_str().unwrap(),
        "archive",
        held.as_fd(),
        &serde_json::to_vec(&names).unwrap(),
    ));
    let snapshot = id(&result.fields.snapshot_id);
    let digest = format!("{:x}", Sha256::digest(&fixture.payload));
    let (mut read, _custody) = fixture.read(None, "daily").unwrap();
    assert!(matches!(
        read.discover_historical_snapshot(&snapshot, &[fixture.file(&digest)]),
        Err(ReadbackError::IncompleteDiscovery)
    ));
}
