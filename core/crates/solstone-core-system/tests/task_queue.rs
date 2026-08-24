// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use solstone_core_system::cap::CapResolver;
use solstone_core_system::partition::Partition;
use solstone_core_system::process::{ProcessEvent, ProcessEventSink};
use solstone_core_system::queue::{
    ProcessState, ProcessStateProbe, SystemProcessStateProbe, TIMEOUT_EXIT_STATUS, TaskQueue,
    TaskQueueEvent, TaskQueueEventSink, TaskQueueOptions,
};
use solstone_core_system::request::{BusTaskRequest, ExecutionRequest, TaskArgv};

const FIXTURE: &str = env!("CARGO_BIN_EXE_solstone-system-test-child");

struct Bed {
    root: PathBuf,
}

impl Bed {
    fn new(name: &str) -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("solstone-task-queue-{name}-{stamp}"));
        fs::create_dir_all(&root).expect("temporary journal");
        Self { root }
    }
}

impl Drop for Bed {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[derive(Default)]
struct QueueCollector(Mutex<Vec<TaskQueueEvent>>);

impl TaskQueueEventSink for QueueCollector {
    fn emit(&self, event: TaskQueueEvent) {
        self.0.lock().expect("queue collector lock").push(event);
    }
}

#[derive(Default)]
struct ProcessCollector(Mutex<Vec<ProcessEvent>>);

impl ProcessEventSink for ProcessCollector {
    fn emit(&self, event: ProcessEvent) {
        self.0.lock().expect("process collector lock").push(event);
    }
}

struct FixedCap(Duration);

impl CapResolver for FixedCap {
    fn cap_for(&self, _partition: &Partition) -> Duration {
        self.0
    }
}

struct PartitionCaps(BTreeMap<String, Duration>);

impl CapResolver for PartitionCaps {
    fn cap_for(&self, partition: &Partition) -> Duration {
        self.0
            .get(partition.as_str())
            .copied()
            .expect("partition cap")
    }
}

enum PanicTarget {
    QueueChanged,
    Started,
    Stopped(String),
}

struct PanicOnceSink {
    target: PanicTarget,
    panicked: AtomicBool,
    events: Mutex<Vec<TaskQueueEvent>>,
}

impl PanicOnceSink {
    fn new(target: PanicTarget) -> Self {
        Self {
            target,
            panicked: AtomicBool::new(false),
            events: Mutex::new(Vec::new()),
        }
    }

    fn should_panic(&self, event: &TaskQueueEvent) -> bool {
        match (&self.target, event) {
            (PanicTarget::QueueChanged, TaskQueueEvent::QueueChanged { .. })
            | (PanicTarget::Started, TaskQueueEvent::Started { .. }) => true,
            (
                PanicTarget::Stopped(reference),
                TaskQueueEvent::Stopped {
                    reference: event_ref,
                    ..
                },
            ) => reference == event_ref,
            _ => false,
        }
    }
}

impl TaskQueueEventSink for PanicOnceSink {
    fn emit(&self, event: TaskQueueEvent) {
        if self.should_panic(&event) && !self.panicked.swap(true, Ordering::SeqCst) {
            panic!("intentional sink panic");
        }
        self.events.lock().expect("panic sink events").push(event);
    }
}

struct NotifyingProbe {
    calls: AtomicUsize,
    sender: Mutex<Option<mpsc::Sender<()>>>,
}

impl ProcessStateProbe for NotifyingProbe {
    fn state(&self, _pid: u32) -> ProcessState {
        if self.calls.fetch_add(1, Ordering::SeqCst) > 0
            && let Some(sender) = self.sender.lock().expect("probe notification lock").take()
        {
            sender.send(()).expect("notify probe");
        }
        ProcessState::Other
    }
}

fn queue(
    bed: &Bed,
    cap: Duration,
    ready: bool,
    queue_sink: Option<Arc<QueueCollector>>,
    process_sink: Option<Arc<ProcessCollector>>,
) -> TaskQueue {
    TaskQueue::new(TaskQueueOptions {
        journal_root: bed.root.clone(),
        cap_resolver: Arc::new(FixedCap(cap)),
        process_state_probe: Arc::new(SystemProcessStateProbe),
        queue_sink: queue_sink.map(|sink| sink as Arc<dyn TaskQueueEventSink>),
        process_sink: process_sink.map(|sink| sink as Arc<dyn ProcessEventSink>),
        ready,
        before_deadline_commit: None,
    })
}

fn queue_with_probe(
    bed: &Bed,
    cap: Duration,
    probe: Arc<dyn ProcessStateProbe>,
    hook: Option<Arc<dyn Fn() + Send + Sync>>,
) -> TaskQueue {
    TaskQueue::new(TaskQueueOptions {
        journal_root: bed.root.clone(),
        cap_resolver: Arc::new(FixedCap(cap)),
        process_state_probe: probe,
        queue_sink: None,
        process_sink: None,
        ready: true,
        before_deadline_commit: hook,
    })
}

fn queue_with_event_sink(bed: &Bed, cap: Duration, sink: Arc<dyn TaskQueueEventSink>) -> TaskQueue {
    TaskQueue::new(TaskQueueOptions {
        journal_root: bed.root.clone(),
        cap_resolver: Arc::new(FixedCap(cap)),
        process_state_probe: Arc::new(SystemProcessStateProbe),
        queue_sink: Some(sink),
        process_sink: None,
        ready: true,
        before_deadline_commit: None,
    })
}

fn request(
    reference: &str,
    args: &[String],
    day: Option<&str>,
    scheduler_name: Option<&str>,
) -> ExecutionRequest {
    ExecutionRequest::Bus(BusTaskRequest {
        cmd: TaskArgv::from_wire(args.to_vec()).expect("fixture command"),
        reference: reference.to_owned(),
        day: day.map(str::to_owned),
        scheduler_name: scheduler_name.map(str::to_owned),
        queue_if_active_cmd_differs: false,
        daily_catchup_provenance: None,
    })
}

fn command(args: &[&str]) -> Vec<String> {
    let mut command = vec![FIXTURE.to_owned()];
    command.extend(args.iter().map(|arg| (*arg).to_owned()));
    command
}

fn wait_until(mut condition: impl FnMut() -> bool) {
    for _ in 0..500 {
        if condition() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for condition");
}

fn wait_for_ready(path: &Path) {
    wait_until(|| path.exists());
}

fn wait_for_history(queue: &TaskQueue, count: usize) {
    wait_until(|| queue.history().len() >= count);
}

fn process_is_gone(pid: u32) -> bool {
    let pid = i32::try_from(pid).expect("fixture pid fits i32");
    matches!(
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None),
        Err(nix::errno::Errno::ESRCH)
    )
}

fn started_references(sink: &QueueCollector) -> Vec<String> {
    sink.0
        .lock()
        .expect("queue collector lock")
        .iter()
        .filter_map(|event| match event {
            TaskQueueEvent::Started { reference, .. } => Some(reference.clone()),
            _ => None,
        })
        .collect()
}

fn stopped_references(sink: &QueueCollector) -> Vec<String> {
    sink.0
        .lock()
        .expect("queue collector lock")
        .iter()
        .filter_map(|event| match event {
            TaskQueueEvent::Stopped { reference, .. } => Some(reference.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn ac7_different_partitions_do_not_block_each_other() {
    let bed = Bed::new("ac7");
    let first = bed.root.join("first-child");
    let second = bed.root.join("second-child");
    std::os::unix::fs::symlink(FIXTURE, &first).expect("first fixture link");
    std::os::unix::fs::symlink(FIXTURE, &second).expect("second fixture link");
    let queue = queue(&bed, Duration::from_secs(5), true, None, None);
    let ready_a = bed.root.join("first-ready");
    let ready_b = bed.root.join("second-ready");
    queue.submit(request(
        "first",
        &[
            first.to_string_lossy().into_owned(),
            "ready-sleep".into(),
            ready_a.to_string_lossy().into_owned(),
            "300".into(),
        ],
        None,
        None,
    ));
    queue.submit(request(
        "second",
        &[
            second.to_string_lossy().into_owned(),
            "ready-sleep".into(),
            ready_b.to_string_lossy().into_owned(),
            "300".into(),
        ],
        None,
        None,
    ));
    wait_for_ready(&ready_a);
    wait_for_ready(&ready_b);
    assert_eq!(queue.active_process_handles().len(), 2);
}

#[test]
fn ac8_busy_partition_queues_without_second_process() {
    let bed = Bed::new("ac8");
    let sink = Arc::new(QueueCollector::default());
    let queue = queue(&bed, Duration::from_secs(5), true, Some(sink.clone()), None);
    let ready = bed.root.join("ready");
    queue.submit(request(
        "one",
        &command(&["ready-sleep", ready.to_str().expect("utf8"), "300"]),
        None,
        None,
    ));
    wait_for_ready(&ready);
    queue.submit(request("two", &command(&["lines"]), None, None));
    assert_eq!(queue.collect_queue_counts().values().sum::<usize>(), 1);
    assert_eq!(started_references(&sink), vec!["one"]);
}

#[test]
fn ac9_identical_queued_commands_coalesce_and_notify_both_refs() {
    let bed = Bed::new("ac9");
    let sink = Arc::new(QueueCollector::default());
    let queue = queue(&bed, Duration::from_secs(5), true, Some(sink.clone()), None);
    let ready = bed.root.join("ready");
    queue.submit(request(
        "running",
        &command(&["ready-sleep", ready.to_str().expect("utf8"), "120"]),
        None,
        None,
    ));
    wait_for_ready(&ready);
    let queued = command(&["lines"]);
    queue.submit(request("queued-a", &queued, None, None));
    queue.submit(request("queued-b", &queued, None, None));
    assert_eq!(queue.collect_queue_counts().values().sum::<usize>(), 1);
    assert!(
        sink.0
            .lock()
            .expect("queue events")
            .iter()
            .any(|event| matches!(event,
                TaskQueueEvent::QueueChanged { queue, .. }
                    if queue.len() == 1 && queue[0].references == vec!["queued-a", "queued-b"]
            ))
    );
    wait_for_history(&queue, 2);
    let stopped = stopped_references(&sink);
    assert!(stopped.contains(&"queued-a".to_owned()));
    assert!(stopped.contains(&"queued-b".to_owned()));
}

#[test]
fn ac10_running_identical_command_becomes_a_second_execution() {
    let bed = Bed::new("ac10");
    let sink = Arc::new(QueueCollector::default());
    let queue = queue(&bed, Duration::from_secs(5), true, Some(sink.clone()), None);
    let ready = bed.root.join("ready");
    let cmd = command(&["ready-sleep", ready.to_str().expect("utf8"), "100"]);
    queue.submit(request("first", &cmd, None, None));
    wait_for_ready(&ready);
    queue.submit(request("second", &cmd, None, None));
    wait_for_history(&queue, 2);
    assert_eq!(started_references(&sink), vec!["first", "second"]);
}

#[test]
fn ac11_different_commands_same_partition_are_sequential() {
    let bed = Bed::new("ac11");
    let sink = Arc::new(QueueCollector::default());
    let queue = queue(&bed, Duration::from_secs(5), true, Some(sink.clone()), None);
    let ready = bed.root.join("ready");
    queue.submit(request(
        "first",
        &command(&["ready-sleep", ready.to_str().expect("utf8"), "100"]),
        None,
        None,
    ));
    wait_for_ready(&ready);
    queue.submit(request("second", &command(&["lines"]), None, None));
    wait_for_history(&queue, 2);
    assert_eq!(started_references(&sink), vec!["first", "second"]);
}

#[test]
fn ac12_duplicate_queued_reference_is_not_added_twice() {
    let bed = Bed::new("ac12");
    let sink = Arc::new(QueueCollector::default());
    let queue = queue(&bed, Duration::from_secs(5), true, Some(sink.clone()), None);
    let ready = bed.root.join("ready");
    queue.submit(request(
        "running",
        &command(&["ready-sleep", ready.to_str().expect("utf8"), "100"]),
        None,
        None,
    ));
    wait_for_ready(&ready);
    let cmd = command(&["lines"]);
    queue.submit(request("queued", &cmd, None, None));
    queue.submit(request("queued", &cmd, None, None));
    wait_for_history(&queue, 2);
    assert_eq!(
        stopped_references(&sink)
            .iter()
            .filter(|reference| *reference == "queued")
            .count(),
        1
    );
}

#[test]
fn ac13_distinct_queued_commands_start_in_submission_order() {
    let bed = Bed::new("ac13");
    let sink = Arc::new(QueueCollector::default());
    let queue = queue(&bed, Duration::from_secs(5), true, Some(sink.clone()), None);
    let ready = bed.root.join("ready");
    queue.submit(request(
        "running",
        &command(&["ready-sleep", ready.to_str().expect("utf8"), "100"]),
        None,
        None,
    ));
    wait_for_ready(&ready);
    for reference in ["one", "two", "three"] {
        queue.submit(request(
            reference,
            &command(&["lines", reference]),
            None,
            None,
        ));
    }
    wait_for_history(&queue, 4);
    assert_eq!(
        started_references(&sink),
        vec!["running", "one", "two", "three"]
    );
}

#[test]
fn ac14_pre_ready_identical_requests_deterministically_make_two_runs() {
    let bed = Bed::new("ac14");
    let sink = Arc::new(QueueCollector::default());
    let queue = queue(
        &bed,
        Duration::from_secs(5),
        false,
        Some(sink.clone()),
        None,
    );
    let cmd = command(&["lines"]);
    for reference in ["one", "two", "three", "four"] {
        queue.submit(request(reference, &cmd, None, None));
    }
    assert_eq!(queue.collect_queue_counts().get("pending"), Some(&4));
    queue.set_ready();
    wait_for_history(&queue, 2);
    assert_eq!(started_references(&sink).len(), 2);
    assert_eq!(stopped_references(&sink).len(), 4);
}

#[test]
fn set_ready_after_shutdown_keeps_pending_work_inert() {
    let bed = Bed::new("ready-after-shutdown");
    let queue = queue(&bed, Duration::from_secs(5), false, None, None);
    queue.submit(request("pending", &command(&["lines"]), None, None));
    assert_eq!(queue.shutdown(), 0);
    queue.set_ready();
    assert!(queue.active_process_handles().is_empty());
    assert_eq!(queue.collect_queue_counts().get("pending"), Some(&1));
    assert!(queue.history().is_empty());
}

#[test]
fn submit_after_shutdown_is_rejected_without_queueing() {
    let bed = Bed::new("submit-after-shutdown");
    let queue = queue(&bed, Duration::from_secs(5), true, None, None);
    assert_eq!(queue.shutdown(), 0);
    assert_eq!(
        queue.submit(request("rejected", &command(&["lines"]), None, None)),
        solstone_core_system::queue::SubmitOutcome::Rejected
    );
    assert!(queue.collect_queue_counts().is_empty());
}

#[test]
fn panicking_queue_changed_or_started_sink_does_not_wedge_partition() {
    for (name, target) in [
        ("queue-changed", PanicTarget::QueueChanged),
        ("started", PanicTarget::Started),
    ] {
        let bed = Bed::new(name);
        let sink = Arc::new(PanicOnceSink::new(target));
        let queue = queue_with_event_sink(&bed, Duration::from_secs(5), sink);
        let ready = bed.root.join("ready");
        queue.submit(request(
            "first",
            &command(&["ready-sleep", ready.to_str().expect("utf8"), "100"]),
            None,
            None,
        ));
        wait_for_ready(&ready);
        queue.submit(request("second", &command(&["lines"]), None, None));
        wait_for_history(&queue, 2);
        assert_eq!(
            queue
                .history()
                .iter()
                .map(|entry| entry.reference.as_str())
                .collect::<Vec<_>>(),
            vec!["first", "second"]
        );
    }
}

#[test]
fn panicking_stopped_sink_does_not_skip_later_coalesced_ref() {
    let bed = Bed::new("stopped-fanout");
    let sink = Arc::new(PanicOnceSink::new(PanicTarget::Stopped(
        "queued-a".to_owned(),
    )));
    let queue = queue_with_event_sink(&bed, Duration::from_secs(5), sink.clone());
    let ready = bed.root.join("ready");
    queue.submit(request(
        "running",
        &command(&["ready-sleep", ready.to_str().expect("utf8"), "100"]),
        None,
        None,
    ));
    wait_for_ready(&ready);
    let queued = command(&["lines"]);
    queue.submit(request("queued-a", &queued, None, None));
    queue.submit(request("queued-b", &queued, None, None));
    wait_for_history(&queue, 2);
    // History reaching 2 is a PROXY for the thing under test, not the thing
    // itself: history and sink fan-out are separate paths, so the `Stopped`
    // event for `queued-b` can still be in flight when history is complete.
    // Asserting once on that proxy failed roughly one whole-crate run in four.
    // Poll instead — a sink that never receives the event still fails, just
    // after a bounded wait rather than on a race.
    let saw_queued_b = || {
        sink.events
            .lock()
            .expect("panic sink events")
            .iter()
            .any(|event| {
                matches!(event, TaskQueueEvent::Stopped { reference, .. } if reference == "queued-b")
            })
    };
    for _ in 0..2_000 {
        if saw_queued_b() {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("panicking stopped sink never received the coalesced ref queued-b");
}

#[test]
fn ac16_first_queued_submitter_owns_history_metadata_but_all_refs_stop() {
    let bed = Bed::new("ac16");
    let sink = Arc::new(QueueCollector::default());
    let queue = queue(&bed, Duration::from_secs(5), true, Some(sink.clone()), None);
    let ready = bed.root.join("ready");
    queue.submit(request(
        "running",
        &command(&["ready-sleep", ready.to_str().expect("utf8"), "100"]),
        None,
        None,
    ));
    wait_for_ready(&ready);
    let cmd = command(&["lines"]);
    queue.submit(request(
        "first",
        &cmd,
        Some("20260801"),
        Some("first-schedule"),
    ));
    queue.submit(request(
        "second",
        &cmd,
        Some("20260802"),
        Some("second-schedule"),
    ));
    wait_for_history(&queue, 2);
    let history = queue.history();
    assert_eq!(history[1].scheduler_name.as_deref(), Some("first-schedule"));
    assert!(
        sink.0
            .lock()
            .expect("queue events")
            .iter()
            .any(|event| matches!(event,
                TaskQueueEvent::QueueChanged { queue, .. }
                    if queue.iter().any(|entry| entry.day.as_deref() == Some("20260801")
                        && entry.scheduler_name.as_deref() == Some("first-schedule"))
            ))
    );
    assert!(stopped_references(&sink).contains(&"second".to_owned()));
    assert_eq!(
        history
            .iter()
            .filter(|entry| entry.reference == "second")
            .count(),
        0
    );
}

#[test]
fn ac18_cap_overrun_is_terminated_and_labeled_timeout() {
    let bed = Bed::new("ac18");
    let queue = queue(&bed, Duration::from_millis(10), true, None, None);
    let ready = bed.root.join("ready");
    queue.submit(request(
        "overrun",
        &command(&["ready-sleep", ready.to_str().expect("utf8"), "1000"]),
        None,
        None,
    ));
    wait_for_ready(&ready);
    queue.enforce_deadlines(Instant::now() + Duration::from_secs(1));
    wait_for_history(&queue, 1);
    assert_eq!(queue.history()[0].exit_status, TIMEOUT_EXIT_STATUS);
}

#[test]
fn ac19_two_stopped_ticks_terminate_with_the_same_timeout_label() {
    let bed = Bed::new("ac19");
    let queue = queue(&bed, Duration::from_secs(5), true, None, None);
    let ready = bed.root.join("ready");
    queue.submit(request(
        "stopped",
        &command(&["ready-sleep", ready.to_str().expect("utf8"), "5000"]),
        None,
        None,
    ));
    wait_for_ready(&ready);
    let pid = i32::try_from(queue.active_process_handles()[0].pid()).expect("pid");
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(pid),
        nix::sys::signal::Signal::SIGSTOP,
    )
    .expect("stop child");
    wait_until(|| SystemProcessStateProbe.state(pid as u32) == ProcessState::Stopped);
    queue.enforce_deadlines(Instant::now());
    queue.enforce_deadlines(Instant::now());
    wait_for_history(&queue, 1);
    assert_eq!(queue.history()[0].exit_status, TIMEOUT_EXIT_STATUS);
}

#[test]
fn ac20_under_cap_task_is_not_terminated() {
    let bed = Bed::new("ac20");
    let queue = queue(&bed, Duration::from_secs(5), true, None, None);
    let ready = bed.root.join("ready");
    queue.submit(request(
        "under",
        &command(&["ready-sleep", ready.to_str().expect("utf8"), "250"]),
        None,
        None,
    ));
    wait_for_ready(&ready);
    queue.enforce_deadlines(Instant::now());
    assert_eq!(queue.active_process_handles().len(), 1);
}

#[test]
fn ac21_stopped_ticks_reset_after_resume() {
    let bed = Bed::new("ac21");
    let queue = queue(&bed, Duration::from_secs(5), true, None, None);
    let ready = bed.root.join("ready");
    queue.submit(request(
        "stopped",
        &command(&["ready-sleep", ready.to_str().expect("utf8"), "5000"]),
        None,
        None,
    ));
    wait_for_ready(&ready);
    let pid = i32::try_from(queue.active_process_handles()[0].pid()).expect("pid");
    let process = nix::unistd::Pid::from_raw(pid);
    nix::sys::signal::kill(process, nix::sys::signal::Signal::SIGSTOP).expect("stop child");
    wait_until(|| SystemProcessStateProbe.state(pid as u32) == ProcessState::Stopped);
    queue.enforce_deadlines(Instant::now());
    assert_eq!(queue.active_process_handles().len(), 1);
    nix::sys::signal::kill(process, nix::sys::signal::Signal::SIGCONT).expect("resume child");
    queue.enforce_deadlines(Instant::now());
    nix::sys::signal::kill(process, nix::sys::signal::Signal::SIGSTOP).expect("stop child again");
    wait_until(|| SystemProcessStateProbe.state(pid as u32) == ProcessState::Stopped);
    queue.enforce_deadlines(Instant::now());
    assert_eq!(queue.active_process_handles().len(), 1);
    nix::sys::signal::kill(process, nix::sys::signal::Signal::SIGCONT).expect("resume cleanup");
    let _ = queue.shutdown();
}

#[test]
fn ac22_three_deadline_passes_start_one_termination_attempt() {
    let bed = Bed::new("ac22");
    let queue = queue(&bed, Duration::from_millis(10), true, None, None);
    let ready = bed.root.join("ready");
    let count = bed.root.join("count");
    queue.submit(request(
        "blocked",
        &command(&[
            "block-term-count",
            ready.to_str().expect("utf8"),
            count.to_str().expect("utf8"),
        ]),
        None,
        None,
    ));
    wait_for_ready(&ready);
    for _ in 0..3 {
        queue.enforce_deadlines(Instant::now() + Duration::from_secs(1));
    }
    wait_until(|| count.exists());
    assert_eq!(
        fs::read_to_string(&count).expect("count").lines().count(),
        1
    );
    let pid = i32::try_from(queue.active_process_handles()[0].pid()).expect("pid");
    let _ = nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(pid),
        nix::sys::signal::Signal::SIGKILL,
    );
}

#[test]
fn ac23_completed_during_probe_window_is_not_resurrected_as_timeout() {
    let bed = Bed::new("ac23");
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let release_rx = Mutex::new(release_rx);
    let hook: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
        entered_tx.send(()).expect("signal phase b");
        release_rx
            .lock()
            .expect("phase-c release lock")
            .recv()
            .expect("release phase c");
    });
    let queue = queue_with_probe(
        &bed,
        Duration::from_millis(10),
        Arc::new(SystemProcessStateProbe),
        Some(hook),
    );
    let ready = bed.root.join("ready");
    queue.submit(request(
        "fast",
        &command(&["ready-sleep", ready.to_str().expect("utf8"), "20"]),
        None,
        None,
    ));
    wait_for_ready(&ready);
    let enforcing = {
        let queue = queue.clone();
        std::thread::spawn(move || queue.enforce_deadlines(Instant::now() + Duration::from_secs(1)))
    };
    entered_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("phase b reached");
    wait_for_history(&queue, 1);
    release_tx.send(()).expect("release phase c");
    enforcing.join().expect("enforcement thread");
    assert_eq!(queue.history()[0].exit_status, "ok");
}

struct BlockingProbe {
    entered: Mutex<Option<mpsc::Sender<()>>>,
    release: Mutex<mpsc::Receiver<()>>,
}

impl ProcessStateProbe for BlockingProbe {
    fn state(&self, _pid: u32) -> ProcessState {
        if let Some(sender) = self.entered.lock().expect("entered lock").take() {
            sender.send(()).expect("probe entered");
        }
        self.release
            .lock()
            .expect("release lock")
            .recv()
            .expect("release probe");
        ProcessState::Other
    }
}

#[test]
fn ac24_probe_runs_without_holding_queue_lock() {
    let bed = Bed::new("ac24");
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let probe = Arc::new(BlockingProbe {
        entered: Mutex::new(Some(entered_tx)),
        release: Mutex::new(release_rx),
    });
    let queue = queue_with_probe(&bed, Duration::from_secs(5), probe, None);
    let ready = bed.root.join("ready");
    queue.submit(request(
        "first",
        &command(&["ready-sleep", ready.to_str().expect("utf8"), "500"]),
        None,
        None,
    ));
    wait_for_ready(&ready);
    let enforcing = {
        let queue = queue.clone();
        std::thread::spawn(move || queue.enforce_deadlines(Instant::now()))
    };
    entered_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("probe entered");
    let (submitted_tx, submitted_rx) = mpsc::channel();
    let submit_queue = queue.clone();
    std::thread::spawn(move || {
        let _ = submit_queue.submit(request("second", &command(&["lines"]), None, None));
        submitted_tx.send(()).expect("submitted");
    });
    submitted_rx
        .recv_timeout(Duration::from_millis(200))
        .expect("submit was not blocked by probe");
    release_tx.send(()).expect("release probe");
    enforcing.join().expect("enforcement");
}

#[test]
fn ac26_shutdown_returns_active_snapshot_count_and_blocks_advancement() {
    let bed = Bed::new("ac26");
    let sink = Arc::new(QueueCollector::default());
    let queue = queue(&bed, Duration::from_secs(5), true, Some(sink.clone()), None);
    let ready = bed.root.join("ready");
    queue.submit(request(
        "running",
        &command(&["ready-sleep", ready.to_str().expect("utf8"), "5000"]),
        None,
        None,
    ));
    wait_for_ready(&ready);
    queue.submit(request("queued", &command(&["lines"]), None, None));
    assert_eq!(queue.shutdown(), 1);
    wait_for_history(&queue, 1);
    assert!(!started_references(&sink).contains(&"queued".to_owned()));
    assert_eq!(queue.collect_queue_counts().values().sum::<usize>(), 1);
}

#[test]
fn ac27_status_truncates_while_enforcement_uses_fractional_duration() {
    let bed = Bed::new("ac27");
    let queue = queue(&bed, Duration::from_secs(4), true, None, None);
    let ready = bed.root.join("ready");
    queue.submit(request(
        "fractional",
        &command(&["ready-sleep", ready.to_str().expect("utf8"), "5000"]),
        None,
        None,
    ));
    wait_for_ready(&ready);
    let now = Instant::now();
    let status = queue.collect_task_status(now + Duration::from_millis(3_900));
    assert_eq!(status[0].cap_seconds, 4);
    assert!(status[0].slow, "3 truncated seconds is >= 75% of 4");
    let status = queue.collect_task_status(now + Duration::from_millis(4_100));
    assert!(!status[0].stuck, "4 truncated seconds is not > 4");
    queue.enforce_deadlines(now + Duration::from_millis(4_100));
    wait_for_history(&queue, 1);
    assert_eq!(queue.history()[0].exit_status, TIMEOUT_EXIT_STATUS);
}

#[test]
fn shutdown_waits_for_an_already_running_deadline_termination() {
    let bed = Bed::new("shutdown-in-flight");
    let queue = queue(&bed, Duration::from_millis(10), true, None, None);
    let ready = bed.root.join("ready");
    let count = bed.root.join("count");
    queue.submit(request(
        "blocked",
        &command(&[
            "block-term-count",
            ready.to_str().expect("utf8"),
            count.to_str().expect("utf8"),
        ]),
        None,
        None,
    ));
    wait_for_ready(&ready);
    queue.enforce_deadlines(Instant::now() + Duration::from_secs(1));
    wait_until(|| count.exists());
    assert_eq!(queue.shutdown(), 1);
    assert!(queue.active_process_handles().is_empty());
}

#[test]
fn phase_a_snapshot_does_not_wait_for_a_terminating_process_mutex() {
    let bed = Bed::new("phase-a-process-lock");
    let blocker = bed.root.join("blocker-child");
    let probe_child = bed.root.join("probe-child");
    std::os::unix::fs::symlink(FIXTURE, &blocker).expect("blocker fixture link");
    std::os::unix::fs::symlink(FIXTURE, &probe_child).expect("probe fixture link");
    let mut caps = BTreeMap::new();
    caps.insert("blocker-child".to_owned(), Duration::from_millis(10));
    caps.insert("probe-child".to_owned(), Duration::from_secs(5));
    let (probe_tx, probe_rx) = mpsc::channel();
    let queue = TaskQueue::new(TaskQueueOptions {
        journal_root: bed.root.clone(),
        cap_resolver: Arc::new(PartitionCaps(caps)),
        process_state_probe: Arc::new(NotifyingProbe {
            calls: AtomicUsize::new(0),
            sender: Mutex::new(Some(probe_tx)),
        }),
        queue_sink: None,
        process_sink: None,
        ready: true,
        before_deadline_commit: None,
    });
    let blocked_ready = bed.root.join("blocked-ready");
    let count = bed.root.join("count");
    let probe_ready = bed.root.join("probe-ready");
    queue.submit(request(
        "a-blocker",
        &[
            blocker.to_string_lossy().into_owned(),
            "block-term-count".into(),
            blocked_ready.to_string_lossy().into_owned(),
            count.to_string_lossy().into_owned(),
        ],
        None,
        None,
    ));
    queue.submit(request(
        "z-probe",
        &[
            probe_child.to_string_lossy().into_owned(),
            "ready-sleep".into(),
            probe_ready.to_string_lossy().into_owned(),
            "5000".into(),
        ],
        None,
        None,
    ));
    wait_for_ready(&blocked_ready);
    wait_for_ready(&probe_ready);
    queue.enforce_deadlines(Instant::now() + Duration::from_secs(1));
    wait_until(|| count.exists());
    let enforcing = {
        let queue = queue.clone();
        std::thread::spawn(move || queue.enforce_deadlines(Instant::now()))
    };
    // The budget is deliberately generous, and that costs no discrimination.
    // The blocker child blocks SIGTERM and never exits, so if Phase A did wait
    // on its process mutex the probe would never be reached AT ALL -- the two
    // outcomes here are "fires in milliseconds" and "never fires", not "fast"
    // and "slow". A tight budget therefore measures machine load rather than
    // the property: at 200ms this passed in isolation and failed under the rest
    // of this suite. A violated property still fails, it just takes the full
    // timeout to say so.
    probe_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("Phase A blocked on the terminating process mutex: unlocked probe never reached");
    enforcing.join().expect("enforcement thread");
    let _ = queue.shutdown();
}

#[test]
fn ac29_queue_counts_omit_empty_partitions_and_name_pending_only_when_present() {
    let bed = Bed::new("ac29");
    let queue = queue(&bed, Duration::from_secs(5), false, None, None);
    assert_eq!(queue.collect_queue_counts(), BTreeMap::new());
    queue.submit(request("pending", &command(&["lines"]), None, None));
    assert_eq!(queue.collect_queue_counts().get("pending"), Some(&1));
}

#[test]
fn ac30_history_is_bounded_and_retains_complete_record_shape() {
    let bed = Bed::new("ac30");
    let queue = queue(&bed, Duration::from_secs(5), true, None, None);
    for index in 0..101 {
        let mut cmd = command(&["lines"]);
        cmd.push(index.to_string());
        queue.submit(request(
            &format!("ref-{index}"),
            &cmd,
            None,
            Some("schedule"),
        ));
    }
    wait_until(|| {
        queue
            .history()
            .last()
            .is_some_and(|entry| entry.reference == "ref-100")
    });
    let history = queue.history();
    assert_eq!(history.len(), 100);
    assert_eq!(history[0].reference, "ref-1");
    assert!(!history[0].partition.as_str().is_empty());
    assert!(!history[0].command.is_empty());
    assert!(history[0].ended_at.duration_since(UNIX_EPOCH).is_ok());
    assert!(!history[0].exit_status.is_empty());
    assert_eq!(history[0].scheduler_name.as_deref(), Some("schedule"));
}

#[test]
fn ac31_real_completion_connects_history_process_output_and_queue_events() {
    let bed = Bed::new("ac31");
    let queue_sink = Arc::new(QueueCollector::default());
    let process_sink = Arc::new(ProcessCollector::default());
    let queue = queue(
        &bed,
        Duration::from_secs(5),
        true,
        Some(queue_sink.clone()),
        Some(process_sink.clone()),
    );
    queue.submit(request("complete", &command(&["lines"]), None, None));
    wait_for_history(&queue, 1);
    assert_eq!(queue.history()[0].exit_status, "ok");
    assert!(
        process_sink
            .0
            .lock()
            .expect("process events")
            .iter()
            .any(|event| matches!(event, ProcessEvent::Line { line, .. } if line == "stdout-line"))
    );
    assert!(stopped_references(&queue_sink).contains(&"complete".to_owned()));
}

#[test]
fn ac32_cap_termination_drains_continuous_output_without_partial_lines() {
    let bed = Bed::new("ac32");
    let process_sink = Arc::new(ProcessCollector::default());
    let queue = queue(
        &bed,
        Duration::from_millis(10),
        true,
        None,
        Some(process_sink.clone()),
    );
    let ready = bed.root.join("ready");
    queue.submit(request(
        "lines",
        &command(&["continuous-lines", ready.to_str().expect("utf8")]),
        None,
        None,
    ));
    wait_for_ready(&ready);
    wait_until(|| {
        process_sink
            .0
            .lock()
            .expect("process events")
            .iter()
            .any(|event| matches!(event, ProcessEvent::Line { .. }))
    });
    queue.enforce_deadlines(Instant::now() + Duration::from_secs(1));
    wait_for_history(&queue, 1);
    let lines = process_sink
        .0
        .lock()
        .expect("process events")
        .iter()
        .filter_map(|event| match event {
            ProcessEvent::Line { line, .. } => Some(line.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(!lines.is_empty());
    assert!(lines.iter().all(|line| {
        line.strip_prefix("line-")
            .is_some_and(|suffix| suffix.parse::<u64>().is_ok())
    }));
    assert_eq!(queue.history()[0].exit_status, TIMEOUT_EXIT_STATUS);
}

#[test]
fn ac33_dropping_one_clone_does_not_terminate_worker_another_clone_holds() {
    let bed = Bed::new("ac33");
    let queue = queue(&bed, Duration::from_secs(30), true, None, None);
    let clone = queue.clone();
    let ready = bed.root.join("ready");
    clone.submit(request(
        "held",
        &command(&["ready-sleep", ready.to_str().expect("utf8"), "5000"]),
        None,
        None,
    ));
    wait_for_ready(&ready);
    let pid = queue.active_process_handles()[0].pid();
    drop(clone);
    assert!(
        !process_is_gone(pid),
        "dropping one TaskQueue clone must not kill a worker another clone still holds"
    );
    assert_eq!(queue.shutdown(), 1);
    wait_until(|| process_is_gone(pid));
}
