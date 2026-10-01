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

use chrono::{NaiveDate, SecondsFormat, Utc};
use serde_json::{Value, json};
use solstone_core_facets::{
    FacetWriteError, append_action_log, hold_facet_trust_lock, require_declared_facet,
    write_news_file,
};
#[cfg(not(target_os = "ios"))]
use solstone_core_import_sources::ImportSourcesError;
#[cfg(not(target_os = "ios"))]
use solstone_core_import_sources::archive::{
    ArchiveMergeOptions, ArchiveMergeResult, ArchivePlan, FullReindexRequester, ReindexStatus,
    RetryDisposition, merge_journal_archive, plan_journal_archive,
};
#[cfg(not(target_os = "ios"))]
use solstone_core_indexer_query::{
    CountsResponse, IndexAccessError, Order, OwnerBoundary, SearchHit, SearchRequest, search,
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
    AtomicWriteOptions, DirEntryKind, append_jsonl, atomic_replace, write_bytes_exclusive,
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
        "entities doctor" => entities_doctor(args),
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
    let today = Utc::now()
        .with_timezone(&solstone_core_journal_config::owner_zone(journal))
        .date_naive();
    let response = search(journal, OwnerBoundary, &request, today).map_err(index_query_error)?;
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
    if args
        .first()
        .is_some_and(|arg| arg == OsStr::new("--retire"))
    {
        return facet_doctor_retire(&args[1..]);
    }
    let mut fix = false;
    let mut adopt = false;
    let mut merge = false;
    for arg in args {
        match arg.to_str() {
            Some("--fix") => fix = true,
            Some("--adopt") => adopt = true,
            Some("--merge") => merge = true,
            Some("--help" | "-h") if args.len() == 1 => {
                return success(
                    "Usage: journal facet doctor [--fix] [--adopt [--merge]]\n       journal facet doctor --retire NAME (--into FACET | --deleted)\n".to_owned(),
                );
            }
            _ => return usage("facet doctor", "unexpected argument"),
        }
    }
    if merge && !adopt {
        return usage("facet doctor", "--merge requires --adopt");
    }
    let journal = match journal_root("facet doctor") {
        Ok(path) => path,
        Err(outcome) => return outcome,
    };
    #[cfg(target_os = "ios")]
    {
        if fix {
            return failure(
                "facet doctor",
                "--fix is unavailable on iOS",
                EXIT_UNAVAILABLE,
            );
        }
        return facet_doctor_orphans(&journal, adopt, merge, &Default::default());
    }
    #[cfg(not(target_os = "ios"))]
    {
        facet_doctor_all(&journal, fix, adopt, merge)
    }
}

/// Every facet doctor check, in order: the retired-name record, orphan
/// folders, names the action logs show were retired, stored references to
/// names that match nothing, dot-named folders, and, with --fix or --adopt,
/// bringing search up to date. `--fix` repairs; only `--adopt` registers
/// orphan folders as facets. No check stops the ones after it.
#[cfg(not(target_os = "ios"))]
fn facet_doctor_all(journal: &Path, fix: bool, adopt: bool, merge: bool) -> Outcome {
    use crate::facet_names;
    use solstone_core_facets::RetiredFacets;

    let mut stdout = String::new();
    let mut failures = Vec::new();
    let mut retired_readable = true;
    match solstone_core_facets::read_retired_facets(journal) {
        RetiredFacets::Malformed(detail) => {
            if fix {
                match facet_names::repair_malformed_retired_file(journal) {
                    Ok(Some(message)) => stdout.push_str(&format!("{message}\n")),
                    Ok(None) => {}
                    Err(error) => {
                        retired_readable = false;
                        failures.push(format!(
                            "retired facet names could not be repaired: {error}"
                        ));
                    }
                }
            } else {
                retired_readable = false;
                stdout.push_str(&format!(
                    "facets/retired.json does not parse ({detail}). Run with --fix to rebuild it; no name it reserves is freed.\n"
                ));
            }
        }
        RetiredFacets::Unreadable(detail) => {
            retired_readable = false;
            stdout.push_str(&format!(
                "facets/retired.json could not be read ({detail}). Check its permissions; it was left as it is.\n"
            ));
        }
        RetiredFacets::Absent | RetiredFacets::Loaded(_) => {}
    }

    // Names the action logs show were retired, not yet recorded: no folder of
    // that name is registered until --fix records them.
    let mut held = std::collections::BTreeSet::new();
    if retired_readable {
        let retired = solstone_core_facets::read_retired_facets(journal).entries();
        match facet_names::scan_history(journal, &retired) {
            Ok(scan) => {
                if !fix {
                    held.extend(scan.proposals.iter().map(|proposal| proposal.name.clone()));
                }
                if !scan.proposals.is_empty() {
                    stdout.push_str(if fix {
                        "\nRecorded retired facet names from the action logs:\n"
                    } else {
                        "\nFacet names the action logs show were retired but aren't recorded (run with --fix to record them):\n"
                    });
                    for proposal in &scan.proposals {
                        if fix
                            && let Err(error) = solstone_core_facets::record_retired_facet(
                                journal,
                                &proposal.name,
                                proposal.entry.clone(),
                            )
                        {
                            failures.push(format!("{}: {error}", proposal.name));
                            continue;
                        }
                        stdout.push_str(&format!("- {}\n", proposal.evidence));
                    }
                }
                if !scan.reused.is_empty() {
                    stdout.push_str(&format!(
                        "\nThese facets were renamed or merged away earlier and their names later given to a new facet, so older material that names them can't be told apart. Nothing was changed: {}\n",
                        scan.reused.join(", ")
                    ));
                }
                if !scan.unrecoverable.is_empty() {
                    stdout.push_str(&format!(
                        "\nThe action logs mention these names but not where they went: {}. If material still names one, record it with --retire NAME --into FACET or --deleted.\n",
                        scan.unrecoverable.join(", ")
                    ));
                }
            }
            Err(error) => failures.push(format!("action log scan failed: {error}")),
        }
        let retired = solstone_core_facets::read_retired_facets(journal).entries();
        match facet_names::scan_unresolved_references(journal, &retired) {
            Ok(found) if !found.is_empty() => {
                stdout.push_str("\nSegments name these facets, which don't exist and aren't recorded as retired. Agents limited to chosen facets can't see that material; everything else can:\n");
                for (name, (count, first, last)) in &found {
                    stdout.push_str(&format!("- {name}: {count} ({first} to {last})\n"));
                }
                stdout.push_str("To make one reachable again, run: journal facet doctor --retire NAME --into FACET\n");
            }
            Ok(_) => {}
            Err(error) => failures.push(format!("reference scan failed: {error}")),
        }
    }

    let orphans = facet_doctor_orphans(journal, adopt, merge, &held);
    match orphans {
        Outcome::LocalSuccess { stdout: text, .. } => stdout.push_str(&text),
        Outcome::LocalFailure {
            stdout: text,
            stderr,
            ..
        } => {
            stdout.push_str(&text);
            failures.push(
                stderr
                    .trim()
                    .trim_start_matches("journal facet doctor: ")
                    .to_owned(),
            );
        }
        other => return other,
    }

    entity_link_section(journal, fix, &mut stdout, &mut failures);

    let (leftovers, hidden) = facet_names::scan_dot_directories(journal);
    if !leftovers.is_empty() {
        stdout.push_str(&format!(
            "\nLeftover folders from an interrupted facet merge, safe to delete once no merge is running: {}\n",
            leftovers.join(", ")
        ));
    }
    if !hidden.is_empty() {
        stdout.push_str(&format!(
            "\nFolders whose names start with a dot are never facets, so these facet declarations are ignored: {}\n",
            hidden.join(", ")
        ));
    }

    // Without an index there is nothing stored to bring up to date, and the
    // doctor never creates one. Adopting or merging orphans leaves this to
    // the doctor too.
    if (fix || adopt) && solstone_core_indexer_store::db::db_path(journal).exists() {
        if retired_readable {
            match facet_names::reconcile_facet_classifications(journal) {
                Ok(report) if report.incomplete => failures.push(
                    "facets kept changing while search was being updated; run again".to_owned(),
                ),
                Ok(report) if report.changed > 0 => stdout.push_str(&format!(
                    "\nUpdated search for {} stored entries so it matches facet names.\n",
                    report.changed
                )),
                Ok(_) => {}
                Err(error) => failures.push(format!("search could not be updated: {error}")),
            }
        } else {
            stdout.push_str(
                "\nSearch was not updated, because facets/retired.json can't be used yet.\n",
            );
        }
    }

    if stdout.is_empty() {
        stdout.push_str("Nothing to report.\n");
    }
    if failures.is_empty() {
        success(stdout)
    } else {
        Outcome::LocalFailure {
            stdout,
            stderr: format!("journal facet doctor: {}\n", failures.join("; ")),
            exit: EXIT_IO,
        }
    }
}

#[cfg(not(target_os = "ios"))]
fn facet_doctor_retire(args: &[OsString]) -> Outcome {
    let (name, into) = match args {
        [name, flag] if flag == OsStr::new("--deleted") => (name, None),
        [name, flag, target] if flag == OsStr::new("--into") => (name, Some(target)),
        _ => {
            return usage(
                "facet doctor",
                "--retire needs NAME and then --into FACET or --deleted",
            );
        }
    };
    let (Some(name), into) = (name.to_str(), into.map(|target| target.to_str())) else {
        return usage("facet doctor", "names must be UTF-8");
    };
    if into.is_some_and(|target| target.is_none()) {
        return usage("facet doctor", "names must be UTF-8");
    }
    let journal = match journal_root("facet doctor") {
        Ok(path) => path,
        Err(outcome) => return outcome,
    };
    match crate::facet_names::retire_name(&journal, name, into.flatten()) {
        Ok(stdout) => success(stdout),
        Err(error) => failure("facet doctor", &error, EXIT_DATA),
    }
}

#[cfg(target_os = "ios")]
fn facet_doctor_retire(_args: &[OsString]) -> Outcome {
    failure("facet doctor", "unavailable on iOS", EXIT_UNAVAILABLE)
}

/// Find facet folders with content but no declaration and, with `adopt`,
/// register them. A folder whose name matches a declared facet is a twin of
/// it and is never registered beside it; with `merge` it joins that facet.
fn facet_doctor_orphans(
    journal: &Path,
    adopt: bool,
    merge: bool,
    held: &std::collections::BTreeSet<String>,
) -> Outcome {
    let journal = journal.to_path_buf();
    let (orphans, retired_orphans) = match adoptable_orphan_facets(&journal) {
        Ok(found) => found,
        Err(error) => return failure("facet doctor", &error, EXIT_DATA),
    };
    let mut retired_note = if retired_orphans.is_empty() {
        String::new()
    } else {
        format!(
            "These folders have content under facet names that were deleted or merged, so they are not registered as facets; their files are left in place: {}\n",
            retired_orphans.join(", ")
        )
    };
    let (held_orphans, orphans): (Vec<_>, Vec<_>) =
        orphans.into_iter().partition(|slug| held.contains(slug));
    if !held_orphans.is_empty() {
        retired_note.push_str(&format!(
            "The action logs show these facet names were retired, so their folders are not registered; run with --fix to record them: {}\n",
            held_orphans.join(", ")
        ));
    }
    if orphans.is_empty() {
        return success(format!("No orphan facets found.\n{retired_note}"));
    }
    if !adopt {
        let groups = match orphan_groups(&journal, &orphans) {
            Ok(groups) => groups,
            Err(error) => return failure("facet doctor", &error, EXIT_DATA),
        };
        let lone = groups
            .iter()
            .filter(|group| group.adopts_alone())
            .map(|group| group.members[0].as_str())
            .collect::<Vec<_>>();
        let mut stdout = String::new();
        if !lone.is_empty() {
            stdout.push_str("Orphan facets:\n");
            for slug in &lone {
                stdout.push_str(&format!("- {slug}\n"));
            }
            stdout.push_str(&format!(
                "{} orphan facet(s) found. Run with --adopt to register them.\n",
                lone.len()
            ));
        }
        append_orphan_groups_left(&mut stdout, &groups, true);
        stdout.push_str(&retired_note);
        return success(stdout);
    }
    let _lock = match hold_facet_trust_lock(&journal) {
        Ok(lock) => lock,
        Err(error) => return failure("facet doctor", &error.to_string(), EXIT_IO),
    };
    let groups = match adoptable_orphan_facets(&journal).and_then(|(orphans, _)| {
        let orphans = orphans
            .into_iter()
            .filter(|slug| !held.contains(slug))
            .collect::<Vec<_>>();
        orphan_groups(&journal, &orphans)
    }) {
        Ok(groups) => groups,
        Err(error) => return failure("facet doctor", &error, EXIT_DATA),
    };
    if groups.is_empty() {
        return success(format!("No orphan facets found.\n{retired_note}"));
    }
    if merge {
        #[cfg(not(target_os = "ios"))]
        {
            return facet_doctor_adopt_merge(&journal, &groups);
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
    let mut stdout = String::from("Repaired orphan facets:\n");
    let mut record_failures = Vec::new();
    let mut repaired = 0;
    for group in groups.iter().filter(|group| group.adopts_alone()) {
        let slug = &group.members[0];
        match adopt_orphan_facet(&journal, slug, &transaction) {
            Ok(None) => {}
            Ok(Some(error)) => record_failures.push(error),
            Err(error) => return failure("facet doctor", &error, EXIT_IO),
        }
        stdout.push_str(&format!("- {slug}\n"));
        repaired += 1;
    }
    if repaired == 0 {
        stdout = String::from("No orphan facet was registered.\n");
    } else {
        stdout.push_str(&format!(
            "{repaired} orphan facet(s) repaired. Run 'journal indexer --rescan-full' to refresh the index.\n"
        ));
    }
    append_orphan_groups_left(&mut stdout, &groups, false);
    if record_failures.is_empty() {
        return success(stdout);
    }
    Outcome::LocalFailure {
        stdout,
        stderr: format!("journal facet doctor: {}\n", record_failures.join("; ")),
        exit: EXIT_IO,
    }
}

/// Orphan folders that share a normalised name, the declared facets whose
/// names normalise the same way, and those of them whose declaration needs
/// repair first.
struct OrphanGroup {
    members: Vec<String>,
    declared: Vec<String>,
    damaged: Vec<String>,
}

impl OrphanGroup {
    /// A lone folder that matches no facet: `--adopt` registers it.
    fn adopts_alone(&self) -> bool {
        self.members.len() == 1 && self.declared.is_empty() && self.damaged.is_empty()
    }
}

fn orphan_groups(journal: &Path, orphans: &[String]) -> Result<Vec<OrphanGroup>, String> {
    let inventory = solstone_core_facets::observe_declared_facet_inventory(journal)
        .map_err(|error| error.to_string())?;
    let named_like = |names: Vec<&String>, key: &str| {
        names
            .into_iter()
            .filter(|name| normalized_orphan_slug(name) == key)
            .cloned()
            .collect::<Vec<_>>()
    };
    Ok(group_orphan_facets(orphans)
        .into_iter()
        .map(|(key, members)| OrphanGroup {
            members,
            declared: named_like(
                inventory.enabled.iter().chain(&inventory.muted).collect(),
                &key,
            ),
            damaged: named_like(
                inventory
                    .malformed
                    .iter()
                    .chain(&inventory.unreadable)
                    .collect(),
                &key,
            ),
        })
        .collect())
}

/// List the orphan groups `--adopt` alone leaves as they are, and what would
/// move them. In a report (`before`), lone folders are already listed.
fn append_orphan_groups_left(stdout: &mut String, groups: &[OrphanGroup], before: bool) {
    let variants = groups
        .iter()
        .filter(|group| {
            group.members.len() > 1 && group.declared.is_empty() && group.damaged.is_empty()
        })
        .map(|group| format!("- {} -> {}\n", group.members.join(", "), group.members[0]))
        .collect::<String>();
    if !variants.is_empty() {
        if !stdout.is_empty() {
            stdout.push('\n');
        }
        stdout.push_str("Name-variant groups:\n");
        stdout.push_str(&variants);
        stdout.push_str(if before {
            "Run with --adopt --merge to collapse name variants before registering them.\n"
        } else {
            "These were not registered. Run with --adopt --merge to collapse them into one.\n"
        });
    }
    let twins = groups
        .iter()
        .filter(|group| group.declared.len() == 1 && group.damaged.is_empty())
        .collect::<Vec<_>>();
    if !twins.is_empty() {
        if !stdout.is_empty() {
            stdout.push('\n');
        }
        stdout.push_str("Folders named like a facet you have:\n");
        for group in &twins {
            stdout.push_str(&format!(
                "- {} -> {}\n",
                group.members.join(", "),
                group.declared[0]
            ));
        }
        stdout.push_str(
            "These are never registered beside it. Run with --adopt --merge to fold them into it; a merge is permanent, and a file both have keeps the facet's copy.\n",
        );
        let first = twins[0];
        stdout.push_str(&format!(
            "To see what one would bring first: journal facet merge {} --into {} --dry-run\n",
            first.members[0], first.declared[0]
        ));
    }
    append_facet_doctor_section(stdout, UNCLEAR_HEADING, &unclear_orphan_groups(groups));
}

const UNCLEAR_HEADING: &str = "Folders left as they are";

/// Groups no command here can place: named like more than one facet, or like
/// a facet whose declaration needs repair first. Each line says what to do.
fn unclear_orphan_groups(groups: &[OrphanGroup]) -> Vec<String> {
    groups
        .iter()
        .filter_map(|group| {
            let members = group.members.join(", ");
            if !group.damaged.is_empty() {
                Some(format!(
                    "{members}: named like {}, whose facet.json needs repair first",
                    group.damaged.join(", ")
                ))
            } else if group.declared.len() > 1 {
                Some(format!(
                    "{members}: named like {}; rename the folder to match one, then run with --adopt --merge",
                    group.declared.join(", ")
                ))
            } else {
                None
            }
        })
        .collect()
}

/// Register an orphan folder as a facet. The facet stands once this returns
/// `Ok`; `Ok(Some(_))` says its heal record could not be written.
fn adopt_orphan_facet(
    journal: &Path,
    slug: &str,
    transaction: &str,
) -> Result<Option<String>, String> {
    let title = title_case_slug(slug);
    // An adopted facet gets a stable id like any other, so agents can be
    // granted it and merged names can resolve to it.
    solstone_core_facets::create_facet(journal, slug, &title, "", "#667eea", "📦", None)
        .map_err(|error| error.to_string())?;
    let audit = journal.join("logs/facet-heals.jsonl");
    Ok(append_jsonl(
        &audit,
        &json!({
            "transaction_id": transaction,
            "facet": slug,
            "action": "facet_heal",
            "params": {"title": title}
        }),
    )
    .err()
    .map(|error| format!("registered {slug}, but its record could not be written: {error}")))
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
fn facet_doctor_adopt_merge(journal: &Path, groups: &[OrphanGroup]) -> Outcome {
    let transaction = transaction_id();
    let left = unclear_orphan_groups(groups);
    let mut merged = Vec::new();
    let mut collisions = Vec::new();
    let mut adopted = Vec::new();
    let mut failed = Vec::new();
    let mut failed_orphans = 0;
    let mut committed_failures = Vec::new();

    for group in groups {
        let members = &group.members;
        // A twin folds into the facet it is named like; otherwise the sorted
        // slug identity, not filesystem metadata, determines the destination
        // and fold order.
        let (destination, sources) = match group.declared.as_slice() {
            _ if !group.damaged.is_empty() => continue,
            [declared] => (declared, &members[..]),
            [] => (&members[0], &members[1..]),
            _ => continue,
        };
        if group.declared.is_empty() {
            let adopted_with = match adopt_orphan_facet(journal, destination, &transaction) {
                Ok(record_failure) => record_failure,
                Err(error) => {
                    let unmerged = members[1..].join(", ");
                    let detail = if unmerged.is_empty() {
                        format!("{destination} (adoption failed: {error})")
                    } else {
                        format!(
                            "{destination} (adoption failed: {error}; {unmerged} were not merged)"
                        )
                    };
                    failed.push(detail);
                    failed_orphans += members.len();
                    continue;
                }
            };
            if let Some(record_failure) = adopted_with {
                committed_failures.push(record_failure);
            }
            adopted.push(destination.clone());
        }
        let mut retained_origins = BTreeMap::<PathBuf, String>::new();
        for source in sources {
            // --merge is the caller's explicit consent for each derived merge audit record.
            match facet_merge_transaction_in_journal(
                journal,
                source,
                destination,
                true,
                FacetMergeMode::DoctorOrphan,
            ) {
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
    append_facet_doctor_section(&mut stdout, UNCLEAR_HEADING, &left);
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
        if repaired == 0 {
            stdout.push_str("No orphan facet was registered or merged.\n");
        } else {
            stdout.push_str(&format!(
                "{repaired} orphan facet(s) repaired. Run 'journal indexer --rescan-full' to refresh the index.\n"
            ));
        }
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

/// Each entity has one link folder per facet, named by its id. Report what
/// breaks that, and with `fix` repair what needs no judgment call.
#[cfg(not(target_os = "ios"))]
fn entity_link_section(journal: &Path, fix: bool, stdout: &mut String, failures: &mut Vec<String>) {
    use solstone_core_facets::facet_links::{check_journal_links, repair_journal_links};
    let list = |stdout: &mut String, heading: &str, issues: &[_], repaired: bool| {
        if issues.is_empty() {
            return;
        }
        stdout.push_str(&format!("\n{heading}\n"));
        for issue in issues {
            stdout.push_str(&format!(
                "- {}\n",
                describe_link_issue(issue, repaired, fix)
            ));
        }
    };
    if fix {
        match repair_journal_links(journal) {
            Ok((repaired, left)) => {
                list(stdout, "Entity links repaired:", &repaired, true);
                if !repaired.is_empty() {
                    stdout.push_str("Run 'journal indexer --rescan-full' to refresh the index.\n");
                }
                list(stdout, "Entity links left as they are:", &left, false);
                for issue in left.iter().filter(|issue| issue.failed) {
                    failures.push(format!(
                        "entity link repair failed: {}",
                        describe_link_issue(issue, false, true)
                    ));
                }
            }
            Err(error) => failures.push(format!("entity link repair failed: {error}")),
        }
    } else {
        match check_journal_links(journal) {
            Ok(issues) => list(
                stdout,
                "Entity links to repair (run with --fix to repair the ones that can be):",
                &issues,
                false,
            ),
            Err(error) => failures.push(format!("entity link check failed: {error}")),
        }
    }
}

#[cfg(not(target_os = "ios"))]
fn describe_link_issue(
    issue: &solstone_core_facets::facet_links::LinkIssue,
    repaired: bool,
    fix: bool,
) -> String {
    use solstone_core_facets::facet_links::LinkIssueKind;
    let folders = issue
        .folders
        .iter()
        .map(|folder| format!("'{folder}'"))
        .collect::<Vec<_>>()
        .join(", ");
    let facet = &issue.facet;
    let id = &issue.entity_id;
    let (is, links) = if repaired {
        ("was", "linked")
    } else {
        ("is", "still links")
    };
    let left_alone = if fix {
        ""
    } else {
        "; --fix leaves it as it is"
    };
    let needs_decision = fix
        && issue.note.is_none()
        && matches!(
            issue.kind,
            LinkIssueKind::Deleted | LinkIssueKind::Unreadable
        );
    let described = match &issue.kind {
        LinkIssueKind::Duplicate => {
            format!("{facet}: '{id}' {is} linked from more than one folder ({folders})")
        }
        LinkIssueKind::Misnamed => {
            format!("{facet}: '{id}' {is} linked from folder {folders}, not a folder named '{id}'")
        }
        LinkIssueKind::Merged { successor } => format!(
            "{facet}: '{id}' was merged into '{successor}', and folder {folders} {links} it"
        ),
        LinkIssueKind::Deleted => {
            format!("{facet}: '{id}' was deleted, and folder {folders} still links it{left_alone}")
        }
        LinkIssueKind::Unreadable => {
            format!("{facet}: folder {folders} has a link that can't be read or used{left_alone}")
        }
    };
    match &issue.note {
        Some(note) => format!("{described} ({note})"),
        None if needs_decision => match issue.kind {
            LinkIssueKind::Deleted => {
                format!("{described} (this needs your decision, so --fix leaves it)")
            }
            _ => format!("{described} (--fix can't tell which entity it links, so it leaves it)"),
        },
        None => described,
    }
}

/// Find deletes the action log shows that the journal hasn't recorded, and
/// with `--fix` record them, so their names are never re-created.
fn entities_doctor(args: &[OsString]) -> Outcome {
    let fix = match args {
        [] => false,
        [arg] if arg == OsStr::new("--fix") => true,
        [arg] if arg == OsStr::new("--help") || arg == OsStr::new("-h") => {
            return success("Usage: journal entities doctor [--fix]\n".to_owned());
        }
        _ => return usage("entities doctor", "unexpected argument"),
    };
    let journal = match journal_root("entities doctor") {
        Ok(path) => path,
        Err(outcome) => return outcome,
    };
    let result = if fix {
        solstone_core_facets::entity_doctor::repair_entity_records(&journal)
    } else {
        solstone_core_facets::entity_doctor::check_entity_records(&journal)
    };
    match result {
        Ok(report) => success(entities_doctor_text(&report, fix)),
        Err(error) => failure("entities doctor", &error.to_string(), EXIT_IO),
    }
}

fn entities_doctor_text(
    report: &solstone_core_facets::entity_doctor::EntityDoctorReport,
    fix: bool,
) -> String {
    use solstone_core_facets::entity_doctor::DeleteVerdict;
    let mut text = String::new();
    if let Some(problem) = &report.record_problem {
        let mut chars = problem.chars();
        let problem = chars.next().map_or_else(String::new, |first| {
            first.to_uppercase().chain(chars).collect::<String>()
        });
        text.push_str(&format!("{problem}\n\n"));
    }
    if !report.unreadable_entities.is_empty() {
        text.push_str(&format!(
            "These entity folders can't be read or don't say which entity they hold, so no delete can be checked against them, and --fix records nothing: {}\n\n",
            report.unreadable_entities.join(", ")
        ));
    }
    if report.merge_log_incomplete {
        text.push_str(
            "A line in logs/entity-merges.jsonl can't be read and doesn't say which entity it merged, so no delete can be checked against it, and --fix records nothing.\n\n",
        );
    }
    if report.merge_recovery_pending {
        text.push_str(
            "An entity merge was interrupted; --fix settles it before recording anything.\n\n",
        );
    }
    match &report.window {
        Some((first, last)) if report.findings.is_empty() => text.push_str(&format!(
            "No finished deletes in the action log from {first} to {last}; deletes before or after those days can't be found.\n"
        )),
        Some((first, last)) => text.push_str(&format!(
            "Deletes in the action log from {first} to {last} (deletes before or after those days can't be found):\n"
        )),
        None if report.unreadable_days.is_empty() => {
            text.push_str("There is no action log to check.\n")
        }
        None => text.push_str("The action log has no days that could be read.\n"),
    }
    for finding in &report.findings {
        let outcome = match &finding.verdict {
            DeleteVerdict::Record if fix && report.recorded.contains(&finding.entity_id) => {
                "recorded".to_owned()
            }
            DeleteVerdict::Record if fix => "already recorded".to_owned(),
            DeleteVerdict::Record => "would be recorded".to_owned(),
            DeleteVerdict::Leave(reason) => format!("left as it is: {reason}"),
        };
        let note = if finding.kind.starts_with("failed") {
            " (the delete reported an error, but the entity was removed)"
        } else {
            ""
        };
        text.push_str(&format!(
            "- '{}', deleted {}{note}: {outcome}\n",
            finding.entity_id, finding.day
        ));
    }
    if !report.unfinished.is_empty() {
        text.push_str("\nDeletes that were asked for and haven't finished:\n");
        for unfinished in &report.unfinished {
            let line = match &unfinished.entity_id {
                None => "which entity can't be told, and --fix doesn't record it".to_owned(),
                Some(id) => match unfinished.still_here {
                    Some(true) => format!("'{id}': it is still in the journal"),
                    Some(false) => {
                        format!("'{id}': it isn't in the journal, and --fix doesn't record it")
                    }
                    None => format!("'{id}': it can't be checked, and --fix doesn't record it"),
                },
            };
            text.push_str(&format!("- ({}) {line}\n", unfinished.day));
        }
    }
    if !report.live_again_merged.is_empty() {
        text.push_str(&format!(
            "\nEntities that were merged away and are back in the journal, left as they are: {}\n",
            report.live_again_merged.join(", ")
        ));
    }
    if !report.unreadable_days.is_empty() {
        text.push_str(&format!(
            "\nAction-log days that can't be read, so their deletes can't be checked: {}\n",
            report.unreadable_days.join(", ")
        ));
    }
    if report.malformed_action_lines + report.malformed_merge_lines > 0 {
        text.push_str(&format!(
            "\nLines that couldn't be read: {} in the action log, {} in logs/entity-merges.jsonl.\n",
            report.malformed_action_lines, report.malformed_merge_lines
        ));
    }
    let pending = report
        .findings
        .iter()
        .filter(|finding| finding.verdict == DeleteVerdict::Record)
        .count();
    if !fix && pending > 0 {
        text.push_str(
            "\nRun with --fix to record the deletes marked \"would be recorded\", so the journal never brings those names back on its own.\n",
        );
    }
    text
}

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
            success(
                "Usage: journal facet merge SOURCE --into DEST (--dry-run | --yes) [--consent]\n"
                    .to_owned(),
            )
        } else {
            usage("facet merge", "unexpected argument")
        };
    }
    let mut destination: Option<&str> = None;
    let mut consent = false;
    let mut dry_run = false;
    let mut yes = false;
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
            Some("--dry-run") if !dry_run => {
                dry_run = true;
                index += 1;
            }
            Some("--yes") if !yes => {
                yes = true;
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
    if dry_run {
        return facet_merge_preview_in_journal(&journal, source, destination);
    }
    if !yes {
        return usage(
            "facet merge",
            "a merge is permanent and can't be undone; review it with --dry-run, then run it again with --yes",
        );
    }
    facet_merge_in_journal(&journal, source, destination, consent)
}

/// Stages a merge exactly as the real one does, reports what it would lose,
/// and discards the staged copy. Nothing under `facets/` changes.
#[cfg(not(target_os = "ios"))]
fn facet_merge_preview_in_journal(journal: &Path, source: &str, destination: &str) -> Outcome {
    let source_path = journal.join("facets").join(source);
    let destination_path = journal.join("facets").join(destination);
    for path in [&source_path, &destination_path] {
        if let Err(error) = require_real_directory(path) {
            return failure("facet merge", &error, EXIT_DATA);
        }
    }
    let _lock = match hold_facet_trust_lock(journal) {
        Ok(lock) => lock,
        Err(error) => return failure("facet merge", &error.to_string(), EXIT_IO),
    };
    for path in [&source_path, &destination_path] {
        if let Err(error) = require_real_directory(path) {
            return failure("facet merge", &error, EXIT_DATA);
        }
    }
    let stage = journal
        .join("facets")
        .join(format!(".facet-merge-{}.preview", transaction_id()));
    let report = match stage_facet_merge(&source_path, &destination_path, &stage) {
        Ok(report) => report,
        Err(outcome) => return outcome,
    };
    if let Err(error) = fs::remove_dir_all(&stage) {
        return failure(
            "facet merge",
            &format!(
                "dry run could not remove its scratch copy {}: {error}; nothing else was changed, and that folder can be deleted",
                stage.display()
            ),
            EXIT_IO,
        );
    }
    let settings_dropped = !matches!(
        solstone_core_facets::observe_facet_destination(journal, source),
        Ok(solstone_core_facets::DestinationObservation::Absent)
    );
    success(facet_merge_preview_text(
        source,
        destination,
        &report,
        settings_dropped,
    ))
}

#[cfg(not(target_os = "ios"))]
fn facet_merge_preview_text(
    source: &str,
    destination: &str,
    report: &FacetTreeMergeReport,
    settings_dropped: bool,
) -> String {
    let mut text = format!(
        "Dry run, nothing was changed. Merging '{source}' into '{destination}' would lose:\n"
    );
    let before = text.len();
    if !report.regular_file_collisions.is_empty() {
        text.push_str(&format!(
            "Files both facets have that can't be combined. '{destination}' would keep its own copy, and the copy in '{source}' would be deleted with that facet:\n"
        ));
        for path in &report.regular_file_collisions {
            text.push_str(&format!("  {}\n", path.display()));
        }
    }
    if !report.jsonl_records_dropped.is_empty() {
        text.push_str(&format!(
            "Records that would be dropped because a different record with the same id is kept (the first record with each id is kept, reading '{destination}' first):\n"
        ));
        for (path, count) in &report.jsonl_records_dropped {
            text.push_str(&format!("  {}: {count}\n", path.display()));
        }
    }
    if !report.entity_fields_superseded.is_empty() {
        text.push_str(&format!(
            "Entity fields where '{destination}' would keep its own, different value:\n"
        ));
        for (path, count) in &report.entity_fields_superseded {
            text.push_str(&format!("  {}: {count}\n", path.display()));
        }
    }
    if settings_dropped {
        text.push_str(&format!(
            "The settings in the facet.json of '{source}', which would not be carried over.\n"
        ));
    }
    if text.len() == before {
        text.push_str("Nothing.\n");
    }
    if report.links_combined > 0 || report.link_notes_renumbered > 0 {
        text.push('\n');
    }
    match report.links_combined {
        0 => {}
        1 => text.push_str(
            "1 entity is linked in both facets; it would end up with one link, keeping every note.\n",
        ),
        count => text.push_str(&format!(
            "{count} entities are linked in both facets; each would end up with one link, keeping every note.\n"
        )),
    }
    match report.link_notes_renumbered {
        0 => {}
        1 => text.push_str("1 note would get a new number, after the notes already there.\n"),
        count => text.push_str(&format!(
            "{count} notes would get a new number, after the notes already there.\n"
        )),
    }
    text
}

#[cfg(not(target_os = "ios"))]
fn facet_merge_in_journal(
    journal: &Path,
    source: &str,
    destination: &str,
    consent: bool,
) -> Outcome {
    let FacetMergeCommit {
        report,
        post_commit_failure,
        source_removed,
    } = match facet_merge_transaction_in_journal(
        journal,
        source,
        destination,
        consent,
        FacetMergeMode::Owner,
    ) {
        Err(outcome) => return outcome,
        Ok(commit) => commit,
    };
    // The merge has committed on both paths below, so the files it did not
    // carry over are reported either way.
    let collisions = facet_merge_collision_notice(source, destination, &report, source_removed);
    match post_commit_failure {
        None => success(format!(
            "Merged '{source}' into '{destination}'. Index rebuild completed.\n{collisions}"
        )),
        Some(Outcome::LocalFailure {
            stdout,
            stderr,
            exit,
        }) => Outcome::LocalFailure {
            stdout: stdout + &collisions,
            stderr,
            exit,
        },
        Some(outcome) => outcome,
    }
}

#[cfg(not(target_os = "ios"))]
fn facet_merge_collision_notice(
    source: &str,
    destination: &str,
    report: &FacetTreeMergeReport,
    source_removed: bool,
) -> String {
    if report.regular_file_collisions.is_empty() {
        return String::new();
    }
    let fate = if source_removed {
        "was deleted with that facet"
    } else {
        "was not carried over"
    };
    let mut notice = format!(
        "Both facets had these files. '{destination}' kept its own copy; the copy in '{source}' {fate}:\n"
    );
    for path in &report.regular_file_collisions {
        notice.push_str(&format!("  {}\n", path.display()));
    }
    notice
}

/// Copies DEST into a fresh STAGE and merges SOURCE into it. On failure the
/// stage is removed; on success the caller owns it.
#[cfg(not(target_os = "ios"))]
fn stage_facet_merge(
    source_path: &Path,
    destination_path: &Path,
    stage: &Path,
) -> Result<FacetTreeMergeReport, Outcome> {
    if let Err(error) = create_private_dir_exclusive(stage) {
        return Err(failure("facet merge", &error.to_string(), EXIT_IO));
    }
    if let Err(error) = copy_tree(destination_path, stage) {
        let _ = fs::remove_dir_all(stage);
        return Err(failure("facet merge", &error, EXIT_IO));
    }
    merge_tree(source_path, stage).map_err(|error| {
        let _ = fs::remove_dir_all(stage);
        failure("facet merge", &error, EXIT_IO)
    })
}

#[cfg(not(target_os = "ios"))]
struct FacetMergeCommit {
    report: FacetTreeMergeReport,
    post_commit_failure: Option<Outcome>,
    /// False when cleanup failed, so the source tree may still sit in its
    /// backup directory.
    source_removed: bool,
}

/// Which merge is running: the owner's `facet merge` of one live facet into
/// another, or `facet doctor --adopt --merge` folding an undeclared folder into
/// the facet it adopted for its name-variant group, or into the declared facet
/// it is named like.
#[cfg(not(target_os = "ios"))]
#[derive(Clone, Copy, PartialEq, Eq)]
enum FacetMergeMode {
    Owner,
    DoctorOrphan,
}

#[cfg(not(target_os = "ios"))]
fn facet_merge_transaction_in_journal(
    journal: &Path,
    source: &str,
    destination: &str,
    consent: bool,
    mode: FacetMergeMode,
) -> Result<FacetMergeCommit, Outcome> {
    let committed = facet_merge_commit_locked(journal, source, destination, consent, mode)?;
    let FacetMergeCommit {
        report,
        post_commit_failure,
        source_removed,
    } = committed;
    if post_commit_failure.is_some() {
        return Ok(FacetMergeCommit {
            report,
            post_commit_failure,
            source_removed,
        });
    }
    // The doctor runs its merges under its own facet trust lock and brings
    // search up to date once, after releasing it; reconciling here would wait
    // on another process's reconcile while holding that lock.
    let reconcile_here = mode == FacetMergeMode::Owner;
    // Both run after the facet trust lock is released: the rebuild refreshes
    // the moved files, and the reconcile brings stored classifications for
    // material that still names SOURCE onto DEST.
    if let Err(error) = scan_journal(journal, true) {
        return Ok(FacetMergeCommit {
            report,
            source_removed,
            post_commit_failure: Some(failure(
                "facet merge",
                &format!("merge committed but index rebuild failed: {error}"),
                EXIT_FAILED,
            )),
        });
    }
    if !reconcile_here {
        return Ok(FacetMergeCommit {
            report,
            post_commit_failure: None,
            source_removed,
        });
    }
    let post_commit_failure = match crate::facet_names::reconcile_facet_classifications(journal) {
        Ok(report) if !report.incomplete => None,
        Ok(_) => Some(failure(
            "facet merge",
            "the merge finished, but facets kept changing while search was being updated; run 'journal facet doctor --fix'",
            EXIT_FAILED,
        )),
        Err(error) => Some(failure(
            "facet merge",
            &format!(
                "the merge finished, but search could not be updated for '{source}': {error}; run 'journal facet doctor --fix'"
            ),
            EXIT_FAILED,
        )),
    };
    Ok(FacetMergeCommit {
        report,
        post_commit_failure,
        source_removed,
    })
}

#[cfg(not(target_os = "ios"))]
fn facet_merge_commit_locked(
    journal: &Path,
    source: &str,
    destination: &str,
    consent: bool,
    mode: FacetMergeMode,
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
    let admission = crate::facet_names::admit_facet_merge(
        journal,
        source,
        destination,
        mode == FacetMergeMode::DoctorOrphan,
    )
    .map_err(|message| failure("facet merge", &message, EXIT_DATA))?;
    let transaction = transaction_id();
    let facets = journal.join("facets");
    let stage = facets.join(format!(".facet-merge-{transaction}.stage"));
    let backup = facets.join(format!(".facet-merge-{transaction}.dest"));
    let source_backup = facets.join(format!(".facet-merge-{transaction}.source"));
    if let Err(error) = require_missing(&backup).and_then(|()| require_missing(&source_backup)) {
        return Err(failure("facet merge", &error, EXIT_IO));
    }
    let report = stage_facet_merge(&source_path, &destination_path, &stage)?;
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
    // SOURCE's name now resolves to DEST and can never be given to another facet.
    let retired_before = match solstone_core_facets::snapshot_retired_files(journal) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            let rollback =
                rollback_facet_trees(&destination_path, &backup, &source_path, &source_backup);
            return Err(transaction_failure(
                "facet merge",
                &error.to_string(),
                rollback,
            ));
        }
    };
    if let Err(error) =
        solstone_core_facets::record_retired_facet(journal, source, admission.retired_entry())
    {
        let _ = solstone_core_facets::restore_retired_files(journal, &retired_before);
        let rollback =
            rollback_facet_trees(&destination_path, &backup, &source_path, &source_backup);
        return Err(transaction_failure(
            "facet merge",
            &error.to_string(),
            rollback,
        ));
    }
    let mut params = json!({
        "source": source,
        "dest": destination,
        "source_id": admission.source_id,
        "dest_id": admission.destination_id,
    });
    if consent {
        params["consent"] = Value::Bool(true);
    }
    if let Err(error) = append_action_log(journal, None, "cli", "user", "facet_merge", params) {
        let rollback =
            rollback_facet_trees(&destination_path, &backup, &source_path, &source_backup)
                .and_then(|()| {
                    solstone_core_facets::restore_retired_files(journal, &retired_before)
                        .map_err(|error| error.to_string())
                });
        return Err(transaction_failure(
            "facet merge",
            &error.to_string(),
            rollback,
        ));
    }
    if let Err(error) = remove_tree_pair(&backup, &source_backup) {
        return Ok(FacetMergeCommit {
            report,
            source_removed: false,
            post_commit_failure: Some(failure(
                "facet merge",
                &format!(
                    "the merge finished, but its leftover folders couldn't be removed: {error}; run 'journal facet doctor' to list them, remove them, then run 'journal facet doctor --fix'"
                ),
                EXIT_IO,
            )),
        });
    }
    Ok(FacetMergeCommit {
        report,
        post_commit_failure: None,
        source_removed: true,
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
    // Refuse before reading stdin so a caller does not lose a piped draft. The writer
    // repeats this check under the facet trust lock.
    if let Err(error) = require_declared_facet(&journal, facet) {
        let code = match error {
            FacetWriteError::DeclarationUnreadable { .. } => EXIT_IO,
            _ => EXIT_DATA,
        };
        return failure("news write", &error.to_string(), code);
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

    use super::{Outcome, facet_merge_in_journal, facet_merge_preview_in_journal};

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
                path.join("facets/source/facet.json"),
                br#"{"id":"11111111-1111-4111-8111-111111111111","title":"Source"}"#,
            )
            .expect("source declaration");
            fs::write(
                path.join("facets/destination/facet.json"),
                br#"{"id":"22222222-2222-4222-8222-222222222222","title":"Destination"}"#,
            )
            .expect("destination declaration");
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
        // Both facets have `notes/`, so the merge recurses into it rather than
        // copying the directory whole.
        fs::create_dir_all(journal.path().join("facets/source/notes"))
            .expect("nested source directory");
        fs::create_dir_all(journal.path().join("facets/destination/notes"))
            .expect("nested destination directory");
        fs::write(
            journal.path().join("facets/source/notes/facet.json"),
            b"nested",
        )
        .expect("nested facet.json");

        let outcome = facet_merge_in_journal(journal.path(), "source", "destination", false);

        let Outcome::LocalSuccess { stdout, .. } = outcome else {
            panic!("merge succeeds");
        };
        assert!(
            stdout.contains("the copy in 'source' was deleted with that facet:\n  collision.md\n"),
            "{stdout}"
        );
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
        assert_eq!(
            fs::read(journal.path().join("facets/destination/notes/facet.json"))
                .expect("nested facet.json is carried over"),
            b"nested"
        );
    }

    /// Collisions of every kind a merge can lose, plus the source's settings.
    fn seed_lossy_merge(journal: &TempJournal) {
        let facets = journal.path().join("facets");
        for (facet, body) in [("source", "source"), ("destination", "destination")] {
            fs::create_dir_all(facets.join(facet).join("notes")).expect("notes directory");
            fs::create_dir_all(facets.join(facet).join("entities/ada")).expect("entity directory");
            fs::write(facets.join(facet).join("collision.md"), body).expect("collision");
            fs::write(facets.join(facet).join("notes/today.md"), body).expect("nested collision");
        }
        fs::write(facets.join("source/facet.json"), br#"{"title":"Source"}"#)
            .expect("source settings");
        fs::write(
            facets.join("source/log.jsonl"),
            "{\"id\":\"1\",\"v\":\"source\"}\n{\"id\":\"2\",\"v\":\"same\"}\n{\"id\":\"3\"}\n",
        )
        .expect("source log");
        fs::write(
            facets.join("destination/log.jsonl"),
            "{\"id\":\"1\",\"v\":\"destination\"}\n{\"id\":\"2\",\"v\":\"same\"}\n{\"id\":\"4\",\"v\":\"old\"}\n{\"id\":\"4\",\"v\":\"new\"}\n",
        )
        .expect("destination log");
        fs::write(
            facets.join("source/entities/ada/entity.json"),
            br#"{"name":"Ada","role":"source","seen":"x"}"#,
        )
        .expect("source entity");
        fs::write(
            facets.join("destination/entities/ada/entity.json"),
            br#"{"name":"Ada","role":"destination"}"#,
        )
        .expect("destination entity");
    }

    fn listed_after<'a>(stdout: &'a str, header: &str) -> Vec<&'a str> {
        stdout
            .lines()
            .skip_while(|line| !line.starts_with(header))
            .skip(1)
            .take_while(|line| line.starts_with("  "))
            .map(str::trim)
            .collect()
    }

    fn without_locks(tree: BTreeMap<PathBuf, Vec<u8>>) -> BTreeMap<PathBuf, Vec<u8>> {
        tree.into_iter()
            .filter(|(path, _)| !path.starts_with("health/locks"))
            .collect()
    }

    #[test]
    fn facet_merge_dry_run_changes_nothing_and_names_what_a_merge_would_lose() {
        let journal = TempJournal::new();
        seed_lossy_merge(&journal);
        let before = without_locks(tree_bytes(journal.path()));

        let outcome = facet_merge_preview_in_journal(journal.path(), "source", "destination");

        let Outcome::LocalSuccess { stdout, .. } = outcome else {
            panic!("dry run succeeds");
        };
        assert_eq!(without_locks(tree_bytes(journal.path())), before);
        assert!(
            stdout.starts_with("Dry run, nothing was changed."),
            "{stdout}"
        );
        assert_eq!(
            listed_after(&stdout, "Files both facets have that"),
            ["collision.md", "notes/today.md"]
        );
        assert_eq!(
            // id 1 loses to the destination; the destination's own second id 4 loses to its first.
            listed_after(&stdout, "Records that would be dropped"),
            ["log.jsonl: 2"]
        );
        assert_eq!(
            listed_after(&stdout, "Entity fields"),
            ["entities/ada/entity.json: 1"]
        );
        assert!(
            stdout.contains("The settings in the facet.json of 'source'"),
            "{stdout}"
        );
    }

    #[test]
    fn facet_merge_dry_run_lists_the_same_collisions_the_merge_then_reports() {
        let journal = TempJournal::new();
        seed_lossy_merge(&journal);

        let Outcome::LocalSuccess {
            stdout: preview, ..
        } = facet_merge_preview_in_journal(journal.path(), "source", "destination")
        else {
            panic!("dry run succeeds");
        };
        let Outcome::LocalSuccess { stdout: merged, .. } =
            facet_merge_in_journal(journal.path(), "source", "destination", false)
        else {
            panic!("merge succeeds");
        };

        let previewed = listed_after(&preview, "Files both facets have that");
        assert!(!previewed.is_empty());
        assert_eq!(
            previewed,
            listed_after(&merged, "Both facets had these files.")
        );
    }

    #[test]
    fn facet_merge_retires_the_source_name_onto_the_destination() {
        let journal = TempJournal::new();
        let Outcome::LocalSuccess { .. } =
            facet_merge_in_journal(journal.path(), "source", "destination", true)
        else {
            panic!("merge succeeds");
        };
        let entry = solstone_core_facets::retired_facet_entry(journal.path(), "source")
            .expect("record readable")
            .expect("source retired");
        assert_eq!(entry.state, solstone_core_facets::RetiredFacetState::Merged);
        assert_eq!(
            entry.id.as_deref(),
            Some("11111111-1111-4111-8111-111111111111")
        );
        assert_eq!(
            entry.successor.as_deref(),
            Some("22222222-2222-4222-8222-222222222222")
        );
        assert!(matches!(
            solstone_core_facets::create_facet(
                journal.path(),
                "source",
                "Source",
                "",
                "",
                "",
                None
            ),
            Err(solstone_core_facets::FacetWriteError::NameRetired { .. })
        ));
    }

    fn write_link(journal: &TempJournal, facet: &str, folder: &str, link: &str, notes: &str) {
        let dir = journal
            .path()
            .join("facets")
            .join(facet)
            .join("entities")
            .join(folder);
        fs::create_dir_all(&dir).expect("link folder");
        if !link.is_empty() {
            fs::write(dir.join("entity.json"), link).expect("link");
        }
        fs::write(dir.join("observations.jsonl"), notes).expect("notes");
    }

    fn notes_in(journal: &TempJournal, facet: &str, folder: &str) -> Vec<serde_json::Value> {
        fs::read_to_string(
            journal
                .path()
                .join("facets")
                .join(facet)
                .join("entities")
                .join(folder)
                .join("observations.jsonl"),
        )
        .expect("notes")
        .lines()
        .map(|line| serde_json::from_str(line).expect("note"))
        .collect()
    }

    fn folders_in(journal: &TempJournal, facet: &str) -> Vec<String> {
        let mut folders: Vec<String> =
            fs::read_dir(journal.path().join("facets").join(facet).join("entities"))
                .expect("entities")
                .map(|entry| {
                    entry
                        .expect("entry")
                        .file_name()
                        .into_string()
                        .expect("name")
                })
                .collect();
        folders.sort();
        folders
    }

    fn unique_ids(notes: &[serde_json::Value]) -> bool {
        let ids: std::collections::BTreeSet<u64> = notes
            .iter()
            .map(|note| note["id"].as_u64().expect("id"))
            .collect();
        ids.len() == notes.len()
    }

    #[test]
    fn facet_merge_combines_one_entitys_links_into_one_folder_keeping_every_note() {
        let journal = TempJournal::new();
        write_link(
            &journal,
            "source",
            "ada_lovelace",
            r#"{"entity_id":"ada","attached_at":"2026-01-01"}"#,
            "{\"id\":1,\"content\":\"from source\",\"observed_at\":1}\n",
        );
        write_link(
            &journal,
            "destination",
            "ada",
            r#"{"entity_id":"ada","attached_at":"2026-02-01","detached":true}"#,
            "{\"id\":1,\"content\":\"from destination\",\"observed_at\":2}\n",
        );
        let Outcome::LocalSuccess { stdout, .. } =
            facet_merge_preview_in_journal(journal.path(), "source", "destination")
        else {
            panic!("dry run succeeds");
        };
        assert!(
            stdout.contains("\n\n1 entity is linked in both facets; it would end up with one link"),
            "{stdout}"
        );
        assert!(
            matches!(
                facet_merge_in_journal(journal.path(), "source", "destination", false),
                Outcome::LocalSuccess { .. }
            ),
            "merge succeeds"
        );
        assert_eq!(folders_in(&journal, "destination"), ["ada"]);
        let notes = notes_in(&journal, "destination", "ada");
        assert_eq!(notes.len(), 2);
        assert!(unique_ids(&notes));
        let link: serde_json::Value = serde_json::from_slice(
            &fs::read(
                journal
                    .path()
                    .join("facets/destination/entities/ada/entity.json"),
            )
            .expect("link"),
        )
        .expect("link json");
        assert_eq!(link["attached_at"], "2026-01-01");
        assert_eq!(link.get("detached"), None, "a link still in a facet wins");
    }

    #[test]
    fn facet_merge_never_puts_one_entitys_notes_under_another() {
        let journal = TempJournal::new();
        write_link(
            &journal,
            "source",
            "bob",
            r#"{"entity_id":"robert"}"#,
            "{\"id\":1,\"content\":\"about robert\",\"observed_at\":1}\n",
        );
        write_link(
            &journal,
            "destination",
            "bob",
            r#"{"entity_id":"bob"}"#,
            "{\"id\":1,\"content\":\"about bob\",\"observed_at\":1}\n",
        );
        assert!(matches!(
            facet_merge_in_journal(journal.path(), "source", "destination", false),
            Outcome::LocalSuccess { .. }
        ));
        assert_eq!(folders_in(&journal, "destination"), ["bob", "robert"]);
        let bob = notes_in(&journal, "destination", "bob");
        assert_eq!(bob.len(), 1);
        assert_eq!(bob[0]["content"], "about bob");
        let robert = notes_in(&journal, "destination", "robert");
        assert_eq!(robert.len(), 1);
        assert_eq!(robert[0]["content"], "about robert");
    }

    #[test]
    fn facet_merge_gives_unlinked_notes_meeting_a_link_their_own_ids() {
        let journal = TempJournal::new();
        write_link(
            &journal,
            "source",
            "ada",
            "",
            "{\"id\":1,\"content\":\"unlinked note\",\"observed_at\":1}\n",
        );
        write_link(
            &journal,
            "destination",
            "ada",
            r#"{"entity_id":"ada"}"#,
            "{\"id\":1,\"content\":\"linked note\",\"observed_at\":2}\n",
        );
        assert!(matches!(
            facet_merge_in_journal(journal.path(), "source", "destination", false),
            Outcome::LocalSuccess { .. }
        ));
        let notes = notes_in(&journal, "destination", "ada");
        assert_eq!(notes.len(), 2);
        assert!(unique_ids(&notes), "{notes:?}");
    }

    #[test]
    fn facet_merge_dry_run_says_nothing_is_lost_when_nothing_is() {
        let journal = TempJournal::new();
        // A source with no settings of its own loses nothing.
        fs::remove_file(journal.path().join("facets/source/facet.json")).expect("no settings");

        let Outcome::LocalSuccess { stdout, .. } =
            facet_merge_preview_in_journal(journal.path(), "source", "destination")
        else {
            panic!("dry run succeeds");
        };
        assert!(stdout.ends_with("would lose:\nNothing.\n"), "{stdout}");
        assert!(journal.path().join("facets/source/source.txt").exists());
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

    use super::{adopt_orphan_facet, orphan_facets};

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
    fn an_orphan_is_adopted_through_the_facets_store_and_a_linked_declaration_is_unsafe() {
        let journal = TempJournal::new();
        let logs = journal.0.join("facets/work/logs/20260801.jsonl");
        fs::create_dir_all(logs.parent().unwrap()).unwrap();
        fs::write(logs, b"{\"message\":\"kept\"}\n").unwrap();
        assert_eq!(orphan_facets(&journal.0).unwrap(), vec!["work"]);

        assert_eq!(adopt_orphan_facet(&journal.0, "work", "t1").unwrap(), None);
        assert!(matches!(
            solstone_core_facets::observe_facet_destination(&journal.0, "work").unwrap(),
            solstone_core_facets::DestinationObservation::Ready { .. }
        ));
        assert_eq!(orphan_facets(&journal.0).unwrap(), Vec::<String>::new());
        // Adopting again is refused by the store, not written over.
        assert!(adopt_orphan_facet(&journal.0, "work", "t2").is_err());

        // A heal record that can't be written leaves the facet registered and
        // says so.
        let notes = journal.0.join("facets/notes/logs/20260801.jsonl");
        fs::create_dir_all(notes.parent().unwrap()).unwrap();
        fs::write(notes, b"{\"message\":\"kept\"}\n").unwrap();
        fs::remove_file(journal.0.join("logs/facet-heals.jsonl")).unwrap();
        fs::create_dir_all(journal.0.join("logs/facet-heals.jsonl")).unwrap();
        let record_failure = adopt_orphan_facet(&journal.0, "notes", "t3")
            .unwrap()
            .expect("the heal record could not be written");
        assert!(
            record_failure.contains("registered notes"),
            "{record_failure}"
        );
        assert!(matches!(
            solstone_core_facets::observe_facet_destination(&journal.0, "notes").unwrap(),
            solstone_core_facets::DestinationObservation::Ready { .. }
        ));

        #[cfg(unix)]
        {
            let other = journal.0.join("facets/other/news/20260801.md");
            fs::create_dir_all(other.parent().unwrap()).unwrap();
            fs::write(other, b"kept").unwrap();
            std::os::unix::fs::symlink(
                journal.0.join("facets/work/facet.json"),
                journal.0.join("facets/other/facet.json"),
            )
            .unwrap();
            let error = orphan_facets(&journal.0).unwrap_err();
            assert!(
                error.contains("unsafe facet declaration for other"),
                "{error}"
            );
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

/// Orphan folders that may be registered, and those under a retired name,
/// which never are.
fn adoptable_orphan_facets(journal: &Path) -> Result<(Vec<String>, Vec<String>), String> {
    let retired = solstone_core_facets::read_retired_facets(journal)
        .entries_for_write()
        .map_err(|error| error.to_string())?;
    let (retired_orphans, orphans) = orphan_facets(journal)?
        .into_iter()
        .partition(|slug| retired.contains_key(slug));
    Ok((orphans, retired_orphans))
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
        match solstone_core_facets::facet_declaration_entry(journal, &slug)
            .map_err(|error| error.to_string())?
        {
            Some(DirEntryKind::File) => continue,
            Some(_) => return Err(format!("unsafe facet declaration for {slug}")),
            None => {}
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

fn copy_tree(source: &Path, destination: &Path) -> Result<Vec<PathBuf>, String> {
    let mut copied_regular_files = Vec::new();
    copy_tree_into(
        source,
        destination,
        Path::new(""),
        &mut copied_regular_files,
    )?;
    copied_regular_files.sort();
    Ok(copied_regular_files)
}

fn copy_tree_into(
    source: &Path,
    destination: &Path,
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
        let name = entry.file_name();
        let kind = entry.file_type().map_err(|error| error.to_string())?;
        let target = destination.join(&name);
        let child_relative = relative.join(&name);
        if kind.is_dir() {
            copy_tree_into(
                &entry.path(),
                &target,
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
    /// Per `.jsonl` file: records dropped because a different record with the
    /// same id is kept (the first one, reading the destination first).
    jsonl_records_dropped: Vec<(PathBuf, usize)>,
    /// Per `entity.json`: fields both copies set to different values. The
    /// destination's value is kept.
    entity_fields_superseded: Vec<(PathBuf, usize)>,
    /// Entity links that joined the destination's link to the same entity.
    links_combined: usize,
    /// Notes those links brought, and how many took a fresh id.
    link_notes_added: usize,
    link_notes_renumbered: usize,
}

fn merge_tree(source: &Path, destination: &Path) -> Result<FacetTreeMergeReport, String> {
    let mut report = FacetTreeMergeReport::default();
    merge_tree_into(source, destination, Path::new(""), &mut report)?;
    report.copied_regular_files.sort();
    report.regular_file_collisions.sort();
    report.jsonl_records_dropped.sort();
    report.entity_fields_superseded.sort();
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
        // The facet's own settings record lives only at its root.
        if relative.as_os_str().is_empty() && entry.file_name() == OsStr::new("facet.json") {
            continue;
        }
        let name = entry.file_name();
        let target = destination.join(&name);
        let child_relative = relative.join(&name);
        // Entity links merge by the entity they link, not by folder name.
        if relative.as_os_str().is_empty()
            && name == OsStr::new("entities")
            && entry
                .file_type()
                .map_err(|error| error.to_string())?
                .is_dir()
            && matches!(
                existing_path_kind(&target)?,
                Some(ExistingPathKind::Directory)
            )
        {
            merge_entity_links(source, destination, report)?;
            continue;
        }
        merge_tree_entry(&entry, &target, &child_relative, report)?;
    }
    Ok(())
}

fn merge_tree_entry(
    entry: &fs::DirEntry,
    target: &Path,
    child_relative: &Path,
    report: &mut FacetTreeMergeReport,
) -> Result<(), String> {
    let kind = entry.file_type().map_err(|error| error.to_string())?;
    let child_relative = child_relative.to_path_buf();
    if kind.is_dir() {
        match existing_path_kind(target)? {
            Some(ExistingPathKind::Directory) => {
                merge_tree_into(&entry.path(), target, &child_relative, report)?;
            }
            None => {
                let copied = copy_tree(&entry.path(), target)?;
                report
                    .copied_regular_files
                    .extend(copied.into_iter().map(|path| child_relative.join(path)));
            }
            Some(_) => return Err(format!("unsafe facet entry: {}", target.display())),
        }
    } else if kind.is_file() {
        match existing_path_kind(target)? {
            None => {
                let bytes = fs::read(entry.path()).map_err(|error| error.to_string())?;
                write_bytes_exclusive(target, &bytes, AtomicWriteOptions { mode: Some(0o600) })
                    .map_err(|error| error.to_string())?;
                report.copied_regular_files.push(child_relative);
            }
            Some(ExistingPathKind::RegularFile)
                if entry.path().extension() == Some(OsStr::new("jsonl")) =>
            {
                let dropped = merge_jsonl(target, &entry.path())?;
                if dropped > 0 {
                    report.jsonl_records_dropped.push((child_relative, dropped));
                }
            }
            Some(ExistingPathKind::RegularFile)
                if entry.file_name() == OsStr::new("entity.json") =>
            {
                let superseded = merge_json_object(target, &entry.path())?;
                if superseded > 0 {
                    report
                        .entity_fields_superseded
                        .push((child_relative, superseded));
                }
            }
            Some(ExistingPathKind::RegularFile) => {
                report.regular_file_collisions.push(child_relative);
            }
            Some(_) => return Err(format!("unsafe facet entry: {}", target.display())),
        }
    } else {
        return Err(format!("unsafe facet entry: {}", entry.path().display()));
    }
    Ok(())
}

/// Merge a facet's `entities/` into the staged destination's. Each entity
/// link joins the destination's link to the same entity, in the folder named
/// by its id, keeping every note. Notes under a name no link claims join a
/// link folder of that name the same way, and otherwise overlay as any other
/// files do.
fn merge_entity_links(
    source: &Path,
    destination: &Path,
    report: &mut FacetTreeMergeReport,
) -> Result<(), String> {
    use solstone_core_facets::facet_links::{FolderState, LinkDirs, LinkFieldPolicy};
    let incoming = LinkDirs::at(source, "entities");
    let staged = LinkDirs::at(destination, "entities");
    let mut entries = fs::read_dir(source.join("entities"))
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    entries.sort_by_key(fs::DirEntry::file_name);
    // Links first, so notes under an unlinked name meet the link they belong
    // with however the folder names sort.
    let mut rest = Vec::new();
    let mut brought = std::collections::BTreeSet::new();
    for entry in entries {
        let is_dir = entry
            .file_type()
            .map_err(|error| error.to_string())?
            .is_dir();
        let name = entry.file_name();
        let link = match name.to_str() {
            Some(dir) if is_dir => match incoming.state(dir).map_err(|error| error.to_string())? {
                // A link whose id can't name a folder overlays as it always has.
                FolderState::Link(link)
                    if solstone_core_facets::facet_links::is_folder_name(&link.entity_id) =>
                {
                    Some((dir.to_owned(), link))
                }
                _ => None,
            },
            _ => None,
        };
        let Some((dir, link)) = link else {
            rest.push(entry);
            continue;
        };
        // Joined a link the destination already had, not one this merge
        // brought in a moment ago.
        let joined = !brought.contains(&link.entity_id)
            && staged
                .find(&link.entity_id)
                .map_err(|error| error.to_string())?
                .is_some();
        brought.insert(link.entity_id.clone());
        let (into, rows) = staged
            .take_in(
                &link.entity_id,
                &incoming,
                &dir,
                LinkFieldPolicy::Merge,
                &mut |_| Ok(()),
            )
            .map_err(|error| error.to_string())?;
        if joined {
            report.links_combined += 1;
        }
        if rows.fields_kept > 0 {
            report
                .entity_fields_superseded
                .push((PathBuf::from(staged.link_rel(&into)), rows.fields_kept));
        }
        report.link_notes_added += rows.added;
        report.link_notes_renumbered += rows.renumbered;
    }
    for entry in rest {
        let name = entry.file_name();
        let target = destination.join("entities").join(&name);
        let child_relative = Path::new("entities").join(&name);
        if entry
            .file_type()
            .map_err(|error| error.to_string())?
            .is_dir()
            && let Some(dir) = name.to_str()
            && incoming.state(dir).map_err(|error| error.to_string())? == FolderState::Orphan
            && matches!(
                staged.state(dir).map_err(|error| error.to_string())?,
                FolderState::Link(_)
            )
        {
            let rows = staged
                .fold_into(
                    &incoming,
                    dir,
                    dir,
                    LinkFieldPolicy::TargetWins,
                    &mut |_| Ok(()),
                )
                .map_err(|error| error.to_string())?;
            report.link_notes_added += rows.added;
            report.link_notes_renumbered += rows.renumbered;
            continue;
        }
        merge_tree_entry(&entry, &target, &child_relative, report)?;
    }
    Ok(())
}

/// Returns how many records were dropped for a different record with the
/// same id. Identical duplicates lose nothing and are not counted.
fn merge_jsonl(destination: &Path, source: &Path) -> Result<usize, String> {
    let destination_text = fs::read_to_string(destination).map_err(|error| error.to_string())?;
    let source_text = fs::read_to_string(source).map_err(|error| error.to_string())?;
    let mut kept_by_id: BTreeMap<String, &str> = BTreeMap::new();
    let mut seen_lines = BTreeSet::new();
    let mut lines = Vec::new();
    let mut dropped = 0;
    for line in destination_text.lines().chain(source_text.lines()) {
        if line.trim().is_empty() {
            continue;
        }
        let id = serde_json::from_str::<Value>(line)
            .ok()
            .and_then(|value| value.get("id").and_then(Value::as_str).map(str::to_owned));
        let keep = match id {
            Some(id) if !id.is_empty() => match kept_by_id.get(&id) {
                Some(kept) => {
                    if *kept != line {
                        dropped += 1;
                    }
                    false
                }
                None => {
                    kept_by_id.insert(id, line);
                    true
                }
            },
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
    .map_err(|error| error.to_string())?;
    Ok(dropped)
}

/// Returns how many source fields lost to a different destination value.
fn merge_json_object(destination: &Path, source: &Path) -> Result<usize, String> {
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
    let superseded = destination_map
        .iter()
        .filter(|(key, value)| merged.get(*key).is_some_and(|source| source != *value))
        .count();
    merged.extend(destination_map);
    let mut bytes =
        serde_json::to_vec_pretty(&Value::Object(merged)).map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    atomic_replace(
        destination,
        &bytes,
        AtomicWriteOptions { mode: Some(0o600) },
    )
    .map_err(|error| error.to_string())?;
    Ok(superseded)
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
