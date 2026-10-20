// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! One coalescing discovery worker. It reconstructs work; TaskQueue executes it.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::{Duration, Instant, SystemTime};

use solstone_core_import::thinking::{Coordinate, ImportThinking};
use solstone_core_journal_config::work_priority::{WorkBand, WorkPriority};
use solstone_core_system::{
    daily_coverage::{self, CoverageState},
    partition::canonical_journal_command,
    queue::{DISCOVERED_ADMISSION, DiscoveredTask, TaskQueue},
    request::{BusTaskRequest, DailyCatchupProvenance, TaskArgv},
    work_intent::{self, IntentState, OwnerIntent},
    work_inventory::{HistoryProgress, HistorySample, WorkHold, WorkInventory},
    work_policy::WorkPolicy,
    work_retry::{self, Unit},
};
use solstone_core_system_health::SegmentReadiness;

/// The least time between automatic passes.
const MIN_REST: Duration = Duration::from_secs(60);
/// An automatic pass then rests nine times as long as it ran, so on any journal
/// discovery holds at most a tenth of one core. A request still wakes it at once.
const REST_PER_PASS: u32 = 9;
/// How often a pass publishes its progress before it finishes.
const PROGRESS_INTERVAL: Duration = Duration::from_secs(30);
/// How much of its ranking a pass keeps. The queue admits a few tasks at a
/// time; the rest wait here, in order, and are admitted as those tasks leave
/// the queue, so a backlog drains at the pace work finishes rather than a few
/// tasks per pass.
const RANKED: usize = 1024;

fn rest_after(pass: Duration) -> Duration {
    MIN_REST.max(pass * REST_PER_PASS)
}

/// While it watches the queue, a resting worker checks it this often: for
/// the end of an owner's requested work, and for discovered work that has left.
const QUEUE_POLL: Duration = Duration::from_secs(1);
/// A resting worker tells readers a later next pass at most this often.
const REPROMISE: Duration = Duration::from_secs(60);

/// Days the owner asked to redo whose discovered work is still in the queue.
fn owner_days_in_queue(journal: &Path, queue: &TaskQueue) -> BTreeSet<String> {
    let Ok(intents) = work_intent::read(journal) else {
        return BTreeSet::new();
    };
    intents
        .into_keys()
        .filter(|day| owner_work_in_queue(queue, day))
        .collect()
}

fn owner_work_in_queue(queue: &TaskQueue, day: &str) -> bool {
    let day = format!("-{day}-");
    queue
        .any_reference(|reference| reference.starts_with("supervisor-") && reference.contains(&day))
}

/// Rest until a wake, the next automatic pass, or the end of an owner's
/// requested work. A day's later steps (its daily batch, or segments past the
/// queue's admission bound) are found only by a pass, so a finished owner
/// request starts one rather than waiting out the automatic rest. It still
/// waits as long as the last pass ran, so discovery stays at most half a core
/// while owner work keeps finishing. Returns false when the worker must exit.
///
/// While it rests, `top_up` admits the pass's next ranked work as earlier work
/// leaves the queue, until it returns false. Each top-up then rests nine times
/// as long as it ran and moves the next pass back by ten times as long, so
/// discovery as a whole still holds at most a tenth of one core.
///
/// `promise` carries the time readers were told the next pass is due. When
/// that time arrives and top-ups have moved the next pass back, it tells them
/// the new time. Only a live worker does this, so if it stops, its snapshot
/// goes stale on the time last promised.
fn rest(
    receiver: &mpsc::Receiver<()>,
    mut next_pass: Option<Instant>,
    owner: Option<(Instant, &dyn Fn() -> bool)>,
    mut top_up: Option<&mut dyn FnMut() -> bool>,
    mut promise: Option<(Instant, &mut dyn FnMut(Instant))>,
) -> bool {
    let mut top_up_at = Instant::now();
    let mut told = promise.as_ref().map(|(told, _)| *told);
    let mut check_at = told;
    loop {
        let now = Instant::now();
        if next_pass.is_some_and(|due| now >= due)
            || owner.is_some_and(|(from, done)| now >= from && done())
        {
            return true;
        }
        if let (Some(due), Some(at), Some(was)) = (next_pass, check_at, told)
            && now >= at
            && let Some((_, tell)) = promise.as_mut()
        {
            if due > was + QUEUE_POLL {
                tell(due);
                told = Some(due);
            }
            check_at = Some(due.max(now + REPROMISE));
        }
        if now >= top_up_at
            && let Some(admit_next) = top_up.as_mut()
        {
            let began = Instant::now();
            let more = admit_next();
            let spent = began.elapsed();
            top_up_at = Instant::now() + spent * REST_PER_PASS;
            next_pass = next_pass.map(|due| due + spent * (REST_PER_PASS + 1));
            if !more {
                top_up = None;
            }
        }
        let now = Instant::now();
        let poll = owner
            .map(|(from, _)| from.saturating_duration_since(now).max(QUEUE_POLL))
            .into_iter()
            .chain(
                top_up
                    .is_some()
                    .then(|| top_up_at.saturating_duration_since(now).max(QUEUE_POLL)),
            )
            .chain(check_at.map(|at| at.saturating_duration_since(now)))
            .min();
        let timeout = match (
            next_pass.map(|due| due.saturating_duration_since(now)),
            poll,
        ) {
            (Some(due), Some(poll)) => Some(due.min(poll)),
            (due, poll) => due.or(poll),
        };
        let received = match timeout {
            Some(timeout) => receiver.recv_timeout(timeout),
            None => receiver
                .recv()
                .map_err(|_| mpsc::RecvTimeoutError::Disconnected),
        };
        match received {
            Ok(()) => return true,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return false,
        }
    }
}

/// Packaged talent resources admitted by an embedded host independently of its executable.
#[derive(Clone)]
pub struct DiscoveryTalentRoots {
    pub talent: PathBuf,
    pub apps: PathBuf,
}

impl DiscoveryTalentRoots {
    fn revalidate(&self) -> Result<(), String> {
        for directory in [&self.talent, &self.apps] {
            if !directory.is_dir() {
                return Err(format!(
                    "packaged talent directory is unavailable: {}",
                    directory.display()
                ));
            }
        }
        Ok(())
    }
}

fn read_coverage(
    journal: &Path,
    day: &str,
    roots: Option<&DiscoveryTalentRoots>,
) -> Result<daily_coverage::DailyCoverage, String> {
    match roots {
        Some(roots) => {
            roots.revalidate()?;
            daily_coverage::read_daily_coverage_with_roots(journal, day, &roots.talent, &roots.apps)
        }
        None => daily_coverage::read_daily_coverage(journal, day),
    }
}

fn observe_daily_retry(
    journal: &Path,
    unit: &Unit,
    roots: Option<&DiscoveryTalentRoots>,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<Option<String>, String> {
    match roots {
        Some(roots) => {
            roots.revalidate()?;
            work_retry::daily::observe_with_roots_checked(
                journal,
                unit,
                &roots.talent,
                &roots.apps,
                checkpoint,
            )
        }
        None => work_retry::daily::observe_checked(journal, unit, checkpoint),
    }
}

pub struct WorkDiscovery {
    stopped: Arc<AtomicBool>,
    wake: mpsc::SyncSender<()>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl WorkDiscovery {
    pub fn new(journal: PathBuf, queue: TaskQueue, automatic: bool) -> Self {
        Self::with_reconciler(journal, queue, automatic, move |journal, queue, stopped| {
            reconcile_pass(journal, queue, stopped, automatic)
        })
    }

    fn with_reconciler(
        journal: PathBuf,
        queue: TaskQueue,
        automatic: bool,
        reconcile_pass: impl FnMut(&Path, &TaskQueue, &AtomicBool) -> Result<Backlog, String>
        + Send
        + 'static,
    ) -> Self {
        Self::with_reconciler_and_stop(
            journal,
            queue,
            automatic,
            Arc::new(AtomicBool::new(false)),
            Arc::new(|| Ok(())),
            None,
            reconcile_pass,
        )
    }

    pub fn new_with_checkpoint(
        journal: PathBuf,
        queue: TaskQueue,
        automatic: bool,
        checkpoint: Arc<dyn Fn() -> Result<(), String> + Send + Sync>,
    ) -> Self {
        let reconcile_checkpoint = Arc::clone(&checkpoint);
        Self::with_reconciler_and_stop(
            journal,
            queue,
            automatic,
            Arc::new(AtomicBool::new(false)),
            checkpoint,
            None,
            move |journal, queue, stopped| {
                reconcile_pass_checked(
                    journal,
                    queue,
                    stopped,
                    automatic,
                    reconcile_checkpoint.as_ref(),
                )
            },
        )
    }

    pub fn new_with_talent_roots_and_checkpoint(
        journal: PathBuf,
        queue: TaskQueue,
        automatic: bool,
        roots: DiscoveryTalentRoots,
        checkpoint: Arc<dyn Fn() -> Result<(), String> + Send + Sync>,
    ) -> Self {
        let reconcile_checkpoint = Arc::clone(&checkpoint);
        let startup_roots = Some((roots.talent.clone(), roots.apps.clone()));
        Self::with_reconciler_and_stop(
            journal,
            queue,
            automatic,
            Arc::new(AtomicBool::new(false)),
            checkpoint,
            startup_roots,
            move |journal, queue, stopped| {
                reconcile_pass_with_roots(
                    journal,
                    queue,
                    stopped,
                    automatic,
                    Some(&roots),
                    reconcile_checkpoint.as_ref(),
                )
            },
        )
    }

    fn with_reconciler_and_stop(
        journal: PathBuf,
        queue: TaskQueue,
        automatic: bool,
        stopped: Arc<AtomicBool>,
        checkpoint: Arc<dyn Fn() -> Result<(), String> + Send + Sync>,
        startup_roots: Option<(PathBuf, PathBuf)>,
        mut reconcile_pass: impl FnMut(&Path, &TaskQueue, &AtomicBool) -> Result<Backlog, String>
        + Send
        + 'static,
    ) -> Self {
        let (wake, receiver) = mpsc::sync_channel(1);
        let worker_stop = Arc::clone(&stopped);
        let spawned = std::thread::Builder::new()
            .name("journal-work-discovery".into())
            .spawn(move || {
                if receiver.recv().is_err() {
                    return;
                }
                let mut initial = true;
                loop {
                    if worker_stop.load(Ordering::Acquire) || checkpoint().is_err() {
                        return;
                    }
                    let started = Instant::now();
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        if initial {
                            solstone_core_system::catchup::reconcile_stale_catchup_attempts_with_roots_checked(
                                &journal,
                                SystemTime::now(),
                                checkpoint.as_ref(),
                                startup_roots.as_ref().map(|(talent, apps)| (talent.as_path(), apps.as_path())),
                            )
                            .map_err(|error| error.to_string())?;
                            initial = false;
                        }
                        reconcile_pass(&journal, &queue, &worker_stop)
                    }));
                    let mut backlog = Backlog::default();
                    let error = match result {
                        Ok(Ok(found)) => {
                            backlog = found;
                            None
                        }
                        Ok(Err(error)) => Some(error),
                        Err(_) => Some("work discovery panicked".into()),
                    };
                    if let Some(error) = error {
                        log::warn!("work discovery: {error}");
                        if !worker_stop.load(Ordering::Acquire) && checkpoint().is_ok() {
                            let mut report = WorkInventory::default();
                            report.unreadable.push(error);
                            let _ = report.publish_checked(&journal, checkpoint.as_ref());
                        }
                    }
                    if worker_stop.load(Ordering::Acquire) || checkpoint().is_err() {
                        return;
                    }
                    let pass = started.elapsed();
                    let ended = Instant::now();
                    let owner_days = owner_days_in_queue(&journal, &queue);
                    let owner_done = || {
                        owner_days
                            .iter()
                            .any(|day| !owner_work_in_queue(&queue, day))
                    };
                    let mut published = backlog.published.take();
                    let told = published
                        .as_ref()
                        .and_then(|report| report.next_pass_ms)
                        .map(|due| {
                            let left = due - chrono::Utc::now().timestamp_millis();
                            Instant::now() + Duration::from_millis(left.max(0) as u64)
                        });
                    let mut promise = |due: Instant| {
                        if let Some(report) = published.as_mut() {
                            repromise(&journal, report, due, checkpoint.as_ref());
                        }
                    };
                    let mut top_up =
                        || backlog.top_up(&journal, &queue, &worker_stop, checkpoint.as_ref());
                    if !rest(
                        &receiver,
                        automatic.then(|| ended + rest_after(pass)),
                        (!owner_days.is_empty())
                            .then_some((ended + pass, &owner_done as &dyn Fn() -> bool)),
                        automatic.then_some(&mut top_up as &mut dyn FnMut() -> bool),
                        told.map(|told| (told, &mut promise as &mut dyn FnMut(Instant))),
                    ) {
                        return;
                    }
                }
            });
        let handle = match spawned {
            Ok(handle) => Some(handle),
            Err(error) => {
                stopped.store(true, Ordering::Release);
                log::warn!("could not start work discovery: {error}");
                None
            }
        };
        Self {
            stopped,
            wake,
            handle,
        }
    }

    pub fn request(&self) {
        let _ = self.wake.try_send(());
    }
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
        self.request();
    }
    pub fn shutdown_checked(&mut self) -> Result<(), String> {
        self.stop();
        if let Some(handle) = self.handle.take() {
            handle
                .join()
                .map_err(|_| "work discovery failed to join".to_owned())?;
        }
        Ok(())
    }
    pub fn shutdown(&mut self) {
        let _ = self.shutdown_checked();
    }
}

impl Drop for WorkDiscovery {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Republish a resting pass's snapshot with a later next pass. It is still the
/// snapshot as of its pass; only the promise moves.
fn repromise(
    journal: &Path,
    report: &mut WorkInventory,
    due: Instant,
    checkpoint: &dyn Fn() -> Result<(), String>,
) {
    report.next_pass_ms = Some(
        chrono::Utc::now().timestamp_millis()
            + due.saturating_duration_since(Instant::now()).as_millis() as i64,
    );
    if let Err(error) = report.publish_checked(journal, checkpoint) {
        log::warn!("work discovery: {error}");
    }
}

/// What the last pass handed the queue, the rest of its ranking in order, and
/// the snapshot it published.
#[derive(Default)]
struct Backlog {
    admitted: Vec<DiscoveredTask>,
    ranked: VecDeque<DiscoveredTask>,
    published: Option<WorkInventory>,
    roots: Option<DiscoveryTalentRoots>,
}

impl Backlog {
    /// Once admitted work has left the queue, admit the next ranked work that
    /// is still owed in its place, without another pass. Returns whether any
    /// ranked work remains.
    fn top_up(
        &mut self,
        journal: &Path,
        queue: &TaskQueue,
        stopped: &AtomicBool,
        checkpoint: &dyn Fn() -> Result<(), String>,
    ) -> bool {
        if checkpoint().is_err() || stopped.load(Ordering::Acquire) {
            return false;
        }
        if self.ranked.is_empty() {
            return false;
        }
        let present = self
            .admitted
            .iter()
            .filter(|task| queue.contains_reference(&task.request.reference))
            .cloned()
            .collect::<Vec<_>>();
        if present.len() == self.admitted.len() {
            return true;
        }
        let mut report = WorkInventory::default();
        match admit(
            journal,
            queue,
            stopped,
            present,
            &mut self.ranked,
            &mut report,
            self.roots.as_ref(),
            checkpoint,
        ) {
            Ok(admitted) => self.admitted = admitted,
            Err(error) => {
                log::warn!("work discovery: {error}");
                self.ranked.clear();
            }
        }
        for error in report.unreadable {
            log::warn!("work discovery: {error}");
        }
        !self.ranked.is_empty()
    }
}

struct DayWork {
    candidates: Vec<DiscoveredTask>,
    completed_segments: BTreeSet<Coordinate>,
    daily_current: bool,
    history: HistoryDay,
}

#[derive(Default)]
struct HistoryDay {
    /// Daily evidence is deferred while the indexed archive prerequisite remains.
    archive_pending: bool,
    owed: u64,
    segments_owed: u64,
    daily_units_owed: u64,
    set_aside: u64,
    segments_set_aside: u64,
    daily_units_set_aside: u64,
}

fn priority(
    day: &str,
    segment: Option<&str>,
    today: chrono::NaiveDate,
    explicit: Option<WorkBand>,
) -> WorkPriority {
    let mut command = vec!["journal".into(), "think".into(), "--day".into(), day.into()];
    if let Some(segment) = segment {
        command.extend(["--segment".into(), segment.into()]);
    }
    let mut priority = solstone_core_system::priority::classify(&command, today);
    if let Some(band) = explicit {
        priority.band = band;
    }
    priority
}

/// A past day's daily outputs complete history alongside its recordings when
/// one was never made or its own day's evidence changed, as when the walk
/// re-senses that day. The entity review reads only the seven days before its
/// day, which the newest-first walk re-senses after it, so its re-make waits
/// for band 6 rather than repeating once per day the walk passes. A mixed day
/// takes the same band; explicit owner/import priorities win.
fn daily_priority(
    mut priority: WorkPriority,
    coverage: &daily_coverage::DailyCoverage,
) -> WorkPriority {
    if priority.band == WorkBand::Remake
        && coverage.units.iter().any(|unit| {
            unit.state.is_owed()
                && match unit.owed_by.as_deref() {
                    Some("never_made") => true,
                    Some("evidence_changed") => {
                        solstone_core_indexer::daily_evidence::evidence_day_offsets(
                            &unit.identity.name,
                        ) == [0]
                    }
                    _ => false,
                }
        })
    {
        priority.band = WorkBand::Completing;
    }
    priority
}

fn task(
    day: &str,
    segment: Option<(&str, &str)>,
    priority: WorkPriority,
    intent: Option<&OwnerIntent>,
) -> DiscoveredTask {
    let mut tail = vec!["think".to_owned(), "--day".into(), day.into()];
    let from_scratch = intent.is_some_and(|intent| intent.from_scratch);
    if let Some((segment, stream)) = segment {
        tail.extend([
            "--segment".into(),
            segment.into(),
            "--stream".into(),
            stream.into(),
            "--repair-segment".into(),
        ]);
    } else if let Some(intent) = intent.filter(|intent| intent.from_scratch) {
        tail.extend([
            "--from-scratch".into(),
            "--work-intent".into(),
            intent.id.clone(),
        ]);
    } else {
        tail.push("--daily-only".into());
    }
    let suffix = segment
        .map(|(segment, stream)| format!("{stream}-{segment}"))
        .unwrap_or_else(|| {
            intent
                .filter(|i| i.from_scratch)
                .map(|i| i.id.clone())
                .unwrap_or_else(|| "daily".into())
        });
    DiscoveredTask {
        request: BusTaskRequest {
            cmd: TaskArgv::from_wire(canonical_journal_command(tail))
                .expect("nonempty journal command"),
            reference: format!("supervisor-work-{day}-{suffix}"),
            day: Some(day.into()),
            scheduler_name: None,
            queue_if_active_cmd_differs: false,
            priority: Some(priority),
            daily_catchup_provenance: (segment.is_none() && !from_scratch)
                .then(|| DailyCatchupProvenance { day: day.into() }),
        },
    }
}

fn held(
    report: &mut WorkInventory,
    day: &str,
    segment: Option<(&str, &str)>,
    band: WorkBand,
    reason: &str,
) {
    report.holds.push(WorkHold {
        retry_unit: None,
        day: day.into(),
        segment: segment.map(|(s, _)| s.into()),
        stream: segment.map(|(_, s)| s.into()),
        band,
        reason: reason.into(),
    });
}

fn retry_imported(unit: &Unit, imports: &[ImportThinking]) -> bool {
    imports.iter().any(|import| match unit {
        Unit::Raw {
            day,
            segment,
            stream,
            ..
        }
        | Unit::Segment {
            day,
            segment,
            stream,
            ..
        } => import.segments.contains(&Coordinate {
            day: day.clone(),
            segment: segment.clone(),
            stream: stream.clone(),
        }),
        Unit::Daily { day, talent, .. } if talent != "daily_schedule" => import.days.contains(day),
        _ => false,
    })
}

/// Read native writer evidence on the discovery worker. The permission ledger
/// keeps the original failed input; it does not make this scan authoritative.
#[allow(clippy::too_many_arguments)]
fn raw_retries(
    journal: &Path,
    day: &str,
    segment: &solstone_core_system_health::DaySegment,
    intent: Option<&OwnerIntent>,
    imports: &[ImportThinking],
    policy: WorkPolicy,
    capped: &BTreeSet<solstone_core_system_health::TerminalUnit>,
    permanently_held: &mut bool,
    report: &mut WorkInventory,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<Vec<DiscoveredTask>, String> {
    let directory = match segment.stream_layout {
        solstone_core_journal_io::SegmentLayout::Direct => {
            journal.join("chronicle").join(day).join(&segment.key)
        }
        solstone_core_journal_io::SegmentLayout::Named => journal
            .join("chronicle")
            .join(day)
            .join(&segment.stream)
            .join(&segment.key),
    };
    let today = chrono::Utc::now()
        .with_timezone(&solstone_core_journal_config::owner_zone(journal))
        .date_naive();
    let mut tasks = Vec::new();
    for talent in solstone_core_system_health::SEGMENT_FLOOR_TALENTS {
        let unit = Unit::Segment {
            day: day.into(),
            stream: segment.stream.clone(),
            segment: segment.key.clone(),
            talent: (*talent).into(),
            direct: matches!(
                segment.stream_layout,
                solstone_core_journal_io::SegmentLayout::Direct
            ),
        };
        if work_retry::read(journal, &unit.id()?)?.is_some() {
            continue;
        }
        if capped.contains(&solstone_core_system_health::TerminalUnit {
            day: day.into(),
            mode: "segment".into(),
            name: (*talent).into(),
            facet: None,
            stream: Some(segment.stream.clone()),
            segment: Some(segment.key.clone()),
            activity: None,
        }) {
            match work_retry::input::current(journal, &unit) {
                Ok(input) => work_retry::observe_held_checked(
                    journal,
                    &unit,
                    "floor_failed",
                    &input,
                    checkpoint,
                )?,
                Err(error) => report.unreadable.push(error),
            }
        }
    }
    for entry in std::fs::read_dir(directory).map_err(|e| e.to_string())? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.extension().and_then(|p| p.to_str()) != Some("jsonl") {
            continue;
        }
        let evidence = match work_retry::raw::inspect_output(journal, &path) {
            Ok(Some(evidence)) => evidence,
            Ok(None) => continue,
            Err(error) => {
                report
                    .unreadable
                    .push(format!("{}: {error}", path.display()));
                continue;
            }
        };
        work_retry::raw::observe_checked(journal, &evidence, checkpoint)?;
        let id = evidence.unit.id()?;
        // A durable day ask is expanded idempotently, never renewed by ticks.
        if let Some(intent) = intent {
            work_retry::request_with_id_checked(journal, &evidence.unit, &intent.id, checkpoint)?;
        }
        let record = work_retry::read(journal, &id)?.ok_or("missing native hold")?;
        let grant = record.permission(&evidence.input, work_retry::DECLARED_FIXES);
        let imported = retry_imported(&record.unit, imports);
        let explicit = match grant.as_ref().map(|a| a.cause.as_str()) {
            Some(cause) if cause.starts_with("owner:") => Some(WorkBand::Waiting),
            Some(cause) if cause.starts_with("fix:") => Some(WorkBand::Completing),
            _ => imported.then_some(WorkBand::Import),
        };
        let p = priority(day, Some(&segment.key), today, explicit);
        report.holds.push(WorkHold {
            retry_unit: Some(id.clone()),
            day: day.into(),
            segment: Some(segment.key.clone()),
            stream: Some(segment.stream.clone()),
            band: p.band,
            reason: record.reason.clone(),
        });
        let Some(grant) = grant else {
            *permanently_held = true;
            continue;
        };
        if !policy.permits(p.band) {
            report.band(p.band).permission_held += 1;
            continue;
        }
        report.band(p.band).eligible += 1;
        tasks.push(DiscoveredTask {
            request: BusTaskRequest {
                cmd: TaskArgv::from_wire(canonical_journal_command([
                    "think",
                    "--day",
                    day,
                    "--retry-unit",
                    &id,
                    "--retry-cause",
                    &grant.cause,
                ]))
                .expect("native retry command"),
                reference: format!("supervisor-retry-raw-{day}-{id}"),
                day: Some(day.into()),
                scheduler_name: None,
                queue_if_active_cmd_differs: false,
                priority: Some(p),
                daily_catchup_provenance: None,
            },
        });
    }
    Ok(tasks)
}

#[allow(clippy::too_many_arguments)]
fn daily_retries(
    journal: &Path,
    day: &str,
    coverage: &daily_coverage::DailyCoverage,
    roots: Option<&DiscoveryTalentRoots>,
    intent: Option<&OwnerIntent>,
    imports: &[ImportThinking],
    policy: WorkPolicy,
    ready: bool,
    permanently_held: &mut BTreeSet<String>,
    report: &mut WorkInventory,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<Vec<DiscoveredTask>, String> {
    let today = chrono::Utc::now()
        .with_timezone(&solstone_core_journal_config::owner_zone(journal))
        .date_naive();
    let mut candidates = Vec::new();
    let maintenance = coverage
        .maintenance
        .as_ref()
        .filter(|_| day == today.format("%Y%m%d").to_string());
    for covered in coverage.units.iter().chain(maintenance) {
        let unit = Unit::Daily {
            day: day.into(),
            talent: covered.identity.name.clone(),
            facet: covered.identity.facet.clone(),
        };
        let Some(input) = observe_daily_retry(journal, &unit, roots, checkpoint)? else {
            continue;
        };
        let id = unit.id()?;
        if let Some(intent) = intent {
            work_retry::request_with_id_checked(journal, &unit, &intent.id, checkpoint)?;
        }
        let record = work_retry::read(journal, &id)?.ok_or("daily permission missing")?;
        let grant = record.permission(&input, work_retry::DECLARED_FIXES);
        let imported = retry_imported(&record.unit, imports);
        let explicit = match grant.as_ref().map(|a| a.cause.as_str()) {
            Some(cause) if cause.starts_with("owner:") => Some(WorkBand::Waiting),
            Some(cause) if cause.starts_with("fix:") => Some(WorkBand::Completing),
            _ => imported.then_some(WorkBand::Import),
        };
        let p = priority(day, None, today, explicit);
        if !covered.state.is_owed() {
            report.band(p.band).daily_units_owed += 1;
        }
        report.holds.push(WorkHold {
            retry_unit: Some(id.clone()),
            day: day.into(),
            segment: None,
            stream: None,
            band: p.band,
            reason: record.reason.clone(),
        });
        let Some(grant) = grant else {
            permanently_held.insert(id);
            continue;
        };
        if !ready {
            report.band(p.band).prerequisite_held += 1;
            continue;
        }
        if !policy.permits(p.band) {
            report.band(p.band).permission_held += 1;
            continue;
        }
        report.band(p.band).eligible += 1;
        candidates.push(DiscoveredTask {
            request: BusTaskRequest {
                cmd: TaskArgv::from_wire(canonical_journal_command([
                    "think",
                    "--day",
                    day,
                    "--retry-unit",
                    &id,
                    "--retry-cause",
                    &grant.cause,
                ]))
                .expect("daily retry command"),
                reference: format!("supervisor-retry-daily-{day}-{id}"),
                day: Some(day.into()),
                scheduler_name: None,
                queue_if_active_cmd_differs: false,
                priority: Some(p),
                daily_catchup_provenance: None,
            },
        });
    }
    Ok(candidates)
}

#[allow(
    clippy::too_many_arguments,
    reason = "The discovery inputs and bound publication checkpoint remain explicit."
)]
fn talent_retries(
    journal: &Path,
    day: &str,
    intent: Option<&OwnerIntent>,
    imports: &[ImportThinking],
    policy: WorkPolicy,
    permanently_held_activities: &mut u64,
    report: &mut WorkInventory,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<Vec<DiscoveredTask>, String> {
    let (records, errors) = work_retry::list(journal);
    report.unreadable.extend(errors);
    let today = chrono::Utc::now()
        .with_timezone(&solstone_core_journal_config::owner_zone(journal))
        .date_naive();
    let mut candidates = Vec::new();
    for record in records.into_iter().filter(|r| {
        r.unit.day() == day && matches!(r.unit, Unit::Segment { .. } | Unit::Activity { .. })
    }) {
        let id = record.unit.id()?;
        let current = match &record.unit {
            Unit::Activity { .. } => crate::activity_held_input(journal, &record.unit),
            _ => work_retry::input::current(journal, &record.unit).map(Some),
        };
        let input = match current {
            Ok(Some(input)) => Some(input),
            Ok(None) => continue,
            Err(error) => {
                report.unreadable.push(format!("{day}: {error}"));
                None
            }
        };
        if let Some(intent) = intent {
            work_retry::request_with_id_checked(journal, &record.unit, &intent.id, checkpoint)?;
        }
        let record = work_retry::read(journal, &id)?.ok_or("missing talent retry")?;
        let grant = input
            .as_ref()
            .and_then(|input| record.permission(input, work_retry::DECLARED_FIXES));
        let imported = retry_imported(&record.unit, imports);
        let explicit = match grant.as_ref().map(|a| a.cause.as_str()) {
            Some(cause) if cause.starts_with("owner:") => Some(WorkBand::Waiting),
            Some(cause) if cause.starts_with("fix:") => Some(WorkBand::Completing),
            _ => imported.then_some(WorkBand::Import),
        };
        let (segment, stream) = match &record.unit {
            Unit::Segment {
                segment, stream, ..
            } => (Some(segment.clone()), Some(stream.clone())),
            _ => (None, None),
        };
        let p = priority(day, segment.as_deref(), today, explicit);
        report.holds.push(WorkHold {
            retry_unit: Some(id.clone()),
            day: day.into(),
            segment,
            stream,
            band: p.band,
            reason: record.reason.clone(),
        });
        if matches!(record.unit, Unit::Activity { .. }) {
            report.band(p.band).activities_owed += 1;
        }
        let Some(grant) = grant else {
            if matches!(record.unit, Unit::Activity { .. }) {
                *permanently_held_activities += 1;
            }
            continue;
        };
        if !policy.permits(p.band) {
            report.band(p.band).permission_held += 1;
            continue;
        }
        report.band(p.band).eligible += 1;
        candidates.push(DiscoveredTask {
            request: BusTaskRequest {
                cmd: TaskArgv::from_wire(canonical_journal_command([
                    "think",
                    "--day",
                    day,
                    "--retry-unit",
                    &id,
                    "--retry-cause",
                    &grant.cause,
                ]))
                .expect("talent retry command"),
                reference: format!("supervisor-retry-talent-{day}-{id}"),
                day: Some(day.into()),
                scheduler_name: None,
                queue_if_active_cmd_differs: false,
                priority: Some(p),
                daily_catchup_provenance: None,
            },
        });
    }
    Ok(candidates)
}

struct DaySources<'a> {
    imports: &'a [ImportThinking],
    archive_coordinates: &'a BTreeSet<Coordinate>,
    roots: Option<&'a DiscoveryTalentRoots>,
}

#[allow(
    clippy::too_many_arguments,
    reason = "The discovery inputs and bound publication checkpoint remain explicit."
)]
fn inspect_day(
    journal: &Path,
    day: &str,
    queue: &TaskQueue,
    intent: Option<&OwnerIntent>,
    sources: DaySources<'_>,
    policy: WorkPolicy,
    report: &mut WorkInventory,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<DayWork, String> {
    let now = chrono::Utc::now();
    let today = now
        .with_timezone(&solstone_core_journal_config::owner_zone(journal))
        .date_naive();
    let mut work = DayWork {
        candidates: Vec::new(),
        completed_segments: BTreeSet::new(),
        daily_current: false,
        history: HistoryDay::default(),
    };
    if let Some(intent) = intent.filter(|intent| intent.from_scratch) {
        work.history.owed = 1;
        if intent.state == IntentState::Queued {
            let p = priority(day, None, today, Some(WorkBand::Waiting));
            report.band(p.band).owner_requests += 1;
            if policy.permits(p.band) {
                work.candidates.push(task(day, None, p, Some(intent)));
                report.band(p.band).eligible += 1;
            } else {
                report.band(p.band).permission_held += 1;
                held(report, day, None, p.band, "thinking_unavailable");
            }
        } else {
            held(
                report,
                day,
                None,
                WorkBand::Waiting,
                if intent.state == IntentState::Claimed {
                    "interrupted_from_scratch"
                } else {
                    "failed_from_scratch"
                },
            );
        }
        return Ok(work);
    }
    let selected: Vec<_> = sources
        .archive_coordinates
        .iter()
        .filter(|c| c.day == day)
        .map(|c| (c.stream.clone(), c.segment.clone()))
        .collect();
    let (_, _, segments) = solstone_core_system_health::scan_day(
        &solstone_core_system_health::ArchiveFrontierSegmentSource {
            selected: &selected,
        },
        journal,
        day,
        now,
    )
    .map_err(|error| error.to_string())?;
    let progress = solstone_core_system_health::read_segment_progress(
        &solstone_core_system_health::FilesystemHealthLogSource::new(journal),
        day,
    )
    .map_err(|error| error.to_string())?;
    if progress.malformed_line_count != 0 {
        return Err("segment progress contains unreadable records".into());
    }
    // One read of the day's logs; never one per segment.
    let failures = solstone_core_system_health::read_segment_talent_failures(
        &solstone_core_system_health::FilesystemHealthLogSource::new(journal),
        day,
    )
    .map_err(|error| error.to_string())?;
    if failures.malformed_line_count != 0 {
        return Err("floor failure evidence is unreadable".into());
    }
    // Ready also needs every segment's saved outputs published; thought does
    // not, as daily work publishes those itself.
    let mut prerequisites_ready = true;
    let mut segments_thought = true;
    let mut owed_segments = Vec::new();
    for segment in &segments {
        if is_mcp_audit_segment(Some(&segment.stream)) {
            continue;
        }
        // Archive ASR belongs to the shared Sense workers. A segment repair
        // would create a second handler pool and bypass live-file priority.
        if segment.stream == "import.archive"
            && !solstone_core_system_health::segment_fully_sensed(&segment.data_state)
        {
            prerequisites_ready = false;
            segments_thought = false;
            work.history.owed += 1;
            continue;
        }
        let mut permanently_held = false;
        work.candidates.extend(raw_retries(
            journal,
            day,
            segment,
            intent,
            sources.imports,
            policy,
            &failures.value.capped,
            &mut permanently_held,
            report,
            checkpoint,
        )?);
        let coordinate = Coordinate {
            day: day.into(),
            stream: segment.stream.clone(),
            segment: segment.key.clone(),
        };
        let imported = sources
            .imports
            .iter()
            .any(|import| import.segments.contains(&coordinate));
        let explicit = intent
            .map(|_| WorkBand::Waiting)
            .or_else(|| (segment.stream == "import.archive").then_some(WorkBand::Import))
            .or_else(|| imported.then_some(WorkBand::Import));
        let p = priority(day, Some(&segment.key), today, explicit);
        let readiness =
            solstone_core_system_health::segment_readiness(journal, day, segment, &progress.value)?;
        if readiness == SegmentReadiness::Ready {
            report.band(p.band).segments_complete += 1;
            work.completed_segments.insert(coordinate);
            continue;
        }
        prerequisites_ready = false;
        let thinking_held = if readiness == SegmentReadiness::Owed {
            held_thinking(
                journal,
                day,
                segment,
                &progress.value,
                &failures.value.repeated,
                checkpoint,
            )?
        } else {
            None
        };
        if permanently_held || thinking_held == Some(HeldThinking::SetAside) {
            work.history.set_aside += 1;
            work.history.segments_set_aside += u64::from(p.band == WorkBand::Completing);
        } else {
            work.history.owed += 1;
        }
        if readiness == SegmentReadiness::Owed {
            segments_thought = false;
        }
        // Held thinking is retried one talent at a time, by its own cause.
        // Repairing the segment again would only repeat its failures.
        if thinking_held.is_some() {
            continue;
        }
        owed_segments.push((segment, p, readiness));
    }
    work.history.archive_pending =
        solstone_core_import::audio_archive::archive_day_has_pending(journal, day)
            .map_err(|error| error.to_string())?;
    let closes_day = if work.history.archive_pending {
        // Coverage projects the whole day's sources. Pending archive parts
        // already prohibit daily work, so do not walk their unselected tiles
        // just to rediscover that prerequisite. Segment work still proceeds.
        held(report, day, None, WorkBand::Import, "segment_prerequisites");
        false
    } else {
        let closing = day_still_closing(journal, queue, SystemTime::now()).as_deref() == Some(day);
        let imported = sources
            .imports
            .iter()
            .any(|import| import.days.contains(day));
        let explicit = intent
            .map(|_| WorkBand::Waiting)
            .or_else(|| imported.then_some(WorkBand::Import));
        let coverage = read_coverage(journal, day, sources.roots)?;
        let p = daily_priority(priority(day, None, today, explicit), &coverage);
        let mut permanently_held_daily = BTreeSet::new();
        work.candidates.extend(daily_retries(
            journal,
            day,
            &coverage,
            sources.roots,
            intent,
            sources.imports,
            policy,
            prerequisites_ready && !closing,
            &mut permanently_held_daily,
            report,
            checkpoint,
        )?);
        // A daily output waiting solely on set-aside recordings, or on their
        // set-aside thinking, is also set aside for the range. Ordinary
        // prerequisites remain work owed.
        let permanently_blocked =
            !segments_thought && work.history.owed == 0 && work.history.set_aside > 0;
        for covered in &coverage.units {
            let unit = Unit::Daily {
                day: day.into(),
                talent: covered.identity.name.clone(),
                facet: covered.identity.facet.clone(),
            };
            let id = unit.id()?;
            let hold = report
                .holds
                .iter()
                .find(|hold| hold.retry_unit.as_ref() == Some(&id));
            if covered.state.is_owed() || hold.is_some() {
                if permanently_blocked || permanently_held_daily.contains(&id) {
                    work.history.set_aside += 1;
                    work.history.daily_units_set_aside += u64::from(p.band >= WorkBand::Completing);
                } else if p.band != WorkBand::Remake {
                    // Updating an older day's outputs (band 6) is still counted as
                    // left, but it runs after history and must not hold the range.
                    work.history.owed += 1;
                }
            }
        }
        let queued = day < today.format("%Y%m%d").to_string().as_str()
            && queue_daily(
                journal,
                day,
                &coverage,
                segments_thought && !closing,
                prerequisites_ready,
                intent,
                p,
                policy,
                &mut work,
                report,
            )?
            && work
                .candidates
                .last()
                .is_some_and(|candidate| queue.admits_command(candidate.request.cmd.as_wire()));
        // A band-6 daily runs only after all history, so a segment owed only
        // its publication must not wait for it: the range would stop there.
        queued && p.band != WorkBand::Remake
    };
    for (segment, p, readiness) in owed_segments {
        report.band(p.band).segments_owed += 1;
        // The day's daily work publishes the segments only owed publication.
        if closes_day && readiness == SegmentReadiness::Unpublished {
            continue;
        }
        let coordinate = Coordinate {
            day: day.into(),
            stream: segment.stream.clone(),
            segment: segment.key.clone(),
        };
        if segment.stream == "import.archive" && !sources.archive_coordinates.contains(&coordinate)
        {
            continue;
        }
        if policy.permits(p.band) {
            report.band(p.band).eligible += 1;
            work.candidates
                .push(task(day, Some((&segment.key, &segment.stream)), p, None));
        } else {
            report.band(p.band).permission_held += 1;
            held(
                report,
                day,
                Some((&segment.key, &segment.stream)),
                p.band,
                "spending_confirmation",
            );
        }
    }
    Ok(work)
}

/// Account for a closed day's daily units and queue its daily work when its
/// segments are thought. Returns whether that work was queued.
#[allow(clippy::too_many_arguments)]
fn queue_daily(
    journal: &Path,
    day: &str,
    coverage: &daily_coverage::DailyCoverage,
    segments_thought: bool,
    prerequisites_ready: bool,
    intent: Option<&OwnerIntent>,
    p: WorkPriority,
    policy: WorkPolicy,
    work: &mut DayWork,
    report: &mut WorkInventory,
) -> Result<bool, String> {
    let owed = coverage
        .units
        .iter()
        .filter(|unit| unit.state.is_owed())
        .count() as u64;
    report.band(p.band).daily_units_complete += coverage
        .units
        .iter()
        .filter(|unit| unit.state.is_current())
        .count() as u64;
    if coverage
        .units
        .iter()
        .any(|unit| unit.state == CoverageState::HistoricalUnverified)
    {
        report
            .unreadable
            .push(format!("{day}: unsupported daily evidence contract"));
    }
    report.band(p.band).daily_units_owed += owed;
    work.daily_current = coverage.state.is_current()
        && prerequisites_ready
        && !report.holds.iter().any(|hold| hold.retry_unit.is_some());
    if coverage.state == CoverageState::Unreadable {
        return Err("daily coverage is unreadable".into());
    }
    if owed == 0 {
        return Ok(false);
    }
    if !segments_thought {
        report.band(p.band).prerequisite_held += owed;
        held(report, day, None, p.band, "segment_prerequisites");
    } else if !policy.permits(p.band) {
        report.band(p.band).permission_held += owed;
        held(report, day, None, p.band, "spending_confirmation");
    } else if intent.is_none()
        && !solstone_core_system::catchup::eligible_or_fail_open(journal, day, SystemTime::now())
    {
        held(report, day, None, p.band, "retry_wait");
    } else {
        report.band(p.band).eligible += owed;
        work.candidates.push(task(day, None, p, None));
        return Ok(true);
    }
    Ok(false)
}

fn retain_best(candidates: &mut Vec<DiscoveredTask>, new: Vec<DiscoveredTask>) {
    candidates.extend(new);
    candidates.sort_by_key(|candidate| {
        candidate
            .request
            .priority
            .expect("discovery priority")
            .sort_key()
    });
    candidates.dedup_by(|a, b| a.request.cmd == b.request.cmd);
    candidates.truncate(RANKED);
}

#[cfg(test)]
thread_local! {
    /// How many times this thread checked whether a candidate is still owed.
    static STILL_OWED_CHECKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// A candidate's readiness depends on its own coordinate, never its siblings.
struct ExactSegmentSource<'a> {
    stream: &'a str,
    key: &'a str,
}

impl solstone_core_system_health::SegmentSource for ExactSegmentSource<'_> {
    fn segments(
        &self,
        journal: &Path,
        day: &str,
    ) -> Result<Vec<solstone_core_journal_io::Segment>, solstone_core_system_health::HealthError>
    {
        solstone_core_journal_io::Segment::resolve_exact(journal, day, self.stream, self.key)
            .map(|segment| segment.into_iter().collect())
            .map_err(|error| solstone_core_system_health::HealthError::Source(error.to_string()))
    }
}

fn still_owed(
    journal: &Path,
    candidate: &DiscoveredTask,
    roots: Option<&DiscoveryTalentRoots>,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<bool, String> {
    #[cfg(test)]
    STILL_OWED_CHECKS.with(|checks| checks.set(checks.get() + 1));
    let day = candidate.request.day.as_deref().ok_or("work has no day")?;
    let argv = candidate.request.cmd.as_wire();
    let argument = |flag: &str| {
        argv.windows(2)
            .find(|pair| pair[0] == flag)
            .map(|pair| pair[1].as_str())
    };
    if let Some(id) = argument("--retry-unit") {
        let Some(record) = work_retry::read(journal, id)? else {
            return Ok(false);
        };
        let (input, reason) = match &record.unit {
            Unit::Raw { output, .. } => {
                let output = record.unit.segment_directory(journal)?.join(output);
                let Some(evidence) = work_retry::raw::inspect_output(journal, &output)? else {
                    return Ok(false);
                };
                (evidence.input, evidence.reason)
            }
            Unit::Daily { .. } => {
                if solstone_core_import::audio_archive::archive_day_has_pending(journal, day)
                    .map_err(|error| error.to_string())?
                    || !solstone_core_system_health::day_segments_ready(journal, day)?
                {
                    return Ok(false);
                }
                let Some(input) = observe_daily_retry(journal, &record.unit, roots, checkpoint)?
                else {
                    return Ok(false);
                };
                (input, record.reason.clone())
            }
            Unit::Segment { .. } => (
                work_retry::input::current(journal, &record.unit)?,
                record.reason.clone(),
            ),
            Unit::Activity { .. } => {
                let Some(input) = crate::activity_held_input(journal, &record.unit)? else {
                    return Ok(false);
                };
                (input, record.reason.clone())
            }
        };
        return Ok(record
            .permission(&input, work_retry::DECLARED_FIXES)
            .is_some_and(|a| {
                Some(a.cause.as_str()) == argument("--retry-cause") && a.reason == reason
            }));
    }
    if let Some(activity) = argument("--activity") {
        return crate::activity_retry_due(
            journal,
            &crate::ActivityRetry {
                day: day.into(),
                facet: argument("--facet").ok_or("activity has no facet")?.into(),
                activity: activity.into(),
            },
            chrono::Utc::now().timestamp_millis(),
        );
    }
    if let Some(id) = argument("--work-intent") {
        return Ok(work_intent::read(journal)?.get(day).is_some_and(|intent| {
            intent.id == id && intent.from_scratch && intent.state == IntentState::Queued
        }));
    }
    if let Some(segment) = argument("--segment") {
        let stream = argument("--stream").ok_or("work has no stream")?;
        let (_, _, segments) = solstone_core_system_health::scan_day(
            &ExactSegmentSource {
                stream,
                key: segment,
            },
            journal,
            day,
            chrono::Utc::now(),
        )
        .map_err(|error| error.to_string())?;
        let Some(segment) = segments
            .iter()
            .find(|entry| entry.key == segment && entry.stream == stream)
        else {
            return Ok(false);
        };
        let progress = solstone_core_system_health::read_segment_progress(
            &solstone_core_system_health::FilesystemHealthLogSource::new(journal),
            day,
        )
        .map_err(|error| error.to_string())?;
        if progress.malformed_line_count != 0 {
            return Err("segment progress contains unreadable records".into());
        }
        return solstone_core_system_health::segment_work_ready(
            journal,
            day,
            segment,
            &progress.value,
        )
        .map(|ready| !ready);
    }
    if solstone_core_import::audio_archive::archive_day_has_pending(journal, day)
        .map_err(|error| error.to_string())?
    {
        return Ok(false);
    }
    Ok(read_coverage(journal, day, roots)?.state.is_owed()
        && solstone_core_system_health::day_unpublished_segments(journal, day)?.is_some())
}

/// Hand the queue `current` and then the best of `ranked` still owed, up to
/// its admission bound, replacing the discovered work it has not started.
/// Returns what was handed over; `ranked` keeps what was not examined.
#[allow(
    clippy::too_many_arguments,
    reason = "Admission retains the pass resources and publication checkpoint."
)]
fn admit(
    journal: &Path,
    queue: &TaskQueue,
    stopped: &AtomicBool,
    mut current: Vec<DiscoveredTask>,
    ranked: &mut VecDeque<DiscoveredTask>,
    report: &mut WorkInventory,
    roots: Option<&DiscoveryTalentRoots>,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<Vec<DiscoveredTask>, String> {
    if stopped.load(Ordering::Acquire) {
        return Ok(Vec::new());
    }
    while current.len() < DISCOVERED_ADMISSION
        && let Some(candidate) = ranked.pop_front()
    {
        if stopped.load(Ordering::Acquire) {
            return Ok(Vec::new());
        }
        match still_owed(journal, &candidate, roots, checkpoint) {
            Ok(true) => {
                current.push(candidate);
            }
            Ok(false) => {}
            Err(error) => {
                report.unreadable.push(error);
                report.complete = false;
            }
        }
    }
    if stopped.load(Ordering::Acquire) {
        return Ok(Vec::new());
    }
    checkpoint()?;
    queue.replace_discovered(current.clone());
    Ok(current)
}

/// Where a segment's unfinished thinking stands when every talent it still
/// waits on is held after failing again and again on the same input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HeldThinking {
    /// Nothing will retry it until its input changes, the owner asks, or a
    /// release fixes the reason.
    SetAside,
    /// One of its holds has a cause to retry, which talent retries run.
    Retrying,
}

/// Hold each talent this segment still waits on that keeps failing on its
/// current input, then say whether all of its waiting thinking is held.
/// `None` is ordinary work: something it waits on is not held, its input is
/// newer than its last Sense read, or the evidence cannot be read.
fn held_thinking(
    journal: &Path,
    day: &str,
    segment: &solstone_core_system_health::DaySegment,
    progress: &BTreeMap<
        solstone_core_system_health::SegmentIdentity,
        solstone_core_system_health::SegmentProgress,
    >,
    repeated: &BTreeMap<
        solstone_core_system_health::TerminalUnit,
        solstone_core_system_health::RepeatedFailure,
    >,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<Option<HeldThinking>, String> {
    if !solstone_core_system_health::segment_fully_sensed(&segment.data_state) {
        return Ok(None);
    }
    let thought = solstone_core_system_health::lookup_segment_progress(
        progress,
        &segment.stream,
        &segment.key,
    );
    let pending = solstone_core_system_health::segment_pending_talents(thought);
    if pending.is_empty()
        || pending.iter().any(|name| {
            name == "facet_routing"
                || solstone_core_system_health::SEGMENT_FLOOR_TALENTS.contains(&name.as_str())
        })
    {
        return Ok(None);
    }
    let direct = matches!(
        segment.stream_layout,
        solstone_core_journal_io::SegmentLayout::Direct
    );
    // Only a segment with a failure pattern or a hold reads its files: an
    // ordinary backlog costs one lookup per waiting talent.
    let mut held_talents = Vec::new();
    for name in &pending {
        let unit = Unit::Segment {
            day: day.into(),
            stream: segment.stream.clone(),
            segment: segment.key.clone(),
            talent: name.clone(),
            direct,
        };
        let id = unit.id()?;
        let failing = repeated.get(&solstone_core_system_health::TerminalUnit {
            day: day.into(),
            mode: "segment".into(),
            name: name.clone(),
            facet: None,
            stream: Some(segment.stream.clone()),
            segment: Some(segment.key.clone()),
            activity: None,
        });
        let is_held = work_retry::read(journal, &id)?.is_some_and(|record| record.held);
        if failing.is_none() && !is_held {
            return Ok(None);
        }
        held_talents.push((unit, id, failing, is_held));
    }
    let directory = if direct {
        journal.join("chronicle").join(day).join(&segment.key)
    } else {
        journal
            .join("chronicle")
            .join(day)
            .join(&segment.stream)
            .join(&segment.key)
    };
    let newest = solstone_core_system_health::newest_segment_input_ms(&directory)
        .map_err(|error| error.to_string())?;
    let read_current = thought
        .and_then(|progress| progress.sense_input_ms)
        .is_some_and(|read| newest.is_none_or(|newest| newest < read));
    // Input newer than the last Sense read is ordinary work: Sense reads it first.
    if !pending.contains("sense") && !read_current {
        return Ok(None);
    }
    let mut held = HeldThinking::SetAside;
    for (unit, id, failing, is_held) in held_talents {
        let Ok(input) = work_retry::input::current(journal, &unit) else {
            return Ok(None);
        };
        // Failures on input older than what the segment holds now say
        // nothing about it.
        if let Some(failure) =
            failing.filter(|failure| newest.is_none_or(|newest| failure.latest_ts > newest))
            && !is_held
        {
            work_retry::observe_held_checked(
                journal,
                &unit,
                &failure.reason_code,
                &input,
                checkpoint,
            )?;
        }
        match work_retry::read(journal, &id)?.filter(|record| record.held) {
            None => return Ok(None),
            Some(record)
                if record
                    .permission(&input, work_retry::DECLARED_FIXES)
                    .is_some() =>
            {
                held = HeldThinking::Retrying;
            }
            Some(_) => {}
        }
    }
    Ok(Some(held))
}

/// All historical days after the newest day still owed its recordings or its
/// first daily outputs are caught up. Today and
/// yesterday are outside this range; unreadable days and permanent holds are
/// counted separately. This folds only evidence already read by the pass.
fn history_progress(
    report: &WorkInventory,
    days: &BTreeMap<String, (HistoryDay, bool)>,
    today: chrono::NaiveDate,
    previous: Option<&HistorySample>,
) -> HistoryProgress {
    let through = today - chrono::Duration::days(2);
    let through_day = through.format("%Y%m%d").to_string();
    let historical = days
        .iter()
        .filter(|(day, _)| *day <= &through_day)
        .collect::<Vec<_>>();
    let newest_owed = historical
        .iter()
        .rev()
        .find(|(_, (work, unreadable))| !unreadable && (work.owed > 0 || work.archive_pending))
        .map(|(day, _)| day.as_str());
    let caught_up_since = match newest_owed {
        Some(day) if day == through_day => None,
        Some(day) => chrono::NaiveDate::parse_from_str(day, "%Y%m%d")
            .ok()
            .and_then(|day| day.succ_opt())
            .map(|day| day.format("%Y%m%d").to_string()),
        None => historical.first().map(|(day, _)| (*day).clone()),
    };
    let readable = days
        .values()
        .filter(|(_, unreadable)| !unreadable)
        .map(|(work, _)| work)
        .collect::<Vec<_>>();
    let segments_set_aside = readable.iter().map(|work| work.segments_set_aside).sum();
    let daily_units_set_aside = readable.iter().map(|work| work.daily_units_set_aside).sum();
    let segments_owed = readable
        .iter()
        .map(|work| work.segments_owed)
        .sum::<u64>()
        .saturating_sub(segments_set_aside);
    let daily_units_owed = readable
        .iter()
        .map(|work| work.daily_units_owed)
        .sum::<u64>()
        .saturating_sub(daily_units_set_aside);
    let mut unreadable = report.unreadable.clone();
    unreadable.sort();
    unreadable.dedup();
    let sample = (report.days_inspected == report.days_total
        && !days.values().any(|(work, _)| work.archive_pending))
    .then(|| HistorySample {
        local_day: today.format("%Y%m%d").to_string(),
        journal_version: env!("CARGO_PKG_VERSION").into(),
        as_of_ms: report.as_of_ms,
        owed: segments_owed + daily_units_owed,
        set_aside: segments_set_aside + daily_units_set_aside,
        unreadable,
    });
    let net_per_hour = sample
        .as_ref()
        .zip(previous)
        .and_then(|(current, previous)| current.net_per_hour(previous));
    HistoryProgress {
        caught_up_since,
        through_day,
        segments_owed,
        daily_units_owed,
        set_aside_units: historical.iter().map(|(_, (work, _))| work.set_aside).sum(),
        unreadable_days: historical
            .iter()
            .filter(|(_, (_, unreadable))| *unreadable)
            .count(),
        sample,
        net_per_hour,
    }
}

/// Fold the per-day snapshots into the published totals. Built only when it
/// is about to be published: the holds grow with the journal.
fn aggregate(
    report: &mut WorkInventory,
    day_reports: &BTreeMap<String, WorkInventory>,
    import_errors: &[String],
    days: &BTreeMap<String, PathBuf>,
    days_inspected: usize,
) {
    report.bands = std::array::from_fn(|_| Default::default());
    report.holds.clear();
    report.unreadable = import_errors.to_vec();
    for day_report in day_reports.values() {
        for (total, day) in report.bands.iter_mut().zip(&day_report.bands) {
            total.add(day);
        }
        report.holds.extend(day_report.holds.iter().cloned());
        report
            .unreadable
            .extend(day_report.unreadable.iter().cloned());
    }
    report.days_inspected = days_inspected;
    report.days_total = days.len();
    report.oldest_day = days.keys().next().cloned();
    report.newest_day = days.keys().next_back().cloned();
    report.as_of_ms = chrono::Utc::now().timestamp_millis();
}

#[cfg(test)]
fn reconcile(journal: &Path, queue: &TaskQueue, stopped: &AtomicBool) -> Result<(), String> {
    reconcile_pass(journal, queue, stopped, false).map(|_| ())
}

/// An automatic pass says when the next one is due, so its snapshot stays
/// current while discovery rests rather than being rewritten to look fresh.
fn reconcile_pass(
    journal: &Path,
    queue: &TaskQueue,
    stopped: &AtomicBool,
    automatic: bool,
) -> Result<Backlog, String> {
    reconcile_pass_checked(journal, queue, stopped, automatic, &|| {
        if stopped.load(Ordering::Acquire) {
            Err("work discovery stopped".into())
        } else {
            Ok(())
        }
    })
}

fn reconcile_pass_checked(
    journal: &Path,
    queue: &TaskQueue,
    stopped: &AtomicBool,
    automatic: bool,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<Backlog, String> {
    reconcile_pass_with_roots(journal, queue, stopped, automatic, None, checkpoint)
}

fn reconcile_pass_with_roots(
    journal: &Path,
    queue: &TaskQueue,
    stopped: &AtomicBool,
    automatic: bool,
    roots: Option<&DiscoveryTalentRoots>,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<Backlog, String> {
    if let Some(roots) = roots {
        roots.revalidate()?;
    }
    let external_checkpoint = checkpoint;
    let checkpoint = &|| {
        if stopped.load(Ordering::Acquire) {
            Err("work discovery stopped".into())
        } else {
            external_checkpoint()
        }
    };
    checkpoint()?;
    let pass_started = Instant::now();
    // One small reporting snapshot, before this pass publishes its progress.
    // No day or segment is read again to obtain the rate baseline.
    let (last_history, last_current) = match solstone_core_system::work_inventory::read(journal) {
        Ok(inventory) => (
            inventory
                .as_ref()
                .and_then(|inventory| inventory.history.clone()),
            inventory.is_some_and(|inventory| {
                inventory.current_at(chrono::Utc::now().timestamp_millis())
            }),
        ),
        Err(error) => {
            log::warn!("previous history progress unreadable: {error}");
            (None, false)
        }
    };
    let policy = WorkPolicy::read(journal)?;
    let mut days = solstone_core_journal_io::day_dirs(journal)
        .map_err(|error| error.to_string())?
        .into_iter()
        .collect::<BTreeMap<_, _>>();
    let imports = solstone_core_import::thinking::read(journal)?;
    let pending_archives =
        match solstone_core_import::audio_archive::update_pending_archive_tiles_checked(
            journal, 64, checkpoint,
        ) {
            Ok(batch) => {
                for tile in &batch.tiles {
                    days.entry(tile.day.clone())
                        .or_insert_with(|| journal.join("chronicle").join(&tile.day));
                }
                batch
            }
            Err(error) => {
                log::warn!("pending audio archive batch unreadable: {error}");
                solstone_core_import::audio_archive::PendingArchiveBatch {
                    errors: vec![("archive".to_string(), error.to_string())],
                    ..Default::default()
                }
            }
        };
    let (activities, activity_errors) =
        crate::discover_activity_retries(journal, chrono::Utc::now().timestamp_millis());
    let (held_activities, held_errors) = crate::discover_held_activities(journal);
    let (held_records, retry_errors) = work_retry::list(journal);
    let now = chrono::Utc::now();
    let today = daily_coverage::local_day(journal, now);
    let pass_day = today.clone();
    solstone_core_system::daily_coverage::activate_daily_inputs_checked(journal, now, checkpoint)?;
    let maintenance =
        solstone_core_journal_io::DailyUnitIdentity::new(&today, "daily_schedule", None);
    if solstone_core_journal_io::load_daily_unit_record(journal, &maintenance)
        .map_err(|e| e.to_string())?
        .is_some_and(|record| record.status == solstone_core_journal_io::DailyUnitStatus::Capped)
    {
        days.entry(today.clone())
            .or_insert_with(|| journal.join("chronicle").join(today));
    }

    for unit in held_activities
        .iter()
        .chain(held_records.iter().map(|r| &r.unit))
    {
        days.entry(unit.day().into())
            .or_insert_with(|| journal.join("chronicle").join(unit.day()));
    }
    for activity in &activities {
        days.entry(activity.day.clone())
            .or_insert_with(|| journal.join("chronicle").join(&activity.day));
    }
    let mut report = WorkInventory {
        days_total: days.len(),
        oldest_day: days.keys().next().cloned(),
        newest_day: days.keys().next_back().cloned(),
        unreadable: imports.unreadable,
        ..WorkInventory::default()
    };
    report.unreadable.extend(activity_errors);
    report.unreadable.extend(held_errors);
    report.unreadable.extend(retry_errors);
    for (id, error) in pending_archives.errors {
        report.unreadable.push(format!("{id}: {error}"));
    }
    let mut archive_coordinates = BTreeSet::new();
    for tile in &pending_archives.tiles {
        archive_coordinates.insert(Coordinate {
            day: tile.day.clone(),
            stream: tile.stream.clone(),
            segment: tile.segment_key.clone(),
        });
    }
    // Until this pass can say more, its progress carries the last finished
    // pass's history line: only while that line was still current, and not
    // when this pass could not have produced one.
    if last_current && report.unreadable.is_empty() {
        report.history = last_history.clone();
    }
    report.publish_checked(journal, checkpoint)?;
    let mut last_publish = Instant::now();
    let mut candidates = Vec::new();
    let mut day_reports = BTreeMap::<String, WorkInventory>::new();
    let mut history_days = BTreeMap::new();
    let import_errors = report.unreadable.clone();
    let mut inspected = BTreeSet::new();
    let mut observed_intents = BTreeMap::new();
    let mut fulfilled_segments = BTreeSet::new();
    let mut fulfilled_days = BTreeSet::new();
    // The early work last handed to the queue, and the references it was
    // chosen from. Checking whether work is still owed can read a whole day,
    // so the early work is admitted again only when it could change.
    let mut early_considered: Option<BTreeSet<String>> = None;
    let mut early_admitted: Vec<DiscoveredTask> = Vec::new();
    // Check owner requests before the history pass and between days. Source
    // reads run only here, never on the supervisor's event loop.
    let mut ordered = days.keys().rev().cloned().collect::<VecDeque<_>>();
    loop {
        if stopped.load(Ordering::Acquire) || checkpoint().is_err() {
            return Ok(Backlog::default());
        }
        let intents = work_intent::read(journal)?;
        let owner_day = intents
            .iter()
            .rev()
            .find(|(day, intent)| observed_intents.get(*day) != Some(&intent.id))
            .map(|(day, _)| day.clone());
        let owner_wakeup = owner_day.is_some();
        let day = match owner_day.or_else(|| {
            while let Some(day) = ordered.pop_front() {
                if !inspected.contains(&day) {
                    return Some(day);
                }
            }
            None
        }) {
            Some(day) => day,
            None => break,
        };
        if let Some(intent) = intents.get(&day) {
            observed_intents.insert(day.clone(), intent.id.clone());
        }
        // An owner can replace an intent while its day is being inspected.
        // Replace that day's snapshot and queued contribution; never add it twice.
        candidates
            .retain(|candidate: &DiscoveredTask| candidate.request.day.as_deref() != Some(&day));
        fulfilled_segments.retain(|coordinate: &Coordinate| coordinate.day != day);
        fulfilled_days.remove(&day);
        days.entry(day.clone())
            .or_insert_with(|| journal.join("chronicle").join(&day));
        let mut day_report = WorkInventory::default();
        if let Err(error) = crate::activity_work::seed_activity_retries_checked(
            journal,
            &day,
            chrono::Utc::now().timestamp_millis(),
            checkpoint,
        ) {
            day_report.unreadable.push(format!("{day}: {error}"));
        }
        let mut history_day = HistoryDay::default();
        match inspect_day(
            journal,
            &day,
            queue,
            intents.get(&day),
            DaySources {
                imports: &imports.imports,
                archive_coordinates: &archive_coordinates,
                roots,
            },
            policy,
            &mut day_report,
            checkpoint,
        ) {
            Ok(work) => {
                history_day = work.history;
                for tile in pending_archives.tiles.iter().filter(|tile| tile.day == day) {
                    let coordinate = Coordinate {
                        day: tile.day.clone(),
                        stream: tile.stream.clone(),
                        segment: tile.segment_key.clone(),
                    };
                    if !work.completed_segments.contains(&coordinate) {
                        continue;
                    }
                    let directory = journal
                        .join("chronicle")
                        .join(&tile.day)
                        .join(&tile.stream)
                        .join(&tile.segment_key);
                    for part in &tile.parts {
                        // Text-row existence proves neither current thinking
                        // nor saved-output publication. Both are required by
                        // completed_segments before retiring durable backlog.
                        if solstone_core_sense::archive::transcription_state(
                            &directory.join(&part.stored_name),
                        ) == solstone_core_sense::archive::ArchiveTranscription::Complete
                            && let Err(error) =
                                solstone_core_import::audio_archive::finish_archive_part_checked(
                                    journal,
                                    &tile.import_id,
                                    &tile.day,
                                    &tile.segment_key,
                                    &part.stored_name,
                                    false,
                                    checkpoint,
                                )
                        {
                            day_report
                                .unreadable
                                .push(format!("{}: {error}", tile.import_id));
                        }
                    }
                }
                fulfilled_segments.extend(work.completed_segments);
                if work.daily_current {
                    fulfilled_days.insert(day.clone());
                }
                retain_best(&mut candidates, work.candidates);
            }
            Err(error) => day_report.unreadable.push(format!("{day}: {error}")),
        }
        let mut permanent_activities = 0;
        if !intents.get(&day).is_some_and(|intent| intent.from_scratch) {
            match talent_retries(
                journal,
                &day,
                intents.get(&day),
                &imports.imports,
                policy,
                &mut permanent_activities,
                &mut day_report,
                checkpoint,
            ) {
                Ok(work) => retain_best(&mut candidates, work),
                Err(error) => day_report.unreadable.push(format!("{day}: {error}")),
            }
        }
        if day_report
            .holds
            .iter()
            .any(|hold| hold.retry_unit.is_some())
        {
            fulfilled_days.remove(&day);
        }
        if fulfilled_days.contains(&day)
            && !activities.iter().any(|a| a.day == day)
            && let Some(intent) = intents.get(&day).filter(|intent| !intent.from_scratch)
        {
            work_intent::retire_current_checked(journal, &day, &intent.id, checkpoint)?;
        }
        for activity in activities.iter().filter(|a| {
            a.day == day && !intents.get(&day).is_some_and(|intent| intent.from_scratch)
        }) {
            let today = chrono::Utc::now()
                .with_timezone(&solstone_core_journal_config::owner_zone(journal))
                .date_naive();
            let p = priority(
                &day,
                None,
                today,
                intents.get(&day).map(|_| WorkBand::Waiting),
            );
            day_report.band(p.band).activities_owed += 1;
            if !policy.permits(p.band) {
                day_report.band(p.band).permission_held += 1;
                held(&mut day_report, &day, None, p.band, "spending_confirmation");
                continue;
            }
            day_report.band(p.band).eligible += 1;
            let identity = solstone_core_system::work_retry::Unit::Activity {
                day: day.clone(),
                facet: activity.facet.clone(),
                activity: activity.activity.clone(),
                talent: "activity".into(),
            };
            let reference = format!("supervisor-activity-{day}-{}", identity.id()?);
            let candidate = DiscoveredTask {
                request: BusTaskRequest {
                    cmd: TaskArgv::from_wire(canonical_journal_command([
                        "think",
                        "--day",
                        &day,
                        "--facet",
                        &activity.facet,
                        "--activity",
                        &activity.activity,
                    ]))
                    .expect("activity command"),
                    reference,
                    day: Some(day.clone()),
                    scheduler_name: None,
                    queue_if_active_cmd_differs: false,
                    priority: Some(p),
                    daily_catchup_provenance: None,
                },
            };
            retain_best(&mut candidates, vec![candidate]);
        }
        history_day.owed += day_report
            .bands
            .iter()
            .map(|band| band.activities_owed)
            .sum::<u64>();
        history_day.segments_owed = day_report.bands[4].segments_owed;
        history_day.daily_units_owed =
            day_report.bands[4].daily_units_owed + day_report.bands[5].daily_units_owed;
        // Only holds without renewed permission are permanent exceptions.
        history_day.owed = history_day.owed.saturating_sub(permanent_activities);
        history_day.set_aside += permanent_activities;
        history_days.insert(
            day.clone(),
            (history_day, !day_report.unreadable.is_empty()),
        );
        inspected.insert(day.clone());
        day_reports.insert(day, day_report);
        if stopped.load(Ordering::Acquire) || checkpoint().is_err() {
            return Ok(Backlog::default());
        }
        // Progress, not a rewrite of the whole inventory after every day.
        if last_publish.elapsed() >= PROGRESS_INTERVAL {
            aggregate(
                &mut report,
                &day_reports,
                &import_errors,
                &days,
                inspected.len(),
            );
            report.publish_checked(journal, checkpoint)?;
            last_publish = Instant::now();
        }
        // Reconstruct an independent copy; TaskQueue owns submissions.
        let mut early = candidates
            .iter()
            .filter(|task| {
                task.request
                    .priority
                    .is_some_and(|p| p.band <= WorkBand::Waiting)
            })
            .cloned()
            .collect::<VecDeque<_>>();
        if owner_wakeup || !early.is_empty() {
            // Admit again only when an owner asked, this day changed the early
            // work, or early work handed over before has left the queue.
            // Otherwise the result is the one already handed over, and checking
            // it again after every older day re-reads the newest days each time.
            let considered = early
                .iter()
                .map(|task| task.request.reference.clone())
                .collect::<BTreeSet<_>>();
            if owner_wakeup
                || early_considered.as_ref() != Some(&considered)
                || early_admitted
                    .iter()
                    .any(|task| !queue.contains_reference(&task.request.reference))
            {
                // Track only what the queue actually took: work it skipped
                // because the same command is already queued or running under
                // another reference never "leaves", and must not re-trigger.
                early_admitted = admit(
                    journal,
                    queue,
                    stopped,
                    Vec::new(),
                    &mut early,
                    &mut report,
                    roots,
                    checkpoint,
                )?
                .into_iter()
                .filter(|task| queue.contains_reference(&task.request.reference))
                .collect();
                early_considered = Some(considered);
            }
        }
    }
    aggregate(
        &mut report,
        &day_reports,
        &import_errors,
        &days,
        inspected.len(),
    );
    for import in imports.imports {
        let segments = import
            .segments
            .intersection(&fulfilled_segments)
            .cloned()
            .collect::<BTreeSet<_>>();
        let days = import
            .days
            .intersection(&fulfilled_days)
            .cloned()
            .collect::<BTreeSet<_>>();
        if !segments.is_empty() || !days.is_empty() {
            solstone_core_import::thinking::fulfill_checked(
                journal, &import, segments, days, checkpoint,
            )?;
        }
    }
    report.complete = report.unreadable.is_empty() && report.days_inspected == report.days_total;
    let mut ranked = VecDeque::from(candidates);
    let admitted = admit(
        journal,
        queue,
        stopped,
        Vec::new(),
        &mut ranked,
        &mut report,
        roots,
        checkpoint,
    )?;
    if stopped.load(Ordering::Acquire) {
        return Ok(Backlog::default());
    }
    report.as_of_ms = chrono::Utc::now().timestamp_millis();
    report.next_pass_ms =
        automatic.then(|| report.as_of_ms + rest_after(pass_started.elapsed()).as_millis() as i64);
    let ended = chrono::Utc::now();
    // A pass that crossed midnight keeps the last line until the next pass.
    if import_errors.is_empty() && daily_coverage::local_day(journal, ended) == pass_day {
        let today = chrono::NaiveDate::parse_from_str(&pass_day, "%Y%m%d")
            .map_err(|error| error.to_string())?;
        report.history = Some(history_progress(
            &report,
            &history_days,
            today,
            last_history
                .as_ref()
                .and_then(|history| history.sample.as_ref()),
        ));
    }
    report.publish_checked(journal, checkpoint)?;
    Ok(Backlog {
        admitted,
        ranked,
        published: Some(report),
        roots: roots.cloned(),
    })
}

/// The idle flush still owed to each stream, keyed by stream.
#[derive(Default)]
pub struct FlushState {
    pub pending: BTreeMap<Option<String>, PendingFlush>,
}

/// The last segment a stream delivered, not yet flushed.
pub struct PendingFlush {
    pub last_segment_ts: Instant,
    pub day: String,
    pub segment: String,
}

pub const FLUSH_TIMEOUT: Duration = Duration::from_secs(3600);
pub const SETTLE_SCAN_INTERVAL: Duration = Duration::from_secs(30);
pub const DAY_CLOSE_GRACE: Duration = Duration::from_secs(10 * 60);
pub const DAY_CLOSE_LIMIT: Duration = Duration::from_secs(60 * 60);

pub fn flush_think_argv(day: &str, segment: &str, stream: Option<&str>) -> Vec<String> {
    let mut tail = vec![
        "think".to_owned(),
        "-v".to_owned(),
        "--day".to_owned(),
        day.to_owned(),
        "--segment".to_owned(),
        segment.to_owned(),
        "--flush".to_owned(),
    ];
    if let Some(stream) = stream {
        tail.extend(["--stream".to_owned(), stream.to_owned()]);
    }
    canonical_journal_command(tail)
}

pub fn settle_think_argv(stream: &str) -> Vec<String> {
    canonical_journal_command(["think", "-v", "--settle", "--stream", stream])
}

pub fn submit_think(
    queue: &TaskQueue,
    argv: Vec<String>,
    day: &str,
    reference: String,
) -> solstone_core_system::queue::SubmitOutcome {
    submit_task(queue, argv, reference, Some(day), None)
}

pub fn submit_task(
    queue: &TaskQueue,
    argv: Vec<String>,
    reference: String,
    day: Option<&str>,
    daily_catchup_provenance: Option<DailyCatchupProvenance>,
) -> solstone_core_system::queue::SubmitOutcome {
    let cmd = TaskArgv::from_wire(argv).expect("supervisor constructs a non-empty argv");
    queue.submit(solstone_core_system::queue::ExecutionRequest::Bus(
        BusTaskRequest {
            cmd,
            reference,
            day: day.map(str::to_owned),
            scheduler_name: None,
            queue_if_active_cmd_differs: false,
            priority: None,
            daily_catchup_provenance,
        },
    ))
}

pub fn check_segment_flush(
    journal: &Path,
    queue: &TaskQueue,
    flush: &mut FlushState,
    force_day: Option<&str>,
    now: Instant,
) {
    if flush.pending.is_empty()
        || !solstone_core_system::processing::processing_allowed(journal)
        || solstone_core_journal_config::no_thinking_engine_chosen(journal)
    {
        return;
    }
    flush.pending.retain(|stream, pending| {
        let due = force_day == Some(pending.day.as_str())
            || now.saturating_duration_since(pending.last_segment_ts) >= FLUSH_TIMEOUT;
        if due {
            let _ = submit_think(
                queue,
                flush_think_argv(&pending.day, &pending.segment, stream.as_deref()),
                &pending.day,
                format!("supervisor-flush-{}-{}", pending.day, pending.segment),
            );
        }
        !due
    });
}

pub fn check_activity_settle(
    journal: &Path,
    queue: &TaskQueue,
    last_scan: &mut Instant,
    tick: Instant,
    now_ms: i64,
    today: &str,
) {
    if tick.saturating_duration_since(*last_scan) < SETTLE_SCAN_INTERVAL
        || !solstone_core_system::processing::processing_allowed(journal)
        || solstone_core_journal_config::no_thinking_engine_chosen(journal)
    {
        return;
    }
    *last_scan = tick;
    for stream in crate::activity_settle_due(journal, now_ms) {
        let reference = format!("supervisor-settle-{stream}");
        if queue.contains_reference(&reference) {
            continue;
        }
        let _ = submit_think(queue, settle_think_argv(&stream), today, reference);
    }
}

pub fn is_mcp_audit_segment(stream: Option<&str>) -> bool {
    stream == Some("mcp.agent")
}

pub fn day_still_closing(journal: &Path, queue: &TaskQueue, now: SystemTime) -> Option<String> {
    let local_day = |at: SystemTime| {
        solstone_core_system::daily_coverage::local_day(journal, chrono::DateTime::from(at))
    };
    let day = local_day(now.checked_sub(DAY_CLOSE_LIMIT)?);
    if day == local_day(now) {
        return None;
    }
    let within_grace = now
        .checked_sub(DAY_CLOSE_GRACE)
        .is_some_and(|at| local_day(at) == day);
    let flush = format!("supervisor-flush-{day}-");
    let observed = format!("supervisor-observed-{day}-");
    let activity = format!("\"day\":\"{day}\"");
    let busy = || {
        queue.any_reference(|reference| {
            reference.starts_with(&flush)
                || reference.starts_with(&observed)
                || reference.starts_with("supervisor-settle-")
                || reference.starts_with(&format!("supervisor-retry-raw-{day}-"))
                || reference.starts_with(&format!("supervisor-retry-talent-{day}-"))
                || reference.starts_with(&format!("supervisor-activity-{day}-"))
                || (reference.starts_with("supervisor-activity-") && reference.contains(&activity))
        })
    };
    (within_grace || busy() || crate::activity_settle_holds_day(journal, &day)).then_some(day)
}

pub fn plan_observed_segment(
    journal: &Path,
    queue: &TaskQueue,
    flush: &mut FlushState,
    day: &str,
    segment: &str,
    stream: Option<&str>,
    batch: bool,
) -> bool {
    if batch {
        log::debug!("planner: batch observed segment held for daily catchup: {day}/{segment}");
        return false;
    }
    if solstone_core_system::processing::is_deferred(journal).unwrap_or(true)
        || solstone_core_journal_config::no_thinking_engine_chosen(journal)
    {
        log::debug!("planner: observed segment held by processing configuration: {day}/{segment}");
        return false;
    }
    if is_mcp_audit_segment(stream) {
        log::debug!("planner: MCP audit segment is not enriched: {day}/{segment}");
        return false;
    }
    let stream_key = stream.map(str::to_owned);
    if flush
        .pending
        .get(&stream_key)
        .is_none_or(|pending| (day, segment) >= (pending.day.as_str(), pending.segment.as_str()))
    {
        flush.pending.insert(
            stream_key,
            PendingFlush {
                last_segment_ts: Instant::now(),
                day: day.to_owned(),
                segment: segment.to_owned(),
            },
        );
    }
    let mut tail = vec![
        "think".to_owned(),
        "-v".to_owned(),
        "--day".to_owned(),
        day.to_owned(),
        "--segment".to_owned(),
        segment.to_owned(),
    ];
    if let Some(stream) = stream {
        tail.extend(["--stream".to_owned(), stream.to_owned()]);
    }
    tail.push("--live".to_owned());
    let argv = canonical_journal_command(tail);
    let _ = submit_think(
        queue,
        argv,
        day,
        format!("supervisor-observed-{day}-{segment}"),
    );
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use solstone_core_system::queue::{ProcessState, ProcessStateProbe, TaskQueueOptions};

    struct NoProcesses;
    impl ProcessStateProbe for NoProcesses {
        fn state(&self, _: u32) -> ProcessState {
            panic!("unready fixture must not launch")
        }
    }
    fn queue(journal: &Path) -> TaskQueue {
        TaskQueue::new(TaskQueueOptions {
            #[cfg(windows)]
            read_file_grants: Vec::new(),
            journal_root: journal.into(),
            cap_resolver: Arc::new(solstone_core_system::cap::DefaultCapResolver::default()),
            process_state_probe: Arc::new(NoProcesses),
            queue_sink: None,
            process_sink: None,
            ready: false,
            before_deadline_commit: None,
            child_environment: BTreeMap::new(),
            task_binary: None,
        })
    }
    fn fixture(provider: &str) -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("config")).unwrap();
        std::fs::write(root.path().join("config/journal.json"), serde_json::json!({
            "identity":{"timezone":"UTC"}, "providers":{"active":{"provider":provider,"model":"fixture"}}
        }).to_string()).unwrap();
        root
    }
    fn segment(root: &Path, day: &str, key: &str) {
        let path = root.join("chronicle").join(day).join("notes").join(key);
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(
            path.join("audio.jsonl"),
            "{\"text\":\"A meeting about a garden.\"}\n",
        )
        .unwrap();
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[cfg(unix)]
    #[test]
    fn embedded_resources_survive_discovery_and_refill_outside_a_checkout() {
        const CHILD: &str = "SOLSTONE_DISCOVERY_RESOURCE_TEST";
        if let Some(payload) = std::env::var_os(CHILD) {
            // The executable has no checkout or installed-share ancestor. This
            // reproduces an app process even though the host supplied its assets.
            assert!(daily_coverage::package_roots().is_err());
            let resources = DiscoveryTalentRoots {
                talent: PathBuf::from(&payload).join("solstone/talent"),
                apps: PathBuf::from(&payload).join("solstone/apps"),
            };
            let root = fixture("local");
            for second in 0..DISCOVERED_ADMISSION + 2 {
                segment(root.path(), "19990101", &format!("1200{second:02}_60"));
            }
            let queue = queue(root.path());
            let stopped = AtomicBool::new(false);
            let negative = reconcile_pass(root.path(), &queue, &stopped, false).unwrap();
            assert!(
                negative
                    .published
                    .unwrap()
                    .unreadable
                    .iter()
                    .any(|error| { error.contains("could not locate packaged talent roots") })
            );
            let mut backlog = reconcile_pass_with_roots(
                root.path(),
                &queue,
                &stopped,
                false,
                Some(&resources),
                &|| Ok(()),
            )
            .unwrap();
            let inventory = backlog.published.as_ref().unwrap();
            assert!(inventory.complete, "{:?}", inventory.unreadable);
            assert!(inventory.unreadable.is_empty());
            assert_eq!(backlog.admitted.len(), DISCOVERED_ADMISSION);
            let next = backlog
                .ranked
                .front()
                .expect("ranked work beyond admission")
                .request
                .reference
                .clone();
            queue.replace_discovered(Vec::new());
            backlog.top_up(root.path(), &queue, &stopped, &|| Ok(()));
            assert!(queue.contains_reference(&next));

            // The daily refill/retry reads must use those same resources, rather
            // than falling back to current_exe after the first pass succeeds.
            let today = daily_coverage::local_day(root.path(), chrono::Utc::now());
            let unit = Unit::Daily {
                day: today.clone(),
                talent: "daily_schedule".into(),
                facet: None,
            };
            let evidence = work_retry::daily::evidence_with_roots(
                root.path(),
                &unit,
                &resources.talent,
                &resources.apps,
            )
            .unwrap();
            let mut record = solstone_core_journal_io::DailyUnitRecord::new(
                work_retry::daily::identity(&unit).unwrap(),
                &evidence.revision,
                &evidence.contract,
            );
            record.status = solstone_core_journal_io::DailyUnitStatus::Capped;
            record.reason_code = Some("schema_invalid".into());
            solstone_core_journal_io::save_daily_unit_record(root.path(), &record).unwrap();
            assert!(observe_daily_retry(root.path(), &unit, None, &|| Ok(())).is_err());
            let input = observe_daily_retry(root.path(), &unit, backlog.roots.as_ref(), &|| Ok(()))
                .unwrap()
                .unwrap();
            assert!(
                work_retry::read(root.path(), &unit.id().unwrap())
                    .unwrap()
                    .unwrap()
                    .permission(&input, work_retry::DECLARED_FIXES)
                    .is_none()
            );
            work_retry::request(root.path(), &unit).unwrap();
            let granted = work_retry::read(root.path(), &unit.id().unwrap())
                .unwrap()
                .unwrap()
                .permission(&input, work_retry::DECLARED_FIXES)
                .unwrap();
            let retry = DiscoveredTask {
                request: BusTaskRequest {
                    cmd: TaskArgv::from_wire(canonical_journal_command([
                        "think",
                        "--day",
                        &today,
                        "--retry-unit",
                        &unit.id().unwrap(),
                        "--retry-cause",
                        &granted.cause,
                    ]))
                    .unwrap(),
                    reference: "resource-retry".into(),
                    day: Some(today),
                    scheduler_name: None,
                    queue_if_active_cmd_differs: false,
                    priority: None,
                    daily_catchup_provenance: None,
                },
            };
            assert!(still_owed(root.path(), &retry, backlog.roots.as_ref(), &|| Ok(())).unwrap());
            assert!(still_owed(root.path(), &retry, None, &|| Ok(())).is_err());
            reconcile_pass_with_roots(
                root.path(),
                &queue,
                &stopped,
                false,
                Some(&resources),
                &|| Ok(()),
            )
            .unwrap();
            assert!(queue.contains_reference(&format!(
                "supervisor-retry-daily-{}-{}",
                unit.day(),
                unit.id().unwrap()
            )));
            queue.replace_discovered(Vec::new());
            backlog.ranked = VecDeque::from([retry.clone()]);
            backlog.top_up(root.path(), &queue, &stopped, &|| Ok(()));
            assert!(queue.contains_reference(&retry.request.reference));
            queue.replace_discovered(Vec::new());
            backlog.ranked = VecDeque::from([retry.clone()]);
            stopped.store(true, Ordering::Release);
            assert!(!backlog.top_up(root.path(), &queue, &stopped, &|| Ok(())));
            assert!(!queue.contains_reference(&retry.request.reference));
            let missing = DiscoveryTalentRoots {
                talent: root.path().join("missing-payload"),
                apps: resources.apps.clone(),
            };
            assert!(read_coverage(root.path(), unit.day(), Some(&missing)).is_err());
            assert!(
                reconcile_pass_with_roots(
                    root.path(),
                    &queue,
                    &AtomicBool::new(false),
                    false,
                    Some(&missing),
                    &|| Ok(())
                )
                .is_err()
            );
            queue.shutdown();
            return;
        }
        fn copy_tree(from: &Path, to: &Path) {
            std::fs::create_dir_all(to).unwrap();
            for entry in std::fs::read_dir(from).unwrap() {
                let entry = entry.unwrap();
                let target = to.join(entry.file_name());
                if entry.file_type().unwrap().is_dir() {
                    copy_tree(&entry.path(), &target);
                } else {
                    std::fs::copy(entry.path(), target).unwrap();
                }
            }
        }
        let isolated = tempfile::tempdir().unwrap();
        let executable = isolated.path().join("discovery-test");
        std::fs::copy(std::env::current_exe().unwrap(), &executable).unwrap();
        let payload = isolated.path().join("payload");
        copy_tree(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../payload"),
            &payload,
        );
        let output = std::process::Command::new(executable)
            .args(["--exact", "planner::tests::embedded_resources_survive_discovery_and_refill_outside_a_checkout", "--nocapture"])
            .env(CHILD, payload)
            .output().unwrap();
        assert!(
            output.status.success(),
            "stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn history_range_keeps_prerequisites_but_excepts_young_and_unreadable_days() {
        let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 13).unwrap();
        let mut days = BTreeMap::from([
            (
                "20260901".into(),
                (
                    HistoryDay {
                        owed: 10,
                        set_aside: 0,
                        ..HistoryDay::default()
                    },
                    false,
                ),
            ),
            (
                "20260902".into(),
                (
                    HistoryDay {
                        owed: 0,
                        set_aside: 5,
                        ..HistoryDay::default()
                    },
                    false,
                ),
            ),
            (
                "20261009".into(),
                (
                    HistoryDay {
                        owed: 0,
                        set_aside: 0,
                        ..HistoryDay::default()
                    },
                    false,
                ),
            ),
            (
                "20261010".into(),
                (
                    HistoryDay {
                        owed: 9,
                        set_aside: 0,
                        ..HistoryDay::default()
                    },
                    true,
                ),
            ),
            (
                "20261011".into(),
                (
                    HistoryDay {
                        owed: 0,
                        set_aside: 0,
                        ..HistoryDay::default()
                    },
                    false,
                ),
            ),
            (
                "20261012".into(),
                (
                    HistoryDay {
                        owed: 30,
                        set_aside: 0,
                        ..HistoryDay::default()
                    },
                    false,
                ),
            ),
            (
                "20261013".into(),
                (
                    HistoryDay {
                        owed: 40,
                        set_aside: 0,
                        ..HistoryDay::default()
                    },
                    false,
                ),
            ),
        ]);
        let mut report = WorkInventory {
            complete: true,
            ..WorkInventory::default()
        };
        report.bands[3].segments_owed = 100;
        report.bands[4].segments_owed = 20;
        report.bands[5].segments_owed = 200;
        report.bands[4].daily_units_owed = 3;
        report.bands[5].daily_units_owed = 4;
        days.get_mut("20260901").unwrap().0.segments_owed = 20;
        days.get_mut("20260901").unwrap().0.daily_units_owed = 7;
        days.get_mut("20261010").unwrap().0.segments_owed = 100;
        days.get_mut("20261010").unwrap().0.daily_units_owed = 100;
        let progress = history_progress(&report, &days, today, None);
        assert_eq!(progress.caught_up_since.as_deref(), Some("20260902"));
        assert_eq!(progress.through_day, "20261011");
        assert_eq!(progress.set_aside_units, 5);
        assert_eq!(progress.unreadable_days, 1);
        assert_eq!(progress.segments_owed, 20);
        assert_eq!(progress.daily_units_owed, 7);
        assert!(progress.net_per_hour.is_none());
        // A real prerequisite on a readable day moves the range forward.
        days.get_mut("20261009").unwrap().0.owed = 1;
        assert_eq!(
            history_progress(&report, &days, today, None)
                .caught_up_since
                .as_deref(),
            Some("20261010")
        );
        days.get_mut("20261011").unwrap().0.owed = 1;
        assert!(
            history_progress(&report, &days, today, None)
                .caught_up_since
                .is_none()
        );
        // A full pass with stable unreadable exceptions is comparable even
        // though the inventory's stronger complete flag remains false.
        report.complete = false;
        report.unreadable.push("20261010: unreadable".into());
        assert!(
            history_progress(&report, &days, today, None)
                .sample
                .is_some()
        );
        report.days_total = 1;
        assert!(
            history_progress(&report, &days, today, progress.sample.as_ref())
                .sample
                .is_none()
        );
    }

    #[test]
    fn missing_old_daily_outputs_follow_their_recordings_before_older_history() {
        let root = fixture("local");
        let day = "19990102";
        segment(root.path(), day, "120000_60");
        let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 13).unwrap();
        let mut coverage = daily_coverage::read_daily_coverage(root.path(), day).unwrap();
        assert!(coverage.units.iter().any(|unit| unit.state.is_owed()));
        let missing = daily_priority(priority(day, None, today, None), &coverage);
        assert_eq!(missing.band, WorkBand::Completing);
        assert_eq!(missing.source_time, 19990102000000);
        // Re-sensed recordings re-owe the made outputs, which follow them too.
        for unit in &mut coverage.units {
            unit.owed_by = Some("evidence_changed".into());
        }
        assert_eq!(
            daily_priority(priority(day, None, today, None), &coverage),
            missing
        );
        // An entity review re-owed by the days before it waits for band 6.
        for unit in &mut coverage.units {
            unit.identity.name = "entities:entities_review".into();
        }
        let remake = daily_priority(priority(day, None, today, None), &coverage);
        assert_eq!(remake.band, WorkBand::Remake);
        for owed_by in ["contract_changed", "retry", "conflict", "unconfirmed_write"] {
            for unit in &mut coverage.units {
                unit.owed_by = Some(owed_by.into());
            }
            assert_eq!(daily_priority(remake, &coverage), remake, "{owed_by}");
        }
        let index = coverage
            .units
            .iter()
            .position(|unit| unit.state.is_owed())
            .unwrap();
        coverage.units[index].owed_by = Some("never_made".into());
        assert_eq!(daily_priority(remake, &coverage), missing);
        coverage.units[index].state = CoverageState::Current;
        assert_eq!(daily_priority(remake, &coverage), remake);
        coverage.units[index].state = CoverageState::Outstanding;
        for band in [
            WorkBand::Now,
            WorkBand::Waiting,
            WorkBand::Recent,
            WorkBand::Import,
        ] {
            let explicit = priority(day, None, today, Some(band));
            assert_eq!(daily_priority(explicit, &coverage), explicit);
        }
        let mut ranked = Vec::new();
        retain_best(
            &mut ranked,
            vec![
                task(
                    "19990101",
                    Some(("120000_60", "notes")),
                    priority("19990101", Some("120000_60"), today, None),
                    None,
                ),
                task(
                    "19990103",
                    None,
                    priority("19990103", None, today, None),
                    None,
                ),
                task(day, None, missing, None),
                task(
                    day,
                    Some(("120000_60", "notes")),
                    priority(day, Some("120000_60"), today, None),
                    None,
                ),
            ],
        );
        assert_eq!(
            ranked
                .iter()
                .map(|task| task.request.reference.as_str())
                .collect::<Vec<_>>(),
            [
                "supervisor-work-19990102-notes-120000_60",
                "supervisor-work-19990102-daily",
                "supervisor-work-19990101-notes-120000_60",
                "supervisor-work-19990103-daily",
            ]
        );
    }

    #[test]
    fn capped_global_maintenance_is_discovered_on_today_without_recordings() {
        let root = fixture("local");
        let today = daily_coverage::local_day(root.path(), chrono::Utc::now());
        let unit = Unit::Daily {
            day: today.clone(),
            talent: "daily_schedule".into(),
            facet: None,
        };
        let evidence = work_retry::daily::evidence(root.path(), &unit).unwrap();
        let mut record = solstone_core_journal_io::DailyUnitRecord::new(
            work_retry::daily::identity(&unit).unwrap(),
            &evidence.revision,
            &evidence.contract,
        );
        record.status = solstone_core_journal_io::DailyUnitStatus::Capped;
        record.reason_code = Some("schema_invalid".into());
        solstone_core_journal_io::save_daily_unit_record(root.path(), &record).unwrap();
        let queue = queue(root.path());
        reconcile(root.path(), &queue, &AtomicBool::new(false)).unwrap();
        let id = unit.id().unwrap();
        assert!(work_retry::read(root.path(), &id).unwrap().unwrap().held);
        work_retry::request(root.path(), &unit).unwrap();
        reconcile(root.path(), &queue, &AtomicBool::new(false)).unwrap();
        assert!(queue.contains_reference(&format!("supervisor-retry-daily-{today}-{id}")));
        assert!(!queue.contains_reference(&format!("supervisor-work-{today}-daily")));
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn history_excepts_only_permanently_held_input_and_its_blocked_outputs() {
        let root = fixture("local");
        let day = "19990101";
        let directory = root
            .path()
            .join("chronicle")
            .join(day)
            .join("notes/120000_60");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("audio.wav"), "failed source").unwrap();
        std::fs::write(
            directory.join("audio.jsonl"),
            format!(
                "{}\n",
                serde_json::json!({
                    "raw":"audio.wav", "_solstone_processing": {
                        "schema":"solstone.processing.v1", "handler":"transcribe",
                        "state":"failed", "reason_code":"analysis_failed", "attempts":3
                    }
                })
            ),
        )
        .unwrap();
        let queue = queue(root.path());
        let policy = WorkPolicy::read(root.path()).unwrap();
        let mut report = WorkInventory::default();
        let work = inspect_day(
            root.path(),
            day,
            &queue,
            None,
            DaySources {
                imports: &[],
                archive_coordinates: &BTreeSet::new(),
                roots: None,
            },
            policy,
            &mut report,
            &|| Ok(()),
        )
        .unwrap();
        assert_eq!(work.history.owed, 0);
        assert_eq!(work.history.segments_set_aside, 1);
        assert!(work.history.daily_units_set_aside > 0);
        let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 13).unwrap();
        let mut history_day = work.history;
        history_day.segments_owed = report.bands[4].segments_owed;
        history_day.daily_units_owed =
            report.bands[4].daily_units_owed + report.bands[5].daily_units_owed;
        let history = history_progress(
            &report,
            &BTreeMap::from([(day.into(), (history_day, false))]),
            today,
            None,
        );
        assert_eq!(history.segments_owed, 0);
        assert_eq!(history.daily_units_owed, 0);
        assert!(history.set_aside_units > 1);
        // Another, ordinary prerequisite on the same day still pins the range.
        segment(root.path(), day, "120100_60");
        let mut report = WorkInventory::default();
        let work = inspect_day(
            root.path(),
            day,
            &queue,
            None,
            DaySources {
                imports: &[],
                archive_coordinates: &BTreeSet::new(),
                roots: None,
            },
            policy,
            &mut report,
            &|| Ok(()),
        )
        .unwrap();
        assert!(work.history.owed > 1);
        assert_eq!(work.history.segments_set_aside, 1);
        assert_eq!(work.history.daily_units_set_aside, 0);
        // Fresh input permits a retry. It is real work again, not an exception.
        std::fs::write(directory.join("audio.wav"), "changed source for retry").unwrap();
        let mut report = WorkInventory::default();
        let work = inspect_day(
            root.path(),
            day,
            &queue,
            None,
            DaySources {
                imports: &[],
                archive_coordinates: &BTreeSet::new(),
                roots: None,
            },
            policy,
            &mut report,
            &|| Ok(()),
        )
        .unwrap();
        assert_eq!(work.history.segments_set_aside, 0);
        assert_eq!(work.history.set_aside, 0);
        assert!(work.history.owed > 0);
    }

    #[test]
    fn native_readmissions_keep_today_owner_and_background_priority_and_spend() {
        use solstone_core_system::work_policy::ThinkingLane;
        let root = fixture("local");
        let today = daily_coverage::local_day(root.path(), chrono::Utc::now());
        let local = WorkPolicy {
            lane: ThinkingLane::Bundled,
            history_confirmed: false,
            confidential_history_rest: std::time::Duration::ZERO,
        };
        let own_keys = WorkPolicy {
            lane: ThinkingLane::OwnKeys,
            history_confirmed: false,
            confidential_history_rest: std::time::Duration::ZERO,
        };
        let confidential = WorkPolicy {
            lane: ThinkingLane::Confidential,
            history_confirmed: false,
            confidential_history_rest: std::time::Duration::ZERO,
        };
        let mut selected = Vec::new();
        for (day, expected) in [
            (today.as_str(), WorkBand::Now),
            ("19990101", WorkBand::Completing),
        ] {
            let directory = root
                .path()
                .join("chronicle")
                .join(day)
                .join("notes/120000_60");
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::write(directory.join("audio.wav"), "changed source").unwrap();
            std::fs::write(directory.join("audio.jsonl"), format!("{}\n", serde_json::json!({
                "raw":"audio.wav", "_solstone_processing": {
                    "schema":"solstone.processing.v1",
                    "handler":"transcribe", "state":"failed", "reason_code":"analysis_failed", "attempts":3,
                    "input_sha256":"a".repeat(64)
                }
            }))).unwrap();
            let (_, _, segments) = solstone_core_system_health::scan_day(
                &solstone_core_system_health::FilesystemSegmentSource,
                root.path(),
                day,
                chrono::Utc::now(),
            )
            .unwrap();
            assert_eq!(segments.len(), 1);
            let mut report = WorkInventory::default();
            let tasks = raw_retries(
                root.path(),
                day,
                &segments[0],
                None,
                &[],
                local,
                &BTreeSet::new(),
                &mut false,
                &mut report,
                &|| Ok(()),
            )
            .unwrap();
            assert_eq!(tasks.len(), 1);
            assert_eq!(tasks[0].request.priority.unwrap().band, expected);
            selected.extend(tasks);
            if day != today {
                let mut report = WorkInventory::default();
                assert!(
                    raw_retries(
                        root.path(),
                        day,
                        &segments[0],
                        None,
                        &[],
                        own_keys,
                        &BTreeSet::new(),
                        &mut false,
                        &mut report,
                        &|| Ok(())
                    )
                    .unwrap()
                    .is_empty()
                );
                assert_eq!(report.bands[4].permission_held, 1);
                let mut report = WorkInventory::default();
                let tasks = raw_retries(
                    root.path(),
                    day,
                    &segments[0],
                    None,
                    &[],
                    confidential,
                    &BTreeSet::new(),
                    &mut false,
                    &mut report,
                    &|| Ok(()),
                )
                .unwrap();
                assert_eq!(tasks.len(), 1);
                assert_eq!(tasks[0].request.priority.unwrap().band, expected);
                assert_eq!(report.bands[4].permission_held, 0);
                let intent = work_intent::register(root.path(), day, false).unwrap();
                let tasks = raw_retries(
                    root.path(),
                    day,
                    &segments[0],
                    Some(&intent),
                    &[],
                    own_keys,
                    &BTreeSet::new(),
                    &mut false,
                    &mut WorkInventory::default(),
                    &|| Ok(()),
                )
                .unwrap();
                assert_eq!(tasks.len(), 1);
                assert_eq!(tasks[0].request.priority.unwrap().band, WorkBand::Waiting);
                selected.extend(tasks);
            }
        }
        let mut ranked = Vec::new();
        retain_best(&mut ranked, selected);
        assert_eq!(
            ranked
                .iter()
                .map(|task| task.request.priority.unwrap().band)
                .collect::<Vec<_>>(),
            [WorkBand::Now, WorkBand::Waiting, WorkBand::Completing]
        );
    }

    #[test]
    fn discovery_reaches_old_segments_and_never_queues_their_day_first() {
        let root = fixture("local");
        for day in [
            "19990101", "20000101", "20010101", "20020101", "20030101", "20040101",
        ] {
            segment(root.path(), day, "120000_60");
        }
        let queue = queue(root.path());
        reconcile(root.path(), &queue, &AtomicBool::new(false)).unwrap();
        let report = solstone_core_system::work_inventory::read(root.path())
            .unwrap()
            .unwrap();
        assert_eq!(report.days_total, 6);
        assert_eq!(report.days_inspected, 6);
        assert_eq!(report.oldest_day.as_deref(), Some("19990101"));
        assert_eq!(report.bands[4].segments_owed, 6);
        assert_eq!(queue.collect_queue_counts()["pending"], 4);
        assert!(queue.contains_reference("supervisor-work-20040101-notes-120000_60"));
        assert!(!queue.contains_reference("supervisor-work-19990101-notes-120000_60"));
        assert!(!queue.contains_reference("supervisor-work-20040101-daily"));
    }

    #[test]
    fn other_importers_records_and_oversized_capture_rows_leave_the_inventory_complete() {
        let root = fixture("local");
        segment(root.path(), "19990101", "120000_60");
        // A captured page whose first row is longer than any processing header.
        let row = serde_json::json!({"text": "x".repeat(64 * 1024)});
        std::fs::write(
            root.path()
                .join("chronicle/19990101/notes/120000_60/browser_mail-example-com.jsonl"),
            format!("{row}\n"),
        )
        .unwrap();
        let import = root.path().join("imports/19990101_120000");
        std::fs::create_dir_all(&import).unwrap();
        std::fs::write(
            import.join("imported.json"),
            serde_json::json!({
                "processed_timestamp": "19990101_120000",
                "target_day": "19990101",
                "segments": ["120000_60"]
            })
            .to_string(),
        )
        .unwrap();
        reconcile(root.path(), &queue(root.path()), &AtomicBool::new(false)).unwrap();
        let report = solstone_core_system::work_inventory::read(root.path())
            .unwrap()
            .unwrap();
        assert!(report.unreadable.is_empty(), "{:?}", report.unreadable);
        assert_eq!(report.days_inspected, report.days_total);
        assert!(report.complete);
    }

    #[test]
    fn historical_activity_retries_share_the_bound_and_survive_a_malformed_sibling() {
        use sha2::{Digest, Sha256};
        let root = fixture("local");
        let directory = root.path().join("health/activity-work");
        std::fs::create_dir_all(&directory).unwrap();
        for day in [
            "19990101", "20000101", "20010101", "20020101", "20030101", "20040101",
        ] {
            let identity = crate::ActivityRetry {
                day: day.into(),
                facet: "work".into(),
                activity: "reading_090000".into(),
            };
            let path = directory.join(format!(
                "{:x}.json",
                Sha256::digest(serde_json::to_vec(&identity).unwrap())
            ));
            std::fs::write(path, serde_json::json!({"version":1,"identity":identity,"input_hash":"fixture","remaining":["participation"],"uses":{},"attempts":1,"next_attempt_ms":0}).to_string()).unwrap();
        }
        std::fs::write(directory.join("broken.json"), "{").unwrap();
        let queue = queue(root.path());
        for _ in 0..3 {
            reconcile(root.path(), &queue, &AtomicBool::new(false)).unwrap();
            assert_eq!(queue.collect_queue_counts()["pending"], 4);
            let report = solstone_core_system::work_inventory::read(root.path())
                .unwrap()
                .unwrap();
            assert!(!report.complete);
            assert!(
                report
                    .unreadable
                    .iter()
                    .any(|error| error.contains("broken.json"))
            );
            assert_eq!(report.bands[5].activities_owed, 6);
            // Never-made daily outputs now precede these old activity retries.
            assert!(report.bands[4].daily_units_owed > 0);
            assert!(queue.contains_reference("supervisor-work-20040101-daily"));
            assert!(
                !queue.any_reference(
                    |reference| reference.starts_with("supervisor-activity-19990101-")
                )
            );
        }
        work_intent::register(root.path(), "19990101", false).unwrap();
        reconcile(root.path(), &queue, &AtomicBool::new(false)).unwrap();
        assert!(
            queue.any_reference(|reference| reference.starts_with("supervisor-activity-19990101-"))
        );
        let report = solstone_core_system::work_inventory::read(root.path())
            .unwrap()
            .unwrap();
        assert_eq!(report.bands[1].activities_owed, 1);
        assert_eq!(queue.collect_queue_counts()["pending"], 4);
    }

    #[test]
    fn owner_request_replaces_old_waiting_work_and_own_key_history_waits_for_confirmation() {
        let root = fixture("openai");
        segment(root.path(), "19990101", "120000_60");
        let queue = queue(root.path());
        reconcile(root.path(), &queue, &AtomicBool::new(false)).unwrap();
        assert_eq!(
            queue
                .collect_queue_counts()
                .get("pending")
                .copied()
                .unwrap_or(0),
            0
        );
        let report = solstone_core_system::work_inventory::read(root.path())
            .unwrap()
            .unwrap();
        assert_eq!(report.bands[4].permission_held, 1);
        work_intent::register(root.path(), "19990101", false).unwrap();
        reconcile(root.path(), &queue, &AtomicBool::new(false)).unwrap();
        assert!(queue.contains_reference("supervisor-work-19990101-notes-120000_60"));
        let report = solstone_core_system::work_inventory::read(root.path())
            .unwrap()
            .unwrap();
        assert_eq!(report.bands[1].segments_owed, 1);
        assert_eq!(report.bands[4].segments_owed, 0);
    }

    #[test]
    fn claimed_from_scratch_is_visible_and_shutdown_prevents_admission() {
        let root = fixture("local");
        segment(root.path(), "19990101", "120000_60");
        let intent = work_intent::register(root.path(), "19990101", true).unwrap();
        work_intent::claim_from_scratch(root.path(), "19990101", &intent.id).unwrap();
        let queue = queue(root.path());
        reconcile(root.path(), &queue, &AtomicBool::new(false)).unwrap();
        let report = solstone_core_system::work_inventory::read(root.path())
            .unwrap()
            .unwrap();
        assert!(
            report
                .holds
                .iter()
                .any(|hold| hold.reason == "interrupted_from_scratch")
        );
        assert_eq!(
            queue
                .collect_queue_counts()
                .get("pending")
                .copied()
                .unwrap_or(0),
            0
        );
        work_intent::register(root.path(), "19990101", true).unwrap();
        assert_eq!(
            reconcile(root.path(), &queue, &AtomicBool::new(true)).unwrap_err(),
            "work discovery stopped"
        );
        assert_eq!(
            queue
                .collect_queue_counts()
                .get("pending")
                .copied()
                .unwrap_or(0),
            0
        );
    }

    #[test]
    fn import_priority_does_not_authorize_an_unrelated_segment_on_the_same_day() {
        use solstone_core_import::publish::*;
        let root = fixture("openai");
        let day = "19990101";
        segment(root.path(), day, "120000_60");
        segment(root.path(), day, "130000_60");
        let directory = root.path().join("imports/fixture");
        std::fs::create_dir_all(&directory).unwrap();
        let record = PublicationRecord {
            schema: PUBLICATION_SCHEMA.into(),
            status: PublicationStatus::Success,
            segments: vec![SegmentPublicationOutcome {
                day: day.into(),
                stream: "notes".into(),
                segment: "120000_60".into(),
                outcome: SegmentBindingOutcome::Bound {
                    prev_day: None,
                    prev_segment: None,
                    seq: 1,
                },
            }],
            indexing: IndexPublicationOutcomes::default(),
            day_markers: Vec::new(),
        };
        write_publication_record(Some(&directory), &record).unwrap();
        let queue = queue(root.path());
        reconcile(root.path(), &queue, &AtomicBool::new(false)).unwrap();
        assert!(queue.contains_reference("supervisor-work-19990101-notes-120000_60"));
        assert!(!queue.contains_reference("supervisor-work-19990101-notes-130000_60"));
        assert!(!queue.contains_reference("supervisor-work-19990101-daily"));
        let report = solstone_core_system::work_inventory::read(root.path())
            .unwrap()
            .unwrap();
        assert_eq!(report.bands[3].segments_owed, 1);
        assert_eq!(report.bands[4].permission_held, 1);
    }

    /// Record transcribed segments as thought, after their input, the way an
    /// earlier version did: no publication receipt beside their outputs.
    fn thought_without_receipt(root: &Path, day: &str, keys: &[&str]) {
        use solstone_core_journal_io::{
            JournalRoot,
            operational_log::{OplogFormat, create_oplog_at},
        };
        use std::io::Write;
        let mut log = create_oplog_at(
            JournalRoot::open(root).unwrap(),
            "think",
            "segment",
            OplogFormat::Jsonl,
            chrono::NaiveDate::parse_from_str(day, "%Y%m%d")
                .unwrap()
                .and_hms_opt(12, 0, 0)
                .unwrap()
                .and_utc()
                .fixed_offset(),
        )
        .unwrap();
        let sensed = chrono::Utc::now().timestamp_millis() + 3_600_000;
        for key in keys {
            segment(root, day, key);
            std::fs::write(
                root.join("chronicle")
                    .join(day)
                    .join("notes")
                    .join(key)
                    .join("audio.jsonl"),
                "{\"start\":\"00:00:01\",\"text\":\"A meeting about a garden.\"}\n",
            )
            .unwrap();
            let row = serde_json::json!({"event":"sense.complete","ts":sensed,"mode":"segment","stream":"notes","segment":key,"density":"idle"});
            writeln!(log, "{row}").unwrap();
        }
    }

    #[test]
    fn refused_daily_execution_does_not_suppress_existing_segment_publication_recovery() {
        let root = fixture("local");
        let day = "19990101";
        thought_without_receipt(root.path(), day, &["120000_60"]);
        let queue = TaskQueue::with_callable_worker_and_admission(
            TaskQueueOptions {
                #[cfg(windows)]
                read_file_grants: Vec::new(),
                journal_root: root.path().to_path_buf(),
                cap_resolver: Arc::new(solstone_core_system::cap::DefaultCapResolver::default()),
                process_state_probe: Arc::new(NoProcesses),
                queue_sink: None,
                process_sink: None,
                ready: false,
                before_deadline_commit: None,
                child_environment: BTreeMap::new(),
                task_binary: None,
            },
            Arc::new(|_, _| panic!("unready planning control cannot execute")),
            Arc::new(|args| !args.iter().any(|arg| arg == "--daily-only")),
            None,
        );
        reconcile(root.path(), &queue, &AtomicBool::new(false)).unwrap();
        assert!(!queue.contains_reference("supervisor-work-19990101-daily"));
        assert!(
            queue.contains_reference("supervisor-work-19990101-notes-120000_60"),
            "held daily work must leave segment publication independently recoverable"
        );
        let report = solstone_core_system::work_inventory::read(root.path())
            .unwrap()
            .unwrap();
        assert!(
            report.bands.iter().any(|band| band.daily_units_owed > 0),
            "daily remains owed"
        );
    }

    /// Segments thought before their publication was recorded must not hold
    /// their day's close behind one repair each: the daily work that closes
    /// the day is queued in the first pass, and publishes them itself.
    #[test]
    fn a_thought_day_without_publication_receipts_is_closed_in_the_first_pass() {
        let root = fixture("local");
        let day = "19990101";
        let keys = [
            "090000_60",
            "100000_60",
            "110000_60",
            "120000_60",
            "130000_60",
            "140000_60",
        ];
        thought_without_receipt(root.path(), day, &keys);
        // Every segment is sensed and thought on its current input.
        let (_, _, segments) = solstone_core_system_health::scan_day(
            &solstone_core_system_health::FilesystemSegmentSource,
            root.path(),
            day,
            chrono::Utc::now(),
        )
        .unwrap();
        let progress = solstone_core_system_health::read_segment_progress(
            &solstone_core_system_health::FilesystemHealthLogSource::new(root.path()),
            day,
        )
        .unwrap()
        .value;
        assert_eq!(segments.len(), keys.len());
        for segment in &segments {
            let directory = root
                .path()
                .join("chronicle")
                .join(day)
                .join("notes")
                .join(&segment.key);
            assert!(solstone_core_system_health::segment_fully_sensed(
                &segment.data_state
            ));
            assert!(solstone_core_system_health::segment_thinking_is_current(
                solstone_core_system_health::lookup_segment_progress(
                    &progress,
                    "notes",
                    &segment.key
                ),
                solstone_core_system_health::newest_segment_input_ms(&directory).unwrap(),
            ));
        }
        let queue = queue(root.path());
        reconcile(root.path(), &queue, &AtomicBool::new(false)).unwrap();
        assert!(queue.contains_reference("supervisor-work-19990101-daily"));
        let repairs = |reference: &str| reference.starts_with("supervisor-work-19990101-notes-");
        assert!(!queue.any_reference(repairs));
        let report = solstone_core_system::work_inventory::read(root.path())
            .unwrap()
            .unwrap();
        assert_eq!(report.bands[4].segments_owed, keys.len() as u64);
        assert!(
            !report
                .holds
                .iter()
                .any(|hold| hold.reason == "segment_prerequisites")
        );
    }

    /// Owner-requested daily work is early work, and checking whether it is still
    /// owed reads the whole day. A pass must not check it again after every
    /// older day it inspects when nothing about the early work has changed.
    #[test]
    fn early_work_is_not_checked_again_after_every_older_day() {
        let root = fixture("local");
        // Stay clear of yesterday's intentional hold just after UTC midnight,
        // while retaining band-2 early admission through the owner request.
        let recent_day = (chrono::Utc::now().date_naive() - chrono::Duration::days(2))
            .format("%Y%m%d")
            .to_string();
        thought_without_receipt(root.path(), &recent_day, &["090000_60", "100000_60"]);
        work_intent::register(root.path(), &recent_day, false).unwrap();
        let older = 24;
        for n in 0..older {
            segment(
                root.path(),
                &format!("1999{:02}{:02}", 1 + n / 28, 1 + n % 28),
                "090000_60",
            );
        }
        let queue = queue(root.path());
        STILL_OWED_CHECKS.with(|checks| checks.set(0));
        reconcile(root.path(), &queue, &AtomicBool::new(false)).unwrap();
        let checks = STILL_OWED_CHECKS.with(|checks| checks.get());
        assert!(queue.contains_reference(&format!("supervisor-work-{recent_day}-daily")));
        // One early admission after the recent day, then the final admission of the
        // ranking: a few checks, not one more per older day.
        assert!(
            checks < older,
            "{checks} still-owed checks for {older} older days"
        );
    }

    #[test]
    fn segment_outputs_from_an_earlier_version_are_repaired_not_held() {
        let root = fixture("local");
        let day = "19990101";
        segment(root.path(), day, "120000_60");
        let talents = root
            .path()
            .join("chronicle")
            .join(day)
            .join("notes/120000_60/talents");
        std::fs::create_dir_all(&talents).unwrap();
        // Outputs with no digest file, and a digest file that disagrees with them.
        std::fs::write(talents.join("activity.md"), "an earlier summary").unwrap();
        std::fs::write(
            talents.join("sense.json"),
            r#"{"density":"active","content_type":"work"}"#,
        )
        .unwrap();
        std::fs::write(
            talents.join(".generated-outputs.json"),
            format!(
                r#"{{"version":1,"files":{{"sense.json":"{}"}}}}"#,
                "0".repeat(64)
            ),
        )
        .unwrap();
        let queue = queue(root.path());
        reconcile(root.path(), &queue, &AtomicBool::new(false)).unwrap();
        assert!(queue.contains_reference("supervisor-work-19990101-notes-120000_60"));
        let report = solstone_core_system::work_inventory::read(root.path())
            .unwrap()
            .unwrap();
        assert_eq!(report.bands[4].segments_owed, 1);
        assert_eq!(report.bands[4].eligible, 1);
        assert!(
            !report
                .holds
                .iter()
                .any(|hold| hold.segment.as_deref() == Some("120000_60")),
            "{:?}",
            report.holds
        );
    }

    #[test]
    fn only_a_requested_day_with_work_in_the_queue_is_owner_work() {
        let root = fixture("local");
        segment(root.path(), "19990101", "120000_60");
        segment(root.path(), "19990102", "120000_60");
        work_intent::register(root.path(), "19990101", false).unwrap();
        let queue = queue(root.path());
        reconcile(root.path(), &queue, &AtomicBool::new(false)).unwrap();
        // Both days have queued work; only the requested one is waited on.
        assert!(queue.contains_reference("supervisor-work-19990102-notes-120000_60"));
        assert_eq!(
            owner_days_in_queue(root.path(), &queue),
            BTreeSet::from(["19990101".to_owned()])
        );
        queue.replace_discovered(Vec::new());
        assert!(owner_days_in_queue(root.path(), &queue).is_empty());
    }

    /// A requested day's next step is found only by a pass, so the end of its
    /// queued work starts one instead of waiting out the automatic rest.
    #[cfg(all(unix, feature = "full-tests"))]
    #[test]
    fn finished_owner_work_starts_the_next_pass_before_the_rest_ends() {
        use solstone_core_system::lifecycle::{
            CoordinatorBootstrap, DeclaredParent, ParentLossCoordinator,
        };
        use solstone_core_system::queue::{SubmitOutcome, SystemProcessStateProbe};
        use solstone_core_system::request::ExecutionRequest;
        let root = fixture("local");
        let parent = DeclaredParent::capture_current().unwrap();
        let (_coordinator, _) = ParentLossCoordinator::bootstrap(CoordinatorBootstrap {
            journal: root.path().into(),
            supervisor: parent.instance(),
            enabled: Vec::new(),
            supervisor_heartbeat_filename: format!(
                "solstone-v2-owner-wake-test-{}.check",
                std::process::id()
            ),
            capability: format!("owner-wake-test-{}", root.path().display()).into_bytes(),
        })
        .unwrap();
        let queue = TaskQueue::new(TaskQueueOptions {
            journal_root: root.path().into(),
            cap_resolver: Arc::new(solstone_core_system::cap::DefaultCapResolver::default()),
            process_state_probe: Arc::new(SystemProcessStateProbe),
            queue_sink: None,
            process_sink: None,
            ready: true,
            before_deadline_commit: None,
            child_environment: BTreeMap::new(),
            task_binary: None,
        });
        let (passes, passed) = mpsc::channel();
        let mut count = 0;
        let worker = WorkDiscovery::with_reconciler(
            root.path().into(),
            queue.clone(),
            true,
            move |journal, queue, _| {
                count += 1;
                if count == 1 {
                    // The owner's request admits the day's segment work.
                    work_intent::register(journal, "19990101", false)?;
                    let mut work = task(
                        "19990101",
                        Some(("120000_60", "notes")),
                        WorkPriority {
                            band: WorkBand::Waiting,
                            source_time: 19990101120000,
                        },
                        None,
                    );
                    work.request.cmd =
                        TaskArgv::from_wire(vec!["/bin/sleep".into(), "0.5".into()]).unwrap();
                    assert_eq!(
                        queue.submit(ExecutionRequest::Bus(work.request)),
                        SubmitOutcome::Dispatched
                    );
                }
                passes.send(count).unwrap();
                Ok(Backlog::default())
            },
        );
        worker.request();
        assert_eq!(passed.recv_timeout(Duration::from_secs(5)).unwrap(), 1);
        // The automatic rest is at least a minute; the owner's work ends sooner.
        assert_eq!(passed.recv_timeout(Duration::from_secs(15)).unwrap(), 2);
        assert!(!owner_work_in_queue(&queue, "19990101"));
        // With no owner work left, the next pass waits for the rest.
        assert!(passed.recv_timeout(Duration::from_secs(3)).is_err());
        worker.stop();
        queue.shutdown();
    }

    /// The queue admits a few discovered tasks at a time. As they finish, the
    /// pass's next ranked work is admitted in their place, newest first, well
    /// before the automatic rest would start another pass.
    #[cfg(all(unix, feature = "full-tests"))]
    #[test]
    fn ranked_work_past_the_admission_bound_is_admitted_as_the_queue_drains() {
        use solstone_core_system::lifecycle::{
            CoordinatorBootstrap, DeclaredParent, ParentLossCoordinator,
        };
        use solstone_core_system::queue::SystemProcessStateProbe;
        let root = fixture("local");
        let keys = (10..20)
            .map(|hour| format!("{hour}0000_60"))
            .collect::<Vec<_>>();
        for key in &keys {
            segment(root.path(), "19990101", key);
        }
        let parent = DeclaredParent::capture_current().unwrap();
        let (_coordinator, _) = ParentLossCoordinator::bootstrap(CoordinatorBootstrap {
            journal: root.path().into(),
            supervisor: parent.instance(),
            enabled: Vec::new(),
            supervisor_heartbeat_filename: format!(
                "solstone-v2-drain-test-{}.check",
                std::process::id()
            ),
            capability: format!("drain-test-{}", root.path().display()).into_bytes(),
        })
        .unwrap();
        // Each task finishes at once and leaves its segment owed.
        let queue = TaskQueue::new(TaskQueueOptions {
            journal_root: root.path().into(),
            cap_resolver: Arc::new(solstone_core_system::cap::DefaultCapResolver::default()),
            process_state_probe: Arc::new(SystemProcessStateProbe),
            queue_sink: None,
            process_sink: None,
            ready: true,
            before_deadline_commit: None,
            child_environment: BTreeMap::new(),
            task_binary: Some("/bin/true".into()),
        });
        let worker = WorkDiscovery::new(root.path().into(), queue.clone(), true);
        worker.request();
        // The automatic rest is at least a minute.
        let deadline = Instant::now() + Duration::from_secs(30);
        let finished = loop {
            let finished = queue
                .history()
                .into_iter()
                .filter(|record| {
                    record
                        .reference
                        .starts_with("supervisor-work-19990101-notes-")
                })
                .map(|record| record.reference)
                .collect::<Vec<_>>();
            if finished.len() >= keys.len() || Instant::now() >= deadline {
                break finished;
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        worker.stop();
        queue.shutdown();
        let expected = keys
            .iter()
            .rev()
            .map(|key| format!("supervisor-work-19990101-notes-{key}"))
            .collect::<Vec<_>>();
        assert_eq!(finished, expected);
    }

    #[cfg(all(unix, feature = "full-tests"))]
    #[test]
    fn blocked_discovery_coalesces_wakes_and_does_not_hold_task_admission_or_status() {
        use solstone_core_system::request::ExecutionRequest;
        let root = fixture("local");
        use solstone_core_system::lifecycle::{
            CoordinatorBootstrap, DeclaredParent, ParentLossCoordinator,
        };
        let parent = DeclaredParent::capture_current().unwrap();
        let (_coordinator, _) = ParentLossCoordinator::bootstrap(CoordinatorBootstrap {
            journal: root.path().into(),
            supervisor: parent.instance(),
            enabled: Vec::new(),
            supervisor_heartbeat_filename: format!(
                "solstone-v2-discovery-test-{}.check",
                std::process::id()
            ),
            capability: format!("discovery-test-{}", root.path().display()).into_bytes(),
        })
        .unwrap();
        use solstone_core_system::queue::{
            SystemProcessStateProbe, TaskQueueEvent, TaskQueueEventSink,
        };
        struct Events(mpsc::Sender<TaskQueueEvent>);
        impl TaskQueueEventSink for Events {
            fn emit(&self, event: TaskQueueEvent) {
                let _ = self.0.send(event);
            }
        }
        let (events, observed) = mpsc::channel();
        let queue = TaskQueue::new(TaskQueueOptions {
            journal_root: root.path().into(),
            cap_resolver: Arc::new(solstone_core_system::cap::DefaultCapResolver::default()),
            process_state_probe: Arc::new(SystemProcessStateProbe),
            queue_sink: Some(Arc::new(Events(events))),
            process_sink: None,
            ready: true,
            before_deadline_commit: None,
            child_environment: BTreeMap::new(),
            task_binary: None,
        });
        let (entered, received) = mpsc::channel();
        let (release, blocked) = mpsc::channel();
        let worker = WorkDiscovery::with_reconciler(
            root.path().into(),
            queue.clone(),
            false,
            move |_, _, _| {
                entered.send(()).unwrap();
                blocked.recv().unwrap();
                Ok(Backlog::default())
            },
        );
        worker.request();
        received.recv_timeout(Duration::from_secs(5)).unwrap();
        for _ in 0..100 {
            worker.request();
        }
        let mut live = task(
            "20261011",
            Some(("120000_60", "desktop")),
            WorkPriority {
                band: WorkBand::Now,
                source_time: 20261011120000,
            },
            None,
        );
        // A real bounded child reaches the worker while the discovery read is
        // blocked. The separate supervisor race harness covers capture routing.
        live.request.cmd = TaskArgv::from_wire(vec!["/bin/sleep".into(), "0.1".into()]).unwrap();
        let reference = live.request.reference.clone();
        assert_eq!(
            queue.submit(ExecutionRequest::Bus(live.request)),
            solstone_core_system::queue::SubmitOutcome::Dispatched,
            "{:?}",
            queue.collect_status_snapshot(std::time::Instant::now())
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            match observed
                .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
                .unwrap()
            {
                TaskQueueEvent::Started {
                    reference: started, ..
                } if started == reference => break,
                _ => {}
            }
        }
        let _status = queue.collect_status_snapshot(std::time::Instant::now());
        assert!(std::time::Instant::now() < deadline);
        release.send(()).unwrap();
        received.recv_timeout(Duration::from_secs(5)).unwrap();
        worker.stop();
        release.send(()).unwrap();
        assert!(received.recv_timeout(Duration::from_secs(5)).is_err());
        queue.shutdown();
    }

    #[test]
    fn reconcile_pass_persists_the_cutoff_before_inspect_day() {
        let root = fixture("local");
        let journal = root.path();
        let zone = solstone_core_journal_config::owner_zone(journal);
        let now = chrono::Utc::now();
        let today =
            chrono::NaiveDate::parse_from_str(&daily_coverage::local_day(journal, now), "%Y%m%d")
                .unwrap();
        let mut days = Vec::new();
        for age in 1..=12 {
            let day = (today - chrono::Duration::days(age))
                .format("%Y%m%d")
                .to_string();
            std::fs::create_dir_all(journal.join("chronicle").join(&day)).unwrap();
            let coverage = daily_coverage::read_daily_coverage(journal, &day).unwrap();
            for unit in &coverage.units {
                if unit.evidence_revision.is_empty() || unit.contract_digest.is_empty() {
                    continue;
                }
                let mut record = solstone_core_journal_io::DailyUnitRecord::new(
                    unit.identity.clone(),
                    &unit.evidence_revision,
                    &unit.contract_digest,
                );
                record.status = solstone_core_journal_io::DailyUnitStatus::CommittedNoOutput;
                record.accepted = Some(solstone_core_journal_io::AcceptedDailyResult {
                    evidence_revision: unit.evidence_revision.clone(),
                    contract_digest: unit.contract_digest.clone(),
                    status: solstone_core_journal_io::DailyUnitStatus::CommittedNoOutput,
                    packet_digest: Some("a".repeat(64)),
                    generated_result: Some(serde_json::json!({"response":"[]","output":"[]"})),
                    receipts: Vec::new(),
                    committed_at_ms: 1,
                });
                solstone_core_journal_io::save_daily_unit_record(journal, &record).unwrap();
            }
            days.push(day);
        }
        let before = chrono::Utc::now();
        let backlog =
            reconcile_pass(journal, &queue(journal), &AtomicBool::new(false), false).unwrap();
        let after = chrono::Utc::now();
        let early = daily_coverage::daily_input_since_day(before, &zone, 7);
        let late = daily_coverage::daily_input_since_day(after, &zone, 7);
        let config = solstone_core_journal_config::read_journal_config(journal)
            .unwrap()
            .config
            .unwrap();
        let stored = solstone_core_journal_config::daily_input_since(
            &config,
            "location",
            "location-words-1",
        )
        .unwrap()
        .unwrap()
        .format("%Y%m%d")
        .to_string();
        assert!(stored == early || stored == late, "{stored} {early} {late}");
        let mut owed = BTreeSet::new();
        for task in backlog.admitted.iter().chain(backlog.ranked.iter()) {
            let wire = task.request.cmd.as_wire();
            if wire.iter().any(|arg| arg == "--daily-only")
                && let Some(day) = task.request.day.clone()
            {
                owed.insert(day);
            }
        }
        if let Some(report) = solstone_core_system::work_inventory::read(journal).unwrap() {
            for hold in report.holds {
                if hold.segment.is_none() && hold.retry_unit.is_none() {
                    owed.insert(hold.day);
                }
            }
        }
        let earlier = early.clone().min(late.clone());
        let later = early.max(late);
        for day in &owed {
            assert!(
                day.as_str() >= earlier.as_str(),
                "{day} is before {earlier}: {owed:?}"
            );
        }
        for day in &days {
            if day.as_str() >= later.as_str() {
                assert!(owed.contains(day), "{day} missing from {owed:?}");
            }
            if day.as_str() < earlier.as_str() {
                assert!(!owed.contains(day), "{day} present in {owed:?}");
            }
        }
    }

    /// One old segment, sensed, whose `talent` then failed `failures` times an
    /// hour apart with `reason`. Returns the segment directory.
    fn failing_talent(
        root: &Path,
        talent: &str,
        failures: i64,
        reason: &str,
        sensed: bool,
    ) -> PathBuf {
        use solstone_core_journal_io::{
            JournalRoot,
            operational_log::{OplogFormat, create_oplog_at},
        };
        use std::io::Write;
        let (day, key) = ("19990101", "120000_60");
        segment(root, day, key);
        let directory = root.join("chronicle").join(day).join("notes").join(key);
        std::fs::write(
            directory.join("audio.jsonl"),
            "{\"start\":\"00:00:01\",\"text\":\"A meeting about a garden.\"}\n",
        )
        .unwrap();
        std::fs::create_dir_all(directory.join("talents")).unwrap();
        std::fs::write(
            directory.join("talents/sense.json"),
            "{\"density\":\"active\"}",
        )
        .unwrap();
        let mut log = create_oplog_at(
            JournalRoot::open(root).unwrap(),
            "think",
            "segment",
            OplogFormat::Jsonl,
            chrono::NaiveDate::parse_from_str(day, "%Y%m%d")
                .unwrap()
                .and_hms_opt(12, 0, 0)
                .unwrap()
                .and_utc()
                .fixed_offset(),
        )
        .unwrap();
        let after_input = chrono::Utc::now().timestamp_millis() + 3_600_000;
        if sensed {
            writeln!(log, "{}", serde_json::json!({"event":"sense.complete","ts":after_input,"mode":"segment","stream":"notes","segment":key,"density":"active"})).unwrap();
            writeln!(log, "{}", serde_json::json!({"event":"talent.dispatch","ts":after_input,"mode":"segment","stream":"notes","segment":key,"name":"documents","use_id":"use-documents"})).unwrap();
            writeln!(log, "{}", serde_json::json!({"event":"talent.complete","ts":after_input,"mode":"segment","stream":"notes","segment":key,"name":"documents","use_id":"use-documents"})).unwrap();
        }
        for attempt in 0..failures {
            let ts = after_input + 1 + attempt * 3_600_000;
            let use_id = format!("use-{attempt}");
            writeln!(log, "{}", serde_json::json!({"event":"talent.dispatch","ts":ts,"mode":"segment","stream":"notes","segment":key,"name":talent,"use_id":use_id})).unwrap();
            writeln!(log, "{}", serde_json::json!({"event":"talent.fail","ts":ts + 1,"mode":"segment","stream":"notes","segment":key,"name":talent,"use_id":use_id,"reason_code":reason})).unwrap();
        }
        directory
    }

    fn held_unit(talent: &str) -> Unit {
        Unit::Segment {
            day: "19990101".into(),
            stream: "notes".into(),
            segment: "120000_60".into(),
            talent: talent.into(),
            direct: false,
        }
    }

    /// A talent that fails on every attempt is set aside rather than repaired
    /// on every pass, its day's outputs wait with it, and it comes back for
    /// changed input or an owner's ask, one retry per cause.
    #[test]
    fn a_talent_that_keeps_failing_is_set_aside_until_a_cause_to_retry() {
        let root = fixture("local");
        let directory =
            failing_talent(root.path(), "screen", 30, "provider_request_rejected", true);
        let queue = queue(root.path());
        let repair = "supervisor-work-19990101-notes-120000_60";
        let unit = held_unit("screen");
        let id = unit.id().unwrap();
        let retry = format!("supervisor-retry-talent-19990101-{id}");
        for _ in 0..2 {
            reconcile(root.path(), &queue, &AtomicBool::new(false)).unwrap();
            assert!(!queue.contains_reference(repair));
            assert!(!queue.contains_reference(&retry));
            assert!(!queue.contains_reference("supervisor-work-19990101-daily"));
            let record = work_retry::read(root.path(), &id).unwrap().unwrap();
            assert!(record.held);
            assert_eq!(record.reason, "provider_request_rejected");
            let report = solstone_core_system::work_inventory::read(root.path())
                .unwrap()
                .unwrap();
            assert!(
                report
                    .holds
                    .iter()
                    .any(|hold| hold.retry_unit.as_deref() == Some(&id))
            );
            assert_eq!(
                report
                    .bands
                    .iter()
                    .map(|band| band.segments_owed)
                    .sum::<u64>(),
                0
            );
        }
        let policy = WorkPolicy::read(root.path()).unwrap();
        let mut report = WorkInventory::default();
        let work = inspect_day(
            root.path(),
            "19990101",
            &queue,
            None,
            DaySources {
                imports: &[],
                archive_coordinates: &BTreeSet::new(),
                roots: None,
            },
            policy,
            &mut report,
            &|| Ok(()),
        )
        .unwrap();
        assert_eq!(work.history.owed, 0);
        assert_eq!(work.history.segments_set_aside, 1);
        assert!(work.history.daily_units_set_aside > 0);

        // Its input changes: one retry, still no repeat of the whole segment.
        std::fs::write(
            directory.join("talents/sense.json"),
            "{\"density\":\"busy\"}",
        )
        .unwrap();
        reconcile(root.path(), &queue, &AtomicBool::new(false)).unwrap();
        assert!(queue.contains_reference(&retry));
        assert!(!queue.contains_reference(repair));

        // That retry spent its cause; it failed again, so it waits again.
        let input = work_retry::input::current(root.path(), &unit).unwrap();
        let attempt = work_retry::read(root.path(), &id)
            .unwrap()
            .unwrap()
            .permission(&input, work_retry::DECLARED_FIXES)
            .unwrap();
        work_retry::consume(root.path(), &unit, &attempt, &attempt.reason, &input, &[]).unwrap();
        work_retry::finish(
            root.path(),
            &unit,
            &attempt,
            Some("provider_request_rejected"),
        )
        .unwrap();
        reconcile(root.path(), &queue, &AtomicBool::new(false)).unwrap();
        assert!(!queue.contains_reference(&retry));
        assert!(!queue.contains_reference(repair));

        // The owner asks again from Health: one more retry.
        work_retry::request(root.path(), &unit).unwrap();
        reconcile(root.path(), &queue, &AtomicBool::new(false)).unwrap();
        assert!(queue.contains_reference(&retry));
        assert!(!queue.contains_reference(repair));
    }

    /// Sense that never succeeds is held against the recording itself.
    #[test]
    fn a_sense_that_keeps_failing_is_held_against_the_recording() {
        let root = fixture("local");
        let directory = failing_talent(root.path(), "sense", 8, "context_window_exceeded", false);
        std::fs::remove_dir_all(directory.join("talents")).unwrap();
        let queue = queue(root.path());
        let unit = held_unit("sense");
        let id = unit.id().unwrap();
        reconcile(root.path(), &queue, &AtomicBool::new(false)).unwrap();
        assert!(work_retry::read(root.path(), &id).unwrap().unwrap().held);
        assert!(!queue.contains_reference("supervisor-work-19990101-notes-120000_60"));
        std::fs::write(
            directory.join("audio.jsonl"),
            "{\"start\":\"00:00:01\",\"text\":\"A longer meeting about the garden.\"}\n",
        )
        .unwrap();
        reconcile(root.path(), &queue, &AtomicBool::new(false)).unwrap();
        assert!(queue.contains_reference(&format!("supervisor-retry-talent-19990101-{id}")));
    }

    /// An outage or a lost connection says nothing about the recording: its
    /// work keeps its turn.
    #[test]
    fn failures_while_the_lane_is_down_never_set_work_aside() {
        for reason in [
            "local_endpoint_unreachable",
            "network_unreachable",
            "wait failed: Dispatch(Closed)",
        ] {
            let root = fixture("local");
            failing_talent(root.path(), "screen", 30, reason, true);
            let queue = queue(root.path());
            reconcile(root.path(), &queue, &AtomicBool::new(false)).unwrap();
            assert!(queue.contains_reference("supervisor-work-19990101-notes-120000_60"));
            assert!(
                work_retry::read(root.path(), &held_unit("screen").id().unwrap())
                    .unwrap()
                    .is_none()
            );
        }
    }

    /// A few recent failures are ordinary work, not yet a pattern.
    #[test]
    fn failures_short_of_the_cap_keep_repairing() {
        let root = fixture("local");
        failing_talent(root.path(), "screen", 4, "provider_request_rejected", true);
        let queue = queue(root.path());
        reconcile(root.path(), &queue, &AtomicBool::new(false)).unwrap();
        assert!(queue.contains_reference("supervisor-work-19990101-notes-120000_60"));
    }

    /// The history walk re-senses an older day whose outputs were already made.
    /// Its outputs are re-made in band 5, right after its recordings, and hold
    /// the range until then. Its entity review reads the days before it, which
    /// the walk reaches later: that residual waits for band 6, so the walk
    /// passing those days never re-makes it again, and the range follows the
    /// walk. A pass in progress keeps showing the last finished line.
    #[test]
    fn a_resensed_old_day_is_remade_with_its_recordings_and_its_review_waits() {
        let root = fixture("local");
        let journal = root.path();
        solstone_core_facets::create_facet(journal, "work", "Work", "", "", "", None).unwrap();
        // The walk is on 19990101, an older day still owing a recording.
        segment(journal, "19990101", "120000_60");
        let walked = "19990110";
        thought_without_receipt(journal, walked, &["120000_60"]);
        let recording = journal
            .join("chronicle")
            .join(walked)
            .join("notes/120000_60");
        std::fs::create_dir_all(recording.join("talents")).unwrap();
        std::fs::write(
            recording.join("talents/facets.json"),
            r#"[{"facet":"work"}]"#,
        )
        .unwrap();
        let save = |unit: &daily_coverage::UnitCoverage, evidence: &str| {
            let mut record = solstone_core_journal_io::DailyUnitRecord::new(
                unit.identity.clone(),
                evidence,
                &unit.contract_digest,
            );
            record.status = solstone_core_journal_io::DailyUnitStatus::CommittedNoOutput;
            record.accepted = Some(solstone_core_journal_io::AcceptedDailyResult {
                evidence_revision: evidence.into(),
                contract_digest: unit.contract_digest.clone(),
                status: solstone_core_journal_io::DailyUnitStatus::CommittedNoOutput,
                packet_digest: Some("a".repeat(64)),
                generated_result: Some(serde_json::json!({"response":"[]","output":"[]"})),
                receipts: Vec::new(),
                committed_at_ms: 1,
            });
            solstone_core_journal_io::save_daily_unit_record(journal, &record).unwrap();
        };
        let review =
            |unit: &daily_coverage::UnitCoverage| unit.identity.name == "entities:entities_review";
        let coverage = daily_coverage::read_daily_coverage(journal, walked).unwrap();
        let made: Vec<_> = coverage
            .units
            .iter()
            .filter(|unit| !unit.evidence_revision.is_empty() && !unit.contract_digest.is_empty())
            .cloned()
            .collect();
        assert!(made.iter().any(review));
        assert!(made.iter().any(|unit| !review(unit)));
        // Made from the earlier sensing: every made output's evidence changed.
        for unit in &made {
            save(unit, &"0".repeat(64));
        }
        // The walk's own day owes its never-made outputs in band 5.
        let walk_owed = daily_coverage::read_daily_coverage(journal, "19990101")
            .unwrap()
            .units
            .iter()
            .filter(|unit| unit.state.is_owed())
            .count() as u64;
        let queue = queue(journal);
        reconcile(journal, &queue, &AtomicBool::new(false)).unwrap();
        let inventory = solstone_core_system::work_inventory::read(journal)
            .unwrap()
            .unwrap();
        assert!(inventory.bands[4].daily_units_owed > walk_owed);
        assert_eq!(inventory.bands[5].daily_units_owed, 0);
        assert!(queue.contains_reference("supervisor-work-19990110-daily"));
        let history = inventory.history.clone().unwrap();
        assert_eq!(history.caught_up_since.as_deref(), Some("19990111"));
        // Its run re-makes the set, then the walk re-senses the days before it.
        queue.shutdown();
        let queue = super::tests::queue(journal);
        for unit in &made {
            save(unit, &unit.evidence_revision);
        }
        let residual = |coverage: &daily_coverage::DailyCoverage| {
            coverage
                .units
                .iter()
                .filter(|unit| unit.state.is_owed())
                .map(|unit| (unit.identity.name.clone(), unit.owed_by.clone()))
                .collect::<Vec<_>>()
        };
        let mut published = false;
        for older in ["19990109", "19990108", "19990103"] {
            solstone_core_facets::save_detected_entity(
                journal,
                "work",
                older,
                "Person",
                &format!("Wren {older}"),
                "Joined the launch call.",
            )
            .unwrap();
            let coverage = daily_coverage::read_daily_coverage(journal, walked).unwrap();
            assert_eq!(
                residual(&coverage),
                [(
                    "entities:entities_review".to_owned(),
                    Some("evidence_changed".to_owned())
                )]
            );
            assert_eq!(
                daily_priority(
                    priority(walked, None, chrono::Utc::now().date_naive(), None),
                    &coverage
                )
                .band,
                WorkBand::Remake
            );
            reconcile(journal, &queue, &AtomicBool::new(false)).unwrap();
            let inventory = solstone_core_system::work_inventory::read(journal)
                .unwrap()
                .unwrap();
            assert_eq!(inventory.bands[4].daily_units_owed, walk_owed);
            assert_eq!(inventory.bands[5].daily_units_owed, 1);
            let history = inventory.history.clone().unwrap();
            if !published {
                // Owed only its publication, the recording does not wait for
                // the band-6 daily: it is published on its own, in band 5.
                assert!(queue.contains_reference("supervisor-work-19990110-notes-120000_60"));
                assert_eq!(history.caught_up_since.as_deref(), Some("19990111"));
                let input =
                    solstone_core_system_health::newest_segment_input_ms(&recording).unwrap();
                solstone_core_system::segment_publication::publish(
                    journal,
                    &recording,
                    input,
                    |_, _| {
                        Ok(
                            solstone_core_indexer_store::scan::RescanFileStatus::Indexed {
                                warnings: Vec::new(),
                            },
                        )
                    },
                )
                .unwrap();
                published = true;
                reconcile(journal, &queue, &AtomicBool::new(false)).unwrap();
            }
            let inventory = solstone_core_system::work_inventory::read(journal)
                .unwrap()
                .unwrap();
            let history = inventory.history.clone().unwrap();
            assert_eq!(history.caught_up_since.as_deref(), Some("19990102"));
            assert_eq!(history.segments_owed, 1);
        }
        let inventory = solstone_core_system::work_inventory::read(journal)
            .unwrap()
            .unwrap();
        let history = inventory.history.clone().unwrap();
        // Stop leaves the last durable snapshot intact; freshness is a read.
        assert_eq!(
            reconcile(journal, &queue, &AtomicBool::new(true)).unwrap_err(),
            "work discovery stopped"
        );
        let progress = solstone_core_system::work_inventory::read(journal)
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::to_value(&progress).unwrap(),
            serde_json::to_value(&inventory).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&progress.history).unwrap(),
            serde_json::to_value(Some(history)).unwrap()
        );
        let mut old = progress;
        old.as_of_ms = 1;
        old.next_pass_ms = None;
        old.publish(journal).unwrap();
        assert_eq!(
            reconcile(journal, &queue, &AtomicBool::new(true)).unwrap_err(),
            "work discovery stopped"
        );
        let progress = solstone_core_system::work_inventory::read(journal)
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::to_value(&progress).unwrap(),
            serde_json::to_value(&old).unwrap()
        );
        assert!(!progress.current_at(chrono::Utc::now().timestamp_millis()));
        // A daily output never made on that newer day still holds the range.
        let unmade = made.iter().find(|unit| !review(unit)).unwrap();
        solstone_core_journal_io::save_daily_unit_record(
            journal,
            &solstone_core_journal_io::DailyUnitRecord::new(
                unmade.identity.clone(),
                &unmade.evidence_revision,
                &unmade.contract_digest,
            ),
        )
        .unwrap();
        reconcile(journal, &queue, &AtomicBool::new(false)).unwrap();
        let history = solstone_core_system::work_inventory::read(journal)
            .unwrap()
            .unwrap()
            .history
            .unwrap();
        assert_eq!(history.caught_up_since.as_deref(), Some("19990111"));
        queue.shutdown();
    }

    /// Top-ups move the next pass back. Readers hear the new time when the
    /// promised one arrives, once; with nothing moved, they hear nothing.
    #[cfg(feature = "full-tests")]
    #[test]
    fn a_resting_worker_tells_readers_when_top_ups_move_the_next_pass_back() {
        let (_wake, receiver) = mpsc::sync_channel(1);
        let promised = Instant::now() + Duration::from_millis(200);
        let mut told = Vec::new();
        let mut tell = |due: Instant| told.push(due);
        let mut calls = 0;
        let mut top_up = || {
            calls += 1;
            if calls == 1 {
                std::thread::sleep(Duration::from_millis(150));
            }
            true
        };
        assert!(rest(
            &receiver,
            Some(promised),
            None,
            Some(&mut top_up as &mut dyn FnMut() -> bool),
            Some((promised, &mut tell as &mut dyn FnMut(Instant))),
        ));
        let returned = Instant::now();
        assert_eq!(told.len(), 1);
        assert!(told[0] >= promised + Duration::from_millis(1_400));
        assert!(returned >= told[0]);
        let mut told = Vec::new();
        let mut tell = |due: Instant| told.push(due);
        let due = Instant::now() + Duration::from_millis(200);
        assert!(rest(
            &receiver,
            Some(due),
            None,
            None,
            Some((due, &mut tell as &mut dyn FnMut(Instant))),
        ));
        assert!(told.is_empty());
    }

    #[test]
    fn a_later_next_pass_is_published_without_refreshing_the_snapshot() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("health")).unwrap();
        let mut report = WorkInventory {
            as_of_ms: 1_000,
            next_pass_ms: Some(2_000),
            ..WorkInventory::default()
        };
        report.publish(root.path()).unwrap();
        let before = chrono::Utc::now().timestamp_millis();
        repromise(
            root.path(),
            &mut report,
            Instant::now() + Duration::from_secs(600),
            &|| Ok(()),
        );
        let after = chrono::Utc::now().timestamp_millis();
        let read = solstone_core_system::work_inventory::read(root.path())
            .unwrap()
            .unwrap();
        assert_eq!(read.as_of_ms, 1_000);
        let due = read.next_pass_ms.unwrap();
        assert!(due >= before + 599_000 && due <= after + 600_000, "{due}");
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn inspect_day_enqueues_archive_segment_by_coordinate_and_sets_work_band() {
        let root = fixture("local");
        let now = chrono::Utc::now();
        let today = daily_coverage::local_day(root.path(), now);
        let archive_key = "120000_60";
        let notes_key = "120100_60";

        // 1. Today: import.archive/<key> and notes/<key>
        let archive_dir = root
            .path()
            .join("chronicle")
            .join(&today)
            .join("import.archive")
            .join(archive_key);
        std::fs::create_dir_all(&archive_dir).unwrap();
        std::fs::write(archive_dir.join("audio.opus"), "audio").unwrap();
        std::fs::write(
            archive_dir.join("audio.jsonl"),
            "{}\n{\"start\":\"12:00:00\",\"text\":\"speech\"}\n",
        )
        .unwrap();

        let notes_dir = root
            .path()
            .join("chronicle")
            .join(&today)
            .join("notes")
            .join(notes_key);
        std::fs::create_dir_all(&notes_dir).unwrap();
        std::fs::write(notes_dir.join("audio.opus"), "audio").unwrap();

        let queue = queue(root.path());
        let policy = WorkPolicy::read(root.path()).unwrap();
        let mut report_today = WorkInventory::default();

        let mut archive_coords_today = BTreeSet::new();
        archive_coords_today.insert(Coordinate {
            day: today.clone(),
            stream: "import.archive".to_string(),
            segment: archive_key.to_string(),
        });

        let work_today = inspect_day(
            root.path(),
            &today,
            &queue,
            None,
            DaySources {
                imports: &[],
                archive_coordinates: &archive_coords_today,
                roots: None,
            },
            policy,
            &mut report_today,
            &|| Ok(()),
        )
        .unwrap();

        let archive_c = work_today
            .candidates
            .iter()
            .find(|c| c.request.reference.ends_with("import.archive-120000_60"))
            .expect("archive candidate");
        assert_eq!(archive_c.request.priority.unwrap().band, WorkBand::Import);

        let notes_c = work_today
            .candidates
            .iter()
            .find(|c| c.request.reference.ends_with("notes-120100_60"))
            .expect("notes candidate");
        assert_eq!(notes_c.request.priority.unwrap().band, WorkBand::Now);

        // 2. Day 19990101
        let past_day = "19990101";
        let past_archive_dir = root
            .path()
            .join("chronicle")
            .join(past_day)
            .join("import.archive")
            .join(archive_key);
        std::fs::create_dir_all(&past_archive_dir).unwrap();
        std::fs::write(past_archive_dir.join("audio.opus"), "audio").unwrap();
        std::fs::write(
            past_archive_dir.join("audio.jsonl"),
            "{}\n{\"start\":\"12:00:00\",\"text\":\"speech\"}\n",
        )
        .unwrap();

        let mut report_past_empty = WorkInventory::default();
        let work_past_empty = inspect_day(
            root.path(),
            past_day,
            &queue,
            None,
            DaySources {
                imports: &[],
                archive_coordinates: &BTreeSet::new(),
                roots: None,
            },
            policy,
            &mut report_past_empty,
            &|| Ok(()),
        )
        .unwrap();
        assert!(
            !work_past_empty
                .candidates
                .iter()
                .any(|c| c.request.reference.contains("import.archive"))
        );

        let mut past_archive_coords = BTreeSet::new();
        past_archive_coords.insert(Coordinate {
            day: past_day.to_string(),
            stream: "import.archive".to_string(),
            segment: archive_key.to_string(),
        });
        let mut report_past_set = WorkInventory::default();
        let work_past_set = inspect_day(
            root.path(),
            past_day,
            &queue,
            None,
            DaySources {
                imports: &[],
                archive_coordinates: &past_archive_coords,
                roots: None,
            },
            policy,
            &mut report_past_set,
            &|| Ok(()),
        )
        .unwrap();

        let past_archive_c = work_past_set
            .candidates
            .iter()
            .find(|c| c.request.reference.ends_with("import.archive-120000_60"))
            .expect("past archive candidate");
        assert_eq!(
            past_archive_c.request.priority.unwrap().band,
            WorkBand::Import
        );
    }

    #[cfg(all(unix, feature = "full-tests"))]
    #[test]
    fn pending_archive_defers_coverage_without_claiming_finished_history() {
        use solstone_core_import::audio_archive::claims::{
            register_pending_tile, retire_pending_tile,
        };
        let root = fixture("local");
        let day = "19990101";
        let selected = root
            .path()
            .join("chronicle")
            .join(day)
            .join("import.archive/120000_60");
        std::fs::create_dir_all(&selected).unwrap();
        std::fs::write(selected.join("audio.opus"), "audio").unwrap();
        std::fs::write(
            selected.join("audio.jsonl"),
            "{}\n{\"start\":\"12:00:00\",\"text\":\"speech\"}\n",
        )
        .unwrap();
        segment(root.path(), day, "120100_60");
        let other = root
            .path()
            .join("chronicle")
            .join(day)
            .join("import.archive/130000_300");
        std::fs::create_dir_all(&other).unwrap();
        let poison = other.join("unselected-source");
        std::os::unix::fs::symlink(root.path().join("absent"), &poison).unwrap();
        // Positive control: the old daily evidence projection reaches this sibling.
        assert!(daily_coverage::read_daily_coverage(root.path(), day).is_err());
        std::fs::create_dir_all(root.path().join("imports")).unwrap();
        use solstone_core_import::audio_archive::{
            AudioArchiveMemberInput, AudioArchiveSettings, DurationInput, MemberStartEvidence,
            plan_audio_archive,
            record::{plan_to_record, save_audio_archive_record},
        };
        let plan = plan_audio_archive(
            &[AudioArchiveMemberInput {
                original_relative_name: "other.opus".into(),
                byte_len: 1,
                sha256: "a".repeat(64),
                duration: DurationInput::Milliseconds(1000),
                start: MemberStartEvidence {
                    instant: Some("1999-01-01T13:00:00Z".into()),
                    ..Default::default()
                },
                run_note: None,
            }],
            chrono_tz::UTC,
            AudioArchiveSettings {
                qualifier: "archive".into(),
                archive_identity: "fixture".into(),
            },
        )
        .unwrap();
        save_audio_archive_record(root.path(), "19990101_010101", &plan_to_record(&plan)).unwrap();

        register_pending_tile(root.path(), "19990101_010101", day, "130000_300").unwrap();
        let q = queue(root.path());
        let policy = WorkPolicy::read(root.path()).unwrap();
        let coords = BTreeSet::from([Coordinate {
            day: day.into(),
            stream: "import.archive".into(),
            segment: "120000_60".into(),
        }]);
        let mut report = WorkInventory::default();
        let work = inspect_day(
            root.path(),
            day,
            &q,
            None,
            DaySources {
                imports: &[],
                archive_coordinates: &coords,
                roots: None,
            },
            policy,
            &mut report,
            &|| Ok(()),
        )
        .unwrap();
        assert!(!work.daily_current);
        assert!(work.history.archive_pending);
        assert!(
            work.candidates
                .iter()
                .any(|c| c.request.reference.ends_with("import.archive-120000_60"))
        );
        assert!(
            work.candidates
                .iter()
                .any(|c| c.request.reference.ends_with("notes-120100_60"))
        );
        assert!(!work.candidates.iter().any(|c| {
            c.request
                .cmd
                .as_wire()
                .iter()
                .any(|arg| arg == "--daily-only")
        }));
        assert!(
            report
                .holds
                .iter()
                .any(|h| h.day == day && h.reason == "segment_prerequisites")
        );
        let daily = task(
            day,
            None,
            priority(day, None, chrono::Utc::now().date_naive(), None),
            None,
        );
        assert!(!still_owed(root.path(), &daily, None, &|| Ok(())).unwrap());
        let unit = Unit::Daily {
            day: day.into(),
            talent: "schedule".into(),
            facet: None,
        };
        work_retry::observe_held(root.path(), &unit, "fixture", "input").unwrap();
        let mut retry = daily.clone();
        let mut wire = retry.request.cmd.as_wire().to_vec();
        wire.extend(["--retry-unit".into(), unit.id().unwrap()]);
        retry.request.cmd = TaskArgv::from_wire(wire).unwrap();
        assert!(!still_owed(root.path(), &retry, None, &|| Ok(())).unwrap());
        // Even a selected Ready tile cannot release the day's other pending
        // archive prerequisite or complete its outstanding owner intent.
        thought_without_receipt(root.path(), day, &["120000_60"]);
        std::fs::remove_dir_all(&selected).unwrap();
        std::fs::rename(
            root.path()
                .join("chronicle")
                .join(day)
                .join("notes/120000_60"),
            &selected,
        )
        .unwrap();
        for entry in
            std::fs::read_dir(root.path().join("chronicle").join(day).join("health")).unwrap()
        {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|ext| ext == "jsonl") {
                let text = std::fs::read_to_string(&path)
                    .unwrap()
                    .replace("\"stream\":\"notes\"", "\"stream\":\"import.archive\"");
                std::fs::write(path, text).unwrap();
            }
        }
        let input = solstone_core_system_health::newest_segment_input_ms(&selected).unwrap();
        solstone_core_system::segment_publication::publish(
            root.path(),
            &selected,
            input,
            |_, _| {
                Ok(
                    solstone_core_indexer_store::scan::RescanFileStatus::Indexed {
                        warnings: Vec::new(),
                    },
                )
            },
        )
        .unwrap();
        let intent = work_intent::register(root.path(), day, false).unwrap();
        let mut report = WorkInventory::default();
        let work = inspect_day(
            root.path(),
            day,
            &q,
            Some(&intent),
            DaySources {
                imports: &[],
                archive_coordinates: &coords,
                roots: None,
            },
            policy,
            &mut report,
            &|| Ok(()),
        )
        .unwrap();
        assert!(work.completed_segments.contains(&Coordinate {
            day: day.into(),
            stream: "import.archive".into(),
            segment: "120000_60".into()
        }));
        assert!(!work.daily_current);
        assert!(work.history.archive_pending);
        assert_eq!(work_intent::read(root.path()).unwrap()[day].id, intent.id);
        let days = BTreeMap::from([(day.to_string(), root.path().join("chronicle").join(day))]);
        let reports = BTreeMap::from([(day.to_string(), report)]);
        let mut total = WorkInventory::default();
        aggregate(&mut total, &reports, &[], &days, 1);
        let history = history_progress(
            &total,
            &BTreeMap::from([(day.to_string(), (work.history, false))]),
            chrono::Utc::now().date_naive(),
            None,
        );
        assert_eq!(history.caught_up_since.as_deref(), Some("19990102"));
        assert!(history.sample.is_none());
        assert!(history.net_per_hour.is_none());
        assert!(
            total
                .holds
                .iter()
                .any(|h| h.reason == "segment_prerequisites")
        );
        reconcile(root.path(), &q, &AtomicBool::new(false)).unwrap();
        assert_eq!(work_intent::read(root.path()).unwrap()[day].id, intent.id);
        let published = solstone_core_system::work_inventory::read(root.path())
            .unwrap()
            .unwrap();
        let history = published.history.unwrap();
        assert!(history.sample.is_none());
        assert!(history.net_per_hour.is_none());
        assert!(
            published
                .holds
                .iter()
                .any(|h| h.day == day && h.reason == "segment_prerequisites")
        );
        // Clearing the final indexed prerequisite restores the actual coverage
        // read. The poisoned source must still be visible as an error then.
        retire_pending_tile(root.path(), "19990101_010101", day, "130000_300").unwrap();
        assert!(
            inspect_day(
                root.path(),
                day,
                &q,
                None,
                DaySources {
                    imports: &[],
                    archive_coordinates: &coords,
                    roots: None,
                },
                policy,
                &mut WorkInventory::default(),
                &|| Ok(())
            )
            .is_err()
        );
        assert!(still_owed(root.path(), &daily, None, &|| Ok(())).is_err());
        std::fs::remove_file(poison).unwrap();
        let work = inspect_day(
            root.path(),
            day,
            &q,
            None,
            DaySources {
                imports: &[],
                archive_coordinates: &coords,
                roots: None,
            },
            policy,
            &mut WorkInventory::default(),
            &|| Ok(()),
        )
        .unwrap();
        assert!(!work.history.archive_pending);
        assert!(
            !work.daily_current,
            "ordinary unsensed segment still blocks daily readiness"
        );
        assert!(!work.candidates.iter().any(|c| {
            c.request
                .cmd
                .as_wire()
                .iter()
                .any(|arg| arg == "--daily-only")
        }));
        assert!(!still_owed(root.path(), &daily, None, &|| Ok(())).unwrap());
        let coverage = daily_coverage::read_daily_coverage(root.path(), day).unwrap();
        let covered = coverage
            .units
            .iter()
            .find(|u| !u.evidence_revision.is_empty())
            .unwrap();
        let valid_unit = Unit::Daily {
            day: day.into(),
            talent: covered.identity.name.clone(),
            facet: covered.identity.facet.clone(),
        };
        let evidence = work_retry::daily::evidence(root.path(), &valid_unit).unwrap();
        let mut capped = solstone_core_journal_io::DailyUnitRecord::new(
            work_retry::daily::identity(&valid_unit).unwrap(),
            &evidence.revision,
            &evidence.contract,
        );
        capped.status = solstone_core_journal_io::DailyUnitStatus::Capped;
        capped.reason_code = Some("schema_invalid".into());
        solstone_core_journal_io::save_daily_unit_record(root.path(), &capped).unwrap();
        let owner = work_retry::daily::request(root.path(), &valid_unit).unwrap();
        let mut valid_retry = daily.clone();
        let mut wire = valid_retry.request.cmd.as_wire().to_vec();
        wire.extend([
            "--retry-unit".into(),
            valid_unit.id().unwrap(),
            "--retry-cause".into(),
            format!("owner:{owner}"),
        ]);
        valid_retry.request.cmd = TaskArgv::from_wire(wire).unwrap();
        assert!(
            !still_owed(root.path(), &valid_retry, None, &|| Ok(())).unwrap(),
            "ordinary prerequisite also holds a genuinely granted daily retry"
        );
        std::fs::remove_dir_all(
            root.path()
                .join("chronicle")
                .join(day)
                .join("notes/120100_60"),
        )
        .unwrap();
        let work = inspect_day(
            root.path(),
            day,
            &q,
            Some(&intent),
            DaySources {
                imports: &[],
                archive_coordinates: &coords,
                roots: None,
            },
            policy,
            &mut WorkInventory::default(),
            &|| Ok(()),
        )
        .unwrap();
        assert!(
            work.candidates.iter().any(|c| c
                .request
                .cmd
                .as_wire()
                .iter()
                .any(|arg| arg == "--daily-only")),
            "last archive prerequisite released and real daily work is admitted"
        );
        assert!(still_owed(root.path(), &daily, None, &|| Ok(())).unwrap());
        assert!(
            still_owed(root.path(), &valid_retry, None, &|| Ok(())).unwrap(),
            "granted daily retry resumes once prerequisites are Ready"
        );
        register_pending_tile(root.path(), "19990101_010101", day, "130000_300").unwrap();
        assert!(
            !still_owed(root.path(), &valid_retry, None, &|| Ok(())).unwrap(),
            "pending archive also holds a valid retry grant"
        );
        retire_pending_tile(root.path(), "19990101_010101", day, "130000_300").unwrap();
        assert!(still_owed(root.path(), &valid_retry, None, &|| Ok(())).unwrap());
    }

    #[cfg(all(unix, feature = "full-tests"))]
    #[test]
    fn exact_candidate_recheck_does_not_read_unrelated_segments() {
        use std::os::unix::fs::PermissionsExt;
        let root = fixture("local");
        let day = "19990101";
        segment(root.path(), day, "120000_60");
        let sibling = root
            .path()
            .join("chronicle")
            .join(day)
            .join("notes/130000_60");
        std::fs::create_dir_all(&sibling).unwrap();
        std::fs::write(sibling.join("audio.opus"), "audio").unwrap();
        std::fs::set_permissions(&sibling, std::fs::Permissions::from_mode(0o000)).unwrap();
        let same_key = root
            .path()
            .join("chronicle")
            .join(day)
            .join("import.archive/120000_60");
        std::fs::create_dir_all(&same_key).unwrap();
        std::fs::write(same_key.join("audio.opus"), "other audio").unwrap();
        let whole = solstone_core_system_health::scan_day(
            &solstone_core_system_health::FilesystemSegmentSource,
            root.path(),
            day,
            chrono::Utc::now(),
        );
        let ordinary = task(
            day,
            Some(("120000_60", "notes")),
            priority(
                day,
                Some("120000_60"),
                chrono::Utc::now().date_naive(),
                None,
            ),
            None,
        );
        let archive = task(
            day,
            Some(("120000_60", "import.archive")),
            priority(
                day,
                Some("120000_60"),
                chrono::Utc::now().date_naive(),
                None,
            ),
            None,
        );
        let ordinary_result = still_owed(root.path(), &ordinary, None, &|| Ok(()));
        let archive_result = still_owed(root.path(), &archive, None, &|| Ok(()));
        std::fs::set_permissions(&sibling, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(
            whole.is_err(),
            "positive control requires denied sibling traversal"
        );
        assert!(ordinary_result.unwrap());
        assert!(archive_result.unwrap());
        thought_without_receipt(root.path(), day, &["120000_60"]);
        assert!(
            still_owed(root.path(), &ordinary, None, &|| Ok(())).unwrap(),
            "thought output still owes publication"
        );
        let target = root
            .path()
            .join("chronicle")
            .join(day)
            .join("notes/120000_60");
        let input = solstone_core_system_health::newest_segment_input_ms(&target).unwrap();
        solstone_core_system::segment_publication::publish(root.path(), &target, input, |_, _| {
            Ok(
                solstone_core_indexer_store::scan::RescanFileStatus::Indexed {
                    warnings: Vec::new(),
                },
            )
        })
        .unwrap();
        assert!(
            !still_owed(root.path(), &ordinary, None, &|| Ok(())).unwrap(),
            "Ready target is not requeued"
        );
        assert!(
            still_owed(root.path(), &archive, None, &|| Ok(())).unwrap(),
            "same-key archive remains independently owed"
        );
        std::fs::remove_dir_all(&target).unwrap();
        assert!(!still_owed(root.path(), &ordinary, None, &|| Ok(())).unwrap());
        thought_without_receipt(root.path(), day, &["120000_60"]);
        std::fs::remove_dir_all(&same_key).unwrap();
        std::fs::rename(&target, &same_key).unwrap();
        for entry in
            std::fs::read_dir(root.path().join("chronicle").join(day).join("health")).unwrap()
        {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|ext| ext == "jsonl") {
                let text = std::fs::read_to_string(&path)
                    .unwrap()
                    .replace("\"stream\":\"notes\"", "\"stream\":\"import.archive\"");
                std::fs::write(path, text).unwrap();
            }
        }
        assert!(
            still_owed(root.path(), &archive, None, &|| Ok(())).unwrap(),
            "archive thinking alone still owes publication"
        );
        let input = solstone_core_system_health::newest_segment_input_ms(&same_key).unwrap();
        solstone_core_system::segment_publication::publish(
            root.path(),
            &same_key,
            input,
            |_, _| {
                Ok(
                    solstone_core_indexer_store::scan::RescanFileStatus::Indexed {
                        warnings: Vec::new(),
                    },
                )
            },
        )
        .unwrap();
        assert!(
            !still_owed(root.path(), &archive, None, &|| Ok(())).unwrap(),
            "Ready archive target is not requeued"
        );
        assert!(
            !still_owed(root.path(), &ordinary, None, &|| Ok(())).unwrap(),
            "other stream does not borrow archive readiness"
        );
        std::fs::remove_dir_all(same_key).unwrap();
        assert!(!still_owed(root.path(), &archive, None, &|| Ok(())).unwrap());
        assert!(!still_owed(root.path(), &ordinary, None, &|| Ok(())).unwrap());
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn corrupt_archive_barrier_does_not_admit_daily_work() {
        let root = fixture("local");
        std::fs::create_dir_all(root.path().join("imports")).unwrap();
        std::fs::write(
            solstone_core_import::audio_archive::claim_db_path(root.path()),
            "not sqlite",
        )
        .unwrap();
        let day = "19990101";
        let candidate = task(
            day,
            None,
            priority(day, None, chrono::Utc::now().date_naive(), None),
            None,
        );
        assert!(still_owed(root.path(), &candidate, None, &|| Ok(())).is_err());
        assert!(
            inspect_day(
                root.path(),
                day,
                &queue(root.path()),
                None,
                DaySources {
                    imports: &[],
                    archive_coordinates: &BTreeSet::new(),
                    roots: None,
                },
                WorkPolicy::read(root.path()).unwrap(),
                &mut WorkInventory::default(),
                &|| Ok(())
            )
            .is_err()
        );
    }

    #[cfg(all(unix, feature = "full-tests"))]
    #[test]
    fn maintenance_keeps_its_recent_key_projection_outside_archive_processing_bound() {
        use std::os::unix::fs::PermissionsExt;
        let root = fixture("local");
        let recent = (chrono::Utc::now().date_naive() - chrono::Duration::days(1))
            .format("%Y%m%d")
            .to_string();
        let stream = root
            .path()
            .join("chronicle")
            .join(&recent)
            .join("import.archive");
        std::fs::create_dir_all(stream.join("120000_300")).unwrap();
        std::fs::write(stream.join("120000_300/audio.opus"), "audio").unwrap();
        std::fs::create_dir_all(root.path().join("imports")).unwrap();
        solstone_core_import::audio_archive::claims::register_pending_tile(
            root.path(),
            "19990101_010101",
            &recent,
            "120000_300",
        )
        .unwrap();
        assert!(
            !solstone_core_import::audio_archive::archive_day_has_pending(root.path(), "19990102")
                .unwrap()
        );
        std::fs::set_permissions(&stream, std::fs::Permissions::from_mode(0o000)).unwrap();
        let coverage = daily_coverage::read_daily_coverage(root.path(), "19990102");
        std::fs::set_permissions(&stream, std::fs::Permissions::from_mode(0o755)).unwrap();
        let maintenance = coverage.unwrap().maintenance.unwrap();
        assert_eq!(maintenance.identity.name, "daily_schedule");
        assert_eq!(
            maintenance.state,
            CoverageState::Unreadable,
            "global maintenance still reads recent archive keys from a nonpending day's coverage"
        );
        let restored = daily_coverage::read_daily_coverage(root.path(), "19990102")
            .unwrap()
            .maintenance
            .unwrap();
        assert_ne!(restored.state, CoverageState::Unreadable);
    }
}
