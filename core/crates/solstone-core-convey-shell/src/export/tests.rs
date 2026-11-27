// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "export service unit tests"
)]

use std::fs;
use std::io::{Cursor, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

use axum::body::to_bytes;
use axum::http::{HeaderMap, HeaderValue, Method, Request, StatusCode, header};
use futures_util::StreamExt;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use solstone_core_convey_http::identity::{AccessBasis, Carrier};
use solstone_core_journal_archive::ExportMode;
use solstone_core_journal_io::disk_space::JournalDiskSpace;
use solstone_core_removals::{Action, Page, Surface, Unit};
use solstone_core_trash::{Change, Hold, Item, Kind, detach, erase};
use tempfile::TempDir;
use tower::ServiceExt;
use zip::ZipArchive;

use super::staging::{ChangeFingerprint, ReadyRecord, RequesterScope, StagingArea};
use super::{DiskSpaceProbe, ExportClock, ExportService, ROLLING_INACTIVITY_SECS, router};

struct MockClock(AtomicI64);

impl MockClock {
    fn new(epoch_secs: i64) -> Self {
        Self(AtomicI64::new(epoch_secs))
    }

    fn advance(&self, delta_secs: i64) {
        self.0.fetch_add(delta_secs, Ordering::SeqCst);
    }
}

impl ExportClock for MockClock {
    fn now_epoch_secs(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

struct MockDiskSpace {
    free: AtomicU64,
    total: u64,
}

impl MockDiskSpace {
    fn new(free: u64, total: u64) -> Self {
        Self {
            free: AtomicU64::new(free),
            total,
        }
    }

    fn set_free(&self, free: u64) {
        self.free.store(free, Ordering::SeqCst);
    }
}

impl DiskSpaceProbe for MockDiskSpace {
    fn read(&self, _staging: &StagingArea) -> std::io::Result<JournalDiskSpace> {
        Ok(JournalDiskSpace {
            free_bytes: self.free.load(Ordering::SeqCst),
            total_bytes: self.total,
        })
    }
}

struct NoopHold;

impl Hold for NoopHold {
    fn check(&self) -> Result<(), String> {
        Ok(())
    }
}

struct FakeTrashKind;

impl Kind for FakeTrashKind {
    fn handles(&self, unit: &Unit) -> bool {
        matches!(unit, Unit::Entity)
    }
    fn hold<'a>(&'a self, _journal: &Path, _item: &Item) -> Result<Box<dyn Hold + 'a>, String> {
        Ok(Box::new(NoopHold))
    }
    fn present(&self, journal: &Path, path: &str) -> bool {
        journal.join(path).exists()
    }
    fn references(
        &self,
        _journal: &Path,
        _item: &Item,
        _change: Change,
        _files: &[String],
    ) -> Result<(), String> {
        Ok(())
    }
}

fn seed_journal(root: &Path) {
    let write = |rel: &str, bytes: &[u8]| {
        let full = root.join(rel);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(full, bytes).unwrap();
    };
    write("config/journal.json", br#"{"schema":1}"#);
    write(
        "entities/alice/entity.json",
        br#"{"id":"alice","name":"Alice"}"#,
    );
    write("notes/owner.txt", b"owner note bytes");
    write(
        "chronicle/20261125/mic/090000_60/audio.flac",
        &vec![b'A'; 8192],
    );
    let sidecar = format!(
        "{}\n{}\n",
        json!({
            "_solstone_processing": {
                "schema": "solstone.processing.v1",
                "handler": "transcribe",
                "state": "analyzed",
                "input_size": 8192,
                "attempted_at": "2026-11-25T10:00:00Z"
            }
        }),
        json!({"start": 0.0, "end": 1.0, "text": "hello"})
    );
    write(
        "chronicle/20261125/mic/090000_60/audio.jsonl",
        sidecar.as_bytes(),
    );
    write(
        "chronicle/20261125/photos/100000_0/photo.jpg",
        b"jpeg-bytes",
    );
    write("events.jsonl", b"{\"event\":\"boot\"}\n");
}

fn web_surface() -> Surface {
    Surface::Web {
        page: Page::Settings,
    }
}

fn trigger_permanent_trash_erase(journal: &Path) {
    let kind = FakeTrashKind;
    let entity_path = journal.join("entities/disposable/entity.json");
    fs::create_dir_all(entity_path.parent().unwrap()).unwrap();
    fs::write(&entity_path, br#"{"id":"disposable"}"#).unwrap();
    let item = detach(
        journal,
        &kind,
        Unit::Entity,
        Action::EntityDelete,
        web_surface(),
        vec!["entities/disposable".into()],
    )
    .unwrap();
    erase(journal, &[&kind], &item.id, Action::Erase, web_surface()).unwrap();
}

async fn wait_until_not_preparing(service: &ExportService, requester: &RequesterScope) -> Value {
    for _ in 0..200 {
        let state = service.state_payload(requester);
        if state["status"] != "preparing" {
            return state;
        }
        tokio::time::sleep(std::time::Duration::from_millis(15)).await;
    }
    panic!("export job did not finish in time");
}

fn make_service(
    journal_root: &Path,
    staging_dir: &Path,
    clock: Arc<MockClock>,
    disk: Arc<MockDiskSpace>,
) -> ExportService {
    let staging = StagingArea::at_dir(staging_dir.to_owned(), true).unwrap();
    ExportService::with_deps(journal_root.to_owned(), staging, clock, disk)
}

#[tokio::test]
async fn prepare_download_range_resume_lighter_mode_and_discard() {
    let work = TempDir::new().unwrap();
    let journal = work.path().join("journal");
    let staging = work.path().join("staging");
    seed_journal(&journal);

    let clock = Arc::new(MockClock::new(1_764_150_000));
    let disk = Arc::new(MockDiskSpace::new(
        50 * 1024 * 1024 * 1024,
        100 * 1024 * 1024 * 1024,
    ));
    let service = make_service(&journal, &staging, Arc::clone(&clock), Arc::clone(&disk));

    // Start lighter copy (`WithoutProcessedMedia`).
    let (status, _) = service
        .start_job(
            RequesterScope::Local,
            ExportMode::WithoutProcessedMedia,
            false,
        )
        .unwrap();
    assert_eq!(status, StatusCode::ACCEPTED);

    let ready_state = wait_until_not_preparing(&service, &RequesterScope::Local).await;
    assert_eq!(ready_state["status"], "ready");
    let export_id = ready_state["ready"]["export_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let size_bytes = ready_state["ready"]["size_bytes"].as_u64().unwrap();
    let sha256 = ready_state["ready"]["sha256"].as_str().unwrap().to_owned();
    let etag = ready_state["ready"]["etag"].as_str().unwrap().to_owned();
    assert!(
        ready_state["ready"]["filename"]
            .as_str()
            .unwrap()
            .ends_with("-light.zip")
    );

    // HEAD preserves exact length, ETag, Accept-Ranges, Content-Disposition.
    let head_resp = service
        .serve_download(
            &RequesterScope::Local,
            &export_id,
            &Method::HEAD,
            &HeaderMap::new(),
        )
        .await;
    assert_eq!(head_resp.status(), StatusCode::OK);
    assert_eq!(
        head_resp
            .headers()
            .get(header::CONTENT_LENGTH)
            .unwrap()
            .to_str()
            .unwrap(),
        size_bytes.to_string()
    );
    assert_eq!(
        head_resp.headers().get(header::ACCEPT_RANGES).unwrap(),
        "bytes"
    );
    assert_eq!(
        head_resp.headers().get(header::ETAG).unwrap(),
        etag.as_str()
    );
    assert!(
        head_resp
            .headers()
            .get(header::CONTENT_DISPOSITION)
            .unwrap()
            .to_str()
            .unwrap()
            .contains("-light.zip")
    );

    // Interrupted download + Range + If-Range resume across two pieces.
    let split = size_bytes / 2;
    let mut h1 = HeaderMap::new();
    h1.insert(
        header::RANGE,
        HeaderValue::from_str(&format!("bytes=0-{}", split - 1)).unwrap(),
    );
    h1.insert(header::IF_RANGE, HeaderValue::from_str(&etag).unwrap());
    let p1 = service
        .serve_download(&RequesterScope::Local, &export_id, &Method::GET, &h1)
        .await;
    assert_eq!(p1.status(), StatusCode::PARTIAL_CONTENT);
    let b1 = to_bytes(p1.into_body(), usize::MAX).await.unwrap();
    assert_eq!(b1.len() as u64, split);

    let mut h2 = HeaderMap::new();
    h2.insert(
        header::RANGE,
        HeaderValue::from_str(&format!("bytes={split}-")).unwrap(),
    );
    h2.insert(header::IF_RANGE, HeaderValue::from_str(&etag).unwrap());
    let p2 = service
        .serve_download(&RequesterScope::Local, &export_id, &Method::GET, &h2)
        .await;
    assert_eq!(p2.status(), StatusCode::PARTIAL_CONTENT);
    let b2 = to_bytes(p2.into_body(), usize::MAX).await.unwrap();

    let mut assembled = Vec::with_capacity(size_bytes as usize);
    assembled.extend_from_slice(&b1);
    assembled.extend_from_slice(&b2);
    assert_eq!(assembled.len() as u64, size_bytes);
    assert_eq!(format!("{:x}", Sha256::digest(&assembled)), sha256);

    // Verify assembled ZIP omits processed audio.flac while keeping photo.jpg and notes.
    let mut zip = ZipArchive::new(Cursor::new(assembled)).unwrap();
    assert!(
        zip.by_name("chronicle/20261125/mic/090000_60/audio.flac")
            .is_err()
    );
    assert!(
        zip.by_name("chronicle/20261125/mic/090000_60/audio.jsonl")
            .is_ok()
    );
    assert!(
        zip.by_name("chronicle/20261125/photos/100000_0/photo.jpg")
            .is_ok()
    );
    assert!(zip.by_name("notes/owner.txt").is_ok());
    assert!(zip.by_name("_export.json").is_ok());

    // Mismatched If-Range falls back to 200 OK full archive.
    let mut mismatch = HeaderMap::new();
    mismatch.insert(header::RANGE, HeaderValue::from_static("bytes=0-15"));
    mismatch.insert(header::IF_RANGE, HeaderValue::from_static("\"stale\""));
    let full = service
        .serve_download(&RequesterScope::Local, &export_id, &Method::GET, &mismatch)
        .await;
    assert_eq!(full.status(), StatusCode::OK);
    assert_eq!(
        full.headers()
            .get(header::CONTENT_LENGTH)
            .unwrap()
            .to_str()
            .unwrap(),
        size_bytes.to_string()
    );
    drop(full);

    // Unsatisfiable range returns 416 with exact Content-Range.
    let mut oob = HeaderMap::new();
    oob.insert(
        header::RANGE,
        HeaderValue::from_str(&format!("bytes={size_bytes}-")).unwrap(),
    );
    let r416 = service
        .serve_download(&RequesterScope::Local, &export_id, &Method::GET, &oob)
        .await;
    assert_eq!(r416.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(
        r416.headers()
            .get(header::CONTENT_RANGE)
            .unwrap()
            .to_str()
            .unwrap(),
        format!("bytes */{size_bytes}")
    );

    // Discard removes staged ZIP from disk.
    assert_eq!(
        service
            .discard_ready(&RequesterScope::Local, &export_id, false)
            .unwrap(),
        StatusCode::OK
    );
    assert!(!staging.join(format!("{export_id}.zip")).exists());
    assert_eq!(
        service.state_payload(&RequesterScope::Local)["status"],
        "idle"
    );
}

#[tokio::test]
async fn rolling_24h_inactivity_and_over_24h_continuous_transfer() {
    let work = TempDir::new().unwrap();
    let journal = work.path().join("journal");
    let staging = work.path().join("staging");
    seed_journal(&journal);

    let clock = Arc::new(MockClock::new(1_764_150_000));
    let disk = Arc::new(MockDiskSpace::new(
        50 * 1024 * 1024 * 1024,
        100 * 1024 * 1024 * 1024,
    ));
    let service = make_service(&journal, &staging, Arc::clone(&clock), Arc::clone(&disk));

    service
        .start_job(RequesterScope::Local, ExportMode::Full, false)
        .unwrap();
    let ready = wait_until_not_preparing(&service, &RequesterScope::Local).await;
    let export_id = ready["ready"]["export_id"].as_str().unwrap().to_owned();

    // Advance 23h55m -> still alive.
    clock.advance(ROLLING_INACTIVITY_SECS - 300);
    assert_eq!(
        service.state_payload(&RequesterScope::Local)["status"],
        "ready"
    );

    // Range read resets the 24h rolling clock.
    let mut rh = HeaderMap::new();
    rh.insert(header::RANGE, HeaderValue::from_static("bytes=0-15"));
    let resp = service
        .serve_download(&RequesterScope::Local, &export_id, &Method::GET, &rh)
        .await;
    assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
    let _ = to_bytes(resp.into_body(), usize::MAX).await.unwrap();

    // Advance another 23h -> 46h55m since creation, STILL alive because activity reset!
    clock.advance(23 * 3600);
    assert_eq!(
        service.state_payload(&RequesterScope::Local)["status"],
        "ready"
    );

    // Open a single continuous response, advance 26 hours WHILE IT IS OPEN, and
    // verify the reaper does NOT delete it mid-stream!
    let long_resp = service
        .serve_download(
            &RequesterScope::Local,
            &export_id,
            &Method::GET,
            &HeaderMap::new(),
        )
        .await;
    assert_eq!(long_resp.status(), StatusCode::OK);
    let mut stream = long_resp.into_body().into_data_stream();
    let first_chunk = stream.next().await.unwrap().unwrap();
    assert!(!first_chunk.is_empty());

    clock.advance(26 * 3600);
    service.reconcile_ready_state();
    assert!(
        staging.join(format!("{export_id}.zip")).exists(),
        "copy must stay alive while single continuous response lasts >24h"
    );

    // Finish draining the stream cleanly.
    while let Some(chunk) = stream.next().await {
        chunk.unwrap();
    }

    // Now go idle for 24h + 1s -> reaper expires and unlinks the staged file.
    clock.advance(ROLLING_INACTIVITY_SECS + 1);
    let expired = service.state_payload(&RequesterScope::Local);
    assert_eq!(expired["status"], "expired");
    assert!(!staging.join(format!("{export_id}.zip")).exists());
}

#[tokio::test]
async fn permanent_trash_erasure_invalidates_idle_active_and_preparing_exports() {
    let work = TempDir::new().unwrap();
    let journal = work.path().join("journal");
    let staging = work.path().join("staging");
    seed_journal(&journal);

    let clock = Arc::new(MockClock::new(1_764_150_000));
    let disk = Arc::new(MockDiskSpace::new(
        50 * 1024 * 1024 * 1024,
        100 * 1024 * 1024 * 1024,
    ));
    let service = make_service(&journal, &staging, Arc::clone(&clock), Arc::clone(&disk));

    // Case A: Idle ready copy invalidated immediately on permanent Trash erasure.
    service
        .start_job(RequesterScope::Local, ExportMode::Full, false)
        .unwrap();
    let ready_a = wait_until_not_preparing(&service, &RequesterScope::Local).await;
    let id_a = ready_a["ready"]["export_id"].as_str().unwrap().to_owned();
    assert!(staging.join(format!("{id_a}.zip")).exists());

    trigger_permanent_trash_erase(&journal);
    let after_a = service
        .serve_download(
            &RequesterScope::Local,
            &id_a,
            &Method::GET,
            &HeaderMap::new(),
        )
        .await;
    assert_eq!(after_a.status(), StatusCode::GONE);
    assert!(!staging.join(format!("{id_a}.zip")).exists());

    // Case B: Active transfer during permanent Trash erasure refuses new Range (410)
    // and unlinks staged ZIP the instant the active transfer becomes idle.
    service
        .start_job(RequesterScope::Local, ExportMode::Full, false)
        .unwrap();
    let ready_b = wait_until_not_preparing(&service, &RequesterScope::Local).await;
    let id_b = ready_b["ready"]["export_id"].as_str().unwrap().to_owned();

    let active_resp = service
        .serve_download(
            &RequesterScope::Local,
            &id_b,
            &Method::GET,
            &HeaderMap::new(),
        )
        .await;
    assert_eq!(active_resp.status(), StatusCode::OK);

    trigger_permanent_trash_erase(&journal);

    // New Range request while active is immediately refused with 410 Gone.
    let mut rh = HeaderMap::new();
    rh.insert(header::RANGE, HeaderValue::from_static("bytes=0-15"));
    let second = service
        .serve_download(&RequesterScope::Local, &id_b, &Method::GET, &rh)
        .await;
    assert_eq!(second.status(), StatusCode::GONE);

    // Once active response drops (transfer becomes idle), staged ZIP is immediately deleted.
    drop(active_resp);
    assert!(
        !staging.join(format!("{id_b}.zip")).exists(),
        "staged copy must delete immediately once active transfer becomes idle after erasure"
    );

    // Case C: Permanent erase of a trashed item during preparation must never
    // publish a ready copy containing erased data.
    for i in 0..40 {
        let p = journal.join(format!("notes/bulk-{i:03}.txt"));
        fs::write(p, vec![b'x'; 64 * 1024]).unwrap();
    }
    let kind = FakeTrashKind;
    let disposable_path = journal.join("entities/disposable/entity.json");
    fs::create_dir_all(disposable_path.parent().unwrap()).unwrap();
    fs::write(&disposable_path, br#"{"id":"disposable"}"#).unwrap();
    let trashed_item = detach(
        &journal,
        &kind,
        Unit::Entity,
        Action::EntityDelete,
        web_surface(),
        vec!["entities/disposable".into()],
    )
    .unwrap();
    service
        .start_job(RequesterScope::Local, ExportMode::Full, false)
        .unwrap();
    erase(
        &journal,
        &[&kind],
        &trashed_item.id,
        Action::Erase,
        web_surface(),
    )
    .unwrap();
    let outcome_c = wait_until_not_preparing(&service, &RequesterScope::Local).await;
    // Either the job saw the epoch bump mid-prepare (`erased_invalidated`), OR it
    // finished right before the erase and `state_payload` immediately retired it
    // as `erased_invalidated`! Either way, NO ready copy survives with erased data.
    assert_eq!(
        outcome_c["status"], "erased_invalidated",
        "outcome_c={outcome_c}"
    );
    assert!(outcome_c["ready"].is_null());
}

#[tokio::test]
async fn requester_scoping_revocation_disk_pressure_and_restart_recovery() {
    let work = TempDir::new().unwrap();
    let journal = work.path().join("journal");
    let staging = work.path().join("staging");
    seed_journal(&journal);

    // Authorize device cid-a and cid-b.
    let cid_a = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let cid_b = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let client_entry = |cid: &str, label: &str| {
        json!({
            "fingerprint": cid,
            "device_label": label,
            "paired_at": "2026-11-26T00:00:00Z",
            "instance_id": "device-instance",
            "role": "observer",
            "network": "home",
            "client_label": "Phone",
            "label_ordinal": 1,
            "kind": "cert",
        })
    };
    let auth_path = journal.join("link/authorized_clients.json");
    fs::create_dir_all(auth_path.parent().unwrap()).unwrap();
    fs::write(
        &auth_path,
        serde_json::to_vec(&json!([
            client_entry(cid_a, "Phone A"),
            client_entry(cid_b, "Phone B"),
        ]))
        .unwrap(),
    )
    .unwrap();

    let clock = Arc::new(MockClock::new(1_764_150_000));
    let disk = Arc::new(MockDiskSpace::new(
        50 * 1024 * 1024 * 1024,
        100 * 1024 * 1024 * 1024,
    ));
    let service = make_service(&journal, &staging, Arc::clone(&clock), Arc::clone(&disk));

    let dev_a = RequesterScope::LinkedDevice {
        cid: cid_a.to_owned(),
    };
    let dev_b = RequesterScope::LinkedDevice {
        cid: cid_b.to_owned(),
    };

    service
        .start_job(dev_a.clone(), ExportMode::Full, false)
        .unwrap();
    let state_a = wait_until_not_preparing(&service, &dev_a).await;
    assert_eq!(state_a["status"], "ready");
    let export_id = state_a["ready"]["export_id"].as_str().unwrap().to_owned();

    // Cross-requester B cannot see metadata or download A's export.
    let state_b = service.state_payload(&dev_b);
    assert_eq!(state_b["status"], "idle");
    assert!(state_b["ready"].is_null());
    let cross_dl = service
        .serve_download(&dev_b, &export_id, &Method::GET, &HeaderMap::new())
        .await;
    assert_eq!(cross_dl.status(), StatusCode::NOT_FOUND);

    // PairingPeer is refused 403 at router boundary.
    let app = router(journal.clone(), service.clone());
    let mut req = Request::builder()
        .uri("/app/export/api/state")
        .body(axum::body::Body::empty())
        .unwrap();
    req.extensions_mut().insert(AccessBasis::PairingPeer {
        carrier: Carrier::Direct,
    });
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // Restart recovery: orphaned `.part` is swept while valid ready copy survives!
    fs::write(staging.join("orphan.zip.part"), b"partial").unwrap();
    let recovered = make_service(&journal, &staging, Arc::clone(&clock), Arc::clone(&disk));
    assert!(!staging.join("orphan.zip.part").exists());
    assert_eq!(recovered.state_payload(&dev_a)["status"], "ready");
    assert_eq!(
        recovered.state_payload(&dev_a)["ready"]["export_id"],
        export_id
    );

    // Revoking cid-a in authorized_clients.json retires its ready copy immediately.
    fs::write(
        &auth_path,
        serde_json::to_vec(&json!([client_entry(cid_b, "Phone B")])).unwrap(),
    )
    .unwrap();
    let after_revoke = recovered
        .serve_download(&dev_a, &export_id, &Method::GET, &HeaderMap::new())
        .await;
    assert_eq!(after_revoke.status(), StatusCode::NOT_FOUND);
    assert!(!staging.join(format!("{export_id}.zip")).exists());

    // Disk pressure preflight refuses when free < required, then recovers when space frees up.
    disk.set_free(512 * 1024 * 1024); // 512 MiB < 1 GiB headroom floor
    recovered
        .start_job(RequesterScope::Local, ExportMode::Full, false)
        .unwrap();
    let failed = wait_until_not_preparing(&recovered, &RequesterScope::Local).await;
    assert_eq!(failed["status"], "failed");
    assert_eq!(
        failed["active_job"]["error_code"],
        "insufficient_journal_space"
    );

    disk.set_free(20 * 1024 * 1024 * 1024);
    recovered
        .start_job(
            RequesterScope::Local,
            ExportMode::WithoutProcessedMedia,
            false,
        )
        .unwrap();
    let ok = wait_until_not_preparing(&recovered, &RequesterScope::Local).await;
    assert_eq!(ok["status"], "ready");
}

#[tokio::test]
async fn serves_real_byte_ranges_beyond_4_gib_offset_over_sparse_staged_file() {
    let work = TempDir::new().unwrap();
    let journal = work.path().join("journal");
    let staging = work.path().join("staging");
    seed_journal(&journal);
    fs::create_dir_all(&staging).unwrap();

    // Create a 5 GiB sparse staged file with known magic bytes past 4 GiB.
    let offset_past_4gib: u64 = 4 * 1024 * 1024 * 1024 + 131_072;
    let payload = b"SOLSTONE_EXPORT_PAST_4GIB_RANGE_OK";
    let total_size = offset_past_4gib + payload.len() as u64;
    let zip_name = "exp_sparse4g.zip";
    {
        let mut f = fs::File::create(staging.join(zip_name)).unwrap();
        f.seek(SeekFrom::Start(offset_past_4gib)).unwrap();
        f.write_all(payload).unwrap();
        f.sync_all().unwrap();
    }

    let clock = Arc::new(MockClock::new(1_764_150_000));
    let disk = Arc::new(MockDiskSpace::new(
        50 * 1024 * 1024 * 1024,
        100 * 1024 * 1024 * 1024,
    ));
    let staging_area = StagingArea::at_dir(staging.clone(), true).unwrap();
    let epoch = solstone_core_trash::read_erasure_epoch(&journal).unwrap();
    let record = ReadyRecord {
        schema: ReadyRecord::SCHEMA,
        export_id: "exp_sparse4g".to_owned(),
        mode: ExportMode::Full,
        filename: "solstone-journal-2026-11-26.zip".to_owned(),
        zip_filename: zip_name.to_owned(),
        size_bytes: total_size,
        sha256: "a".repeat(64),
        created_at_epoch_secs: 1_764_150_000,
        last_activity_epoch_secs: 1_764_150_000,
        owner: RequesterScope::Local,
        erasure_epoch: epoch,
        fingerprint: ChangeFingerprint::capture(&journal),
        days_count: 1,
        files_count: 2,
        omitted_media_files: 0,
        omitted_media_bytes: 0,
        offloaded_days: 0,
        offloaded_bytes: 0,
        offload_degraded: false,
    };
    staging_area.write_ready_record(&record).unwrap();

    let service = ExportService::with_deps(journal.clone(), staging_area, clock, disk);
    let app = router(journal, service);

    // Exercise via the real Axum router (both `/app/export/api/ready/...` AND legacy `?id=`).
    let end = total_size - 1;
    let mut req = Request::builder()
        .uri("/app/export/api/ready/exp_sparse4g/download")
        .header(header::RANGE, format!("bytes={offset_past_4gib}-{end}"))
        .header(header::IF_RANGE, record.strong_etag())
        .body(axum::body::Body::empty())
        .unwrap();
    req.extensions_mut().insert(AccessBasis::Localhost);
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        resp.headers()
            .get(header::CONTENT_RANGE)
            .unwrap()
            .to_str()
            .unwrap(),
        format!("bytes {offset_past_4gib}-{end}/{total_size}")
    );
    let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    assert_eq!(&body[..], payload);

    // Legacy `/app/import/api/journal-archive/export?id=exp_sparse4g` serves the same prepared bytes!
    let mut legacy_req = Request::builder()
        .uri("/app/import/api/journal-archive/export?id=exp_sparse4g")
        .header(header::RANGE, format!("bytes={offset_past_4gib}-{end}"))
        .body(axum::body::Body::empty())
        .unwrap();
    legacy_req.extensions_mut().insert(AccessBasis::Localhost);
    let legacy_resp = app.oneshot(legacy_req).await.unwrap();
    assert_eq!(legacy_resp.status(), StatusCode::PARTIAL_CONTENT);
    let legacy_body = to_bytes(legacy_resp.into_body(), usize::MAX).await.unwrap();
    assert_eq!(&legacy_body[..], payload);
}

#[tokio::test]
async fn staging_failure_fails_closed_without_shared_fallback_or_touching_other_journals() {
    let work = TempDir::new().unwrap();
    let journal_a = work.path().join("journal-a");
    let journal_b = work.path().join("journal-b");
    seed_journal(&journal_a);
    seed_journal(&journal_b);

    // Journal A prepares a real ready export in its own isolated staging directory.
    let service_a = ExportService::new(journal_a.clone());
    service_a
        .start_job(RequesterScope::Local, ExportMode::Full, false)
        .unwrap();
    let ready_a = wait_until_not_preparing(&service_a, &RequesterScope::Local).await;
    assert_eq!(ready_a["status"], "ready");
    let export_id_a = ready_a["ready"]["export_id"].as_str().unwrap().to_owned();

    // Block BOTH Journal B's sibling staging path and its per-journal hashed temp fallback path.
    let sibling_b = work.path().join(".journal-b.export-staging");
    fs::write(&sibling_b, b"blocking-file").unwrap();
    let temp_b = StagingArea::temp_fallback_path(&journal_b).unwrap();
    let _ = fs::remove_dir_all(&temp_b);
    let _ = fs::remove_file(&temp_b);
    fs::write(&temp_b, b"blocking-temp-file").unwrap();

    // Remove any stale legacy shared fallback path if present so we prove we never create/use it.
    let forbidden_shared = std::env::temp_dir().join("solstone-export-staging-fallback");
    let shared_existed_before = forbidden_shared.exists();

    let service_b = ExportService::new(journal_b.clone());
    let _ = fs::remove_file(&temp_b);

    if !shared_existed_before {
        assert!(
            !forbidden_shared.exists(),
            "ExportService::new must never create or use a shared temp fallback directory"
        );
    }

    // Journal B fails closed on all export endpoints while staying constructed/usable.
    let state_b = service_b.state_payload(&RequesterScope::Local);
    assert_eq!(state_b["status"], "unavailable");
    assert_eq!(state_b["staging_available"], false);
    assert_eq!(state_b["error_code"], "export_staging_unavailable");

    let start_err = service_b
        .start_job(RequesterScope::Local, ExportMode::Full, false)
        .expect_err("start_job must fail closed when staging is unavailable");
    assert_eq!(start_err.0, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(start_err.1["code"], "export_staging_unavailable");

    let dl_resp = service_b
        .serve_download(
            &RequesterScope::Local,
            "any",
            &Method::GET,
            &HeaderMap::new(),
        )
        .await;
    assert_eq!(dl_resp.status(), StatusCode::SERVICE_UNAVAILABLE);

    let discard_err = service_b
        .discard_ready(&RequesterScope::Local, "any", false)
        .expect_err("discard must fail closed when staging is unavailable");
    assert_eq!(discard_err.0, StatusCode::SERVICE_UNAVAILABLE);

    // Journal A's isolated ready copy is completely unaffected.
    let state_a_after = service_a.state_payload(&RequesterScope::Local);
    assert_eq!(state_a_after["status"], "ready");
    let dl_a = service_a
        .serve_download(
            &RequesterScope::Local,
            &export_id_a,
            &Method::HEAD,
            &HeaderMap::new(),
        )
        .await;
    assert_eq!(dl_a.status(), StatusCode::OK);
}

#[tokio::test]
async fn in_flight_download_aborts_when_permanent_trash_erasure_occurs() {
    let work = TempDir::new().unwrap();
    let journal = work.path().join("journal");
    let staging = work.path().join("staging");
    seed_journal(&journal);
    // Add incompressible high-entropy payload > 512 KiB so download spans multiple 64 KiB chunks.
    let mut blob = vec![0u8; 600 * 1024];
    let mut prng: u64 = 0x1234_5678_9abc_def0;
    for byte in &mut blob {
        prng = prng.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = prng;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        *byte = ((z ^ (z >> 31)) & 0xff) as u8;
    }
    fs::write(journal.join("notes/large.bin"), &blob).unwrap();

    let clock = Arc::new(MockClock::new(1_764_150_000));
    let disk = Arc::new(MockDiskSpace::new(
        50 * 1024 * 1024 * 1024,
        100 * 1024 * 1024 * 1024,
    ));
    let service = make_service(&journal, &staging, Arc::clone(&clock), Arc::clone(&disk));

    // Case 1: On-disk erasure detected on 30s throttled check without any other API call.
    service
        .start_job(RequesterScope::Local, ExportMode::Full, false)
        .unwrap();
    let ready = wait_until_not_preparing(&service, &RequesterScope::Local).await;
    let export_id = ready["ready"]["export_id"].as_str().unwrap().to_owned();

    let resp = service
        .serve_download(
            &RequesterScope::Local,
            &export_id,
            &Method::GET,
            &HeaderMap::new(),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let mut stream = resp.into_body().into_data_stream();
    let first = stream.next().await.unwrap().unwrap();
    assert!(!first.is_empty());

    trigger_permanent_trash_erase(&journal);
    clock.advance(super::range::POLICY_RECHECK_INTERVAL_SECS);
    let mut saw_error = false;
    while let Some(item) = stream.next().await {
        if item.is_err() {
            saw_error = true;
            break;
        }
    }
    assert!(
        saw_error,
        "in-flight download must abort once permanent Trash erasure is detected"
    );
    drop(stream);
    assert!(
        !staging.join(format!("{export_id}.zip")).exists(),
        "erased staged copy must unlink immediately after aborted stream drops"
    );

    // Case 2: Immediate zero-delay abort when reconcile_ready_state runs while streaming.
    service
        .start_job(RequesterScope::Local, ExportMode::Full, false)
        .unwrap();
    let ready2 = wait_until_not_preparing(&service, &RequesterScope::Local).await;
    let id2 = ready2["ready"]["export_id"].as_str().unwrap().to_owned();
    let resp2 = service
        .serve_download(
            &RequesterScope::Local,
            &id2,
            &Method::GET,
            &HeaderMap::new(),
        )
        .await;
    let mut stream2 = resp2.into_body().into_data_stream();
    let _ = stream2.next().await.unwrap().unwrap();

    trigger_permanent_trash_erase(&journal);
    service.reconcile_ready_state();
    // Do NOT advance clock: fast atomic signal stops the next chunk immediately.
    let next_item = stream2.next().await.expect("stream produces error");
    assert!(
        next_item.is_err(),
        "reconcile must abort in-flight stream immediately"
    );
    drop(stream2);
    assert!(!staging.join(format!("{id2}.zip")).exists());
}

#[tokio::test]
async fn disk_space_enforces_journal_20gb_floor_at_preflight_and_during_packing() {
    let work = TempDir::new().unwrap();
    let journal = work.path().join("journal");
    let staging = work.path().join("staging");
    seed_journal(&journal);
    for i in 0..32 {
        fs::write(
            journal.join(format!("notes/chunk-{i:02}.bin")),
            vec![(i as u8).wrapping_mul(37); 128 * 1024],
        )
        .unwrap();
    }

    let clock = Arc::new(MockClock::new(1_764_150_000));
    let total = 100_000_000_000u64; // 100 GB -> 20 GB floor

    // 1. Preflight with 19.5 GB free (< 20 GB floor) must fail cleanly.
    let disk = Arc::new(MockDiskSpace::new(19_500_000_000, total));
    let service = make_service(&journal, &staging, Arc::clone(&clock), Arc::clone(&disk));
    service
        .start_job(RequesterScope::Local, ExportMode::Full, false)
        .unwrap();
    let failed = wait_until_not_preparing(&service, &RequesterScope::Local).await;
    assert_eq!(failed["status"], "failed");
    assert_eq!(
        failed["active_job"]["error_code"],
        "insufficient_journal_space"
    );

    // 2. Mid-packing drop below the 20 GB floor must stop packing and delete `.part`.
    struct DroppingDiskProbe {
        calls: AtomicU64,
        total: u64,
    }
    impl DiskSpaceProbe for DroppingDiskProbe {
        fn read(&self, _staging: &StagingArea) -> std::io::Result<JournalDiskSpace> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            // First 2 reads (start_job + worker preflight) pass; packing check drops below 20 GB.
            let free_bytes = if n < 2 {
                25_000_000_000
            } else {
                19_000_000_000
            };
            Ok(JournalDiskSpace {
                free_bytes,
                total_bytes: self.total,
            })
        }
    }
    let dropping = Arc::new(DroppingDiskProbe {
        calls: AtomicU64::new(0),
        total,
    });
    let staging_area = StagingArea::at_dir(staging.clone(), true).unwrap();
    let service2 = ExportService::with_deps(
        journal.clone(),
        staging_area,
        Arc::clone(&clock) as Arc<dyn ExportClock>,
        dropping,
    );
    service2
        .start_job(RequesterScope::Local, ExportMode::Full, false)
        .unwrap();
    let aborted = wait_until_not_preparing(&service2, &RequesterScope::Local).await;
    assert_eq!(aborted["status"], "failed");
    assert_eq!(
        aborted["active_job"]["error_code"],
        "insufficient_journal_space"
    );
    assert!(
        fs::read_dir(&staging)
            .unwrap()
            .flatten()
            .all(|e| !e.file_name().to_string_lossy().ends_with(".part")),
        "mid-packing space abort must remove partial staged archive"
    );
}

#[tokio::test]
async fn burst_download_throttles_ready_json_writes_to_thirty_second_interval() {
    let work = TempDir::new().unwrap();
    let journal = work.path().join("journal");
    let staging = work.path().join("staging");
    seed_journal(&journal);
    // ~3 MiB incompressible payload so archive > 3 MiB (used to trigger 3+ per-MiB rewrites).
    let mut payload = vec![0u8; 3 * 1024 * 1024];
    let mut prng: u64 = 0xfedc_ba98_7654_3210;
    for byte in &mut payload {
        prng = prng.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = prng;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        *byte = ((z ^ (z >> 31)) & 0xff) as u8;
    }
    fs::write(journal.join("notes/incompressible.bin"), &payload).unwrap();

    let clock = Arc::new(MockClock::new(1_764_150_000));
    let disk = Arc::new(MockDiskSpace::new(
        50 * 1024 * 1024 * 1024,
        100 * 1024 * 1024 * 1024,
    ));
    let service = make_service(&journal, &staging, Arc::clone(&clock), Arc::clone(&disk));

    service
        .start_job(RequesterScope::Local, ExportMode::Full, false)
        .unwrap();
    let ready = wait_until_not_preparing(&service, &RequesterScope::Local).await;
    let export_id = ready["ready"]["export_id"].as_str().unwrap().to_owned();
    let ready_json = staging.join(super::staging::READY_META_FILE);
    let before_bytes = fs::read(&ready_json).unwrap();
    #[cfg(unix)]
    let before_ino = {
        use std::os::unix::fs::MetadataExt;
        fs::metadata(&ready_json).unwrap().ino()
    };

    // Full download within 10 seconds -> zero extra ready.json rewrites!
    clock.advance(10);
    let resp = service
        .serve_download(
            &RequesterScope::Local,
            &export_id,
            &Method::GET,
            &HeaderMap::new(),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    assert!(bytes.len() > 2 * 1024 * 1024);
    assert_eq!(fs::read(&ready_json).unwrap(), before_bytes);
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(
            fs::metadata(&ready_json).unwrap().ino(),
            before_ino,
            "burst download within 30s must not rewrite ready.json"
        );
    }

    // Advancing >= 30s mid-stream DOES persist refreshed last_activity_epoch_secs.
    let resp2 = service
        .serve_download(
            &RequesterScope::Local,
            &export_id,
            &Method::GET,
            &HeaderMap::new(),
        )
        .await;
    let mut stream2 = resp2.into_body().into_data_stream();
    let _ = stream2.next().await.unwrap().unwrap();
    clock.advance(super::range::ACTIVITY_TOUCH_INTERVAL_SECS + 5);
    while let Some(chunk) = stream2.next().await {
        chunk.unwrap();
    }
    let after_bytes = fs::read(&ready_json).unwrap();
    assert_ne!(
        after_bytes, before_bytes,
        "download crossing 30s interval must persist refreshed activity timestamp"
    );
}

#[tokio::test]
async fn estimate_reports_staging_volume_identity_for_same_and_distinct_sibling_mounts() {
    let work = TempDir::new().unwrap();
    let journal = work.path().join("journal");
    seed_journal(&journal);
    let expected_sibling = fs::canonicalize(work.path())
        .unwrap()
        .join(".journal.export-staging");

    // 1. Sibling staging directory on the SAME volume as the journal.
    let same_staging = StagingArea::resolve_with_volume_check(&journal, |j, s| {
        assert_eq!(s, expected_sibling.as_path());
        assert_ne!(j, s);
        true
    })
    .unwrap();
    assert_eq!(same_staging.dir, expected_sibling);
    assert!(same_staging.same_volume_as_journal);

    let clock: Arc<dyn ExportClock> = Arc::new(MockClock::new(1_764_150_000));
    let same_disk: Arc<dyn DiskSpaceProbe> =
        Arc::new(MockDiskSpace::new(99_586_048, 96 * 1024 * 1024));
    let same_service =
        ExportService::with_deps(journal.clone(), same_staging, Arc::clone(&clock), same_disk);
    same_service.trigger_estimate_if_needed(true);
    let mut same_est = Value::Null;
    for _ in 0..100 {
        same_est = same_service.estimate_payload();
        if same_est["status"] == "ready" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(same_est["status"], "ready");
    assert_eq!(same_est["staging_volume_same_as_journal"], true);
    assert_eq!(same_est["journal_disk_free_bytes"], 99_586_048u64);
    let same_full_bytes = same_est["breakdown"]["full_bytes"].as_u64().unwrap();
    assert_eq!(
        same_est["full_required_bytes"],
        same_full_bytes + super::staging::staging_disk_floor_bytes(96 * 1024 * 1024)
    );

    // 2. Sibling staging directory on a DISTINCT parent volume (e.g. 96 MiB mounted
    //    journal beneath a 256 MiB writable parent holding `.journal.export-staging`).
    let distinct_staging = StagingArea::resolve_with_volume_check(&journal, |j, s| {
        assert_eq!(s, expected_sibling.as_path());
        assert_ne!(j, s);
        false
    })
    .unwrap();
    assert_eq!(distinct_staging.dir, expected_sibling);
    assert!(
        !distinct_staging.same_volume_as_journal,
        "sibling staging on a distinct parent volume must report same_volume_as_journal = false"
    );

    let parent_total = 256 * 1024 * 1024u64;
    let parent_free = 268_431_360u64;
    let distinct_disk: Arc<dyn DiskSpaceProbe> =
        Arc::new(MockDiskSpace::new(parent_free, parent_total));
    let distinct_service =
        ExportService::with_deps(journal.clone(), distinct_staging, clock, distinct_disk);
    distinct_service.trigger_estimate_if_needed(true);
    let mut distinct_est = Value::Null;
    for _ in 0..100 {
        distinct_est = distinct_service.estimate_payload();
        if distinct_est["status"] == "ready" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(distinct_est["status"], "ready");
    assert_eq!(
        distinct_est["staging_volume_same_as_journal"], false,
        "distinct-mount sibling must not label parent volume space as journal drive"
    );
    assert_eq!(distinct_est["journal_disk_free_bytes"], parent_free);
    assert_eq!(distinct_est["journal_disk_total_bytes"], parent_total);
    let distinct_full_bytes = distinct_est["breakdown"]["full_bytes"].as_u64().unwrap();
    assert_eq!(
        distinct_est["full_required_bytes"],
        distinct_full_bytes + super::staging::staging_disk_floor_bytes(parent_total),
        "reserve must apply to the actual volume holding the staged ZIP"
    );
}

struct BarrierTrashKind {
    entered_tx: std::sync::Mutex<Option<std::sync::mpsc::Sender<()>>>,
    release_rx: std::sync::Mutex<Option<std::sync::mpsc::Receiver<()>>>,
}

impl Kind for BarrierTrashKind {
    fn handles(&self, unit: &Unit) -> bool {
        matches!(unit, Unit::Entity)
    }

    fn hold<'a>(&'a self, _journal: &Path, _item: &Item) -> Result<Box<dyn Hold + 'a>, String> {
        Ok(Box::new(NoopHold))
    }

    fn present(&self, journal: &Path, path: &str) -> bool {
        journal.join(path).exists()
    }

    fn references(
        &self,
        _journal: &Path,
        _item: &Item,
        _change: Change,
        _files: &[String],
    ) -> Result<(), String> {
        Ok(())
    }

    fn erasing(&self, _journal: &Path, _item: &Item) -> Result<(), String> {
        if let Some(tx) = self
            .entered_tx
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            let _ = tx.send(());
        }
        if let Some(rx) = self
            .release_rx
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            let _ = rx.recv();
        }
        Ok(())
    }
}

#[tokio::test]
async fn permanent_erasure_barrier_before_epoch_bump_classifies_as_erased_invalidated_without_timing_waits()
 {
    let work = TempDir::new().unwrap();
    let journal = work.path().join("journal");
    let staging = work.path().join("staging");
    seed_journal(&journal);

    let clock = Arc::new(MockClock::new(1_764_150_000));
    let disk = Arc::new(MockDiskSpace::new(
        50 * 1024 * 1024 * 1024,
        100 * 1024 * 1024 * 1024,
    ));
    let service = make_service(&journal, &staging, Arc::clone(&clock), Arc::clone(&disk));

    // Prepare an initial ready copy and place one entity into Trash (State::Trashed).
    service
        .start_job(RequesterScope::Local, ExportMode::Full, false)
        .unwrap();
    let ready_initial = wait_until_not_preparing(&service, &RequesterScope::Local).await;
    assert_eq!(ready_initial["status"], "ready");
    let initial_id = ready_initial["ready"]["export_id"]
        .as_str()
        .unwrap()
        .to_owned();

    // Populate 100 State::Trashed items so every check proves O(1) bounded work
    // with populated Trash rather than scanning .trash/*/item.json.
    for idx in 0..100 {
        let p = journal.join(format!("entities/trashed-filler-{idx:03}/entity.json"));
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, format!(r#"{{"id":"trashed-filler-{idx:03}"}}"#)).unwrap();
        let _ = detach(
            &journal,
            &FakeTrashKind,
            Unit::Entity,
            Action::EntityDelete,
            web_surface(),
            vec![format!("entities/trashed-filler-{idx:03}")],
        )
        .unwrap();
    }

    let entity_path = journal.join("entities/barrier-target/entity.json");
    fs::create_dir_all(entity_path.parent().unwrap()).unwrap();
    fs::write(&entity_path, br#"{"id":"barrier-target"}"#).unwrap();
    let trashed_item = detach(
        &journal,
        &FakeTrashKind,
        Unit::Entity,
        Action::EntityDelete,
        web_surface(),
        vec!["entities/barrier-target".into()],
    )
    .unwrap();
    let epoch_before = solstone_core_trash::read_erasure_epoch(&journal).unwrap();

    // 101 ordinary State::Trashed items must NOT invalidate exports.
    assert!(!super::permanent_erasure_invalidated(
        &journal,
        &epoch_before
    ));
    assert_eq!(
        service.state_payload(&RequesterScope::Local)["status"],
        "ready"
    );

    // Start permanent erase on a background thread and block inside Kind::erasing()
    // BEFORE backup work or file unlinking finishes. Trash must already have advanced
    // the O(1) erasure epoch and set erasing=true before Kind::erasing() ran.
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let journal_for_erase = journal.clone();
    let item_id = trashed_item.id.clone();
    let erase_handle = std::thread::spawn(move || {
        let barrier_kind = BarrierTrashKind {
            entered_tx: std::sync::Mutex::new(Some(entered_tx)),
            release_rx: std::sync::Mutex::new(Some(release_rx)),
        };
        erase(
            &journal_for_erase,
            &[&barrier_kind],
            &item_id,
            Action::Erase,
            web_surface(),
        )
    });

    entered_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("erase must reach Kind::erasing barrier");

    // Epoch already advanced and erasing=true is visible in O(1) while Kind::erasing()
    // remains blocked indefinitely before any files are unlinked.
    let epoch_during = solstone_core_trash::read_erasure_epoch(&journal).unwrap();
    assert_eq!(epoch_during.epoch, epoch_before.epoch + 1);
    assert!(
        epoch_during.erasing,
        "erasing flag must be set while Kind::erasing barrier is held"
    );

    // 1. Existing ready copy is invalidated immediately while Kind::erasing is blocked.
    let dl_while_blocked = service
        .serve_download(
            &RequesterScope::Local,
            &initial_id,
            &Method::GET,
            &HeaderMap::new(),
        )
        .await;
    assert_eq!(dl_while_blocked.status(), StatusCode::GONE);
    assert!(!staging.join(format!("{initial_id}.zip")).exists());

    // 2. Starting a new prepare while permanent erasure is still held in Kind::erasing
    //    classifies deterministically as erased_invalidated (never source_changed).
    service
        .start_job(RequesterScope::Local, ExportMode::Full, false)
        .unwrap();
    let outcome_while_blocked = wait_until_not_preparing(&service, &RequesterScope::Local).await;
    assert_eq!(
        outcome_while_blocked["status"], "erased_invalidated",
        "outcome_while_blocked={outcome_while_blocked}"
    );
    assert_eq!(
        outcome_while_blocked["active_job"]["error_code"],
        "export_invalidated_by_erasure"
    );
    assert!(outcome_while_blocked["ready"].is_null());
    assert!(
        solstone_core_trash::read_erasure_epoch(&journal)
            .unwrap()
            .erasing
    );

    // Release the barrier and confirm erase settles erasing=false without double-bumping epoch.
    release_tx.send(()).unwrap();
    erase_handle.join().unwrap().unwrap();
    let epoch_settled = solstone_core_trash::read_erasure_epoch(&journal).unwrap();
    assert_eq!(epoch_settled.epoch, epoch_before.epoch + 1);
    assert_eq!(epoch_settled.token, epoch_during.token);
    assert!(!epoch_settled.erasing);

    // 3. Mid-encode permanent erase held behind the barrier (even if a source mutation occurs
    //    at the exact same moment before bump_erasure_epoch runs) classifies as erased_invalidated.
    let entity2_path = journal.join("entities/barrier-target-2/entity.json");
    fs::create_dir_all(entity2_path.parent().unwrap()).unwrap();
    fs::write(&entity2_path, br#"{"id":"barrier-target-2"}"#).unwrap();
    let trashed_item2 = detach(
        &journal,
        &FakeTrashKind,
        Unit::Entity,
        Action::EntityDelete,
        web_surface(),
        vec!["entities/barrier-target-2".into()],
    )
    .unwrap();
    let victim2 = journal.join("notes/zzz-erasure-race-victim.bin");
    fs::write(&victim2, vec![0x33u8; 32 * 1024]).unwrap();
    let epoch_before_2 = solstone_core_trash::read_erasure_epoch(&journal).unwrap();

    let (entered2_tx, entered2_rx) = std::sync::mpsc::channel();
    let (release2_tx, release2_rx) = std::sync::mpsc::channel();
    type EraseHandle = std::thread::JoinHandle<Result<(), solstone_core_trash::TrashError>>;
    let erase2_handle_slot: Arc<std::sync::Mutex<Option<EraseHandle>>> =
        Arc::new(std::sync::Mutex::new(None));

    struct OnEncodeProbe {
        calls: AtomicU64,
        action: std::sync::Mutex<Option<Box<dyn FnOnce() + Send>>>,
    }
    impl DiskSpaceProbe for OnEncodeProbe {
        fn read(&self, _staging: &StagingArea) -> std::io::Result<JournalDiskSpace> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            // Call 0 is start_job preflight; call 1 runs inside encode_archive_with_options'
            // check_abort hook after open_live has frozen the inventory.
            if n == 1
                && let Some(action) = self.action.lock().unwrap_or_else(|e| e.into_inner()).take()
            {
                action();
            }
            Ok(JournalDiskSpace {
                free_bytes: 50 * 1024 * 1024 * 1024,
                total_bytes: 100 * 1024 * 1024 * 1024,
            })
        }
    }

    let journal_for_hook = journal.clone();
    let victim2_for_hook = victim2.clone();
    let item2_id = trashed_item2.id.clone();
    let handle_slot_for_hook = Arc::clone(&erase2_handle_slot);
    let mid_encode_probe: Arc<dyn DiskSpaceProbe> = Arc::new(OnEncodeProbe {
        calls: AtomicU64::new(0),
        action: std::sync::Mutex::new(Some(Box::new(move || {
            let j = journal_for_hook.clone();
            let h = std::thread::spawn(move || {
                let barrier_kind = BarrierTrashKind {
                    entered_tx: std::sync::Mutex::new(Some(entered2_tx)),
                    release_rx: std::sync::Mutex::new(Some(release2_rx)),
                };
                erase(
                    &j,
                    &[&barrier_kind],
                    &item2_id,
                    Action::Erase,
                    web_surface(),
                )
            });
            *handle_slot_for_hook
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = Some(h);
            entered2_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .expect("mid-encode erase must reach barrier before epoch bump");
            let _ = fs::remove_file(&victim2_for_hook);
        }))),
    });

    let service2 = ExportService::with_deps(
        journal.clone(),
        StagingArea::at_dir(staging.clone(), true).unwrap(),
        Arc::clone(&clock) as Arc<dyn ExportClock>,
        mid_encode_probe,
    );
    service2
        .start_job(RequesterScope::Local, ExportMode::Full, false)
        .unwrap();
    let outcome_mid_encode = wait_until_not_preparing(&service2, &RequesterScope::Local).await;
    assert_eq!(
        outcome_mid_encode["status"], "erased_invalidated",
        "mid-encode erase blocked before epoch bump must classify as erased_invalidated; outcome={outcome_mid_encode}"
    );
    assert_eq!(
        outcome_mid_encode["active_job"]["error_code"],
        "export_invalidated_by_erasure"
    );
    let epoch_during_2 = solstone_core_trash::read_erasure_epoch(&journal).unwrap();
    assert_eq!(
        epoch_during_2.epoch,
        epoch_before_2.epoch + 1,
        "epoch must already advance before Kind::erasing completes"
    );
    assert!(epoch_during_2.erasing);

    release2_tx.send(()).unwrap();
    erase2_handle_slot
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
        .expect("mid-encode erase thread spawned")
        .join()
        .unwrap()
        .unwrap();
    let epoch_settled_2 = solstone_core_trash::read_erasure_epoch(&journal).unwrap();
    assert_eq!(epoch_settled_2.epoch, epoch_before_2.epoch + 1);
    assert_eq!(epoch_settled_2.token, epoch_during_2.token);
    assert!(!epoch_settled_2.erasing);

    // 4. Subsequent prepare with 100 State::Trashed items still in .trash/ succeeds cleanly.
    service
        .start_job(RequesterScope::Local, ExportMode::Full, false)
        .unwrap();
    let ready_after = wait_until_not_preparing(&service, &RequesterScope::Local).await;
    assert_eq!(ready_after["status"], "ready");
    assert!(ready_after["ready"].is_object());
}

#[tokio::test]
async fn ordinary_non_erasure_source_changed_remains_failed_source_changed_even_with_trashed_item()
{
    let work = TempDir::new().unwrap();
    let journal = work.path().join("journal");
    let staging = work.path().join("staging");
    seed_journal(&journal);

    // Keep 100 State::Trashed items present (not permanently erasing) to ensure populated
    // Trash never misclassifies an ordinary non-erasure SourceChanged or triggers .trash scans.
    for idx in 0..100 {
        let trashed_src = journal.join(format!("entities/kept-in-trash-{idx:03}/entity.json"));
        fs::create_dir_all(trashed_src.parent().unwrap()).unwrap();
        fs::write(
            &trashed_src,
            format!(r#"{{"id":"kept-in-trash-{idx:03}"}}"#),
        )
        .unwrap();
        let _trashed = detach(
            &journal,
            &FakeTrashKind,
            Unit::Entity,
            Action::EntityDelete,
            web_surface(),
            vec![format!("entities/kept-in-trash-{idx:03}")],
        )
        .unwrap();
    }

    let victim = journal.join("notes/zzz-victim.bin");
    fs::write(&victim, vec![0x5Au8; 64 * 1024]).unwrap();

    struct DeleteVictimDuringEncodeProbe {
        calls: AtomicU64,
        victim: std::path::PathBuf,
        deleted: Arc<std::sync::atomic::AtomicBool>,
    }
    impl DiskSpaceProbe for DeleteVictimDuringEncodeProbe {
        fn read(&self, _staging: &StagingArea) -> std::io::Result<JournalDiskSpace> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            // Call 0 is start_job preflight; call 1 runs inside encode_archive_with_options'
            // check_abort hook after open_live has frozen `notes/zzz-victim.bin` in inventory.
            if n == 1 {
                fs::remove_file(&self.victim).unwrap();
                self.deleted.store(true, Ordering::SeqCst);
            }
            Ok(JournalDiskSpace {
                free_bytes: 50 * 1024 * 1024 * 1024,
                total_bytes: 100 * 1024 * 1024 * 1024,
            })
        }
    }

    let deleted = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let clock = Arc::new(MockClock::new(1_764_150_000));
    let disk: Arc<dyn DiskSpaceProbe> = Arc::new(DeleteVictimDuringEncodeProbe {
        calls: AtomicU64::new(0),
        victim: victim.clone(),
        deleted: Arc::clone(&deleted),
    });
    let service = ExportService::with_deps(
        journal.clone(),
        StagingArea::at_dir(staging.clone(), true).unwrap(),
        clock,
        disk,
    );
    service
        .start_job(RequesterScope::Local, ExportMode::Full, false)
        .unwrap();

    let outcome = wait_until_not_preparing(&service, &RequesterScope::Local).await;
    assert!(
        deleted.load(Ordering::SeqCst),
        "expected deterministic hook to remove victim mid-encode; outcome={outcome}"
    );
    assert_eq!(outcome["status"], "failed", "outcome={outcome}");
    assert_eq!(
        outcome["active_job"]["error_code"], "source_changed",
        "ordinary non-erasure mutation must stay source_changed; outcome={outcome}"
    );
    assert!(outcome["ready"].is_null());
}
