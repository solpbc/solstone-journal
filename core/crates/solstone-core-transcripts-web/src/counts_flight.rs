// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! `complete: true` covers every change whose stream-marker bump committed before the read, and every change to a live day. A change on an older day whose producer committed but did not bump appears within about an hour of it while the calendar is in use.
//!
//! A queued request waits for the in-flight run and at most one successor. The month route's wait is that bound plus the folds of its own month (at most 31 days).

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::{Arc, LazyLock, Mutex};

use axum::body::Body;
use axum::http::StatusCode;
use axum::response::Response;
use serde_json::Value;
use solstone_core_convey_http::owner_read::{OwnerReadRole, join_failed, spawn_blocking_response};
use solstone_core_journal_io::{JournalRoot, ObjectIdentity};
use tokio::sync::oneshot;

use crate::Clock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RequestRole {
    Index,
    MonthStats,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Disposition {
    Immediate,
    Queued,
}

impl Disposition {
    #[allow(dead_code)]
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::Immediate => "immediate",
            Self::Queued => "queued",
        }
    }
}

#[derive(Clone)]
pub(crate) struct InventorySnapshot {
    pub(crate) counts: Arc<BTreeMap<String, u64>>,
    pub(crate) freshness: Value,
}

pub(crate) enum Delivery {
    Ready {
        counts: Arc<BTreeMap<String, u64>>,
        freshness: Value,
        disposition: Disposition,
    },
    Failed {
        response: Response,
        disposition: Disposition,
    },
}

#[derive(Clone)]
enum FailedOutcome {
    JoinFailed,
}

impl FailedOutcome {
    fn into_response(self) -> Response {
        match self {
            Self::JoinFailed => join_failed(),
        }
    }
}

type FlightResult = Result<Arc<InventorySnapshot>, FailedOutcome>;
type WaiterSender = oneshot::Sender<FlightResult>;

struct FlightSlot {
    pending: Option<PendingCohort>,
}

struct PendingCohort {
    role: RequestRole,
    waiters: Vec<WaiterSender>,
}

static FLIGHTS: LazyLock<Mutex<HashMap<ObjectIdentity, FlightSlot>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub(crate) async fn execute(
    admitted: Option<Arc<JournalRoot>>,
    fallback: &Path,
    clock: &Clock,
    role: RequestRole,
) -> Delivery {
    let Some(root) = admitted else {
        // Unshared run against fallback
        let scan_path = fallback.to_path_buf();
        let owner_role = match role {
            RequestRole::Index => OwnerReadRole::TranscriptsIndex,
            RequestRole::MonthStats => OwnerReadRole::TranscriptsMonthStats,
        };
        let clock = clock.clone();
        let (tx, rx) = oneshot::channel::<FlightResult>();
        let _ = spawn_blocking_response(owner_role, move || {
            let now = clock.now();
            let (counts, freshness) =
                crate::inventory::read_inventory_unadmitted(&scan_path, now, &clock, role);
            let _ = tx.send(Ok(Arc::new(InventorySnapshot { counts, freshness })));
            Response::builder()
                .status(StatusCode::OK)
                .body(Body::empty())
                .unwrap()
        })
        .await;
        return match rx.await {
            Ok(Ok(snapshot)) => Delivery::Ready {
                counts: snapshot.counts.clone(),
                freshness: snapshot.freshness.clone(),
                disposition: Disposition::Immediate,
            },
            Ok(Err(failed)) => Delivery::Failed {
                response: failed.into_response(),
                disposition: Disposition::Immediate,
            },
            Err(_) => Delivery::Failed {
                response: join_failed(),
                disposition: Disposition::Immediate,
            },
        };
    };

    let key = root.identity();
    let (tx, rx) = oneshot::channel();

    let (disposition, opener) = {
        let mut reg = FLIGHTS.lock().expect("flights lock");
        if let Some(slot) = reg.get_mut(&key) {
            if let Some(cohort) = &mut slot.pending {
                cohort.waiters.push(tx);
            } else {
                slot.pending = Some(PendingCohort {
                    role,
                    waiters: vec![tx],
                });
            }
            (Disposition::Queued, None)
        } else {
            reg.insert(key, FlightSlot { pending: None });
            (Disposition::Immediate, Some(tx))
        }
    };

    #[cfg(test)]
    if disposition == Disposition::Queued {
        test_support::note_queued(key);
    }

    if let Some(opener_tx) = opener {
        spawn_flight(root, clock.clone(), role, vec![opener_tx]);
    }

    match rx.await {
        Ok(Ok(snapshot)) => Delivery::Ready {
            counts: snapshot.counts.clone(),
            freshness: snapshot.freshness.clone(),
            disposition,
        },
        Ok(Err(failed)) => Delivery::Failed {
            response: failed.into_response(),
            disposition,
        },
        Err(_) => Delivery::Failed {
            response: join_failed(),
            disposition,
        },
    }
}

fn spawn_flight(
    root: Arc<JournalRoot>,
    clock: Clock,
    role: RequestRole,
    waiters: Vec<WaiterSender>,
) {
    let key = root.identity();
    tokio::spawn(async move {
        #[cfg(test)]
        test_support::increment_run_count(key);

        let owner_role = match role {
            RequestRole::Index => OwnerReadRole::TranscriptsIndex,
            RequestRole::MonthStats => OwnerReadRole::TranscriptsMonthStats,
        };

        struct FlightDropGuard {
            root: Arc<JournalRoot>,
            clock: Clock,
            waiters: Option<Vec<WaiterSender>>,
            disarmed: bool,
        }

        impl Drop for FlightDropGuard {
            fn drop(&mut self) {
                if self.disarmed {
                    return;
                }
                let key = self.root.identity();
                let next = {
                    let mut reg = FLIGHTS.lock().expect("flights lock");
                    if let Some(slot) = reg.get_mut(&key) {
                        let pending = slot.pending.take();
                        if pending.is_none() {
                            reg.remove(&key);
                        }
                        pending
                    } else {
                        None
                    }
                };
                if let Some(waiters) = self.waiters.take() {
                    for tx in waiters {
                        let _ = tx.send(Err(FailedOutcome::JoinFailed));
                    }
                }
                if let Some(cohort) = next {
                    spawn_flight(
                        self.root.clone(),
                        self.clock.clone(),
                        cohort.role,
                        cohort.waiters,
                    );
                }
            }
        }

        let mut guard = FlightDropGuard {
            root: root.clone(),
            clock: clock.clone(),
            waiters: Some(waiters),
            disarmed: false,
        };

        let (scan_tx, scan_rx) = oneshot::channel();
        let root_c = root.clone();
        let clock_c = clock.clone();

        let _ = spawn_blocking_response(owner_role, move || {
            let now = clock_c.now();
            #[cfg(test)]
            test_support::record_instant(key, now);

            let (counts, freshness) =
                crate::inventory::read_inventory_admitted(root_c, now, &clock_c, role);
            #[cfg(test)]
            test_support::run_pauser(key);
            let _ = scan_tx.send(Ok(Arc::new(InventorySnapshot { counts, freshness })));
            Response::builder()
                .status(StatusCode::OK)
                .body(Body::empty())
                .unwrap()
        })
        .await;

        let final_outcome = match scan_rx.await {
            Ok(outcome) => outcome,
            Err(_) => Err(FailedOutcome::JoinFailed),
        };

        // Hand off under mutex before answering waiters
        let next = {
            let mut reg = FLIGHTS.lock().expect("flights lock");
            if let Some(slot) = reg.get_mut(&key) {
                let pending = slot.pending.take();
                if pending.is_none() {
                    reg.remove(&key);
                }
                pending
            } else {
                None
            }
        };

        guard.disarmed = true;
        let waiters = guard.waiters.take().unwrap_or_default();

        if let Some(cohort) = next {
            spawn_flight(root, clock, cohort.role, cohort.waiters);
        }

        // Answer current waiters
        for tx in waiters {
            let _ = tx.send(final_outcome.clone());
        }
    });
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::collections::HashMap;
    use std::sync::{Arc, LazyLock, Mutex};

    use chrono::{DateTime, Utc};
    use solstone_core_journal_io::ObjectIdentity;
    use tokio::sync::Notify;

    #[derive(Default)]
    struct State {
        pausers: HashMap<ObjectIdentity, Arc<dyn Fn() + Send + Sync>>,
        queued_notifies: HashMap<ObjectIdentity, Arc<Notify>>,
        queued_counts: HashMap<ObjectIdentity, usize>,
        bound_instants: HashMap<ObjectIdentity, Vec<DateTime<Utc>>>,
        run_counts: HashMap<ObjectIdentity, usize>,
    }

    static STATE: LazyLock<Mutex<State>> = LazyLock::new(|| Mutex::new(State::default()));

    pub(crate) fn install_pauser(
        identity: ObjectIdentity,
        pauser: impl Fn() + Send + Sync + 'static,
    ) {
        let mut s = STATE.lock().unwrap();
        s.pausers.insert(identity, Arc::new(pauser));
    }

    pub(crate) fn queued_notify(identity: ObjectIdentity) -> Arc<Notify> {
        let mut s = STATE.lock().unwrap();
        s.queued_notifies.entry(identity).or_default().clone()
    }

    pub(crate) fn note_queued(identity: ObjectIdentity) {
        let notify = {
            let mut s = STATE.lock().unwrap();
            *s.queued_counts.entry(identity).or_default() += 1;
            s.queued_notifies.get(&identity).cloned()
        };
        if let Some(n) = notify {
            n.notify_waiters();
        }
    }

    pub(crate) fn queued_count(identity: ObjectIdentity) -> usize {
        let s = STATE.lock().unwrap();
        s.queued_counts.get(&identity).copied().unwrap_or(0)
    }

    pub(crate) fn bound_instants(identity: ObjectIdentity) -> Vec<DateTime<Utc>> {
        let s = STATE.lock().unwrap();
        s.bound_instants.get(&identity).cloned().unwrap_or_default()
    }

    pub(crate) fn record_instant(identity: ObjectIdentity, instant: DateTime<Utc>) {
        let mut s = STATE.lock().unwrap();
        s.bound_instants.entry(identity).or_default().push(instant);
    }

    pub(crate) fn run_count(identity: ObjectIdentity) -> usize {
        let s = STATE.lock().unwrap();
        s.run_counts.get(&identity).copied().unwrap_or(0)
    }

    pub(crate) fn increment_run_count(identity: ObjectIdentity) {
        let mut s = STATE.lock().unwrap();
        *s.run_counts.entry(identity).or_default() += 1;
    }

    pub(crate) fn run_pauser(identity: ObjectIdentity) {
        let pauser = {
            let s = STATE.lock().unwrap();
            s.pausers.get(&identity).cloned()
        };
        if let Some(p) = pauser {
            p();
        }
    }

    pub(crate) fn reset(identity: ObjectIdentity) {
        let mut s = STATE.lock().unwrap();
        s.pausers.remove(&identity);
        s.queued_notifies.remove(&identity);
        s.queued_counts.remove(&identity);
        s.bound_instants.remove(&identity);
        s.run_counts.remove(&identity);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use chrono::{TimeZone, Utc};
    use serde_json::Value;
    use solstone_core_journal_io::{JournalRoot, ObjectIdentity, day_dirs, day_path};
    use solstone_core_journal_stats_cli::{DayCacheWriter, DayScan, FilesystemDayCacheWriter};
    use solstone_core_segment::write_content;
    use solstone_core_segment::{ContentName, ContentWriteOutcome, SegmentDir};
    use tower::ServiceExt;

    use super::*;
    use crate::calendar::day_range_count;

    struct TestResetGuard(ObjectIdentity);
    impl Drop for TestResetGuard {
        fn drop(&mut self) {
            test_support::reset(self.0);
            crate::inventory::drop_inventory(self.0);
        }
    }

    fn copy_dir_all(src: &Path, dst: &Path) {
        std::fs::create_dir_all(dst).unwrap();
        for entry in std::fs::read_dir(src).unwrap() {
            let entry = entry.unwrap();
            let ty = entry.file_type().unwrap();
            let src_path = entry.path();
            let dst_path = dst.join(entry.file_name());
            if ty.is_dir() {
                copy_dir_all(&src_path, &dst_path);
            } else if ty.is_file() {
                std::fs::copy(&src_path, &dst_path).unwrap();
            }
        }
    }

    fn stats_free_walk(journal: &Path) -> std::collections::BTreeMap<String, u64> {
        let mut aside = Vec::new();
        if let Ok(entries) = std::fs::read_dir(journal.join("chronicle")) {
            for entry in entries.flatten() {
                let stats_path = entry.path().join("stats.json");
                let aside_path = entry.path().join("stats.json.aside");
                if stats_path.is_file() {
                    std::fs::rename(&stats_path, &aside_path).unwrap();
                    aside.push((stats_path, aside_path));
                }
            }
        }
        let walk = crate::calendar::counts_by_walk(journal, chrono::Utc::now()).unwrap();
        for (stats_path, aside_path) in aside {
            std::fs::rename(&aside_path, &stats_path).unwrap();
        }
        walk
    }

    fn setup_temp_journal() -> (tempfile::TempDir, Arc<JournalRoot>) {
        let temp = tempfile::tempdir().unwrap();
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../tests/fixtures/journal");
        copy_dir_all(&fixture, temp.path());

        // Pre-seed day cache for each existing chronicle day
        let days = day_dirs(temp.path()).unwrap();
        let writer = FilesystemDayCacheWriter;
        let future_time = filetime::FileTime::from_system_time(
            std::time::SystemTime::now() + std::time::Duration::from_secs(100),
        );
        for day in days.keys() {
            let count = day_range_count(temp.path(), day, Utc::now()).unwrap();
            let mut scan = DayScan::default();
            scan.schema_version = solstone_core_journal_stats_cli::SCHEMA_VERSION;
            scan.stats.transcript_ranges = count.saturating_add(1_000_000);
            scan.stats.percept_ranges = 0;
            scan.stats.browser_segments = 0;
            let dir = day_path(temp.path(), day, false).unwrap();
            writer
                .write_day_cache(&dir.join("stats.json"), &scan)
                .unwrap();
            filetime::set_file_mtime(dir.join("stats.json"), future_time).unwrap();
        }

        let root = Arc::new(JournalRoot::open(temp.path()).unwrap());
        (temp, root)
    }

    #[tokio::test]
    async fn transcript_index_coalescing_second_generation_soundness() {
        let (temp, root) = setup_temp_journal();
        let identity = root.identity();
        let _guard = TestResetGuard(identity);

        let app = crate::router(temp.path().to_path_buf(), Clock::system(), || {
            Response::new(Body::empty())
        });

        // Solo index before write
        let req_a = Request::get("/app/transcripts/api/index")
            .extension(root.clone())
            .body(Body::empty())
            .unwrap();
        let resp_a = app.clone().oneshot(req_a).await.unwrap();
        assert_eq!(resp_a.status(), StatusCode::OK);
        let body_a_bytes = to_bytes(resp_a.into_body(), usize::MAX).await.unwrap();
        let body_a: Value = serde_json::from_slice(&body_a_bytes).unwrap();

        assert!(body_a["months"].get("202603").is_some());
        assert!(body_a["months"].get("202608").is_none());
        let index_total: u64 = body_a["months"]
            .as_object()
            .unwrap()
            .values()
            .map(|value| value.as_u64().unwrap())
            .sum();
        let walk_total: u64 = stats_free_walk(temp.path()).values().sum();
        assert_eq!(index_total, walk_total);
        assert_ne!(index_total, walk_total.saturating_add(1_000_000));

        // Setup pauser for the first two successful runs only
        let (entered_tx, mut entered_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
        let (p1_tx, p1_rx) = std::sync::mpsc::sync_channel::<()>(0);
        let (p2_tx, p2_rx) = std::sync::mpsc::sync_channel::<()>(0);
        let pause_queue = Arc::new(Mutex::new(VecDeque::from([p1_rx, p2_rx])));

        let pause_queue_c = pause_queue.clone();
        test_support::install_pauser(identity, move || {
            let rx = {
                let mut q = pause_queue_c.lock().unwrap();
                q.pop_front()
            };
            if let Some(rx) = rx {
                entered_tx.send(()).unwrap();
                let _ = rx.recv();
            }
        });

        // R1 starts run 1
        let app1 = app.clone();
        let root1 = root.clone();
        let task_r1 = tokio::spawn(async move {
            let req = Request::get("/app/transcripts/api/index")
                .extension(root1)
                .body(Body::empty())
                .unwrap();
            app1.oneshot(req).await.unwrap()
        });

        // Wait until run 1 enters pauser
        entered_rx.recv().await.unwrap();

        // One or more index requests register in pending set (run 2's cohort)
        let q_not = test_support::queued_notify(identity);
        let app2 = app.clone();
        let root2 = root.clone();
        let task_r2 = tokio::spawn(async move {
            let req = Request::get("/app/transcripts/api/index")
                .extension(root2)
                .body(Body::empty())
                .unwrap();
            app2.oneshot(req).await.unwrap()
        });

        while test_support::queued_count(identity) < 1 {
            q_not.notified().await;
        }

        // Release run 1
        p1_tx.send(()).unwrap();

        // Wait until run 2 enters pauser (snapshot already built)
        entered_rx.recv().await.unwrap();

        // Commit the audio.flac write
        let seg = SegmentDir::resolve(temp.path(), "20260804", "120000_60", "workstation").unwrap();
        let write_res = write_content(
            &seg,
            ContentName::new("audio.flac").unwrap(),
            b"test audio bytes content",
        )
        .unwrap();
        assert!(matches!(write_res, ContentWriteOutcome::Written(_)));
        solstone_core_journal_io::bump_stream_marker(temp.path(), "20260804").unwrap();

        // R3 registers
        let app3 = app.clone();
        let root3 = root.clone();
        let task_r3 = tokio::spawn(async move {
            let req = Request::get("/app/transcripts/api/index")
                .extension(root3)
                .body(Body::empty())
                .unwrap();
            app3.oneshot(req).await.unwrap()
        });

        while test_support::queued_count(identity) < 2 {
            q_not.notified().await;
        }

        // Release run 2 (run 3 will not be paused as queue is empty)
        p2_tx.send(()).unwrap();

        let resp_r1 = task_r1.await.unwrap();
        let resp_r2 = task_r2.await.unwrap();
        let resp_r3 = task_r3.await.unwrap();

        assert_eq!(resp_r1.status(), StatusCode::OK);
        assert_eq!(resp_r2.status(), StatusCode::OK);
        assert_eq!(resp_r3.status(), StatusCode::OK);

        let body_r2 = to_bytes(resp_r2.into_body(), usize::MAX).await.unwrap();
        assert_eq!(body_r2, body_a_bytes);

        let body_r3: Value =
            serde_json::from_slice(&to_bytes(resp_r3.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert!(body_r3["months"].get("202608").is_some());

        // Solo index after everything
        let req_after = Request::get("/app/transcripts/api/index")
            .extension(root.clone())
            .body(Body::empty())
            .unwrap();
        let resp_after = app.clone().oneshot(req_after).await.unwrap();
        let body_after: Value =
            serde_json::from_slice(&to_bytes(resp_after.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert!(body_after["months"].get("202608").is_some());
    }

    #[tokio::test]
    async fn transcript_counts_coalescing_advancing_clock_binds_per_run() {
        let (temp, root) = setup_temp_journal();
        let identity = root.identity();
        let _guard = TestResetGuard(identity);

        let t0 = Utc.with_ymd_and_hms(2026, 3, 1, 10, 0, 0).unwrap();
        let clock_cell = Arc::new(Mutex::new(t0));
        let clock = Clock::new({
            let c = clock_cell.clone();
            move || *c.lock().unwrap()
        });

        let app = crate::router(temp.path().to_path_buf(), clock, || {
            Response::new(Body::empty())
        });

        // Hold only the first run
        let (entered_tx, mut entered_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
        let (p1_tx, p1_rx) = std::sync::mpsc::sync_channel::<()>(0);
        let pause_queue = Arc::new(Mutex::new(VecDeque::from([p1_rx])));

        let pause_queue_c = pause_queue.clone();
        test_support::install_pauser(identity, move || {
            let rx = {
                let mut q = pause_queue_c.lock().unwrap();
                q.pop_front()
            };
            if let Some(rx) = rx {
                entered_tx.send(()).unwrap();
                let _ = rx.recv();
            }
        });

        // R1 starts run 1
        let app1 = app.clone();
        let root1 = root.clone();
        let task_r1 = tokio::spawn(async move {
            let req = Request::get("/app/transcripts/api/index")
                .extension(root1)
                .body(Body::empty())
                .unwrap();
            app1.oneshot(req).await.unwrap()
        });

        entered_rx.recv().await.unwrap();
        assert_eq!(test_support::bound_instants(identity), vec![t0]);

        // Spawn stats/202603 while run 1 is held
        let q_not = test_support::queued_notify(identity);
        let app2 = app.clone();
        let root2 = root.clone();
        let task_r2 = tokio::spawn(async move {
            let req = Request::get("/app/transcripts/api/stats/202603")
                .extension(root2)
                .body(Body::empty())
                .unwrap();
            app2.oneshot(req).await.unwrap()
        });

        while test_support::queued_count(identity) < 1 {
            q_not.notified().await;
        }

        // Only after it is queued, set clock to T1
        let t1 = t0 + chrono::Duration::hours(1);
        *clock_cell.lock().unwrap() = t1;

        // Release run 1
        p1_tx.send(()).unwrap();

        let resp_r1 = task_r1.await.unwrap();
        let resp_r2 = task_r2.await.unwrap();

        assert_eq!(resp_r1.status(), StatusCode::OK);
        assert_eq!(resp_r2.status(), StatusCode::OK);

        assert_eq!(test_support::bound_instants(identity), vec![t0, t1]);
    }

    #[tokio::test]
    async fn transcript_counts_coalescing_fans_in_index_and_stats_across_months() {
        let (temp, root) = setup_temp_journal();
        let identity = root.identity();
        let _guard = TestResetGuard(identity);

        let app = crate::router(temp.path().to_path_buf(), Clock::system(), || {
            Response::new(Body::empty())
        });

        // Hold only the first run
        let (entered_tx, mut entered_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
        let (p1_tx, p1_rx) = std::sync::mpsc::sync_channel::<()>(0);
        let pause_queue = Arc::new(Mutex::new(VecDeque::from([p1_rx])));

        let pause_queue_c = pause_queue.clone();
        test_support::install_pauser(identity, move || {
            let rx = {
                let mut q = pause_queue_c.lock().unwrap();
                q.pop_front()
            };
            if let Some(rx) = rx {
                entered_tx.send(()).unwrap();
                let _ = rx.recv();
            }
        });

        // R1 starts run 1
        let app1 = app.clone();
        let root1 = root.clone();
        let task_r1 = tokio::spawn(async move {
            let req = Request::get("/app/transcripts/api/index")
                .extension(root1)
                .body(Body::empty())
                .unwrap();
            app1.oneshot(req).await.unwrap()
        });

        entered_rx.recv().await.unwrap();

        // While run 1 is held:
        // Register index, stats/202603, stats/202401
        let q_not = test_support::queued_notify(identity);

        let app_idx = app.clone();
        let root_idx = root.clone();
        let task_idx = tokio::spawn(async move {
            let req = Request::get("/app/transcripts/api/index")
                .extension(root_idx)
                .body(Body::empty())
                .unwrap();
            app_idx.oneshot(req).await.unwrap()
        });

        let app_202603 = app.clone();
        let root_202603 = root.clone();
        let task_202603 = tokio::spawn(async move {
            let req = Request::get("/app/transcripts/api/stats/202603")
                .extension(root_202603)
                .body(Body::empty())
                .unwrap();
            app_202603.oneshot(req).await.unwrap()
        });

        let app_202401 = app.clone();
        let root_202401 = root.clone();
        let task_202401 = tokio::spawn(async move {
            let req = Request::get("/app/transcripts/api/stats/202401")
                .extension(root_202401)
                .body(Body::empty())
                .unwrap();
            app_202401.oneshot(req).await.unwrap()
        });

        while test_support::queued_count(identity) < 3 {
            q_not.notified().await;
        }

        // stats/13 returns 400 and does not increase run_count
        let req_invalid = Request::get("/app/transcripts/api/stats/13")
            .extension(root.clone())
            .body(Body::empty())
            .unwrap();
        let resp_invalid = app.clone().oneshot(req_invalid).await.unwrap();
        assert_eq!(resp_invalid.status(), StatusCode::BAD_REQUEST);
        assert_eq!(test_support::run_count(identity), 1);

        // Release run 1
        p1_tx.send(()).unwrap();

        let resp_r1 = task_r1.await.unwrap();
        let resp_idx = task_idx.await.unwrap();
        let resp_202603 = task_202603.await.unwrap();
        let resp_202401 = task_202401.await.unwrap();

        assert_eq!(resp_r1.status(), StatusCode::OK);
        assert_eq!(resp_idx.status(), StatusCode::OK);
        assert_eq!(resp_202603.status(), StatusCode::OK);
        assert_eq!(resp_202401.status(), StatusCode::OK);

        assert_eq!(test_support::run_count(identity), 2);

        let body_idx_bytes = to_bytes(resp_idx.into_body(), usize::MAX).await.unwrap();
        let body_202603_bytes = to_bytes(resp_202603.into_body(), usize::MAX).await.unwrap();
        let body_202401_bytes = to_bytes(resp_202401.into_body(), usize::MAX).await.unwrap();

        let val_idx: Value = serde_json::from_slice(&body_idx_bytes).unwrap();
        assert!(val_idx.get("coverage").is_some());
        assert!(val_idx.get("months").is_some());

        let val_202603: Value = serde_json::from_slice(&body_202603_bytes).unwrap();
        let obj_202603 = val_202603.as_object().unwrap();
        assert!(!obj_202603.is_empty());
        for (day, count) in obj_202603 {
            assert!(day.starts_with("202603"));
            assert!(count.as_u64().unwrap() > 0);
        }

        let val_202401: Value = serde_json::from_slice(&body_202401_bytes).unwrap();
        let obj_202401 = val_202401.as_object().unwrap();
        assert!(!obj_202401.is_empty());
        for (day, count) in obj_202401 {
            assert!(day.starts_with("202401"));
            assert!(count.as_u64().unwrap() > 0);
        }

        assert_ne!(body_idx_bytes, body_202603_bytes);
        assert_ne!(body_idx_bytes, body_202401_bytes);
        assert_ne!(body_202603_bytes, body_202401_bytes);
    }
}
