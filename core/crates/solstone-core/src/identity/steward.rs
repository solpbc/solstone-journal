// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native implementation of the steward-backed identity health refresh.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::{Local, NaiveDate};
use nix::errno::Errno;
use nix::fcntl::{Flock, FlockArg};
use serde_json::{Map, Value};
use solstone_core_callosum::{CallosumEnvelope, CallosumOneShotSender, CallosumSocketConnection};

use super::EXIT_FAILURE;

const CLAIM_WINDOWS: [Duration; 3] = [Duration::from_secs(1); 3];
const CLAIM_POLL_INTERVAL: Duration = Duration::from_millis(100);
const OUTCOME_TIMEOUT: Duration = Duration::from_secs(600);
const OUTCOME_POLL_INTERVAL: Duration = Duration::from_millis(500);
const SOCKET_TIMEOUT: Duration = Duration::from_secs(2);
const STATUS_HEADING: &str = "## Status";
const GENERATED_AT_PREFIX: &str = "<!-- generated_at: ";
const GENERATED_AT_SUFFIX: &str = " -->";

#[derive(Clone)]
struct RefreshOptions {
    today: String,
    claim_windows: Vec<Duration>,
    claim_poll_interval: Duration,
    outcome_timeout: Duration,
    outcome_poll_interval: Duration,
}

impl Default for RefreshOptions {
    fn default() -> Self {
        Self {
            today: Local::now().format("%Y%m%d").to_string(),
            claim_windows: CLAIM_WINDOWS.to_vec(),
            claim_poll_interval: CLAIM_POLL_INTERVAL,
            outcome_timeout: OUTCOME_TIMEOUT,
            outcome_poll_interval: OUTCOME_POLL_INTERVAL,
        }
    }
}

enum LockAcquire {
    Held(StewardLock),
    Contended,
    Error(String),
}

struct StewardLock {
    _guard: Flock<File>,
}

pub(super) fn refresh(journal: &Path, health_path: &Path) -> ExitCode {
    refresh_with_options(journal, health_path, RefreshOptions::default())
}

fn refresh_with_options(journal: &Path, health_path: &Path, options: RefreshOptions) -> ExitCode {
    let _lock = match acquire_lock(journal) {
        LockAcquire::Held(lock) => lock,
        LockAcquire::Contended => {
            eprintln!("Error: steward already in flight.");
            return ExitCode::from(EXIT_FAILURE);
        }
        LockAcquire::Error(error) => {
            eprintln!("Error: {error}");
            return ExitCode::from(EXIT_FAILURE);
        }
    };

    let before_body = match read_optional(health_path) {
        Ok(Some(body)) => body,
        Ok(None) => String::new(),
        Err(error) => return print_io_error(health_path, error),
    };
    if let Some(generated_at) = is_already_fresh(&before_body, journal, &options.today) {
        println!("already fresh (generated_at: {generated_at})");
        return ExitCode::SUCCESS;
    }

    let before_mtime_ns = match modified_ns(health_path) {
        Ok(value) => value,
        Err(error) => return print_io_error(health_path, error),
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("Error: steward runtime unavailable: {error}");
            return ExitCode::from(EXIT_FAILURE);
        }
    };
    let outcome = match runtime.block_on(request_and_wait(journal, &options)) {
        Ok(outcome) => outcome,
        Err(RequestError::Unavailable | RequestError::NotClaimed) => {
            eprintln!("Error: failed to send steward request to cortex.");
            return ExitCode::from(EXIT_FAILURE);
        }
    };
    if outcome.timed_out {
        eprintln!("Error: steward request timed out.");
        return ExitCode::from(EXIT_FAILURE);
    }
    if outcome.end_state != "finish" {
        eprintln!("Error: steward request failed: {}.", outcome.end_state);
        return ExitCode::from(EXIT_FAILURE);
    }

    let (stamp, bytes) = match file_change_proof(health_path, before_mtime_ns) {
        Ok(proof) => proof,
        Err(FileProofError::NotUpdated) => {
            eprintln!("Error: identity/health.md was not updated.");
            return ExitCode::from(EXIT_FAILURE);
        }
        Err(FileProofError::Io(error)) => return print_io_error(health_path, error),
    };
    println!(
        "regenerated {} (generated_at: {stamp}, {bytes} bytes)",
        health_path.display()
    );
    ExitCode::SUCCESS
}

fn is_already_fresh(before_body: &str, journal: &Path, today: &str) -> Option<String> {
    let generated_at = generated_at_from_body(before_body)?;
    let generated_at_ms = generated_at_ms(&generated_at)?;
    let latest_ts = latest_daily_run_complete_ts(journal, today)?;
    (generated_at_ms >= latest_ts).then_some(generated_at)
}

enum FileProofError {
    NotUpdated,
    Io(std::io::Error),
}

fn file_change_proof(
    health_path: &Path,
    before_mtime_ns: Option<u128>,
) -> Result<(String, u64), FileProofError> {
    if !health_path.exists() {
        return Err(FileProofError::NotUpdated);
    }
    let after_mtime_ns = modified_ns(health_path).map_err(FileProofError::Io)?;
    let Some(after_mtime_ns) = after_mtime_ns else {
        return Err(FileProofError::NotUpdated);
    };
    if before_mtime_ns.is_some_and(|before| after_mtime_ns <= before) {
        return Err(FileProofError::NotUpdated);
    }
    let after_body = fs::read_to_string(health_path).map_err(FileProofError::Io)?;
    let Some(stamp) = generated_at_from_body(&after_body) else {
        return Err(FileProofError::NotUpdated);
    };
    let bytes = fs::metadata(health_path).map_err(FileProofError::Io)?.len();
    Ok((stamp, bytes))
}

fn acquire_lock(journal: &Path) -> LockAcquire {
    let path = journal.join("health").join(".steward.lock");
    if let Err(error) = fs::create_dir_all(path.parent().expect("lock has parent")) {
        return LockAcquire::Error(format!("{}: {error}", path.display()));
    }
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(false);
    #[cfg(unix)]
    options.mode(0o600);
    let file = match options.open(&path) {
        Ok(file) => file,
        Err(error) => return LockAcquire::Error(format!("{}: {error}", path.display())),
    };
    match Flock::lock(file, FlockArg::LockExclusiveNonblock) {
        Ok(guard) => LockAcquire::Held(StewardLock { _guard: guard }),
        Err((file, Errno::EACCES | Errno::EAGAIN)) => {
            drop(file);
            LockAcquire::Contended
        }
        Err((file, error)) => {
            drop(file);
            LockAcquire::Error(format!("{}: {error}", path.display()))
        }
    }
}

enum RequestError {
    Unavailable,
    NotClaimed,
}

struct Outcome {
    end_state: String,
    timed_out: bool,
}

async fn request_and_wait(
    journal: &Path,
    options: &RefreshOptions,
) -> Result<Outcome, RequestError> {
    let use_id = cortex_request(journal, options).await?;
    Ok(wait_for_uses(journal, &[use_id], options).await)
}

async fn cortex_request(journal: &Path, options: &RefreshOptions) -> Result<String, RequestError> {
    let ts = current_ms().ok_or(RequestError::Unavailable)?;
    cortex_request_for_use_id(journal, options, ts, ts.to_string()).await
}

async fn cortex_request_for_use_id(
    journal: &Path,
    options: &RefreshOptions,
    ts: i64,
    use_id: String,
) -> Result<String, RequestError> {
    let talents_dir = journal.join("talents");
    if fs::create_dir_all(&talents_dir).is_err() {
        return Err(RequestError::Unavailable);
    }
    let line = request_line(ts, &use_id, &options.today).map_err(|_| RequestError::Unavailable)?;
    let sender =
        CallosumOneShotSender::new(journal.join("health").join("callosum.sock"), SOCKET_TIMEOUT);

    if sender.send_line(&line).is_err() {
        return Err(RequestError::Unavailable);
    }
    for (index, window) in options.claim_windows.iter().enumerate() {
        if index > 0 && sender.send_line(&line).is_err() {
            return Err(RequestError::Unavailable);
        }
        if claimed(&talents_dir, &use_id) {
            return Ok(use_id);
        }
        let deadline = tokio::time::Instant::now() + *window;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            tokio::time::sleep(options.claim_poll_interval.min(remaining)).await;
            if claimed(&talents_dir, &use_id) {
                return Ok(use_id);
            }
        }
    }
    Err(RequestError::NotClaimed)
}

fn request_line(ts: i64, use_id: &str, today: &str) -> Result<String, serde_json::Error> {
    let mut extra = Map::new();
    extra.insert("use_id".to_owned(), Value::String(use_id.to_owned()));
    extra.insert("prompt".to_owned(), Value::String(String::new()));
    extra.insert("name".to_owned(), Value::String("steward".to_owned()));
    extra.insert("day".to_owned(), Value::String(today.to_owned()));
    extra.insert("output".to_owned(), Value::String("md".to_owned()));
    extra.insert("refresh".to_owned(), Value::Bool(true));
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

async fn wait_for_uses(journal: &Path, use_ids: &[String], options: &RefreshOptions) -> Outcome {
    let mut pending = use_ids.iter().cloned().collect::<HashSet<_>>();
    let mut completed = HashMap::new();
    let mut listener =
        CallosumSocketConnection::new(journal.join("health").join("callosum.sock"), Map::new());
    listener.start();

    recover_completed_from_disk(journal, &mut pending, &mut completed);
    if pending.is_empty() {
        listener.stop().await;
        return Outcome {
            end_state: completed
                .get(&use_ids[0])
                .cloned()
                .unwrap_or_else(|| "unknown".to_owned()),
            timed_out: false,
        };
    }

    let deadline = tokio::time::Instant::now() + options.outcome_timeout;
    while !pending.is_empty() {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        let wait_for = options.outcome_poll_interval.min(remaining);
        if let Ok(Some(message)) = tokio::time::timeout(wait_for, listener.next_message()).await
            && message.tract == "cortex"
            && matches!(message.event.as_str(), "finish" | "error")
            && let Some(use_id) = message.extra.get("use_id").and_then(Value::as_str)
            && pending.remove(use_id)
        {
            completed.insert(use_id.to_owned(), message.event);
        }
        recover_completed_from_disk(journal, &mut pending, &mut completed);
    }
    listener.stop().await;
    recover_completed_from_disk(journal, &mut pending, &mut completed);
    let use_id = &use_ids[0];
    Outcome {
        end_state: completed
            .get(use_id)
            .cloned()
            .unwrap_or_else(|| "unknown".to_owned()),
        timed_out: pending.contains(use_id),
    }
}

fn recover_completed_from_disk(
    journal: &Path,
    pending: &mut HashSet<String>,
    completed: &mut HashMap<String, String>,
) {
    for use_id in pending.clone() {
        if let Some(end_state) = use_end_state(journal, &use_id) {
            completed.insert(use_id.clone(), end_state);
            pending.remove(&use_id);
        }
    }
}

fn claimed(talents_dir: &Path, use_id: &str) -> bool {
    find_use_file(talents_dir, use_id).is_some()
}

fn find_use_file(talents_dir: &Path, use_id: &str) -> Option<PathBuf> {
    find_use_file_named(talents_dir, &format!("{use_id}.jsonl"))
        .or_else(|| find_use_file_named(talents_dir, &format!("{use_id}_active.jsonl")))
}

fn find_use_file_named(talents_dir: &Path, file_name: &str) -> Option<PathBuf> {
    fs::read_dir(talents_dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .map(|path| path.join(file_name))
        .find(|path| path.is_file())
}

fn use_end_state(journal: &Path, use_id: &str) -> Option<String> {
    let talents_dir = journal.join("talents");
    let path = find_use_file(&talents_dir, use_id)?;
    let file = File::open(path).ok()?;
    let mut events = Vec::new();
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(event) = serde_json::from_str::<Value>(line) {
            events.push(event);
        }
    }
    for event in events.into_iter().rev() {
        match event.get("event").and_then(Value::as_str) {
            Some("finish") => return Some("finish".to_owned()),
            Some("error") => return Some("error".to_owned()),
            _ => {}
        }
    }
    None
}

fn generated_at_from_body(body: &str) -> Option<String> {
    let mut headings = Vec::new();
    let mut sections = HashMap::<String, Vec<&str>>::new();
    let mut current = None::<String>;
    for line in body.lines() {
        if line.starts_with("## ") {
            let heading = line.to_owned();
            headings.push(heading.clone());
            sections.insert(heading.clone(), Vec::new());
            current = Some(heading);
        } else if let Some(heading) = &current {
            sections.entry(heading.clone()).or_default().push(line);
        }
    }
    if headings.first().map(String::as_str) != Some(STATUS_HEADING) {
        return None;
    }
    let line = sections.get(STATUS_HEADING)?.first()?;
    parse_generated_at(line).map(str::to_owned)
}

fn parse_generated_at(line: &str) -> Option<&str> {
    let stamp = line
        .strip_prefix(GENERATED_AT_PREFIX)?
        .strip_suffix(GENERATED_AT_SUFFIX)?;
    let bytes = stamp.as_bytes();
    if bytes.len() != 20
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
        || bytes[19] != b'Z'
        || [0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18]
            .into_iter()
            .any(|index| !bytes[index].is_ascii_digit())
    {
        return None;
    }
    Some(stamp)
}

fn generated_at_ms(stamp: &str) -> Option<i64> {
    chrono::NaiveDateTime::parse_from_str(stamp, "%Y-%m-%dT%H:%M:%SZ")
        .ok()
        .map(|date_time| date_time.and_utc().timestamp_millis())
}

fn latest_daily_run_complete_ts(journal: &Path, today: &str) -> Option<i64> {
    let previous = NaiveDate::parse_from_str(today, "%Y%m%d")
        .ok()?
        .pred_opt()?
        .format("%Y%m%d")
        .to_string();
    let mut latest = None;
    for day in [today, previous.as_str()] {
        let Ok(day_path) = solstone_core_journal_io::day_path(journal, Some(day), false) else {
            continue;
        };
        let health_dir = day_path.join("health");
        let Ok(entries) = fs::read_dir(&health_dir) else {
            continue;
        };
        let mut paths = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.is_file()
                    && path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.ends_with("_daily.jsonl"))
            })
            .collect::<Vec<_>>();
        paths.sort();
        for path in paths {
            let Ok(file) = File::open(path) else {
                continue;
            };
            for line in BufReader::new(file).lines().map_while(Result::ok) {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let Ok(Value::Object(row)) = serde_json::from_str::<Value>(line) else {
                    continue;
                };
                if row.get("event").and_then(Value::as_str) != Some("run.complete") {
                    continue;
                }
                // Daily writers emit numeric timestamps; strings are intentionally ignored.
                let Some(ts) = row.get("ts").and_then(Value::as_i64) else {
                    continue;
                };
                latest = Some(latest.map_or(ts, |current: i64| current.max(ts)));
            }
        }
    }
    latest
}

fn current_ms() -> Option<i64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
}

fn read_optional(path: &Path) -> Result<Option<String>, std::io::Error> {
    match fs::read_to_string(path) {
        Ok(body) => Ok(Some(body)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn modified_ns(path: &Path) -> Result<Option<u128>, std::io::Error> {
    match fs::metadata(path) {
        Ok(metadata) => Ok(Some(
            metadata
                .modified()?
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn print_io_error(path: &Path, error: std::io::Error) -> ExitCode {
    eprintln!("Error: {}: {error}", path.display());
    ExitCode::from(EXIT_FAILURE)
}

#[cfg(test)]
mod tests {
    use super::{
        FileProofError, LockAcquire, RefreshOptions, RequestError, acquire_lock, claimed,
        cortex_request, cortex_request_for_use_id, file_change_proof, find_use_file,
        generated_at_from_body, generated_at_ms, is_already_fresh, latest_daily_run_complete_ts,
        modified_ns, refresh_with_options, wait_for_uses,
    };
    use std::fs::{self, FileTimes};
    use std::os::unix::net::UnixListener as StdUnixListener;
    use std::path::Path;
    use std::process::ExitCode;
    use std::time::{Duration, UNIX_EPOCH};
    use tokio::io::AsyncBufReadExt;
    use tokio::net::UnixListener;

    const STAMP: &str = "2026-05-26T17:32:18Z";

    fn options() -> RefreshOptions {
        RefreshOptions {
            today: "20260526".to_owned(),
            claim_windows: vec![Duration::from_millis(25); 3],
            claim_poll_interval: Duration::from_millis(5),
            outcome_timeout: Duration::from_millis(25),
            outcome_poll_interval: Duration::from_millis(5),
        }
    }

    fn health_body(stamp: &str) -> String {
        format!(
            "## Status\n<!-- generated_at: {stamp} -->\nsol is well.\n\n## Needs your attention\n\n## Auto-repairs (last 7d)\n"
        )
    }

    fn active_use(journal: &Path, use_id: &str, body: &str) {
        let path = journal
            .join("talents/steward")
            .join(format!("{use_id}_active.jsonl"));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    fn bind_callosum(journal: &Path) -> StdUnixListener {
        let path = journal.join("health/callosum.sock");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let listener = StdUnixListener::bind(path).unwrap();
        listener.set_nonblocking(true).unwrap();
        listener
    }

    fn async_listener(listener: &StdUnixListener) -> UnixListener {
        UnixListener::from_std(listener.try_clone().unwrap()).unwrap()
    }

    async fn accept_request(listener: &UnixListener) -> String {
        let (stream, _) = listener.accept().await.unwrap();
        let mut line = String::new();
        tokio::io::BufReader::new(stream)
            .read_line(&mut line)
            .await
            .unwrap();
        line
    }

    fn assert_no_extra_request(listener: &StdUnixListener) {
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }

    fn write_daily_run_complete(journal: &Path, day: &str, ts: i64) {
        let path = journal.join(format!("chronicle/{day}/health/run_daily.jsonl"));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            path,
            format!("{{\"event\":\"run.complete\",\"ts\":{ts}}}\n"),
        )
        .unwrap();
    }

    #[test]
    fn generated_at_requires_status_as_first_heading_and_first_content_line() {
        assert_eq!(
            generated_at_from_body("## Status\n<!-- generated_at: 2026-05-26T17:32:18Z -->\nok\n"),
            Some("2026-05-26T17:32:18Z".to_owned())
        );
        assert_eq!(
            generated_at_from_body(
                "## Other\n\n## Status\n<!-- generated_at: 2026-05-26T17:32:18Z -->\n"
            ),
            None
        );
        assert_eq!(generated_at_ms("2026-02-30T17:32:18Z"), None);
    }

    #[test]
    fn daily_reader_uses_two_days_and_only_daily_logs() {
        let journal = tempfile::tempdir().unwrap();
        let yesterday = journal.path().join("chronicle/20260525/health");
        let today = journal.path().join("chronicle/20260526/health");
        fs::create_dir_all(&yesterday).unwrap();
        fs::create_dir_all(&today).unwrap();
        fs::write(
            yesterday.join("one_daily.jsonl"),
            "{\"event\":\"run.complete\",\"ts\":4}\n",
        )
        .unwrap();
        fs::write(
            today.join("other.jsonl"),
            "{\"event\":\"run.complete\",\"ts\":9}\n",
        )
        .unwrap();
        fs::write(
            today.join("two_daily.jsonl"),
            "bad\n{\"event\":\"run.complete\",\"ts\":8}\n",
        )
        .unwrap();
        assert_eq!(
            latest_daily_run_complete_ts(journal.path(), "20260526"),
            Some(8)
        );
    }

    #[test]
    fn default_options_use_local_day_and_python_intervals() {
        let options = RefreshOptions::default();
        assert_eq!(
            options.today,
            chrono::Local::now().format("%Y%m%d").to_string()
        );
        assert_eq!(options.claim_windows, vec![Duration::from_secs(1); 3]);
        assert_eq!(options.claim_poll_interval, Duration::from_millis(100));
        assert_eq!(options.outcome_timeout, Duration::from_secs(600));
        assert_eq!(options.outcome_poll_interval, Duration::from_millis(500));
    }

    #[test]
    fn refresh_options_are_fully_overridable_by_native_tests() {
        let options = RefreshOptions {
            today: "20990102".to_owned(),
            claim_windows: vec![Duration::ZERO],
            claim_poll_interval: Duration::ZERO,
            outcome_timeout: Duration::ZERO,
            outcome_poll_interval: Duration::ZERO,
        };
        assert_eq!(options.today, "20990102");
        assert_eq!(options.claim_windows, vec![Duration::ZERO]);
        assert_eq!(options.claim_poll_interval, Duration::ZERO);
        assert_eq!(options.outcome_timeout, Duration::ZERO);
        assert_eq!(options.outcome_poll_interval, Duration::ZERO);
    }

    #[test]
    fn cortex_request_broadcasts_three_times_when_never_claimed() {
        let journal = tempfile::tempdir().unwrap();
        let options = options();
        let result = runtime().block_on(async {
            let socket = bind_callosum(journal.path());
            let listener = async_listener(&socket);
            let server = tokio::spawn(async move {
                for _ in 0..3 {
                    let _ = accept_request(&listener).await;
                }
                listener
            });
            let result = cortex_request(journal.path(), &options).await;
            server.await.unwrap();
            assert_no_extra_request(&socket);
            result
        });

        assert!(matches!(result, Err(RequestError::NotClaimed)));
    }

    #[test]
    fn cortex_request_stops_after_claim_on_second_broadcast() {
        let journal = tempfile::tempdir().unwrap();
        let journal_path = journal.path().to_path_buf();
        let options = options();
        let result = runtime().block_on(async {
            let socket = bind_callosum(journal.path());
            let listener = async_listener(&socket);
            let server = tokio::spawn(async move {
                for index in 1..=2 {
                    let line = accept_request(&listener).await;
                    if index == 2 {
                        let use_id =
                            serde_json::from_str::<serde_json::Value>(&line).unwrap()["use_id"]
                                .as_str()
                                .unwrap()
                                .to_owned();
                        active_use(&journal_path, &use_id, "{\"event\":\"request\"}\n");
                    }
                }
                listener
            });
            let result = cortex_request(journal.path(), &options).await;
            server.await.unwrap();
            assert_no_extra_request(&socket);
            result
        });

        assert!(result.is_ok());
    }

    #[test]
    fn cortex_request_sends_once_when_claim_is_already_present() {
        let journal = tempfile::tempdir().unwrap();
        let options = options();
        let use_id = "known-use".to_owned();
        active_use(journal.path(), &use_id, "{\"event\":\"request\"}\n");
        let expected_use_id = use_id.clone();
        let result = runtime().block_on(async {
            let socket = bind_callosum(journal.path());
            let listener = async_listener(&socket);
            let server = tokio::spawn(async move {
                let _ = accept_request(&listener).await;
                listener
            });
            let result =
                cortex_request_for_use_id(journal.path(), &options, 42, expected_use_id).await;
            server.await.unwrap();
            assert_no_extra_request(&socket);
            result
        });

        assert!(matches!(result, Ok(ref result_use_id) if result_use_id == &use_id));
    }

    #[test]
    fn cortex_request_returns_unavailable_after_the_first_failed_send() {
        let journal = tempfile::tempdir().unwrap();

        let result = runtime().block_on(cortex_request(journal.path(), &options()));

        assert!(matches!(result, Err(RequestError::Unavailable)));
    }

    #[test]
    fn wait_for_uses_times_out_for_a_pending_use_without_terminal_row() {
        let journal = tempfile::tempdir().unwrap();
        let use_id = "pending".to_owned();
        active_use(journal.path(), &use_id, "{\"event\":\"request\"}\n");

        let outcome = runtime().block_on(wait_for_uses(journal.path(), &[use_id], &options()));

        assert!(outcome.timed_out);
        assert_eq!(outcome.end_state, "unknown");
    }

    #[test]
    fn wait_for_uses_recovers_preexisting_error_from_disk() {
        let journal = tempfile::tempdir().unwrap();
        let use_id = "error".to_owned();
        active_use(journal.path(), &use_id, "{\"event\":\"error\"}\n");

        let outcome = runtime().block_on(wait_for_uses(journal.path(), &[use_id], &options()));

        assert!(!outcome.timed_out);
        assert_eq!(outcome.end_state, "error");
    }

    #[test]
    fn wait_for_uses_recovers_preexisting_active_finish_from_disk() {
        let journal = tempfile::tempdir().unwrap();
        let use_id = "finish".to_owned();
        active_use(journal.path(), &use_id, "{\"event\":\"finish\"}\n");

        let outcome = runtime().block_on(wait_for_uses(journal.path(), &[use_id], &options()));

        assert!(!outcome.timed_out);
        assert_eq!(outcome.end_state, "finish");
    }

    #[test]
    fn pending_use_file_is_neither_a_claim_nor_a_use_file() {
        let journal = tempfile::tempdir().unwrap();
        let talents = journal.path().join("talents/steward");
        fs::create_dir_all(&talents).unwrap();
        fs::write(talents.join("pending_pending.jsonl"), "{}\n").unwrap();

        assert!(!claimed(&journal.path().join("talents"), "pending"));
        assert_eq!(
            find_use_file(&journal.path().join("talents"), "pending"),
            None
        );
    }

    #[test]
    fn freshness_decision_matches_the_steward_contract_matrix() {
        let journal = tempfile::tempdir().unwrap();
        let stamp_ms = generated_at_ms(STAMP).unwrap();
        write_daily_run_complete(journal.path(), "20260526", stamp_ms - 1);
        assert_eq!(
            is_already_fresh(&health_body(STAMP), journal.path(), "20260526"),
            Some(STAMP.to_owned())
        );
        write_daily_run_complete(journal.path(), "20260526", stamp_ms);
        assert_eq!(
            is_already_fresh(&health_body(STAMP), journal.path(), "20260526"),
            Some(STAMP.to_owned())
        );
        write_daily_run_complete(journal.path(), "20260526", stamp_ms + 1);
        assert_eq!(
            is_already_fresh(&health_body(STAMP), journal.path(), "20260526"),
            None
        );

        let no_run = tempfile::tempdir().unwrap();
        let no_run_path = no_run
            .path()
            .join("chronicle/20260526/health/run_daily.jsonl");
        fs::create_dir_all(no_run_path.parent().unwrap()).unwrap();
        fs::write(no_run_path, "{\"event\":\"other\",\"ts\":9}\n").unwrap();
        assert_eq!(
            is_already_fresh(&health_body(STAMP), no_run.path(), "20260526"),
            None
        );

        let valid_journal = tempfile::tempdir().unwrap();
        write_daily_run_complete(valid_journal.path(), "20260526", stamp_ms);
        assert_eq!(
            is_already_fresh(
                "## Other\n\n## Status\n<!-- generated_at: 2026-05-26T17:32:18Z -->\n",
                valid_journal.path(),
                "20260526"
            ),
            None
        );
        assert_eq!(
            is_already_fresh(
                "## Status\n\n<!-- generated_at: 2026-05-26T17:32:18Z -->\n",
                valid_journal.path(),
                "20260526"
            ),
            None
        );
        assert_eq!(
            is_already_fresh(
                "## Status\n<!-- generated_at: 2026-02-30T17:32:18Z -->\n",
                valid_journal.path(),
                "20260526"
            ),
            None
        );
        assert_eq!(
            is_already_fresh(
                "# H1\n\n## Status\n<!-- generated_at: 2026-05-26T17:32:18Z -->\nsol is well.\n\n## Needs your attention\n\n## Auto-repairs (last 7d)\n",
                valid_journal.path(),
                "20260526"
            ),
            Some(STAMP.to_owned())
        );
    }

    #[test]
    fn file_change_proof_requires_an_updated_parseable_health_file() {
        let journal = tempfile::tempdir().unwrap();
        let health = journal.path().join("identity/health.md");
        assert!(matches!(
            file_change_proof(&health, None),
            Err(FileProofError::NotUpdated)
        ));

        fs::create_dir_all(health.parent().unwrap()).unwrap();
        fs::write(&health, health_body(STAMP)).unwrap();
        let unchanged = modified_ns(&health).unwrap();
        assert!(matches!(
            file_change_proof(&health, unchanged),
            Err(FileProofError::NotUpdated)
        ));

        let before = UNIX_EPOCH + Duration::from_secs(10);
        let after = before + Duration::from_nanos(1);
        fs::File::open(&health)
            .unwrap()
            .set_times(FileTimes::new().set_modified(before))
            .unwrap();
        let before_ns = modified_ns(&health).unwrap();
        fs::write(&health, "## Status\nmissing stamp\n").unwrap();
        fs::File::open(&health)
            .unwrap()
            .set_times(FileTimes::new().set_modified(after))
            .unwrap();
        assert!(matches!(
            file_change_proof(&health, before_ns),
            Err(FileProofError::NotUpdated)
        ));

        fs::write(&health, health_body(STAMP)).unwrap();
        assert!(matches!(
            file_change_proof(&health, None),
            Ok((ref stamp, _)) if stamp == STAMP
        ));

        fs::File::open(&health)
            .unwrap()
            .set_times(FileTimes::new().set_modified(before))
            .unwrap();
        let before_ns = modified_ns(&health).unwrap();
        fs::File::open(&health)
            .unwrap()
            .set_times(FileTimes::new().set_modified(after))
            .unwrap();
        assert!(file_change_proof(&health, before_ns).is_ok());
    }

    #[test]
    fn refresh_releases_its_lock_after_a_request_failure() {
        let journal = tempfile::tempdir().unwrap();
        let health = journal.path().join("identity/health.md");

        assert_eq!(
            refresh_with_options(journal.path(), &health, options()),
            ExitCode::from(1)
        );
        assert!(matches!(acquire_lock(journal.path()), LockAcquire::Held(_)));
    }
}
