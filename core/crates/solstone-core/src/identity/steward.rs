// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native implementation of the steward-backed identity health refresh.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::process::ExitCode;
use std::time::UNIX_EPOCH;

use chrono::{Local, NaiveDate};
use nix::errno::Errno;
use nix::fcntl::{Flock, FlockArg};
use serde_json::{Map, Value};
use solstone_core_cortex_client::{
    CortexClientError, CortexRequest, CortexRequestClient, CortexRequestPolicy, DispatchError,
    UseEndState,
};
use solstone_core_journal_io::{
    JournalRoot,
    operational_log::{OplogFormat, catalog_oplogs},
};

use super::EXIT_FAILURE;

const STATUS_HEADING: &str = "## Status";
const GENERATED_AT_PREFIX: &str = "<!-- generated_at: ";
const GENERATED_AT_SUFFIX: &str = " -->";

#[derive(Clone)]
struct RefreshOptions {
    today: String,
}

impl Default for RefreshOptions {
    fn default() -> Self {
        Self {
            today: Local::now().format("%Y%m%d").to_string(),
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
        Err(RequestError::Read) => {
            eprintln!("Error: failed to read steward Cortex use log.");
            return ExitCode::from(EXIT_FAILURE);
        }
    };
    if outcome.timed_out {
        eprintln!("Error: steward request timed out.");
        return ExitCode::from(EXIT_FAILURE);
    }
    if outcome.end_state != UseEndState::Finish {
        eprintln!(
            "Error: steward request failed: {}.",
            outcome.end_state.as_str()
        );
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
    Read,
}

struct Outcome {
    end_state: UseEndState,
    timed_out: bool,
}

async fn request_and_wait(
    journal: &Path,
    options: &RefreshOptions,
) -> Result<Outcome, RequestError> {
    let client = CortexRequestClient::new(journal, CortexRequestPolicy::interactive());
    let request = steward_request(&options.today);
    let use_id = client
        .dispatch(&request)
        .await
        .map_err(map_dispatch_error)?;
    let report = client
        .wait_for_uses(std::slice::from_ref(&use_id))
        .await
        .map_err(map_client_error)?;
    Ok(Outcome {
        end_state: report
            .completed
            .get(&use_id)
            .map(|completion| completion.end_state)
            .unwrap_or(UseEndState::Unknown),
        timed_out: report
            .timed_out
            .iter()
            .any(|timed_out| timed_out.use_id() == use_id),
    })
}

fn map_dispatch_error(error: DispatchError) -> RequestError {
    match error {
        DispatchError::Unavailable => RequestError::Unavailable,
        DispatchError::NotClaimed { .. } => RequestError::NotClaimed,
    }
}

fn map_client_error(error: CortexClientError) -> RequestError {
    match error {
        CortexClientError::Dispatch(error) => map_dispatch_error(error),
        CortexClientError::ReadUseLog(_) => RequestError::Read,
    }
}

fn steward_request(today: &str) -> CortexRequest {
    let mut config = Map::new();
    config.insert("day".to_owned(), Value::String(today.to_owned()));
    config.insert("output".to_owned(), Value::String("md".to_owned()));
    config.insert("refresh".to_owned(), Value::Bool(true));
    CortexRequest::new("", "steward").with_config(config)
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
    let today = NaiveDate::parse_from_str(today, "%Y%m%d").ok()?;
    let previous = today.pred_opt()?;
    let snapshot = catalog_oplogs(JournalRoot::open(journal).ok()?, &[previous, today]).ok()?;
    let mut latest = None;
    for (entry, mut file) in snapshot.into_catalogued_entries() {
        if entry.name().source().display_slug() != "think"
            || entry.name().run().display_slug() != "daily"
            || entry.name().format() != OplogFormat::Jsonl
        {
            continue;
        }
        if file
            .seek(SeekFrom::Start(entry.payload_offset() as u64))
            .is_err()
        {
            continue;
        }
        for line in BufReader::new(file).lines().map_while(Result::ok) {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Ok(Value::Object(row)) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            if row.get("event").and_then(Value::as_str) != Some("daily.completion")
                || row.get("complete").and_then(Value::as_bool) != Some(true)
            {
                continue;
            }
            // Daily writers emit numeric timestamps; strings are intentionally ignored.
            let Some(ts) = row.get("ts").and_then(Value::as_i64) else {
                continue;
            };
            latest = Some(latest.map_or(ts, |current: i64| current.max(ts)));
        }
    }
    latest
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
        FileProofError, RefreshOptions, file_change_proof, generated_at_from_body, generated_at_ms,
        is_already_fresh, latest_daily_run_complete_ts, modified_ns, steward_request,
    };
    use solstone_core_cortex_client::CortexRequest;
    use solstone_core_journal_io::{
        JournalRoot,
        operational_log::{OplogFormat, create_oplog_at},
    };
    use std::fs::{self, FileTimes};
    use std::io::Write;
    use std::path::Path;
    use std::time::{Duration, UNIX_EPOCH};

    const STAMP: &str = "2026-05-26T17:32:18Z";

    fn health_body(stamp: &str) -> String {
        format!(
            "## Status\n<!-- generated_at: {stamp} -->\nyour journal is well.\n\n## Needs your attention\n\n## Auto-repairs (last 7d)\n"
        )
    }

    fn write_oplog(journal: &Path, day: &str, source: &str, run: &str, body: &str) {
        let instant = chrono::NaiveDate::parse_from_str(day, "%Y%m%d")
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap()
            .and_utc()
            .fixed_offset();
        let mut writer = create_oplog_at(
            JournalRoot::open(journal).unwrap(),
            source,
            run,
            OplogFormat::Jsonl,
            instant,
        )
        .unwrap();
        writer.write_all(body.as_bytes()).unwrap();
    }

    fn write_daily_run_complete(journal: &Path, day: &str, ts: i64) {
        write_oplog(
            journal,
            day,
            "think",
            "daily",
            &format!("{{\"event\":\"daily.completion\",\"complete\":true,\"ts\":{ts}}}\n"),
        );
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
        write_oplog(
            journal.path(),
            "20260525",
            "think",
            "daily",
            "{\"event\":\"daily.completion\",\"complete\":true,\"ts\":4}\n",
        );
        write_oplog(
            journal.path(),
            "20260526",
            "think",
            "weekly",
            "{\"event\":\"daily.completion\",\"complete\":true,\"ts\":9}\n",
        );
        write_oplog(
            journal.path(),
            "20260526",
            "think",
            "daily",
            "bad\n{\"event\":\"daily.completion\",\"complete\":false,\"ts\":10}\n{\"event\":\"daily.completion\",\"complete\":true,\"ts\":8}\n",
        );
        assert_eq!(
            latest_daily_run_complete_ts(journal.path(), "20260526"),
            Some(8)
        );
    }

    #[test]
    fn default_options_use_the_local_day() {
        let options = RefreshOptions::default();
        assert_eq!(
            options.today,
            chrono::Local::now().format("%Y%m%d").to_string()
        );
    }

    #[test]
    fn steward_envelope_pins_all_required_fields() {
        let request: CortexRequest = steward_request("20260526");
        let value: serde_json::Value =
            serde_json::from_str(&request.request_line(42, "42").unwrap()).unwrap();
        assert_eq!(value["tract"], "cortex");
        assert_eq!(value["event"], "request");
        assert_eq!(value["ts"], 42);
        assert_eq!(value["use_id"], "42");
        assert_eq!(value["name"], "steward");
        assert_eq!(value["prompt"], "");
        assert_eq!(value["day"], "20260526");
        assert_eq!(value["output"], "md");
        assert_eq!(value["refresh"], true);
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
                "# H1\n\n## Status\n<!-- generated_at: 2026-05-26T17:32:18Z -->\nyour journal is well.\n\n## Needs your attention\n\n## Auto-repairs (last 7d)\n",
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
}
