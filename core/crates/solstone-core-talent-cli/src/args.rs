// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::ffi::OsString;

pub const HELP: &str = concat!(
    "usage: journal talent [-h] [-v] [-d] {list,inventory,show,logs,log} ...\n\n",
    "Inspect talent prompt configurations\n\n",
    "positional arguments:\n",
    "  {list,inventory,show,logs,log}\n",
    "    list                List prompts grouped by schedule\n",
    "    inventory           List cogitate talent runtime surfaces\n",
    "    show                Show details for a specific prompt\n",
    "    logs                Show recent talent run log\n",
    "    log                 Show events for an agent run\n\n",
    "options:\n",
    "  -h, --help            show this help message and exit\n",
    "  -v, --verbose         Enable verbose output\n",
    "  -d, --debug           Enable debug logging\n",
);

pub const LIST_HELP: &str = concat!(
    "usage: journal talent list [-h] [--schedule {daily,segment,activity}]\n",
    "                           [--source {system,app}] [--disabled] [--json]\n\n",
    "options:\n",
    "  -h, --help            show this help message and exit\n",
    "  --schedule {daily,segment,activity}\n",
    "                        Filter by schedule type\n",
    "  --source {system,app}\n",
    "                        Filter by origin\n",
    "  --disabled            Include disabled prompts\n",
    "  --json                Output as JSONL\n",
);

pub const LOG_HELP: &str = concat!(
    "usage: journal talent log [-h] [--json] [--full] id\n\n",
    "positional arguments:\n",
    "  id          Agent ID\n\n",
    "options:\n",
    "  -h, --help  show this help message and exit\n",
    "  --json      Output raw JSONL\n",
    "  --full      Expand event details\n",
);

pub const INVENTORY_HELP: &str = concat!(
    "usage: journal talent inventory [-h] [--json]\n\n",
    "options:\n",
    "  -h, --help  show this help message and exit\n",
    "  --json      Output as JSON\n",
);

pub const LOGS_HELP: &str = concat!(
    "usage: journal talent logs [-h] [-c COUNT] [--day YYYYMMDD] [--daily] [--errors]\n",
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

pub const SHOW_HELP: &str = concat!(
    "usage: journal talent show [-h] [--json] [--prompt] [--day YYYYMMDD]\n",
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

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct InventoryOptions {
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

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Command {
    Help(String),
    List(ListOptions),
    Log(LogOptions),
    Inventory(InventoryOptions),
    Logs(LogsOptions),
    Show(ShowOptions),
    Error(String),
}

pub(crate) fn parse(args: &[OsString]) -> Command {
    let args = match args
        .iter()
        .map(|arg| arg.to_str())
        .collect::<Option<Vec<_>>>()
    {
        Some(args) => args,
        None => return error("journal talent", "arguments are not valid UTF-8"),
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
                Command::Help(HELP.to_owned())
            } else {
                error(
                    "journal talent",
                    &format!("unrecognized arguments: {}", args[index + 1..].join(" ")),
                )
            }
        }
        "list" => parse_list(&args[index + 1..]),
        "log" => parse_log(&args[index + 1..]),
        "inventory" => parse_inventory(&args[index + 1..]),
        "logs" => parse_logs(&args[index + 1..]),
        "show" => parse_show(&args[index + 1..]),
        value if value.starts_with('-') => error(
            "journal talent",
            &format!("unrecognized arguments: {value}"),
        ),
        value => error(
            "journal talent",
            &format!(
                "argument subcommand: invalid choice: '{value}' (choose from 'list', 'inventory', 'show', 'logs', 'log')"
            ),
        ),
    }
}

fn parse_inventory(args: &[&str]) -> Command {
    let mut options = InventoryOptions::default();
    let mut index = 0;
    while index < args.len() {
        match args[index] {
            "-h" | "--help" => return Command::Help(INVENTORY_HELP.to_owned()),
            "--json" => options.json = true,
            value => {
                return error(
                    "journal talent inventory",
                    &format!("unrecognized arguments: {value}"),
                );
            }
        }
        index += 1;
    }
    Command::Inventory(options)
}

fn parse_show(args: &[&str]) -> Command {
    let mut options = ShowOptions::default();
    let mut unrecognized = Vec::new();
    let mut index = 0;
    while index < args.len() {
        match args[index] {
            "-h" | "--help" => return Command::Help(SHOW_HELP.to_owned()),
            "--json" => options.json = true,
            "--prompt" => options.prompt = true,
            "--full" => options.full = true,
            "--day" | "--segment" | "--facet" | "--activity" | "--query" => {
                let flag = args[index];
                index += 1;
                let Some(value) = args.get(index) else {
                    return error(
                        "journal talent show",
                        &format!("argument {flag}: expected one argument"),
                    );
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
        return error(
            "journal talent show",
            &format!("unrecognized arguments: {}", unrecognized.join(" ")),
        );
    }
    if options.name.is_empty() {
        return error(
            "journal talent show",
            "the following arguments are required: name",
        );
    }
    Command::Show(options)
}

fn parse_logs(args: &[&str]) -> Command {
    let mut options = LogsOptions::default();
    let mut unrecognized = Vec::new();
    let mut index = 0;
    while index < args.len() {
        match args[index] {
            "-h" | "--help" => return Command::Help(LOGS_HELP.to_owned()),
            "-c" | "--count" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return error(
                        "journal talent logs",
                        "argument -c/--count: expected one argument",
                    );
                };
                match value.parse::<i64>() {
                    Ok(value) => options.count = Some(value),
                    Err(_) => {
                        return error(
                            "journal talent logs",
                            &format!("argument -c/--count: invalid int value: '{value}'"),
                        );
                    }
                }
            }
            value if value.starts_with("--count=") => {
                let value = &value[8..];
                match value.parse::<i64>() {
                    Ok(value) => options.count = Some(value),
                    Err(_) => {
                        return error(
                            "journal talent logs",
                            &format!("argument -c/--count: invalid int value: '{value}'"),
                        );
                    }
                }
            }
            "--day" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return error(
                        "journal talent logs",
                        "argument --day: expected one argument",
                    );
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
        return error(
            "journal talent",
            &format!("unrecognized arguments: {}", unrecognized.join(" ")),
        );
    }
    Command::Logs(options)
}

fn parse_list(args: &[&str]) -> Command {
    let mut options = ListOptions::default();
    let mut index = 0;
    while index < args.len() {
        match args[index] {
            "-h" | "--help" => return Command::Help(LIST_HELP.to_owned()),
            "--disabled" => options.disabled = true,
            "--json" => options.json = true,
            "--schedule" | "--source" => {
                let flag = args[index];
                index += 1;
                let Some(value) = args.get(index) else {
                    return error(
                        "journal talent list",
                        &format!("argument {flag}: expected one argument"),
                    );
                };
                let choices: &[&str] = if flag == "--schedule" {
                    &["daily", "segment", "activity"]
                } else {
                    &["system", "app"]
                };
                if !choices.contains(value) {
                    return error(
                        "journal talent list",
                        &format!(
                            "argument {flag}: invalid choice: '{value}' (choose from {})",
                            choices
                                .iter()
                                .map(|item| format!("'{item}'"))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    );
                }
                if flag == "--schedule" {
                    options.schedule = Some((*value).to_owned());
                } else {
                    options.source = Some((*value).to_owned());
                }
            }
            value => {
                return error(
                    "journal talent list",
                    &format!("unrecognized arguments: {value}"),
                );
            }
        }
        index += 1;
    }
    Command::List(options)
}

fn parse_log(args: &[&str]) -> Command {
    let mut id = None;
    let mut json = false;
    let mut full = false;
    let mut unrecognized = Vec::new();
    let mut index = 0;
    while index < args.len() {
        match args[index] {
            "-h" | "--help" => return Command::Help(LOG_HELP.to_owned()),
            "--json" => json = true,
            "--full" => full = true,
            value if value.starts_with('-') => unrecognized.push(value),
            value if id.is_none() => id = Some(value.to_owned()),
            value => unrecognized.push(value),
        }
        index += 1;
    }
    let Some(id) = id else {
        return error(
            "journal talent log",
            "the following arguments are required: id",
        );
    };
    if !unrecognized.is_empty() {
        return error(
            "journal talent",
            &format!("unrecognized arguments: {}", unrecognized.join(" ")),
        );
    }
    Command::Log(LogOptions { id, json, full })
}

fn error(program: &str, message: &str) -> Command {
    let usage = match program {
        "journal talent list" => LIST_HELP.lines().next().unwrap_or_default(),
        "journal talent log" => LOG_HELP.lines().next().unwrap_or_default(),
        "journal talent inventory" => INVENTORY_HELP.lines().next().unwrap_or_default(),
        "journal talent logs" => LOGS_HELP.lines().next().unwrap_or_default(),
        "journal talent show" => SHOW_HELP.lines().next().unwrap_or_default(),
        _ => HELP.lines().next().unwrap_or_default(),
    };
    Command::Error(format!("{usage}\n{program}: error: {message}\n"))
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
            "usage: journal talent [-h] [-v] [-d] {list,inventory,show,logs,log} ...\njournal talent: error: unrecognized arguments: --nonsense\n"
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
            "usage: journal talent log [-h] [--json] [--full] id\njournal talent log: error: the following arguments are required: id\n"
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
                    "usage: journal talent [-h] [-v] [-d] {{list,inventory,show,logs,log}} ...\njournal talent: error: {expected}\n"
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
            "usage: journal talent show [-h] [--json] [--prompt] [--day YYYYMMDD]\njournal talent show: error: the following arguments are required: name\n"
        );
    }

    #[test]
    fn inventory_parses_options_help_and_errors() {
        assert_eq!(
            parse(&[OsString::from("inventory")]),
            Command::Inventory(InventoryOptions::default())
        );
        assert_eq!(
            parse(&[OsString::from("inventory"), OsString::from("--json")]),
            Command::Inventory(InventoryOptions { json: true })
        );
        for help in ["-h", "--help"] {
            assert_eq!(
                parse(&[OsString::from("inventory"), OsString::from(help)]),
                Command::Help(INVENTORY_HELP.to_owned())
            );
        }
        assert_eq!(
            parse(&[OsString::from("inventory"), OsString::from("--bad")]),
            Command::Error(
                "usage: journal talent inventory [-h] [--json]\njournal talent inventory: error: unrecognized arguments: --bad\n".to_owned()
            )
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
            "usage: journal talent logs [-h] [-c COUNT] [--day YYYYMMDD] [--daily] [--errors]\njournal talent logs: error: argument -c/--count: invalid int value: 'nope'\n"
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
