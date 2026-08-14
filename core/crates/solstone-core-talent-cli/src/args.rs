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

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct ListOptions {
    pub(crate) schedule: Option<String>,
    pub(crate) source: Option<String>,
    pub(crate) disabled: bool,
    pub(crate) json: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Command {
    Help(String),
    List(ListOptions),
    Stub(&'static str),
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
        "show" | "logs" | "log" | "inventory" => {
            let name = args[index];
            if args[index + 1..]
                .iter()
                .any(|arg| matches!(*arg, "-h" | "--help"))
            {
                Command::Help(format!(
                    "usage: journal talent {name} [-h]\n\noptions:\n  -h, --help  show this help message and exit\n"
                ))
            } else {
                Command::Stub(match name {
                    "show" => "show",
                    "logs" => "logs",
                    "log" => "log",
                    _ => "inventory",
                })
            }
        }
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

fn error(program: &str, message: &str) -> Command {
    let usage = if program == "journal talent list" {
        LIST_HELP.lines().next().unwrap_or_default()
    } else {
        HELP.lines().next().unwrap_or_default()
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
}
