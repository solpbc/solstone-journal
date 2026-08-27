// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
#[cfg(not(target_os = "ios"))]
use std::fmt::Write as _;
use std::fs::{self, DirBuilder};
#[cfg(not(target_os = "ios"))]
use std::io::Write as _;
use std::io::{self, Read};
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};

#[cfg(not(target_os = "ios"))]
use chrono::Local;
use chrono::{NaiveDate, SecondsFormat, Utc};
use serde_json::{Value, json};
use solstone_core_facets::{append_action_log, hold_facet_trust_lock, write_news_file};
#[cfg(not(target_os = "ios"))]
use solstone_core_import_sources::ImportSourcesError;
#[cfg(not(target_os = "ios"))]
use solstone_core_import_sources::archive::{
    ArchiveMergeOptions, ArchiveMergeResult, ArchivePlan, FullReindexRequester, ReindexStatus,
    RetryDisposition, merge_journal_archive, plan_journal_archive,
};
#[cfg(not(target_os = "ios"))]
use solstone_core_indexer_query::{
    CountsResponse, IndexAccessError, Order, SearchHit, SearchRequest, search,
};
#[cfg(not(target_os = "ios"))]
use solstone_core_indexer_store::db::reset_index;
#[cfg(not(target_os = "ios"))]
use solstone_core_indexer_store::scan::{
    RescanFileStatus, rebuild_edges, rescan_file, scan_journal,
};
use solstone_core_journal_archive::{
    ArchiveSource, DayWindow, EncodeArchiveRequest, ExplicitArchiveOutputRequest,
    acquire_explicit_output_target, publish_archive,
};
use solstone_core_journal_io::{
    AtomicWriteOptions, append_jsonl, atomic_replace, write_bytes_exclusive,
};

use crate::Outcome;
use crate::layout::resolve_current_journal;

const EXIT_FAILED: u8 = 1;
const EXIT_USAGE: u8 = 64;
const EXIT_DATA: u8 = 65;
const EXIT_UNAVAILABLE: u8 = 69;
const EXIT_IO: u8 = 74;
#[cfg(not(target_os = "ios"))]
const EXIT_TEMPFAIL: u8 = 75;

pub(crate) fn dispatch(token: &str, args: &[OsString]) -> Outcome {
    match token {
        "indexer" => indexer(args),
        "archive export" => archive_export(args),
        "archive merge" => archive_merge(args),
        "facet doctor" => facet_doctor(args),
        "facet merge" => facet_merge(args),
        "news write" => news_write(args),
        _ => failure(token, "unknown local operation", EXIT_USAGE),
    }
}

#[cfg(not(target_os = "ios"))]
#[derive(Default)]
struct IndexerQuery {
    requested: bool,
    text: String,
    day: Option<String>,
    day_from: Option<String>,
    day_to: Option<String>,
    facet: Option<String>,
    agent: Option<String>,
    stream: Option<String>,
    limit: usize,
    offset: usize,
    top: usize,
}

#[cfg(not(target_os = "ios"))]
impl IndexerQuery {
    fn with_defaults() -> Self {
        Self {
            limit: 10,
            top: 5,
            ..Self::default()
        }
    }
}

#[cfg(target_os = "ios")]
fn indexer(_args: &[OsString]) -> Outcome {
    failure("indexer", "unavailable on iOS", EXIT_UNAVAILABLE)
}

#[cfg(not(target_os = "ios"))]
fn indexer(args: &[OsString]) -> Outcome {
    const HELP: &str = "Usage: journal indexer [--reset] [--rebuild-edges] [--rescan | --rescan-full | --rescan-file PATH] [-q [QUERY]] [--day DAY] [--day-from DAY] [--day-to DAY] [--facet FACET] [--agent AGENT] [--stream STREAM] [--limit N] [--offset N] [--top N]\n";

    let mut reset = false;
    let mut rebuild = false;
    let mut rescan = false;
    let mut rescan_full = false;
    let mut rescan_file_path: Option<PathBuf> = None;
    let mut query = IndexerQuery::with_defaults();
    let mut index = 0;
    while index < args.len() {
        match args[index].to_str() {
            Some("--reset") if !reset => {
                reset = true;
                index += 1;
            }
            Some("--rebuild-edges") if !rebuild => {
                rebuild = true;
                index += 1;
            }
            Some("--rescan") if !rescan => {
                rescan = true;
                index += 1;
            }
            Some("--rescan-full") if !rescan_full => {
                rescan_full = true;
                index += 1;
            }
            Some("--rescan-file") if rescan_file_path.is_none() => {
                let Some(path) = args.get(index + 1) else {
                    return usage("indexer", "--rescan-file requires PATH");
                };
                rescan_file_path = Some(PathBuf::from(path));
                index += 2;
            }
            Some("-q" | "--query") if !query.requested => {
                query.requested = true;
                if let Some(value) = args.get(index + 1).and_then(|value| value.to_str())
                    && !value.starts_with('-')
                {
                    query.text = value.to_owned();
                    index += 2;
                } else {
                    index += 1;
                }
            }
            Some(value) if value.starts_with("--query=") && !query.requested => {
                query.requested = true;
                query.text = value["--query=".len()..].to_owned();
                index += 1;
            }
            Some("--day") if query.day.is_none() => {
                let Some(value) = utf8_value(args, index + 1) else {
                    return usage("indexer", "--day requires DAY");
                };
                query.day = Some(value.to_owned());
                index += 2;
            }
            Some("--day-from") if query.day_from.is_none() => {
                let Some(value) = utf8_value(args, index + 1) else {
                    return usage("indexer", "--day-from requires DAY");
                };
                query.day_from = Some(value.to_owned());
                index += 2;
            }
            Some("--day-to") if query.day_to.is_none() => {
                let Some(value) = utf8_value(args, index + 1) else {
                    return usage("indexer", "--day-to requires DAY");
                };
                query.day_to = Some(value.to_owned());
                index += 2;
            }
            Some("--facet") if query.facet.is_none() => {
                let Some(value) = utf8_value(args, index + 1) else {
                    return usage("indexer", "--facet requires FACET");
                };
                query.facet = Some(value.to_owned());
                index += 2;
            }
            Some("--agent" | "-a") if query.agent.is_none() => {
                let Some(value) = utf8_value(args, index + 1) else {
                    return usage("indexer", "--agent requires AGENT");
                };
                query.agent = Some(value.to_owned());
                index += 2;
            }
            Some("--stream") if query.stream.is_none() => {
                let Some(value) = utf8_value(args, index + 1) else {
                    return usage("indexer", "--stream requires STREAM");
                };
                query.stream = Some(value.to_owned());
                index += 2;
            }
            Some("--limit") => {
                let Some(value) = usize_value(args, index + 1) else {
                    return usage("indexer", "--limit requires a non-negative integer");
                };
                query.limit = value;
                index += 2;
            }
            Some("--offset") => {
                let Some(value) = usize_value(args, index + 1) else {
                    return usage("indexer", "--offset requires a non-negative integer");
                };
                query.offset = value;
                index += 2;
            }
            Some("--top") => {
                let Some(value) = usize_value(args, index + 1) else {
                    return usage("indexer", "--top requires a non-negative integer");
                };
                query.top = value;
                index += 2;
            }
            Some("--help" | "-h") if args.len() == 1 => return success(HELP.to_owned()),
            _ => return usage("indexer", "unexpected or duplicate argument"),
        }
    }

    if rescan_file_path.is_some() && (rescan || rescan_full) {
        return usage(
            "indexer",
            "--rescan-file cannot be combined with --rescan or --rescan-full",
        );
    }
    if !reset
        && !rebuild
        && !rescan
        && !rescan_full
        && rescan_file_path.is_none()
        && !query.requested
    {
        return success(HELP.to_owned());
    }

    let journal = match journal_root("indexer") {
        Ok(path) => path,
        Err(outcome) => return outcome,
    };
    let mut stdout = String::new();
    let mut stderr = String::new();

    if reset && let Err(error) = reset_index(&journal) {
        return failure("indexer", &format!("reset failed: {error}"), EXIT_IO);
    }

    if rebuild {
        match rebuild_edges(&journal) {
            Ok(report) => {
                for warning in report.warnings {
                    stderr.push_str(&format!("warning: {warning}\n"));
                }
                if report.failed > 0 {
                    return Outcome::LocalFailure {
                        stdout,
                        stderr,
                        exit: EXIT_IO,
                    };
                }
            }
            Err(error) => {
                return failure("indexer", &format!("edge rebuild failed: {error}"), EXIT_IO);
            }
        }
    }

    if let Some(path) = rescan_file_path {
        match rescan_file(&journal, &path) {
            Ok(RescanFileStatus::Indexed { warnings }) => {
                for warning in warnings {
                    stderr.push_str(&format!("warning: {warning}\n"));
                }
            }
            Ok(RescanFileStatus::Declined) => {
                return failure("indexer", "unsupported file", EXIT_UNAVAILABLE);
            }
            Err(error) => {
                return failure("indexer", &format!("rescan-file failed: {error}"), EXIT_IO);
            }
        }
    } else if rescan || rescan_full {
        match scan_journal(&journal, rescan_full) {
            Ok(report) => {
                for warning in report.warnings {
                    stderr.push_str(&format!("warning: {warning}\n"));
                }
                stdout.push_str(&format!(
                    "Indexed {} file(s), removed {}, skipped {}\n",
                    report.indexed, report.removed, report.skipped
                ));
                if rescan_full && !reset && !rebuild && report.edge_rows_inserted == 0 {
                    stdout.push_str("Zero edges indexed: edges are talent-derived, and the --rescan-full edge phase remains modification-time incremental — run journal indexer --rebuild-edges to force full edge re-extraction.\n");
                }
            }
            Err(error) => {
                return failure("indexer", &format!("scan failed: {error}"), EXIT_IO);
            }
        }
    }

    if query.requested {
        if query.text.is_empty() {
            if !stdout.is_empty() {
                print!("{stdout}");
                stdout.clear();
            }
            if !stderr.is_empty() {
                eprint!("{stderr}");
                stderr.clear();
            }
            if let Err((message, exit)) = run_interactive_indexer_query(&journal, &query) {
                return failure("indexer", &message, exit);
            }
        } else {
            match run_one_indexer_query(&journal, &query.text, &query) {
                Ok(output) => stdout.push_str(&output),
                Err((message, exit)) => {
                    stderr.push_str(&format!("indexer: {message}\n"));
                    return Outcome::LocalFailure {
                        stdout,
                        stderr,
                        exit,
                    };
                }
            }
        }
    }

    Outcome::LocalSuccess { stdout, stderr }
}

#[cfg(not(target_os = "ios"))]
fn utf8_value(args: &[OsString], index: usize) -> Option<&str> {
    args.get(index).and_then(|value| value.to_str())
}

#[cfg(not(target_os = "ios"))]
fn usize_value(args: &[OsString], index: usize) -> Option<usize> {
    utf8_value(args, index)?.parse().ok()
}

#[cfg(not(target_os = "ios"))]
fn run_interactive_indexer_query(
    journal: &Path,
    options: &IndexerQuery,
) -> Result<(), (String, u8)> {
    loop {
        print!("search> ");
        io::stdout()
            .flush()
            .map_err(|error| (format!("stdout write failed: {error}"), EXIT_IO))?;
        let mut query = String::new();
        let read = io::stdin()
            .read_line(&mut query)
            .map_err(|error| (format!("stdin read failed: {error}"), EXIT_IO))?;
        if read == 0 || query.trim().is_empty() {
            return Ok(());
        }
        let output = run_one_indexer_query(journal, query.trim(), options)?;
        print!("{output}");
        io::stdout()
            .flush()
            .map_err(|error| (format!("stdout write failed: {error}"), EXIT_IO))?;
    }
}

#[cfg(not(target_os = "ios"))]
fn run_one_indexer_query(
    journal: &Path,
    query: &str,
    options: &IndexerQuery,
) -> Result<String, (String, u8)> {
    let mut request = SearchRequest::new(query, Order::Relevance);
    request.limit = options.limit;
    request.offset = options.offset;
    request.day = options.day.clone();
    request.day_from = options.day_from.clone();
    request.day_to = options.day_to.clone();
    request.facet = options.facet.clone();
    request.agent = options.agent.clone();
    request.stream = options.stream.clone();
    request.counts = true;
    let response =
        search(journal, &request, Local::now().date_naive()).map_err(index_query_error)?;
    let counts = response.counts.unwrap_or_default();
    Ok(format_indexer_query(
        &counts,
        &response.results,
        options.offset,
        options.top,
    ))
}

#[cfg(not(target_os = "ios"))]
fn index_query_error(error: IndexAccessError) -> (String, u8) {
    let exit = if error.reason() == "index_locked" {
        EXIT_TEMPFAIL
    } else {
        EXIT_UNAVAILABLE
    };
    (error.to_string(), exit)
}

#[cfg(not(target_os = "ios"))]
fn format_indexer_query(
    counts: &CountsResponse,
    results: &[SearchHit],
    offset: usize,
    top: usize,
) -> String {
    let facets = top_count_column(&counts.facets, top);
    let agents = top_count_column(&counts.agents, top);
    let days = top_day_column(&counts.days, top);
    let mut output = String::new();
    writeln!(output, "Total: {} chunks\n", counts.total).expect("write string");
    writeln!(output, "{:<20} {:<20} {:<20}", "Facet", "Agent", "Day").expect("write string");
    writeln!(output, "{}", "-".repeat(60)).expect("write string");
    for index in 0..facets.len().max(agents.len()).max(days.len()) {
        writeln!(
            output,
            "{:<20} {:<20} {:<20}",
            facets.get(index).map(String::as_str).unwrap_or(""),
            agents.get(index).map(String::as_str).unwrap_or(""),
            days.get(index).map(String::as_str).unwrap_or("")
        )
        .expect("write string");
    }
    output.push('\n');

    if counts.total == 0 || results.is_empty() {
        output.push_str("No results found\n");
        return output;
    }
    writeln!(
        output,
        "Showing {}-{} of {} results\n",
        offset + 1,
        offset + results.len(),
        counts.total
    )
    .expect("write string");
    for (index, hit) in results.iter().enumerate() {
        let text = hit.text.replace('\n', " ");
        let mut snippet = text.chars().take(100).collect::<String>();
        if text.chars().count() > 100 {
            snippet.push_str("...");
        }
        let facet = if hit.metadata.facet.is_empty() {
            String::new()
        } else {
            format!(" ({})", hit.metadata.facet)
        };
        writeln!(
            output,
            "{}. {} {}{}: {}",
            offset + index + 1,
            hit.metadata.day,
            hit.metadata.agent,
            facet,
            snippet
        )
        .expect("write string");
    }
    output
}

#[cfg(not(target_os = "ios"))]
fn top_count_column(values: &BTreeMap<String, u64>, top: usize) -> Vec<String> {
    let mut entries = values.iter().collect::<Vec<_>>();
    entries.sort_by(|left, right| right.1.cmp(left.1).then_with(|| left.0.cmp(right.0)));
    format_count_column(entries, values.len(), top)
}

#[cfg(not(target_os = "ios"))]
fn top_day_column(values: &BTreeMap<String, u64>, top: usize) -> Vec<String> {
    let mut entries = values.iter().collect::<Vec<_>>();
    entries.sort_by(|left, right| right.0.cmp(left.0));
    format_count_column(entries, values.len(), top)
}

#[cfg(not(target_os = "ios"))]
fn format_count_column(entries: Vec<(&String, &u64)>, total: usize, top: usize) -> Vec<String> {
    let mut lines = entries
        .into_iter()
        .take(top)
        .map(|(name, count)| format!("{name} ({count})"))
        .collect::<Vec<_>>();
    if total > top {
        lines.push(format!("... +{} more", total - top));
    }
    lines
}

fn archive_export(args: &[OsString]) -> Outcome {
    let mut output: Option<PathBuf> = None;
    let mut quiet = false;
    let mut day: Option<String> = None;
    let mut from: Option<String> = None;
    let mut to: Option<String> = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].to_str() {
            Some("--out") if output.is_none() => {
                let Some(value) = args.get(index + 1) else {
                    return usage("archive export", "--out requires PATH");
                };
                output = Some(PathBuf::from(value));
                index += 2;
            }
            Some("--quiet") if !quiet => {
                quiet = true;
                index += 1;
            }
            Some("--day") if day.is_none() => {
                let Some(value) = args.get(index + 1).and_then(|value| value.to_str()) else {
                    return usage("archive export", "--day requires YYYYMMDD");
                };
                if !valid_day(value) {
                    return failure(
                        "archive export",
                        "DAY must be a real YYYYMMDD date",
                        EXIT_DATA,
                    );
                }
                day = Some(value.to_owned());
                index += 2;
            }
            Some("--from") if from.is_none() => {
                let Some(value) = args.get(index + 1).and_then(|value| value.to_str()) else {
                    return usage("archive export", "--from requires YYYYMMDD");
                };
                if !valid_day(value) {
                    return failure(
                        "archive export",
                        "FROM must be a real YYYYMMDD date",
                        EXIT_DATA,
                    );
                }
                from = Some(value.to_owned());
                index += 2;
            }
            Some("--to") if to.is_none() => {
                let Some(value) = args.get(index + 1).and_then(|value| value.to_str()) else {
                    return usage("archive export", "--to requires YYYYMMDD");
                };
                if !valid_day(value) {
                    return failure(
                        "archive export",
                        "TO must be a real YYYYMMDD date",
                        EXIT_DATA,
                    );
                }
                to = Some(value.to_owned());
                index += 2;
            }
            Some("--help" | "-h") if args.len() == 1 => {
                return success(
                    "Usage: journal archive export [--out PATH] [--quiet] [--day YYYYMMDD | --from YYYYMMDD [--to YYYYMMDD] | --to YYYYMMDD]\n".to_owned(),
                );
            }
            _ => return usage("archive export", "unexpected argument"),
        }
    }
    if day.is_some() && (from.is_some() || to.is_some()) {
        return usage(
            "archive export",
            "--day cannot be combined with --from or --to",
        );
    }
    if let (Some(from_day), Some(to_day)) = (&from, &to)
        && from_day > to_day
    {
        return usage("archive export", "--from must not be after --to");
    }
    let day_window = if day.is_some() || from.is_some() || to.is_some() {
        Some(DayWindow {
            from: day.clone().or(from),
            to: day.or(to),
        })
    } else {
        None
    };

    let journal = match journal_root("archive export") {
        Ok(path) => path,
        Err(outcome) => return outcome,
    };
    let source = match ArchiveSource::open(&journal) {
        Ok(source) => source,
        Err(error) => return archive_error("archive export", &error.to_string()),
    };
    let exported_at = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    let output = match output {
        Some(path) => path,
        None => match default_export_path(source.canonical_source(), &exported_at) {
            Ok(path) => path,
            Err(error) => return failure("archive export", &error, EXIT_IO),
        },
    };
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(error) => return failure("archive export", &error.to_string(), EXIT_IO),
    };
    if let Err(error) = reject_export_tree_output(source.canonical_source(), &output, &cwd) {
        return failure("archive export", &error, EXIT_DATA);
    }
    let target =
        match acquire_explicit_output_target(&ExplicitArchiveOutputRequest::new(output, cwd)) {
            Ok(target) => target,
            Err(error) => {
                return failure("archive export", &error.to_string(), target_exit(&error));
            }
        };
    let final_path = target.final_path().to_owned();
    let request = EncodeArchiveRequest {
        source: &source,
        solstone_version: env!("CARGO_PKG_VERSION"),
        exported_at: &exported_at,
        day_window,
    };
    if let Err(error) = publish_archive(&target, &request) {
        return archive_error("archive export", &error.to_string());
    }
    let mut stderr = String::new();
    if !quiet && !source.inventory().skipped_root_names().is_empty() {
        let skipped = source
            .inventory()
            .skipped_root_names()
            .iter()
            .map(|name| name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        stderr = format!("journal archive export: skipped top-level entries: {skipped}\n");
    }
    Outcome::LocalSuccess {
        stdout: if quiet {
            String::new()
        } else {
            format!("{}\n", final_path.display())
        },
        stderr,
    }
}

#[cfg(target_os = "ios")]
fn archive_merge(_args: &[OsString]) -> Outcome {
    failure("archive merge", "unavailable on iOS", EXIT_UNAVAILABLE)
}

#[cfg(not(target_os = "ios"))]
struct LocalScanReindex {
    journal: PathBuf,
}

#[cfg(not(target_os = "ios"))]
impl FullReindexRequester for LocalScanReindex {
    fn request_full_reindex(&self) -> Result<bool, String> {
        scan_journal(&self.journal, true)
            .map(|_| true)
            .map_err(|error| error.to_string())
    }
}

#[cfg(not(target_os = "ios"))]
fn archive_merge(args: &[OsString]) -> Outcome {
    let Some(source_arg) = args.first() else {
        return usage("archive merge", "SOURCE is required");
    };
    if source_arg == OsStr::new("--help") || source_arg == OsStr::new("-h") {
        return if args.len() == 1 {
            success("Usage: journal archive merge SOURCE [--dry-run] [--json]\n".to_owned())
        } else {
            usage("archive merge", "unexpected argument")
        };
    }
    let source = PathBuf::from(source_arg);
    let mut dry_run = false;
    let mut json_output = false;
    for arg in &args[1..] {
        match arg.to_str() {
            Some("--dry-run") if !dry_run => dry_run = true,
            Some("--json") if !json_output => json_output = true,
            _ => return usage("archive merge", "unexpected argument"),
        }
    }
    let journal = match journal_root("archive merge") {
        Ok(path) => path,
        Err(outcome) => return outcome,
    };
    let source = match regular_archive_file(&source) {
        Ok(path) => path,
        Err(error) => return failure("archive merge", &error, EXIT_DATA),
    };
    if dry_run {
        return match plan_journal_archive(&source) {
            Ok(plan) => archive_merge_plan(&plan, &source, json_output),
            Err(error) => archive_merge_json(Err(error), &source, true, json_output),
        };
    }
    let options = ArchiveMergeOptions {
        working_root: journal.join("imports").join("archive-merge-work"),
        ..ArchiveMergeOptions::default()
    };
    let reindexer = LocalScanReindex {
        journal: journal.clone(),
    };
    archive_merge_json(
        merge_journal_archive(&source, &journal, &options, Some(&reindexer)),
        &source,
        false,
        json_output,
    )
}

#[cfg(not(target_os = "ios"))]
fn regular_archive_file(source: &Path) -> Result<PathBuf, String> {
    let metadata = fs::symlink_metadata(source).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            format!("SOURCE does not exist: {}", source.display())
        } else {
            error.to_string()
        }
    })?;
    if !metadata.file_type().is_file() {
        return Err("SOURCE must be a regular file".to_owned());
    }
    fs::canonicalize(source).map_err(|error| error.to_string())
}

fn facet_doctor(args: &[OsString]) -> Outcome {
    let (fix, merge) = match args {
        [] => (false, false),
        [arg] if arg == OsStr::new("--fix") => (true, false),
        [first, second]
            if (first == OsStr::new("--fix") && second == OsStr::new("--merge"))
                || (first == OsStr::new("--merge") && second == OsStr::new("--fix")) =>
        {
            (true, true)
        }
        [arg] if arg == OsStr::new("--merge") => {
            return usage("facet doctor", "--merge requires --fix");
        }
        [arg] if arg == OsStr::new("--help") || arg == OsStr::new("-h") => {
            return success(
                "Usage: journal facet doctor [--fix [--merge] | --merge --fix]\n".to_owned(),
            );
        }
        _ => return usage("facet doctor", "unexpected argument"),
    };
    let journal = match journal_root("facet doctor") {
        Ok(path) => path,
        Err(outcome) => return outcome,
    };
    let mut orphans = match orphan_facets(&journal) {
        Ok(orphans) => orphans,
        Err(error) => return failure("facet doctor", &error, EXIT_DATA),
    };
    if orphans.is_empty() {
        return success("No orphan facets found.\n".to_owned());
    }
    if !fix {
        let mut stdout = String::from("Orphan facets:\n");
        for slug in &orphans {
            stdout.push_str(&format!("- {slug}\n"));
        }
        stdout.push_str(&format!(
            "{} orphan facet(s) found. Run with --fix to register them.\n",
            orphans.len()
        ));
        let groups = group_orphan_facets(&orphans);
        let variants = groups
            .values()
            .filter(|members| members.len() > 1)
            .collect::<Vec<_>>();
        if !variants.is_empty() {
            stdout.push_str("\nName-variant groups:\n");
            for members in variants {
                let destination = &members[0];
                stdout.push_str(&format!("- {} -> {destination}\n", members.join(", ")));
            }
            stdout.push_str(
                "Run with --fix --merge to collapse name variants before registering them.\n",
            );
        }
        return success(stdout);
    }
    let _lock = match hold_facet_trust_lock(&journal) {
        Ok(lock) => lock,
        Err(error) => return failure("facet doctor", &error.to_string(), EXIT_IO),
    };
    orphans = match orphan_facets(&journal) {
        Ok(orphans) => orphans,
        Err(error) => return failure("facet doctor", &error, EXIT_DATA),
    };
    if orphans.is_empty() {
        return success("No orphan facets found.\n".to_owned());
    }
    if merge {
        #[cfg(not(target_os = "ios"))]
        {
            return facet_doctor_fix_merge(&journal, &orphans);
        }
        #[cfg(target_os = "ios")]
        {
            return failure(
                "facet doctor",
                "facet merging unavailable on iOS",
                EXIT_UNAVAILABLE,
            );
        }
    }
    let transaction = transaction_id();
    let mut repaired = Vec::new();
    for slug in &orphans {
        if let Err(error) = adopt_orphan_facet(&journal, slug, &transaction) {
            return failure("facet doctor", &error, EXIT_IO);
        }
        repaired.push(slug);
    }
    let mut stdout = String::from("Repaired orphan facets:\n");
    for slug in repaired {
        stdout.push_str(&format!("- {slug}\n"));
    }
    stdout.push_str(&format!(
        "{} orphan facet(s) repaired. Run 'journal indexer --rescan-full' to refresh the index.\n",
        orphans.len()
    ));
    success(stdout)
}

fn adopt_orphan_facet(journal: &Path, slug: &str, transaction: &str) -> Result<(), String> {
    let title = title_case_slug(slug);
    let declaration = journal.join("facets").join(slug).join("facet.json");
    let mut body = serde_json::to_vec_pretty(&json!({
        "title": title,
        "description": "",
        "color": "#667eea",
        "emoji": "📦"
    }))
    .map_err(|error| error.to_string())?;
    body.push(b'\n');
    write_bytes_exclusive(
        &declaration,
        &body,
        AtomicWriteOptions { mode: Some(0o600) },
    )
    .map_err(|error| error.to_string())?;
    let audit = journal.join("logs/facet-heals.jsonl");
    if let Err(error) = append_jsonl(
        &audit,
        &json!({
            "transaction_id": transaction,
            "facet": slug,
            "action": "facet_heal",
            "params": {"title": title}
        }),
    ) {
        let _ = fs::remove_file(&declaration);
        return Err(error.to_string());
    }
    Ok(())
}

fn normalized_orphan_slug(slug: &str) -> String {
    slug.bytes()
        .filter(|byte| !matches!(byte, b'.' | b'_' | b'-'))
        .map(|byte| byte.to_ascii_lowercase() as char)
        .collect()
}

fn group_orphan_facets(orphans: &[String]) -> BTreeMap<String, Vec<String>> {
    let mut groups = BTreeMap::new();
    for slug in orphans {
        groups
            .entry(normalized_orphan_slug(slug))
            .or_insert_with(Vec::new)
            .push(slug.clone());
    }
    groups
}

#[cfg(not(target_os = "ios"))]
fn facet_doctor_fix_merge(journal: &Path, orphans: &[String]) -> Outcome {
    let transaction = transaction_id();
    let groups = group_orphan_facets(orphans);
    let mut merged = Vec::new();
    let mut collisions = Vec::new();
    let mut adopted = Vec::new();
    let mut failed = Vec::new();
    let mut failed_orphans = 0;
    let mut committed_failures = Vec::new();

    for members in groups.values() {
        // Sorted slug identity, not filesystem metadata, determines the destination and fold order.
        let destination = &members[0];
        if let Err(error) = adopt_orphan_facet(journal, destination, &transaction) {
            let unmerged = members[1..].join(", ");
            let detail = if unmerged.is_empty() {
                format!("{destination} (adoption failed: {error})")
            } else {
                format!("{destination} (adoption failed: {error}; {unmerged} were not merged)")
            };
            failed.push(detail);
            failed_orphans += members.len();
            continue;
        }
        adopted.push(destination.clone());
        let mut retained_origins = BTreeMap::<PathBuf, String>::new();
        for source in &members[1..] {
            // --merge is the caller's explicit consent for each derived merge audit record.
            match facet_merge_transaction_in_journal(journal, source, destination, true) {
                Err(outcome) => {
                    failed.push(format!(
                        "{source} -> {destination} (merge failed before commit: {})",
                        outcome_diagnostic(&outcome)
                    ));
                    failed_orphans += 1;
                }
                Ok(commit) => {
                    for path in &commit.report.regular_file_collisions {
                        let retained = retained_origins
                            .get(path)
                            .cloned()
                            .unwrap_or_else(|| destination.clone());
                        collisions.push(format!(
                            "{source} -> {destination}: {} (kept {retained})",
                            path.display()
                        ));
                    }
                    for path in commit.report.copied_regular_files {
                        retained_origins.insert(path, source.clone());
                    }
                    merged.push(format!("{source} -> {destination}"));
                    if let Some(outcome) = commit.post_commit_failure {
                        committed_failures.push(format!(
                            "{source} -> {destination} ({})",
                            outcome_diagnostic(&outcome)
                        ));
                    }
                }
            }
        }
    }

    let mut stdout = String::new();
    append_facet_doctor_section(&mut stdout, "Merged orphan facets", &merged);
    append_facet_doctor_section(&mut stdout, "Regular-file collisions", &collisions);
    append_facet_doctor_section(&mut stdout, "Adopted orphan facets", &adopted);
    append_facet_doctor_section(&mut stdout, "Failed orphan facets", &failed);
    append_facet_doctor_section(
        &mut stdout,
        "Committed merge maintenance failures",
        &committed_failures,
    );
    if !stdout.is_empty() {
        stdout.push('\n');
    }
    let repaired = adopted.len() + merged.len();
    if failed_orphans == 0 && committed_failures.is_empty() {
        stdout.push_str(&format!(
            "{repaired} orphan facet(s) repaired. Run 'journal indexer --rescan-full' to refresh the index.\n"
        ));
        return success(stdout);
    }
    if failed_orphans == 0 {
        stdout.push_str(&format!(
            "{repaired} orphan facet(s) repaired; {} merge(s) committed but reported a maintenance failure after commit. See 'Committed merge maintenance failures' above. Run 'journal indexer --rescan-full' to refresh the index.\n",
            committed_failures.len()
        ));
        return Outcome::LocalFailure {
            stdout,
            stderr: "journal facet doctor: one or more orphan facet merges committed with maintenance failures\n"
                .to_owned(),
            exit: EXIT_IO,
        };
    }
    stdout.push_str(&format!(
        "{repaired} orphan facet(s) repaired; {failed_orphans} orphan facet(s) failed. Run 'journal indexer --rescan-full' to refresh the index.\n"
    ));
    Outcome::LocalFailure {
        stdout,
        stderr: "journal facet doctor: one or more orphan facet repairs failed\n".to_owned(),
        exit: EXIT_IO,
    }
}

#[cfg(not(target_os = "ios"))]
fn append_facet_doctor_section(stdout: &mut String, heading: &str, entries: &[String]) {
    if entries.is_empty() {
        return;
    }
    if !stdout.is_empty() {
        stdout.push('\n');
    }
    stdout.push_str(heading);
    stdout.push_str(":\n");
    for entry in entries {
        stdout.push_str(&format!("- {entry}\n"));
    }
}

#[cfg(not(target_os = "ios"))]
fn outcome_diagnostic(outcome: &Outcome) -> String {
    match outcome {
        Outcome::LocalFailure { stderr, .. } | Outcome::ProcessFailure { stderr, .. } => {
            stderr.trim().to_owned()
        }
        _ => "facet merge failed".to_owned(),
    }
}

#[cfg(target_os = "ios")]
fn facet_merge(_args: &[OsString]) -> Outcome {
    failure("facet merge", "unavailable on iOS", EXIT_UNAVAILABLE)
}

#[cfg(not(target_os = "ios"))]
fn facet_merge(args: &[OsString]) -> Outcome {
    let Some(source) = args.first().and_then(|arg| arg.to_str()) else {
        return usage("facet merge", "SOURCE is required");
    };
    if source == "--help" || source == "-h" {
        return if args.len() == 1 {
            success("Usage: journal facet merge SOURCE --into DEST [--consent]\n".to_owned())
        } else {
            usage("facet merge", "unexpected argument")
        };
    }
    let mut destination: Option<&str> = None;
    let mut consent = false;
    let mut index = 1;
    while index < args.len() {
        match args[index].to_str() {
            Some("--into") if destination.is_none() => {
                destination = args.get(index + 1).and_then(|arg| arg.to_str());
                if destination.is_none() {
                    return usage("facet merge", "--into requires DEST");
                }
                index += 2;
            }
            Some("--consent") if !consent => {
                consent = true;
                index += 1;
            }
            _ => return usage("facet merge", "unexpected argument"),
        }
    }
    let Some(destination) = destination else {
        return usage("facet merge", "--into DEST is required");
    };
    if !safe_component(source) || !safe_component(destination) || source == destination {
        return failure("facet merge", "invalid SOURCE or DEST", EXIT_DATA);
    }
    let journal = match journal_root("facet merge") {
        Ok(path) => path,
        Err(outcome) => return outcome,
    };
    facet_merge_in_journal(&journal, source, destination, consent)
}

#[cfg(not(target_os = "ios"))]
fn facet_merge_in_journal(
    journal: &Path,
    source: &str,
    destination: &str,
    consent: bool,
) -> Outcome {
    match facet_merge_transaction_in_journal(journal, source, destination, consent) {
        Err(outcome) => outcome,
        Ok(FacetMergeCommit {
            post_commit_failure: Some(outcome),
            ..
        }) => outcome,
        Ok(FacetMergeCommit {
            post_commit_failure: None,
            ..
        }) => success(format!(
            "Merged '{source}' into '{destination}'. Index rebuild completed.\n"
        )),
    }
}

#[cfg(not(target_os = "ios"))]
struct FacetMergeCommit {
    report: FacetTreeMergeReport,
    post_commit_failure: Option<Outcome>,
}

#[cfg(not(target_os = "ios"))]
fn facet_merge_transaction_in_journal(
    journal: &Path,
    source: &str,
    destination: &str,
    consent: bool,
) -> Result<FacetMergeCommit, Outcome> {
    let source_path = journal.join("facets").join(source);
    let destination_path = journal.join("facets").join(destination);
    if let Err(error) = require_real_directory(&source_path) {
        return Err(failure("facet merge", &error, EXIT_DATA));
    }
    if let Err(error) = require_real_directory(&destination_path) {
        return Err(failure("facet merge", &error, EXIT_DATA));
    }
    let _lock = match hold_facet_trust_lock(journal) {
        Ok(lock) => lock,
        Err(error) => return Err(failure("facet merge", &error.to_string(), EXIT_IO)),
    };
    if let Err(error) = require_real_directory(&source_path) {
        return Err(failure("facet merge", &error, EXIT_DATA));
    }
    if let Err(error) = require_real_directory(&destination_path) {
        return Err(failure("facet merge", &error, EXIT_DATA));
    }
    let transaction = transaction_id();
    let facets = journal.join("facets");
    let stage = facets.join(format!(".facet-merge-{transaction}.stage"));
    let backup = facets.join(format!(".facet-merge-{transaction}.dest"));
    let source_backup = facets.join(format!(".facet-merge-{transaction}.source"));
    if let Err(error) = require_missing(&backup).and_then(|()| require_missing(&source_backup)) {
        return Err(failure("facet merge", &error, EXIT_IO));
    }
    if let Err(error) = create_private_dir_exclusive(&stage) {
        return Err(failure("facet merge", &error.to_string(), EXIT_IO));
    }
    if let Err(error) = copy_tree(&destination_path, &stage, true) {
        let _ = fs::remove_dir_all(&stage);
        return Err(failure("facet merge", &error, EXIT_IO));
    }
    let report = match merge_tree(&source_path, &stage) {
        Ok(report) => report,
        Err(error) => {
            let _ = fs::remove_dir_all(&stage);
            return Err(failure("facet merge", &error, EXIT_IO));
        }
    };
    if let Err(error) = fs::rename(&destination_path, &backup) {
        let _ = fs::remove_dir_all(&stage);
        return Err(failure("facet merge", &error.to_string(), EXIT_IO));
    }
    if let Err(error) = fs::rename(&stage, &destination_path) {
        let _ = fs::rename(&backup, &destination_path);
        return Err(failure("facet merge", &error.to_string(), EXIT_IO));
    }
    if let Err(error) = fs::rename(&source_path, &source_backup) {
        let _ = fs::rename(&destination_path, &stage);
        let _ = fs::rename(&backup, &destination_path);
        let _ = fs::remove_dir_all(&stage);
        return Err(failure("facet merge", &error.to_string(), EXIT_IO));
    }
    let mut params = json!({"source": source, "dest": destination});
    if consent {
        params["consent"] = Value::Bool(true);
    }
    if let Err(error) = append_action_log(journal, None, "cli", "user", "facet_merge", params) {
        let rollback =
            rollback_facet_trees(&destination_path, &backup, &source_path, &source_backup);
        return Err(transaction_failure(
            "facet merge",
            &error.to_string(),
            rollback,
        ));
    }
    if let Err(error) = remove_tree_pair(&backup, &source_backup) {
        return Ok(FacetMergeCommit {
            report,
            post_commit_failure: Some(failure(
                "facet merge",
                &format!("merge committed but backup cleanup failed: {error}"),
                EXIT_IO,
            )),
        });
    }
    let post_commit_failure = scan_journal(journal, true).err().map(|error| {
        failure(
            "facet merge",
            &format!("merge committed but index rebuild failed: {error}"),
            EXIT_FAILED,
        )
    });
    Ok(FacetMergeCommit {
        report,
        post_commit_failure,
    })
}

fn news_write(args: &[OsString]) -> Outcome {
    if args.len() == 1 && matches!(args[0].to_str(), Some("--help" | "-h")) {
        return success("Usage: journal news write FACET --day YYYYMMDD\n".to_owned());
    }
    let Some(facet) = args.first().and_then(|arg| arg.to_str()) else {
        return usage("news write", "FACET is required");
    };
    if args.len() != 3 || args[1] != OsStr::new("--day") {
        return usage("news write", "expected FACET --day YYYYMMDD");
    }
    let Some(day) = args[2].to_str() else {
        return usage("news write", "DAY must be UTF-8");
    };
    if !safe_component(facet) {
        return failure("news write", "FACET is invalid", EXIT_DATA);
    }
    if !valid_day(day) {
        return failure("news write", "DAY must be a real YYYYMMDD date", EXIT_DATA);
    }
    let journal = match journal_root("news write") {
        Ok(path) => path,
        Err(outcome) => return outcome,
    };
    if let Err(error) = require_real_directory(&journal.join("facets").join(facet)) {
        return failure("news write", &error, EXIT_DATA);
    }
    let mut bytes = Vec::new();
    if let Err(error) = io::stdin().read_to_end(&mut bytes) {
        return failure("news write", &error.to_string(), EXIT_IO);
    }
    let markdown = match String::from_utf8(bytes) {
        Ok(markdown) if !markdown.trim().is_empty() => markdown,
        Ok(_) => return failure("news write", "no content provided on stdin", EXIT_DATA),
        Err(_) => return failure("news write", "stdin must be UTF-8", EXIT_DATA),
    };
    if let Err(error) = write_news_file(&journal, facet, &format!("{day}.md"), &markdown) {
        return failure("news write", &error.to_string(), EXIT_IO);
    }
    success(format!("News for {day} saved to {facet}.\n"))
}

enum ExistingPathKind {
    RegularFile,
    Directory,
    Unsafe,
}

fn existing_path_kind(path: &Path) -> Result<Option<ExistingPathKind>, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => Ok(Some(ExistingPathKind::RegularFile)),
        Ok(metadata) if metadata.file_type().is_dir() => Ok(Some(ExistingPathKind::Directory)),
        Ok(_) => Ok(Some(ExistingPathKind::Unsafe)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn rollback_facet_trees(
    destination: &Path,
    destination_backup: &Path,
    source: &Path,
    source_backup: &Path,
) -> Result<(), String> {
    if destination.exists() {
        fs::remove_dir_all(destination).map_err(|error| error.to_string())?;
    }
    fs::rename(destination_backup, destination).map_err(|error| error.to_string())?;
    fs::rename(source_backup, source).map_err(|error| error.to_string())?;
    Ok(())
}

fn remove_tree_pair(first: &Path, second: &Path) -> Result<(), String> {
    fs::remove_dir_all(first).map_err(|error| error.to_string())?;
    fs::remove_dir_all(second).map_err(|error| error.to_string())
}

fn transaction_failure(token: &str, primary: &str, rollback: Result<(), String>) -> Outcome {
    match rollback {
        Ok(()) => failure(
            token,
            &format!("transaction failed and was rolled back: {primary}"),
            EXIT_IO,
        ),
        Err(rollback_error) => failure(
            token,
            &format!("transaction failed ({primary}); rollback also failed ({rollback_error})"),
            EXIT_IO,
        ),
    }
}

#[cfg(all(test, not(target_os = "ios")))]
mod facet_merge_tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use solstone_core_facets::hold_facet_trust_lock;

    use super::{Outcome, facet_merge_in_journal};

    static NEXT_TEMP_DIR: AtomicU64 = AtomicU64::new(0);

    struct TempJournal(PathBuf);

    impl TempJournal {
        fn new() -> Self {
            let sequence = NEXT_TEMP_DIR.fetch_add(1, Ordering::Relaxed);
            let path = PathBuf::from("/var/tmp").join(format!(
                "solstone-core-journal-cli-facet-merge-{}-{}",
                std::process::id(),
                sequence
            ));
            fs::create_dir_all(path.join("facets/source")).expect("source facet");
            fs::create_dir_all(path.join("facets/destination")).expect("destination facet");
            fs::create_dir_all(path.join("config")).expect("config directory");
            fs::write(path.join("facets/source/source.txt"), b"source").expect("source tree");
            fs::write(
                path.join("facets/destination/destination.txt"),
                b"destination",
            )
            .expect("destination tree");
            fs::write(
                path.join("config/convey.json"),
                br#"{ "facets": { "selected": "source", "order": "malformed" } }"#,
            )
            .expect("legacy convey config");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempJournal {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn tree_bytes(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
        fn visit(root: &Path, directory: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
            for entry in fs::read_dir(directory).expect("read tree") {
                let entry = entry.expect("tree entry");
                let path = entry.path();
                if path.is_dir() {
                    visit(root, &path, files);
                } else {
                    files.insert(
                        path.strip_prefix(root)
                            .expect("relative tree path")
                            .to_path_buf(),
                        fs::read(path).expect("tree bytes"),
                    );
                }
            }
        }

        let mut files = BTreeMap::new();
        visit(root, root, &mut files);
        files
    }

    #[test]
    fn facet_merge_leaves_legacy_config_and_destination_file_collision_byte_identical() {
        let journal = TempJournal::new();
        let config = journal.path().join("config/convey.json");
        let before = fs::read(&config).expect("convey config before merge");
        fs::write(journal.path().join("facets/source/collision.md"), b"source")
            .expect("source collision");
        fs::write(
            journal.path().join("facets/destination/collision.md"),
            b"destination",
        )
        .expect("destination collision");

        let outcome = facet_merge_in_journal(journal.path(), "source", "destination", false);

        assert!(matches!(outcome, Outcome::LocalSuccess { .. }));
        assert_eq!(fs::read(config).expect("convey config after merge"), before);
        assert_eq!(
            fs::read(journal.path().join("facets/destination/collision.md"))
                .expect("destination collision after merge"),
            b"destination"
        );
        assert!(!journal.path().join("facets/source").exists());
        assert!(
            journal
                .path()
                .join("facets/destination/source.txt")
                .exists()
        );
    }

    #[test]
    fn facet_merge_action_log_failure_restores_trees_and_config_without_staging() {
        let journal = TempJournal::new();
        fs::write(journal.path().join("config/actions"), b"block action log")
            .expect("action log conflict");
        drop(hold_facet_trust_lock(journal.path()).expect("seed trust lock"));
        let before = tree_bytes(journal.path());

        let outcome = facet_merge_in_journal(journal.path(), "source", "destination", false);

        assert!(matches!(outcome, Outcome::LocalFailure { .. }));
        assert_eq!(tree_bytes(journal.path()), before);
        let leftovers = fs::read_dir(journal.path().join("facets"))
            .expect("facet directory")
            .map(|entry| {
                entry
                    .expect("facet entry")
                    .file_name()
                    .into_string()
                    .expect("utf8")
            })
            .filter(|name| name.starts_with(".facet-merge-"))
            .collect::<Vec<_>>();
        assert!(
            leftovers.is_empty(),
            "staging artifacts remain: {leftovers:?}"
        );
    }
}

#[cfg(test)]
mod orphan_facet_tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::orphan_facets;

    static NEXT_TEMP_DIR: AtomicU64 = AtomicU64::new(0);

    struct TempJournal(PathBuf);

    impl TempJournal {
        fn new() -> Self {
            let sequence = NEXT_TEMP_DIR.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "solstone-core-journal-cli-orphan-facets-{}-{sequence}",
                std::process::id()
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempJournal {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn retired_facet_content_alone_is_not_repaired_into_metadata() {
        let journal = TempJournal::new();
        let retired = journal.0.join("facets/retired/todos/20260801.jsonl");
        fs::create_dir_all(retired.parent().unwrap()).unwrap();
        fs::write(&retired, b"{\"text\":\"leave it alone\"}\n").unwrap();

        assert_eq!(orphan_facets(&journal.0).unwrap(), Vec::<String>::new());
        assert!(!journal.0.join("facets/retired/facet.json").exists());

        let active = journal.0.join("facets/active/logs/20260801.jsonl");
        fs::create_dir_all(active.parent().unwrap()).unwrap();
        fs::write(active, b"{\"message\":\"still supported\"}\n").unwrap();
        assert_eq!(orphan_facets(&journal.0).unwrap(), vec!["active"]);
    }
}

fn orphan_facets(journal: &Path) -> Result<Vec<String>, String> {
    let facets = journal.join("facets");
    let entries = match fs::read_dir(&facets) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.to_string()),
    };
    let mut orphans = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        let kind = entry.file_type().map_err(|error| error.to_string())?;
        if !kind.is_dir() {
            continue;
        }
        let Some(slug) = entry.file_name().to_str().map(str::to_owned) else {
            return Err("facet name is not UTF-8".to_owned());
        };
        if !safe_component(&slug) {
            continue;
        }
        match fs::symlink_metadata(entry.path().join("facet.json")) {
            Ok(metadata) if metadata.file_type().is_file() => continue,
            Ok(_) => return Err(format!("unsafe facet declaration for {slug}")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
        for content in ["entities", "activities", "news", "logs"] {
            if contains_content(&entry.path().join(content))? {
                orphans.push(slug.to_owned());
                break;
            }
        }
    }
    orphans.sort();
    Ok(orphans)
}

fn contains_content(path: &Path) -> Result<bool, String> {
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.to_string()),
    };
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        let kind = entry.file_type().map_err(|error| error.to_string())?;
        if kind.is_dir() {
            if contains_content(&entry.path())? {
                return Ok(true);
            }
        } else if kind.is_file() {
            let name = entry.file_name();
            if name != OsStr::new(".gitkeep") && !name.to_string_lossy().ends_with(".lock") {
                return Ok(true);
            }
        } else {
            return Err(format!("unsafe facet content: {}", entry.path().display()));
        }
    }
    Ok(false)
}

fn copy_tree(
    source: &Path,
    destination: &Path,
    include_declaration: bool,
) -> Result<Vec<PathBuf>, String> {
    let mut copied_regular_files = Vec::new();
    copy_tree_into(
        source,
        destination,
        include_declaration,
        Path::new(""),
        &mut copied_regular_files,
    )?;
    copied_regular_files.sort();
    Ok(copied_regular_files)
}

fn copy_tree_into(
    source: &Path,
    destination: &Path,
    include_declaration: bool,
    relative: &Path,
    copied_regular_files: &mut Vec<PathBuf>,
) -> Result<(), String> {
    create_private_dir(destination).map_err(|error| error.to_string())?;
    let mut entries = fs::read_dir(source)
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        if !include_declaration && entry.file_name() == OsStr::new("facet.json") {
            continue;
        }
        let name = entry.file_name();
        let kind = entry.file_type().map_err(|error| error.to_string())?;
        let target = destination.join(&name);
        let child_relative = relative.join(&name);
        if kind.is_dir() {
            copy_tree_into(
                &entry.path(),
                &target,
                include_declaration,
                &child_relative,
                copied_regular_files,
            )?;
        } else if kind.is_file() {
            let bytes = fs::read(entry.path()).map_err(|error| error.to_string())?;
            write_bytes_exclusive(&target, &bytes, AtomicWriteOptions { mode: Some(0o600) })
                .map_err(|error| error.to_string())?;
            copied_regular_files.push(child_relative);
        } else {
            return Err(format!("unsafe facet entry: {}", entry.path().display()));
        }
    }
    Ok(())
}

#[derive(Default)]
struct FacetTreeMergeReport {
    copied_regular_files: Vec<PathBuf>,
    regular_file_collisions: Vec<PathBuf>,
}

fn merge_tree(source: &Path, destination: &Path) -> Result<FacetTreeMergeReport, String> {
    let mut report = FacetTreeMergeReport::default();
    merge_tree_into(source, destination, Path::new(""), &mut report)?;
    report.copied_regular_files.sort();
    report.regular_file_collisions.sort();
    Ok(report)
}

fn merge_tree_into(
    source: &Path,
    destination: &Path,
    relative: &Path,
    report: &mut FacetTreeMergeReport,
) -> Result<(), String> {
    let mut entries = fs::read_dir(source)
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        if entry.file_name() == OsStr::new("facet.json") {
            continue;
        }
        let name = entry.file_name();
        let kind = entry.file_type().map_err(|error| error.to_string())?;
        let target = destination.join(&name);
        let child_relative = relative.join(&name);
        if kind.is_dir() {
            match existing_path_kind(&target)? {
                Some(ExistingPathKind::Directory) => {
                    merge_tree_into(&entry.path(), &target, &child_relative, report)?;
                }
                None => {
                    let copied = copy_tree(&entry.path(), &target, true)?;
                    report
                        .copied_regular_files
                        .extend(copied.into_iter().map(|path| child_relative.join(path)));
                }
                Some(_) => return Err(format!("unsafe facet entry: {}", target.display())),
            }
        } else if kind.is_file() {
            match existing_path_kind(&target)? {
                None => {
                    let bytes = fs::read(entry.path()).map_err(|error| error.to_string())?;
                    write_bytes_exclusive(
                        &target,
                        &bytes,
                        AtomicWriteOptions { mode: Some(0o600) },
                    )
                    .map_err(|error| error.to_string())?;
                    report.copied_regular_files.push(child_relative);
                }
                Some(ExistingPathKind::RegularFile)
                    if entry.path().extension() == Some(OsStr::new("jsonl")) =>
                {
                    merge_jsonl(&target, &entry.path())?;
                }
                Some(ExistingPathKind::RegularFile)
                    if entry.file_name() == OsStr::new("entity.json") =>
                {
                    merge_json_object(&target, &entry.path())?;
                }
                Some(ExistingPathKind::RegularFile) => {
                    report.regular_file_collisions.push(child_relative);
                }
                Some(_) => return Err(format!("unsafe facet entry: {}", target.display())),
            }
        } else {
            return Err(format!("unsafe facet entry: {}", entry.path().display()));
        }
    }
    Ok(())
}

fn merge_jsonl(destination: &Path, source: &Path) -> Result<(), String> {
    let destination_text = fs::read_to_string(destination).map_err(|error| error.to_string())?;
    let source_text = fs::read_to_string(source).map_err(|error| error.to_string())?;
    let mut seen_ids = BTreeSet::new();
    let mut seen_lines = BTreeSet::new();
    let mut lines = Vec::new();
    for line in destination_text.lines().chain(source_text.lines()) {
        if line.trim().is_empty() {
            continue;
        }
        let id = serde_json::from_str::<Value>(line)
            .ok()
            .and_then(|value| value.get("id").and_then(Value::as_str).map(str::to_owned));
        let keep = match id {
            Some(id) if !id.is_empty() => seen_ids.insert(id),
            _ => seen_lines.insert(line.to_owned()),
        };
        if keep {
            lines.push(line);
        }
    }
    let mut merged = lines.join("\n");
    if !merged.is_empty() {
        merged.push('\n');
    }
    atomic_replace(
        destination,
        merged.as_bytes(),
        AtomicWriteOptions { mode: Some(0o600) },
    )
    .map_err(|error| error.to_string())
}

fn merge_json_object(destination: &Path, source: &Path) -> Result<(), String> {
    let source_value: Value =
        serde_json::from_slice(&fs::read(source).map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
    let destination_value: Value =
        serde_json::from_slice(&fs::read(destination).map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
    let mut merged = match source_value {
        Value::Object(map) => map,
        _ => return Err(format!("{} is not a JSON object", source.display())),
    };
    let Value::Object(destination_map) = destination_value else {
        return Err(format!("{} is not a JSON object", destination.display()));
    };
    merged.extend(destination_map);
    let mut bytes =
        serde_json::to_vec_pretty(&Value::Object(merged)).map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    atomic_replace(
        destination,
        &bytes,
        AtomicWriteOptions { mode: Some(0o600) },
    )
    .map_err(|error| error.to_string())
}

fn default_export_path(journal: &Path, exported_at: &str) -> Result<PathBuf, String> {
    let parent = journal
        .parent()
        .ok_or_else(|| "journal root has no parent".to_owned())?;
    let name = journal
        .file_name()
        .ok_or_else(|| "journal root has no file name".to_owned())?;
    let mut exports_name = name.to_os_string();
    exports_name.push(".exports");
    let exports = parent.join(exports_name);
    match fs::symlink_metadata(&exports) {
        Ok(metadata) if metadata.file_type().is_dir() => {
            #[cfg(unix)]
            fs::set_permissions(&exports, fs::Permissions::from_mode(0o700))
                .map_err(|error| error.to_string())?;
        }
        Ok(_) => return Err(format!("unsafe exports directory: {}", exports.display())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            create_private_dir(&exports).map_err(|error| error.to_string())?;
        }
        Err(error) => return Err(error.to_string()),
    }
    let filename = exported_at.replace(['-', ':'], "");
    Ok(exports.join(format!("{filename}.zip")))
}

fn reject_export_tree_output(journal: &Path, output: &Path, cwd: &Path) -> Result<(), String> {
    let absolute = if output.is_absolute() {
        output.to_owned()
    } else {
        cwd.join(output)
    };
    let parent = absolute
        .parent()
        .ok_or_else(|| "archive output has no parent".to_owned())?;
    let canonical_parent = fs::canonicalize(parent).map_err(|error| error.to_string())?;
    for family in ["chronicle", "entities", "facets", "imports"] {
        let root = journal.join(family);
        if let Ok(root) = fs::canonicalize(root)
            && canonical_parent.starts_with(root)
        {
            return Err(format!("output is inside exported {family} tree"));
        }
    }
    Ok(())
}

fn title_case_slug(slug: &str) -> String {
    slug.split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn safe_component(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes[0].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        && !matches!(value, "." | "..")
}

fn valid_day(value: &str) -> bool {
    value.len() == 8
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && NaiveDate::parse_from_str(value, "%Y%m%d").is_ok()
}

fn journal_root(token: &str) -> Result<PathBuf, Outcome> {
    let resolved = resolve_current_journal().map_err(|error| {
        failure(
            token,
            &format!("journal resolution failed: {error}"),
            EXIT_IO,
        )
    })?;
    canonical_directory(&resolved.path).map_err(|error| failure(token, &error, EXIT_IO))
}

fn canonical_directory(path: &Path) -> Result<PathBuf, String> {
    let canonical = fs::canonicalize(path).map_err(|error| error.to_string())?;
    require_real_directory(&canonical)?;
    Ok(canonical)
}

fn require_real_directory(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if metadata.file_type().is_dir() {
        Ok(())
    } else {
        Err(format!("not a real directory: {}", path.display()))
    }
}

fn require_missing(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(format!(
            "transaction path already exists: {}",
            path.display()
        )),
        Err(error) => Err(error.to_string()),
    }
}

fn create_private_dir_exclusive(path: &Path) -> io::Result<()> {
    let mut builder = DirBuilder::new();
    #[cfg(unix)]
    builder.mode(0o700);
    builder.create(path)?;
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

fn create_private_dir(path: &Path) -> io::Result<()> {
    let mut builder = DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    builder.mode(0o700);
    builder.create(path)?;
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

fn transaction_id() -> String {
    format!(
        "{}-{}",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or_default()
    )
}

fn target_exit(error: &solstone_core_journal_archive::ExplicitTargetError) -> u8 {
    use solstone_core_journal_archive::ExplicitTargetError;
    match error {
        ExplicitTargetError::InvalidTarget { .. } | ExplicitTargetError::UnsafeTarget { .. } => {
            EXIT_DATA
        }
        ExplicitTargetError::Collision { .. } => EXIT_FAILED,
        ExplicitTargetError::TargetIo { .. } | ExplicitTargetError::TargetChanged { .. } => EXIT_IO,
    }
}

fn archive_error(token: &str, message: &str) -> Outcome {
    let exit = if message.contains("unsafe") || message.contains("invalid archive") {
        EXIT_DATA
    } else {
        EXIT_IO
    };
    failure(token, message, exit)
}

#[cfg(not(target_os = "ios"))]
fn archive_merge_plan(plan: &ArchivePlan, source: &Path, json_output: bool) -> Outcome {
    if !json_output {
        return success(format!(
            "Archive merge (days: {}): 0 committed, {} skipped, 0 failed.\n",
            plan.days.join(", "),
            plan.payload_files,
        ));
    }
    emit_merge_json(
        true,
        "ok",
        source,
        true,
        Some(&plan.days),
        0,
        plan.payload_files,
        0,
        None,
        None,
        "not-requested",
        json_output,
        EXIT_FAILED,
    )
}

#[cfg(not(target_os = "ios"))]
fn archive_merge_json(
    result: Result<ArchiveMergeResult, ImportSourcesError>,
    source: &Path,
    dry_run: bool,
    json_output: bool,
) -> Outcome {
    match result {
        Ok(outcome) => {
            let committed = outcome.merge_summary.segments_copied
                + outcome.merge_summary.imports_copied
                + outcome.merge_summary.entities_created
                + outcome.merge_summary.entities_merged
                + outcome.merge_summary.facets_created
                + outcome.merge_summary.facets_merged;
            let skipped = outcome.merge_summary.segments_skipped
                + outcome.merge_summary.entities_skipped
                + outcome.merge_summary.imports_skipped;
            let failed = outcome.merge_summary.segments_errored;
            let ok = matches!(
                outcome.retry_disposition,
                RetryDisposition::Applied | RetryDisposition::IdempotentNoop
            );
            let code = match &outcome.reindex_status {
                ReindexStatus::NotAccepted { .. } => "index-rebuild-failed",
                _ if outcome.retry_disposition == RetryDisposition::Incomplete => "incomplete",
                _ => "ok",
            };
            let index_rebuild = match &outcome.reindex_status {
                ReindexStatus::Accepted => "completed",
                ReindexStatus::NotAccepted { .. } => "failed",
                ReindexStatus::NotRequested => "not-requested",
            };
            emit_merge_json(
                ok,
                code,
                source,
                dry_run,
                None,
                committed,
                skipped,
                failed,
                Some(&outcome.decision_log_path),
                Some(&outcome.staging_path),
                index_rebuild,
                json_output,
                EXIT_FAILED,
            )
        }
        Err(error) => {
            let (code, exit) = match &error {
                ImportSourcesError::MergePublishFailed { .. } => {
                    ("merge-publish-failed", EXIT_FAILED)
                }
                ImportSourcesError::LockBusy { .. } => ("lock-busy", EXIT_IO),
                ImportSourcesError::ArchiveUnsafeEntry { .. }
                | ImportSourcesError::ArchiveInvalid { .. }
                | ImportSourcesError::ArchiveEntryEncrypted { .. } => ("merge-failed", EXIT_DATA),
                _ => ("merge-failed", EXIT_IO),
            };
            if json_output {
                emit_merge_json(
                    false,
                    code,
                    source,
                    dry_run,
                    None,
                    0,
                    0,
                    0,
                    None,
                    None,
                    "not-requested",
                    true,
                    exit,
                )
            } else {
                failure("archive merge", &error.to_string(), exit)
            }
        }
    }
}

#[cfg(not(target_os = "ios"))]
#[allow(clippy::too_many_arguments)] // JSON line keys are independently specified by the local-ops census.
fn emit_merge_json(
    ok: bool,
    code: &str,
    source: &Path,
    dry_run: bool,
    days: Option<&[String]>,
    committed: usize,
    skipped: usize,
    failed: usize,
    decision_log: Option<&Path>,
    staging_dir: Option<&Path>,
    index_rebuild: &str,
    json_output: bool,
    exit: u8,
) -> Outcome {
    if json_output {
        let line = json!({
            "ok": ok,
            "code": code,
            "source": source.display().to_string(),
            "dry_run": dry_run,
            "days": days,
            "committed": committed,
            "skipped": skipped,
            "failed": failed,
            "decision_log": decision_log.map(|path| path.display().to_string()),
            "staging_dir": staging_dir.map(|path| path.display().to_string()),
            "index_rebuild": index_rebuild,
            "summary": {"committed": committed, "skipped": skipped, "failed": failed}
        })
        .to_string()
            + "\n";
        if ok {
            return success(line);
        }
        return Outcome::LocalFailure {
            stdout: String::new(),
            stderr: line,
            exit,
        };
    }
    if ok {
        success(format!(
            "Archive merge: {committed} committed, {skipped} skipped, {failed} failed.\n"
        ))
    } else {
        failure(
            "archive merge",
            &format!("{committed} committed, {skipped} skipped, {failed} failed"),
            exit,
        )
    }
}

fn success(stdout: String) -> Outcome {
    Outcome::LocalSuccess {
        stdout,
        stderr: String::new(),
    }
}

fn usage(token: &str, message: &str) -> Outcome {
    failure(token, message, EXIT_USAGE)
}

fn failure(token: &str, message: &str, exit: u8) -> Outcome {
    Outcome::LocalFailure {
        stdout: String::new(),
        stderr: format!("journal {token}: {message}\n"),
        exit,
    }
}

#[cfg(all(test, not(target_os = "ios")))]
mod archive_merge_source_kind_tests {
    use super::*;

    #[test]
    fn missing_source_names_the_absent_path() {
        let path = PathBuf::from("/no/such/solstone-archive-source.zip");
        let error = regular_archive_file(&path).unwrap_err();
        assert!(error.contains("SOURCE does not exist"), "{error}");
        assert!(error.contains("solstone-archive-source.zip"), "{error}");
        assert!(!error.contains("must be a regular file"), "{error}");
    }

    #[test]
    fn directory_source_names_a_regular_file_requirement() {
        let dir = std::env::temp_dir().join(format!(
            "solstone-archive-merge-src-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        fs::create_dir(&dir).unwrap();
        let error = regular_archive_file(&dir).unwrap_err();
        let _ = fs::remove_dir(&dir);
        assert_eq!(error, "SOURCE must be a regular file");
    }
}
