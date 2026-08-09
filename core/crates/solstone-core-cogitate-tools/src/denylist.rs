// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use crate::patterns::fnmatch;
use crate::refusals::{REFUSAL_CREDENTIAL_FILE, REFUSAL_DENIED_COMPONENT};

pub const READ_FILE_MAX_LINES: i64 = 2000;
pub const READ_FILE_MAX_BYTES: i64 = 65536;
pub const LIST_DIRECTORY_MAX_ENTRIES: i64 = 200;
pub const GLOB_MAX_MATCHES: i64 = 200;
pub const GREP_MAX_MATCHES: i64 = 100;
pub const GREP_MAX_FILES: i64 = 1000;
pub const GREP_MAX_BYTES_PER_FILE: i64 = 20480;
pub const DEFAULT_READ_CALL_BUDGET: i64 = 200;

pub const DENIED_PATH_COMPONENTS: [&str; 14] = [
    ".git",
    ".cache",
    "__pycache__",
    ".mypy_cache",
    ".pytest_cache",
    ".ruff_cache",
    "node_modules",
    ".venv",
    "venv",
    ".tox",
    "site-packages",
    ".ssh",
    ".gnupg",
    ".aws",
];
pub const DENIED_CREDENTIAL_PATTERNS: [&str; 7] = [
    "id_rsa*",
    "*.pem",
    "*.key",
    ".env",
    "*.env",
    "credentials",
    "*.credentials",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Classification {
    Allowed,
    DeniedComponent,
    DeniedCredential,
}

pub(crate) fn classify(resolved: &Path, root: &Path) -> Classification {
    if resolved == root {
        return Classification::Allowed;
    }
    let Ok(relative) = resolved.strip_prefix(root) else {
        return Classification::DeniedComponent;
    };
    if relative.components().any(|part| {
        part.as_os_str()
            .to_str()
            .is_some_and(|name| DENIED_PATH_COMPONENTS.contains(&name))
    }) {
        return Classification::DeniedComponent;
    }
    let name = resolved
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if DENIED_CREDENTIAL_PATTERNS
        .iter()
        .any(|pattern| fnmatch(name, pattern))
    {
        Classification::DeniedCredential
    } else {
        Classification::Allowed
    }
}

pub(crate) fn refusal_for(classification: Classification) -> Option<&'static str> {
    match classification {
        Classification::Allowed => None,
        Classification::DeniedComponent => Some(REFUSAL_DENIED_COMPONENT),
        Classification::DeniedCredential => Some(REFUSAL_CREDENTIAL_FILE),
    }
}

pub(crate) fn broad_recursive_refusal(resolved: &Path, root: &Path) -> bool {
    match resolved.strip_prefix(root) {
        Ok(relative) => {
            relative.as_os_str().is_empty()
                || relative == Path::new("chronicle")
                || relative == Path::new("facets")
        }
        Err(_) => false,
    }
}
