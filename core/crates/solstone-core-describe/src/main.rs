// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::env;
use std::ffi::OsString;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde_json::Value;
use solstone_core_describe::{
    ConveyFiducialMask, WinnowConfig, pipeline, process_video_with_transform,
};
use solstone_core_journal_config::read_journal_config;

const EXIT_DECODE_FAILURE: u8 = 2;
const EXIT_USAGE: u8 = 64;
const EXIT_CONFIG: u8 = 78;
const EXIT_PROVIDER_BLOCKED: u8 = 69;

fn main() -> ExitCode {
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
        Err(CliError::Blocked) => ExitCode::from(EXIT_PROVIDER_BLOCKED),
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
        Command::FramesOnly(arguments) => {
            let config = read_config(arguments.journal.as_deref())?;
            let mut transform = ConveyFiducialMask;
            let result =
                process_video_with_transform(&arguments.video_path, &mut transform, config.winnow);
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
                jobs: arguments.jobs,
                config: config.winnow,
                redact_rules: config.redact_rules,
            })
            .map_err(|error| match error {
                pipeline::RunError::Blocked => CliError::Blocked,
                pipeline::RunError::Internal(message) => CliError::Internal(message),
            })
        }
    }
}

enum Command {
    Version,
    FramesOnly(FramesOnlyArguments),
    Describe(DescribeArguments),
}

struct FramesOnlyArguments {
    video_path: PathBuf,
    journal: Option<PathBuf>,
}
struct DescribeArguments {
    video_path: PathBuf,
    journal: Option<PathBuf>,
    jobs: usize,
}

enum CliError {
    Usage(String),
    Config(String),
    Decode(String),
    Blocked,
    Internal(String),
}

fn parse_arguments(arguments: impl IntoIterator<Item = OsString>) -> Result<Command, CliError> {
    let mut values = arguments.into_iter();
    let Some(first) = values.next() else {
        return Err(usage("missing required --frames-only and video path"));
    };
    if first == "--version" {
        if values.next().is_some() {
            return Err(usage("--version does not accept other arguments"));
        }
        return Ok(Command::Version);
    }

    let mut frames_only = false;
    let mut describe = false;
    let mut jobs = None;
    let mut journal = None;
    let mut video_path = None;
    let mut pending = Some(first);
    while let Some(argument) = pending.take().or_else(|| values.next()) {
        if argument == "--frames-only" {
            if frames_only {
                return Err(usage("--frames-only was provided more than once"));
            }
            frames_only = true;
        } else if argument == "--describe" {
            if describe {
                return Err(usage("--describe was provided more than once"));
            }
            describe = true;
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
        return Err(usage("missing video path"));
    };
    if frames_only {
        Ok(Command::FramesOnly(FramesOnlyArguments {
            video_path,
            journal,
        }))
    } else {
        Ok(Command::Describe(DescribeArguments {
            video_path,
            journal,
            jobs: jobs.unwrap_or(1),
        }))
    }
}

struct DescribeConfig {
    winnow: WinnowConfig,
    redact_rules: Vec<String>,
}

fn read_config(journal_path: Option<&Path>) -> Result<DescribeConfig, CliError> {
    let Some(journal_path) = journal_path else {
        return Ok(DescribeConfig {
            winnow: WinnowConfig::default(),
            redact_rules: Vec::new(),
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
    Ok(DescribeConfig {
        winnow: result,
        redact_rules,
    })
}

fn usage(message: &str) -> CliError {
    CliError::Usage(format!(
        "{message}\nUsage: solstone-core-describe --frames-only <video-path> [--journal <path>]\n       solstone-core-describe --describe <video-path> [-j N] [--journal <path>]\n       solstone-core-describe --version"
    ))
}
