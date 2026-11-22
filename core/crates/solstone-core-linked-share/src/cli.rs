// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! `solstone journal share`: read-only matching and packaging, JSON out.

use std::ffi::OsString;
use std::path::Path;

use serde_json::json;
use solstone_core_cli_copy::{Diagnostic, ENGLISH, MessageDetail, render_for};

use crate::matching::{ActivityRef, match_activity};
use crate::names::PersonRef;
use crate::people::{Owner, activity_read_detail, facet_list_detail, resolve_person_detail};
use crate::summary::build_package;
use crate::talents::{PartialTrackingTalents, TalentCaller};

// These public constants are the source-English usage strings for library callers.
pub const SHARE_USAGE: &str = "usage: solstone journal share [-h] {match,package} ...\n";
pub const MATCH_USAGE: &str =
    "usage: solstone journal share match [-h] --person PERSON --day YYYYMMDD [--facet FACET]\n";
pub const PACKAGE_USAGE: &str = "usage: solstone journal share package [-h] --person PERSON --day YYYYMMDD --facet FACET --activity ID\n";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CommandPage {
    Root,
    Match,
    Package,
}

impl CommandPage {
    fn help(self, locale: &str) -> String {
        let id = match self {
            Self::Root => "journal.cli.share.help.root",
            Self::Match => "journal.cli.share.help.match",
            Self::Package => "journal.cli.share.help.package",
        };
        render_for(locale, id, &[])
    }

    fn usage(self, locale: &str) -> String {
        let id = match self {
            Self::Root => "journal.cli.share.usage.root",
            Self::Match => "journal.cli.share.usage.match",
            Self::Package => "journal.cli.share.usage.package",
        };
        render_for(locale, id, &[])
    }

    fn refuse<const N: usize>(
        self,
        id: &'static str,
        args: [(&'static str, &dyn std::fmt::Display); N],
    ) -> ShareCommand {
        ShareCommand::Usage {
            page: self,
            error: Diagnostic::new(id, args),
        }
    }

    fn required(self, option: &str) -> ShareCommand {
        self.refuse("journal.cli.share.parse.required", [("option", &option)])
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CliRun {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ShareCommand {
    Help(CommandPage),
    Usage {
        page: CommandPage,
        error: Diagnostic,
    },
    Match {
        person: String,
        day: String,
        facet: Option<String>,
    },
    Package {
        person: String,
        day: String,
        facet: String,
        activity: String,
    },
}

/// Whether the arguments name a subcommand that reads the journal. Help and
/// usage errors read nothing and need no journal or model.
pub fn reads_journal(args: &[OsString]) -> bool {
    matches!(
        parse(args),
        ShareCommand::Match { .. } | ShareCommand::Package { .. }
    )
}

pub(crate) fn parse(args: &[OsString]) -> ShareCommand {
    let Some(args) = args
        .iter()
        .map(|arg| arg.to_str().map(str::to_owned))
        .collect::<Option<Vec<_>>>()
    else {
        return CommandPage::Root.refuse("journal.cli.share.parse.encoding", []);
    };
    let Some((command, rest)) = args.split_first() else {
        return CommandPage::Root.refuse("journal.cli.share.parse.missing", []);
    };
    let page = match command.as_str() {
        "-h" | "--help" => return ShareCommand::Help(CommandPage::Root),
        "match" => CommandPage::Match,
        "package" => CommandPage::Package,
        other => {
            return CommandPage::Root
                .refuse("journal.cli.share.parse.unknown", [("argument", &other)]);
        }
    };
    let mut person = None;
    let mut day = None;
    let mut facet = None;
    let mut activity = None;
    let mut values = rest.iter();
    while let Some(flag) = values.next() {
        let slot = match flag.as_str() {
            "-h" | "--help" => return ShareCommand::Help(page),
            "--person" => &mut person,
            "--day" => &mut day,
            "--facet" => &mut facet,
            "--activity" if command == "package" => &mut activity,
            other => {
                return page.refuse("journal.cli.share.parse.argument", [("argument", &other)]);
            }
        };
        match values.next() {
            Some(value) if !value.is_empty() && slot.is_none() => *slot = Some(value.clone()),
            Some(_) if slot.is_some() => {
                return page.refuse("journal.cli.share.parse.duplicate", [("flag", flag)]);
            }
            _ => return page.refuse("journal.cli.share.parse.value", [("flag", flag)]),
        }
    }
    let Some(person) = person else {
        return page.required("--person");
    };
    let Some(day) = day.filter(|day| solstone_core_format::segment::is_date_key(day)) else {
        return page.required("--day YYYYMMDD");
    };
    if command == "match" {
        return ShareCommand::Match { person, day, facet };
    }
    let Some(facet) = facet else {
        return page.required("--facet");
    };
    let Some(activity) = activity else {
        return page.required("--activity");
    };
    ShareCommand::Package {
        person,
        day,
        facet,
        activity,
    }
}

/// Run a read-only share preview using the source language for API callers.
pub fn run_cli(args: &[OsString], journal: &Path, talents: &dyn TalentCaller) -> CliRun {
    run_cli_for_locale(ENGLISH, args, journal, talents)
}

/// Run the same preview with terminal diagnostics in the caller's language.
pub fn run_cli_for_locale(
    locale: &str,
    args: &[OsString],
    journal: &Path,
    talents: &dyn TalentCaller,
) -> CliRun {
    let tracked = PartialTrackingTalents::new(talents, locale);
    let mut run = run_cli_inner(locale, args, journal, &tracked);
    if tracked.saw_partial() {
        run.stderr
            .push_str(&solstone_core_format::stored_markdown::Message::PartialInput.render(locale));
        run.stderr.push('\n');
    }
    run
}

fn run_cli_inner(
    locale: &str,
    args: &[OsString],
    journal: &Path,
    talents: &dyn TalentCaller,
) -> CliRun {
    match parse(args) {
        ShareCommand::Help(page) => finished(format!("{}\n", page.help(locale)), String::new(), 0),
        ShareCommand::Usage { page, error } => finished(
            String::new(),
            format!("{}\n{}\n", page.usage(locale), error.render(locale)),
            2,
        ),
        ShareCommand::Match { person, day, facet } => {
            match run_match(journal, &person, &day, facet.as_deref(), talents) {
                Ok(stdout) => finished(stdout, String::new(), 0),
                Err(error) => failed(locale, error),
            }
        }
        ShareCommand::Package {
            person,
            day,
            facet,
            activity,
        } => match run_package(journal, &person, &day, &facet, &activity, talents) {
            Ok(stdout) => finished(stdout, String::new(), 0),
            Err(error) => failed(locale, error),
        },
    }
}

fn finished(stdout: String, stderr: String, exit_code: i32) -> CliRun {
    CliRun {
        stdout,
        stderr,
        exit_code,
    }
}

fn failed(locale: &str, error: MessageDetail) -> CliRun {
    finished(
        String::new(),
        format!(
            "{}\n",
            error.context("journal.cli.share.failure").render(locale)
        ),
        1,
    )
}

fn subjects(
    journal: &Path,
    person: &str,
    locale: &str,
) -> Result<(PersonRef, Owner), MessageDetail> {
    Ok((
        resolve_person_detail(journal, person, locale)?,
        Owner::load_detail(journal, locale)?,
    ))
}

fn run_match(
    journal: &Path,
    person: &str,
    day: &str,
    facet: Option<&str>,
    talents: &dyn TalentCaller,
) -> Result<String, MessageDetail> {
    let (person, owner) = subjects(journal, person, talents.diagnostic_locale())?;
    let facets = match facet {
        Some(facet) => vec![facet.to_owned()],
        None => solstone_core_facets::list_declared_facet_names(journal)
            .map_err(|error| facet_list_detail(&error).context("journal.cli.share.facets"))?,
    };
    let mut stdout = String::new();
    for facet in facets {
        let records = solstone_core_facets::load_activity_records(journal, &facet, day, false)
            .map_err(|error| {
                activity_read_detail(&error).context_with(
                    "journal.cli.share.activities",
                    [("facet", &facet), ("day", &day)],
                )
            })?;
        for record in records {
            let Some(matched) =
                match_activity(journal, &facet, day, &record, &person, &owner, talents)
            else {
                continue;
            };
            let activity = ActivityRef::from_record(&record);
            let mut line = json!({
                "day": day,
                "facet": facet,
                "activity_id": activity.id,
                "activity_type": activity.activity_type,
                "basis": matched.basis,
                "shape": matched.shape,
                "segments": matched.segments,
                "threads": matched.threads,
            });
            if let Some(partial) = matched.partial_input {
                line["partial_input"] = partial;
            }
            stdout.push_str(&line.to_string());
            stdout.push('\n');
        }
    }
    Ok(stdout)
}

fn run_package(
    journal: &Path,
    person: &str,
    day: &str,
    facet: &str,
    activity: &str,
    talents: &dyn TalentCaller,
) -> Result<String, MessageDetail> {
    let (person, owner) = subjects(journal, person, talents.diagnostic_locale())?;
    let record = solstone_core_facets::get_activity_record(journal, facet, day, activity)
        .map_err(|error| {
            activity_read_detail(&error).context_with(
                "journal.cli.share.activities",
                [("facet", &facet), ("day", &day)],
            )
        })?
        .filter(|record| {
            !record
                .get("hidden")
                .is_some_and(solstone_core_facets::activity_value_truthy)
        })
        .ok_or_else(|| {
            MessageDetail::message(
                "journal.cli.share.activity.missing",
                [("activity", &activity), ("facet", &facet), ("day", &day)],
            )
        })?;
    let package = build_package(journal, facet, day, &record, &person, &owner, talents)
        .ok_or_else(|| {
            MessageDetail::message(
                "journal.cli.share.activity.unmatched",
                [("activity", &activity), ("person", &person.name)],
            )
        })?;
    let mut stdout = serde_json::to_string(&package).map_err(|error| error.to_string())?;
    stdout.push('\n');
    Ok(stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn match_and_package_arguments_parse() {
        assert_eq!(
            parse(&args(&[
                "match", "--person", "jane_doe", "--day", "20261013"
            ])),
            ShareCommand::Match {
                person: "jane_doe".into(),
                day: "20261013".into(),
                facet: None
            }
        );
        assert_eq!(
            parse(&args(&[
                "package",
                "--day",
                "20261013",
                "--facet",
                "work",
                "--activity",
                "a1",
                "--person",
                "Jane Doe"
            ])),
            ShareCommand::Package {
                person: "Jane Doe".into(),
                day: "20261013".into(),
                facet: "work".into(),
                activity: "a1".into()
            }
        );
    }

    #[test]
    fn help_and_usage_errors_need_no_journal() {
        assert!(!reads_journal(&args(&["--help"])));
        assert!(!reads_journal(&args(&["match", "-h"])));
        assert!(!reads_journal(&args(&[
            "package", "--person", "x", "--help"
        ])));
        assert!(!reads_journal(&args(&["--nonsense"])));
        assert!(!reads_journal(&args(&["match"])));
        assert!(reads_journal(&args(&[
            "match", "--person", "x", "--day", "20261013"
        ])));
    }

    #[test]
    fn malformed_arguments_are_usage_errors() {
        let refused = |values: &[&str]| matches!(parse(&args(values)), ShareCommand::Usage { .. });
        assert!(refused(&[]));
        assert!(refused(&["send"]));
        assert!(refused(&["match", "--day", "20261013"]));
        assert!(refused(&["match", "--person", "x", "--day", "2026-10-13"]));
        assert!(refused(&[
            "match",
            "--person",
            "x",
            "--day",
            "20261013",
            "--activity",
            "a"
        ]));
        assert!(refused(&[
            "match", "--person", "x", "--person", "y", "--day", "20261013"
        ]));
        assert!(refused(&["match", "--person"]));
        assert!(refused(&[
            "package", "--person", "x", "--day", "20261013", "--facet", "w"
        ]));
        let run = run_cli(&args(&["match"]), Path::new(""), &|_: &str, _: &str| {
            Err("unused".to_owned())
        });
        assert_eq!(run.exit_code, 2);
        assert!(run.stderr.starts_with(MATCH_USAGE), "{}", run.stderr);
    }
}
