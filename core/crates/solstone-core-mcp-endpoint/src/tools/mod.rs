// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Closed read-only MCP tool validation and execution.

mod fetch;
mod search;

use std::path::Path;

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::jsonrpc::ToolName;

pub(crate) use fetch::ValidatedFetch;
pub(crate) use search::ValidatedSearch;

/// One validated tool call whose schema has been admitted for audit.
pub(crate) enum ValidatedTool {
    Search(ValidatedSearch),
    Fetch(ValidatedFetch),
}

/// A public-safe reason to reject or fail one read-only tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolError {
    InvalidInput,
    IndexAbsent,
    IndexUnreadable,
    IndexLocked,
    EmptyIndex,
    NotIndexed,
    InvalidPath,
    FileTooLarge,
    FileNotUtf8,
    FileNotFound,
    FileUnreadable,
    Serialization,
    AuditUnavailable,
}

impl ToolError {
    pub(crate) fn reason(self) -> &'static str {
        match self {
            Self::InvalidInput => "invalid_tool_input",
            Self::IndexAbsent => "index_absent",
            Self::IndexUnreadable => "index_unreadable",
            Self::IndexLocked => "index_locked",
            Self::EmptyIndex => "empty_index",
            Self::NotIndexed => "not_indexed",
            Self::InvalidPath => "invalid_path",
            Self::FileTooLarge => "file_too_large",
            Self::FileNotUtf8 => "file_not_utf8",
            Self::FileNotFound => "file_not_found",
            Self::FileUnreadable => "file_unreadable",
            Self::Serialization => "tool_result_unavailable",
            Self::AuditUnavailable => "audit_unavailable",
        }
    }
}

/// Validate all tool input before the call is eligible for durable auditing.
pub(crate) fn validate(
    tool_name: ToolName,
    params: Option<&Value>,
) -> Result<ValidatedTool, ToolError> {
    match tool_name {
        ToolName::Search => search::validate(params).map(ValidatedTool::Search),
        ToolName::Fetch => fetch::validate(params).map(ValidatedTool::Fetch),
    }
}

/// Execute a previously validated read-only tool after audit publication succeeds.
pub(crate) fn execute(
    journal_root: &Path,
    tool: &ValidatedTool,
    now: DateTime<Utc>,
) -> Result<Value, ToolError> {
    match tool {
        ValidatedTool::Search(request) => search::execute(journal_root, request, now),
        ValidatedTool::Fetch(request) => fetch::execute(journal_root, request),
    }
}

/// Keep audit publication as a mandatory predecessor of native tool execution.
pub(crate) fn execute_after_audit<T>(
    audit: impl FnOnce() -> Result<(), ToolError>,
    executor: impl FnOnce() -> Result<T, ToolError>,
) -> Result<T, ToolError> {
    audit()?;
    executor()
}

#[cfg(all(test, not(feature = "full-tests")))]
mod tests {
    use std::cell::Cell;

    use super::{ToolError, execute_after_audit};

    #[test]
    fn audit_failure_prevents_native_execution() {
        let invoked = Cell::new(false);
        let result = execute_after_audit(
            || Err(ToolError::AuditUnavailable),
            || {
                invoked.set(true);
                Ok(())
            },
        );
        assert_eq!(result, Err(ToolError::AuditUnavailable));
        assert!(!invoked.get());
    }
}
