// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value};
use solstone_core_callosum::{CallosumEnvelope, CallosumOneShotSender, CallosumSocketConnection};
use solstone_core_cli::EngageOptions;

const CLAIM_WINDOWS: [Duration; 3] = [Duration::from_secs(1); 3];
const CLAIM_POLL_INTERVAL: Duration = Duration::from_millis(100);
const OUTCOME_TIMEOUT: Duration = Duration::from_secs(600);
const OUTCOME_POLL_INTERVAL: Duration = Duration::from_millis(500);
const SOCKET_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone)]
struct Timing {
    claim_windows: Vec<Duration>,
    claim_poll_interval: Duration,
    outcome_timeout: Duration,
    outcome_poll_interval: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            claim_windows: CLAIM_WINDOWS.to_vec(),
            claim_poll_interval: CLAIM_POLL_INTERVAL,
            outcome_timeout: OUTCOME_TIMEOUT,
            outcome_poll_interval: OUTCOME_POLL_INTERVAL,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RequestError {
    Unavailable,
    NotClaimed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Outcome {
    end_state: String,
    timed_out: bool,
}

pub(crate) fn run(journal: &Path, options: EngageOptions) -> ExitCode {
    let mut prompt = String::new();
    if let Err(error) = io::stdin().read_to_string(&mut prompt) {
        eprintln!("Error: failed to read prompt from stdin: {error}");
        return ExitCode::from(1);
    }
    let prompt = trim_python_whitespace(&prompt);
    if prompt.is_empty() {
        eprintln!("Error: no prompt provided on stdin.");
        return ExitCode::from(1);
    }

    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => {
            eprintln!("Error: failed to send cortex request.");
            return ExitCode::from(1);
        }
    };
    let timing = Timing::default();
    let use_id = match runtime.block_on(cortex_request(journal, prompt, &options, &timing)) {
        Ok(use_id) => use_id,
        Err(RequestError::Unavailable | RequestError::NotClaimed) => {
            eprintln!("Error: failed to send cortex request.");
            return ExitCode::from(1);
        }
    };
    if !options.wait {
        println!("{use_id}");
        return ExitCode::SUCCESS;
    }

    let outcome = runtime.block_on(wait_for_use(journal, &use_id, &timing));
    if outcome.timed_out {
        eprintln!("Error: agent timed out.");
        return ExitCode::from(1);
    }
    if outcome.end_state != "finish" {
        eprintln!("Error: agent ended with state: {}", outcome.end_state);
        return ExitCode::from(1);
    }
    match finish_result(journal, &use_id) {
        Ok(result) => {
            println!("{result}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("Error: failed to read agent result: {error}");
            ExitCode::from(1)
        }
    }
}

async fn cortex_request(
    journal: &Path,
    prompt: &str,
    options: &EngageOptions,
    timing: &Timing,
) -> Result<String, RequestError> {
    let ts = current_ms().ok_or(RequestError::Unavailable)?;
    let use_id = ts.to_string();
    let talents_dir = journal.join("talents");
    fs::create_dir_all(&talents_dir).map_err(|_| RequestError::Unavailable)?;
    let line = request_line(ts, &use_id, prompt, options).map_err(|_| RequestError::Unavailable)?;
    let sender = CallosumOneShotSender::new(journal.join("health/callosum.sock"), SOCKET_TIMEOUT);

    for window in &timing.claim_windows {
        if sender.send_line(&line).is_err() {
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
            tokio::time::sleep(timing.claim_poll_interval.min(remaining)).await;
            if claimed(&talents_dir, &use_id) {
                return Ok(use_id);
            }
        }
    }
    Err(RequestError::NotClaimed)
}

fn request_line(
    ts: i64,
    use_id: &str,
    prompt: &str,
    options: &EngageOptions,
) -> Result<String, serde_json::Error> {
    let mut extra = Map::new();
    extra.insert("use_id".to_owned(), Value::String(use_id.to_owned()));
    extra.insert("prompt".to_owned(), Value::String(prompt.to_owned()));
    extra.insert("name".to_owned(), Value::String(options.name.clone()));
    if let Some(facet) = &options.facet {
        extra.insert("facet".to_owned(), Value::String(facet.clone()));
    }
    if let Some(day) = &options.day {
        extra.insert("day".to_owned(), Value::String(day.clone()));
    }
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

async fn wait_for_use(journal: &Path, use_id: &str, timing: &Timing) -> Outcome {
    let mut pending = HashSet::from([use_id.to_owned()]);
    let mut completed = HashMap::new();
    let mut listener =
        CallosumSocketConnection::new(journal.join("health/callosum.sock"), Map::new());
    listener.start();

    recover_completed(journal, &mut pending, &mut completed);
    let deadline = tokio::time::Instant::now() + timing.outcome_timeout;
    while !pending.is_empty() {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        let wait_for = timing.outcome_poll_interval.min(remaining);
        if let Ok(Some(message)) = tokio::time::timeout(wait_for, listener.next_message()).await
            && message.tract == "cortex"
            && matches!(message.event.as_str(), "finish" | "error")
            && let Some(message_use_id) = message.extra.get("use_id").and_then(Value::as_str)
            && pending.remove(message_use_id)
        {
            completed.insert(message_use_id.to_owned(), message.event);
        }
        recover_completed(journal, &mut pending, &mut completed);
    }
    listener.stop().await;
    recover_completed(journal, &mut pending, &mut completed);
    Outcome {
        end_state: completed
            .get(use_id)
            .cloned()
            .unwrap_or_else(|| "unknown".to_owned()),
        timed_out: pending.contains(use_id),
    }
}

fn recover_completed(
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

fn read_use_events(journal: &Path, use_id: &str) -> Vec<Value> {
    read_use_events_result(journal, use_id).unwrap_or_default()
}

fn read_use_events_result(journal: &Path, use_id: &str) -> io::Result<Vec<Value>> {
    let Some(path) = find_use_file(&journal.join("talents"), use_id) else {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("Talent log not found: {use_id}"),
        ));
    };
    let file = File::open(path)?;
    let lines = BufReader::new(file)
        .lines()
        .collect::<io::Result<Vec<_>>>()?;
    Ok(lines
        .into_iter()
        .filter_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
        .collect())
}

fn use_end_state(journal: &Path, use_id: &str) -> Option<String> {
    read_use_events(journal, use_id)
        .into_iter()
        .rev()
        .find_map(|event| match event.get("event").and_then(Value::as_str) {
            Some("finish") => Some("finish".to_owned()),
            Some("error") => Some("error".to_owned()),
            _ => None,
        })
}

fn finish_result(journal: &Path, use_id: &str) -> io::Result<String> {
    Ok(read_use_events_result(journal, use_id)?
        .into_iter()
        .rev()
        .find(|event| event.get("event").and_then(Value::as_str) == Some("finish"))
        .and_then(|event| {
            event
                .get("result")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_default())
}

fn current_ms() -> Option<i64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
}

fn trim_python_whitespace(value: &str) -> &str {
    value.trim_matches(|character: char| {
        character.is_whitespace() || matches!(character, '\u{1c}'..='\u{1f}')
    })
}

#[cfg(test)]
mod tests {
    use super::{EngageOptions, Timing, request_line, trim_python_whitespace};

    #[test]
    fn request_line_is_flat_and_omits_absent_context() {
        let line = request_line(
            42,
            "42",
            "review this",
            &EngageOptions {
                name: "partner".to_owned(),
                wait: false,
                facet: None,
                day: Some("20260404".to_owned()),
            },
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["tract"], "cortex");
        assert_eq!(value["event"], "request");
        assert_eq!(value["ts"], 42);
        assert_eq!(value["use_id"], "42");
        assert_eq!(value["prompt"], "review this");
        assert_eq!(value["name"], "partner");
        assert_eq!(value["day"], "20260404");
        assert!(value.get("facet").is_none());
        let timing = Timing::default();
        assert_eq!(timing.claim_windows.len(), 3);
    }

    #[test]
    fn prompt_trim_matches_python_c0_information_separators() {
        assert_eq!(
            trim_python_whitespace("\u{1c}\u{1f} prompt \u{1e}"),
            "prompt"
        );
        assert_eq!(trim_python_whitespace("\u{1c}\u{1d}"), "");
    }
}
