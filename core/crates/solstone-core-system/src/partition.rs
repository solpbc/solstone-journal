// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::borrow::Cow;
use std::path::Path;

/// Canonical queue and log partition for a command.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Partition(String);

impl Partition {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for Partition {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

/// The `journal <args>` form of a command spelled `solstone journal <args>`.
///
/// `solstone journal` is the canonical name of the journal command family and
/// `journal` is its alias, so both spellings name one operation. Classification,
/// resource caps and launch binding all read a command through this, which
/// keeps a prefixed command's policy identical to its alias form. Any other
/// command is returned unchanged.
pub fn journal_alias_form(cmd: &[String]) -> Cow<'_, [String]> {
    match cmd {
        [solstone, journal, operation @ ..] if solstone == "solstone" && journal == "journal" => {
            Cow::Owned(
                std::iter::once(journal.clone())
                    .chain(operation.iter().cloned())
                    .collect(),
            )
        }
        _ => Cow::Borrowed(cmd),
    }
}

pub fn canonical_journal_command<I, S>(tail: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut command = vec!["solstone".to_owned(), "journal".to_owned()];
    command.extend(tail.into_iter().map(Into::into));
    command
}

/// Mirror the Python supervisor's ordered command partition resolver.
pub fn partition_for(cmd: &[String]) -> Partition {
    alias_partition_for(&journal_alias_form(cmd))
}

fn alias_partition_for(cmd: &[String]) -> Partition {
    let Some(first) = cmd.first() else {
        return Partition::new("unknown");
    };

    let recognized = matches!(first.as_str(), "solstone" | "journal")
        || Path::new(first).file_name().and_then(|name| name.to_str())
            == Some("solstone-core-journal");
    if recognized && cmd.len() > 1 {
        let mut name = cmd[1].clone();
        if name == "think" {
            // Order is a contract: the first matching mode wins.
            for (flag, mode) in [
                ("--activity", "activity"),
                ("--flush", "flush"),
                ("--segments", "segment"),
                ("--weekly", "weekly"),
                ("--cadence", "cadence"),
                ("--segment", "segment"),
            ] {
                if cmd.iter().any(|arg| arg == flag) {
                    name = mode.to_owned();
                    break;
                }
            }
            if name == "think" {
                name = "daily".to_owned();
            }
        } else if name == "maintenance" && cmd.len() >= 4 && cmd[2] == "run" {
            name = format!("maintenance:{}", cmd[3]);
        }
        return Partition::new(name);
    }

    Partition::new(
        Path::new(first)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(first)
            .to_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::{Partition, journal_alias_form, partition_for};

    #[test]
    fn resolves_service_partition_for_sibling_journal_binary() {
        assert_eq!(
            partition_for(&[
                "/foo/bar/solstone-core-journal".to_owned(),
                "convey".to_owned(),
                "--port".to_owned(),
                "5015".to_owned(),
            ]),
            Partition::new("convey")
        );
    }

    #[test]
    fn retains_exact_matching_for_bare_journal_and_solstone() {
        assert_eq!(
            partition_for(&["journal".to_owned(), "think".to_owned()]),
            Partition::new("daily")
        );
        assert_eq!(
            partition_for(&["solstone".to_owned(), "heartbeat".to_owned()]),
            Partition::new("heartbeat")
        );
    }

    #[test]
    fn a_solstone_journal_command_shares_its_alias_forms_partition() {
        for alias in [
            &["journal", "think"][..],
            &["journal", "think", "--segment", "120000_300"],
            &["journal", "-v", "think", "--weekly"],
            &["journal", "maintenance", "run", "backup:run"],
            &["journal", "indexer", "--rescan-full"],
            &["journal", "heartbeat"],
            &["journal"],
        ] {
            let alias = alias
                .iter()
                .map(|arg| (*arg).to_owned())
                .collect::<Vec<_>>();
            let canonical = std::iter::once("solstone".to_owned())
                .chain(alias.iter().cloned())
                .collect::<Vec<_>>();
            assert_eq!(
                partition_for(&canonical),
                partition_for(&alias),
                "{alias:?}"
            );
        }
        assert_eq!(
            partition_for(&[
                "solstone".to_owned(),
                "journal".to_owned(),
                "maintenance".to_owned(),
                "run".to_owned(),
                "backup:run".to_owned(),
            ]),
            Partition::new("maintenance:backup:run")
        );
    }

    #[test]
    fn only_the_solstone_journal_prefix_takes_the_alias_form() {
        let args = |values: &[&str]| values.iter().map(|v| (*v).to_owned()).collect::<Vec<_>>();
        assert_eq!(
            journal_alias_form(&args(&["solstone", "journal", "think", "-v"])).as_ref(),
            args(&["journal", "think", "-v"])
        );
        for unchanged in [
            args(&["journal", "think"]),
            args(&["solstone", "heartbeat"]),
            args(&["solstone", "call", "journal", "read"]),
            args(&["/usr/local/bin/solstone", "journal", "think"]),
            args(&["svc", "solstone", "journal"]),
        ] {
            assert_eq!(journal_alias_form(&unchanged).as_ref(), unchanged);
        }
    }

    #[test]
    fn does_not_treat_an_unrelated_journal_path_as_a_service_command() {
        assert_eq!(
            partition_for(&["/opt/tools/journal".to_owned(), "backup".to_owned()]),
            Partition::new("journal")
        );
    }
}
