// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::types::{ReadPayload, ReadResult, ToolName};

pub const REFUSAL_PATH_ESCAPE: &str = "path_escape: refused a path that resolves outside the journal; use a journal-root-relative path inside the journal.";
pub const REFUSAL_DENIED_COMPONENT: &str = "denied_component: refused a path under a blocked component; use a journal evidence path outside caches, dependencies, or private config.";
pub const REFUSAL_CREDENTIAL_FILE: &str = "credential_file: refused a credential-like file; use a non-secret journal evidence file or a domain command that reports safe status.";
pub const REFUSAL_NOT_FILE: &str = "not_a_file: refused a directory or non-regular target; choose a regular text file path instead.";
pub const REFUSAL_BINARY: &str = "binary_file: refused binary or non-UTF-8 content; use a text export or a domain command that summarizes the file.";
pub const REFUSAL_SPECIAL_FILE: &str =
    "special_file: refused a socket, device, or FIFO; use a regular text file inside the journal.";
pub const REFUSAL_MISSING: &str = "missing_or_dangling: refused a missing path or dangling symlink; choose an existing journal path.";
pub const REFUSAL_PERMISSION_DENIED: &str = "permission_denied: refused a path the process cannot read; choose a readable journal file or use a domain command.";
pub const REFUSAL_BAD_PATH: &str = "bad_path: refused an invalid journal-relative path; use POSIX separators without absolute, empty, '.', or '..' components.";
pub const REFUSAL_BAD_PATTERN: &str = "bad_pattern: refused an invalid regex pattern; fix the regex pattern or drop regex=True for a literal search.";
pub const REFUSAL_BUDGET_EXHAUSTED: &str = "budget_exhausted: read-call budget is exhausted; stop raw reads and use the evidence already gathered or a domain command.";
pub const REFUSAL_BROAD_ROOT: &str = "broad_root: refused a recursive scan of the whole journal, chronicle, or facets before any narrowing; scope to a day 'chronicle/YYYYMMDD', a facet tree 'facets/<facet>', the 'entities' tree, or an exact file path.";
pub const REFUSAL_TOOL_NOT_BOUND: &str = "tool_not_bound: this tool is not available for this cogitate run; use one of the tools provided for this run or finish with the best result available.";
pub const NOTICE_READ_FILE_TRUNCATED: &str = "read_file_truncated: hit max_lines or max_bytes; use start_line to continue or choose a smaller file.";
pub const NOTICE_LIST_DIRECTORY_TRUNCATED: &str =
    "list_directory_truncated: hit max_entries; narrow with pattern or list a subdirectory.";
pub const NOTICE_GLOB_TRUNCATED: &str =
    "glob_truncated: hit max_matches; use a more specific pattern or root.";
pub const NOTICE_GREP_TRUNCATED: &str =
    "grep_search_truncated: hit match, file, or byte cap; narrow pattern, path, or file_glob.";

pub(crate) fn refused(tool: ToolName, refusal: &'static str) -> ReadResult {
    let payload = match tool {
        ToolName::ReadFile => ReadPayload::Text(String::new()),
        ToolName::ListDirectory => ReadPayload::Entries(Vec::new()),
        ToolName::Glob => ReadPayload::Paths(Vec::new()),
        ToolName::GrepSearch => ReadPayload::Matches(Vec::new()),
    };
    ReadResult {
        tool,
        ok: false,
        payload,
        refusal: Some(refusal),
        truncated: false,
        notice: None,
    }
}
pub(crate) fn ok(
    tool: ToolName,
    payload: ReadPayload,
    truncated: bool,
    notice: &'static str,
) -> ReadResult {
    ReadResult {
        tool,
        ok: true,
        payload,
        refusal: None,
        truncated,
        notice: truncated.then_some(notice),
    }
}
pub(crate) fn charge(tool: ToolName, budget: Option<&mut crate::ReadBudget>) -> Option<ReadResult> {
    budget.and_then(|budget| (!budget.charge()).then(|| refused(tool, REFUSAL_BUDGET_EXHAUSTED)))
}
