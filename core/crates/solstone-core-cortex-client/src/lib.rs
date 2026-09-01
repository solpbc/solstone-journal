// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Shared native Cortex request dispatch and durable use-log waiting.

use std::collections::{BTreeMap, HashSet};
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value};
use solstone_core_callosum::{CallosumEnvelope, CallosumOneShotSender, CallosumSocketConnection};

const CLAIM_POLL_INTERVAL: Duration = Duration::from_millis(100);
const OUTCOME_POLL_INTERVAL: Duration = Duration::from_millis(500);
const SOCKET_TIMEOUT: Duration = Duration::from_secs(2);
const CLAIM_WINDOW_LADDERS: [&[Duration]; 2] = [
    &[
        Duration::from_secs(1),
        Duration::from_secs(1),
        Duration::from_secs(1),
    ],
    &[
        Duration::from_secs(1),
        Duration::from_secs(2),
        Duration::from_secs(4),
        Duration::from_secs(8),
        Duration::from_secs(15),
    ],
];

/// Explicit dispatch and wait timing for a Cortex caller.
///
/// The claim windows are deliberately constructors rather than a default: the
/// caller must choose either interactive or think behavior.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CortexRequestPolicy {
    claim_windows: Vec<Duration>,
    outcome_deadline: Option<Duration>,
}

impl CortexRequestPolicy {
    /// Fast-fail timing for an interactive request.
    #[must_use]
    pub fn interactive() -> Self {
        Self {
            claim_windows: CLAIM_WINDOW_LADDERS[0].to_vec(),
            outcome_deadline: Some(Duration::from_secs(600)),
        }
    }

    /// Patient timing for a think orchestration request.
    #[must_use]
    pub fn think() -> Self {
        Self {
            claim_windows: CLAIM_WINDOW_LADDERS[1].to_vec(),
            outcome_deadline: Some(Duration::from_secs(610)),
        }
    }

    #[must_use]
    pub fn claim_windows(&self) -> &[Duration] {
        &self.claim_windows
    }

    #[must_use]
    pub fn outcome_deadline(&self) -> Option<Duration> {
        self.outcome_deadline
    }

    /// A no-deadline policy still waits on the bounded 0.5-second poll interval;
    /// it never turns the wait loop into a zero-wait spin.
    #[must_use]
    pub fn without_outcome_deadline(mut self) -> Self {
        self.outcome_deadline = None;
        self
    }
}

/// Whether Cortex has durably created the requested use log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UseFileStatus {
    Completed,
    Running,
    NotFound,
}

/// End state of a durable Cortex use log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UseEndState {
    Finish,
    Error,
    Running,
    Unknown,
}

impl UseEndState {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Finish => "finish",
            Self::Error => "error",
            Self::Running => "running",
            Self::Unknown => "unknown",
        }
    }

    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Finish | Self::Error)
    }
}

/// A timeout identified per requested use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TimedOutUse {
    /// No durable use file appeared before the overall deadline.
    LostAtDeadline { use_id: String },
    /// A durable use file exists but it has no terminal event.
    GenuineTimeout { use_id: String },
}

impl TimedOutUse {
    #[must_use]
    pub fn use_id(&self) -> &str {
        match self {
            Self::LostAtDeadline { use_id } | Self::GenuineTimeout { use_id } => use_id,
        }
    }
}

/// Per-use terminal states and deadline outcomes for a batch wait.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WaitForUsesReport {
    pub completed: BTreeMap<String, UseCompletion>,
    pub timed_out: Vec<TimedOutUse>,
}

/// Terminal outcome plus the bool-only fields carried by a Cortex finish event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UseCompletion {
    pub end_state: UseEndState,
    pub finish_fields: FinishFields,
}

/// Finish-only fields the reference exposes to orchestration callers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FinishFields {
    /// Present only when the finish event holds a JSON boolean.  Python uses
    /// `isinstance(output_changed, bool)`, so truthy non-bools are discarded.
    pub output_changed: Option<bool>,
}

/// The dispatch failures distinguish a failed bus send from an unclaimed request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DispatchError {
    Unavailable,
    NotClaimed { use_id: String },
}

/// Read failures are intentionally surfaced instead of being silently truncated.
#[derive(Debug)]
pub enum CortexClientError {
    Dispatch(DispatchError),
    ReadUseLog(io::Error),
}

impl From<DispatchError> for CortexClientError {
    fn from(error: DispatchError) -> Self {
        Self::Dispatch(error)
    }
}

/// One flat Cortex request. `config` is merged after the required fields, as in
/// the reference client, so the think orchestrator can carry its full config.
#[derive(Clone, Debug, PartialEq)]
pub struct CortexRequest {
    pub prompt: String,
    pub name: String,
    pub config: Map<String, Value>,
}

impl CortexRequest {
    #[must_use]
    pub fn new(prompt: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            prompt: prompt.into(),
            name: name.into(),
            config: Map::new(),
        }
    }

    #[must_use]
    pub fn with_config(mut self, config: Map<String, Value>) -> Self {
        self.config = config;
        self
    }

    pub fn request_line(&self, ts: i64, use_id: &str) -> Result<String, serde_json::Error> {
        let mut extra = Map::new();
        extra.insert("use_id".to_owned(), Value::String(use_id.to_owned()));
        extra.insert("prompt".to_owned(), Value::String(self.prompt.clone()));
        extra.insert("name".to_owned(), Value::String(self.name.clone()));
        extra.extend(self.config.clone());
        let envelope = CallosumEnvelope {
            tract: "cortex".to_owned(),
            event: "request".to_owned(),
            ts: Some(ts),
            extra,
        };
        let mut line = serde_json::to_string(&envelope)?;
        line.push('\n');
        Ok(line)
    }
}

/// Thread-safe monotonic use-id allocation from an injected millisecond clock.
pub struct UseIdAllocator {
    clock: Box<dyn Fn() -> Option<i64> + Send + Sync>,
    previous: Mutex<Option<i64>>,
}

impl UseIdAllocator {
    #[must_use]
    pub fn new(clock: impl Fn() -> Option<i64> + Send + Sync + 'static) -> Self {
        Self {
            clock: Box::new(clock),
            previous: Mutex::new(None),
        }
    }

    #[must_use]
    pub fn system() -> Self {
        Self::new(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .ok()
                .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        })
    }

    pub fn next(&self) -> Result<i64, DispatchError> {
        let now = (self.clock)().ok_or(DispatchError::Unavailable)?;
        let mut previous = self.previous.lock().expect("use-id mutex is not poisoned");
        let issued = previous.map_or(now, |last| now.max(last.saturating_add(1)));
        *previous = Some(issued);
        Ok(issued)
    }
}

/// Native client shared by identity-health, engage, and the think orchestrator.
pub struct CortexRequestClient {
    journal: PathBuf,
    policy: CortexRequestPolicy,
    use_ids: UseIdAllocator,
}

impl CortexRequestClient {
    #[must_use]
    pub fn new(journal: impl AsRef<Path>, policy: CortexRequestPolicy) -> Self {
        Self::with_allocator(journal, policy, UseIdAllocator::system())
    }

    #[must_use]
    pub fn with_allocator(
        journal: impl AsRef<Path>,
        policy: CortexRequestPolicy,
        use_ids: UseIdAllocator,
    ) -> Self {
        Self {
            journal: journal.as_ref().to_path_buf(),
            policy,
            use_ids,
        }
    }

    pub async fn dispatch(&self, request: &CortexRequest) -> Result<String, DispatchError> {
        let ts = self.use_ids.next()?;
        self.dispatch_with_use_id(request, ts, ts.to_string()).await
    }

    pub async fn dispatch_with_use_id(
        &self,
        request: &CortexRequest,
        ts: i64,
        use_id: String,
    ) -> Result<String, DispatchError> {
        let talents_dir = self.journal.join("talents");
        fs::create_dir_all(&talents_dir).map_err(|_| DispatchError::Unavailable)?;
        let line = request
            .request_line(ts, &use_id)
            .map_err(|_| DispatchError::Unavailable)?;
        let sender = CallosumOneShotSender::new(
            self.journal.join("health").join("callosum.sock"),
            SOCKET_TIMEOUT,
        );

        // Engage sends at the top of all three iterations while steward sends
        // before its loop and on index > 0. Both therefore send three times
        // with the same poll interleave; this single form preserves that ladder.
        // The reference explains why: these windows exist to survive a lost
        // broadcast, not a slow one.
        for window in self.policy.claim_windows() {
            sender
                .send_line(&line)
                .map_err(|_| DispatchError::Unavailable)?;
            if wait_for_durable_claim(&talents_dir, &use_id, *window)
                .await
                .map_err(|_| DispatchError::Unavailable)?
            {
                return Ok(use_id);
            }
        }
        Err(DispatchError::NotClaimed { use_id })
    }

    /// Wait for all requested uses, retaining a distinct result for every id.
    pub async fn wait_for_uses(
        &self,
        use_ids: &[String],
    ) -> Result<WaitForUsesReport, CortexClientError> {
        self.wait_for_uses_with_deadline(use_ids, self.policy.outcome_deadline())
            .await
    }

    /// Wait with the caller's explicit overall deadline. `None` retains the
    /// bounded poll interval while removing only the overall deadline.
    pub async fn wait_for_uses_with_deadline(
        &self,
        use_ids: &[String],
        deadline: Option<Duration>,
    ) -> Result<WaitForUsesReport, CortexClientError> {
        wait_for_uses_with(&self.journal, use_ids, deadline, OUTCOME_POLL_INTERVAL).await
    }
}

/// Poll for a durable Cortex use file after one broadcast.
async fn wait_for_durable_claim(
    talents_dir: &Path,
    use_id: &str,
    window: Duration,
) -> Result<bool, io::Error> {
    if use_file_status(talents_dir, use_id)? != UseFileStatus::NotFound {
        return Ok(true);
    }
    let deadline = tokio::time::Instant::now() + window;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Ok(false);
        }
        tokio::time::sleep(CLAIM_POLL_INTERVAL.min(remaining)).await;
        if use_file_status(talents_dir, use_id)? != UseFileStatus::NotFound {
            return Ok(true);
        }
    }
}

/// Read the last recognized terminal event from a durable use log.
pub fn get_use_end_state(journal: &Path, use_id: &str) -> Result<UseEndState, io::Error> {
    let talents_dir = journal.join("talents");
    let (status, path) = find_use_file(&talents_dir, use_id)?;
    let Some(path) = path else {
        return Ok(UseEndState::Unknown);
    };
    let events = read_events_at(&path)?;
    if let Some(event) = events.into_iter().rev().find_map(|event| {
        match event.get("event").and_then(Value::as_str) {
            Some("finish") => Some(UseEndState::Finish),
            Some("error") => Some(UseEndState::Error),
            _ => None,
        }
    }) {
        return Ok(event);
    }
    Ok(match status {
        UseFileStatus::Running => UseEndState::Running,
        UseFileStatus::Completed | UseFileStatus::NotFound => UseEndState::Unknown,
    })
}

/// Read all valid event JSON values from a durable use log.
pub fn read_use_events(journal: &Path, use_id: &str) -> Result<Vec<Value>, io::Error> {
    let (_, path) = find_use_file(&journal.join("talents"), use_id)?;
    let path = path.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("Talent log not found: {use_id}"),
        )
    })?;
    read_events_at(&path)
}

/// Return the reference's completed/running/not-found durable-file status.
pub fn use_file_status(talents_dir: &Path, use_id: &str) -> Result<UseFileStatus, io::Error> {
    Ok(find_use_file(talents_dir, use_id)?.0)
}

fn find_use_file(
    talents_dir: &Path,
    use_id: &str,
) -> Result<(UseFileStatus, Option<PathBuf>), io::Error> {
    if !talents_dir.exists() {
        return Ok((UseFileStatus::NotFound, None));
    }
    for entry in fs::read_dir(talents_dir)? {
        let path = entry?.path();
        if path.is_dir() {
            let candidate = path.join(format!("{use_id}.jsonl"));
            if candidate.is_file() && candidate_matches(&candidate, use_id)? {
                return Ok((UseFileStatus::Completed, Some(candidate)));
            }
        }
    }
    for entry in fs::read_dir(talents_dir)? {
        let path = entry?.path();
        if path.is_dir() {
            let candidate = path.join(format!("{use_id}_active.jsonl"));
            if candidate.is_file() && candidate_matches(&candidate, use_id)? {
                return Ok((UseFileStatus::Running, Some(candidate)));
            }
        }
    }
    Ok((UseFileStatus::NotFound, None))
}

fn candidate_matches(path: &Path, use_id: &str) -> io::Result<bool> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    let Some(line) = BufReader::new(file).lines().next() else {
        return Ok(false);
    };
    let line = line?;
    let line = line.trim();
    if line.is_empty() {
        return Ok(false);
    }
    Ok(match serde_json::from_str::<Value>(line) {
        Ok(event) => event.get("use_id").and_then(Value::as_str) == Some(use_id),
        Err(_) => false,
    })
}

fn read_events_at(path: &Path) -> Result<Vec<Value>, io::Error> {
    let file = File::open(path)?;
    let mut events = Vec::new();
    for line in BufReader::new(file).lines() {
        let line = line?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(line) {
            Ok(event) => events.push(event),
            Err(_) => {
                // The reference skips JSONDecodeError and continues. Its
                // UnicodeDecodeError is a ValueError, not caught by
                // get_use_end_state's except OSError, so it propagates. The old
                // steward map_while(Result::ok) truncated at a bad line and old
                // engage discarded the whole file via unwrap_or_default; surface
                // IO/UTF-8 failures instead, per CLAUDE.md §8: fail loudly.
            }
        }
    }
    Ok(events)
}

async fn wait_for_uses_with(
    journal: &Path,
    use_ids: &[String],
    outcome_deadline: Option<Duration>,
    poll_interval: Duration,
) -> Result<WaitForUsesReport, CortexClientError> {
    let mut pending = use_ids.iter().cloned().collect::<HashSet<_>>();
    let mut report = WaitForUsesReport::default();
    let mut listener =
        CallosumSocketConnection::new(journal.join("health").join("callosum.sock"), Map::new());
    listener.start();

    recover_completed_from_disk(journal, &mut pending, &mut report.completed)?;
    let deadline = outcome_deadline.map(|duration| tokio::time::Instant::now() + duration);
    while !pending.is_empty() {
        let Some(wait_for) = wait_interval(deadline, poll_interval) else {
            break;
        };
        if let Ok(Some(message)) = tokio::time::timeout(wait_for, listener.next_message()).await
            && message.tract == "cortex"
            && let Some(use_id) = message.extra.get("use_id").and_then(Value::as_str)
            && pending.contains(use_id)
            && let Some(end_state) = match message.event.as_str() {
                "finish" => Some(UseEndState::Finish),
                "error" => Some(UseEndState::Error),
                _ => None,
            }
        {
            pending.remove(use_id);
            report.completed.insert(
                use_id.to_owned(),
                UseCompletion {
                    end_state,
                    finish_fields: finish_fields(&message.extra, end_state),
                },
            );
        }
        // Recovery is deliberately here, as well as before and after listener
        // lifetime, matching both old clients' three durable-broadcast positions.
        recover_completed_from_disk(journal, &mut pending, &mut report.completed)?;
    }
    listener.stop().await;
    recover_completed_from_disk(journal, &mut pending, &mut report.completed)?;
    for use_id in pending {
        let timed_out = match use_file_status(&journal.join("talents"), &use_id)
            .map_err(CortexClientError::ReadUseLog)?
        {
            UseFileStatus::NotFound => TimedOutUse::LostAtDeadline { use_id },
            UseFileStatus::Completed | UseFileStatus::Running => {
                TimedOutUse::GenuineTimeout { use_id }
            }
        };
        report.timed_out.push(timed_out);
    }
    report
        .timed_out
        .sort_by(|left, right| left.use_id().cmp(right.use_id()));
    Ok(report)
}

fn wait_interval(
    deadline: Option<tokio::time::Instant>,
    poll_interval: Duration,
) -> Option<Duration> {
    match deadline {
        Some(deadline) => {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            (!remaining.is_zero()).then(|| poll_interval.min(remaining))
        }
        // No overall deadline still blocks on this bounded interval, never spins.
        None => Some(poll_interval),
    }
}

fn recover_completed_from_disk(
    journal: &Path,
    pending: &mut HashSet<String>,
    completed: &mut BTreeMap<String, UseCompletion>,
) -> Result<(), CortexClientError> {
    for use_id in pending.clone() {
        let end_state =
            get_use_end_state(journal, &use_id).map_err(CortexClientError::ReadUseLog)?;
        if end_state.is_terminal() {
            let fields = if end_state == UseEndState::Finish {
                let events =
                    read_use_events(journal, &use_id).map_err(CortexClientError::ReadUseLog)?;
                events
                    .iter()
                    .rev()
                    .find_map(|event| {
                        (event.get("event").and_then(Value::as_str) == Some("finish")).then(|| {
                            event
                                .as_object()
                                .map(|event| finish_fields(event, end_state))
                                .unwrap_or_default()
                        })
                    })
                    .unwrap_or_default()
            } else {
                FinishFields::default()
            };
            completed.insert(
                use_id.clone(),
                UseCompletion {
                    end_state,
                    finish_fields: fields,
                },
            );
            pending.remove(&use_id);
        }
    }
    Ok(())
}

fn finish_fields(event: &Map<String, Value>, end_state: UseEndState) -> FinishFields {
    FinishFields {
        output_changed: (end_state == UseEndState::Finish)
            .then(|| event.get("output_changed").and_then(Value::as_bool))
            .flatten(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn active_use(journal: &Path, use_id: &str, body: &str) {
        let path = journal
            .join("talents/test")
            .join(format!("{use_id}_active.jsonl"));
        fs::create_dir_all(path.parent().expect("parent")).expect("create talent dir");
        fs::write(path, body).expect("write use log");
    }

    #[test]
    fn policies_are_explicit_and_match_the_reference_ladders() {
        assert_eq!(
            CortexRequestPolicy::interactive().claim_windows(),
            [Duration::from_secs(1); 3]
        );
        assert_eq!(
            CortexRequestPolicy::interactive().outcome_deadline(),
            Some(Duration::from_secs(600))
        );
        assert_eq!(
            CortexRequestPolicy::think().claim_windows(),
            [1, 2, 4, 8, 15].map(Duration::from_secs)
        );
        assert_eq!(
            CortexRequestPolicy::think().outcome_deadline(),
            Some(Duration::from_secs(610))
        );
    }

    #[test]
    fn think_envelope_is_flat_and_preserves_the_orchestrator_config() {
        let mut config = Map::new();
        config.insert("day".to_owned(), Value::String("20260101".to_owned()));
        config.insert("facet".to_owned(), Value::String("work".to_owned()));
        config.insert("output".to_owned(), Value::String("md".to_owned()));
        config.insert("refresh".to_owned(), Value::Bool(true));
        config.insert("schedule".to_owned(), Value::String("daily".to_owned()));
        let request =
            CortexRequest::new("Run the daily task.", "daily_summary").with_config(config);
        let value: Value = serde_json::from_str(&request.request_line(42, "42").unwrap()).unwrap();
        assert_eq!(value["tract"], "cortex");
        assert_eq!(value["event"], "request");
        assert_eq!(value["ts"], 42);
        assert_eq!(value["use_id"], "42");
        assert_eq!(value["prompt"], "Run the daily task.");
        assert_eq!(value["name"], "daily_summary");
        assert_eq!(value["day"], "20260101");
        assert_eq!(value["facet"], "work");
        assert_eq!(value["output"], "md");
        assert_eq!(value["refresh"], true);
        assert_eq!(value["schedule"], "daily");
    }

    #[test]
    fn concurrent_allocator_bumps_same_millisecond_ids() {
        const WORKERS: usize = 32;
        let allocator = Arc::new(UseIdAllocator::new(|| Some(42)));
        let start = Arc::new(std::sync::Barrier::new(WORKERS));
        let mut workers = Vec::new();
        for _ in 0..WORKERS {
            let allocator = Arc::clone(&allocator);
            let start = Arc::clone(&start);
            workers.push(std::thread::spawn(move || {
                start.wait();
                allocator.next().unwrap()
            }));
        }
        let mut ids: Vec<_> = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect();
        ids.sort_unstable();
        assert_eq!(
            ids,
            (42..42 + i64::try_from(WORKERS).unwrap()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn malformed_json_does_not_hide_a_later_terminal_event() {
        let journal = tempfile::tempdir().unwrap();
        active_use(
            journal.path(),
            "finished",
            "{\"use_id\":\"finished\"}\n{bad json}\n{\"event\":\"finish\"}\n",
        );
        assert_eq!(
            get_use_end_state(journal.path(), "finished").unwrap(),
            UseEndState::Finish
        );
    }

    #[test]
    fn non_utf8_use_log_surfaces_an_error_instead_of_truncating_or_discarding() {
        let journal = tempfile::tempdir().unwrap();
        let path = journal.path().join("talents/test/non_utf8_active.jsonl");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            b"{\"event\":\"finish\",\"use_id\":\"non_utf8\"}\n\xff",
        )
        .unwrap();
        let error = get_use_end_state(journal.path(), "non_utf8").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn no_deadline_still_uses_the_bounded_poll_interval() {
        assert_eq!(
            wait_interval(None, OUTCOME_POLL_INTERVAL),
            Some(Duration::from_millis(500))
        );
        assert_eq!(
            CortexRequestPolicy::think()
                .without_outcome_deadline()
                .outcome_deadline(),
            None
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn batch_wait_reports_terminal_and_timeout_per_use() {
        let journal = tempfile::tempdir().unwrap();
        active_use(
            journal.path(),
            "finish",
            "{\"use_id\":\"finish\"}\n{bad}\n{\"event\":\"finish\"}\n",
        );
        active_use(
            journal.path(),
            "pending",
            "{\"event\":\"request\",\"use_id\":\"pending\"}\n",
        );
        let report = wait_for_uses_with(
            journal.path(),
            &["finish".to_owned(), "pending".to_owned()],
            Some(Duration::ZERO),
            Duration::from_millis(1),
        )
        .await
        .unwrap();
        assert_eq!(
            report
                .completed
                .get("finish")
                .map(|completion| completion.end_state),
            Some(UseEndState::Finish)
        );
        assert_eq!(
            report.timed_out,
            vec![TimedOutUse::GenuineTimeout {
                use_id: "pending".to_owned()
            }]
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn finish_fields_preserve_only_boolean_output_changed() {
        let journal = tempfile::tempdir().unwrap();
        active_use(
            journal.path(),
            "changed",
            r#"{"event":"finish","output_changed":true,"use_id":"changed"}"#,
        );
        active_use(
            journal.path(),
            "truthy",
            r#"{"event":"finish","output_changed":"yes","use_id":"truthy"}"#,
        );
        let report = wait_for_uses_with(
            journal.path(),
            &["changed".to_owned(), "truthy".to_owned()],
            Some(Duration::ZERO),
            Duration::from_millis(1),
        )
        .await
        .unwrap();
        assert_eq!(
            report.completed["changed"].finish_fields.output_changed,
            Some(true)
        );
        assert_eq!(
            report.completed["truthy"].finish_fields.output_changed,
            None
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn deadline_without_a_use_file_is_lost_at_deadline() {
        let journal = tempfile::tempdir().unwrap();
        let lost = wait_for_uses_with(
            journal.path(),
            &["lost".to_owned()],
            Some(Duration::ZERO),
            Duration::from_millis(1),
        )
        .await
        .unwrap();
        assert_eq!(
            lost.timed_out,
            vec![TimedOutUse::LostAtDeadline {
                use_id: "lost".to_owned()
            }]
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn deadline_with_a_running_use_file_is_a_genuine_timeout() {
        let journal = tempfile::tempdir().unwrap();
        active_use(
            journal.path(),
            "running",
            "{\"event\":\"request\",\"use_id\":\"running\"}\n",
        );
        let running = wait_for_uses_with(
            journal.path(),
            &["running".to_owned()],
            Some(Duration::ZERO),
            Duration::from_millis(1),
        )
        .await
        .unwrap();
        assert_eq!(
            running.timed_out,
            vec![TimedOutUse::GenuineTimeout {
                use_id: "running".to_owned()
            }]
        );
    }

    #[test]
    fn use_file_status_distinguishes_completed_running_and_missing() {
        let journal = tempfile::tempdir().unwrap();
        let talents = journal.path().join("talents/test");
        fs::create_dir_all(&talents).unwrap();
        fs::write(talents.join("complete.jsonl"), r#"{"use_id":"complete"}"#).unwrap();
        fs::write(
            talents.join("running_active.jsonl"),
            r#"{"use_id":"running"}"#,
        )
        .unwrap();
        assert_eq!(
            use_file_status(&journal.path().join("talents"), "complete").unwrap(),
            UseFileStatus::Completed
        );
        assert_eq!(
            use_file_status(&journal.path().join("talents"), "running").unwrap(),
            UseFileStatus::Running
        );
        assert_eq!(
            use_file_status(&journal.path().join("talents"), "missing").unwrap(),
            UseFileStatus::NotFound
        );
    }

    #[test]
    fn use_file_status_rejects_completed_filename_match_with_wrong_content() {
        let journal = tempfile::tempdir().unwrap();
        let talents = journal.path().join("talents/test");
        fs::create_dir_all(&talents).unwrap();
        fs::write(
            talents.join("wrong_completed.jsonl"),
            r#"{"use_id":"other"}"#,
        )
        .unwrap();
        assert_eq!(
            use_file_status(&journal.path().join("talents"), "wrong_completed").unwrap(),
            UseFileStatus::NotFound
        );
    }

    #[test]
    fn use_file_status_rejects_active_filename_match_with_wrong_content() {
        let journal = tempfile::tempdir().unwrap();
        let talents = journal.path().join("talents/test");
        fs::create_dir_all(&talents).unwrap();
        fs::write(
            talents.join("wrong_running_active.jsonl"),
            r#"{"use_id":"other"}"#,
        )
        .unwrap();
        assert_eq!(
            use_file_status(&journal.path().join("talents"), "wrong_running").unwrap(),
            UseFileStatus::NotFound
        );
    }

    #[test]
    fn candidate_matches_treats_vanished_file_as_non_match() {
        let journal = tempfile::tempdir().unwrap();
        let path = journal.path().join("ghost.jsonl");
        fs::write(&path, r#"{"use_id":"ghost"}"#).unwrap();
        fs::remove_file(&path).unwrap();
        assert!(!candidate_matches(&path, "ghost").unwrap());
    }

    #[test]
    fn candidate_matches_rejects_malformed_json() {
        let journal = tempfile::tempdir().unwrap();
        let path = journal.path().join("bad.jsonl");
        fs::write(&path, "not-json\n").unwrap();
        assert!(!candidate_matches(&path, "bad").unwrap());
    }

    #[test]
    fn candidate_matches_rejects_missing_use_id_field() {
        let journal = tempfile::tempdir().unwrap();
        let path = journal.path().join("noid.jsonl");
        fs::write(&path, r#"{"event":"request"}"#).unwrap();
        assert!(!candidate_matches(&path, "noid").unwrap());
    }
}
