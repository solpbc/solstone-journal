// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! One historical segment, without activity or daily work.

use std::time::Duration;

use crate::{
    args::ThinkArgs, context::ThinkContext, dispatch::ModeResult, run_log::RunLogWriter, segment,
};

pub(crate) fn run(
    context: &ThinkContext,
    log: &mut RunLogWriter,
    args: &ThinkArgs,
    timeout: Option<Duration>,
    child_environment: &solstone_core_system::process::ChildLaunchContext,
) -> Result<ModeResult, crate::dispatch::RunFailure> {
    let segment = args.segment.as_deref().ok_or_else(|| {
        context.failure(|locale| {
            solstone_core_cli_copy::render_for(
                locale,
                "journal.cli.think.repair.segment.missing",
                &[],
            )
        })
    })?;
    let request = solstone_core_sense::batch::BatchRequest {
        day: context.day.clone(),
        jobs: args.jobs,
        reprocess: None,
        segment: Some(segment.to_owned()),
        stream: args.stream.clone(),
        dry_run: false,
        verbose: args.verbose,
        debug: args.debug,
    };
    if let Some(executor) = &context.sense_executor {
        solstone_core_sense::batch::run_batch_with_executor(
            &context.journal,
            &request,
            child_environment,
            Some(Duration::from_secs(1800)),
            std::sync::Arc::clone(executor),
            solstone_core_sense::dispatch::BatchMarkerPolicy::AdvanceStream,
        )
    } else {
        solstone_core_sense::batch::run_batch_in(
            context.terminal_locale,
            &context.journal,
            &request,
            child_environment,
            Some(Duration::from_secs(1800)),
            solstone_core_sense::dispatch::BatchMarkerPolicy::AdvanceStream,
        )
    }
    .map_err(|error| context.failure(|locale| error.render(locale)))?;
    if (context.cancel_checkpoint)() {
        return Err(context.failure(|locale| {
            solstone_core_cli_copy::render_for(locale, "journal.cli.think.cancelled", &[])
        }));
    }
    // Thinking that is already current is skipped, so a segment owed only its
    // publication is published without a model call.
    let mut result = segment::run_with(
        context,
        log,
        segment,
        false,
        args.stream.as_deref(),
        args.jobs,
        timeout,
        false,
        &[],
        segment::CurrentSegment::Skip,
    )?;
    if result.failed == 0 && !result.timed_out {
        let now = jiff::Timestamp::from_millisecond(context.now_ms)
            .unwrap_or_else(|_| jiff::Timestamp::now());
        let today = solstone_core_system::daily_coverage::local_day(&context.journal, now);
        if context.day < today
            && matches!(
                solstone_core_system_health::day_segments_ready_typed(
                    &context.journal,
                    &context.day
                ),
                Ok(true)
            )
            && let Err(error) = segment::ensure_historical_day_activities(
                context,
                log,
                args.jobs,
                args.no_activity_prompts,
            )
        {
            crate::dispatch::record_followup_failure(&mut result, "activity replay", &error);
        }
    }
    Ok(result)
}
