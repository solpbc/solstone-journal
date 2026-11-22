// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ThinkArgs {
    pub terminal_locale: Option<&'static str>,
    pub day: Option<String>,
    pub segment: Option<String>,
    pub repair_segment: bool,
    pub daily_only: bool,
    pub work_intent: Option<String>,
    pub retry_unit: Option<String>,
    pub retry_cause: Option<String>,
    pub refresh: bool,
    pub from_scratch: bool,
    pub segments: bool,
    pub facet: Option<String>,
    pub activity: Option<String>,
    pub stream: Option<String>,
    pub flush: bool,
    pub settle: bool,
    pub jobs: i64,
    pub no_timeout: bool,
    pub segment_workers: Option<i64>,
    pub no_activity_prompts: bool,
    pub skip_talents: String,
    pub live: bool,
    pub updated: bool,
    pub weekly: bool,
    pub cadence: bool,
    pub dry_run: bool,
    pub sense_batch: bool,
    pub reactivate: bool,
    pub verbose: bool,
    pub debug: bool,
}

impl Default for ThinkArgs {
    fn default() -> Self {
        Self {
            terminal_locale: None,
            day: None,
            segment: None,
            repair_segment: false,
            daily_only: false,
            work_intent: None,
            retry_unit: None,
            retry_cause: None,
            refresh: false,
            from_scratch: false,
            segments: false,
            facet: None,
            activity: None,
            stream: None,
            flush: false,
            settle: false,
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
            sense_batch: false,
            reactivate: false,
            verbose: false,
            debug: false,
        }
    }
}

#[cfg(test)]
pub(crate) const REACTIVATE_INCOMPATIBLE_REFRESH: &str =
    "--reactivate is incompatible with --refresh";
#[cfg(test)]
pub(crate) const REACTIVATE_REQUIRES_ACTIVITY: &str = "--reactivate requires --activity";

pub(crate) enum ParseOutcome {
    Help,
    Args(Box<ThinkArgs>),
}

pub(crate) fn parse(args: &[String]) -> Result<ParseOutcome, String> {
    parse_in(args, None)
}

pub(crate) fn parse_in(
    args: &[String],
    terminal_locale: Option<&'static str>,
) -> Result<ParseOutcome, String> {
    let locale = terminal_locale.unwrap_or(solstone_core_cli_copy::ENGLISH);
    let mut parsed = ThinkArgs {
        terminal_locale,
        ..ThinkArgs::default()
    };
    let mut index = 0;
    while let Some(argument) = args.get(index) {
        let mut value = |name: &str| {
            index += 1;
            args.get(index).cloned().ok_or_else(|| {
                solstone_core_cli_copy::display_for(
                    locale,
                    "journal.cli.think.argument.value",
                    [("option", &(name))],
                )
            })
        };
        match argument.as_str() {
            "--day" => parsed.day = Some(value("--day")?),
            "--segment" => parsed.segment = Some(value("--segment")?),
            "--repair-segment" => parsed.repair_segment = true,
            "--daily-only" => parsed.daily_only = true,
            "--work-intent" => parsed.work_intent = Some(value("--work-intent")?),
            "--retry-unit" => parsed.retry_unit = Some(value("--retry-unit")?),
            "--retry-cause" => parsed.retry_cause = Some(value("--retry-cause")?),
            "--refresh" => parsed.refresh = true,
            "--from-scratch" => parsed.from_scratch = true,
            "--segments" => parsed.segments = true,
            "--facet" => parsed.facet = Some(value("--facet")?),
            "--activity" => parsed.activity = Some(value("--activity")?),
            "--reactivate" => parsed.reactivate = true,
            "--stream" => parsed.stream = Some(value("--stream")?),
            "--flush" => parsed.flush = true,
            "--settle" => parsed.settle = true,
            "-j" | "--jobs" => {
                parsed.jobs = value(argument)?.parse().map_err(|_| {
                    solstone_core_cli_copy::display_for(
                        locale,
                        "journal.cli.think.argument.integer",
                        [("option", &(argument))],
                    )
                })?
            }
            "--no-timeout" => parsed.no_timeout = true,
            "--segment-workers" => {
                parsed.segment_workers =
                    Some(value("--segment-workers")?.parse().map_err(|_| {
                        solstone_core_cli_copy::display_for(
                            locale,
                            "journal.cli.think.argument.integer",
                            [("option", &argument)],
                        )
                    })?)
            }
            "--no-activity-prompts" => parsed.no_activity_prompts = true,
            "--skip-talents" => parsed.skip_talents = value("--skip-talents")?,
            "--live" => parsed.live = true,
            "--updated" => parsed.updated = true,
            "--weekly" => parsed.weekly = true,
            "--cadence" => parsed.cadence = true,
            "--dry-run" => parsed.dry_run = true,
            "--sense-batch" => parsed.sense_batch = true,
            "-v" | "--verbose" => parsed.verbose = true,
            "-d" | "--debug" => parsed.debug = true,
            "-h" | "--help" => return Ok(ParseOutcome::Help),
            unknown => {
                return Err(solstone_core_cli_copy::display_for(
                    locale,
                    "journal.cli.think.argument.unknown",
                    [("argument", &(unknown))],
                ));
            }
        }
        index += 1;
    }
    Ok(ParseOutcome::Args(Box::new(parsed)))
}

pub(crate) fn sense_batch_offenders(args: &ThinkArgs) -> Vec<&'static str> {
    let mut offenders = Vec::new();
    if args.segment.is_some() {
        offenders.push("--segment");
    }
    if args.segments {
        offenders.push("--segments");
    }
    if args.flush {
        offenders.push("--flush");
    }
    if args.weekly {
        offenders.push("--weekly");
    }
    if args.cadence {
        offenders.push("--cadence");
    }
    if args.activity.is_some() {
        offenders.push("--activity");
    }
    if args.updated {
        offenders.push("--updated");
    }
    if args.dry_run {
        offenders.push("--dry-run");
    }
    offenders
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
    if args.sense_batch {
        offenders.push("--sense-batch");
    }
    offenders
}
