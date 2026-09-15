// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Closed read-only MCP tool validation and execution.

pub(crate) mod entities;
pub(crate) mod facets;
pub(crate) mod fetch;
pub(crate) mod search;
pub(crate) mod transcripts;

pub(crate) use entities::{ValidatedGetEntity, ValidatedListEntities};
pub(crate) use facets::ValidatedListFacets;
pub(crate) use fetch::ValidatedFetch;
pub(crate) use search::ValidatedSearch;
pub(crate) use transcripts::{ValidatedGetTranscript, ValidatedListTranscripts};

pub(crate) const MAX_OPAQUE_REFERENCE_BYTES: usize = 2_048;
pub(crate) const MAX_DAY_BYTES: usize = 32;
pub(crate) const MAX_FACET_BYTES: usize = 256;

pub(crate) fn optional_string_within_limit(value: &Option<String>, maximum: usize) -> bool {
    value
        .as_ref()
        .is_none_or(|value| !value.is_empty() && value.len() <= maximum)
}

/// One validated tool call whose schema has been admitted for audit.
pub(crate) enum ValidatedTool {
    ListFacets(ValidatedListFacets),
    Search(ValidatedSearch),
    Fetch(ValidatedFetch),
    ListTranscripts(ValidatedListTranscripts),
    GetTranscript(ValidatedGetTranscript),
    ListEntities(ValidatedListEntities),
    GetEntity(ValidatedGetEntity),
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
    FileUnreadable,
    AuditUnavailable,
    ReferenceNotFound,
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
            Self::FileUnreadable => "file_unreadable",
            Self::AuditUnavailable => "audit_unavailable",
            Self::ReferenceNotFound => "not_found",
        }
    }
}

/// Keep audit publication as a mandatory predecessor of native tool execution.
///
/// The admission's own value — its coordinates — comes back alongside the
/// execution result so the outcome sibling can be published against the same
/// segment, ⚠ including when the execution fails. The execution result is
/// returned rather than propagated for exactly that reason: an errored call
/// still owes the owner a record of how it ended.
pub(crate) fn execute_after_audit<A, T, E>(
    audit: impl FnOnce() -> Result<A, E>,
    executor: impl FnOnce() -> Result<T, E>,
) -> Result<(A, Result<T, E>), E> {
    let admitted = audit()?;
    Ok((admitted, executor()))
}

#[cfg(all(test, not(feature = "full-tests")))]
mod tests {
    use std::cell::Cell;

    use super::{ToolError, execute_after_audit};

    #[test]
    fn audit_failure_prevents_native_execution() {
        let invoked = Cell::new(false);
        let result = execute_after_audit(
            || Err::<(), _>(ToolError::AuditUnavailable),
            || {
                invoked.set(true);
                Ok(())
            },
        );
        assert_eq!(result.err(), Some(ToolError::AuditUnavailable));
        assert!(!invoked.get());
    }

    #[test]
    fn an_admitted_call_that_fails_still_hands_back_its_admission() {
        // ⚠ The failure path is where the owner's record is easiest to lose:
        // a `?` here would drop the coordinates the outcome has to be written
        // against, and the call would read as uncertain instead of errored.
        let (admitted, executed) = execute_after_audit(
            || Ok::<_, ToolError>("coordinates"),
            || Err::<(), _>(ToolError::IndexAbsent),
        )
        .expect("admission succeeded");
        assert_eq!(admitted, "coordinates");
        assert_eq!(executed.err(), Some(ToolError::IndexAbsent));
    }
}
