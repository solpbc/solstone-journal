// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::{
    context::CheckContext,
    vocabulary::{Check, RunnerResult, Status, make_result, truncate},
};
use solstone_core_cli_copy as copy;
use std::path::{Path, PathBuf};
pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    let chronicle = context.journal_path.join("chronicle");
    if !chronicle.is_dir() {
        return Ok(make_result(
            check,
            Status::Skip,
            copy::text("journal.cli.doctor.orphan.segment.pdf.chronicle.unavailable"),
            None::<String>,
        ));
    }
    let mut orphans = segment_files(&context.journal_path, &chronicle)
        .into_iter()
        .filter(|path| {
            path.extension()
                .and_then(|value| value.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
        })
        .filter(|path| !has_transcript(path.parent().unwrap_or(Path::new(""))))
        .filter_map(|path| {
            path.strip_prefix(&context.journal_path)
                .ok()
                .map(|path| path.display().to_string())
        })
        .collect::<Vec<_>>();
    orphans.sort();
    if orphans.is_empty() {
        Ok(make_result(
            check,
            Status::Ok,
            copy::text("journal.cli.doctor.orphan.segment.pdf.clear"),
            None::<String>,
        ))
    } else {
        Ok(make_result(
            check,
            Status::Warn,
            context.report_form.counted(
                "journal.cli.doctor.orphan.pdf.found",
                &[("quantity", orphans.len() as u64)],
                &[
                    ("count", &orphans.len()),
                    ("paths", &(truncate(&orphans.join(", "), 360))),
                ],
            ),
            Some(copy::text("journal.cli.doctor.orphan.segment.pdf.repair")),
        ))
    }
}
/// Files held directly in a chronicle segment, in both layouts: a segment
/// under a stream directory (`<day>/<stream>/<segment>/`) and a default-stream
/// segment that sits directly in the day (`<day>/<segment>/`).
fn segment_files(journal: &Path, chronicle: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    visit(chronicle, 0, &mut files);
    for day in sorted_entries(chronicle) {
        if !day.is_dir() {
            continue;
        }
        let Ok(segments) = solstone_core_journal_io::iter_segments_skipping_unreadable_streams(
            journal,
            solstone_core_journal_io::PathOrDay::Directory(&day),
        ) else {
            continue;
        };
        for segment in segments
            .iter()
            .filter(|segment| segment.stream().is_direct())
        {
            files.extend(
                sorted_entries(segment.path())
                    .into_iter()
                    .filter(|entry| entry.is_file()),
            );
        }
    }
    files.sort();
    files.dedup();
    files
}
fn sorted_entries(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut entries = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    entries.sort();
    entries
}
fn visit(root: &Path, depth: usize, files: &mut Vec<PathBuf>) {
    for entry in sorted_entries(root) {
        if depth == 3 {
            if entry.is_file() {
                files.push(entry);
            }
        } else if entry.is_dir() {
            visit(&entry, depth + 1, files);
        }
    }
}
fn has_transcript(parent: &Path) -> bool {
    std::fs::read_dir(parent).ok().is_some_and(|entries| {
        entries.filter_map(Result::ok).any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .ends_with("_transcript.md")
        })
    })
}
