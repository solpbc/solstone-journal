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
    Prune {
        day: Option<String>,
        day_range: Option<(String, String)>,
        all: bool,
        stream: Option<String>,
        execute: bool,
        cross_start: bool,
    },
    Create,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObserverUsageError;

/// Manual parser deliberately accepts --json on either side of the verb.  The
/// native reconcile default is dry-run-safe; --commit is the write opt-in.
pub fn parse_observer_args(args: &[OsString]) -> Result<ObserverCommand, ObserverUsageError> {
    if let Some((first, rest)) = args.split_first()
        && first == OsStr::new("prune")
    {
        return parse_prune_args(rest);
    }
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

/// `prune`'s flag set is disjoint from every other verb's (no `--json`, and
/// two of its flags take a value), so it is parsed on its own rather than
/// forced through the shared boolean-flag loop above.
fn parse_prune_args(args: &[OsString]) -> Result<ObserverCommand, ObserverUsageError> {
    let mut day: Option<String> = None;
    let mut day_range_raw: Option<String> = None;
    let mut all = false;
    let mut stream: Option<String> = None;
    let mut execute = false;
    let mut cross_start = false;
    let mut index = 0;
    while index < args.len() {
        let argument = &args[index];
        if argument == OsStr::new("--day") {
            index += 1;
            let value = args.get(index).ok_or(ObserverUsageError)?;
            if day.is_some() {
                return Err(ObserverUsageError);
            }
            day = Some(value.to_str().ok_or(ObserverUsageError)?.to_owned());
        } else if argument == OsStr::new("--day-range") {
            index += 1;
            let value = args.get(index).ok_or(ObserverUsageError)?;
            if day_range_raw.is_some() {
                return Err(ObserverUsageError);
            }
            day_range_raw = Some(value.to_str().ok_or(ObserverUsageError)?.to_owned());
        } else if argument == OsStr::new("--all") {
            if all {
                return Err(ObserverUsageError);
            }
            all = true;
        } else if argument == OsStr::new("--stream") {
            index += 1;
            let value = args.get(index).ok_or(ObserverUsageError)?;
            if stream.is_some() {
                return Err(ObserverUsageError);
            }
            stream = Some(value.to_str().ok_or(ObserverUsageError)?.to_owned());
        } else if argument == OsStr::new("--execute") {
            if execute {
                return Err(ObserverUsageError);
            }
            execute = true;
        } else if argument == OsStr::new("--cross-start") {
            if cross_start {
                return Err(ObserverUsageError);
            }
            cross_start = true;
        } else {
            return Err(ObserverUsageError);
        }
        index += 1;
    }
    let selector_count = [day.is_some(), day_range_raw.is_some(), all]
        .into_iter()
        .filter(|selected| *selected)
        .count();
    if selector_count != 1 {
        return Err(ObserverUsageError);
    }
    let day_range = day_range_raw
        .map(|raw| {
            raw.split_once("..")
                .map(|(start, end)| (start.to_owned(), end.to_owned()))
                .ok_or(ObserverUsageError)
        })
        .transpose()?;
    Ok(ObserverCommand::Prune {
        day,
        day_range,
        all,
        stream,
        execute,
        cross_start,
    })
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
