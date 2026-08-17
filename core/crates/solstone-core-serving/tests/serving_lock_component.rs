// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use serde_json::json;
use solstone_core_entity::{
    EntityResolutionEntity, hold_entity_trust_lock, record_entity_resolution, save_entity_identity,
};
use solstone_core_facets::{create_facet, hold_facet_trust_lock};
use solstone_core_serving::seam::run_blocking;

const WAIT: Duration = Duration::from_secs(1);

static NEXT_TEMP_DIR: AtomicU64 = AtomicU64::new(0);

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        let sequence = NEXT_TEMP_DIR.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "solstone-core-serving-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

type Acquire<G, E> = fn(&Path) -> Result<G, E>;

fn assert_contender_acquires<G, E>(root: PathBuf, acquire: Acquire<G, E>)
where
    E: Error + Send + 'static,
    G: 'static,
{
    let (acquired_tx, acquired_rx) = mpsc::channel();
    let contender = thread::spawn(move || {
        let guard = acquire(&root).expect("separate-thread contender acquires trust lock");
        acquired_tx.send(()).unwrap();
        drop(guard);
    });

    acquired_rx
        .recv_timeout(WAIT)
        .expect("separate-thread contender acquires after release");
    contender.join().unwrap();
}

async fn normal_completion_release<G, E>(
    root: PathBuf,
    lock_relative_path: &'static str,
    acquire: Acquire<G, E>,
) where
    E: Error + Send + 'static,
    G: 'static,
{
    let test_thread = thread::current().id();
    let (held_tx, held_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let operation_root = root.clone();
    let operation = tokio::spawn(async move {
        run_blocking(move || {
            let guard = acquire(&operation_root).expect("operation acquires trust lock");
            assert!(
                operation_root
                    .join(lock_relative_path)
                    .with_added_extension("lock")
                    .is_file()
            );
            held_tx.send(thread::current().id()).unwrap();
            release_rx.recv().unwrap();
            drop(guard);
        })
        .await
        .unwrap();
    });

    let blocking_thread = held_rx
        .recv_timeout(WAIT)
        .expect("blocking operation signals acquisition");
    assert_ne!(blocking_thread, test_thread);

    let independent = tokio::spawn(async {
        tokio::time::sleep(Duration::from_millis(10)).await;
    });
    tokio::time::timeout(WAIT, independent)
        .await
        .expect("independent Tokio future makes progress")
        .unwrap();

    release_tx.send(()).unwrap();
    operation.await.unwrap();
    assert_contender_acquires(root, acquire);
}

async fn refusal_release<G, E>(root: PathBuf, acquire: Acquire<G, E>)
where
    E: Error + Send + 'static,
    G: 'static,
{
    let operation_root = root.clone();
    let refusal = run_blocking(move || -> Result<(), &'static str> {
        let guard = acquire(&operation_root).expect("refusal path acquires trust lock");
        drop(guard);
        Err("refused")
    })
    .await
    .unwrap();

    assert_eq!(refusal, Err("refused"));
    assert_contender_acquires(root, acquire);
}

async fn panic_release<G, E>(root: PathBuf, acquire: Acquire<G, E>)
where
    E: Error + Send + 'static,
    G: 'static,
{
    let (held_tx, held_rx) = mpsc::channel();
    let operation_root = root.clone();
    let result = run_blocking(move || {
        let _guard = acquire(&operation_root).expect("error path acquires trust lock");
        held_tx.send(()).unwrap();
        panic!("after acquisition");
    })
    .await;

    held_rx
        .recv_timeout(WAIT)
        .expect("panic path held the trust lock before failing");
    assert!(result.expect_err("panic becomes JoinError").is_panic());
    assert_contender_acquires(root, acquire);
}

async fn cancellation_release<G, E>(root: PathBuf, acquire: Acquire<G, E>)
where
    E: Error + Send + 'static,
    G: 'static,
{
    let (held_tx, held_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let operation_root = root.clone();
    let caller = tokio::spawn(async move {
        run_blocking(move || {
            let guard = acquire(&operation_root).expect("cancelled caller acquires trust lock");
            held_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            drop(guard);
        })
        .await
    });

    held_rx
        .recv_timeout(WAIT)
        .expect("cancelled caller signals acquisition");
    caller.abort();
    assert!(caller.await.expect_err("caller aborts").is_cancelled());

    // spawn_blocking work is not preempted by cancellation; releasing its latch
    // lets it complete independently and drop the guard on its worker thread.
    release_tx.send(()).unwrap();
    assert_contender_acquires(root, acquire);
}

async fn seam_contention_and_reentrancy<G, E>(
    root: PathBuf,
    lock_relative_path: &'static str,
    acquire: Acquire<G, E>,
) where
    E: Error + Send + 'static,
    G: 'static,
{
    let (held_tx, held_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let holder_root = root.clone();
    let holder = tokio::spawn(async move {
        run_blocking(move || {
            let outer = acquire(&holder_root).expect("holder acquires outer guard");
            let inner = acquire(&holder_root).expect("holder reenters on its same thread");
            assert!(
                holder_root
                    .join(lock_relative_path)
                    .with_added_extension("lock")
                    .is_file()
            );
            held_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            drop(inner);
            drop(outer);
        })
        .await
        .unwrap();
    });

    held_rx
        .recv_timeout(WAIT)
        .expect("seam holder acquired nested guards");

    let (started_tx, started_rx) = mpsc::channel();
    let (acquired_tx, acquired_rx) = mpsc::channel();
    let contender_root = root.clone();
    let contender = tokio::spawn(async move {
        run_blocking(move || {
            started_tx.send(()).unwrap();
            let guard = acquire(&contender_root).expect("seam contender acquires after holder");
            acquired_tx.send(()).unwrap();
            drop(guard);
        })
        .await
        .unwrap();
    });

    started_rx
        .recv_timeout(WAIT)
        .expect("seam contender started before acquisition");
    assert!(
        acquired_rx.try_recv().is_err(),
        "contender must not acquire while the holder is still live"
    );
    release_tx.send(()).unwrap();
    acquired_rx
        .recv_timeout(WAIT)
        .expect("seam contender acquires after outer guard drops");
    tokio::time::timeout(WAIT, holder).await.unwrap().unwrap();
    tokio::time::timeout(WAIT, contender)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn entity_normal_completion_releases_to_a_separate_thread() {
    let temporary = TempDir::new();
    normal_completion_release(
        temporary.path().to_owned(),
        "health/locks/entity-trust",
        hold_entity_trust_lock,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn facet_normal_completion_releases_to_a_separate_thread() {
    let temporary = TempDir::new();
    normal_completion_release(
        temporary.path().to_owned(),
        "health/locks/facet-trust",
        hold_facet_trust_lock,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn entity_refusal_releases_to_a_separate_thread() {
    let temporary = TempDir::new();
    refusal_release(temporary.path().to_owned(), hold_entity_trust_lock).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn facet_refusal_releases_to_a_separate_thread() {
    let temporary = TempDir::new();
    refusal_release(temporary.path().to_owned(), hold_facet_trust_lock).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn entity_error_after_acquisition_releases_to_a_separate_thread() {
    let temporary = TempDir::new();
    panic_release(temporary.path().to_owned(), hold_entity_trust_lock).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn facet_error_after_acquisition_releases_to_a_separate_thread() {
    let temporary = TempDir::new();
    panic_release(temporary.path().to_owned(), hold_facet_trust_lock).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn entity_cancellation_releases_to_a_separate_thread() {
    let temporary = TempDir::new();
    cancellation_release(temporary.path().to_owned(), hold_entity_trust_lock).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn facet_cancellation_releases_to_a_separate_thread() {
    let temporary = TempDir::new();
    cancellation_release(temporary.path().to_owned(), hold_facet_trust_lock).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn entity_seam_contention_waits_and_reenters() {
    let temporary = TempDir::new();
    seam_contention_and_reentrancy(
        temporary.path().to_owned(),
        "health/locks/entity-trust",
        hold_entity_trust_lock,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn facet_seam_contention_waits_and_reenters() {
    let temporary = TempDir::new();
    seam_contention_and_reentrancy(
        temporary.path().to_owned(),
        "health/locks/facet-trust",
        hold_facet_trust_lock,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn entity_resolution_reenters_through_the_seam() {
    let temporary = TempDir::new();
    let root = temporary.path().to_owned();
    let resolution = run_blocking(move || {
        record_entity_resolution(
            &root,
            "Sarah",
            &[
                EntityResolutionEntity {
                    id: Some("sarah-connor".to_owned()),
                    name: "Sarah Connor".to_owned(),
                    aka: vec![],
                    emails: vec![],
                    blocked: false,
                },
                EntityResolutionEntity {
                    id: Some("sarah-lee".to_owned()),
                    name: "Sarah Lee".to_owned(),
                    aka: vec![],
                    emails: vec![],
                    blocked: false,
                },
            ],
            json!({"kind": "journal"}),
            json!({"lane": "serving-seam", "name": "reentrancy"}),
            90.0,
            false,
        )
    })
    .await
    .unwrap()
    .unwrap();

    assert_eq!(
        resolution.outcome,
        solstone_core_entity::EntityResolutionOutcome::Ambiguous
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn save_entity_identity_runs_through_the_seam() {
    let temporary = TempDir::new();
    let root = temporary.path().to_owned();
    let saved =
        run_blocking(move || save_entity_identity(&root, "alice", &json!({"name": "Alice"}), None))
            .await
            .unwrap()
            .unwrap();

    assert!(saved.changed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_facet_runs_through_the_seam() {
    let temporary = TempDir::new();
    let root = temporary.path().to_owned();
    run_blocking(move || create_facet(&root, "work", "Work", "", "blue", "", None))
        .await
        .unwrap()
        .unwrap();
}
