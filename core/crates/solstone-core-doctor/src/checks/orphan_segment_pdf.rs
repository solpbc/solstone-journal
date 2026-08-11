// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use crate::{
    context::CheckContext,
    vocabulary::{Check, RunnerResult, Status, make_result, truncate},
};
use std::path::{Path, PathBuf};
pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    let chronicle = context.journal_path.join("chronicle");
    if !chronicle.is_dir() {
        return Ok(make_result(
            check,
            Status::Skip,
            "chronicle directory unavailable",
            None::<String>,
        ));
    }
    let mut files = Vec::new();
    visit(&chronicle, 0, &mut files);
    let mut orphans = files
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
            "all raw PDF originals have readable document transcripts",
            None::<String>,
        ))
    } else {
        Ok(make_result(
            check,
            Status::Warn,
            format!(
                "{} raw PDF original(s) without a readable document transcript: {}",
                orphans.len(),
                truncate(&orphans.join(", "), 360)
            ),
            Some(
                "journal maint --force settings:007_migrate_pdf_extractions, then re-run journal doctor",
            ),
        ))
    }
}
fn visit(root: &Path, depth: usize, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let mut entries = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    entries.sort();
    for entry in entries {
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
