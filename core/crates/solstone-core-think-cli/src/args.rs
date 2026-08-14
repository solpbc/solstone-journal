// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ThinkArgs {
    pub day: Option<String>,
    pub segment: Option<String>,
    pub refresh: bool,
    pub from_scratch: bool,
    pub segments: bool,
    pub facet: Option<String>,
    pub activity: Option<String>,
    pub stream: Option<String>,
    pub flush: bool,
    pub jobs: usize,
    pub no_timeout: bool,
    pub segment_workers: Option<usize>,
    pub no_activity_prompts: bool,
    pub skip_talents: String,
    pub live: bool,
    pub updated: bool,
    pub weekly: bool,
    pub cadence: bool,
    pub dry_run: bool,
    pub verbose: bool,
    pub debug: bool,
}

impl Default for ThinkArgs {
    fn default() -> Self {
        Self {
            day: None,
            segment: None,
            refresh: false,
            from_scratch: false,
            segments: false,
            facet: None,
            activity: None,
            stream: None,
            flush: false,
            jobs: 2,
            no_timeout: false,
            segment_workers: None,
            no_activity_prompts: false,
            skip_talents: String::new(),
            live: false,
            updated: false,
            weekly: false,
            cadence: false,
            dry_run: false,
            verbose: false,
            debug: false,
        }
    }
}

pub(crate) const UPDATED_INCOMPATIBLE: &str = "--updated is incompatible with ";
pub(crate) const FACET_REQUIRES_ACTIVITY: &str = "--facet requires --activity";
pub(crate) const ACTIVITY_REQUIRES_FACET: &str = "--activity requires --facet";
pub(crate) const ACTIVITY_REQUIRES_DAY: &str = "--activity requires --day";
pub(crate) const NO_ACTIVITY_PROMPTS_WITH_ACTIVITY: &str =
    "--no-activity-prompts cannot be combined with --activity";
pub(crate) const SEGMENT_WORKERS_RANGE: &str = "--segment-workers must be between 1 and 32";
pub(crate) const ACTIVITY_INCOMPATIBLE: &str =
    "--activity is incompatible with --segment, --segments, and --flush";
pub(crate) const FLUSH_REQUIRES_SEGMENT: &str = "--flush requires --segment";
pub(crate) const FLUSH_INCOMPATIBLE: &str = "--flush is incompatible with --segments and --refresh";
pub(crate) const SEGMENTS_INCOMPATIBLE: &str =
    "--segments is incompatible with --segment and --facet";
pub(crate) const WEEKLY_INCOMPATIBLE: &str =
    "--weekly is incompatible with --segment, --segments, --activity, and --flush";
pub(crate) const CADENCE_INCOMPATIBLE: &str =
    "--cadence is incompatible with --segment, --segments, --activity, --flush, and --weekly";
pub(crate) const MULTI_WORKER_UNLIMITED_JOBS: &str = "--jobs 0 is incompatible with multi-worker --segments; set --jobs to a positive bound or --segment-workers 1";

pub(crate) enum ParseOutcome {
    Help,
    Args(ThinkArgs),
}

pub(crate) fn parse(args: &[String]) -> Result<ParseOutcome, String> {
    let mut parsed = ThinkArgs::default();
    let mut index = 0;
    while let Some(argument) = args.get(index) {
        let mut value = |name: &str| {
            index += 1;
            args.get(index)
                .cloned()
                .ok_or_else(|| format!("argument {name}: expected one argument"))
        };
        match argument.as_str() {
            "--day" => parsed.day = Some(value("--day")?),
            "--segment" => parsed.segment = Some(value("--segment")?),
            "--refresh" => parsed.refresh = true,
            "--from-scratch" => parsed.from_scratch = true,
            "--segments" => parsed.segments = true,
            "--facet" => parsed.facet = Some(value("--facet")?),
            "--activity" => parsed.activity = Some(value("--activity")?),
            "--stream" => parsed.stream = Some(value("--stream")?),
            "--flush" => parsed.flush = true,
            "-j" | "--jobs" => {
                parsed.jobs = value(argument)?
                    .parse()
                    .map_err(|_| format!("argument {argument}: invalid int value"))?
            }
            "--no-timeout" => parsed.no_timeout = true,
            "--segment-workers" => {
                parsed.segment_workers = Some(
                    value("--segment-workers")?
                        .parse()
                        .map_err(|_| "argument --segment-workers: invalid int value".to_owned())?,
                )
            }
            "--no-activity-prompts" => parsed.no_activity_prompts = true,
            "--skip-talents" => parsed.skip_talents = value("--skip-talents")?,
            "--live" => parsed.live = true,
            "--updated" => parsed.updated = true,
            "--weekly" => parsed.weekly = true,
            "--cadence" => parsed.cadence = true,
            "--dry-run" => parsed.dry_run = true,
            "-v" | "--verbose" => parsed.verbose = true,
            "-d" | "--debug" => parsed.debug = true,
            "-h" | "--help" => return Ok(ParseOutcome::Help),
            unknown => return Err(format!("unrecognized arguments: {unknown}")),
        }
        index += 1;
    }
    Ok(ParseOutcome::Args(parsed))
}

pub(crate) fn updated_offenders(args: &ThinkArgs) -> Vec<&'static str> {
    let mut offenders = Vec::new();
    if args.day.is_some() {
        offenders.push("--day");
    }
    if args.segment.is_some() {
        offenders.push("--segment");
    }
    if args.facet.is_some() {
        offenders.push("--facet");
    }
    if args.activity.is_some() {
        offenders.push("--activity");
    }
    if args.flush {
        offenders.push("--flush");
    }
    if args.segments {
        offenders.push("--segments");
    }
    if args.cadence {
        offenders.push("--cadence");
    }
    offenders
}
