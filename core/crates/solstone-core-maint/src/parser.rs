// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

pub const USAGE: &str = "usage: journal maint [-h] [--list] [--force] [-v] [-d] [task]\n";

const HELP: &str = concat!(
    "usage: journal maint [-h] [--list] [--force] [-v] [-d] [task]\n\n",
    "Run maintenance tasks for apps\n\n",
    "positional arguments:\n",
    "  task                 Task to show details for (or to re-run with --force)\n\n",
    "options:\n",
    "  -h, --help           show this help message and exit\n",
    "  --list, -l           List all tasks with their status\n",
    "  --force, -f          Re-run a specific task (requires task name)\n",
    "  -v, --verbose        Enable verbose output\n",
    "  -d, --debug          Enable debug logging\n\n",
    "Examples:\n",
    "    journal maint              Run all pending maintenance tasks\n",
    "    journal maint --list       Show status of all tasks\n",
    "    journal maint chat:fix_x   Show task details and log output\n",
    "    journal maint -f fix_x     Re-run a specific task\n"
);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedArgs {
    pub task: Option<String>,
    pub list: bool,
    pub force: bool,
    pub verbose: bool,
    pub debug: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseOutcome {
    Parsed(ParsedArgs),
    Help,
    Error(String),
}

pub fn parse(args: &[String]) -> ParseOutcome {
    let mut parsed = ParsedArgs {
        task: None,
        list: false,
        force: false,
        verbose: false,
        debug: false,
    };
    let mut positionals = false;
    for argument in args {
        if !positionals && argument == "--" {
            positionals = true;
            continue;
        }
        if !positionals {
            match argument.as_str() {
                "-h" | "--help" => return ParseOutcome::Help,
                "--list" | "-l" => {
                    parsed.list = true;
                    continue;
                }
                "--force" | "-f" => {
                    parsed.force = true;
                    continue;
                }
                "-v" | "--verbose" => {
                    parsed.verbose = true;
                    continue;
                }
                "-d" | "--debug" => {
                    parsed.debug = true;
                    continue;
                }
                _ if argument.starts_with('-') => return unrecognized(argument),
                _ => {}
            }
        }
        if parsed.task.replace(argument.clone()).is_some() {
            return unrecognized(argument);
        }
    }
    ParseOutcome::Parsed(parsed)
}

pub const fn help() -> &'static str {
    HELP
}

fn unrecognized(argument: &str) -> ParseOutcome {
    ParseOutcome::Error(format!(
        "{USAGE}journal maint: error: unrecognized arguments: {argument}\n"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn parser_handles_flags_positionals_and_terminator() {
        assert_eq!(
            parse(&args(&["-v", "--debug", "-f", "task"])),
            ParseOutcome::Parsed(ParsedArgs {
                task: Some("task".to_owned()),
                list: false,
                force: true,
                verbose: true,
                debug: true,
            })
        );
        assert_eq!(
            parse(&args(&["--", "--list"])),
            ParseOutcome::Parsed(ParsedArgs {
                task: Some("--list".to_owned()),
                list: false,
                force: false,
                verbose: false,
                debug: false,
            })
        );
    }

    #[test]
    fn parser_preserves_list_precedence_inputs_and_usage_anchor() {
        let ParseOutcome::Parsed(parsed) = parse(&args(&["--list", "--force", "name"])) else {
            panic!("list arguments parse");
        };
        assert!(parsed.list);
        assert!(parsed.force);
        assert_eq!(parsed.task.as_deref(), Some("name"));
        let ParseOutcome::Error(error) = parse(&args(&["--nonsense"])) else {
            panic!("unknown flag must fail");
        };
        assert!(error.starts_with("usage: journal maint"));
        assert!(help().contains("Run maintenance tasks for apps"));
    }
}
