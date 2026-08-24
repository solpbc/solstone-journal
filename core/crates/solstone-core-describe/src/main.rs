// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::env;
use std::ffi::OsString;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde_json::Value;
use solstone_core_cli::DESCRIBE_USAGE;
use solstone_core_describe::selection::{CategoryOverride, Importance};
use solstone_core_describe::{
    ConveyFiducialMask, WinnowConfig, pipeline, process_video_with_transform_metadata,
};
use solstone_core_journal_config::read_journal_config;

const EXIT_DECODE_FAILURE: u8 = 2;
const EXIT_USAGE: u8 = 2;
const EXIT_CONFIG: u8 = 78;
const EXIT_PROVIDER_BLOCKED: u8 = 69;

fn install_logger() {
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn"))
        .try_init();
}

fn main() -> ExitCode {
    install_logger();
    match run(env::args_os().skip(1)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(CliError::Usage(message)) => {
            eprintln!("{message}");
            ExitCode::from(EXIT_USAGE)
        }
        Err(CliError::Config(message)) => {
            eprintln!("{message}");
            ExitCode::from(EXIT_CONFIG)
        }
        Err(CliError::Decode(message)) => {
            eprintln!("{message}");
            ExitCode::from(EXIT_DECODE_FAILURE)
        }
        Err(CliError::Blocked(reason)) => {
            eprintln!("{}", describe_deferred_message(reason.as_deref()));
            ExitCode::from(EXIT_PROVIDER_BLOCKED)
        }
        Err(CliError::Internal(message)) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

fn run(arguments: impl IntoIterator<Item = OsString>) -> Result<(), CliError> {
    match parse_arguments(arguments)? {
        Command::Version => {
            let version = ffmpeg_next::codec::version();
            println!(
                "solstone-core-describe libavcodec {}.{}.{}",
                version >> 16,
                (version >> 8) & 0xff,
                version & 0xff
            );
            Ok(())
        }
        Command::Help => {
            print!("{DESCRIBE_USAGE}");
            Ok(())
        }
        Command::CategoryRegistry => {
            let rendered = serde_json::to_string_pretty(
                &solstone_core_describe::categories::category_registry(),
            )
            .map_err(|error| CliError::Internal(error.to_string()))?;
            println!("{rendered}");
            Ok(())
        }
        Command::FramesOnly(arguments) => {
            let config = read_config(arguments.journal.as_deref())?;
            let mut transform = ConveyFiducialMask;
            let result = process_video_with_transform_metadata(
                &arguments.video_path,
                &mut transform,
                config.winnow,
            );
            if result.decode_failed {
                return Err(CliError::Decode(format!(
                    "failed to decode video: {}",
                    arguments.video_path.display()
                )));
            }
            let frames: Vec<Value> = result
                .qualified_frames
                .into_iter()
                .map(|frame| {
                    serde_json::json!({
                        "frame_id": frame.frame_id,
                        "timestamp": frame.timestamp,
                    })
                })
                .collect();
            let video = arguments
                .video_path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| {
                    CliError::Usage("video path must have a UTF-8 filename".to_owned())
                })?;
            let output = serde_json::json!({
                "video": video,
                "width": result.width,
                "height": result.height,
                "frames": frames,
            });
            let rendered = serde_json::to_string_pretty(&output)
                .map_err(|error| CliError::Decode(format!("failed to encode output: {error}")))?;
            io::stdout()
                .write_all(format!("{rendered}\n").as_bytes())
                .map_err(|error| CliError::Decode(format!("failed to write output: {error}")))
        }
        Command::Describe(arguments) => {
            let config = read_config(arguments.journal.as_deref())?;
            let default_journal = env::var_os("SOLSTONE_JOURNAL").map(PathBuf::from);
            let journal = arguments
                .journal
                .as_deref()
                .or(default_journal.as_deref())
                .unwrap_or_else(|| Path::new("."));
            pipeline::run(pipeline::DescribeOptions {
                video: &arguments.video_path,
                journal,
                explicit_journal: arguments.journal.as_deref(),
                jobs: arguments.jobs,
                redo: arguments.redo,
                config: config.winnow,
                redact_rules: config.redact_rules,
                max_extractions: config.max_extractions,
                category_overrides: config.category_overrides,
            })
            .map_err(|error| match error {
                pipeline::RunError::Blocked(reason) => CliError::Blocked(reason),
                pipeline::RunError::Internal(message) => CliError::Internal(message),
            })
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Version,
    Help,
    CategoryRegistry,
    FramesOnly(FramesOnlyArguments),
    Describe(DescribeArguments),
}

#[derive(Debug, PartialEq, Eq)]
struct FramesOnlyArguments {
    video_path: PathBuf,
    journal: Option<PathBuf>,
}
#[derive(Debug, PartialEq, Eq)]
struct DescribeArguments {
    video_path: PathBuf,
    journal: Option<PathBuf>,
    jobs: usize,
    redo: bool,
}

#[derive(Debug)]
enum CliError {
    Usage(String),
    Config(String),
    Decode(String),
    Blocked(Option<String>),
    Internal(String),
}

fn describe_deferred_message(reason: Option<&str>) -> String {
    match reason {
        Some(token) if !token.is_empty() => format!("describe deferred: {token}"),
        _ => "describe deferred".to_string(),
    }
}

fn parse_arguments(arguments: impl IntoIterator<Item = OsString>) -> Result<Command, CliError> {
    let mut values = arguments.into_iter();
    let Some(first) = values.next() else {
        return Err(usage("the following arguments are required: FILE"));
    };
    if first == "--version" {
        if values.next().is_some() {
            return Err(usage("--version does not accept other arguments"));
        }
        return Ok(Command::Version);
    }
    if first == "--category-registry" {
        if values.next().is_some() {
            return Err(usage("--category-registry does not accept other arguments"));
        }
        return Ok(Command::CategoryRegistry);
    }

    let mut frames_only = false;
    let mut describe = false;
    let mut redo = false;
    let mut jobs = None;
    let mut journal = None;
    let mut video_path = None;
    let mut pending = Some(first);
    while let Some(argument) = pending.take().or_else(|| values.next()) {
        if argument == "-h" || argument == "--help" {
            if values.next().is_some() {
                return Err(usage("--help does not accept other arguments"));
            }
            return Ok(Command::Help);
        } else if argument == "--frames-only" {
            if frames_only {
                return Err(usage("--frames-only was provided more than once"));
            }
            frames_only = true;
        } else if argument == "--describe" {
            if describe {
                return Err(usage("--describe was provided more than once"));
            }
            describe = true;
        } else if matches!(
            argument.to_str(),
            Some("-d" | "--debug" | "-v" | "--verbose")
        ) {
            // Owner-facing dispatcher flags are intentionally accepted as no-ops.
        } else if argument == "--redo" {
            if redo {
                return Err(usage("--redo was provided more than once"));
            }
            redo = true;
        } else if argument == "-j" || argument == "--jobs" {
            let Some(value) = values.next() else {
                return Err(usage("--jobs requires a positive integer"));
            };
            let value = value
                .to_string_lossy()
                .parse::<usize>()
                .ok()
                .filter(|value| *value > 0)
                .ok_or_else(|| usage("--jobs requires a positive integer"))?;
            if jobs.replace(value).is_some() {
                return Err(usage("--jobs was provided more than once"));
            }
        } else if argument == "--journal" {
            let Some(path) = values.next() else {
                return Err(usage("--journal requires a path"));
            };
            if journal.replace(PathBuf::from(path)).is_some() {
                return Err(usage("--journal was provided more than once"));
            }
        } else if argument.to_string_lossy().starts_with('-') {
            return Err(usage(&format!(
                "unknown argument: {}",
                argument.to_string_lossy()
            )));
        } else if video_path.replace(PathBuf::from(argument)).is_some() {
            return Err(usage("only one video path may be provided"));
        }
    }

    if frames_only == describe {
        return Err(usage(
            "exactly one of --frames-only or --describe is required",
        ));
    }
    let Some(video_path) = video_path else {
        return Err(usage("the following arguments are required: FILE"));
    };
    if frames_only {
        if redo {
            return Err(usage("--redo requires --describe"));
        }
        Ok(Command::FramesOnly(FramesOnlyArguments {
            video_path,
            journal,
        }))
    } else {
        Ok(Command::Describe(DescribeArguments {
            video_path,
            journal,
            jobs: jobs.unwrap_or(1),
            redo,
        }))
    }
}

struct DescribeConfig {
    winnow: WinnowConfig,
    redact_rules: Vec<String>,
    max_extractions: u32,
    category_overrides: BTreeMap<String, CategoryOverride>,
}

fn read_config(journal_path: Option<&Path>) -> Result<DescribeConfig, CliError> {
    let Some(journal_path) = journal_path else {
        return Ok(DescribeConfig {
            winnow: WinnowConfig::default(),
            redact_rules: Vec::new(),
            max_extractions: 20,
            category_overrides: BTreeMap::new(),
        });
    };
    let config = read_journal_config(journal_path)
        .map_err(|error| CliError::Config(format!("failed to read journal config: {error}")))?
        .config;
    let Some(describe) = config
        .as_ref()
        .and_then(|config| config.get("describe"))
        .and_then(Value::as_object)
    else {
        return Ok(DescribeConfig {
            winnow: WinnowConfig::default(),
            redact_rules: Vec::new(),
            max_extractions: 20,
            category_overrides: BTreeMap::new(),
        });
    };

    let mut result = WinnowConfig::default();
    if let Some(value) = describe.get("scene_cut_threshold") {
        let Some(value) = value.as_u64().and_then(|value| u32::try_from(value).ok()) else {
            return Err(CliError::Config(
                "describe.scene_cut_threshold must be an unsigned 32-bit integer".to_owned(),
            ));
        };
        result.scene_cut_threshold = value;
    }
    if let Some(value) = describe.get("min_stride_seconds") {
        let Some(value) = value.as_f64().filter(|value| value.is_finite()) else {
            return Err(CliError::Config(
                "describe.min_stride_seconds must be a finite number".to_owned(),
            ));
        };
        result.min_stride_seconds = value;
    }
    let redact_rules = match describe.get("redact") {
        None => Vec::new(),
        Some(Value::Array(rules)) => rules
            .iter()
            .map(|value| {
                value.as_str().map(str::to_owned).ok_or_else(|| {
                    CliError::Config("describe.redact must be an array of strings".to_owned())
                })
            })
            .collect::<Result<Vec<_>, _>>()?,
        Some(_) => {
            return Err(CliError::Config(
                "describe.redact must be an array of strings".to_owned(),
            ));
        }
    };
    let max_extractions = match describe.get("max_extractions") {
        None => 20,
        Some(value) => value
            .as_u64()
            .and_then(|value| u32::try_from(value).ok())
            .ok_or_else(|| {
                CliError::Config(
                    "describe.max_extractions must be an unsigned 32-bit integer".to_owned(),
                )
            })?,
    };
    let category_overrides = match describe.get("categories") {
        None => BTreeMap::new(),
        Some(Value::Object(categories)) => categories
            .iter()
            .filter_map(|(name, value)| (!value.is_null()).then_some((name, value)))
            .map(|(name, value)| {
                let value = value.as_object().ok_or_else(|| {
                    CliError::Config(format!("describe.categories.{name} must be an object"))
                })?;
                let importance = match value.get("importance") {
                    None => None,
                    Some(Value::String(value)) => Some(Importance::parse(value).ok_or_else(|| {
                        CliError::Config(format!("describe.categories.{name}.importance must be ignore, low, normal, or high"))
                    })?),
                    Some(_) => return Err(CliError::Config(format!("describe.categories.{name}.importance must be a string"))),
                };
                let extraction = match value.get("extraction") {
                    None => None,
                    Some(Value::String(value)) => Some(value.clone()),
                    Some(_) => return Err(CliError::Config(format!("describe.categories.{name}.extraction must be a string"))),
                };
                Ok((name.clone(), CategoryOverride { importance, extraction }))
            })
            .collect::<Result<BTreeMap<_, _>, _>>()?,
        Some(_) => return Err(CliError::Config("describe.categories must be an object".to_owned())),
    };
    Ok(DescribeConfig {
        winnow: result,
        redact_rules,
        max_extractions,
        category_overrides,
    })
}

fn usage(message: &str) -> CliError {
    CliError::Usage(format!(
        "{DESCRIBE_USAGE}journal describe: error: {message}"
    ))
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::PathBuf;

    use super::{CliError, Command, DescribeArguments, parse_arguments};

    #[test]
    fn explicit_describe_mode_is_required_and_parses_arguments() {
        let parsed = parse_arguments([
            OsString::from("--describe"),
            OsString::from("screen.webm"),
            OsString::from("-j"),
            OsString::from("2"),
        ])
        .expect("explicit describe arguments parse");

        assert_eq!(
            parsed,
            Command::Describe(DescribeArguments {
                video_path: PathBuf::from("screen.webm"),
                journal: None,
                jobs: 2,
                redo: false,
            })
        );
        assert!(parse_arguments(["screen.webm"].map(OsString::from)).is_err());
    }

    #[test]
    fn owner_debug_and_verbose_flags_are_noops() {
        assert!(matches!(
            parse_arguments(["--describe", "screen.webm", "-d", "-v"].map(OsString::from)),
            Ok(Command::Describe(_))
        ));
    }

    #[test]
    fn describe_mode_rejects_duplicates_and_frames_only_rejects_redo() {
        let duplicate =
            parse_arguments(["--describe", "--describe", "screen.webm"].map(OsString::from));
        assert!(matches!(
            duplicate,
            Err(CliError::Usage(message)) if message.contains("--describe was provided more than once")
        ));
        let redo = parse_arguments(["--frames-only", "--redo", "screen.webm"].map(OsString::from));
        assert!(matches!(
            redo,
            Err(CliError::Usage(message)) if message.contains("--redo requires --describe")
        ));
    }

    #[test]
    fn logger_install_is_idempotent_and_defaults_to_warn() {
        super::install_logger();
        super::install_logger();
        if std::env::var("RUST_LOG").is_err() {
            assert!(log::max_level() >= log::LevelFilter::Warn);
        }
    }

    #[test]
    fn describe_deferred_message_omits_empty_tokens() {
        assert_eq!(super::describe_deferred_message(None), "describe deferred");
        assert_eq!(
            super::describe_deferred_message(Some("")),
            "describe deferred"
        );
        assert_eq!(
            super::describe_deferred_message(Some("binary_missing")),
            "describe deferred: binary_missing"
        );
    }
}
