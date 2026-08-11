# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import json
import os
import sys

sys.path.insert(0, os.environ["SOLSTONE_REPO_ROOT"])

from solstone.think import deterministic_failure_caps, pipeline_health

day = os.environ.get("ORACLE_DAY", "20990202")
since_ms = int(os.environ.get("ORACLE_SINCE_MS", "200"))


def option_key(value):
    return (value is not None, value or "")


def terminal_states(states):
    rows = []
    for unit, state in states.items():
        rows.append(
            {
                "unit": {
                    "mode": unit.mode,
                    "name": unit.name,
                    "facet": unit.facet,
                    "stream": unit.stream,
                    "segment": unit.segment,
                    "activity": unit.activity,
                },
                "state": {
                    "latest_event": state.latest_event,
                    "latest_ts": state.latest_ts,
                    "last_real_complete_ts": state.last_real_complete_ts,
                    "trailing_fail_count": state.trailing_fail_count,
                    "deterministic_fail_count": state.deterministic_fail_count,
                    "last_fail_ts": state.last_fail_ts,
                    "use_id": state.use_id,
                    "state": state.state,
                    "reason_code": state.reason_code,
                    "provider": state.provider,
                    "model": state.model,
                    "oldest_trailing_fail_ts": state.oldest_trailing_fail_ts,
                },
            }
        )
    return sorted(
        rows,
        key=lambda row: (
            row["unit"]["mode"],
            row["unit"]["name"],
            option_key(row["unit"]["facet"]),
            option_key(row["unit"]["stream"]),
            option_key(row["unit"]["segment"]),
            option_key(row["unit"]["activity"]),
        ),
    )


def completed_units(units):
    return sorted(
        [{"mode": mode, "name": name, "facet": facet} for mode, name, facet in units],
        key=lambda item: (item["mode"], item["name"], option_key(item["facet"])),
    )


def daily_failures(failures):
    return sorted(
        [
            {
                "name": name,
                "facet": facet,
                "count": failure.count,
                "reason_code": failure.reason_code,
            }
            for (name, facet), failure in failures.items()
        ],
        key=lambda item: (item["name"], option_key(item["facet"])),
    )


def segment_progress(progress):
    return sorted(
        [
            {
                "stream": stream,
                "segment": segment,
                "sensed": value.sensed,
                "density": value.density,
                "change_class": value.change_class,
                "dispatched": sorted(value.dispatched),
                "completed": sorted(value.completed),
                "unconfigured": sorted(value.unconfigured),
                "capped_by_skip": sorted(value.capped),
            }
            for (stream, segment), value in progress.items()
        ],
        key=lambda item: (option_key(item["stream"]), item["segment"]),
    )


states = pipeline_health.read_terminal_states(day)
completed_since = pipeline_health.read_completed_since(day, since_ms)
progress = pipeline_health.read_segment_progress(day)
segments = [
    {"key": "progress", "stream": "default", "data_state": {"screen": "analyzed"}},
    {"key": "not-sensed", "stream": "default", "data_state": {"screen": "pending"}},
    {"key": "browser-only", "stream": "default", "data_state": {"browser": "analyzed"}},
]
completion = pipeline_health.classify_segment_completion(segments, progress)
thought_ok, thought_reason = pipeline_health.segment_fully_thought(
    pipeline_health.lookup_segment_progress(progress, "default", "progress")
)
print(
    json.dumps(
        {
            "terminal_states": terminal_states(states),
            "completed_units": completed_units(pipeline_health.read_completed_units(day)),
            "completed_since": {
                "segments": list(completed_since.segments),
                "activities": list(completed_since.activities),
            },
            "daily_deterministic_failures": daily_failures(
                pipeline_health.read_daily_deterministic_failures(day)
            ),
            "segment_progress": segment_progress(progress),
            "floor_caps": {
                "cap_true": pipeline_health.is_floor_talent_capped(
                    day, "default", "cap-true", "documents"
                ),
                "cap_short": pipeline_health.is_floor_talent_capped(
                    day, "default", "cap-short", "documents"
                ),
            },
            "pure_completion": {
                "fully_sensed": [
                    pipeline_health.segment_fully_sensed(segment["data_state"])
                    for segment in segments
                ],
                "requires_processing": [
                    pipeline_health.segment_requires_processing(segment)
                    for segment in segments
                ],
                "thought": [thought_ok, thought_reason],
                "lookup_found": pipeline_health.lookup_segment_progress(
                    progress, "default", "progress"
                )
                is not None,
                "classification": {
                    "blockers": list(completion.blockers),
                    "not_sensed": completion.not_sensed,
                    "not_thought": completion.not_thought,
                    "total": completion.total,
                    "capped": completion.capped,
                    "exhausted": list(completion.exhausted),
                },
                "blocked": [
                    {"stream": stream, "segment": segment}
                    for stream, segment in sorted(
                        pipeline_health.blocked_segment_keys(segments, progress),
                        key=lambda item: (option_key(item[0]), item[1]),
                    )
                ],
            },
            "floor": list(pipeline_health.SEGMENT_FLOOR_TALENTS),
            "nongating": list(pipeline_health.SEGMENT_NONGATING_TALENTS),
            "superseded": pipeline_health.SEGMENT_SUPERSEDED_TALENTS,
            "no_processing": sorted(pipeline_health.SEGMENT_NO_PROCESSING_MODALITIES),
            "cap": pipeline_health.CAP,
            "min_span_ms": pipeline_health.MIN_SPAN_MS,
            "deterministic": sorted(
                deterministic_failure_caps.DETERMINISTIC_FAILURE_REASON_CODES
            ),
        },
        sort_keys=True,
    )
)
