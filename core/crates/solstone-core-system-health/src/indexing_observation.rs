// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only indexing observations and equal-timestamp outcome fold across
//! Think health operational logs and Cortex talent summaries.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
#[cfg(not(all(test, feature = "full-tests")))]
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{Duration, SystemTime};

use jiff::{SignedDuration, Span, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use solstone_core_format::paths::relative_to_journal;
use solstone_core_format::segment::is_date_key;
use solstone_core_journal_config::owner_zone;
use solstone_core_journal_io::{MalformedPolicy, day_dirs, read_jsonl_with_report};

use crate::vocabulary::BACKLOG_DEFAULT_WINDOW;
use crate::{HealthEvent, RunLogRecord};

pub const LIMIT_TOKEN_UNOBSERVED_OUTSIDE_DAYS: &str =
    "unobserved_history_outside_selected_think_days";
pub const LIMIT_TOKEN_UNFINISHED_NO_DAY_INDEX: &str = "unfinished_uses_have_no_day_index_row";
pub const LIMIT_TOKEN_FAILURES_NOT_RETAINED_PAST_HISTORY: &str =
    "failures_not_retained_past_inspected_history";
pub const LIMIT_TOKEN_PUBLICATION_NOT_CRASH_ATOMIC: &str =
    "publication_notification_not_crash_atomic";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexingAttemptObservation {
    pub path: String,
    pub identity: String,
    pub outcome: String,
    pub ts: i64,
    pub warnings: Vec<String>,
    pub cause: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexingDiagnostics {
    pub think_days: Vec<String>,
    pub window_start_ms: i64,
    pub window_end_ms: i64,
    pub partial: bool,
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexingObservations {
    pub outcomes: Vec<IndexingAttemptObservation>,
    pub diagnostics: IndexingDiagnostics,
}

#[derive(Debug, Clone)]
struct RawAttempt {
    path: String,
    identity: String,
    outcome: String,
    ts: i64,
    warnings: Vec<String>,
    cause: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutcomeCategory {
    Indexed,
    Excluded,
    Declined,
    Failed,
    Ambiguous,
    Unknown,
}

impl OutcomeCategory {
    fn from_outcome_str(s: &str) -> Self {
        match s {
            "indexed" | "removed" => Self::Indexed,
            "excluded" => Self::Excluded,
            "declined" => Self::Declined,
            "failed" => Self::Failed,
            _ => Self::Unknown,
        }
    }
}

/// Normalize recorded path identity for search indexing:
/// - Absolute paths under the journal root are stripped to journal-relative paths.
/// - Relative paths with backslashes or `..` keep the raw recorded string.
/// - Relative paths drop empty and `.` components, and drop a leading `chronicle/`
///   component when immediately followed by an 8-digit date key.
pub fn normalize_indexing_identity(journal: &Path, raw_path: &str) -> String {
    let p = Path::new(raw_path);
    if p.is_absolute() {
        if let Some(rel) = relative_to_journal(journal, p) {
            return rel;
        }
        return raw_path.to_owned();
    }
    if raw_path.contains('\\') || raw_path.split('/').any(|part| part == "..") {
        return raw_path.to_owned();
    }
    let parts: Vec<&str> = raw_path
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    if parts.is_empty() {
        return raw_path.to_owned();
    }
    if parts.len() >= 2 && parts[0] == "chronicle" && is_date_key(parts[1]) {
        return parts[1..].join("/");
    }
    parts.join("/")
}

/// Computes the inclusive owner-zone calendar-day window bounds:
/// Start is local midnight of (today - 29 days) in the owner zone,
/// choosing the earlier offset when ambiguous and the later offset in a gap.
/// End is `now` as epoch milliseconds.
pub fn indexing_window_bounds(journal: &Path, now: Timestamp) -> (i64, i64) {
    let zone = owner_zone(journal);
    let today = now.to_zoned_datetime(zone.clone()).date();
    let start_day = today
        .checked_sub(Span::new().days((BACKLOG_DEFAULT_WINDOW - 1) as i64))
        .unwrap_or(today);
    let midnight = start_day.to_datetime(jiff::civil::Time::midnight());
    let window_start_ms = solstone_core_time::from_local(&zone, midnight)
        .earliest()
        .or_else(|| {
            let later = midnight.checked_add(SignedDuration::from_hours(1)).ok()?;
            solstone_core_time::from_local(&zone, later).earliest()
        })
        .map_or(now, |start| start.timestamp())
        .as_millisecond();
    let window_end_ms = now.as_millisecond();
    (window_start_ms, window_end_ms)
}

/// Read indexing observations from Think operational logs and Cortex talent summaries.
///
/// Search and index health relies on journal publication attempts folded across
/// chronicle days and talents logs:
/// - Think days are selected from the newest chronicle day directories up to
///   `BACKLOG_DEFAULT_WINDOW` (30), sorted descending.
/// - Event timestamps are bounded by the inclusive 30-calendar-day owner-zone window.
/// - A partial read keeps any failure already observed from readable files.
/// - Chronicle days outside the selected 30 directories are unobserved even if an event
///   inside them carries a recent timestamp within the window.
/// - A thinking use which never finishes has no day-index row in `talents/<day>.jsonl`.
/// - This landing does not retain index failures past inspected history and does not
///   make publication notification crash-atomic.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FileStamp {
    len: u64,
    modified: Option<SystemTime>,
    #[cfg(unix)]
    ino: u64,
    #[cfg(unix)]
    dev: u64,
    #[cfg(unix)]
    ctime: i64,
    #[cfg(unix)]
    ctime_nsec: i64,
}

fn file_stamp(path: &Path) -> Option<FileStamp> {
    let metadata = fs::metadata(path).ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some(FileStamp {
            len: metadata.len(),
            modified: metadata.modified().ok(),
            ino: metadata.ino(),
            dev: metadata.dev(),
            ctime: metadata.ctime(),
            ctime_nsec: metadata.ctime_nsec(),
        })
    }
    #[cfg(not(unix))]
    {
        Some(FileStamp {
            len: metadata.len(),
            modified: metadata.modified().ok(),
        })
    }
}

#[derive(Debug, Clone)]
struct CachedFile {
    stamp: FileStamp,
    parsed_at: SystemTime,
    attempts: Vec<RawAttempt>,
    detail_lines: Vec<String>,
    partial: bool,
}

const OBSERVATION_FILE_REUSE: Duration = Duration::from_secs(60);
const OBSERVATION_JOURNAL_CAP: usize = 16;

type ObservationCache = HashMap<PathBuf, HashMap<PathBuf, CachedFile>>;

fn with_observation_cache<R>(f: impl FnOnce(&mut ObservationCache) -> R) -> R {
    #[cfg(all(test, feature = "full-tests"))]
    {
        // Fixtures and their fake clocks must not evict another test's entries.
        TEST_CACHE.with(|cache| f(&mut cache.borrow_mut()))
    }
    #[cfg(not(all(test, feature = "full-tests")))]
    {
        static CACHE: OnceLock<Mutex<ObservationCache>> = OnceLock::new();
        let mut cache = CACHE
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        f(&mut cache)
    }
}

fn cached_files_mut<'a>(
    cache: &'a mut ObservationCache,
    journal: &Path,
) -> &'a mut HashMap<PathBuf, CachedFile> {
    if !cache.contains_key(journal)
        && cache.len() >= OBSERVATION_JOURNAL_CAP
        && let Some(root) = cache.keys().next().cloned()
    {
        cache.remove(&root);
    }
    cache.entry(journal.to_path_buf()).or_default()
}

#[cfg(all(test, feature = "full-tests"))]
thread_local! {
    static TEST_CACHE: std::cell::RefCell<ObservationCache> = std::cell::RefCell::new(HashMap::new());
    static EXPIRY_OVERRIDE: std::cell::RefCell<Option<SystemTime>> = const { std::cell::RefCell::new(None) };
    static PARSE_COUNTER: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static PAUSE_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce() + Send>>> = const { std::cell::RefCell::new(None) };
}

fn expiry_now() -> SystemTime {
    #[cfg(all(test, feature = "full-tests"))]
    {
        if let Some(t) = EXPIRY_OVERRIDE.with(|cell| *cell.borrow()) {
            return t;
        }
    }
    SystemTime::now()
}

fn record_file_parsed() {
    #[cfg(all(test, feature = "full-tests"))]
    PARSE_COUNTER.with(|c| c.set(c.get() + 1));
}

fn run_pause_hook() {
    #[cfg(all(test, feature = "full-tests"))]
    {
        let hook = PAUSE_HOOK.with(|cell| cell.borrow_mut().take());
        if let Some(h) = hook {
            h();
        }
    }
}

#[cfg(all(test, feature = "full-tests"))]
pub(crate) fn observation_cache_files(journal: &Path) -> Vec<PathBuf> {
    with_observation_cache(|cache| {
        cache
            .get(journal)
            .map(|m| {
                let mut v: Vec<_> = m.keys().cloned().collect();
                v.sort();
                v
            })
            .unwrap_or_default()
    })
}

#[cfg(all(test, feature = "full-tests"))]
pub(crate) fn observation_cache_journals() -> Vec<PathBuf> {
    with_observation_cache(|cache| {
        let mut v: Vec<_> = cache.keys().cloned().collect();
        v.sort();
        v
    })
}

struct ParsedFile {
    attempts: Vec<RawAttempt>,
    detail_lines: Vec<String>,
    partial: bool,
}

fn parse_health_file(journal: &Path, path: &Path) -> Result<ParsedFile, (Vec<String>, bool)> {
    record_file_parsed();
    let report =
        match read_jsonl_with_report::<RunLogRecord>(path, Vec::new(), MalformedPolicy::Skip) {
            Ok(rep) => rep,
            Err(err) => {
                log::warn!("failed to read health log {}: {err}", path.display());
                return Err((
                    vec![format!("unreadable health log {}: {err}", path.display())],
                    true,
                ));
            }
        };
    let mut partial = false;
    let mut detail_lines = Vec::new();
    if report.malformed_line_count > 0 {
        partial = true;
        detail_lines.push(format!("malformed lines in {}", path.display()));
    }
    let mut attempts = Vec::new();
    for record in report.records {
        let HealthEvent::Unknown(kind, fields) = record.value.event else {
            continue;
        };
        if kind != "index.attempt" {
            continue;
        }
        let path_str = match fields.get("path").and_then(Value::as_str) {
            Some(p) if !p.is_empty() => p.to_owned(),
            _ => {
                partial = true;
                detail_lines.push("index.attempt missing path in think log".to_owned());
                continue;
            }
        };
        let outcome = fields
            .get("outcome")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_owned();
        let warnings = fields
            .get("warnings")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        let cause = fields
            .get("cause")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let ts = record.value.ts;
        let identity = normalize_indexing_identity(journal, &path_str);
        attempts.push(RawAttempt {
            path: path_str,
            identity,
            outcome,
            ts,
            warnings,
            cause,
        });
    }
    Ok(ParsedFile {
        attempts,
        detail_lines,
        partial,
    })
}

fn parse_talent_file(journal: &Path, path: &Path) -> Result<ParsedFile, (Vec<String>, bool)> {
    record_file_parsed();
    let content = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) => {
            log::warn!("failed to read talent log {}: {err}", path.display());
            return Err((
                vec![format!("unreadable talent log {}: {err}", path.display())],
                true,
            ));
        }
    };
    let mut partial = false;
    let mut detail_lines = Vec::new();
    let mut attempts = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let value: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => {
                partial = true;
                detail_lines.push(format!("malformed line in {}", path.display()));
                continue;
            }
        };
        let Some(obj) = value.as_object() else {
            continue;
        };
        let Some(attempts_val) = obj.get("index_attempts") else {
            continue;
        };
        let Some(attempts_arr) = attempts_val.as_array() else {
            partial = true;
            detail_lines.push(format!(
                "index_attempts is not an array in {}",
                path.display()
            ));
            continue;
        };
        for item in attempts_arr {
            let Some(item_obj) = item.as_object() else {
                partial = true;
                continue;
            };
            let Some(ts) = item_obj.get("ts").and_then(Value::as_i64) else {
                partial = true;
                detail_lines.push(format!("missing attempt ts in {}", path.display()));
                continue;
            };
            let Some(path_str) = item_obj.get("path").and_then(Value::as_str) else {
                partial = true;
                detail_lines.push(format!("missing attempt path in {}", path.display()));
                continue;
            };
            if path_str.is_empty() {
                partial = true;
                continue;
            }
            let outcome = item_obj
                .get("outcome")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_owned();
            let warnings = item_obj
                .get("warnings")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            let cause = item_obj
                .get("cause")
                .and_then(Value::as_str)
                .map(str::to_owned);

            let identity = normalize_indexing_identity(journal, path_str);
            attempts.push(RawAttempt {
                path: path_str.to_owned(),
                identity,
                outcome,
                ts,
                warnings,
                cause,
            });
        }
    }
    Ok(ParsedFile {
        attempts,
        detail_lines,
        partial,
    })
}

type FileParser = fn(&Path, &Path) -> Result<ParsedFile, (Vec<String>, bool)>;

fn process_file(
    journal: &Path,
    path: &Path,
    use_cache: bool,
    parser: FileParser,
    all_raw_attempts: &mut Vec<RawAttempt>,
    detail_lines: &mut Vec<String>,
    partial: &mut bool,
) {
    if use_cache {
        // Attribute access alone does not establish read access (notably on
        // Windows). A failed open must take the parser's live diagnostic path.
        let before_stamp = fs::File::open(path).ok().and_then(|_| file_stamp(path));
        let cached_hit = before_stamp.as_ref().and_then(|stamp| {
            with_observation_cache(|cache| {
                cache
                    .get(journal)?
                    .get(path)
                    .filter(|entry| {
                        &entry.stamp == stamp
                            && expiry_now()
                                .duration_since(entry.parsed_at)
                                .is_ok_and(|age| age < OBSERVATION_FILE_REUSE)
                    })
                    .map(|entry| {
                        (
                            entry.attempts.clone(),
                            entry.detail_lines.clone(),
                            entry.partial,
                        )
                    })
            })
        });

        if let Some((attempts, lines, file_partial)) = cached_hit {
            all_raw_attempts.extend(attempts);
            detail_lines.extend(lines);
            *partial |= file_partial;
            return;
        }

        let parsed_at = expiry_now();
        let parsed_res = parser(journal, path);
        run_pause_hook();
        let after_stamp = file_stamp(path);

        match parsed_res {
            Ok(parsed) => {
                with_observation_cache(|cache| {
                    let files = cached_files_mut(cache, journal);
                    match (before_stamp, after_stamp) {
                        (Some(b), Some(a)) if b == a => {
                            files.insert(
                                path.to_path_buf(),
                                CachedFile {
                                    stamp: b,
                                    parsed_at,
                                    attempts: parsed.attempts.clone(),
                                    detail_lines: parsed.detail_lines.clone(),
                                    partial: parsed.partial,
                                },
                            );
                        }
                        _ => {
                            files.remove(path);
                        }
                    }
                });
                all_raw_attempts.extend(parsed.attempts);
                detail_lines.extend(parsed.detail_lines);
                *partial |= parsed.partial;
            }
            Err((err_lines, file_partial)) => {
                with_observation_cache(|cache| {
                    if let Some(files) = cache.get_mut(journal) {
                        files.remove(path);
                    }
                });
                detail_lines.extend(err_lines);
                *partial |= file_partial;
            }
        }
    } else {
        match parser(journal, path) {
            Ok(parsed) => {
                all_raw_attempts.extend(parsed.attempts);
                detail_lines.extend(parsed.detail_lines);
                *partial |= parsed.partial;
            }
            Err((err_lines, file_partial)) => {
                detail_lines.extend(err_lines);
                *partial |= file_partial;
            }
        }
    }
}

fn fold_indexing_attempts(
    raw_attempts: Vec<RawAttempt>,
    window_start_ms: i64,
    window_end_ms: i64,
    think_days: Vec<String>,
    mut detail_lines: Vec<String>,
    partial: bool,
) -> IndexingObservations {
    let windowed_attempts: Vec<RawAttempt> = raw_attempts
        .into_iter()
        .filter(|att| att.ts >= window_start_ms && att.ts <= window_end_ms)
        .collect();

    let mut by_identity: BTreeMap<String, BTreeMap<i64, Vec<RawAttempt>>> = BTreeMap::new();
    for attempt in windowed_attempts {
        by_identity
            .entry(attempt.identity.clone())
            .or_default()
            .entry(attempt.ts)
            .or_default()
            .push(attempt);
    }

    let mut outcomes = Vec::new();
    for (identity, buckets) in by_identity {
        let mut current_state: Option<(OutcomeCategory, RawAttempt)> = None;

        for (_ts, mut attempts) in buckets {
            let mut has_failed = false;
            let mut has_declined = false;
            let mut has_unknown = false;
            let mut has_indexed = false;
            let mut has_excluded = false;

            for att in &attempts {
                match OutcomeCategory::from_outcome_str(&att.outcome) {
                    OutcomeCategory::Failed => has_failed = true,
                    OutcomeCategory::Declined => has_declined = true,
                    OutcomeCategory::Indexed => has_indexed = true,
                    OutcomeCategory::Excluded => has_excluded = true,
                    OutcomeCategory::Unknown | OutcomeCategory::Ambiguous => has_unknown = true,
                }
            }

            let bad_count = (has_failed as u8) + (has_declined as u8) + (has_unknown as u8);
            let bucket_cat = if bad_count >= 2 {
                OutcomeCategory::Ambiguous
            } else if has_failed {
                OutcomeCategory::Failed
            } else if has_declined {
                OutcomeCategory::Declined
            } else if has_unknown {
                OutcomeCategory::Unknown
            } else if has_indexed {
                OutcomeCategory::Indexed
            } else if has_excluded {
                OutcomeCategory::Excluded
            } else {
                OutcomeCategory::Unknown
            };

            attempts.retain(|att| {
                let cat = OutcomeCategory::from_outcome_str(&att.outcome);
                match bucket_cat {
                    OutcomeCategory::Ambiguous => matches!(
                        cat,
                        OutcomeCategory::Failed
                            | OutcomeCategory::Declined
                            | OutcomeCategory::Unknown
                            | OutcomeCategory::Ambiguous
                    ),
                    OutcomeCategory::Failed => cat == OutcomeCategory::Failed,
                    OutcomeCategory::Declined => cat == OutcomeCategory::Declined,
                    OutcomeCategory::Indexed => cat == OutcomeCategory::Indexed,
                    OutcomeCategory::Excluded => cat == OutcomeCategory::Excluded,
                    OutcomeCategory::Unknown => cat == OutcomeCategory::Unknown,
                }
            });

            attempts.sort_by(|a, b| {
                let a_cause = a.cause.as_deref().unwrap_or_default();
                let b_cause = b.cause.as_deref().unwrap_or_default();
                let a_warn = a.warnings.join("\n");
                let b_warn = b.warnings.join("\n");
                (a.path.as_str(), a_cause, a_warn.as_str()).cmp(&(
                    b.path.as_str(),
                    b_cause,
                    b_warn.as_str(),
                ))
            });

            let winning_attempt = attempts
                .into_iter()
                .next()
                .expect("bucket has at least one attempt");

            match bucket_cat {
                OutcomeCategory::Indexed
                | OutcomeCategory::Failed
                | OutcomeCategory::Declined
                | OutcomeCategory::Ambiguous => {
                    current_state = Some((bucket_cat, winning_attempt));
                }
                OutcomeCategory::Excluded | OutcomeCategory::Unknown => {
                    if current_state.is_none()
                        || matches!(
                            current_state.as_ref().map(|(c, _)| *c),
                            Some(OutcomeCategory::Excluded)
                        )
                    {
                        current_state = Some((bucket_cat, winning_attempt));
                    }
                }
            }
        }

        if let Some((cat, winner)) = current_state {
            let outcome_str = match cat {
                OutcomeCategory::Indexed => "indexed".to_owned(),
                OutcomeCategory::Excluded => "excluded".to_owned(),
                OutcomeCategory::Declined => "declined".to_owned(),
                OutcomeCategory::Failed => "failed".to_owned(),
                OutcomeCategory::Ambiguous => "ambiguous".to_owned(),
                OutcomeCategory::Unknown => winner.outcome,
            };
            outcomes.push(IndexingAttemptObservation {
                path: winner.path,
                identity,
                outcome: outcome_str,
                ts: winner.ts,
                warnings: winner.warnings,
                cause: winner.cause,
            });
        }
    }

    outcomes.sort_by(|a, b| a.identity.cmp(&b.identity));

    for outcome in &outcomes {
        if matches!(
            outcome.outcome.as_str(),
            "failed" | "declined" | "ambiguous"
        ) {
            let cause = outcome.cause.as_deref().unwrap_or("");
            let warnings = if outcome.warnings.is_empty() {
                String::new()
            } else {
                format!(" warnings: {}", outcome.warnings.join(", "))
            };
            let cause_text = if cause.is_empty() {
                String::new()
            } else {
                format!(" cause: {cause}")
            };
            detail_lines.push(format!(
                "attempt {} {}{}{}",
                outcome.path, outcome.outcome, cause_text, warnings
            ));
        }
    }

    detail_lines.sort();

    let mut lines = vec![
        LIMIT_TOKEN_UNOBSERVED_OUTSIDE_DAYS.to_owned(),
        LIMIT_TOKEN_UNFINISHED_NO_DAY_INDEX.to_owned(),
        LIMIT_TOKEN_FAILURES_NOT_RETAINED_PAST_HISTORY.to_owned(),
        LIMIT_TOKEN_PUBLICATION_NOT_CRASH_ATOMIC.to_owned(),
    ];
    lines.extend(detail_lines);

    IndexingObservations {
        outcomes,
        diagnostics: IndexingDiagnostics {
            think_days,
            window_start_ms,
            window_end_ms,
            partial,
            lines,
        },
    }
}

fn read_indexing_observations_with(
    journal: &Path,
    now: Timestamp,
    use_cache: bool,
) -> IndexingObservations {
    if use_cache {
        with_observation_cache(|cache| cache.retain(|root, _| root.is_dir()));
    }

    let (window_start_ms, window_end_ms) = indexing_window_bounds(journal, now);

    let mut partial = false;
    let mut detail_lines = Vec::new();

    let (think_days, selected_days) = match day_dirs(journal) {
        Ok(dir_map) => {
            let mut days: Vec<String> = dir_map.into_keys().collect();
            days.sort_by(|a, b| b.cmp(a));
            days.truncate(BACKLOG_DEFAULT_WINDOW);
            (days.clone(), days)
        }
        Err(err) => {
            partial = true;
            log::warn!("failed to list chronicle day directories: {err}");
            detail_lines.push(format!("failed to list chronicle days: {err}"));
            (Vec::new(), Vec::new())
        }
    };

    let mut health_files = Vec::new();
    for day in &selected_days {
        let health_dir = journal.join("chronicle").join(day).join("health");
        match fs::read_dir(&health_dir) {
            Ok(read_dir) => {
                let mut paths = Vec::new();
                for entry in read_dir {
                    let entry = match entry {
                        Ok(entry) => entry,
                        Err(error) => {
                            partial = true;
                            detail_lines
                                .push(format!("unreadable health directory entry: {error}"));
                            continue;
                        }
                    };
                    let path = entry.path();
                    if path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
                        paths.push(path);
                    }
                }
                paths.sort();
                health_files.extend(paths);
            }
            Err(err) => {
                if err.kind() != std::io::ErrorKind::NotFound {
                    partial = true;
                    log::warn!(
                        "failed to read health directory {}: {err}",
                        health_dir.display()
                    );
                    detail_lines.push(format!(
                        "unreadable health directory {}: {err}",
                        health_dir.display()
                    ));
                }
            }
        }
    }

    let mut talent_files = Vec::new();
    let talents_dir = journal.join("talents");
    match fs::read_dir(&talents_dir) {
        Ok(read_dir) => {
            let mut paths = Vec::new();
            for entry in read_dir {
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(error) => {
                        partial = true;
                        detail_lines.push(format!("unreadable talent directory entry: {error}"));
                        continue;
                    }
                };
                let path = entry.path();
                if path.extension().and_then(|x| x.to_str()) == Some("jsonl")
                    && path
                        .file_stem()
                        .and_then(|x| x.to_str())
                        .is_some_and(|stem| stem.len() == 8)
                {
                    paths.push(path);
                }
            }
            paths.sort();
            talent_files.extend(paths);
        }
        Err(err) => {
            if err.kind() != std::io::ErrorKind::NotFound {
                partial = true;
                log::warn!(
                    "failed to read talents directory {}: {err}",
                    talents_dir.display()
                );
                detail_lines.push(format!(
                    "unreadable talents directory {}: {err}",
                    talents_dir.display()
                ));
            }
        }
    }

    if use_cache {
        let mut selected_paths = HashSet::with_capacity(health_files.len() + talent_files.len());
        for p in &health_files {
            selected_paths.insert(p.clone());
        }
        for p in &talent_files {
            selected_paths.insert(p.clone());
        }
        with_observation_cache(|cache| {
            if let Some(files) = cache.get_mut(journal) {
                files.retain(|path, _| selected_paths.contains(path));
            }
        });
    }

    let mut all_raw_attempts = Vec::new();

    for path in health_files {
        process_file(
            journal,
            &path,
            use_cache,
            parse_health_file,
            &mut all_raw_attempts,
            &mut detail_lines,
            &mut partial,
        );
    }

    for path in talent_files {
        process_file(
            journal,
            &path,
            use_cache,
            parse_talent_file,
            &mut all_raw_attempts,
            &mut detail_lines,
            &mut partial,
        );
    }

    fold_indexing_attempts(
        all_raw_attempts,
        window_start_ms,
        window_end_ms,
        think_days,
        detail_lines,
        partial,
    )
}

/// Read indexing observations from Think operational logs and Cortex talent summaries.
///
/// Search and index health relies on journal publication attempts folded across
/// chronicle days and talents logs:
/// - Think days are selected from the newest chronicle day directories up to
///   `BACKLOG_DEFAULT_WINDOW` (30), sorted descending.
/// - Event timestamps are bounded by the inclusive 30-calendar-day owner-zone window.
/// - A partial read keeps any failure already observed from readable files.
/// - Chronicle days outside the selected 30 directories are unobserved even if an event
///   inside them carries a recent timestamp within the window.
/// - A thinking use which never finishes has no day-index row in `talents/<day>.jsonl`.
/// - This landing does not retain index failures past inspected history and does not
///   make publication notification crash-atomic.
pub fn read_indexing_observations(journal: &Path, now: Timestamp) -> IndexingObservations {
    read_indexing_observations_with(journal, now, true)
}

// Independent pre-cache reader: keep parsing and folding separate from the
// production cache path so shared refactor errors cannot satisfy the oracle.
#[cfg(all(test, feature = "full-tests"))]
fn read_indexing_observations_reference(journal: &Path, now: Timestamp) -> IndexingObservations {
    let mut partial = false;
    let mut detail_lines = Vec::new();
    let (window_start_ms, window_end_ms) = indexing_window_bounds(journal, now);

    let (think_days, selected_days) = match day_dirs(journal) {
        Ok(dir_map) => {
            let mut days: Vec<String> = dir_map.into_keys().collect();
            days.sort_by(|a, b| b.cmp(a));
            days.truncate(BACKLOG_DEFAULT_WINDOW);
            (days.clone(), days)
        }
        Err(err) => {
            partial = true;
            log::warn!("failed to list chronicle day directories: {err}");
            detail_lines.push(format!("failed to list chronicle days: {err}"));
            (Vec::new(), Vec::new())
        }
    };

    let mut raw_attempts: Vec<RawAttempt> = Vec::new();

    // 1. Scan selected chronicle think days
    for day in &selected_days {
        let health_dir = journal.join("chronicle").join(day).join("health");
        let entries = match fs::read_dir(&health_dir) {
            Ok(read_dir) => {
                let mut paths = Vec::new();
                for entry in read_dir {
                    let entry = match entry {
                        Ok(entry) => entry,
                        Err(error) => {
                            partial = true;
                            detail_lines
                                .push(format!("unreadable health directory entry: {error}"));
                            continue;
                        }
                    };
                    let path = entry.path();
                    if path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
                        paths.push(path);
                    }
                }
                paths.sort();
                paths
            }
            Err(err) => {
                if err.kind() != std::io::ErrorKind::NotFound {
                    partial = true;
                    log::warn!(
                        "failed to read health directory {}: {err}",
                        health_dir.display()
                    );
                    detail_lines.push(format!(
                        "unreadable health directory {}: {err}",
                        health_dir.display()
                    ));
                }
                continue;
            }
        };

        for path in entries {
            let report = match read_jsonl_with_report::<RunLogRecord>(
                &path,
                Vec::new(),
                MalformedPolicy::Skip,
            ) {
                Ok(rep) => rep,
                Err(err) => {
                    partial = true;
                    log::warn!("failed to read health log {}: {err}", path.display());
                    detail_lines.push(format!("unreadable health log {}: {err}", path.display()));
                    continue;
                }
            };
            if report.malformed_line_count > 0 {
                partial = true;
                detail_lines.push(format!("malformed lines in {}", path.display()));
            }
            for record in report.records {
                let HealthEvent::Unknown(kind, fields) = record.value.event else {
                    continue;
                };
                if kind != "index.attempt" {
                    continue;
                }
                let path_str = match fields.get("path").and_then(Value::as_str) {
                    Some(p) if !p.is_empty() => p.to_owned(),
                    _ => {
                        partial = true;
                        detail_lines.push("index.attempt missing path in think log".to_owned());
                        continue;
                    }
                };
                let outcome = fields
                    .get("outcome")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_owned();
                let warnings = fields
                    .get("warnings")
                    .and_then(Value::as_array)
                    .map(|arr| {
                        arr.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default();
                let cause = fields
                    .get("cause")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                let ts = record.value.ts;
                if ts >= window_start_ms && ts <= window_end_ms {
                    let identity = normalize_indexing_identity(journal, &path_str);
                    raw_attempts.push(RawAttempt {
                        path: path_str,
                        identity,
                        outcome,
                        ts,
                        warnings,
                        cause,
                    });
                }
            }
        }
    }

    // 2. Scan flat talents/*.jsonl in Cortex store
    let talents_dir = journal.join("talents");
    match fs::read_dir(&talents_dir) {
        Ok(read_dir) => {
            let mut paths = Vec::new();
            for entry in read_dir {
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(error) => {
                        partial = true;
                        detail_lines.push(format!("unreadable talent directory entry: {error}"));
                        continue;
                    }
                };
                let path = entry.path();
                if path.extension().and_then(|x| x.to_str()) == Some("jsonl")
                    && path
                        .file_stem()
                        .and_then(|x| x.to_str())
                        .is_some_and(|stem| stem.len() == 8)
                {
                    paths.push(path);
                }
            }
            paths.sort();

            for path in paths {
                let content = match fs::read_to_string(&path) {
                    Ok(text) => text,
                    Err(err) => {
                        partial = true;
                        log::warn!("failed to read talent log {}: {err}", path.display());
                        detail_lines
                            .push(format!("unreadable talent log {}: {err}", path.display()));
                        continue;
                    }
                };

                for line in content.lines() {
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    let value: Value = match serde_json::from_str(line) {
                        Ok(v) => v,
                        Err(_) => {
                            partial = true;
                            detail_lines.push(format!("malformed line in {}", path.display()));
                            continue;
                        }
                    };
                    let Some(obj) = value.as_object() else {
                        continue;
                    };
                    let Some(attempts_val) = obj.get("index_attempts") else {
                        continue;
                    };
                    let Some(attempts_arr) = attempts_val.as_array() else {
                        partial = true;
                        detail_lines.push(format!(
                            "index_attempts is not an array in {}",
                            path.display()
                        ));
                        continue;
                    };

                    for item in attempts_arr {
                        let Some(item_obj) = item.as_object() else {
                            partial = true;
                            continue;
                        };
                        let Some(ts) = item_obj.get("ts").and_then(Value::as_i64) else {
                            partial = true;
                            detail_lines.push(format!("missing attempt ts in {}", path.display()));
                            continue;
                        };
                        let Some(path_str) = item_obj.get("path").and_then(Value::as_str) else {
                            partial = true;
                            detail_lines
                                .push(format!("missing attempt path in {}", path.display()));
                            continue;
                        };
                        if path_str.is_empty() {
                            partial = true;
                            continue;
                        }
                        let outcome = item_obj
                            .get("outcome")
                            .and_then(Value::as_str)
                            .unwrap_or("unknown")
                            .to_owned();
                        let warnings = item_obj
                            .get("warnings")
                            .and_then(Value::as_array)
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(Value::as_str)
                                    .map(str::to_owned)
                                    .collect()
                            })
                            .unwrap_or_default();
                        let cause = item_obj
                            .get("cause")
                            .and_then(Value::as_str)
                            .map(str::to_owned);

                        if ts >= window_start_ms && ts <= window_end_ms {
                            let identity = normalize_indexing_identity(journal, path_str);
                            raw_attempts.push(RawAttempt {
                                path: path_str.to_owned(),
                                identity,
                                outcome,
                                ts,
                                warnings,
                                cause,
                            });
                        }
                    }
                }
            }
        }
        Err(err) => {
            if err.kind() != std::io::ErrorKind::NotFound {
                partial = true;
                log::warn!(
                    "failed to read talents directory {}: {err}",
                    talents_dir.display()
                );
                detail_lines.push(format!(
                    "unreadable talents directory {}: {err}",
                    talents_dir.display()
                ));
            }
        }
    }

    // 3. Fold attempts by identity then timestamp
    let mut by_identity: BTreeMap<String, BTreeMap<i64, Vec<RawAttempt>>> = BTreeMap::new();
    for attempt in raw_attempts {
        by_identity
            .entry(attempt.identity.clone())
            .or_default()
            .entry(attempt.ts)
            .or_default()
            .push(attempt);
    }

    let mut outcomes = Vec::new();
    for (identity, buckets) in by_identity {
        let mut current_state: Option<(OutcomeCategory, RawAttempt)> = None;

        for (_ts, mut attempts) in buckets {
            let mut has_failed = false;
            let mut has_declined = false;
            let mut has_unknown = false;
            let mut has_indexed = false;
            let mut has_excluded = false;

            for att in &attempts {
                match OutcomeCategory::from_outcome_str(&att.outcome) {
                    OutcomeCategory::Failed => has_failed = true,
                    OutcomeCategory::Declined => has_declined = true,
                    OutcomeCategory::Indexed => has_indexed = true,
                    OutcomeCategory::Excluded => has_excluded = true,
                    OutcomeCategory::Unknown | OutcomeCategory::Ambiguous => has_unknown = true,
                }
            }

            let bad_count = (has_failed as u8) + (has_declined as u8) + (has_unknown as u8);
            let bucket_cat = if bad_count >= 2 {
                OutcomeCategory::Ambiguous
            } else if has_failed {
                OutcomeCategory::Failed
            } else if has_declined {
                OutcomeCategory::Declined
            } else if has_unknown {
                OutcomeCategory::Unknown
            } else if has_indexed {
                OutcomeCategory::Indexed
            } else if has_excluded {
                OutcomeCategory::Excluded
            } else {
                OutcomeCategory::Unknown
            };

            // Filter candidates that match winning category
            attempts.retain(|att| {
                let cat = OutcomeCategory::from_outcome_str(&att.outcome);
                match bucket_cat {
                    OutcomeCategory::Ambiguous => {
                        matches!(
                            cat,
                            OutcomeCategory::Failed
                                | OutcomeCategory::Declined
                                | OutcomeCategory::Unknown
                                | OutcomeCategory::Ambiguous
                        )
                    }
                    OutcomeCategory::Failed => cat == OutcomeCategory::Failed,
                    OutcomeCategory::Declined => cat == OutcomeCategory::Declined,
                    OutcomeCategory::Indexed => cat == OutcomeCategory::Indexed,
                    OutcomeCategory::Excluded => cat == OutcomeCategory::Excluded,
                    OutcomeCategory::Unknown => cat == OutcomeCategory::Unknown,
                }
            });

            // Tie-break: lexicographically smallest (path, cause.unwrap_or_default(), warnings joined by \n)
            attempts.sort_by(|a, b| {
                let a_cause = a.cause.as_deref().unwrap_or_default();
                let b_cause = b.cause.as_deref().unwrap_or_default();
                let a_warn = a.warnings.join("\n");
                let b_warn = b.warnings.join("\n");
                (a.path.as_str(), a_cause, a_warn.as_str()).cmp(&(
                    b.path.as_str(),
                    b_cause,
                    b_warn.as_str(),
                ))
            });

            let winning_attempt = attempts
                .into_iter()
                .next()
                .expect("bucket has at least one attempt");

            // Apply progression rules:
            match bucket_cat {
                OutcomeCategory::Indexed
                | OutcomeCategory::Failed
                | OutcomeCategory::Declined
                | OutcomeCategory::Ambiguous => {
                    current_state = Some((bucket_cat, winning_attempt));
                }
                OutcomeCategory::Excluded => {
                    if current_state.is_none()
                        || matches!(
                            current_state.as_ref().map(|(c, _)| *c),
                            Some(OutcomeCategory::Excluded)
                        )
                    {
                        current_state = Some((bucket_cat, winning_attempt));
                    }
                }
                OutcomeCategory::Unknown => {
                    if current_state.is_none()
                        || matches!(
                            current_state.as_ref().map(|(c, _)| *c),
                            Some(OutcomeCategory::Excluded)
                        )
                    {
                        current_state = Some((bucket_cat, winning_attempt));
                    }
                }
            }
        }

        if let Some((cat, winner)) = current_state {
            let outcome_str = match cat {
                OutcomeCategory::Indexed => "indexed".to_owned(),
                OutcomeCategory::Excluded => "excluded".to_owned(),
                OutcomeCategory::Declined => "declined".to_owned(),
                OutcomeCategory::Failed => "failed".to_owned(),
                OutcomeCategory::Ambiguous => "ambiguous".to_owned(),
                OutcomeCategory::Unknown => winner.outcome,
            };
            outcomes.push(IndexingAttemptObservation {
                path: winner.path,
                identity,
                outcome: outcome_str,
                ts: winner.ts,
                warnings: winner.warnings,
                cause: winner.cause,
            });
        }
    }

    outcomes.sort_by(|a, b| a.identity.cmp(&b.identity));

    for outcome in &outcomes {
        if matches!(
            outcome.outcome.as_str(),
            "failed" | "declined" | "ambiguous"
        ) {
            let cause = outcome.cause.as_deref().unwrap_or("");
            let warnings = if outcome.warnings.is_empty() {
                String::new()
            } else {
                format!(" warnings: {}", outcome.warnings.join(", "))
            };
            let cause_text = if cause.is_empty() {
                String::new()
            } else {
                format!(" cause: {cause}")
            };
            detail_lines.push(format!(
                "attempt {} {}{}{}",
                outcome.path, outcome.outcome, cause_text, warnings
            ));
        }
    }

    detail_lines.sort();

    let mut lines = vec![
        LIMIT_TOKEN_UNOBSERVED_OUTSIDE_DAYS.to_owned(),
        LIMIT_TOKEN_UNFINISHED_NO_DAY_INDEX.to_owned(),
        LIMIT_TOKEN_FAILURES_NOT_RETAINED_PAST_HISTORY.to_owned(),
        LIMIT_TOKEN_PUBLICATION_NOT_CRASH_ATOMIC.to_owned(),
    ];
    lines.extend(detail_lines);

    IndexingObservations {
        outcomes,
        diagnostics: IndexingDiagnostics {
            think_days,
            window_start_ms,
            window_end_ms,
            partial,
            lines,
        },
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;

    fn utc(year: i16, month: i8, day: i8, hour: i8, minute: i8, second: i8) -> Timestamp {
        jiff::tz::Offset::UTC
            .to_timestamp(jiff::civil::date(year, month, day).at(hour, minute, second, 0))
            .unwrap()
    }
    use tempfile::tempdir;

    fn setup_utc_journal(dir: &Path) {
        let config_dir = dir.join("config");
        fs::create_dir_all(&config_dir).unwrap();
        fs::write(
            config_dir.join("journal.json"),
            r#"{"identity":{"timezone":"UTC"}}"#,
        )
        .unwrap();
    }

    #[test]
    fn window_start_and_now_boundaries() {
        let dir = tempdir().unwrap();
        setup_utc_journal(dir.path());
        let now = utc(2026, 10, 5, 12, 0, 0);
        let (start, end) = indexing_window_bounds(dir.path(), now);
        let start_dt = utc(2026, 9, 6, 0, 0, 0);
        assert_eq!(start, start_dt.as_millisecond());
        assert_eq!(end, now.as_millisecond());

        // Chronicle day with health log
        let health_dir = dir.path().join("chronicle").join("20261005").join("health");
        fs::create_dir_all(&health_dir).unwrap();
        let log_path = health_dir.join("think.test.jsonl");

        let in_start = serde_json::json!({
            "event": "index.attempt",
            "ts": start,
            "path": "20260906/talents/flow.md",
            "outcome": "failed",
            "cause": "start edge"
        });
        let before_start = serde_json::json!({
            "event": "index.attempt",
            "ts": start - 1,
            "path": "20260905/talents/flow.md",
            "outcome": "failed",
            "cause": "before start"
        });
        let at_now = serde_json::json!({
            "event": "index.attempt",
            "ts": end,
            "path": "20261005/talents/now.md",
            "outcome": "failed",
            "cause": "at now"
        });
        let after_now = serde_json::json!({
            "event": "index.attempt",
            "ts": end + 1,
            "path": "20261005/talents/now.md",
            "outcome": "indexed" // Must not clear failure
        });

        fs::write(
            &log_path,
            format!(
                "{}\n{}\n{}\n{}\n",
                in_start, before_start, at_now, after_now
            ),
        )
        .unwrap();

        let obs = read_indexing_observations(dir.path(), now);
        let identities: Vec<&str> = obs.outcomes.iter().map(|o| o.identity.as_str()).collect();
        assert!(identities.contains(&"20260906/talents/flow.md"));
        assert!(!identities.contains(&"20260905/talents/flow.md"));

        let now_item = obs
            .outcomes
            .iter()
            .find(|o| o.identity == "20261005/talents/now.md")
            .unwrap();
        assert_eq!(now_item.outcome, "failed");
    }

    #[test]
    fn equal_timestamp_and_multi_outcome_progression() {
        let dir = tempdir().unwrap();
        setup_utc_journal(dir.path());
        let now = utc(2026, 10, 5, 12, 0, 0);
        let health_dir = dir.path().join("chronicle").join("20261005").join("health");
        fs::create_dir_all(&health_dir).unwrap();

        let base_ts = now.as_millisecond() - 50_000;
        // 1. Equal ts failed + indexed stays failed
        let row1 = serde_json::json!({"event":"index.attempt","ts":base_ts,"path":"20261005/a.md","outcome":"indexed"});
        let row2 = serde_json::json!({"event":"index.attempt","ts":base_ts,"path":"20261005/a.md","outcome":"failed","cause":"bad"});
        // 2. Equal ts failed + declined is ambiguous
        let row3 = serde_json::json!({"event":"index.attempt","ts":base_ts,"path":"20261005/b.md","outcome":"declined"});
        let row4 = serde_json::json!({"event":"index.attempt","ts":base_ts,"path":"20261005/b.md","outcome":"failed"});
        // 3. Equal ts indexed + excluded is indexed
        let row5 = serde_json::json!({"event":"index.attempt","ts":base_ts,"path":"20261005/c.md","outcome":"excluded"});
        let row6 = serde_json::json!({"event":"index.attempt","ts":base_ts,"path":"20261005/c.md","outcome":"indexed"});

        // 4. Later indexed on same identity clears failure; later excluded or unknown does not; different path does not
        let row7 = serde_json::json!({"event":"index.attempt","ts":base_ts,"path":"20261005/d.md","outcome":"failed"});
        let row8 = serde_json::json!({"event":"index.attempt","ts":base_ts + 1000,"path":"20261005/d.md","outcome":"indexed"});

        let row9 = serde_json::json!({"event":"index.attempt","ts":base_ts,"path":"20261005/e.md","outcome":"failed"});
        let row10 = serde_json::json!({"event":"index.attempt","ts":base_ts + 1000,"path":"20261005/e.md","outcome":"excluded"});
        let row11 = serde_json::json!({"event":"index.attempt","ts":base_ts + 2000,"path":"20261005/e.md","outcome":"some_unknown"});

        let row12 = serde_json::json!({"event":"index.attempt","ts":base_ts + 3000,"path":"20261005/other.md","outcome":"indexed"});
        let row13 = serde_json::json!({"event":"phase.complete","ts":base_ts + 4000,"phase":"indexer","extensions":{"success":true}});

        let content = format!(
            "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n",
            row1, row2, row3, row4, row5, row6, row7, row8, row9, row10, row11, row12, row13
        );
        fs::write(health_dir.join("think.test.jsonl"), content).unwrap();

        let obs = read_indexing_observations(dir.path(), now);
        let a = obs
            .outcomes
            .iter()
            .find(|o| o.identity == "20261005/a.md")
            .unwrap();
        assert_eq!(a.outcome, "failed");

        let b = obs
            .outcomes
            .iter()
            .find(|o| o.identity == "20261005/b.md")
            .unwrap();
        assert_eq!(b.outcome, "ambiguous");

        let c = obs
            .outcomes
            .iter()
            .find(|o| o.identity == "20261005/c.md")
            .unwrap();
        assert_eq!(c.outcome, "indexed");

        let d = obs
            .outcomes
            .iter()
            .find(|o| o.identity == "20261005/d.md")
            .unwrap();
        assert_eq!(d.outcome, "indexed");

        let e = obs
            .outcomes
            .iter()
            .find(|o| o.identity == "20261005/e.md")
            .unwrap();
        assert_eq!(e.outcome, "failed");
    }

    #[test]
    fn path_identity_equivalence_and_missing_file_display() {
        let dir = tempdir().unwrap();
        setup_utc_journal(dir.path());
        let now = utc(2026, 10, 5, 12, 0, 0);
        let health_dir = dir.path().join("chronicle").join("20261005").join("health");
        fs::create_dir_all(&health_dir).unwrap();

        let base_ts = now.as_millisecond() - 50_000;
        let abs = dir
            .path()
            .join("chronicle")
            .join("20261005")
            .join("talents")
            .join("flow.md");
        let row1 = serde_json::json!({"event":"index.attempt","ts":base_ts,"path":"chronicle/20261005/talents/flow.md","outcome":"failed"});
        let row2 = serde_json::json!({"event":"index.attempt","ts":base_ts + 1000,"path":"20261005/talents/flow.md","outcome":"failed"});
        let row3 = serde_json::json!({"event":"index.attempt","ts":base_ts + 2000,"path":abs.to_str().unwrap(),"outcome":"indexed"});

        // Missing file display path kept
        let outside = tempdir().unwrap();
        let missing_path = outside.path().join("note.md").display().to_string();
        let row4 = serde_json::json!({"event":"index.attempt","ts":base_ts,"path":missing_path,"outcome":"failed","cause":"missing"});

        fs::write(
            health_dir.join("think.test.jsonl"),
            format!("{}\n{}\n{}\n{}\n", row1, row2, row3, row4),
        )
        .unwrap();

        let obs = read_indexing_observations(dir.path(), now);
        assert_eq!(obs.outcomes.len(), 2);
        let flow = obs
            .outcomes
            .iter()
            .find(|o| o.identity == "20261005/talents/flow.md")
            .unwrap();
        assert_eq!(flow.outcome, "indexed");
        assert_eq!(flow.ts, base_ts + 2000);

        let missing = obs
            .outcomes
            .iter()
            .find(|o| o.identity == missing_path)
            .unwrap();
        assert_eq!(missing.outcome, "failed");
        assert_eq!(missing.path, missing_path);
    }

    #[test]
    fn talents_file_old_name_with_attempt_inside_window() {
        let dir = tempdir().unwrap();
        setup_utc_journal(dir.path());
        let now = utc(2026, 10, 5, 12, 0, 0);
        let talents_dir = dir.path().join("talents");
        fs::create_dir_all(&talents_dir).unwrap();

        // 19700101.jsonl with outer ts 0 / 1000 (outside window), but attempt ts inside window
        let summary1 = serde_json::json!({
            "use_id": "one",
            "name": "flow",
            "day": "19700101",
            "ts": 1000,
            "status": "completed",
            "index_attempts": [
                {
                    "ts": now.as_millisecond() - 5000,
                    "path": "20261005/talents/flow.md",
                    "outcome": "failed",
                    "warnings": [],
                    "cause": "write error"
                }
            ]
        });
        // Summary without index_attempts field -> unobserved, clears nothing
        let summary2 = serde_json::json!({
            "use_id": "two",
            "name": "flow",
            "day": "19700101",
            "ts": 2000,
            "status": "completed"
        });
        // Summary with empty array -> clears nothing
        let summary3 = serde_json::json!({
            "use_id": "three",
            "name": "flow",
            "day": "19700101",
            "ts": 3000,
            "status": "completed",
            "index_attempts": []
        });

        fs::write(
            talents_dir.join("19700101.jsonl"),
            format!("{}\n{}\n{}\n", summary1, summary2, summary3),
        )
        .unwrap();

        let obs = read_indexing_observations(dir.path(), now);
        let flow = obs
            .outcomes
            .iter()
            .find(|o| o.identity == "20261005/talents/flow.md")
            .unwrap();
        assert_eq!(flow.outcome, "failed");
    }

    #[test]
    fn unreadable_sibling_health_log_keeps_failure_and_sets_partial() {
        let dir = tempdir().unwrap();
        setup_utc_journal(dir.path());
        let now = utc(2026, 10, 5, 12, 0, 0);
        let health_dir = dir.path().join("chronicle").join("20261005").join("health");
        fs::create_dir_all(&health_dir).unwrap();

        let readable = health_dir.join("think.a.jsonl");
        fs::write(
            &readable,
            serde_json::json!({
                "event": "index.attempt",
                "ts": now.as_millisecond() - 1000,
                "path": "20261005/note.md",
                "outcome": "failed",
                "cause": "unreadable test"
            })
            .to_string(),
        )
        .unwrap();

        let unreadable = health_dir.join("think.b.jsonl");
        fs::create_dir(&unreadable).unwrap();

        let obs = read_indexing_observations(dir.path(), now);
        assert!(
            obs.diagnostics
                .lines
                .iter()
                .any(|line| line.starts_with("unreadable health log"))
        );

        assert!(obs.diagnostics.partial);
        let note = obs
            .outcomes
            .iter()
            .find(|o| o.identity == "20261005/note.md")
            .unwrap();
        assert_eq!(note.outcome, "failed");
    }

    #[test]
    fn thirty_first_oldest_chronicle_directory_is_unobserved() {
        let dir = tempdir().unwrap();
        setup_utc_journal(dir.path());
        let now = utc(2026, 10, 5, 12, 0, 0);

        // Create 31 chronicle days: 20260901 through 20261001 (31 days)
        for d in 1..=31 {
            let day_str = format!("202609{:02}", d);
            let health_dir = dir.path().join("chronicle").join(&day_str).join("health");
            fs::create_dir_all(&health_dir).unwrap();
            let row = serde_json::json!({
                "event": "index.attempt",
                "ts": now.as_millisecond() - 1000,
                "path": format!("{day_str}/note.md"),
                "outcome": "failed"
            });
            fs::write(health_dir.join("think.jsonl"), row.to_string()).unwrap();
        }

        let obs = read_indexing_observations(dir.path(), now);
        assert_eq!(obs.diagnostics.think_days.len(), 30);
        // 20260901 is the 31st (oldest), so it should be unobserved
        assert!(!obs.diagnostics.think_days.contains(&"20260901".to_string()));
        let unobserved = obs
            .outcomes
            .iter()
            .find(|o| o.identity == "20260901/note.md");
        assert!(unobserved.is_none());

        // 20260902 is within the 30 newest, so it is observed
        assert!(obs.diagnostics.think_days.contains(&"20260902".to_string()));
        let observed = obs
            .outcomes
            .iter()
            .find(|o| o.identity == "20260902/note.md");
        assert!(observed.is_some());
    }

    #[test]
    fn cold_and_cached_public_reads_match_reference() {
        let dir = tempdir().unwrap();
        setup_utc_journal(dir.path());
        let now = utc(2026, 10, 5, 12, 0, 0);
        let (window_start_ms, _) = indexing_window_bounds(dir.path(), now);

        // 31 chronicle days so oldest is omitted
        for d in 1..=31 {
            let day_str = format!("202609{:02}", d);
            let health_dir = dir.path().join("chronicle").join(&day_str).join("health");
            fs::create_dir_all(&health_dir).unwrap();
            let row = serde_json::json!({
                "event": "index.attempt",
                "ts": now.as_millisecond() - 1000,
                "path": format!("{day_str}/note.md"),
                "outcome": "indexed"
            });
            fs::write(health_dir.join("think.jsonl"), row.to_string()).unwrap();
        }

        // Add extra health file with progression, equal timestamps, malformed line, missing path
        let h_dir = dir.path().join("chronicle").join("20261005").join("health");
        fs::create_dir_all(&h_dir).unwrap();
        let ts = window_start_ms + 10_000;
        let rows = [
            serde_json::json!({"event":"index.attempt","ts":ts,"path":"20261005/a.md","outcome":"failed"}).to_string(),
            serde_json::json!({"event":"index.attempt","ts":ts,"path":"20261005/a.md","outcome":"declined"}).to_string(),
            serde_json::json!({"event":"index.attempt","ts":ts + 1,"path":"20261005/b.md","outcome":"indexed"}).to_string(),
            serde_json::json!({"event":"index.attempt","ts":ts + 2,"path":"20261005/b.md","outcome":"excluded"}).to_string(),
            serde_json::json!({"event":"index.attempt","ts":ts + 3,"path":"","outcome":"failed"}).to_string(), // missing path
            "not json line".to_string(), // malformed
        ];
        fs::write(h_dir.join("extra.jsonl"), rows.join("\n")).unwrap();

        // Unreadable sibling health file (directory instead of file)
        fs::create_dir_all(h_dir.join("unreadable.jsonl")).unwrap();

        // Talents directory
        let talents_dir = dir.path().join("talents");
        fs::create_dir_all(&talents_dir).unwrap();
        // 8-character stem inside window
        let talent_row = serde_json::json!({
            "index_attempts": [{
                "ts": ts + 4,
                "path": "20261005/talent_note.md",
                "outcome": "indexed"
            }]
        });
        fs::write(talents_dir.join("20261005.jsonl"), talent_row.to_string()).unwrap();
        // 8-character stem outside window date (e.g. 19700101.jsonl), but attempt timestamp inside window
        let old_talent_row = serde_json::json!({
            "index_attempts": [{
                "ts": ts + 5,
                "path": "20261005/old_stem_note.md",
                "outcome": "failed"
            }]
        });
        fs::write(
            talents_dir.join("19700101.jsonl"),
            old_talent_row.to_string(),
        )
        .unwrap();
        // Non-8-char stem (ignored)
        let ignored_talent = serde_json::json!({
            "index_attempts": [{
                "ts": ts + 6,
                "path": "20261005/ignored.md",
                "outcome": "failed"
            }]
        });
        fs::write(
            talents_dir.join("toolongname.jsonl"),
            ignored_talent.to_string(),
        )
        .unwrap();
        fs::write(talents_dir.join("notes.txt"), "ignored").unwrap();

        let cold = read_indexing_observations(dir.path(), now);
        let reference = read_indexing_observations_reference(dir.path(), now);
        let cached = read_indexing_observations(dir.path(), now);

        assert_eq!(cold, reference);
        assert_eq!(cached, reference);

        // Verify eight-char talent stem was read
        assert!(
            cold.outcomes
                .iter()
                .any(|o| o.identity == "20261005/old_stem_note.md")
        );
        // Verify longer stem was ignored
        assert!(
            !cold
                .outcomes
                .iter()
                .any(|o| o.identity == "20261005/ignored.md")
        );
    }

    #[test]
    fn mutation_visibility_on_next_call_compared_to_reference() {
        let dir = tempdir().unwrap();
        setup_utc_journal(dir.path());
        let now = utc(2026, 10, 5, 12, 0, 0);
        let ts = now.as_millisecond() - 5000;

        let health_dir = dir.path().join("chronicle").join("20261005").join("health");
        fs::create_dir_all(&health_dir).unwrap();
        let log_file = health_dir.join("think.jsonl");

        let row1 = serde_json::json!({"event":"index.attempt","ts":ts,"path":"20261005/1.md","outcome":"indexed"});
        fs::write(&log_file, format!("{}\n", row1)).unwrap();

        let obs1 = read_indexing_observations(dir.path(), now);
        let ref1 = read_indexing_observations_reference(dir.path(), now);
        assert_eq!(obs1, ref1);

        // a. Append
        let row2 = serde_json::json!({"event":"index.attempt","ts":ts + 1,"path":"20261005/2.md","outcome":"failed"});
        let mut f = fs::OpenOptions::new().append(true).open(&log_file).unwrap();
        use std::io::Write;
        writeln!(f, "{}", row2).unwrap();
        drop(f);

        let obs2 = read_indexing_observations(dir.path(), now);
        let ref2 = read_indexing_observations_reference(dir.path(), now);
        assert_eq!(obs2, ref2);
        assert!(obs2.outcomes.iter().any(|o| o.identity == "20261005/2.md"));

        // b. Same-length rewrite with mtime restored
        let meta_before = fs::metadata(&log_file).unwrap();
        let len_before = meta_before.len();
        let mtime_before = meta_before.modified().unwrap();
        // Write exactly same byte length:
        let row_replacement = serde_json::json!({"event":"index.attempt","ts":ts + 2,"path":"20261005/3.md","outcome":"failed"});
        let mut rep_bytes = row_replacement.to_string().into_bytes();
        rep_bytes.push(b'\n');
        if rep_bytes.len() < len_before as usize {
            rep_bytes.resize(len_before as usize - 1, b' ');
            rep_bytes.push(b'\n');
        } else {
            rep_bytes.truncate(len_before as usize);
            if let Some(last) = rep_bytes.last_mut() {
                *last = b'\n';
            }
        }
        fs::write(&log_file, &rep_bytes).unwrap();
        filetime::set_file_mtime(
            &log_file,
            filetime::FileTime::from_system_time(mtime_before),
        )
        .unwrap();

        let obs_rewrite = read_indexing_observations(dir.path(), now);
        let ref_rewrite = read_indexing_observations_reference(dir.path(), now);
        if obs_rewrite != ref_rewrite {
            // If ctime did not move on this platform, advance expiry clock to parsed_at + 60s
            let future = expiry_now() + Duration::from_secs(61);
            EXPIRY_OVERRIDE.with(|c| *c.borrow_mut() = Some(future));
            let obs_expired = read_indexing_observations(dir.path(), now);
            assert_eq!(obs_expired, ref_rewrite);
            EXPIRY_OVERRIDE.with(|c| *c.borrow_mut() = None);
        } else {
            assert_eq!(obs_rewrite, ref_rewrite);
        }

        // c. Atomic replace
        let temp_write = health_dir.join("temp.jsonl");
        let row_atomic = serde_json::json!({"event":"index.attempt","ts":ts + 3,"path":"20261005/atomic.md","outcome":"indexed"});
        fs::write(&temp_write, format!("{}\n", row_atomic)).unwrap();
        fs::rename(&temp_write, &log_file).unwrap();

        let obs_atomic = read_indexing_observations(dir.path(), now);
        let ref_atomic = read_indexing_observations_reference(dir.path(), now);
        assert_eq!(obs_atomic, ref_atomic);
        assert!(
            obs_atomic
                .outcomes
                .iter()
                .any(|o| o.identity == "20261005/atomic.md")
        );

        // d. Truncation
        fs::write(&log_file, "").unwrap();
        let obs_trunc = read_indexing_observations(dir.path(), now);
        let ref_trunc = read_indexing_observations_reference(dir.path(), now);
        assert_eq!(obs_trunc, ref_trunc);
        assert!(obs_trunc.outcomes.is_empty());

        // e. Deletion
        fs::remove_file(&log_file).unwrap();
        let obs_del = read_indexing_observations(dir.path(), now);
        let ref_del = read_indexing_observations_reference(dir.path(), now);
        assert_eq!(obs_del, ref_del);

        // f. New file
        let new_file = health_dir.join("brand_new.jsonl");
        let row_new = serde_json::json!({"event":"index.attempt","ts":ts + 4,"path":"20261005/new.md","outcome":"failed"});
        fs::write(&new_file, format!("{}\n", row_new)).unwrap();
        let obs_new = read_indexing_observations(dir.path(), now);
        let ref_new = read_indexing_observations_reference(dir.path(), now);
        assert_eq!(obs_new, ref_new);
        assert!(
            obs_new
                .outcomes
                .iter()
                .any(|o| o.identity == "20261005/new.md")
        );

        // g. File replaced by directory
        fs::remove_file(&new_file).unwrap();
        fs::create_dir_all(&new_file).unwrap();
        let obs_dir = read_indexing_observations(dir.path(), now);
        let ref_dir = read_indexing_observations_reference(dir.path(), now);
        assert_eq!(obs_dir, ref_dir);
        assert!(obs_dir.diagnostics.partial);
        assert!(
            obs_dir
                .diagnostics
                .lines
                .iter()
                .any(|l| l.contains("unreadable health log"))
        );

        // h. Malformed repaired
        fs::remove_dir_all(&new_file).unwrap();
        let mal_file = health_dir.join("mal.jsonl");
        fs::write(&mal_file, "corrupt json line\n").unwrap();
        let obs_mal = read_indexing_observations(dir.path(), now);
        assert!(obs_mal.diagnostics.partial);
        assert!(
            obs_mal
                .diagnostics
                .lines
                .iter()
                .any(|l| l.contains("malformed lines"))
        );

        // Repair malformed
        fs::write(&mal_file, serde_json::json!({"event":"index.attempt","ts":ts + 5,"path":"20261005/repaired.md","outcome":"indexed"}).to_string()).unwrap();
        let obs_repaired = read_indexing_observations(dir.path(), now);
        let ref_repaired = read_indexing_observations_reference(dir.path(), now);
        assert_eq!(obs_repaired, ref_repaired);
        assert!(!obs_repaired.diagnostics.partial);
    }

    #[test]
    fn later_now_and_zone_change_without_reparse() {
        let dir = tempdir().unwrap();
        setup_utc_journal(dir.path());
        let now1 = utc(2026, 10, 5, 12, 0, 0);
        let (start1, end1) = indexing_window_bounds(dir.path(), now1);

        let health_dir = dir.path().join("chronicle").join("20261005").join("health");
        fs::create_dir_all(&health_dir).unwrap();

        let row1 = serde_json::json!({"event":"index.attempt","ts":start1 + 1000,"path":"20261005/1.md","outcome":"indexed"});
        let row2 = serde_json::json!({"event":"index.attempt","ts":end1 + 5000,"path":"20261005/2.md","outcome":"failed"});
        fs::write(
            health_dir.join("think.jsonl"),
            format!("{}\n{}\n", row1, row2),
        )
        .unwrap();

        let obs1 = read_indexing_observations(dir.path(), now1);
        assert_eq!(obs1.outcomes.len(), 1);
        assert_eq!(obs1.outcomes[0].identity, "20261005/1.md");

        let p_count0 = PARSE_COUNTER.with(|c| c.get());

        // Later now includes row2
        let now2 = utc(2026, 10, 5, 12, 1, 0);
        let obs2 = read_indexing_observations(dir.path(), now2);
        assert_eq!(obs2.outcomes.len(), 2);
        assert_eq!(PARSE_COUNTER.with(|c| c.get()), p_count0);

        // Zone change without observation file rewrite
        let config_dir = dir.path().join("config");
        fs::write(
            config_dir.join("journal.json"),
            r#"{"identity":{"timezone":"America/New_York"}}"#,
        )
        .unwrap();
        let obs3 = read_indexing_observations(dir.path(), now2);
        assert_eq!(PARSE_COUNTER.with(|c| c.get()), p_count0);
        assert_eq!(
            obs3.diagnostics.window_start_ms,
            start1 + 4 * 60 * 60 * 1000
        );
        assert_eq!(obs3.diagnostics.window_end_ms, now2.as_millisecond());
        assert_eq!(obs3.outcomes.len(), 1);
        assert_eq!(obs3.outcomes[0].identity, "20261005/2.md");
        let zone = solstone_core_journal_config::owner_zone(dir.path());
        assert_eq!(zone.iana_name(), Some("America/New_York"));
    }

    #[test]
    #[cfg(unix)]
    fn cached_success_cannot_hide_lost_read_access_with_matching_stamp() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        setup_utc_journal(dir.path());
        let now = utc(2026, 10, 5, 12, 0, 0);
        let health = dir.path().join("chronicle/20261005/health");
        fs::create_dir_all(&health).unwrap();
        let path = health.join("think.jsonl");
        fs::write(&path, serde_json::json!({"event":"index.attempt","ts":now.as_millisecond(),"path":"20261005/a.md","outcome":"indexed"}).to_string()).unwrap();
        assert_eq!(
            read_indexing_observations(dir.path(), now).outcomes.len(),
            1
        );
        let original_permissions = fs::metadata(&path).unwrap().permissions();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o0)).unwrap();
        if fs::File::open(&path).is_ok() {
            // A privileged test process bypasses mode-bit read denial.
            fs::set_permissions(&path, original_permissions).unwrap();
            return;
        }
        // Model platforms where access changes do not change the identity stamp.
        with_observation_cache(|cache| {
            cache
                .get_mut(dir.path())
                .unwrap()
                .get_mut(&path)
                .unwrap()
                .stamp = file_stamp(&path).unwrap()
        });
        let denied = read_indexing_observations(dir.path(), now);
        assert!(denied.diagnostics.partial);
        assert!(denied.outcomes.is_empty());
        assert_eq!(
            denied,
            read_indexing_observations_reference(dir.path(), now)
        );
        fs::set_permissions(&path, original_permissions).unwrap();
        let repaired = read_indexing_observations(dir.path(), now);
        assert_eq!(repaired.outcomes.len(), 1);
        assert!(!repaired.diagnostics.partial);
    }

    #[test]
    fn retained_existing_journals_have_a_fixed_bound() {
        let roots: Vec<_> = (0..OBSERVATION_JOURNAL_CAP + 2)
            .map(|_| tempdir().unwrap())
            .collect();
        let mut cache = ObservationCache::new();
        for root in &roots {
            cached_files_mut(&mut cache, root.path());
            assert!(cache.len() <= OBSERVATION_JOURNAL_CAP);
            assert!(cache.contains_key(root.path()));
        }
        assert!(roots.iter().all(|root| root.path().is_dir()));
        assert_eq!(cache.len(), OBSERVATION_JOURNAL_CAP);
    }

    #[test]
    fn expiry_clock_at_60s_reparses_before_serving() {
        let dir = tempdir().unwrap();
        setup_utc_journal(dir.path());
        let now = utc(2026, 10, 5, 12, 0, 0);
        let health_dir = dir.path().join("chronicle").join("20261005").join("health");
        fs::create_dir_all(&health_dir).unwrap();
        let path = health_dir.join("think.jsonl");
        fs::write(&path, serde_json::json!({"event":"index.attempt","ts":now.as_millisecond(),"path":"20261005/old.md","outcome":"indexed"}).to_string()).unwrap();

        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        EXPIRY_OVERRIDE.with(|c| *c.borrow_mut() = Some(t0));

        let _ = read_indexing_observations(dir.path(), now);
        let p0 = PARSE_COUNTER.with(|c| c.get());

        fs::write(&path, serde_json::json!({"event":"index.attempt","ts":now.as_millisecond(),"path":"20261005/new.md","outcome":"indexed"}).to_string()).unwrap();
        // Emulate an identical stamp on filesystems with weaker change metadata.
        with_observation_cache(|cache| {
            cache
                .get_mut(dir.path())
                .unwrap()
                .get_mut(&path)
                .unwrap()
                .stamp = file_stamp(&path).unwrap()
        });

        // Hit at t0 + 59s: no reparse
        EXPIRY_OVERRIDE.with(|c| *c.borrow_mut() = Some(t0 + Duration::from_secs(59)));
        let hit = read_indexing_observations(dir.path(), now);
        assert_eq!(hit.outcomes[0].identity, "20261005/old.md");
        assert_eq!(PARSE_COUNTER.with(|c| c.get()), p0);

        // Call at t0 + 60s: reparses before serving
        EXPIRY_OVERRIDE.with(|c| *c.borrow_mut() = Some(t0 + Duration::from_secs(60)));
        let expired = read_indexing_observations(dir.path(), now);
        assert_eq!(expired.outcomes[0].identity, "20261005/new.md");
        assert!(PARSE_COUNTER.with(|c| c.get()) > p0);

        EXPIRY_OVERRIDE.with(|c| *c.borrow_mut() = None);
    }

    #[test]
    fn paused_parse_returns_pre_mutation_and_next_read_returns_post_mutation() {
        let dir = tempdir().unwrap();
        setup_utc_journal(dir.path());
        let now = utc(2026, 10, 5, 12, 0, 0);
        let health_dir = dir.path().join("chronicle").join("20261005").join("health");
        fs::create_dir_all(&health_dir).unwrap();
        let file_path = health_dir.join("think.jsonl");

        let row1 = serde_json::json!({"event":"index.attempt","ts":now.as_millisecond() - 100,"path":"20261005/pre.md","outcome":"failed"});
        fs::write(&file_path, format!("{}\n", row1)).unwrap();

        let mutate_target = file_path.clone();
        let row2 = serde_json::json!({"event":"index.attempt","ts":now.as_millisecond() - 50,"path":"20261005/post.md","outcome":"indexed"});
        PAUSE_HOOK.with(|c| {
            *c.borrow_mut() = Some(Box::new(move || {
                let mut f = fs::OpenOptions::new()
                    .append(true)
                    .open(&mutate_target)
                    .unwrap();
                use std::io::Write;
                writeln!(f, "{}", row2).unwrap();
            }));
        });

        // The paused call returns pre-mutation contents because in-memory parse was already built
        let obs_during = read_indexing_observations(dir.path(), now);
        assert!(
            obs_during
                .outcomes
                .iter()
                .any(|o| o.identity == "20261005/pre.md")
        );
        assert!(
            !obs_during
                .outcomes
                .iter()
                .any(|o| o.identity == "20261005/post.md")
        );

        // Because post-stat != pre-stat, it was not installed in cache; next read gets post-mutation contents
        let obs_after = read_indexing_observations(dir.path(), now);
        assert!(
            obs_after
                .outcomes
                .iter()
                .any(|o| o.identity == "20261005/post.md")
        );
    }

    #[test]
    fn two_temp_journals_with_same_relative_names_stay_distinct() {
        let dir_a = tempdir().unwrap();
        let dir_b = tempdir().unwrap();
        setup_utc_journal(dir_a.path());
        setup_utc_journal(dir_b.path());
        let now = utc(2026, 10, 5, 12, 0, 0);

        let h_a = dir_a
            .path()
            .join("chronicle")
            .join("20261005")
            .join("health");
        let h_b = dir_b
            .path()
            .join("chronicle")
            .join("20261005")
            .join("health");
        fs::create_dir_all(&h_a).unwrap();
        fs::create_dir_all(&h_b).unwrap();

        fs::write(h_a.join("think.jsonl"), serde_json::json!({"event":"index.attempt","ts":now.as_millisecond(),"path":"20261005/a.md","outcome":"failed"}).to_string()).unwrap();
        fs::write(h_b.join("think.jsonl"), serde_json::json!({"event":"index.attempt","ts":now.as_millisecond(),"path":"20261005/b.md","outcome":"indexed"}).to_string()).unwrap();

        let obs_a1 = read_indexing_observations(dir_a.path(), now);
        let obs_b1 = read_indexing_observations(dir_b.path(), now);
        let obs_a2 = read_indexing_observations(dir_a.path(), now);
        let obs_b2 = read_indexing_observations(dir_b.path(), now);

        assert_eq!(obs_a1, obs_a2);
        assert_eq!(obs_b1, obs_b2);
        assert!(
            obs_a1
                .outcomes
                .iter()
                .any(|o| o.identity == "20261005/a.md")
        );
        assert!(
            !obs_a1
                .outcomes
                .iter()
                .any(|o| o.identity == "20261005/b.md")
        );
        assert!(
            obs_b1
                .outcomes
                .iter()
                .any(|o| o.identity == "20261005/b.md")
        );
        assert!(
            !obs_b1
                .outcomes
                .iter()
                .any(|o| o.identity == "20261005/a.md")
        );
    }

    #[test]
    fn second_read_of_unchanged_files_does_not_increment_parse_counter() {
        let dir = tempdir().unwrap();
        setup_utc_journal(dir.path());
        let now = utc(2026, 10, 5, 12, 0, 0);
        let health_dir = dir.path().join("chronicle").join("20261005").join("health");
        fs::create_dir_all(&health_dir).unwrap();
        fs::write(health_dir.join("think.jsonl"), serde_json::json!({"event":"index.attempt","ts":now.as_millisecond(),"path":"20261005/1.md","outcome":"indexed"}).to_string()).unwrap();

        let _ = read_indexing_observations(dir.path(), now);
        let p0 = PARSE_COUNTER.with(|c| c.get());

        let _ = read_indexing_observations(dir.path(), now);
        let p1 = PARSE_COUNTER.with(|c| c.get());

        assert_eq!(p0, p1);
    }

    #[test]
    fn selection_prune_drops_files_outside_newest_30_and_deleted_journals() {
        let dir = tempdir().unwrap();
        setup_utc_journal(dir.path());
        let now = utc(2026, 10, 5, 12, 0, 0);

        // 30 chronicle days: 20260902..=20261001
        for d in 2..=31 {
            let day_str = format!("202609{:02}", d);
            let health_dir = dir.path().join("chronicle").join(&day_str).join("health");
            fs::create_dir_all(&health_dir).unwrap();
            fs::write(health_dir.join("think.jsonl"), serde_json::json!({"event":"index.attempt","ts":now.as_millisecond(),"path":format!("{day_str}/1.md"),"outcome":"indexed"}).to_string()).unwrap();
        }

        let _ = read_indexing_observations(dir.path(), now);
        let files_before = observation_cache_files(dir.path());
        assert_eq!(files_before.len(), 30);

        // Add 20261005 (newer day) -> 20260902 is now 31st and pushed out of newest 30
        let new_day = dir.path().join("chronicle").join("20261005").join("health");
        fs::create_dir_all(&new_day).unwrap();
        fs::write(new_day.join("think.jsonl"), serde_json::json!({"event":"index.attempt","ts":now.as_millisecond(),"path":"20261005/1.md","outcome":"indexed"}).to_string()).unwrap();

        let _ = read_indexing_observations(dir.path(), now);
        let files_after = observation_cache_files(dir.path());
        assert_eq!(files_after.len(), 30);
        let oldest_file = dir
            .path()
            .join("chronicle")
            .join("20260902")
            .join("health")
            .join("think.jsonl");
        assert!(!files_after.contains(&oldest_file));

        // Create a disposable second journal
        let dir2 = tempdir().unwrap();
        setup_utc_journal(dir2.path());
        let h2 = dir2
            .path()
            .join("chronicle")
            .join("20261005")
            .join("health");
        fs::create_dir_all(&h2).unwrap();
        fs::write(h2.join("think.jsonl"), serde_json::json!({"event":"index.attempt","ts":now.as_millisecond(),"path":"20261005/2.md","outcome":"indexed"}).to_string()).unwrap();

        let _ = read_indexing_observations(dir2.path(), now);
        assert!(observation_cache_journals().contains(&dir2.path().to_path_buf()));

        // Delete dir2
        let p2 = dir2.path().to_path_buf();
        drop(dir2);

        // Next read of dir1 drops deleted dir2 root from cache
        let _ = read_indexing_observations(dir.path(), now);
        assert!(!observation_cache_journals().contains(&p2));
    }
}
