// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::ffi::{OsStr, OsString};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObserverCommand {
    List {
        json: bool,
    },
    Status {
        identifier: Option<String>,
        json: bool,
    },
    Rename {
        old: String,
        new: String,
        json: bool,
    },
    Revoke {
        identifier: String,
        json: bool,
    },
    Reconcile {
        dry_run: bool,
        json: bool,
    },
    Create,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObserverUsageError;

/// Manual parser deliberately accepts --json on either side of the verb.  The
/// native reconcile default is dry-run-safe; --commit is the write opt-in.
pub fn parse_observer_args(args: &[OsString]) -> Result<ObserverCommand, ObserverUsageError> {
    let mut json = false;
    let mut dry_run = false;
    let mut commit = false;
    let mut words = Vec::new();
    for argument in args {
        if argument == OsStr::new("--json") {
            if json {
                return Err(ObserverUsageError);
            }
            json = true;
        } else if argument == OsStr::new("--dry-run") {
            if dry_run {
                return Err(ObserverUsageError);
            }
            dry_run = true;
        } else if argument == OsStr::new("--commit") {
            if commit {
                return Err(ObserverUsageError);
            }
            commit = true;
        } else if argument.to_string_lossy().starts_with('-') {
            return Err(ObserverUsageError);
        } else {
            words.push(argument.to_str().ok_or(ObserverUsageError)?.to_owned());
        }
    }
    if dry_run && commit {
        return Err(ObserverUsageError);
    }
    let Some((verb, positional)) = words.split_first() else {
        return Err(ObserverUsageError);
    };
    match verb.as_str() {
        "list" if positional.is_empty() && !dry_run && !commit => {
            Ok(ObserverCommand::List { json })
        }
        "status" if positional.len() <= 1 && !dry_run && !commit => Ok(ObserverCommand::Status {
            identifier: positional.first().cloned(),
            json,
        }),
        "rename" if positional.len() == 2 && !dry_run && !commit => Ok(ObserverCommand::Rename {
            old: positional[0].clone(),
            new: positional[1].clone(),
            json,
        }),
        "revoke" if positional.len() == 1 && !dry_run && !commit => Ok(ObserverCommand::Revoke {
            identifier: positional[0].clone(),
            json,
        }),
        "reconcile" if positional.is_empty() => Ok(ObserverCommand::Reconcile {
            dry_run: !commit,
            json,
        }),
        "create" if positional.is_empty() && !dry_run && !commit => Ok(ObserverCommand::Create),
        _ => Err(ObserverUsageError),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(words: &[&str]) -> Vec<OsString> {
        words.iter().map(OsString::from).collect()
    }
    #[test]
    fn accepts_json_on_both_sides_of_verb() {
        assert_eq!(
            parse_observer_args(&args(&["--json", "list"])),
            Ok(ObserverCommand::List { json: true })
        );
        assert_eq!(
            parse_observer_args(&args(&["list", "--json"])),
            Ok(ObserverCommand::List { json: true })
        );
    }
    #[test]
    fn reconcile_is_dry_run_unless_commit_is_explicit() {
        assert_eq!(
            parse_observer_args(&args(&["reconcile"])),
            Ok(ObserverCommand::Reconcile {
                dry_run: true,
                json: false
            })
        );
        assert_eq!(
            parse_observer_args(&args(&["reconcile", "--commit"])),
            Ok(ObserverCommand::Reconcile {
                dry_run: false,
                json: false
            })
        );
    }
}
