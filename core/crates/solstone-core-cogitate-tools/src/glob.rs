// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::Path;

use crate::denylist::{broad_recursive_refusal, classify, refusal_for};
use crate::paths::{ContainedPathError, journal_root_real, resolve_target};
use crate::patterns::fnmatch;
use crate::read_file::{nonnegative, stat_refusal};
use crate::refusals::{
    NOTICE_GLOB_TRUNCATED, REFUSAL_BAD_PATH, REFUSAL_BROAD_ROOT, REFUSAL_PATH_ESCAPE,
    REFUSAL_PERMISSION_DENIED, charge, ok, refused,
};
use crate::walk::{journal_rel, walk_allowed};
use crate::{GlobOptions, ReadBudget, ReadPayload, ReadResult, ToolName};

pub fn glob(
    journal: &Path,
    pattern: &str,
    root_path: &str,
    options: &GlobOptions,
    budget: Option<&mut ReadBudget>,
) -> ReadResult {
    if let Some(result) = charge(ToolName::Glob, budget) {
        return result;
    }
    let root = match journal_root_real(journal) {
        Ok(root) => root,
        Err(_) => return refused(ToolName::Glob, REFUSAL_PERMISSION_DENIED),
    };
    let resolved = match resolve_target(journal, root_path) {
        Ok(path) => path,
        Err(ContainedPathError::Invalid) => return refused(ToolName::Glob, REFUSAL_BAD_PATH),
        Err(ContainedPathError::Escape) => return refused(ToolName::Glob, REFUSAL_PATH_ESCAPE),
        Err(ContainedPathError::Io) => return refused(ToolName::Glob, REFUSAL_PERMISSION_DENIED),
    };
    if let Some(reason) = refusal_for(classify(&resolved, &root)) {
        return refused(ToolName::Glob, reason);
    }
    if broad_recursive_refusal(&resolved, &root) {
        return refused(ToolName::Glob, REFUSAL_BROAD_ROOT);
    }
    let metadata = match fs::metadata(&resolved) {
        Ok(metadata) => metadata,
        Err(error) => return refused(ToolName::Glob, stat_refusal(&error)),
    };
    if !metadata.is_dir() {
        return ok(
            ToolName::Glob,
            ReadPayload::Paths(Vec::new()),
            false,
            NOTICE_GLOB_TRUNCATED,
        );
    }
    let limit = nonnegative(options.max_matches);
    let mut matches = Vec::new();
    let mut truncated = false;
    for item in walk_allowed(journal, &root, &resolved, options.include_hidden) {
        let Some(rel) = journal_rel(&item.path, &root) else {
            continue;
        };
        if !fnmatch(&rel, pattern) {
            continue;
        }
        if matches.len() >= limit {
            truncated = true;
            break;
        }
        matches.push(rel);
    }
    ok(
        ToolName::Glob,
        ReadPayload::Paths(matches),
        truncated,
        NOTICE_GLOB_TRUNCATED,
    )
}
