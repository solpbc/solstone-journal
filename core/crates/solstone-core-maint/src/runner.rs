// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::env;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use uuid::Uuid;

use crate::attempt_log::{
    AttemptExec, AttemptExit, AttemptLine, MaintAttemptEvent, append_attempt_event,
    open_attempt_log,
};
use crate::registry::{MaintTask, tasks};
use crate::state::{MaintTaskStatus, read_states};

const STALL_WARN_INTERVAL: Duration = Duration::from_secs(30);
const STALL_HARD_CAP: Duration = Duration::from_secs(120);
const TERMINATE_WAIT: Duration = Duration::from_secs(5);
const KILL_WAIT: Duration = Duration::from_secs(5);
const SIGKILL_EXIT_CODE: i32 = -9;

/// Request passed to an injected worker process launcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerRequest {
    pub journal: PathBuf,
    pub task: String,
}

/// Platform seams keep timeout tests independent from real clocks/processes.
pub trait RunnerPlatform {
    fn now_monotonic(&self) -> Duration;
    fn now_epoch_ms(&self) -> i64;
    fn spawn_worker(&self, request: &WorkerRequest) -> Result<Box<dyn WorkerChild>, RunnerError>;

    /// The argv persisted in the `exec` row before spawning the worker.
    fn worker_command(&self, request: &WorkerRequest) -> Vec<String>;
}

pub trait WorkerChild {
    fn recv_line_until(&mut self, deadline: Duration) -> Result<Option<String>, RunnerError>;
    fn try_wait(&mut self) -> Result<Option<i32>, RunnerError>;
    fn terminate(&mut self) -> Result<(), RunnerError>;
    fn wait_until(&mut self, deadline: Duration) -> Result<Option<i32>, RunnerError>;
    fn kill(&mut self) -> Result<(), RunnerError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunnerError {
    message: String,
}

impl RunnerError {
    fn from_io(error: io::Error) -> Self {
        Self {
            message: error.to_string(),
        }
    }
}

impl std::fmt::Display for RunnerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.message.fmt(formatter)
    }
}

impl std::error::Error for RunnerError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskOutcome {
    pub qualified_name: String,
    pub success: bool,
    pub exit_code: i32,
    pub state_file: Option<PathBuf>,
    pub warnings: Vec<String>,
    pub stalled: bool,
}

/// Production process/clock implementation.  The default executable is the
/// current binary; tests can direct the same plumbing at a tiny worker helper.
pub struct ProductionRunnerPlatform {
    executable: PathBuf,
    started: Instant,
}

impl ProductionRunnerPlatform {
    pub fn new() -> Result<Self, RunnerError> {
        Ok(Self::with_executable(
            env::current_exe().map_err(RunnerError::from_io)?,
        ))
    }

    pub fn with_executable(executable: PathBuf) -> Self {
        Self {
            executable,
            started: Instant::now(),
        }
    }
}

impl RunnerPlatform for ProductionRunnerPlatform {
    fn now_monotonic(&self) -> Duration {
        self.started.elapsed()
    }

    fn now_epoch_ms(&self) -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
            .unwrap_or(0)
    }

    fn spawn_worker(&self, request: &WorkerRequest) -> Result<Box<dyn WorkerChild>, RunnerError> {
        let mut child = Command::new(&self.executable)
            .args(["__maint-worker", "--one-task", "--task", &request.task])
            .env("SOLSTONE_JOURNAL", &request.journal)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(RunnerError::from_io)?;
        let (sender, receiver) = mpsc::channel();
        if let Some(stdout) = child.stdout.take() {
            spawn_reader(stdout, sender.clone());
        }
        if let Some(stderr) = child.stderr.take() {
            spawn_reader(stderr, sender.clone());
        }
        drop(sender);
        Ok(Box::new(ProductionWorkerChild {
            child,
            lines: receiver,
            started: self.started,
        }))
    }

    fn worker_command(&self, request: &WorkerRequest) -> Vec<String> {
        vec![
            self.executable.display().to_string(),
            "__maint-worker".to_owned(),
            "--one-task".to_owned(),
            "--task".to_owned(),
            request.task.clone(),
        ]
    }
}

fn spawn_reader<R>(reader: R, sender: mpsc::Sender<String>)
where
    R: io::Read + Send + 'static,
{
    thread::spawn(move || {
        let reader = io::BufReader::new(reader);
        for line in io::BufRead::lines(reader) {
            match line {
                Ok(line) => {
                    if sender.send(line).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
}

struct ProductionWorkerChild {
    child: std::process::Child,
    lines: Receiver<String>,
    started: Instant,
}

impl WorkerChild for ProductionWorkerChild {
    fn recv_line_until(&mut self, deadline: Duration) -> Result<Option<String>, RunnerError> {
        let remaining = deadline.saturating_sub(self.started.elapsed());
        match self.lines.recv_timeout(remaining) {
            Ok(line) => Ok(Some(line)),
            Err(mpsc::RecvTimeoutError::Timeout) => Ok(None),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                // Both pipe readers may finish before the worker process exits.
                // Keep the parent responsive without spinning until the next
                // process-status or stall check.
                thread::sleep(remaining.min(Duration::from_millis(10)));
                Ok(None)
            }
        }
    }

    fn try_wait(&mut self) -> Result<Option<i32>, RunnerError> {
        self.child
            .try_wait()
            .map(|status| status.map(exit_code))
            .map_err(RunnerError::from_io)
    }

    fn terminate(&mut self) -> Result<(), RunnerError> {
        #[cfg(unix)]
        {
            use nix::sys::signal::{Signal, kill};
            use nix::unistd::Pid;

            kill(Pid::from_raw(self.child.id() as i32), Signal::SIGTERM).map_err(|error| {
                RunnerError {
                    message: error.to_string(),
                }
            })
        }
        #[cfg(not(unix))]
        {
            self.child.kill().map_err(RunnerError::from_io)
        }
    }

    fn wait_until(&mut self, deadline: Duration) -> Result<Option<i32>, RunnerError> {
        loop {
            if let Some(code) = self.try_wait()? {
                return Ok(Some(code));
            }
            if self.started.elapsed() >= deadline {
                return Ok(None);
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn kill(&mut self) -> Result<(), RunnerError> {
        self.child.kill().map_err(RunnerError::from_io)
    }
}

fn exit_code(status: std::process::ExitStatus) -> i32 {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        status
            .code()
            .unwrap_or_else(|| -status.signal().unwrap_or(9))
    }
    #[cfg(not(unix))]
    {
        status.code().unwrap_or(SIGKILL_EXIT_CODE)
    }
}

pub fn run_task_with(
    platform: &dyn RunnerPlatform,
    task: &MaintTask,
    journal: &Path,
) -> TaskOutcome {
    let qualified_name = task.qualified_name();
    let request = WorkerRequest {
        journal: journal.to_path_buf(),
        task: qualified_name.clone(),
    };
    let attempt_id = Uuid::new_v4().simple().to_string();
    let mut writer = match open_attempt_log(journal, *task, attempt_id.clone()) {
        Ok(writer) => writer,
        Err(error) => return io_outcome(&qualified_name, None, error),
    };
    let state_file = Some(writer.path.clone());
    let command = platform.worker_command(&request);
    if let Err(error) = append_attempt_event(
        &mut writer,
        &MaintAttemptEvent::Exec(AttemptExec {
            attempt_id: attempt_id.clone(),
            ts: platform.now_epoch_ms(),
            app: task.app.to_owned(),
            task: task.name.to_owned(),
            cmd: command,
        }),
    ) {
        return io_outcome(&qualified_name, state_file, error);
    }
    let mut child = match platform.spawn_worker(&request) {
        Ok(child) => child,
        Err(error) => {
            write_exit(
                &mut writer,
                &attempt_id,
                platform.now_epoch_ms(),
                -1,
                None,
                Some(error.to_string()),
            );
            return failed_outcome(qualified_name, state_file, error.to_string());
        }
    };

    let started = platform.now_monotonic();
    let mut last_output = started;
    let mut next_warning = started + STALL_WARN_INTERVAL;
    let mut warnings = Vec::new();
    loop {
        match child.try_wait() {
            Ok(Some(exit_code)) => {
                return finish(
                    platform,
                    &mut writer,
                    &attempt_id,
                    qualified_name,
                    state_file,
                    started,
                    exit_code,
                    warnings,
                    false,
                );
            }
            Ok(None) => {}
            Err(error) => {
                write_exit(
                    &mut writer,
                    &attempt_id,
                    platform.now_epoch_ms(),
                    -1,
                    None,
                    Some(error.to_string()),
                );
                return failed_outcome(qualified_name, state_file, error.to_string());
            }
        }
        let now = platform.now_monotonic();
        let hard_deadline = last_output + STALL_HARD_CAP;
        if now >= hard_deadline {
            eprintln!("Maint task stalled past hard cap: {qualified_name}");
            let exit_code =
                terminate_with_grace(platform, child.as_mut(), &qualified_name, &mut warnings);
            return finish(
                platform,
                &mut writer,
                &attempt_id,
                qualified_name,
                state_file,
                started,
                exit_code,
                warnings,
                true,
            );
        }
        if now >= next_warning {
            let idle = now.saturating_sub(last_output).as_secs_f64();
            let warning =
                format!("Maint task stalled: {qualified_name} (no output for {idle:.1}s)");
            eprintln!("{warning}");
            warnings.push(warning);
            next_warning += STALL_WARN_INTERVAL;
            continue;
        }
        let deadline = std::cmp::min(next_warning, hard_deadline);
        match child.recv_line_until(deadline) {
            Ok(Some(line)) => {
                last_output = platform.now_monotonic();
                next_warning = last_output + STALL_WARN_INTERVAL;
                if append_attempt_event(
                    &mut writer,
                    &MaintAttemptEvent::Line(AttemptLine {
                        attempt_id: attempt_id.clone(),
                        ts: platform.now_epoch_ms(),
                        line: line.clone(),
                    }),
                )
                .is_err()
                {
                    return failed_outcome(
                        qualified_name,
                        state_file,
                        "maint state write failed".to_owned(),
                    );
                }
                println!("  {line}");
            }
            Ok(None) => {}
            Err(error) => {
                write_exit(
                    &mut writer,
                    &attempt_id,
                    platform.now_epoch_ms(),
                    -1,
                    None,
                    Some(error.to_string()),
                );
                return failed_outcome(qualified_name, state_file, error.to_string());
            }
        }
    }
}

pub fn run_pending_tasks(platform: &dyn RunnerPlatform, journal: &Path) -> Vec<TaskOutcome> {
    let states = read_states(journal);
    tasks()
        .iter()
        .filter(|task| {
            states
                .iter()
                .find(|state| state.app == task.app && state.task == task.name)
                .is_some_and(|state| {
                    state.status == MaintTaskStatus::Pending
                        || (state.status == MaintTaskStatus::Failed && task.retry_on_next_start)
                })
        })
        .map(|task| run_task_with(platform, task, journal))
        .collect()
}

pub fn run_forced_task(
    platform: &dyn RunnerPlatform,
    task: &MaintTask,
    journal: &Path,
) -> TaskOutcome {
    run_task_with(platform, task, journal)
}

fn terminate_with_grace(
    platform: &dyn RunnerPlatform,
    child: &mut dyn WorkerChild,
    qualified_name: &str,
    warnings: &mut Vec<String>,
) -> i32 {
    if child.terminate().is_ok()
        && let Ok(Some(exit_code)) = child.wait_until(platform.now_monotonic() + TERMINATE_WAIT)
    {
        return exit_code;
    }
    if child.kill().is_ok()
        && let Ok(Some(exit_code)) = child.wait_until(platform.now_monotonic() + KILL_WAIT)
    {
        return exit_code;
    }
    let warning = format!("Maint task unkillable: {qualified_name}");
    eprintln!("{warning}");
    warnings.push(warning);
    SIGKILL_EXIT_CODE
}

#[allow(clippy::too_many_arguments)]
fn finish(
    platform: &dyn RunnerPlatform,
    writer: &mut crate::attempt_log::AttemptLogWriter,
    attempt_id: &str,
    qualified_name: String,
    state_file: Option<PathBuf>,
    started: Duration,
    exit_code: i32,
    warnings: Vec<String>,
    stalled: bool,
) -> TaskOutcome {
    let duration_ms = i64::try_from(platform.now_monotonic().saturating_sub(started).as_millis())
        .unwrap_or(i64::MAX);
    write_exit(
        writer,
        attempt_id,
        platform.now_epoch_ms(),
        exit_code,
        Some(duration_ms),
        stalled.then_some("stalled".to_owned()),
    );
    TaskOutcome {
        qualified_name,
        success: exit_code == 0,
        exit_code,
        state_file,
        warnings,
        stalled,
    }
}

fn write_exit(
    writer: &mut crate::attempt_log::AttemptLogWriter,
    attempt_id: &str,
    ts: i64,
    exit_code: i32,
    duration_ms: Option<i64>,
    error: Option<String>,
) {
    let _ = append_attempt_event(
        writer,
        &MaintAttemptEvent::Exit(AttemptExit {
            attempt_id: attempt_id.to_owned(),
            ts,
            exit_code,
            duration_ms,
            error,
        }),
    );
}

fn io_outcome(qualified_name: &str, state_file: Option<PathBuf>, error: io::Error) -> TaskOutcome {
    failed_outcome(qualified_name.to_owned(), state_file, error.to_string())
}

fn failed_outcome(
    qualified_name: String,
    state_file: Option<PathBuf>,
    warning: String,
) -> TaskOutcome {
    TaskOutcome {
        qualified_name,
        success: false,
        exit_code: -1,
        state_file,
        warnings: vec![warning],
        stalled: false,
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::collections::VecDeque;
    use std::fs;
    use std::rc::Rc;

    use serde_json::Value;
    use tempfile::tempdir;

    use super::*;

    #[derive(Clone)]
    struct FakePlatform {
        now: Rc<Cell<Duration>>,
        plans: Rc<RefCell<VecDeque<FakePlan>>>,
        actions: Rc<RefCell<Vec<String>>>,
    }

    struct FakePlan {
        lines: VecDeque<(Duration, String)>,
        exit_at: Option<(Duration, i32)>,
        exits_on_kill: bool,
    }

    impl FakePlatform {
        fn with_plan(plan: FakePlan) -> Self {
            Self {
                now: Rc::new(Cell::new(Duration::ZERO)),
                plans: Rc::new(RefCell::new(VecDeque::from([plan]))),
                actions: Rc::new(RefCell::new(Vec::new())),
            }
        }
    }

    impl RunnerPlatform for FakePlatform {
        fn now_monotonic(&self) -> Duration {
            self.now.get()
        }
        fn now_epoch_ms(&self) -> i64 {
            i64::try_from(self.now.get().as_millis()).expect("test timestamp")
        }
        fn spawn_worker(&self, _: &WorkerRequest) -> Result<Box<dyn WorkerChild>, RunnerError> {
            let plan = self.plans.borrow_mut().pop_front().expect("one plan");
            Ok(Box::new(FakeChild {
                now: Rc::clone(&self.now),
                actions: Rc::clone(&self.actions),
                lines: plan.lines,
                exit_at: plan.exit_at,
                exits_on_kill: plan.exits_on_kill,
            }))
        }
        fn worker_command(&self, request: &WorkerRequest) -> Vec<String> {
            vec![
                "fake-maint".to_owned(),
                "__maint-worker".to_owned(),
                "--one-task".to_owned(),
                "--task".to_owned(),
                request.task.clone(),
            ]
        }
    }

    struct FakeChild {
        now: Rc<Cell<Duration>>,
        actions: Rc<RefCell<Vec<String>>>,
        lines: VecDeque<(Duration, String)>,
        exit_at: Option<(Duration, i32)>,
        exits_on_kill: bool,
    }

    impl WorkerChild for FakeChild {
        fn recv_line_until(&mut self, deadline: Duration) -> Result<Option<String>, RunnerError> {
            if let Some((at, _)) = self.lines.front()
                && *at <= deadline
            {
                let (at, line) = self.lines.pop_front().expect("front exists");
                self.now.set(at);
                return Ok(Some(line));
            }
            self.now.set(deadline);
            Ok(None)
        }
        fn try_wait(&mut self) -> Result<Option<i32>, RunnerError> {
            Ok(self
                .exit_at
                .filter(|(at, _)| *at <= self.now.get())
                .map(|(_, code)| code))
        }
        fn terminate(&mut self) -> Result<(), RunnerError> {
            self.actions.borrow_mut().push("terminate".to_owned());
            Ok(())
        }
        fn wait_until(&mut self, deadline: Duration) -> Result<Option<i32>, RunnerError> {
            self.actions
                .borrow_mut()
                .push(format!("wait:{}", deadline.as_secs()));
            if let Some((at, code)) = self.exit_at
                && at <= deadline
            {
                self.now.set(at);
                return Ok(Some(code));
            }
            self.now.set(deadline);
            Ok(None)
        }
        fn kill(&mut self) -> Result<(), RunnerError> {
            self.actions.borrow_mut().push("kill".to_owned());
            if self.exits_on_kill {
                self.exit_at = Some((self.now.get(), SIGKILL_EXIT_CODE));
            }
            Ok(())
        }
    }

    fn plan(lines: &[(u64, &str)], exits_on_kill: bool) -> FakePlan {
        FakePlan {
            lines: lines
                .iter()
                .map(|(seconds, line)| (Duration::from_secs(*seconds), (*line).to_owned()))
                .collect(),
            exit_at: None,
            exits_on_kill,
        }
    }

    #[test]
    fn fake_clock_records_lines_and_normal_exit_rows() {
        let journal = tempdir().expect("journal");
        let mut fast = plan(&[(1, "one")], false);
        fast.exit_at = Some((Duration::from_secs(2), 1));
        let platform = FakePlatform::with_plan(fast);
        let outcome = run_task_with(&platform, &tasks()[0], journal.path());
        assert_eq!(outcome.exit_code, 1);
        assert!(!outcome.stalled);
        let rows = fs::read_to_string(outcome.state_file.expect("state file"))
            .expect("read state")
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).expect("json row"))
            .collect::<Vec<_>>();
        assert_eq!(
            rows.iter()
                .map(|row| row["event"].as_str())
                .collect::<Vec<_>>(),
            [Some("exec"), Some("line"), Some("exit")]
        );
        assert_eq!(rows[1]["line"], "one");
        assert_eq!(rows[0]["attempt_id"], rows[2]["attempt_id"]);
        assert!(rows[2]["duration_ms"].is_number());
    }

    #[test]
    fn fake_clock_warns_at_thirty_and_kills_after_hard_cap() {
        let journal = tempdir().expect("journal");
        let platform = FakePlatform::with_plan(plan(&[], true));
        let outcome = run_task_with(&platform, &tasks()[0], journal.path());
        assert!(outcome.stalled);
        assert_eq!(outcome.exit_code, SIGKILL_EXIT_CODE);
        assert_eq!(outcome.warnings.len(), 3);
        assert!(outcome.warnings[0].contains("30.0s"));
        assert_eq!(
            platform.actions.borrow().as_slice(),
            ["terminate", "wait:125", "kill", "wait:130"]
        );
        let rows = fs::read_to_string(outcome.state_file.expect("state file")).expect("read state");
        let exit = rows
            .lines()
            .last()
            .and_then(|line| serde_json::from_str::<Value>(line).ok())
            .expect("exit row");
        assert_eq!(exit["error"], "stalled");
        assert_eq!(exit["exit_code"], SIGKILL_EXIT_CODE);
    }

    #[test]
    fn fake_clock_uses_unkillable_fallback_after_both_five_second_waits() {
        let journal = tempdir().expect("journal");
        let platform = FakePlatform::with_plan(plan(&[], false));
        let outcome = run_task_with(&platform, &tasks()[0], journal.path());
        assert_eq!(outcome.exit_code, -9);
        assert!(
            outcome
                .warnings
                .last()
                .is_some_and(|warning| warning.contains("unkillable"))
        );
        assert_eq!(platform.now.get(), Duration::from_secs(130));
        assert_eq!(
            platform.actions.borrow().as_slice(),
            ["terminate", "wait:125", "kill", "wait:130"]
        );
    }
}
