// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::denylist::{
    GLOB_MAX_MATCHES, GREP_MAX_BYTES_PER_FILE, GREP_MAX_FILES, GREP_MAX_MATCHES,
    LIST_DIRECTORY_MAX_ENTRIES, READ_FILE_MAX_BYTES, READ_FILE_MAX_LINES,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolName {
    ReadFile,
    ListDirectory,
    Glob,
    GrepSearch,
}

impl ToolName {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ReadFile => "read_file",
            Self::ListDirectory => "list_directory",
            Self::Glob => "glob",
            Self::GrepSearch => "grep_search",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Entry {
    pub path: String,
    pub is_dir: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GrepMatch {
    pub path: String,
    pub lineno: usize,
    pub line: String,
    pub before: Vec<String>,
    pub after: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReadPayload {
    Text(String),
    Entries(Vec<Entry>),
    Paths(Vec<String>),
    Matches(Vec<GrepMatch>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadResult {
    pub tool: ToolName,
    pub ok: bool,
    pub payload: ReadPayload,
    pub refusal: Option<&'static str>,
    pub truncated: bool,
    pub notice: Option<&'static str>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadFileOptions {
    pub start_line: i64,
    pub max_lines: i64,
    pub max_bytes: i64,
}
impl Default for ReadFileOptions {
    fn default() -> Self {
        Self {
            start_line: 1,
            max_lines: READ_FILE_MAX_LINES,
            max_bytes: READ_FILE_MAX_BYTES,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ListDirectoryOptions {
    pub recursive: bool,
    pub max_entries: i64,
    pub include_hidden: bool,
    pub pattern: Option<String>,
}
impl Default for ListDirectoryOptions {
    fn default() -> Self {
        Self {
            recursive: false,
            max_entries: LIST_DIRECTORY_MAX_ENTRIES,
            include_hidden: false,
            pattern: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GlobOptions {
    pub max_matches: i64,
    pub include_hidden: bool,
}
impl Default for GlobOptions {
    fn default() -> Self {
        Self {
            max_matches: GLOB_MAX_MATCHES,
            include_hidden: false,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GrepSearchOptions {
    pub regex: bool,
    pub case_sensitive: bool,
    pub file_glob: Option<String>,
    pub context_lines: i64,
    pub max_matches: i64,
    pub max_files: i64,
    pub max_bytes_per_file: i64,
    pub include_hidden: bool,
}
impl Default for GrepSearchOptions {
    fn default() -> Self {
        Self {
            regex: false,
            case_sensitive: false,
            file_glob: None,
            context_lines: 0,
            max_matches: GREP_MAX_MATCHES,
            max_files: GREP_MAX_FILES,
            max_bytes_per_file: GREP_MAX_BYTES_PER_FILE,
            include_hidden: false,
        }
    }
}
