// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use serde_json::Value;
use solstone_core_cli_copy as copy;
use solstone_core_segment::{
    DEFAULT_STREAM, LockOptions, Relocation, RelocationOutcome, RelocationRefusal, RepairOutcome,
    available_segment_key, read_text, relocate_segment_for, touch_stream_health_marker,
};

use crate::index::{SegmentIndexStatus, read_segment_index};
use crate::location::SegmentLocation;
use crate::read::{checks_in, read_marker, render_checks, successors};

#[derive(Debug)]
pub(crate) struct MovePlan {
    pub(crate) source: SegmentLocation,
    pub(crate) destination: SegmentLocation,
    pub(crate) marker: Value,
    pub(crate) marker_stream: String,
    pub(crate) successors: Vec<SegmentLocation>,
    pub(crate) events: u64,
    pub(crate) index: SegmentIndexStatus,
}

#[derive(Debug)]
pub(crate) enum MoveRefusal {
    Message(String),
}

impl MoveRefusal {
    pub(crate) fn message(&self) -> &str {
        match self {
            Self::Message(message) => message,
        }
    }
}

/// The seam this crate orchestrates over.
///
/// The four durable journal writes of a move are one call, owned by the segment
/// write door. What remains here is index work, which this crate owns outright.
pub(crate) trait SegmentOperations {
    fn relocate(&self, relocation: &Relocation<'_>)
    -> Result<RelocationOutcome, RelocationRefusal>;
    fn enqueue_path(&self, journal: &Path, path: &Path) -> Result<(), String>;
    fn touch_health(&self, journal: &Path, day: &str) -> Result<(), String>;
}

pub(crate) struct NativeOperations;

impl SegmentOperations for NativeOperations {
    fn relocate(
        &self,
        relocation: &Relocation<'_>,
    ) -> Result<RelocationOutcome, RelocationRefusal> {
        relocate_segment_for(relocation, copy::cli_locale())
    }

    fn enqueue_path(&self, journal: &Path, path: &Path) -> Result<(), String> {
        solstone_core_indexer_store::todo::enqueue_path(journal, path)
            .map_err(|error| error.to_string())
    }

    fn touch_health(&self, journal: &Path, day: &str) -> Result<(), String> {
        touch_stream_health_marker(journal, day).map_err(|error| error.to_string())
    }
}

pub(crate) fn build_plan(
    journal: &Path,
    source: SegmentLocation,
    to_day: &str,
    to_time: Option<&str>,
) -> Result<MovePlan, MoveRefusal> {
    let locale = copy::cli_locale();
    if !to_day.bytes().all(|byte| byte.is_ascii_digit()) || to_day.len() != 8 {
        return Err(MoveRefusal::Message(copy::display_for(
            locale,
            "journal.cli.segment.move.day.format",
            [("to_day", &to_day)],
        )));
    }
    let mut destination_segment = source.segment.clone();
    if let Some(time) = to_time {
        if time.len() != 6 || !time.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(MoveRefusal::Message(copy::display_for(
                locale,
                "journal.cli.segment.move.time.format",
                [("time", &time)],
            )));
        }
        let Some((_, duration)) = source.segment.split_once('_') else {
            return Err(MoveRefusal::Message(copy::render_for(
                locale,
                "journal.cli.segment.move.no.duration",
                &[],
            )));
        };
        destination_segment = format!("{time}_{duration}");
    }
    if source.day == to_day && source.segment == destination_segment {
        return Err(MoveRefusal::Message(copy::render_for(
            locale,
            "journal.cli.segment.move.same",
            &[],
        )));
    }
    let mut destination =
        SegmentLocation::resolve(journal, to_day, &source.stream, &destination_segment)
            .map_err(|error| MoveRefusal::Message(error.to_string()))?;
    if destination.path.exists() {
        if to_time.is_none() {
            return Err(MoveRefusal::Message(copy::display_for(
                locale,
                "journal.cli.segment.move.exists",
                [
                    ("segment", &destination_segment),
                    ("day", &to_day),
                    ("stream", &source.stream),
                ],
            )));
        }
        let parent = journal.join(&destination.parent_rel);
        let available = available_segment_key(&parent, &destination_segment, 100)
            .map_err(|error| MoveRefusal::Message(error.to_string()))?;
        let Some(available) = available else {
            return Err(MoveRefusal::Message(copy::display_for(
                locale,
                "journal.cli.segment.move.no.slot",
                [
                    ("destination_segment", &destination_segment),
                    ("to_day", &to_day),
                ],
            )));
        };
        destination = SegmentLocation::resolve(journal, to_day, &source.stream, &available)
            .map_err(|error| MoveRefusal::Message(error.to_string()))?;
    }
    let Some(marker) = read_marker(&source.path).map_err(|_| {
        MoveRefusal::Message(copy::render_for(
            locale,
            "journal.cli.segment.move.no.marker",
            &[],
        ))
    })?
    else {
        return Err(MoveRefusal::Message(copy::render_for(
            locale,
            "journal.cli.segment.move.no.marker",
            &[],
        )));
    };
    let marker_stream = marker
        .get("stream")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            MoveRefusal::Message(copy::display_for(
                locale,
                "journal.cli.segment.move.stream.mismatch",
                [
                    ("path_stream", &source.stream),
                    (
                        "marker_stream",
                        &(format_args!("{:?}", marker.get("stream"))),
                    ),
                ],
            ))
        })?
        .to_owned();
    if source.stream != DEFAULT_STREAM && marker_stream != source.stream {
        return Err(MoveRefusal::Message(copy::display_for(
            locale,
            "journal.cli.segment.move.stream.mismatch",
            [
                ("path_stream", &source.stream),
                ("marker_stream", &marker_stream),
            ],
        )));
    }
    let events = read_text(source.path.join("events.jsonl"), String::new())
        .unwrap_or_default()
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count() as u64;
    let index = read_segment_index(journal, &source.index_rel);
    let successors = successors(
        journal,
        &source.day,
        &marker_stream,
        &source.segment,
        locale,
    )
    .map_err(MoveRefusal::Message)?;
    Ok(MovePlan {
        source,
        destination,
        marker,
        marker_stream,
        successors,
        events,
        index,
    })
}

pub(crate) fn render_plan(plan: &MovePlan) -> String {
    let locale = copy::cli_locale();
    let mut output = format!(
        "{}\n  {}\n",
        copy::display_for(
            locale,
            "journal.cli.segment.move.plan",
            [
                ("source", &(plan.source.token())),
                ("destination", &(plan.destination.token()))
            ]
        ),
        copy::display_for(
            locale,
            "journal.cli.segment.move.plan.events",
            [("count", &plan.events)]
        )
    );
    match plan.successors.as_slice() {
        [] => output.push_str(&format!(
            "  {}\n",
            copy::render_for(locale, "journal.cli.segment.move.plan.tail", &[])
        )),
        [successor] => output.push_str(&format!(
            "  {}\n",
            copy::display_for(
                locale,
                "journal.cli.segment.move.plan.successor",
                [("path", &(successor.token()))]
            )
        )),
        many => output.push_str(&format!("  {}\n", {
            let count = i64::try_from(many.len()).unwrap_or(i64::MAX);
            copy::render_for(
                locale,
                "journal.cli.segment.move.plan.ambiguous",
                &[
                    ("quantity", copy::Value::Cardinal(count)),
                    ("count", copy::Value::Integer(count)),
                ],
            )
        })),
    }
    match &plan.index {
        SegmentIndexStatus::Ready { chunks, .. } => output.push_str(&format!(
            "  {}\n",
            copy::display_for(
                locale,
                "journal.cli.segment.move.plan.index",
                [("chunks", &chunks)]
            )
        )),
        SegmentIndexStatus::Unreadable { error } => output.push_str(&format!(
            "  {}\n",
            copy::display_for(
                locale,
                "journal.cli.segment.move.plan.index.error",
                [("error", &error)]
            )
        )),
        SegmentIndexStatus::Absent => {}
    }
    output.push_str(&format!(
        "  {}\n",
        copy::display_for(
            locale,
            "journal.cli.segment.move.plan.markers",
            [
                ("source_day", &plan.source.day),
                ("destination_day", &plan.destination.day)
            ]
        )
    ));
    output
}

pub(crate) struct MoveExecution {
    pub(crate) stdout: String,
    pub(crate) stderr: String,
    pub(crate) exit_code: i32,
}

pub(crate) fn execute_plan(
    journal: &Path,
    plan: &MovePlan,
    verbose: bool,
    locks: LockOptions,
    operations: &dyn SegmentOperations,
) -> MoveExecution {
    let locale = copy::cli_locale();
    let mut stdout = format!(
        "\n{}\n",
        copy::render_for(locale, "journal.cli.segment.move.running", &[])
    );
    let mut stderr = String::new();
    let source = plan.source.relocation_end();
    let destination = plan.destination.relocation_end();
    // An ambiguous chain is a planning verdict, not a write: hand the door only
    // the successor it should patch and report the ambiguity here.
    let successor = match plan.successors.as_slice() {
        [successor] => Some(successor.relocation_end()),
        _ => None,
    };
    let outcome = match operations.relocate(&Relocation {
        journal,
        source: &source,
        destination: &destination,
        successor: successor.as_ref(),
        stream: &plan.marker_stream,
        sequence: plan.marker.get("seq").and_then(Value::as_u64).unwrap_or(0),
        locks,
    }) {
        Ok(outcome) => outcome,
        Err(RelocationRefusal::DestinationExists) => {
            return MoveExecution {
                stdout,
                stderr: format!(
                    "{}\n",
                    copy::display_for(
                        locale,
                        "journal.cli.segment.move.directory.exists",
                        [("destination", &(plan.destination.token()))]
                    )
                ),
                exit_code: 1,
            };
        }
        Err(RelocationRefusal::Failed(error)) => {
            return MoveExecution {
                stdout,
                stderr: format!(
                    "{}\n",
                    copy::display_for(
                        locale,
                        "journal.cli.segment.move.directory.failed",
                        [("error", &error)]
                    )
                ),
                exit_code: 3,
            };
        }
    };
    if verbose {
        stdout.push_str(&format!(
            "  {}\n",
            copy::display_for(
                locale,
                "journal.cli.segment.move.directory.created",
                [(
                    "path",
                    &(journal.join(&plan.destination.parent_rel).display())
                )]
            )
        ));
    }
    stdout.push_str(&format!(
        "  {}\n",
        copy::display_for(
            locale,
            "journal.cli.segment.move.directory.moved",
            [
                ("source", &plan.source.segment),
                ("destination", &plan.destination.segment)
            ]
        )
    ));
    let mut failures = PostMoveFailures::default();
    match outcome.events {
        Ok(count) if count > 0 => stdout.push_str(&format!(
            "  {}\n",
            copy::render_for(
                locale,
                "journal.cli.segment.move.events.rewritten",
                &[
                    (
                        "quantity",
                        copy::Value::Cardinal(i64::try_from(count).unwrap_or(i64::MAX))
                    ),
                    ("count", copy::Value::String(&count.to_string())),
                    (
                        "source_day",
                        copy::Value::String(&plan.source.day.to_string())
                    ),
                    (
                        "destination_day",
                        copy::Value::String(&plan.destination.day.to_string())
                    ),
                    (
                        "source",
                        copy::Value::String(&plan.source.segment.to_string())
                    ),
                    (
                        "destination",
                        copy::Value::String(&plan.destination.segment.to_string())
                    )
                ]
            )
        )),
        Ok(_) if verbose => stdout.push_str(&format!(
            "  {}\n",
            copy::render_for(locale, "journal.cli.segment.move.events.none", &[])
        )),
        Ok(_) => {}
        Err(error) => failures.push(
            &mut stderr,
            copy::display_for(
                locale,
                "journal.cli.segment.move.fail.events",
                [("detail", &error)],
            ),
        ),
    }
    match plan.successors.as_slice() {
        [] if verbose => stdout.push_str(&format!(
            "  {}\n",
            copy::render_for(locale, "journal.cli.segment.move.successor.none", &[])
        )),
        [] => {}
        [successor] => match outcome.successor {
            Some(Err(error)) => failures.push(
                &mut stderr,
                copy::display_for(
                    locale,
                    "journal.cli.segment.move.fail.successor",
                    [("detail", &error)],
                ),
            ),
            _ => stdout.push_str(&format!(
                "  {}\n",
                copy::display_for(
                    locale,
                    "journal.cli.segment.move.successor.patched",
                    [("path", &(successor.token()))]
                )
            )),
        },
        successors => failures.push(&mut stderr, {
            let count = i64::try_from(successors.len()).unwrap_or(i64::MAX);
            copy::render_for(
                locale,
                "journal.cli.segment.move.fail.ambiguous",
                &[
                    ("quantity", copy::Value::Cardinal(count)),
                    ("count", copy::Value::Integer(count)),
                ],
            )
        }),
    }
    match outcome.tail {
        RepairOutcome::Repaired | RepairOutcome::Unchanged(_) => stdout.push_str(&format!(
            "  {}\n",
            copy::display_for(
                locale,
                "journal.cli.segment.move.stream.rebuilt",
                [("stream", &plan.marker_stream)]
            )
        )),
        RepairOutcome::NoRecord => failures.push(
            &mut stderr,
            copy::render_for(locale, "journal.cli.segment.move.fail.tail.no.record", &[]),
        ),
        RepairOutcome::Malformed => failures.push(
            &mut stderr,
            copy::render_for(locale, "journal.cli.segment.move.fail.tail.malformed", &[]),
        ),
        RepairOutcome::Locked => failures.push(
            &mut stderr,
            copy::render_for(locale, "journal.cli.segment.move.fail.tail.locked", &[]),
        ),
        RepairOutcome::WriteFailed => failures.push(
            &mut stderr,
            copy::render_for(locale, "journal.cli.segment.move.fail.tail.write", &[]),
        ),
    }
    let mut search_queued = true;
    for rel in [&plan.source.index_rel, &plan.destination.index_rel] {
        if let Err(error) = operations.enqueue_path(journal, Path::new(rel)) {
            search_queued = false;
            failures.push(
                &mut stderr,
                copy::display_for(
                    locale,
                    "journal.cli.segment.move.fail.search",
                    [("error", &error)],
                ),
            );
        }
    }
    if search_queued {
        stdout.push_str(&format!(
            "  {}\n",
            copy::render_for(locale, "journal.cli.segment.move.search.queued", &[])
        ));
    }
    let mutated_days = outcome.mutated_days;
    for day in &mutated_days {
        if let Err(error) = operations.touch_health(journal, day) {
            failures.push(
                &mut stderr,
                copy::display_for(
                    locale,
                    "journal.cli.segment.move.fail.health.marker",
                    [("detail", &error)],
                ),
            );
        }
    }
    stdout.push_str(&format!(
        "  {}\n",
        copy::display_for(
            locale,
            "journal.cli.segment.move.markers.touched",
            [(
                "days",
                &(mutated_days.iter().cloned().collect::<Vec<_>>().join(", "))
            )]
        )
    ));
    if verbose {
        stdout.push_str(&format!(
            "    {}\n",
            copy::render_for(locale, "journal.cli.segment.move.talents", &[])
        ));
    }
    let results = checks_in(journal, &plan.destination, locale);
    stdout.push('\n');
    stdout.push_str(&render_checks(&results, locale));
    let passed = results.iter().filter(|check| check.passed).count();
    stdout.push_str(&format!(
        "\n{}\n",
        crate::check_summary(locale, passed, results.len(), false)
    ));
    MoveExecution {
        stdout,
        stderr,
        exit_code: if failures.any { 3 } else { 0 },
    }
}

#[derive(Default)]
struct PostMoveFailures {
    any: bool,
}

impl PostMoveFailures {
    fn push(&mut self, stderr: &mut String, message: String) {
        self.any = true;
        stderr.push_str(&message);
        stderr.push('\n');
    }
}
