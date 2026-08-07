// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::processes::{process_spec_for, process_tokens};

pub use solstone_core_sol::{JOURNAL_HOST_COMMAND_COUNT, JOURNAL_HOST_COMMANDS};

pub const ROOT_COMMANDS: &[&str] = &["--path", "path", "status", "root", "notify"];
pub(crate) struct UnavailableLocal {
    pub(crate) group: &'static str,
    pub(crate) leaf: &'static str,
    pub(crate) token: &'static str,
}

pub(crate) const UNAVAILABLE_LOCAL_PATHS: &[UnavailableLocal] = &[
    UnavailableLocal {
        group: "archive",
        leaf: "export",
        token: "archive export",
    },
    UnavailableLocal {
        group: "archive",
        leaf: "merge",
        token: "archive merge",
    },
    UnavailableLocal {
        group: "facet",
        leaf: "doctor",
        token: "facet doctor",
    },
    UnavailableLocal {
        group: "facet",
        leaf: "merge",
        token: "facet merge",
    },
    UnavailableLocal {
        group: "news",
        leaf: "write",
        token: "news write",
    },
];
pub const JOURNAL_COMMAND_COUNT: usize =
    ROOT_COMMANDS.len() + crate::processes::PROCESS_SPECS.len() + UNAVAILABLE_LOCAL_PATHS.len();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Primitive {
    Path,
    Status,
    Root,
    Notify,
}

pub(crate) fn primitive_for(token: &str) -> Option<Primitive> {
    match token {
        "--path" | "path" => Some(Primitive::Path),
        "status" => Some(Primitive::Status),
        "root" => Some(Primitive::Root),
        "notify" => Some(Primitive::Notify),
        _ => None,
    }
}

pub(crate) fn known_token(value: &str) -> Option<&'static str> {
    ROOT_COMMANDS
        .iter()
        .copied()
        .find(|token| *token == value)
        .or_else(|| process_spec_for(value).map(|spec| spec.token))
}

pub(crate) fn unavailable_local_for(group: &str, leaf: &str) -> Option<&'static str> {
    UNAVAILABLE_LOCAL_PATHS
        .iter()
        .find(|candidate| candidate.group == group && candidate.leaf == leaf)
        .map(|candidate| candidate.token)
}

pub(crate) fn process_command_tokens() -> impl Iterator<Item = &'static str> {
    process_tokens()
}

#[cfg(test)]
pub(crate) fn all_leaf_paths() -> Vec<Vec<&'static str>> {
    let mut paths = ROOT_COMMANDS
        .iter()
        .map(|token| vec![*token])
        .collect::<Vec<_>>();
    paths.extend(process_tokens().map(|token| vec![token]));
    paths.extend(
        UNAVAILABLE_LOCAL_PATHS
            .iter()
            .map(|path| vec![path.group, path.leaf]),
    );
    paths
}
