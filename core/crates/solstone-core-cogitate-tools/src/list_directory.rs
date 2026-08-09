// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::Path;

use crate::denylist::{broad_recursive_refusal, classify, refusal_for};
use crate::paths::{ContainedPathError, journal_root_real, resolve_target};
use crate::patterns::fnmatch;
use crate::read_file::{nonnegative, stat_refusal};
use crate::refusals::{
    NOTICE_LIST_DIRECTORY_TRUNCATED, REFUSAL_BAD_PATH, REFUSAL_BROAD_ROOT, REFUSAL_PATH_ESCAPE,
    REFUSAL_PERMISSION_DENIED, charge, ok, refused,
};
use crate::walk::{iter_allowed_children, journal_rel, walk_allowed};
use crate::{Entry, ListDirectoryOptions, ReadBudget, ReadPayload, ReadResult, ToolName};

pub fn list_directory(
    journal: &Path,
    path: &str,
    options: &ListDirectoryOptions,
    budget: Option<&mut ReadBudget>,
) -> ReadResult {
    if let Some(result) = charge(ToolName::ListDirectory, budget) {
        return result;
    }
    let root = match journal_root_real(journal) {
        Ok(root) => root,
        Err(_) => return refused(ToolName::ListDirectory, REFUSAL_PERMISSION_DENIED),
    };
    let resolved = match resolve_target(journal, path) {
        Ok(path) => path,
        Err(ContainedPathError::Invalid) => {
            return refused(ToolName::ListDirectory, REFUSAL_BAD_PATH);
        }
        Err(ContainedPathError::Escape) => {
            return refused(ToolName::ListDirectory, REFUSAL_PATH_ESCAPE);
        }
        Err(ContainedPathError::Io) => {
            return refused(ToolName::ListDirectory, REFUSAL_PERMISSION_DENIED);
        }
    };
    if let Some(reason) = refusal_for(classify(&resolved, &root)) {
        return refused(ToolName::ListDirectory, reason);
    }
    if options.recursive && broad_recursive_refusal(&resolved, &root) {
        return refused(ToolName::ListDirectory, REFUSAL_BROAD_ROOT);
    }
    let metadata = match fs::metadata(&resolved) {
        Ok(metadata) => metadata,
        Err(error) => return refused(ToolName::ListDirectory, stat_refusal(&error)),
    };
    if !metadata.is_dir() {
        return ok(
            ToolName::ListDirectory,
            ReadPayload::Entries(Vec::new()),
            false,
            NOTICE_LIST_DIRECTORY_TRUNCATED,
        );
    }
    let walked: Box<dyn Iterator<Item = crate::walk::WalkEntry>> = if options.recursive {
        Box::new(walk_allowed(
            journal,
            &root,
            &resolved,
            options.include_hidden,
        ))
    } else {
        Box::new(
            iter_allowed_children(journal, &root, &resolved, options.include_hidden).into_iter(),
        )
    };
    let limit = nonnegative(options.max_entries);
    let mut entries = Vec::new();
    let mut truncated = false;
    for item in walked {
        let name = item
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if options
            .pattern
            .as_deref()
            .is_some_and(|pattern| !fnmatch(name, pattern))
        {
            continue;
        }
        if entries.len() >= limit {
            truncated = true;
            break;
        }
        let Some(path) = journal_rel(&item.path, &root) else {
            continue;
        };
        entries.push(Entry {
            path,
            is_dir: item.is_dir,
        });
    }
    ok(
        ToolName::ListDirectory,
        ReadPayload::Entries(entries),
        truncated,
        NOTICE_LIST_DIRECTORY_TRUNCATED,
    )
}
