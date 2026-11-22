// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::ffi::OsString;

#[cfg(test)]
pub const HELP: &str = concat!(
    "usage: solstone journal talent [-h] [-v] [-d] {list,show,passages,logs,log} ...\n\n",
    "Inspect talent prompt configurations\n\n",
    "positional arguments:\n",
    "  {list,show,passages,logs,log}\n",
    "    list                List prompts grouped by schedule\n",
    "    show                Show details for a specific prompt\n",
    "    passages            Copy source passages from an activity's source\n",
    "    logs                Show recent talent run log\n",
    "    log                 Show events for an agent run\n\n",
    "options:\n",
    "  -h, --help            show this help message and exit\n",
    "  -v, --verbose         Enable verbose output\n",
    "  -d, --debug           Enable debug logging\n",
);

#[cfg(test)]
pub const LOG_HELP: &str = concat!(
    "usage: solstone journal talent log [-h] [--json] [--full] id\n\n",
    "positional arguments:\n",
    "  id          Agent ID\n\n",
    "options:\n",
    "  -h, --help  show this help message and exit\n",
    "  --json      Output raw JSONL\n",
    "  --full      Expand event details\n",
);

#[cfg(test)]
pub const LOGS_HELP: &str = concat!(
    "usage: solstone journal talent logs [-h] [-c COUNT] [--day YYYYMMDD] [--daily] [--errors]\n",
    "                           [--summary]\n",
    "                           [agent]\n\n",
    "positional arguments:\n",
    "  agent                 Filter to a specific agent\n\n",
    "options:\n",
    "  -h, --help            show this help message and exit\n",
    "  -c COUNT, --count COUNT\n",
    "                        Number of runs to show (default: 20)\n",
    "  --day YYYYMMDD        Show only runs from this day\n",
    "  --daily               Show only daily-scheduled runs\n",
    "  --errors              Show only error runs\n",
    "  --summary             Show grouped summary\n",
);

#[cfg(test)]
pub const SHOW_HELP: &str = concat!(
    "usage: solstone journal talent show [-h] [--json] [--prompt] [--day YYYYMMDD]\n",
    "                           [--segment HHMMSS_LEN] [--facet NAME]\n",
    "                           [--activity ID] [--query TEXT] [--full]\n",
    "                           name\n\n",
    "positional arguments:\n",
    "  name                  Prompt name\n\n",
    "options:\n",
    "  -h, --help            show this help message and exit\n",
    "  --json                Output as JSONL\n",
    "  --prompt              Show full prompt context (dry-run mode)\n",
    "  --day YYYYMMDD        Day for prompt context\n",
    "  --segment HHMMSS_LEN  Segment for segment-scheduled prompts\n",
    "  --facet NAME          Facet for multi-facet prompts\n",
    "  --activity ID         Activity ID for activity-scheduled prompts\n",
    "  --query TEXT          Sample query for tool agents\n",
    "  --full                Show full content without truncation\n",
);

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct ListOptions {
    pub(crate) schedule: Option<String>,
    pub(crate) source: Option<String>,
    pub(crate) disabled: bool,
    pub(crate) json: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct LogOptions {
    pub(crate) id: String,
    pub(crate) json: bool,
    pub(crate) full: bool,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct LogsOptions {
    pub(crate) agent: Option<String>,
    pub(crate) count: Option<i64>,
    pub(crate) day: Option<String>,
    pub(crate) daily: bool,
    pub(crate) errors: bool,
    pub(crate) summary: bool,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct ShowOptions {
    pub(crate) name: String,
    pub(crate) json: bool,
    pub(crate) prompt: bool,
    pub(crate) day: Option<String>,
    pub(crate) segment: Option<String>,
    pub(crate) facet: Option<String>,
    pub(crate) activity: Option<String>,
    pub(crate) query: Option<String>,
    pub(crate) full: bool,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct PassagesOptions {
    pub(crate) name: String,
    pub(crate) json: bool,
    pub(crate) day: Option<String>,
    pub(crate) facet: Option<String>,
    pub(crate) activity: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Command {
    Help(String),
    List(ListOptions),
    Log(LogOptions),
    Logs(LogsOptions),
    Show(ShowOptions),
    Passages(PassagesOptions),
    Error(String),
}

pub(crate) fn parse(args: &[OsString]) -> Command {
    let locale = crate::output_locale(args.iter().any(|arg| arg == "--json"));
    let args = match args
        .iter()
        .map(|arg| arg.to_str())
        .collect::<Option<Vec<_>>>()
    {
        Some(args) => args,
        None => {
            return Command::Error(format!(
                "{}\n",
                solstone_core_cli_copy::render_for(
                    locale,
                    "journal.cli.talent.error.root.encoding",
                    &[]
                )
            ));
        }
    };
    let mut index = 0;
    while index < args.len() && matches!(args[index], "-v" | "--verbose" | "-d" | "--debug") {
        index += 1;
    }
    if index == args.len() {
        return Command::List(ListOptions::default());
    }
    match args[index] {
        "-h" | "--help" => {
            if index + 1 == args.len() {
                Command::Help(format!(
                    "{}\n",
                    solstone_core_cli_copy::render_for(locale, "journal.cli.talent.help.root", &[])
                ))
            } else {
                Command::Error(format!(
                    "{}\n",
                    solstone_core_cli_copy::display_for(
                        locale,
                        "journal.cli.talent.error.root.unknown",
                        [("arguments", &(args[index + 1..].join(" ")))]
                    )
                ))
            }
        }
        "list" => parse_list(&args[index + 1..], locale),
        "log" => parse_log(&args[index + 1..], locale),
        "logs" => parse_logs(&args[index + 1..], locale),
        "show" => parse_show(&args[index + 1..], locale),
        "passages" => parse_passages(&args[index + 1..], locale),
        value if value.starts_with('-') => Command::Error(format!(
            "{}\n",
            solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.talent.error.root.unknown",
                [("arguments", &(value))]
            )
        )),
        value => Command::Error(format!(
            "{}\n",
            solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.talent.error.root.choice",
                [("value", &(value))]
            )
        )),
    }
}

fn parse_show(args: &[&str], locale: &str) -> Command {
    let mut options = ShowOptions::default();
    let mut unrecognized = Vec::new();
    let mut index = 0;
    while index < args.len() {
        match args[index] {
            "-h" | "--help" => {
                return Command::Help(format!(
                    "{}\n",
                    solstone_core_cli_copy::render_for(locale, "journal.cli.talent.help.show", &[])
                ));
            }
            "--json" => options.json = true,
            "--prompt" => options.prompt = true,
            "--full" => options.full = true,
            "--day" | "--segment" | "--facet" | "--activity" | "--query" => {
                let flag = args[index];
                index += 1;
                let Some(value) = args.get(index) else {
                    return Command::Error(format!(
                        "{}\n",
                        solstone_core_cli_copy::display_for(
                            locale,
                            "journal.cli.talent.error.show.value",
                            [("flag", &(flag))]
                        )
                    ));
                };
                match flag {
                    "--day" => options.day = Some((*value).to_owned()),
                    "--segment" => options.segment = Some((*value).to_owned()),
                    "--facet" => options.facet = Some((*value).to_owned()),
                    "--activity" => options.activity = Some((*value).to_owned()),
                    "--query" => options.query = Some((*value).to_owned()),
                    _ => unreachable!("matched show value flag"),
                }
            }
            value if value.starts_with("--day=") => options.day = Some(value[6..].to_owned()),
            value if value.starts_with("--segment=") => {
                options.segment = Some(value[10..].to_owned())
            }
            value if value.starts_with("--facet=") => options.facet = Some(value[8..].to_owned()),
            value if value.starts_with("--activity=") => {
                options.activity = Some(value[11..].to_owned())
            }
            value if value.starts_with("--query=") => options.query = Some(value[8..].to_owned()),
            value if value.starts_with('-') => unrecognized.push(value),
            value if options.name.is_empty() => options.name = value.to_owned(),
            value => unrecognized.push(value),
        }
        index += 1;
    }
    if !unrecognized.is_empty() {
        return Command::Error(format!(
            "{}\n",
            solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.talent.error.show.unknown",
                [("arguments", &(unrecognized.join(" ")))]
            )
        ));
    }
    if options.name.is_empty() {
        return Command::Error(format!(
            "{}\n",
            solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.talent.error.show.required",
                [("arguments", &("name"))]
            )
        ));
    }
    Command::Show(options)
}

fn parse_passages(args: &[&str], locale: &str) -> Command {
    let mut options = PassagesOptions::default();
    let mut unrecognized = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let (flag, inline) = match args[index].split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => (flag, Some(value)),
            _ => (args[index], None),
        };
        match flag {
            "-h" | "--help" => {
                return Command::Help(format!(
                    "{}\n",
                    solstone_core_cli_copy::render_for(
                        locale,
                        "journal.cli.talent.help.passages",
                        &[]
                    )
                ));
            }
            "--json" if inline.is_none() => options.json = true,
            "--day" | "--facet" | "--activity" => {
                let value = match inline {
                    Some(value) => value,
                    None => {
                        index += 1;
                        let Some(value) = args.get(index) else {
                            return Command::Error(format!(
                                "{}\n",
                                solstone_core_cli_copy::display_for(
                                    locale,
                                    "journal.cli.talent.error.passages.value",
                                    [("flag", &(flag))]
                                )
                            ));
                        };
                        value
                    }
                };
                let slot = match flag {
                    "--day" => &mut options.day,
                    "--facet" => &mut options.facet,
                    _ => &mut options.activity,
                };
                *slot = Some(value.to_owned());
            }
            value if value.starts_with('-') => unrecognized.push(args[index]),
            value if options.name.is_empty() => options.name = value.to_owned(),
            _ => unrecognized.push(args[index]),
        }
        index += 1;
    }
    if !unrecognized.is_empty() {
        return Command::Error(format!(
            "{}\n",
            solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.talent.error.passages.unknown",
                [("arguments", &(unrecognized.join(" ")))]
            )
        ));
    }
    let missing = [
        ("name", options.name.is_empty()),
        ("--day", options.day.is_none()),
        ("--facet", options.facet.is_none()),
        ("--activity", options.activity.is_none()),
    ]
    .into_iter()
    .filter_map(|(name, missing)| missing.then_some(name))
    .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Command::Error(format!(
            "{}\n",
            solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.talent.error.passages.required",
                [("arguments", &(missing.join(", ")))]
            )
        ));
    }
    Command::Passages(options)
}

fn parse_logs(args: &[&str], locale: &str) -> Command {
    let mut options = LogsOptions::default();
    let mut unrecognized = Vec::new();
    let mut index = 0;
    while index < args.len() {
        match args[index] {
            "-h" | "--help" => {
                return Command::Help(format!(
                    "{}\n",
                    solstone_core_cli_copy::render_for(locale, "journal.cli.talent.help.logs", &[])
                ));
            }
            "-c" | "--count" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Command::Error(format!(
                        "{}\n",
                        solstone_core_cli_copy::display_for(
                            locale,
                            "journal.cli.talent.error.logs.value",
                            [("flag", &("-c/--count"))]
                        )
                    ));
                };
                match value.parse::<i64>() {
                    Ok(value) => options.count = Some(value),
                    Err(_) => {
                        return Command::Error(format!(
                            "{}\n",
                            solstone_core_cli_copy::display_for(
                                locale,
                                "journal.cli.talent.error.logs.count",
                                [("value", &(value))]
                            )
                        ));
                    }
                }
            }
            value if value.starts_with("--count=") => {
                let value = &value[8..];
                match value.parse::<i64>() {
                    Ok(value) => options.count = Some(value),
                    Err(_) => {
                        return Command::Error(format!(
                            "{}\n",
                            solstone_core_cli_copy::display_for(
                                locale,
                                "journal.cli.talent.error.logs.count",
                                [("value", &(value))]
                            )
                        ));
                    }
                }
            }
            "--day" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Command::Error(format!(
                        "{}\n",
                        solstone_core_cli_copy::display_for(
                            locale,
                            "journal.cli.talent.error.logs.value",
                            [("flag", &("--day"))]
                        )
                    ));
                };
                options.day = Some((*value).to_owned());
            }
            value if value.starts_with("--day=") => options.day = Some(value[6..].to_owned()),
            "--daily" => options.daily = true,
            "--errors" => options.errors = true,
            "--summary" => options.summary = true,
            value if value.starts_with('-') => unrecognized.push(value),
            value if options.agent.is_none() => options.agent = Some(value.to_owned()),
            value => unrecognized.push(value),
        }
        index += 1;
    }
    if !unrecognized.is_empty() {
        return Command::Error(format!(
            "{}\n",
            solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.talent.error.root.unknown",
                [("arguments", &(unrecognized.join(" ")))]
            )
        ));
    }
    Command::Logs(options)
}

fn parse_list(args: &[&str], locale: &str) -> Command {
    let mut options = ListOptions::default();
    let mut index = 0;
    while index < args.len() {
        match args[index] {
            "-h" | "--help" => {
                return Command::Help(format!(
                    "{}\n",
                    solstone_core_cli_copy::render_for(locale, "journal.cli.talent.help.list", &[])
                ));
            }
            "--disabled" => options.disabled = true,
            "--json" => options.json = true,
            "--schedule" | "--source" => {
                let flag = args[index];
                index += 1;
                let Some(value) = args.get(index) else {
                    return Command::Error(format!(
                        "{}\n",
                        solstone_core_cli_copy::display_for(
                            locale,
                            "journal.cli.talent.error.list.value",
                            [("flag", &(flag))]
                        )
                    ));
                };
                let choices: &[&str] = if flag == "--schedule" {
                    &["daily", "segment", "activity"]
                } else {
                    &["system", "app"]
                };
                if !choices.contains(value) {
                    return Command::Error(format!(
                        "{}\n",
                        solstone_core_cli_copy::display_for(
                            locale,
                            "journal.cli.talent.error.list.choice",
                            [
                                ("value", &(value)),
                                ("flag", &(flag)),
                                (
                                    "choices",
                                    &(choices
                                        .iter()
                                        .map(|item| format!("'{item}'"))
                                        .collect::<Vec<_>>()
                                        .join(", "))
                                )
                            ]
                        )
                    ));
                }
                if flag == "--schedule" {
                    options.schedule = Some((*value).to_owned());
                } else {
                    options.source = Some((*value).to_owned());
                }
            }
            value => {
                return Command::Error(format!(
                    "{}\n",
                    solstone_core_cli_copy::display_for(
                        locale,
                        "journal.cli.talent.error.list.unknown",
                        [("arguments", &(value))]
                    )
                ));
            }
        }
        index += 1;
    }
    Command::List(options)
}

fn parse_log(args: &[&str], locale: &str) -> Command {
    let mut id = None;
    let mut json = false;
    let mut full = false;
    let mut unrecognized = Vec::new();
    let mut index = 0;
    while index < args.len() {
        match args[index] {
            "-h" | "--help" => {
                return Command::Help(format!(
                    "{}\n",
                    solstone_core_cli_copy::render_for(locale, "journal.cli.talent.help.log", &[])
                ));
            }
            "--json" => json = true,
            "--full" => full = true,
            value if value.starts_with('-') => unrecognized.push(value),
            value if id.is_none() => id = Some(value.to_owned()),
            value => unrecognized.push(value),
        }
        index += 1;
    }
    let Some(id) = id else {
        return Command::Error(format!(
            "{}\n",
            solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.talent.error.log.required",
                [("arguments", &("id"))]
            )
        ));
    };
    if !unrecognized.is_empty() {
        return Command::Error(format!(
            "{}\n",
            solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.talent.error.root.unknown",
                [("arguments", &(unrecognized.join(" ")))]
            )
        ));
    }
    Command::Log(LogOptions { id, json, full })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    #[test]
    fn help_and_invalid_top_level_match_owner_program_name() {
        assert_eq!(
            parse(&[OsString::from("--help")]),
            Command::Help(HELP.to_owned())
        );
        let Command::Error(stderr) = parse(&[OsString::from("--nonsense")]) else {
            panic!("expected error");
        };
        assert_eq!(
            stderr,
            "usage: solstone journal talent [-h] [-v] [-d] {list,show,passages,logs,log} ...\nsolstone journal talent: error: unrecognized arguments: --nonsense\n"
        );
    }

    #[test]
    fn log_parser_matches_help_and_error_boundaries() {
        for flag in ["-h", "--help"] {
            assert_eq!(
                parse(&[OsString::from("log"), OsString::from(flag)]),
                Command::Help(LOG_HELP.to_owned())
            );
        }
        let Command::Error(missing) = parse(&[OsString::from("log")]) else {
            panic!("expected error");
        };
        assert_eq!(
            missing,
            "usage: solstone journal talent log [-h] [--json] [--full] id\nsolstone journal talent log: error: the following arguments are required: id\n"
        );
        for (arguments, expected) in [
            (
                ["log", "run-id", "--unknown"],
                "unrecognized arguments: --unknown",
            ),
            (["log", "run-id", "spare"], "unrecognized arguments: spare"),
        ] {
            let Command::Error(stderr) = parse(
                &arguments
                    .iter()
                    .map(|value| OsString::from(*value))
                    .collect::<Vec<_>>(),
            ) else {
                panic!("expected error");
            };
            assert_eq!(
                stderr,
                format!(
                    "usage: solstone journal talent [-h] [-v] [-d] {{list,show,passages,logs,log}} ...\nsolstone journal talent: error: {expected}\n"
                )
            );
        }
    }

    #[test]
    fn log_parser_accepts_interspersed_flags() {
        assert_eq!(
            parse(&[
                OsString::from("log"),
                OsString::from("--full"),
                OsString::from("run-id"),
                OsString::from("--json"),
            ]),
            Command::Log(LogOptions {
                id: "run-id".to_owned(),
                json: true,
                full: true,
            })
        );
    }

    #[test]
    fn show_parser_matches_help_and_interspersed_options() {
        for arguments in [&["show", "--help"][..], &["show", "read", "--help"][..]] {
            assert_eq!(
                parse(
                    &arguments
                        .iter()
                        .map(|value| OsString::from(*value))
                        .collect::<Vec<_>>(),
                ),
                Command::Help(SHOW_HELP.to_owned())
            );
        }
        assert_eq!(
            parse(
                &["show", "--prompt", "read", "--facet", "work", "--full"]
                    .iter()
                    .map(|value| OsString::from(*value))
                    .collect::<Vec<_>>(),
            ),
            Command::Show(ShowOptions {
                name: "read".to_owned(),
                prompt: true,
                facet: Some("work".to_owned()),
                full: true,
                ..ShowOptions::default()
            })
        );
        let Command::Error(stderr) = parse(&[OsString::from("show")]) else {
            panic!("expected missing name error");
        };
        assert_eq!(
            stderr,
            "usage: solstone journal talent show [-h] [--json] [--prompt] [--day YYYYMMDD]\nsolstone journal talent show: error: the following arguments are required: name\n"
        );
    }

    #[test]
    fn logs_parser_uses_its_own_help_and_integer_errors() {
        assert_eq!(
            parse(&[OsString::from("logs"), OsString::from("--help")]),
            Command::Help(LOGS_HELP.to_owned())
        );
        let Command::Error(stderr) = parse(&[
            OsString::from("logs"),
            OsString::from("-c"),
            OsString::from("nope"),
        ]) else {
            panic!("expected error");
        };
        assert_eq!(
            stderr,
            "usage: solstone journal talent logs [-h] [-c COUNT] [--day YYYYMMDD] [--daily] [--errors]\nsolstone journal talent logs: error: argument -c/--count: invalid int value: 'nope'\n"
        );
        assert_eq!(
            parse(&[
                OsString::from("logs"),
                OsString::from("app:daily"),
                OsString::from("-c"),
                OsString::from("-1"),
                OsString::from("--daily"),
                OsString::from("--errors"),
                OsString::from("--summary"),
                OsString::from("--day"),
                OsString::from("20260101"),
            ]),
            Command::Logs(LogsOptions {
                agent: Some("app:daily".to_owned()),
                count: Some(-1),
                day: Some("20260101".to_owned()),
                daily: true,
                errors: true,
                summary: true,
            })
        );
    }
}
