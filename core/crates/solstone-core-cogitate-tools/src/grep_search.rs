// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io::Read;
use std::path::Path;

use crate::denylist::{broad_recursive_refusal, classify, refusal_for};
use crate::paths::{ContainedPathError, journal_root_real, resolve_target};
use crate::patterns::{fnmatch, grep_matcher};
use crate::read_file::{decode_clipped, is_special, nonnegative, splitlines, stat_refusal};
use crate::refusals::{
    NOTICE_GREP_TRUNCATED, REFUSAL_BAD_PATH, REFUSAL_BAD_PATTERN, REFUSAL_BROAD_ROOT,
    REFUSAL_PATH_ESCAPE, REFUSAL_PERMISSION_DENIED, REFUSAL_SPECIAL_FILE, charge, ok, refused,
};
use crate::walk::{journal_rel, walk_allowed};
use crate::{GrepMatch, GrepSearchOptions, ReadBudget, ReadPayload, ReadResult, ToolName};

/// With `regex=true`, accepts the linear-time regex crate's strict subset of
/// Python `re` syntax; backreferences and lookaround are unavailable.
pub fn grep_search(
    journal: &Path,
    pattern: &str,
    path: &str,
    options: &GrepSearchOptions,
    budget: Option<&mut ReadBudget>,
) -> ReadResult {
    if let Some(result) = charge(ToolName::GrepSearch, budget) {
        return result;
    }
    let matcher = match grep_matcher(pattern, options.regex, options.case_sensitive) {
        Ok(matcher) => matcher,
        Err(_) => return refused(ToolName::GrepSearch, REFUSAL_BAD_PATTERN),
    };
    let root = match journal_root_real(journal) {
        Ok(root) => root,
        Err(_) => return refused(ToolName::GrepSearch, REFUSAL_PERMISSION_DENIED),
    };
    let resolved = match resolve_target(journal, path) {
        Ok(path) => path,
        Err(ContainedPathError::Invalid) => return refused(ToolName::GrepSearch, REFUSAL_BAD_PATH),
        Err(ContainedPathError::Escape) => {
            return refused(ToolName::GrepSearch, REFUSAL_PATH_ESCAPE);
        }
        Err(ContainedPathError::Io) => {
            return refused(ToolName::GrepSearch, REFUSAL_PERMISSION_DENIED);
        }
    };
    if let Some(reason) = refusal_for(classify(&resolved, &root)) {
        return refused(ToolName::GrepSearch, reason);
    }
    let metadata = match fs::metadata(&resolved) {
        Ok(metadata) => metadata,
        Err(error) => return refused(ToolName::GrepSearch, stat_refusal(&error)),
    };
    if is_special(&metadata) {
        return refused(ToolName::GrepSearch, REFUSAL_SPECIAL_FILE);
    }
    let match_limit = nonnegative(options.max_matches);
    if metadata.is_file() {
        let Some(rel) = journal_rel(&resolved, &root) else {
            return refused(ToolName::GrepSearch, REFUSAL_PERMISSION_DENIED);
        };
        if options
            .file_glob
            .as_deref()
            .is_some_and(|glob| !fnmatch(&rel, glob))
        {
            return ok(
                ToolName::GrepSearch,
                ReadPayload::Matches(Vec::new()),
                false,
                NOTICE_GREP_TRUNCATED,
            );
        }
        let (file_matches, byte_truncated, refusal) =
            grep_file(&resolved, &root, &matcher, options);
        if let Some(refusal) = refusal {
            return refused(ToolName::GrepSearch, refusal);
        }
        let truncated = byte_truncated || file_matches.len() > match_limit;
        return ok(
            ToolName::GrepSearch,
            ReadPayload::Matches(file_matches.into_iter().take(match_limit).collect()),
            truncated,
            NOTICE_GREP_TRUNCATED,
        );
    }
    if !metadata.is_dir() {
        return ok(
            ToolName::GrepSearch,
            ReadPayload::Matches(Vec::new()),
            false,
            NOTICE_GREP_TRUNCATED,
        );
    }
    if broad_recursive_refusal(&resolved, &root) {
        return refused(ToolName::GrepSearch, REFUSAL_BROAD_ROOT);
    }
    let file_limit = nonnegative(options.max_files);
    let mut files = 0usize;
    let mut matches = Vec::new();
    let mut truncated = false;
    for entry in walk_allowed(journal, &root, &resolved, options.include_hidden) {
        if entry.is_dir {
            continue;
        }
        let Ok(metadata) = fs::metadata(&entry.path) else {
            continue;
        };
        if is_special(&metadata) || !metadata.is_file() {
            continue;
        }
        let Some(rel) = journal_rel(&entry.path, &root) else {
            continue;
        };
        if options
            .file_glob
            .as_deref()
            .is_some_and(|glob| !fnmatch(&rel, glob))
        {
            continue;
        }
        if files >= file_limit {
            truncated = true;
            break;
        }
        files += 1;
        let (file_matches, byte_truncated, _) = grep_file(&entry.path, &root, &matcher, options);
        truncated |= byte_truncated;
        for item in file_matches {
            if matches.len() >= match_limit {
                truncated = true;
                break;
            }
            matches.push(item);
        }
        if matches.len() >= match_limit {
            break;
        }
    }
    ok(
        ToolName::GrepSearch,
        ReadPayload::Matches(matches),
        truncated,
        NOTICE_GREP_TRUNCATED,
    )
}

fn grep_file(
    path: &Path,
    root: &Path,
    matcher: &regex::Regex,
    options: &GrepSearchOptions,
) -> (Vec<GrepMatch>, bool, Option<&'static str>) {
    let limit = nonnegative(options.max_bytes_per_file);
    let mut raw = Vec::new();
    let read = fs::File::open(path).and_then(|mut file| {
        file.by_ref()
            .take(limit.saturating_add(1) as u64)
            .read_to_end(&mut raw)
    });
    if let Err(error) = read {
        return (Vec::new(), false, Some(stat_refusal(&error)));
    }
    if raw[..raw.len().min(8192)].contains(&0) {
        return (Vec::new(), false, None);
    }
    let truncated = raw.len() > limit;
    let Some(text) = decode_clipped(&raw[..raw.len().min(limit)]) else {
        return (Vec::new(), truncated, None);
    };
    let lines = splitlines(&text);
    let context = nonnegative(options.context_lines);
    let Some(path) = journal_rel(path, root) else {
        return (Vec::new(), false, Some(REFUSAL_PERMISSION_DENIED));
    };
    let mut matches = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if !matcher.is_match(line) {
            continue;
        }
        matches.push(GrepMatch {
            path: path.clone(),
            lineno: index + 1,
            line: (*line).to_owned(),
            before: lines[index.saturating_sub(context)..index]
                .iter()
                .map(|line| (*line).to_owned())
                .collect(),
            after: lines[index + 1..lines.len().min(index + 1 + context)]
                .iter()
                .map(|line| (*line).to_owned())
                .collect(),
        });
    }
    (matches, truncated, None)
}
