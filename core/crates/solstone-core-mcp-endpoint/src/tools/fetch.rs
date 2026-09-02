// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use serde::Deserialize;
use serde_json::{Value, json};
use solstone_core_indexer_query::{IndexAccessError, hit_at};
use solstone_core_journal_io::JournalReadError;
use solstone_core_journal_io::bounded_read::read_text;

use super::ToolError;

/// A fetch identifier that has passed syntactic validation before auditing.
pub(crate) struct ValidatedFetch {
    path: String,
    idx: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FetchParams {
    id: String,
}

pub(crate) fn validate(params: Option<&Value>) -> Result<ValidatedFetch, ToolError> {
    let params = params.cloned().ok_or(ToolError::InvalidInput)?;
    let params =
        serde_json::from_value::<FetchParams>(params).map_err(|_| ToolError::InvalidInput)?;
    let Some((path, suffix)) = params.id.rsplit_once(':') else {
        return Err(ToolError::InvalidInput);
    };
    if path.is_empty() || suffix.is_empty() || !suffix.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(ToolError::InvalidInput);
    }
    let idx = suffix.parse::<i64>().map_err(|_| ToolError::InvalidInput)?;
    Ok(ValidatedFetch {
        path: path.to_owned(),
        idx,
    })
}

pub(crate) fn execute(journal_root: &Path, request: &ValidatedFetch) -> Result<Value, ToolError> {
    if !hit_at(journal_root, &request.path, request.idx).map_err(map_index_error)? {
        return Err(ToolError::NotIndexed);
    }
    let content = read_text(journal_root, &request.path).map_err(map_read_error)?;
    Ok(json!({ "content": content }))
}

fn map_index_error(error: IndexAccessError) -> ToolError {
    match error {
        IndexAccessError::Absent { .. } => ToolError::IndexAbsent,
        IndexAccessError::Unreadable { .. } => ToolError::IndexUnreadable,
        IndexAccessError::Locked { .. } => ToolError::IndexLocked,
        IndexAccessError::Empty { .. } => ToolError::EmptyIndex,
    }
}

fn map_read_error(error: JournalReadError) -> ToolError {
    match error {
        JournalReadError::Path(_) => ToolError::InvalidPath,
        JournalReadError::TooLarge(_) => ToolError::FileTooLarge,
        JournalReadError::Encoding(_) => ToolError::FileNotUtf8,
        JournalReadError::NotFound => ToolError::FileNotFound,
        JournalReadError::Io => ToolError::FileUnreadable,
    }
}

#[cfg(all(test, not(feature = "full-tests")))]
mod tests {
    use serde_json::json;
    use solstone_core_journal_io::JournalReadError;

    use super::{execute, map_read_error, validate};
    use crate::tools::ToolError;

    #[test]
    fn final_colon_split_accepts_colons_in_paths_and_rejects_invalid_ids() {
        assert!(validate(Some(&json!({"id": "notes/a:b.txt:42"}))).is_ok());
        for value in [
            json!({"id": "notes.txt"}),
            json!({"id": "notes.txt:"}),
            json!({"id": "notes.txt:not-a-number"}),
            json!({"id": ":12"}),
        ] {
            assert!(matches!(
                validate(Some(&value)),
                Err(ToolError::InvalidInput)
            ));
        }
    }

    #[test]
    fn fetch_does_not_serve_a_syntactically_valid_identifier_without_an_index() {
        let journal = tempfile::tempdir().expect("fixture journal");
        let unindexed =
            validate(Some(&json!({"id": "notes/unindexed.txt:3"}))).expect("valid fetch id");
        assert_eq!(
            execute(journal.path(), &unindexed),
            Err(ToolError::IndexAbsent)
        );
    }

    #[test]
    fn contained_reader_errors_have_closed_nonleaking_reason_codes() {
        for (error, expected) in [
            (
                JournalReadError::Path("/journal/private".to_owned()),
                ToolError::InvalidPath,
            ),
            (
                JournalReadError::TooLarge("/journal/private".to_owned()),
                ToolError::FileTooLarge,
            ),
            (
                JournalReadError::Encoding("/journal/private".to_owned()),
                ToolError::FileNotUtf8,
            ),
            (JournalReadError::NotFound, ToolError::FileNotFound),
            (JournalReadError::Io, ToolError::FileUnreadable),
        ] {
            assert_eq!(map_read_error(error), expected);
        }
    }
}
