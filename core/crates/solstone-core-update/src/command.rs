// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::ffi::OsString;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandMode {
    Update,
    Check,
    Hold,
    Unhold,
    Rollback,
    Help,
    AutomaticOn,
    AutomaticOff,
    AutomaticStatus,
    Unattended,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateArgs {
    pub mode: CommandMode,
    pub json: bool,
    pub lane: Option<String>,
    pub version: Option<String>,
    pub yes: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandParseError {
    pub message: String,
}

impl std::fmt::Display for CommandParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for CommandParseError {}

pub fn render_help() -> String {
    render_help_in(solstone_core_cli_copy::ENGLISH)
}

pub fn render_help_in(locale: &str) -> String {
    format!(
        "{}\n",
        solstone_core_cli_copy::render_for(locale, "journal.cli.sol.update.help", &[])
    )
}

pub fn parse_args(args: &[OsString]) -> Result<UpdateArgs, CommandParseError> {
    parse_args_in(solstone_core_cli_copy::ENGLISH, args)
}

pub fn parse_args_in(locale: &str, args: &[OsString]) -> Result<UpdateArgs, CommandParseError> {
    let mut check = false;
    let mut json = false;
    let mut lane = None;
    let mut version = None;
    let mut hold = false;
    let mut unhold = false;
    let mut rollback = false;
    let mut yes = false;
    let mut help = false;
    let mut automatic = None;
    let mut unattended = false;

    let mut iter = args.iter().peekable();
    while let Some(arg) = iter.next() {
        let s = arg.to_str().ok_or_else(|| CommandParseError {
            message: solstone_core_cli_copy::render_for(
                locale,
                "journal.cli.sol.update.argument.utf8",
                &[],
            ),
        })?;

        if s == "--help" || s == "-h" {
            help = true;
        } else if s == "--check" {
            check = true;
        } else if s == "--json" {
            json = true;
        } else if s == "--hold" {
            hold = true;
        } else if s == "--unhold" {
            unhold = true;
        } else if s == "--rollback" {
            rollback = true;
        } else if s == "--yes" || s == "-y" {
            yes = true;
        } else if s == "--unattended" {
            unattended = true;
        } else if s == "--automatic" || s.starts_with("--automatic=") {
            let value = match s.strip_prefix("--automatic=") {
                Some(value) => value.to_owned(),
                None => iter
                    .next()
                    .and_then(|value| value.to_str())
                    .ok_or_else(|| CommandParseError {
                        message: solstone_core_cli_copy::render_for(
                            locale,
                            "journal.cli.sol.update.argument.automatic.missing",
                            &[],
                        ),
                    })?
                    .to_owned(),
            };
            automatic = Some(match value.as_str() {
                "on" => CommandMode::AutomaticOn,
                "off" => CommandMode::AutomaticOff,
                "status" => CommandMode::AutomaticStatus,
                _ => {
                    return Err(CommandParseError {
                        message: solstone_core_cli_copy::display_for(
                            locale,
                            "journal.cli.sol.update.argument.automatic.value",
                            [("value", &(value))],
                        ),
                    });
                }
            });
        } else if s == "--lane" {
            let next_val = iter.next().ok_or_else(|| CommandParseError {
                message: solstone_core_cli_copy::render_for(
                    locale,
                    "journal.cli.sol.update.argument.lane.missing",
                    &[],
                ),
            })?;
            let lane_str = next_val.to_str().ok_or_else(|| CommandParseError {
                message: solstone_core_cli_copy::render_for(
                    locale,
                    "journal.cli.sol.update.argument.lane.utf8",
                    &[],
                ),
            })?;
            if lane_str != "release" && lane_str != "staging" && lane_str != "dev" {
                return Err(CommandParseError {
                    message: solstone_core_cli_copy::display_for(
                        locale,
                        "journal.cli.sol.update.argument.lane.value",
                        [("lane", &(lane_str))],
                    ),
                });
            }
            lane = Some(lane_str.to_owned());
        } else if let Some(lane_str) = s.strip_prefix("--lane=") {
            if lane_str != "release" && lane_str != "staging" && lane_str != "dev" {
                return Err(CommandParseError {
                    message: solstone_core_cli_copy::display_for(
                        locale,
                        "journal.cli.sol.update.argument.lane.value",
                        [("lane", &(lane_str))],
                    ),
                });
            }
            lane = Some(lane_str.to_owned());
        } else if s == "--version" {
            let next_val = iter.next().ok_or_else(|| CommandParseError {
                message: solstone_core_cli_copy::render_for(
                    locale,
                    "journal.cli.sol.update.argument.version.missing",
                    &[],
                ),
            })?;
            let ver_str = next_val.to_str().ok_or_else(|| CommandParseError {
                message: solstone_core_cli_copy::render_for(
                    locale,
                    "journal.cli.sol.update.argument.version.utf8",
                    &[],
                ),
            })?;
            validate_version_pattern(locale, ver_str)?;
            version = Some(ver_str.to_owned());
        } else if let Some(ver_str) = s.strip_prefix("--version=") {
            validate_version_pattern(locale, ver_str)?;
            version = Some(ver_str.to_owned());
        } else {
            return Err(CommandParseError {
                message: solstone_core_cli_copy::display_for(
                    locale,
                    "journal.cli.sol.update.argument.unknown",
                    [("flag", &(s))],
                ),
            });
        }
    }

    if help {
        return Ok(UpdateArgs {
            mode: CommandMode::Help,
            json,
            lane,
            version,
            yes,
        });
    }

    let mode_flags_count =
        (check as usize) + (hold as usize) + (unhold as usize) + (rollback as usize);
    if mode_flags_count > 1 {
        return Err(CommandParseError {
            message: solstone_core_cli_copy::render_for(
                locale,
                "journal.cli.sol.update.argument.mode.conflict",
                &[],
            ),
        });
    }

    if let Some(mode) = automatic.or(unattended.then_some(CommandMode::Unattended)) {
        if mode_flags_count > 0
            || yes
            || lane.is_some()
            || version.is_some()
            || (automatic.is_some() && unattended)
        {
            return Err(CommandParseError {
                message: solstone_core_cli_copy::render_for(
                    locale,
                    "journal.cli.sol.update.argument.automatic.conflict",
                    &[],
                ),
            });
        }
        return Ok(UpdateArgs {
            mode,
            json,
            lane,
            version,
            yes,
        });
    }

    if yes && (check || hold || unhold) {
        return Err(CommandParseError {
            message: solstone_core_cli_copy::render_for(
                locale,
                "journal.cli.sol.update.argument.yes.conflict",
                &[],
            ),
        });
    }

    if version.is_some() && (check || hold || unhold) {
        return Err(CommandParseError {
            message: solstone_core_cli_copy::render_for(
                locale,
                "journal.cli.sol.update.argument.version.conflict",
                &[],
            ),
        });
    }

    let mode = if check {
        CommandMode::Check
    } else if hold {
        CommandMode::Hold
    } else if unhold {
        CommandMode::Unhold
    } else if rollback {
        CommandMode::Rollback
    } else {
        CommandMode::Update
    };

    Ok(UpdateArgs {
        mode,
        json,
        lane,
        version,
        yes,
    })
}

fn validate_version_pattern(locale: &str, version: &str) -> Result<(), CommandParseError> {
    let (base, revision) = match version.split_once("-r") {
        Some((b, r)) => (b, Some(r)),
        None => (version, None),
    };

    let parts: Vec<&str> = base.split('.').collect();
    if parts.len() != 3 {
        return Err(CommandParseError {
            message: solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.sol.update.argument.version.format",
                [("version", &(version))],
            ),
        });
    }

    for part in parts {
        if part.is_empty() || !part.chars().all(|c| c.is_ascii_digit()) {
            return Err(CommandParseError {
                message: solstone_core_cli_copy::display_for(
                    locale,
                    "journal.cli.sol.update.argument.version.numeric",
                    [("version", &(version))],
                ),
            });
        }
    }

    if let Some(rev) = revision
        && (rev.is_empty() || !rev.chars().all(|c| c.is_ascii_digit()))
    {
        return Err(CommandParseError {
            message: solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.sol.update.argument.version.revision",
                [("version", &(version))],
            ),
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_rejects_conflicting_mode_flags() {
        let args = vec![OsString::from("--check"), OsString::from("--hold")];
        let err = parse_args(&args).unwrap_err();
        assert!(err.message.contains("choose only one"));

        let args = vec![OsString::from("--rollback"), OsString::from("--unhold")];
        let err = parse_args(&args).unwrap_err();
        assert!(err.message.contains("choose only one"));
    }

    #[test]
    fn parse_rejects_yes_with_check_or_hold_modes() {
        let args = vec![OsString::from("--check"), OsString::from("--yes")];
        let err = parse_args(&args).unwrap_err();
        assert!(err.message.contains("--yes cannot be combined"));

        let args = vec![OsString::from("--hold"), OsString::from("-y")];
        let err = parse_args(&args).unwrap_err();
        assert!(err.message.contains("--yes cannot be combined"));

        let args = vec![OsString::from("--unhold"), OsString::from("--yes")];
        let err = parse_args(&args).unwrap_err();
        assert!(err.message.contains("--yes cannot be combined"));
    }

    #[test]
    fn parse_rejects_version_with_check_or_hold_modes() {
        let args = vec![
            OsString::from("--check"),
            OsString::from("--version"),
            OsString::from("2.0.48"),
        ];
        let err = parse_args(&args).unwrap_err();
        assert!(err.message.contains("--version cannot be combined"));

        let args = vec![
            OsString::from("--hold"),
            OsString::from("--version"),
            OsString::from("2.0.48"),
        ];
        let err = parse_args(&args).unwrap_err();
        assert!(err.message.contains("--version cannot be combined"));
    }

    #[test]
    fn parse_accepts_rollback_with_explicit_version() {
        let args = vec![
            OsString::from("--rollback"),
            OsString::from("--version"),
            OsString::from("2.0.47"),
        ];
        let parsed = parse_args(&args).expect("valid rollback with version");
        assert_eq!(parsed.mode, CommandMode::Rollback);
        assert_eq!(parsed.version.as_deref(), Some("2.0.47"));
    }

    #[test]
    fn parse_rejects_invalid_lane_string() {
        let args = vec![OsString::from("--lane"), OsString::from("nightly")];
        let err = parse_args(&args).unwrap_err();
        assert!(err.message.contains("invalid release lane"));
    }

    #[test]
    fn parse_rejects_malformed_version_pattern() {
        let bad = ["2.0", "v2.0.48", "2.0.48-beta", "2.0.48-r", "2.0.x"];
        for v in bad {
            let args = vec![OsString::from("--version"), OsString::from(v)];
            assert!(parse_args(&args).is_err(), "expected error for {v}");
        }
    }

    #[test]
    fn automatic_and_unattended_stand_alone() {
        let parse =
            |args: &[&str]| parse_args(&args.iter().map(OsString::from).collect::<Vec<_>>());
        assert_eq!(
            parse(&["--automatic", "on"]).unwrap().mode,
            CommandMode::AutomaticOn
        );
        assert_eq!(
            parse(&["--automatic=off"]).unwrap().mode,
            CommandMode::AutomaticOff
        );
        assert_eq!(
            parse(&["--automatic", "status", "--json"]).unwrap().mode,
            CommandMode::AutomaticStatus
        );
        assert_eq!(
            parse(&["--unattended"]).unwrap().mode,
            CommandMode::Unattended
        );
        for bad in [
            &["--automatic"][..],
            &["--automatic", "yes"],
            &["--unattended", "--yes"],
            &["--unattended", "--version", "2.0.80"],
            &["--automatic", "on", "--check"],
            &["--automatic", "on", "--unattended"],
        ] {
            assert!(parse(bad).is_err(), "expected error for {bad:?}");
        }
    }

    #[test]
    fn parse_rejects_unknown_flag() {
        let args = vec![OsString::from("--unknown-option")];
        let err = parse_args(&args).unwrap_err();
        assert!(err.message.contains("unrecognized flag"));
    }
}
