// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use super::*;
use crate::activity_work::due_activity_retries;
#[cfg(feature = "full-tests")]
use std::sync::atomic::AtomicBool;

const DAY: &str = "20260813";
const NOW: i64 = 1_786_615_200_000;

fn work_and_home(active: &str) -> Value {
    serde_json::json!({"density":"active","content_type":"work","activity_summary":active,
        "facets":[{"facet":"home","level":"high","activity":active},
                  {"facet":"work","level":"high","activity":active}]})
}

#[cfg(feature = "full-tests")]
fn work_only() -> Value {
    serde_json::json!({"density":"active","content_type":"work","activity_summary":"work",
        "facets":[{"facet":"work","level":"high","activity":"work"}]})
}

fn idle() -> Value {
    serde_json::json!({"density":"idle","content_type":"idle","facets":[]})
}

fn write_sense(journal: &Path, stream: &str, key: &str, sense: &Value) {
    let dir = journal
        .join("chronicle")
        .join(DAY)
        .join(stream)
        .join(key)
        .join("talents");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("sense.json"), serde_json::to_vec(sense).unwrap()).unwrap();
}

/// A journal with two facets and one activity talent that runs for both.
fn journal() -> (
    tempfile::TempDir,
    tempfile::TempDir,
    context::ThinkContext,
    Arc<Recorder>,
) {
    let journal = tempdir().unwrap();
    let roots = tempdir().unwrap();
    let (talent_root, apps_root) = talent_roots(
        roots.path(),
        &[(
            "participation",
            "{\n\"type\":\"generate\",\"max_output_tokens\":1024,\"schedule\":\"activity\",\"priority\":1,\"output\":\"json\",\"activities\":[\"work\"]\n}",
        )],
    );
    let (context, recorder) = recorder_context(journal.path(), DAY, NOW);
    for facet in ["home", "work"] {
        solstone_core_facets::create_facet(journal.path(), facet, facet, "", "", "", None).unwrap();
    }
    let context = context.with_talent_roots(talent_root, apps_root);
    (journal, roots, context, recorder)
}

fn records(context: &context::ThinkContext, facet: &str) -> Vec<Value> {
    fs::read_to_string(
        context
            .journal
            .join("facets")
            .join(facet)
            .join("activities")
            .join(format!("{DAY}.jsonl")),
    )
    .unwrap_or_default()
    .lines()
    .map(|line| serde_json::from_str(line).unwrap())
    .collect()
}

fn ids(context: &context::ThinkContext, facet: &str) -> Vec<String> {
    records(context, facet)
        .iter()
        .map(|record| record["id"].as_str().unwrap().to_owned())
        .collect()
}

fn live(context: &context::ThinkContext, stream: &str, key: &str) -> Result<(), String> {
    let mut log = test_log(context, "live");
    segment::replay_activity_state(
        context,
        &mut log,
        &[(key.to_owned(), Some(stream.to_owned()))],
        false,
        1,
        false,
        true,
    )
}

fn flush(context: &context::ThinkContext, stream: &str, key: &str) -> crate::dispatch::ModeResult {
    let mut log = test_log(context, "flush");
    flush::run(context, &mut log, key, Some(stream), 1, false).unwrap()
}

fn with_clock(context: &context::ThinkContext, now_ms: i64) -> context::ThinkContext {
    let mut next = context.clone();
    next.now_ms = now_ms;
    next.with_event_clock(Arc::new(move || now_ms))
}

/// The process dies the first time it asks the model for anything.
struct DiesOnFirstRequest;

impl context::CortexBoundary for DiesOnFirstRequest {
    fn dispatch(
        &self,
        _: &tokio::runtime::Runtime,
        _: &solstone_core_cortex_client::CortexRequest,
    ) -> Result<String, context::DispatchFailure> {
        panic!("the journal stopped while an activity talent was running");
    }
    fn dispatch_prepared(
        &self,
        runtime: &tokio::runtime::Runtime,
        request: &solstone_core_cortex_client::CortexRequest,
        _: Option<&str>,
        _: &mut (dyn FnMut(&str) -> std::io::Result<()> + Send),
    ) -> Result<String, context::DispatchFailure> {
        self.dispatch(runtime, request)
    }
    fn wait(
        &self,
        _: &tokio::runtime::Runtime,
        _: &[String],
        _: Option<std::time::Duration>,
    ) -> Result<solstone_core_cortex_client::WaitForUsesReport, String> {
        unreachable!("nothing is dispatched")
    }
}

#[derive(Clone, Copy, Debug)]
enum Tail {
    Live,
    Flush,
    Batch,
}

/// Two facets' activities end in one step and the process dies while the
/// first one's talent runs. Both activities stay published once, each one's
/// talent work survives the restart, and nothing is published or asked of the
/// model twice.
fn assert_interrupted_tail_recovers(tail: Tail) {
    let (_journal, _roots, context, recorder) = journal();
    write_sense(
        &context.journal,
        "default",
        "090000_300",
        &work_and_home("work"),
    );
    write_sense(&context.journal, "default", "090500_300", &idle());
    let batch_now = NOW + 2 * 86_400_000;
    if !matches!(tail, Tail::Batch) {
        live(&context, "default", "090000_300").unwrap();
    }
    let dying = context.clone().with_boundary(Arc::new(DiesOnFirstRequest));
    let died = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match tail {
        Tail::Live => {
            let _ = live(&dying, "default", "090500_300");
        }
        Tail::Flush => {
            flush(&dying, "default", "090000_300");
        }
        Tail::Batch => {
            let past = with_clock(&dying, batch_now);
            let mut log = test_log(&past, "repair");
            let _ = segment::run_repair_batch_with_activity(
                &past,
                &mut log,
                vec![
                    ("090000_300".to_owned(), Some("default".to_owned())),
                    ("090500_300".to_owned(), Some("default".to_owned())),
                ],
                false,
                1,
                1,
                None,
                vec![],
                false,
                segment::CurrentSegment::Skip,
            );
        }
    }));
    assert!(died.is_err(), "{tail:?}: the injected stop must happen");

    for facet in ["home", "work"] {
        assert_eq!(
            ids(&context, facet),
            ["work_090000_300"],
            "{tail:?}: {facet} must be published before any talent runs"
        );
    }
    let published = (records(&context, "home"), records(&context, "work"));
    assert_eq!(recorder.requests.lock().unwrap().len(), 0);

    // The restarted journal's retry drain finds both activities' work.
    let restart = with_clock(&context, batch_now + 61_000);
    let due = due_activity_retries(&restart.journal, restart.now_ms).unwrap();
    assert_eq!(
        due.iter()
            .map(|retry| retry.facet.as_str())
            .collect::<Vec<_>>(),
        ["home", "work"],
        "{tail:?}: unfinished talent work must be durable"
    );
    for retry in due {
        let retry_context = context::ThinkContext::new_with_event_clock(
            &restart.journal,
            retry.day.clone(),
            restart.journal.join("chronicle").join(&retry.day),
            restart.now_ms,
            restart.event_clock(),
        )
        .unwrap()
        .with_boundary(recorder.clone())
        .with_talent_roots(context.talent_root.clone(), context.apps_root.clone());
        let mut log = test_log(&retry_context, "activity-retry");
        let result = activity::run(
            &retry_context,
            &mut log,
            &retry.activity,
            &retry.facet,
            false,
            false,
            1,
        )
        .unwrap();
        assert_eq!((result.success, result.failed), (1, 0), "{tail:?}");
    }
    assert_eq!(recorder.requests.lock().unwrap().len(), 2);
    assert!(
        due_activity_retries(&restart.journal, i64::MAX)
            .unwrap()
            .is_empty()
    );

    // The stream carries on; the ended activities are not published or
    // thought about again, and their records are untouched.
    match tail {
        Tail::Live => {
            write_sense(&context.journal, "default", "091000_300", &idle());
            live(&restart, "default", "091000_300").unwrap();
        }
        Tail::Flush => {
            assert_eq!(flush(&restart, "default", "090000_300").failed, 0);
        }
        Tail::Batch => {
            let past = with_clock(&restart, batch_now + 120_000);
            let mut log = test_log(&past, "repair-again");
            segment::run_repair_batch_with_activity(
                &past,
                &mut log,
                vec![("090500_300".to_owned(), Some("default".to_owned()))],
                false,
                1,
                1,
                None,
                vec![],
                false,
                segment::CurrentSegment::Skip,
            )
            .unwrap();
        }
    }
    assert_eq!(
        (records(&context, "home"), records(&context, "work")),
        published
    );
    assert_eq!(recorder.requests.lock().unwrap().len(), 2, "{tail:?}");
}

#[test]
fn an_interrupted_live_tail_keeps_every_ended_activity_and_its_talent_work() {
    assert_interrupted_tail_recovers(Tail::Live);
}

#[test]
fn an_interrupted_idle_flush_keeps_every_ended_activity_and_its_talent_work() {
    assert_interrupted_tail_recovers(Tail::Flush);
}

#[test]
fn an_interrupted_repair_keeps_every_ended_activity_and_its_talent_work() {
    assert_interrupted_tail_recovers(Tail::Batch);
}

/// The process stops after the ended activities are published but before
/// the stream's state is saved. The saved state still holds them, so the
/// stream's next segment ends them again: no published activity is lost,
/// published twice or thought about twice.
fn assert_stop_before_saving_state_recovers(tail: Tail) {
    let _serial = INTERLEAVING.lock().unwrap_or_else(|p| p.into_inner());
    let (_journal, _roots, context, recorder) = journal();
    write_sense(
        &context.journal,
        "default",
        "090000_300",
        &work_and_home("work"),
    );
    write_sense(&context.journal, "default", "090500_300", &idle());
    write_sense(&context.journal, "default", "091000_300", &idle());
    live(&context, "default", "090000_300").unwrap();
    let target = context.journal.clone();
    let snapshot = solstone_core_system::activity_state::activity_state_path(
        &context.journal,
        Some("default"),
    );
    let open_at_stop = Arc::new(Mutex::new(None));
    let seen = open_at_stop.clone();
    segment::state_probe::install(Some(Arc::new(move |journal: &Path, point: &str| {
        if journal == target && point == "published" {
            let saved: Value = serde_json::from_slice(&fs::read(&snapshot).unwrap()).unwrap();
            *seen.lock().unwrap() = saved["active"].as_object().map(|active| active.len());
            panic!("the journal stopped before saving activity state");
        }
    })));
    let died = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match tail {
        Tail::Live => {
            let _ = live(&context, "default", "090500_300");
        }
        Tail::Flush | Tail::Batch => {
            flush(&context, "default", "090000_300");
        }
    }));
    segment::state_probe::install(None);
    assert!(died.is_err(), "{tail:?}: the injected stop must happen");
    assert_eq!(
        *open_at_stop.lock().unwrap(),
        Some(2),
        "{tail:?}: both records are published while the saved state still holds them"
    );
    for facet in ["home", "work"] {
        assert_eq!(
            ids(&context, facet),
            ["work_090000_300"],
            "{tail:?}: {facet}"
        );
    }
    assert_eq!(recorder.requests.lock().unwrap().len(), 0);

    let next = with_clock(&context, NOW + 600_000);
    live(&next, "default", "091000_300").unwrap();
    for facet in ["home", "work"] {
        assert_eq!(
            ids(&context, facet),
            ["work_090000_300"],
            "{tail:?}: {facet}"
        );
    }
    assert_eq!(recorder.requests.lock().unwrap().len(), 2, "{tail:?}");
    assert!(
        due_activity_retries(&context.journal, i64::MAX)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_live_segment_stopped_before_saving_state_loses_nothing() {
    assert_stop_before_saving_state_recovers(Tail::Live);
}

#[test]
fn a_flush_stopped_before_saving_state_loses_nothing() {
    assert_stop_before_saving_state_recovers(Tail::Flush);
}

/// While the first activity's talent runs, the retry drain looks for due
/// work. The second activity's work is recorded but held by the run that
/// recorded it, so the drain leaves it alone, and a refresh asks the model
/// once per activity.
struct DrainDuringFirstRequest {
    inner: Arc<Recorder>,
    drain: context::ThinkContext,
    drained: Mutex<Option<Vec<Result<(), String>>>>,
}

impl context::CortexBoundary for DrainDuringFirstRequest {
    fn dispatch(
        &self,
        runtime: &tokio::runtime::Runtime,
        request: &solstone_core_cortex_client::CortexRequest,
    ) -> Result<String, context::DispatchFailure> {
        self.inner.dispatch(runtime, request)
    }
    fn dispatch_prepared(
        &self,
        runtime: &tokio::runtime::Runtime,
        request: &solstone_core_cortex_client::CortexRequest,
        reserved: Option<&str>,
        prepare: &mut (dyn FnMut(&str) -> std::io::Result<()> + Send),
    ) -> Result<String, context::DispatchFailure> {
        let mut drained = self.drained.lock().unwrap();
        if drained.is_none() {
            let attempts = due_activity_retries(&self.drain.journal, i64::MAX)
                .unwrap()
                .into_iter()
                .map(|retry| {
                    let mut log = test_log(&self.drain, "drain");
                    activity::run(
                        &self.drain,
                        &mut log,
                        &retry.activity,
                        &retry.facet,
                        false,
                        false,
                        1,
                    )
                    .map(drop)
                })
                .collect();
            *drained = Some(attempts);
        }
        drop(drained);
        self.inner
            .dispatch_prepared(runtime, request, reserved, prepare)
    }
    fn wait(
        &self,
        runtime: &tokio::runtime::Runtime,
        use_ids: &[String],
        deadline: Option<std::time::Duration>,
    ) -> Result<solstone_core_cortex_client::WaitForUsesReport, String> {
        self.inner.wait(runtime, use_ids, deadline)
    }
}

#[test]
fn the_retry_drain_leaves_recorded_work_to_the_run_that_recorded_it() {
    let (_journal, _roots, context, recorder) = journal();
    write_sense(
        &context.journal,
        "default",
        "090000_300",
        &work_and_home("work"),
    );
    write_sense(&context.journal, "default", "090500_300", &idle());
    let past = with_clock(&context, NOW + 2 * 86_400_000);
    let boundary = Arc::new(DrainDuringFirstRequest {
        inner: recorder.clone(),
        drain: past.clone(),
        drained: Mutex::new(None),
    });
    let repairing = past.clone().with_boundary(boundary.clone());
    let mut log = test_log(&repairing, "repair");
    let result = segment::run_repair_batch_with_activity(
        &repairing,
        &mut log,
        vec![
            ("090000_300".to_owned(), Some("default".to_owned())),
            ("090500_300".to_owned(), Some("default".to_owned())),
        ],
        true,
        1,
        1,
        None,
        vec![],
        false,
        segment::CurrentSegment::Skip,
    )
    .unwrap();
    assert_eq!(result.failed, 0, "{result:?}");
    let drained = boundary.drained.lock().unwrap().take().unwrap();
    assert_eq!(
        drained,
        [
            Err(crate::activity_work::CLAIMED.to_owned()),
            Err(crate::activity_work::CLAIMED.to_owned())
        ],
        "both activities' work is held by the repair"
    );
    assert_eq!(recorder.requests.lock().unwrap().len(), 2);
    assert!(
        due_activity_retries(&context.journal, i64::MAX)
            .unwrap()
            .is_empty()
    );
}

/// Runs `competitor` on another thread at the moment the first reader of
/// `journal`'s live state has read it, and lets that reader go on only once
/// the competitor has finished or is waiting for the stream's turn.
#[cfg(feature = "full-tests")]
fn interleave(
    journal: &Path,
    competitor: impl FnOnce() + Send + 'static,
) -> Arc<Mutex<Option<std::thread::JoinHandle<()>>>> {
    let target = journal.to_path_buf();
    let armed = Arc::new(AtomicBool::new(false));
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let rx = Mutex::new(rx);
    let competitor = Mutex::new(Some(competitor));
    let handle = Arc::new(Mutex::new(None));
    let spawned = handle.clone();
    segment::state_probe::install(Some(Arc::new(move |journal: &Path, point: &str| {
        if journal != target {
            return;
        }
        match point {
            "hydrated" if !armed.swap(true, Ordering::SeqCst) => {
                let run = competitor.lock().unwrap().take().unwrap();
                let done = tx.clone();
                *spawned.lock().unwrap() = Some(std::thread::spawn(move || {
                    run();
                    let _ = done.send(());
                }));
                rx.lock().unwrap().recv().unwrap();
            }
            "turn" if armed.load(Ordering::SeqCst) => {
                let _ = tx.send(());
            }
            _ => {}
        }
    })));
    handle
}

#[cfg(feature = "full-tests")]
fn finish(handle: Arc<Mutex<Option<std::thread::JoinHandle<()>>>>) {
    segment::state_probe::install(None);
    if let Some(thread) = handle.lock().unwrap().take() {
        thread.join().unwrap();
    }
}

static INTERLEAVING: Mutex<()> = Mutex::new(());

#[cfg(feature = "full-tests")]
#[test]
fn an_activity_started_while_the_flush_runs_is_not_lost() {
    let _serial = INTERLEAVING.lock().unwrap_or_else(|p| p.into_inner());
    // The flush reads the stream's state, then a live segment after a long
    // pause ends the old activity and starts a new one before the flush
    // writes. The new activity must still be published when it ends.
    let (_journal, _roots, context, _recorder) = journal();
    write_sense(&context.journal, "default", "090000_300", &work_only());
    write_sense(&context.journal, "default", "093000_300", &work_only());
    write_sense(&context.journal, "default", "093500_300", &idle());
    live(&context, "default", "090000_300").unwrap();
    let competitor = context.clone();
    let handle = interleave(&context.journal, move || {
        live(&competitor, "default", "093000_300").unwrap();
    });
    let flushed = flush(&context, "default", "090000_300");
    finish(handle);
    assert_eq!(flushed.failed, 0, "{flushed:?}");
    live(&context, "default", "093500_300").unwrap();
    assert_eq!(
        ids(&context, "work"),
        ["work_090000_300", "work_093000_300"],
        "an activity started while the flush ran must not be lost"
    );
}

#[cfg(feature = "full-tests")]
#[test]
fn a_segment_thought_while_the_flush_runs_stays_in_its_activity() {
    let _serial = INTERLEAVING.lock().unwrap_or_else(|p| p.into_inner());
    // A live segment that continues the activity reads the state, then the
    // flush for the previous segment runs before it writes. The continuing
    // segment must end up in the published activity.
    let (_journal, _roots, context, _recorder) = journal();
    write_sense(&context.journal, "default", "090000_300", &work_only());
    write_sense(&context.journal, "default", "090500_300", &work_only());
    write_sense(&context.journal, "default", "091000_300", &idle());
    live(&context, "default", "090000_300").unwrap();
    let competitor = context.clone();
    let handle = interleave(&context.journal, move || {
        flush(&competitor, "default", "090000_300");
    });
    live(&context, "default", "090500_300").unwrap();
    finish(handle);
    live(&context, "default", "091000_300").unwrap();
    let published = records(&context, "work");
    assert_eq!(published.len(), 1, "{published:?}");
    assert_eq!(
        published[0]["segments"],
        serde_json::json!(["090000_300", "090500_300"]),
        "a segment thought while the flush ran must not be dropped"
    );
}

#[cfg(feature = "full-tests")]
#[test]
fn one_streams_turn_never_holds_up_another_stream() {
    let _serial = INTERLEAVING.lock().unwrap_or_else(|p| p.into_inner());
    let (_journal, _roots, context, _recorder) = journal();
    write_sense(&context.journal, "desktop", "090000_300", &work_only());
    write_sense(&context.journal, "tmux", "090000_300", &work_only());
    live(&context, "desktop", "090000_300").unwrap();
    let other = Arc::new(Mutex::new(None));
    let result = other.clone();
    let target = context.journal.clone();
    let competitor = context.clone();
    let fired = AtomicBool::new(false);
    let unguarded = Arc::new(AtomicBool::new(false));
    let seen = unguarded.clone();
    segment::state_probe::install(Some(Arc::new(move |journal: &Path, point: &str| {
        if journal != target {
            return;
        }
        if point == "unguarded" {
            seen.store(true, Ordering::SeqCst);
        }
        if point == "hydrated" && !fired.swap(true, Ordering::SeqCst) {
            let outcome = live(&competitor, "tmux", "090000_300");
            *result.lock().unwrap() = Some(outcome);
        }
    })));
    flush(&context, "desktop", "090000_300");
    segment::state_probe::install(None);
    assert_eq!(other.lock().unwrap().take(), Some(Ok(())));
    assert!(
        !unguarded.load(Ordering::SeqCst),
        "each stream takes its own turn"
    );
    assert_eq!(ids(&context, "work"), ["work_090000_300"]);
}
