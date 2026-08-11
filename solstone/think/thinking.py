# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Unified think execution pipeline for solstone.

Segment-scheduled agents use the Sense-first linear orchestrator:
Sense runs first, then remaining agents dispatch based on Sense output.

Daily-scheduled agents use priority-group iteration: grouped by priority,
each group runs in parallel with bounded concurrency.
"""

import argparse
import fnmatch
import json
import logging
import subprocess
import sys
import threading
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import dataclass
from datetime import date, datetime, timedelta, timezone
from pathlib import Path
from typing import Any

from solstone.think import admission
from solstone.think.activities import (
    append_activity_record,
    get_activity_output_path,
    get_activity_record,
    load_activity_records,
)
from solstone.think.activity_state_machine import ActivityStateMachine
from solstone.think.callosum import CallosumConnection
from solstone.think.catchup_state import (
    record_daily_catchup_progress,
    record_segment_repair_attempt,
    record_segment_repair_outcome,
)
from solstone.think.change_detection import detect_segment_change, resolve_predecessor
from solstone.think.cluster import cluster_segments, read_segment_data_state
from solstone.think.cortex_client import (
    PATIENT_CLAIM_WINDOWS,
    CortexNotClaimed,
    CortexSpawnUnavailable,
    cortex_request,
    get_use_log_status,
    read_use_finish_fields,
    read_use_provider_model_reason,
    wait_for_uses,
)
from solstone.think.data_state import DataState
from solstone.think.deterministic_failure_caps import failure_capped
from solstone.think.facets import (
    get_active_facets,
    get_enabled_facets,
    load_segment_facets,
)
from solstone.think.journal_io import atomic_replace
from solstone.think.models import is_local_provider_needed
from solstone.think.pipeline_health import (
    SEGMENT_FLOOR_TALENTS,
    DeterministicFailure,
    blocked_segment_keys,
    classify_segment_completion,
    is_floor_talent_capped,
    lookup_segment_progress,
    read_completed_since,
    read_completed_units,
    read_daily_deterministic_failures,
    read_segment_progress,
    segment_fully_sensed,
    segment_fully_thought,
    segment_requires_processing,
)
from solstone.think.providers import fanout_policy
from solstone.think.runner import DEFAULT_TASK_MAX_RUNTIME, run_task
from solstone.think.sense_splitter import (
    write_change_detection,
    write_idle_stubs,
    write_sense_outputs,
)
from solstone.think.streams import is_import_stream
from solstone.think.talent import get_output_path, get_talent_configs
from solstone.think.talent_provenance import (
    compute_activity_input_hash,
    prune_orphan_provenance,
    read_activity_provenance,
    write_activity_provenance,
)
from solstone.think.talents import check_segment_has_no_input
from solstone.think.utils import (
    day_input_summary,
    day_log,
    day_path,
    get_journal,
    get_owner_timezone,
    get_rev,
    iso_date,
    iter_segments,
    now_ms,
    require_solstone,
    setup_cli,
    sunday_of_week,
    updated_days,
)

# Module-level callosum connection for event emission
_callosum: CallosumConnection | None = None
# Status tracking for periodic status emission
_status: dict = {}
_status_lock = threading.Lock()
_stop_status = threading.Event()


class ThinkingJSONLWriter:
    """Write JSONL events to a file. File-only, fail-silent."""

    def __init__(self, path: str | None = None) -> None:
        self.file = None
        self.skip_count = 0
        self.lock = threading.Lock()
        if path:
            try:
                Path(path).parent.mkdir(parents=True, exist_ok=True)
                self.file = open(path, "a", encoding="utf-8")
            except OSError as exc:
                logging.warning("Failed to open think JSONL sidecar %s: %s", path, exc)

    def log(self, event: str, **fields) -> None:
        if not self.file:
            return
        data = {"event": event, "ts": now_ms(), **fields}
        with self.lock:
            if event == "talent.skip":
                self.skip_count += 1
            try:
                self.file.write(json.dumps(data, ensure_ascii=False) + "\n")
                self.file.flush()
            except OSError as exc:
                logging.warning(
                    "Failed to write think JSONL sidecar %s: %s", self.file.name, exc
                )

    def close(self) -> None:
        if self.file:
            with self.lock:
                try:
                    self.file.close()
                except OSError as exc:
                    logging.warning(
                        "Failed to close think JSONL sidecar %s: %s",
                        self.file.name,
                        exc,
                    )


_jsonl: ThinkingJSONLWriter | None = None
SEGMENT_WORKERS_MAX = 32
# ~10 min wall-clock budget for the journal-stats post-phase
JOURNAL_STATS_MAX_RUNTIME = 600


def _jsonl_log(event: str, **fields) -> None:
    """Write a JSONL event if the writer is active."""
    if _jsonl:
        _jsonl.log(event, **fields)


def load_cadence_state() -> dict[str, int]:
    """Read health/cadence.json per-talent last-run timestamps."""
    path = Path(get_journal()) / "health" / "cadence.json"
    if not path.exists():
        return {}
    try:
        with path.open("r", encoding="utf-8") as handle:
            data = json.load(handle)
        return data if isinstance(data, dict) else {}
    except (json.JSONDecodeError, OSError) as exc:
        logging.warning("Failed to load cadence state: %s", exc)
        return {}


def save_cadence_state(state: dict[str, int]) -> None:
    """Persist cadence state to health/cadence.json atomically."""
    path = Path(get_journal()) / "health" / "cadence.json"
    atomic_replace(path, json.dumps(state, indent=2))


def _provider_model_fields(use_id: str) -> dict[str, str | None]:
    provider, model, reason_code = read_use_provider_model_reason(use_id)
    fields = {"provider": provider, "model": model}
    if reason_code:
        fields["reason_code"] = reason_code
    return fields


def _classify_timeout_state(use_id: str) -> str:
    """Classify a timed-out use as ``request_lost`` or ``timeout``.

    On a ``wait_for_uses`` timeout, a use has no durable terminal event —
    ``_recover_completed_from_disk`` folds any use with a durable finish/error
    event into ``completed`` before the timeout is reported. So a still-timed-out
    use's durable log is either absent (the Callosum request was never claimed by
    cortex — the request was lost) or present-but-non-terminal (the talent genuinely
    ran past the deadline).

    Returns ``"request_lost"`` when no durable use file exists
    (``get_use_log_status`` is ``"not_found"``), otherwise ``"timeout"``.

    ``request_lost`` strictly means "no durable claim existed at classification
    time." A request can rarely be claimed *after* the wait deadline (late bus
    delivery), producing a ``request_lost`` record followed by a later completion;
    the terminal-state fold on the summary side reconciles that. The state name is
    about attributability, not a guarantee the work never ran.
    """
    return "request_lost" if get_use_log_status(use_id) == "not_found" else "timeout"


def _cache_fields(use_id: str) -> dict[str, bool | int | None]:
    fields = read_use_finish_fields(use_id)
    return {
        "output_changed": fields["output_changed"],
        "cache_hit": fields["cache_hit"],
        "completed_at_ms": fields["completed_at_ms"],
    }


def _cache_terminal_fields(
    fields: dict[str, bool | int | None],
) -> dict[str, bool | int | None]:
    return {
        "cache_hit": fields["cache_hit"],
        "completed_at_ms": fields["completed_at_ms"],
    }


def _maybe_rescan_output(
    output_path: Path, output_changed: bool | int | None, day: str
) -> None:
    if output_changed is True and output_path.exists():
        logging.debug("Indexing %s", output_path)
        run_queued_command(
            ["journal", "indexer", "--rescan-file", str(output_path)],
            day,
            timeout=60,
        )


def _activity_input_changed(
    routing_day: str,
    facet: str,
    activity_id: str,
    record: dict,
) -> tuple[bool, str | None]:
    try:
        input_hash = compute_activity_input_hash(routing_day, record)
    except Exception:
        logging.warning(
            "Failed to compute activity input hash for %s/%s",
            facet,
            activity_id,
            exc_info=True,
        )
        return True, None
    stored_hash = read_activity_provenance(routing_day, facet, activity_id)
    return stored_hash != input_hash, input_hash


def _persist_and_maybe_run_activity_prompts(
    *,
    routing_day: str,
    log_day: str,
    segment: str,
    target_schedule: str,
    ended_triples: list[tuple[object, object, object]],
    completed: list[dict],
    refresh: bool,
    verbose: bool,
    max_concurrency: int,
    skip_activity_prompts: bool,
) -> None:
    completed_by_key: dict[tuple[str, str], dict] = {}
    for rec in completed:
        completed_by_key.setdefault((str(rec["facet"]), str(rec["id"])), rec)

    written_by: dict[tuple[str, str], bool] = {}
    record_by: dict[tuple[str, str], dict] = {}

    for activity_id, facet, change in ended_triples:
        activity_id_str = str(activity_id)
        facet_str = str(facet)
        key = (facet_str, activity_id_str)
        _jsonl_log(
            "activity.detected",
            mode=target_schedule,
            day=log_day,
            segment=segment,
            activity=activity_id_str,
            facet=facet_str,
            state="ended",
            change=change,
        )
        rec = completed_by_key.get(key)
        if rec:
            record_by[key] = rec
            written_by[key] = append_activity_record(facet_str, routing_day, rec)
            _jsonl_log(
                "activity.persisted",
                mode=target_schedule,
                day=log_day,
                segment=segment,
                activity=activity_id_str,
                facet=facet_str,
                change=change,
            )

    for activity_id, facet, _change in ended_triples:
        activity_id_str = str(activity_id)
        facet_str = str(facet)
        key = (facet_str, activity_id_str)
        if skip_activity_prompts:
            _jsonl_log(
                "activity.prompts_skipped",
                day=log_day,
                segment=segment,
                activity=activity_id_str,
                facet=facet_str,
                mode=target_schedule,
                reason="--no-activity-prompts",
            )
            continue

        rec = record_by.get(key)
        changed = True
        input_hash: str | None = None
        if rec:
            changed, input_hash = _activity_input_changed(
                routing_day,
                facet_str,
                activity_id_str,
                rec,
            )

        if not (written_by.get(key, False) or refresh or changed):
            _jsonl_log(
                "activity.unchanged",
                day=log_day,
                segment=segment,
                activity=activity_id_str,
                facet=facet_str,
                mode=target_schedule,
            )
            continue

        logging.info(
            "Activity completed: %s facet=%s, running activity agents",
            activity_id_str,
            facet_str,
        )
        ok = run_activity_prompts(
            day=routing_day,
            activity_id=activity_id_str,
            facet=facet_str,
            refresh=refresh,
            verbose=verbose,
            max_concurrency=max_concurrency,
        )
        if ok and input_hash:
            write_activity_provenance(
                routing_day,
                facet_str,
                activity_id_str,
                input_hash,
            )


def _run_activity_state_tail(
    state_machine: ActivityStateMachine | None,
    sense_json: dict,
    segment: str,
    day: str,
    target_schedule: str,
    *,
    refresh: bool,
    verbose: bool,
    max_concurrency: int,
    skip_activity_prompts: bool,
) -> None:
    """Advance the activity state machine for a processed segment and persist.

    Shared by the active dispatch path and the redundant short-circuit so both
    produce a byte-identical activity change-set and state snapshot for the same
    ``sense_json`` (the state machine reads only ``sense_json``, never the
    per-segment write-up talents' outputs).
    """
    if state_machine is None:
        return
    routing_day = state_machine.last_segment_day or day
    changes = state_machine.update(sense_json, segment, day)
    # Persist completed activity records before running activity agents
    ended_triples = [
        (c["id"], c["facet"], c.get("_change"))
        for c in changes
        if c.get("state") == "ended"
    ]
    if state_machine.journal_root is not None:
        try:
            snapshot = {
                "last_segment_key": state_machine.last_segment_key,
                "last_segment_day": state_machine.last_segment_day,
                "active": {
                    facet: {k: v for k, v in entry.items() if k != "_change"}
                    for facet, entry in state_machine.state.items()
                },
            }
            atomic_replace(
                state_machine.journal_root / "awareness" / "activity_state.json",
                json.dumps(snapshot),
            )
        except Exception:
            logging.debug("Failed to write activity state snapshot", exc_info=True)
    _persist_and_maybe_run_activity_prompts(
        routing_day=routing_day,
        log_day=day,
        segment=segment,
        target_schedule=target_schedule,
        ended_triples=ended_triples,
        completed=state_machine.get_completed_activities(),
        refresh=refresh,
        verbose=verbose,
        max_concurrency=max_concurrency,
        skip_activity_prompts=skip_activity_prompts,
    )


def _flush_batch_state_machines(
    batch_state_machines: dict,
    day: str,
    *,
    refresh: bool,
    verbose: bool,
    max_concurrency: int,
    skip_activity_prompts: bool,
) -> None:
    """Close dangling-active activities left when the segment batch ends.

    Finality guard: import/finite streams are always safe to flush (a
    recording's segments all exist at import time); live/observer streams are
    flushed only when the day is capture-final (day strictly before today), so
    an ongoing activity on today is never truncated.
    """
    current_day = datetime.now().strftime("%Y%m%d")
    for stream, sm in batch_state_machines.items():
        if sm.last_segment_key is None:
            continue
        stream_is_import = bool(stream) and is_import_stream(stream)
        if not (stream_is_import or day < current_day):
            continue
        changes = sm.close_active(sm.last_segment_key)
        ended_triples = [
            (c["id"], c["facet"], c.get("_change"))
            for c in changes
            if c.get("state") == "ended"
        ]
        if not ended_triples:
            continue
        _persist_and_maybe_run_activity_prompts(
            routing_day=sm.last_segment_day or day,
            log_day=day,
            segment=sm.last_segment_key,
            target_schedule="segment",
            ended_triples=ended_triples,
            completed=sm.get_completed_activities(),
            refresh=refresh,
            verbose=verbose,
            max_concurrency=max_concurrency,
            skip_activity_prompts=skip_activity_prompts,
        )


_LOCAL_PROVIDER_NEEDED: bool | None = None


def reset_dispatch_admission_state() -> None:
    """Clear the per-process local-provider admission memo."""
    global _LOCAL_PROVIDER_NEEDED

    _LOCAL_PROVIDER_NEEDED = None


def _dispatch_local_provider_needed() -> bool:
    global _LOCAL_PROVIDER_NEEDED

    if _LOCAL_PROVIDER_NEEDED is None:
        _LOCAL_PROVIDER_NEEDED = is_local_provider_needed()
    return _LOCAL_PROVIDER_NEEDED


def _select_segment_repair_targets(
    day: str,
    segments: list[dict],
    *,
    force_all: bool,
) -> tuple[list[dict], dict[str, int]]:
    """Select runnable segment-thinking repair targets.

    Normal repair mode targets only fully-sensed segments whose thinking is not
    complete. ``force_all`` preserves refresh/from-scratch semantics.
    """
    counts = {
        "total": len(segments),
        "selected": 0,
        "complete": 0,
        "raw_blocked": 0,
    }
    if force_all:
        selected = list(segments)
        counts["selected"] = len(selected)
        return selected, counts

    progress = read_segment_progress(day)
    selected: list[dict] = []
    for seg in segments:
        if not segment_requires_processing(seg):
            counts["complete"] += 1
            continue
        if not segment_fully_sensed(seg.get("data_state", {})):
            counts["raw_blocked"] += 1
            continue

        key = seg["key"]
        stream = seg.get("stream")
        segment_progress = lookup_segment_progress(progress, stream, key)
        complete, _reason = segment_fully_thought(segment_progress)
        if complete:
            counts["complete"] += 1
            continue

        selected.append(seg)

    counts["selected"] = len(selected)
    return selected, counts


_SENSE_REQUIRED_KEYS = ("density", "content_type")


def _sense_output_missing_required_keys(data: dict) -> tuple[str, ...]:
    return tuple(key for key in _SENSE_REQUIRED_KEYS if key not in data)


def _read_segment_sense_json(day: str, stream: str | None, segment: str) -> dict | None:
    path = get_output_path(
        day_path(day),
        "sense",
        segment=segment,
        output_format="json",
        stream=stream,
    )
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        logging.debug(
            "No sense output for %s/%s/%s during activity replay", day, stream, segment
        )
        return None
    except (json.JSONDecodeError, OSError):
        logging.warning(
            "Failed to load sense output for %s/%s/%s during activity replay",
            day,
            stream,
            segment,
            exc_info=True,
        )
        return None
    if not isinstance(data, dict):
        return None
    missing = _sense_output_missing_required_keys(data)
    if missing:
        logging.warning(
            "Invalid Sense output for %s/%s/%s during activity replay: "
            "missing required keys: %s",
            day,
            stream,
            segment,
            ", ".join(missing),
        )
        return None
    return data


def _replay_activity_state_for_segments(
    *,
    day: str,
    segments: list[dict],
    refresh: bool,
    verbose: bool,
    max_concurrency: int,
    skip_activity_prompts: bool,
) -> None:
    """Replay activity state chronologically from persisted Sense outputs."""
    batch_state_machines: dict[str | None, ActivityStateMachine] = {}
    for seg in segments:
        stream = seg.get("stream")
        segment = seg["key"]
        sense_json = _read_segment_sense_json(day, stream, segment)
        if sense_json is None:
            continue
        sm = batch_state_machines.get(stream)
        if sm is None:
            sm = ActivityStateMachine()
            batch_state_machines[stream] = sm
        _run_activity_state_tail(
            sm,
            sense_json,
            segment,
            day,
            "segment",
            refresh=refresh,
            verbose=verbose,
            max_concurrency=max_concurrency,
            skip_activity_prompts=skip_activity_prompts,
        )

    _flush_batch_state_machines(
        batch_state_machines,
        day,
        refresh=refresh,
        verbose=verbose,
        max_concurrency=max_concurrency,
        skip_activity_prompts=skip_activity_prompts,
    )


def _run_segment_repair_batch(
    *,
    day: str,
    segments: list[dict],
    refresh: bool,
    verbose: bool,
    max_concurrency: int,
    segment_workers: int,
    timeout: int | None,
    skip_activity_prompts: bool,
    skip_talents: frozenset[str],
) -> tuple[int, int]:
    """Run selected segment repairs with bounded segment-level parallelism."""
    if not segments:
        return (0, 0)

    workers = max(1, min(segment_workers, len(segments)))
    batch_success = 0
    batch_failed = 0
    completed = 0
    total = len(segments)
    status_lock = threading.Lock()

    def _run_one(seg: dict) -> tuple[int, int]:
        seg_key = seg["key"]
        seg_stream = seg.get("stream")
        logging.info(
            "Processing repair segment: %s (%s-%s)",
            seg_key,
            seg.get("start", "?"),
            seg.get("end", "?"),
        )
        success, failed, _fn = run_segment_sense(
            day=day,
            segment=seg_key,
            refresh=refresh,
            verbose=verbose,
            max_concurrency=max_concurrency,
            stream=seg_stream,
            timeout=timeout,
            state_machine=None,
            # Activity state is replayed once across the full chronological day
            # after parallel repairs; worker threads must not mutate rolling state.
            skip_activity_prompts=True,
            skip_talents=skip_talents,
            live=False,
            predecessor=resolve_predecessor(day, seg_stream, seg_key),
        )
        return success, failed

    with ThreadPoolExecutor(max_workers=workers) as executor:
        future_to_segment = {executor.submit(_run_one, seg): seg for seg in segments}
        for future in as_completed(future_to_segment):
            seg = future_to_segment[future]
            try:
                success, failed = future.result()
            except Exception:
                logging.exception("Segment %s failed with exception", seg["key"])
                success, failed = 0, 1
            with status_lock:
                completed += 1
                batch_success += success
                batch_failed += failed
                _update_status(segments_completed=completed, segments_total=total)

    return batch_success, batch_failed


def _log_skip(name: str, reason: str, detail: str, **extra) -> None:
    """Emit an talent.skip JSONL event."""
    _jsonl_log("talent.skip", name=name, reason=reason, detail=detail, **extra)


def _record_request_lost(
    name: str,
    use_id: str,
    *,
    mode: str,
    day: str,
    segment: str | None = None,
    stream: str | None = None,
    facet: str | None = None,
    activity: str | None = None,
) -> None:
    """Record a never-claimed cortex request as an immediate failure."""
    if activity is not None:
        emit(
            "talent_completed",
            mode=mode,
            day=day,
            activity=activity,
            facet=facet,
            name=name,
            use_id=use_id,
            state="request_lost",
        )
        _jsonl_log(
            "talent.fail",
            mode=mode,
            day=day,
            activity=activity,
            facet=facet,
            name=name,
            use_id=use_id,
            state="request_lost",
            **_provider_model_fields(use_id),
        )
        return

    emit(
        "talent_completed",
        mode=mode,
        day=day,
        segment=segment,
        name=name,
        use_id=use_id,
        state="request_lost",
        **({"facet": facet} if facet else {}),
    )
    _jsonl_log(
        "talent.fail",
        mode=mode,
        day=day,
        segment=segment,
        name=name,
        use_id=use_id,
        state="request_lost",
        **_provider_model_fields(use_id),
        **({"stream": stream} if stream else {}),
        **({"facet": facet} if facet else {}),
    )


def _update_status(**fields) -> None:
    """Update shared status dict (thread-safe)."""
    with _status_lock:
        _status.update(fields)


def _clear_status() -> None:
    """Clear shared status dict (thread-safe)."""
    with _status_lock:
        _status.clear()


def _emit_periodic_status() -> None:
    """Emit think.status every 5 seconds while active (runs in daemon thread)."""
    while not _stop_status.is_set():
        _stop_status.wait(5)
        if _stop_status.is_set():
            break
        try:
            with _status_lock:
                snapshot = dict(_status) if _status else None
            if snapshot:
                emit("status", **snapshot)
        except Exception:
            logging.debug("Status emission failed", exc_info=True)


def run_bounded_phase(
    cmd: list[str], day: str, timeout: float | None
) -> tuple[bool, bool]:
    """Run a phase subprocess.

    Returns (ok, timed_out). timed_out is True only when the wall-clock budget
    was exceeded (covers the terminate-re-raise edge).
    """
    logging.info("==> %s", " ".join(cmd))
    cmd_name = cmd[1] if cmd[0] in ("sol", "journal") and len(cmd) > 1 else cmd[0]
    cmd_name = cmd_name.replace("-", "_")

    try:
        success, exit_code, _log_path, timed_out = run_task(
            cmd, day=day, timeout=timeout
        )
        if not success:
            if timed_out:
                logging.error(
                    "Command exceeded its %ss budget: %s", timeout, " ".join(cmd)
                )
                day_log(day, f"{cmd_name} timeout")
            else:
                logging.error(
                    "Command failed with exit code %s: %s", exit_code, " ".join(cmd)
                )
                day_log(day, f"{cmd_name} error {exit_code}")
        return (success, timed_out)
    except subprocess.TimeoutExpired:
        logging.error("Command timed out and could not be reaped: %s", " ".join(cmd))
        day_log(day, f"{cmd_name} timeout")
        return (False, True)
    except Exception as e:
        logging.error("Command exception: %s: %s", e, " ".join(cmd))
        day_log(day, f"{cmd_name} exception")
        return (False, False)


def run_command(cmd: list[str], day: str) -> bool:
    """Run a shell command synchronously (unbounded)."""
    ok, _timed_out = run_bounded_phase(cmd, day, timeout=None)
    return ok


def run_queued_command(cmd: list[str], day: str, timeout: int = 600) -> bool:
    """Run a command through supervisor's task queue and wait for completion."""
    import uuid

    cmd_name = cmd[1] if cmd[0] in ("sol", "journal") and len(cmd) > 1 else cmd[0]
    cmd_name_log = cmd_name.replace("-", "_")
    ref = f"think-{uuid.uuid4().hex[:8]}"

    logging.info("==> %s (queued, ref=%s)", " ".join(cmd), ref)

    if not _callosum:
        logging.error("Callosum not connected, cannot queue command")
        day_log(day, f"{cmd_name_log} error no_callosum")
        return False

    result = {"completed": False, "exit_code": None}
    result_event = threading.Event()

    def on_message(msg: dict) -> None:
        if msg.get("tract") != "supervisor":
            return
        if msg.get("event") != "stopped":
            return
        if msg.get("ref") != ref:
            return
        result["completed"] = True
        result["exit_code"] = msg.get("exit_code", -1)
        result_event.set()

    listener = CallosumConnection()
    listener.start(callback=on_message)

    try:
        _callosum.emit("supervisor", "request", cmd=cmd, ref=ref, day=day)

        if not result_event.wait(timeout=timeout):
            logging.error(f"Timeout waiting for {cmd_name} to complete (ref={ref})")
            day_log(day, f"{cmd_name_log} error timeout")
            return False

        if result["exit_code"] != 0:
            logging.error(
                "Command failed with exit code %s: %s",
                result["exit_code"],
                " ".join(cmd),
            )
            day_log(day, f"{cmd_name_log} error {result['exit_code']}")
            return False

        return True
    finally:
        listener.stop()


def emit(event: str, **fields) -> None:
    """Emit a think tract event if callosum is connected."""
    if _callosum:
        _callosum.emit("think", event, **fields)


def check_callosum_available() -> bool:
    """Check if Callosum socket exists (supervisor running)."""
    socket_path = Path(get_journal()) / "health" / "callosum.sock"
    return socket_path.exists()


_SKIPPED: object = object()


@dataclass(frozen=True)
class CappedDailyUnit:
    name: str
    facet: str | None
    reason_code: str
    count: int


@dataclass(frozen=True)
class DailyCompletionVerdict:
    complete: bool
    daily_units_terminal: bool
    segment_blockers: tuple[dict[str, str], ...]
    capped_daily_units: tuple[CappedDailyUnit, ...]


class _NotClaimed:
    __slots__ = ("use_id",)

    def __init__(self, use_id: str) -> None:
        self.use_id = use_id


def _emit_memory_throttle_started(**fields: Any) -> None:
    emit(
        "memory_throttle_started",
        stage=fields["stage"],
        available_mib=fields["available_mib"],
        floor_mib=fields["floor_mib"],
    )


def _emit_memory_throttle_completed(**fields: Any) -> None:
    emit(
        "memory_throttle_completed",
        stage=fields["stage"],
        waited_seconds=fields["waited_seconds"],
    )
    _jsonl_log(
        "memory_throttle.complete",
        stage=fields["stage"],
        waited_seconds=fields["waited_seconds"],
    )


def _dispatch_cortex_request(**kwargs) -> str | None | _NotClaimed:
    """Call cortex_request and classify dispatch failures for orchestrators.

    Orchestrated units are re-walked hours later when a request is lost, so they
    wait out a broadcast burst on the patient claim schedule rather than
    fast-failing the way interactive callers do.
    """
    floor = admission.resolve_memory_floor_bytes()
    if floor > 0 and _dispatch_local_provider_needed():
        admission.wait_for_memory_headroom(
            "think",
            on_throttle_start=_emit_memory_throttle_started,
            on_throttle_end=_emit_memory_throttle_completed,
        )

    try:
        return cortex_request(**kwargs, claim_windows=PATIENT_CLAIM_WINDOWS)
    except CortexSpawnUnavailable as exc:
        logging.info("cortex_request unavailable: %s", exc.detail or "unknown")
        return None
    except CortexNotClaimed as exc:
        name = kwargs.get("name", "unknown")
        logging.warning(
            "cortex request not claimed for '%s' (use_id=%s)",
            name,
            exc.use_id,
        )
        return _NotClaimed(exc.use_id)


def _drain_priority_batch(
    spawned: list[tuple[str, str, dict, str | None]],
    target_schedule: str,
    day: str,
    segment: str | None,
    stream: str | None = None,
    timeout: int | None = 610,
) -> tuple[int, int, list[str]]:
    """Wait for a batch of spawned agents and process their results.

    Waits for all agents in the batch to complete, checks end states,
    emits completion events, and runs incremental indexing for generators.

    Args:
        spawned: List of (use_id, prompt_name, config, facet) tuples
        target_schedule: "segment" or "daily"
        day: Day in YYYYMMDD format
        segment: Optional segment key
        stream: Optional stream name

    Returns:
        Tuple of (success_count, failed_count, failed_names) where
        failed_names contains descriptions like "flow (error)" or
        "recap/work (timeout)".
    """
    if not spawned:
        return (0, 0, [])

    agent_ids = [use_id for use_id, _, _, _ in spawned]
    logging.info(f"Waiting for {len(agent_ids)} agents...")

    completed, timed_out = wait_for_uses(agent_ids, timeout=timeout)

    success = 0
    failed = 0
    failed_names: list[str] = []

    if timed_out:
        logging.warning(f"{len(timed_out)} agents timed out: {timed_out}")
        failed += len(timed_out)
        for use_id in timed_out:
            timed_name = next(
                (n for aid, n, _, _ in spawned if aid == use_id), "unknown"
            )
            timed_facet = next((f for aid, _, _, f in spawned if aid == use_id), None)
            label = f"{timed_name}/{timed_facet}" if timed_facet else timed_name
            state = _classify_timeout_state(use_id)
            failed_names.append(f"{label} ({state})")
            emit(
                "talent_completed",
                mode=target_schedule,
                day=day,
                segment=segment,
                name=timed_name,
                use_id=use_id,
                state=state,
                **({"facet": timed_facet} if timed_facet else {}),
            )
            _jsonl_log(
                "talent.fail",
                mode=target_schedule,
                day=day,
                segment=segment,
                name=timed_name,
                use_id=use_id,
                state=state,
                **_provider_model_fields(use_id),
                **({"stream": stream} if stream else {}),
                **({"facet": timed_facet} if timed_facet else {}),
            )

    for use_id, prompt_name, config, agent_facet in spawned:
        if use_id in timed_out:
            continue

        end_state = completed.get(use_id, "unknown")
        if end_state == "finish":
            finish_fields = _cache_fields(use_id)
            logging.info(f"{prompt_name} completed successfully")
            success += 1
            emit(
                "talent_completed",
                mode=target_schedule,
                day=day,
                segment=segment,
                name=prompt_name,
                use_id=use_id,
                state="finish",
                **({"facet": agent_facet} if agent_facet else {}),
            )
            _jsonl_log(
                "talent.complete",
                mode=target_schedule,
                day=day,
                segment=segment,
                name=prompt_name,
                use_id=use_id,
                state="finish",
                **({"stream": stream} if stream else {}),
                **({"facet": agent_facet} if agent_facet else {}),
                **_cache_terminal_fields(finish_fields),
            )

            # Incremental indexing for generators (skip JSON —
            # structured metadata not suitable for full-text index)
            is_generate = config["type"] == "generate"
            output_format = config.get("output", "md")
            if is_generate and output_format != "json":
                output_path = get_output_path(
                    day_path(day),
                    prompt_name,
                    segment=segment,
                    output_format=output_format,
                    stream=stream,
                )
                _maybe_rescan_output(
                    output_path,
                    finish_fields["output_changed"],
                    day,
                )
        else:
            label = f"{prompt_name}/{agent_facet}" if agent_facet else prompt_name
            logging.error(f"{label} ended with state: {end_state}")
            failed += 1
            failed_names.append(f"{label} ({end_state})")
            emit(
                "talent_completed",
                mode=target_schedule,
                day=day,
                segment=segment,
                name=prompt_name,
                use_id=use_id,
                state=end_state,
                **({"facet": agent_facet} if agent_facet else {}),
            )
            _jsonl_log(
                "talent.fail",
                mode=target_schedule,
                day=day,
                segment=segment,
                name=prompt_name,
                use_id=use_id,
                state=end_state,
                **_provider_model_fields(use_id),
                **({"stream": stream} if stream else {}),
                **({"facet": agent_facet} if agent_facet else {}),
            )

    return (success, failed, failed_names)


def _segment_dir(day: str, segment: str, stream: str | None) -> Path:
    """Return the expected segment directory without creating it."""
    return day_path(day) / (stream or "default") / segment


def _empty_input_sense_output() -> dict:
    """Schema-valid minimal idle Sense output for a segment with no input to sense."""
    return {
        "density": "idle",
        "content_type": "idle",
        "activity_summary": "",
        "entities": [],
        "facets": [],
        "speculative_facet": None,
        "meeting_detected": False,
        "speakers": [],
        "recommend": {"screen_record": False, "speaker_attribution": False},
        "emotional_register": "neutral",
    }


def _terminalize_idle_segment(
    sense_json: dict,
    seg_dir,
    day: str,
    segment: str,
    target_schedule: str,
    state_machine,
    *,
    verbose: bool,
    max_concurrency: int,
    skip_activity_prompts: bool,
    start_time: float,
    total_success: int,
    total_failed: int,
    all_failed_names: list[str],
) -> tuple[int, int, list[str]]:
    write_idle_stubs(seg_dir)
    logging.info("Segment %s is idle, skipping remaining agents", segment)
    _log_skip(
        "*",
        "density_idle",
        f"Segment {segment} is idle, skipping remaining agents",
        mode=target_schedule,
        day=day,
        segment=segment,
    )
    if state_machine is not None:
        routing_day = state_machine.last_segment_day or day
        idle_changes = state_machine.update(sense_json, segment, day)
        # Persist completed activity records from idle transitions
        ended_triples = [
            (c["id"], c["facet"], c.get("_change"))
            for c in idle_changes
            if c.get("state") == "ended"
        ]
        _persist_and_maybe_run_activity_prompts(
            routing_day=routing_day,
            log_day=day,
            segment=segment,
            target_schedule=target_schedule,
            ended_triples=ended_triples,
            completed=state_machine.get_completed_activities(),
            refresh=False,
            verbose=verbose,
            max_concurrency=max_concurrency,
            skip_activity_prompts=skip_activity_prompts,
        )
        if state_machine.journal_root is not None:
            try:
                snapshot = {
                    "last_segment_key": state_machine.last_segment_key,
                    "last_segment_day": state_machine.last_segment_day,
                    "active": {
                        facet: {k: v for k, v in entry.items() if k != "_change"}
                        for facet, entry in state_machine.state.items()
                    },
                }
                atomic_replace(
                    state_machine.journal_root / "awareness" / "activity_state.json",
                    json.dumps(snapshot),
                )
            except Exception:
                logging.debug("Failed to write activity state snapshot", exc_info=True)

    duration_ms = int((time.time() - start_time) * 1000)
    emit(
        "completed",
        mode=target_schedule,
        day=day,
        segment=segment,
        success=total_success,
        failed=total_failed,
        failed_names=all_failed_names,
        duration_ms=duration_ms,
    )
    return (total_success, total_failed, all_failed_names)


def _resolve_segment_dir(
    day: str,
    segment: str,
    stream: str | None,
) -> Path | None:
    """Resolve a segment directory, searching across streams when needed."""
    if stream:
        path = _segment_dir(day, segment, stream)
        return path if path.is_dir() else None

    for seg_stream, seg_key, seg_path in iter_segments(day):
        if seg_key == segment:
            return seg_path
    return None


def _load_json_file(path: Path, default: object) -> object:
    """Load JSON from a file, returning the provided default on failure."""
    if not path.exists():
        return default
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (json.JSONDecodeError, OSError):
        return default


def _has_audio_embeddings(seg_dir: Path) -> bool:
    """Return True when a segment has audio embedding files."""
    for npz_path in seg_dir.glob("*.npz"):
        if npz_path.stem == "audio" or npz_path.stem.endswith("_audio"):
            return True
    return False


def _check_daily_skip(
    name: str,
    facet: str | None,
    *,
    mode: str,
    completed: set[tuple[str, str, str | None]],
    deterministic_failures: dict[tuple[str, str | None], DeterministicFailure],
    retry_on_deterministic_failure: bool = False,
    from_scratch: bool = False,
) -> tuple[bool, str | None]:
    if mode != "daily":
        return (False, None)
    if from_scratch:
        return (False, None)
    if (mode, name, facet) in completed:
        return (True, "already_complete")
    if not retry_on_deterministic_failure:
        failure = deterministic_failures.get((name, facet))
        # Dispatch uses deterministic_failure_caps.failure_capped, as does
        # evaluate_daily_completion. retry_on_deterministic_failure affects
        # dispatch only; completed day finalization ends retries, so forcing
        # convergence-by-retry with this flag is unsupported.
        if failure is not None and failure_capped(failure.reason_code, failure.count):
            return (True, "deterministic_failure_no_retry")
    return (False, None)


def evaluate_daily_completion(
    applicable_units: set[tuple[str, str | None]],
    completed_units: set[tuple[str, str, str | None]],
    deterministic_failures: dict[tuple[str, str | None], DeterministicFailure],
    segment_blockers: list[dict[str, str]] | tuple[dict[str, str], ...],
) -> DailyCompletionVerdict:
    """Return the terminal daily completion verdict without journal writes."""
    capped_daily_units: list[CappedDailyUnit] = []
    daily_units_terminal = True

    # Invariant shared with _check_daily_skip via deterministic_failure_caps.failure_capped:
    # a unit's failure cap is terminal; the same predicate that stops dispatch
    # marks the unit terminal-degraded for day completion. Day completion means
    # all applicable units terminal, not all units succeeded. Degradation is
    # terminal but visible. This deliberately ignores the dispatch retry override.
    for name, facet in sorted(
        applicable_units, key=lambda unit: (unit[0], unit[1] or "")
    ):
        if ("daily", name, facet) in completed_units:
            continue
        failure = deterministic_failures.get((name, facet))
        if failure is not None and failure_capped(failure.reason_code, failure.count):
            capped_daily_units.append(
                CappedDailyUnit(
                    name=name,
                    facet=facet,
                    reason_code=failure.reason_code,
                    count=failure.count,
                )
            )
            continue
        daily_units_terminal = False

    blockers = tuple(segment_blockers)
    return DailyCompletionVerdict(
        complete=daily_units_terminal and not blockers,
        daily_units_terminal=daily_units_terminal,
        segment_blockers=blockers,
        capped_daily_units=tuple(capped_daily_units),
    )


def _capped_daily_unit_payload(unit: CappedDailyUnit) -> dict[str, object]:
    return {
        "name": unit.name,
        "facet": unit.facet,
        "reason_code": unit.reason_code,
        "count": unit.count,
    }


def finalize_day_completion(
    day: str, verdict: DailyCompletionVerdict
) -> dict[str, object]:
    """Write or withhold the daily marker and return daily_complete extras."""
    capped_payload = [
        _capped_daily_unit_payload(unit) for unit in verdict.capped_daily_units
    ]
    payload_fragment: dict[str, object] = {}
    if capped_payload:
        payload_fragment["capped_daily_units"] = capped_payload

    if verdict.complete:
        health_dir = day_path(day) / "health"
        health_dir.mkdir(parents=True, exist_ok=True)
        (health_dir / "daily.updated").touch()
        if capped_payload:
            logging.info(
                "Day %s complete with capped daily unit(s); wrote daily.updated: capped_daily_units=%s",
                day,
                capped_payload,
            )
        else:
            logging.info("Day %s fully complete; wrote daily.updated", day)
    else:
        logging.info(
            "Day %s withholding daily.updated: daily_units_terminal=%s segment_blockers=%s capped_daily_units=%s",
            day,
            verdict.daily_units_terminal,
            list(verdict.segment_blockers),
            capped_payload,
        )

    return payload_fragment


def run_segment_sense(
    day: str,
    segment: str,
    refresh: bool,
    verbose: bool,
    max_concurrency: int = 2,
    stream: str | None = None,
    timeout: int | None = 610,
    state_machine: ActivityStateMachine | None = None,
    *,
    skip_activity_prompts: bool = False,
    skip_talents: frozenset[str] = frozenset(),
    live: bool = False,
    predecessor: dict | None = None,
) -> tuple[int, int, list[str]]:
    """Run Sense-first linear orchestrator for a single segment.

    Dispatches the Sense agent first, parses its output to determine segment
    density and conditional agent recommendations, then dispatches remaining
    agents based on Sense output.

    A talent whose config declares `new_only` is dispatched only when
    `live=True` (the observe-triggered current-segment think). On any
    historical/batch run (`live=False`, the default) such talents are skipped
    via `talent.skip` with reason `new_only_historical` — never counted as a
    failure.

    `new_only` is read as raw Python truthiness from the config dict
    (`config.get("new_only")`), so the talent frontmatter must declare the JSON
    boolean `true`, not the string `"true"`.
    """
    target_schedule = "segment"
    all_prompts = get_talent_configs(schedule="segment")
    if not all_prompts:
        logging.info("No prompts found for schedule: segment")
        return (0, 0, [])

    def _cfg(name: str) -> dict | None:
        return all_prompts.get(name)

    def _dispatch_agent(name: str, config: dict) -> str | None | _NotClaimed | object:
        if name in skip_talents:
            _log_skip(
                name,
                "skip_talents_flag",
                "Skipped by --skip-talents",
                day=day,
                segment=segment,
            )
            return _SKIPPED

        if config.get("new_only") and not live:
            _log_skip(
                name,
                "new_only_historical",
                "Skipped: new_only talent runs only on a live current-segment think",
                mode=target_schedule,
                day=day,
                segment=segment,
            )
            return _SKIPPED

        is_generate = config["type"] == "generate"
        request_config: dict = {"day": day, "segment": segment}
        if is_generate:
            request_config["output"] = config.get("output", "md")
            if refresh:
                request_config["refresh"] = True
        elif config.get("output"):
            request_config["output"] = config["output"]

        env: dict[str, str] = {"SOL_DAY": day, "SOL_SEGMENT": segment}
        if stream:
            request_config["stream"] = stream
            env["SOL_STREAM"] = stream
        request_config["env"] = env
        request_config["schedule"] = target_schedule

        prompt = (
            ""
            if is_generate
            else f"Running scheduled task for {iso_date(day)}: {day_input_summary(day)}."
        )
        return _dispatch_cortex_request(prompt=prompt, name=name, config=request_config)

    sense_config = _cfg("sense")
    if sense_config is None:
        logging.error("Sense agent not found in segment configs")
        _log_skip(
            "sense",
            "no_config",
            "Sense agent not found in segment configs",
            mode=target_schedule,
            day=day,
            segment=segment,
            **({"stream": stream} if stream else {}),
        )
        return (0, 1, ["sense (not_configured)"])

    data_states = read_segment_data_state(day, segment, stream)
    in_flight = sorted(
        modality
        for modality, state in data_states.items()
        if state in {DataState.PENDING, DataState.ANALYZING}
    )
    if in_flight:
        _jsonl_log(
            "sense.skip",
            mode=target_schedule,
            day=day,
            segment=segment,
            reason="raw_media_pending",
            modalities=in_flight,
            **({"stream": stream} if stream else {}),
        )
        return (0, 0, [])

    day_dir = day_path(day)
    seg_dir = _segment_dir(day, segment, stream)
    prune_orphan_provenance(day, stream, segment)
    start_time = time.time()
    total_success = 0
    total_failed = 0
    all_failed_names: list[str] = []

    _update_status(
        mode=target_schedule,
        day=day,
        segment=segment,
        stream=stream,
        agents_total=1,
        agents_completed=0,
        current_agents=[],
    )

    emit(
        "started",
        mode=target_schedule,
        day=day,
        segment=segment,
        count=1,
        groups=1,
    )

    if check_segment_has_no_input(
        day, segment, sense_config.get("load", {}), stream=stream
    ):
        logging.info(
            "Segment %s has no sense input; gating to idle terminal without dispatch",
            segment,
        )
        idle_sense_json = _empty_input_sense_output()
        write_sense_outputs(idle_sense_json, seg_dir, stream=stream)
        _jsonl_log(
            "sense.complete",
            mode=target_schedule,
            day=day,
            segment=segment,
            density="idle",
            gated="no_input",
            recommend=idle_sense_json["recommend"],
            **({"stream": stream} if stream else {}),
        )
        change_result = detect_segment_change(
            day,
            stream,
            segment,
            seg_dir,
            predecessor=predecessor,
            timestamp=datetime.now(tz=timezone.utc).isoformat(),
        )
        write_change_detection(seg_dir, change_result)
        _jsonl_log(
            "sense.change_detect",
            mode=target_schedule,
            day=day,
            segment=segment,
            change_class=change_result["change_class"],
            changed_sensors=change_result["changed_sensors"],
            predecessor=change_result["predecessor"],
            **({"stream": stream} if stream else {}),
        )
        return _terminalize_idle_segment(
            idle_sense_json,
            seg_dir,
            day,
            segment,
            target_schedule,
            state_machine,
            verbose=verbose,
            max_concurrency=max_concurrency,
            skip_activity_prompts=skip_activity_prompts,
            start_time=start_time,
            total_success=0,
            total_failed=0,
            all_failed_names=[],
        )

    sense_agent_id = _dispatch_agent("sense", sense_config)
    if sense_agent_id is None:
        _log_skip(
            "sense",
            "send_failed",
            "All cortex request attempts failed",
            mode=target_schedule,
            day=day,
            segment=segment,
        )
        duration_ms = int((time.time() - start_time) * 1000)
        emit(
            "completed",
            mode=target_schedule,
            day=day,
            segment=segment,
            success=0,
            failed=1,
            failed_names=["sense (send)"],
            duration_ms=duration_ms,
        )
        return (0, 1, ["sense (send)"])
    if isinstance(sense_agent_id, _NotClaimed):
        _record_request_lost(
            "sense",
            sense_agent_id.use_id,
            mode=target_schedule,
            day=day,
            segment=segment,
            stream=stream,
        )
        duration_ms = int((time.time() - start_time) * 1000)
        emit(
            "completed",
            mode=target_schedule,
            day=day,
            segment=segment,
            success=0,
            failed=1,
            failed_names=["sense (request_lost)"],
            duration_ms=duration_ms,
        )
        return (0, 1, ["sense (request_lost)"])
    elif sense_agent_id is not _SKIPPED:
        emit(
            "talent_started",
            mode=target_schedule,
            day=day,
            segment=segment,
            name="sense",
            use_id=sense_agent_id,
        )
        _jsonl_log(
            "talent.dispatch",
            mode=target_schedule,
            day=day,
            segment=segment,
            name="sense",
            use_id=sense_agent_id,
            **({"stream": stream} if stream else {}),
        )
        _update_status(current_agents=["sense"])

        s, f, fn = _drain_priority_batch(
            [(sense_agent_id, "sense", sense_config, None)],
            target_schedule,
            day,
            segment,
            stream,
            timeout,
        )
        total_success += s
        total_failed += f
        all_failed_names.extend(fn)
        _update_status(agents_completed=total_success + total_failed, current_agents=[])

        if f > 0:
            duration_ms = int((time.time() - start_time) * 1000)
            emit(
                "completed",
                mode=target_schedule,
                day=day,
                segment=segment,
                success=total_success,
                failed=total_failed,
                failed_names=all_failed_names,
                duration_ms=duration_ms,
            )
            return (total_success, total_failed, all_failed_names)

    sense_output_path = get_output_path(
        day_dir,
        "sense",
        segment=segment,
        output_format="json",
        stream=stream,
    )
    try:
        sense_json = json.loads(sense_output_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        logging.error("Failed to read Sense output %s: %s", sense_output_path, exc)
        failed_names = all_failed_names + ["sense (output_parse)"]
        duration_ms = int((time.time() - start_time) * 1000)
        emit(
            "completed",
            mode=target_schedule,
            day=day,
            segment=segment,
            success=total_success,
            failed=total_failed + 1,
            failed_names=failed_names,
            duration_ms=duration_ms,
        )
        return (total_success, total_failed + 1, failed_names)

    if not isinstance(sense_json, dict):
        missing = _SENSE_REQUIRED_KEYS
    else:
        missing = _sense_output_missing_required_keys(sense_json)
    if missing:
        logging.warning(
            "Invalid Sense output %s: missing required keys: %s",
            sense_output_path,
            ", ".join(missing),
        )
        failed_names = all_failed_names + ["sense (output_invalid)"]
        duration_ms = int((time.time() - start_time) * 1000)
        emit(
            "completed",
            mode=target_schedule,
            day=day,
            segment=segment,
            success=total_success,
            failed=total_failed + 1,
            failed_names=failed_names,
            duration_ms=duration_ms,
        )
        return (total_success, total_failed + 1, failed_names)

    write_sense_outputs(sense_json, seg_dir, stream=stream)
    density = sense_json["density"]
    _jsonl_log(
        "sense.complete",
        mode=target_schedule,
        day=day,
        segment=segment,
        density=density,
        recommend=sense_json.get("recommend") or {},
        **({"stream": stream} if stream else {}),
    )
    change_result = detect_segment_change(
        day,
        stream,
        segment,
        seg_dir,
        predecessor=predecessor,
        timestamp=datetime.now(tz=timezone.utc).isoformat(),
    )
    write_change_detection(seg_dir, change_result)
    _jsonl_log(
        "sense.change_detect",
        mode=target_schedule,
        day=day,
        segment=segment,
        change_class=change_result["change_class"],
        changed_sensors=change_result["changed_sensors"],
        predecessor=change_result["predecessor"],
        **({"stream": stream} if stream else {}),
    )

    if density == "idle" and not refresh:
        return _terminalize_idle_segment(
            sense_json,
            seg_dir,
            day,
            segment,
            target_schedule,
            state_machine,
            verbose=verbose,
            max_concurrency=max_concurrency,
            skip_activity_prompts=skip_activity_prompts,
            start_time=start_time,
            total_success=total_success,
            total_failed=total_failed,
            all_failed_names=all_failed_names,
        )

    if change_result["change_class"] == "redundant" and not refresh:
        from solstone.apps.timeline.talent.segment_summary import (
            write_continuation_summary,
        )

        predecessor_segment = change_result["predecessor"]["segment"]
        write_continuation_summary(seg_dir, predecessor_segment)
        logging.info(
            "Segment %s is redundant (continues %s), skipping write-up talents",
            segment,
            predecessor_segment,
        )
        _log_skip(
            "*",
            "change_redundant",
            f"Segment {segment} unchanged vs {predecessor_segment}, "
            "skipping write-up talents",
            mode=target_schedule,
            day=day,
            segment=segment,
        )
        _run_activity_state_tail(
            state_machine,
            sense_json,
            segment,
            day,
            target_schedule,
            refresh=refresh,
            verbose=verbose,
            max_concurrency=max_concurrency,
            skip_activity_prompts=skip_activity_prompts,
        )
        duration_ms = int((time.time() - start_time) * 1000)
        emit(
            "completed",
            mode=target_schedule,
            day=day,
            segment=segment,
            success=total_success,
            failed=total_failed,
            failed_names=all_failed_names,
            duration_ms=duration_ms,
        )
        return (total_success, total_failed, all_failed_names)

    recommend = sense_json.get("recommend") or {}
    has_audio_embeddings = _has_audio_embeddings(seg_dir)
    agents_to_run: list[tuple[str, dict]] = []

    for floor_name in SEGMENT_FLOOR_TALENTS:
        floor_config = _cfg(floor_name)
        if not floor_config:
            _log_skip(
                floor_name,
                "no_config",
                f"{floor_name} config not found",
                mode=target_schedule,
                day=day,
                segment=segment,
                **({"stream": stream} if stream else {}),
            )
            continue
        if not refresh and is_floor_talent_capped(day, stream, segment, floor_name):
            _log_skip(
                floor_name,
                "capped",
                f"{floor_name} skipped after repeated failures; reprocess to retry",
                mode=target_schedule,
                day=day,
                segment=segment,
                **({"stream": stream} if stream else {}),
            )
            continue
        agents_to_run.append((floor_name, floor_config))

    summary_name = "timeline:segment_summary"
    summary_config = _cfg(summary_name)
    if summary_config:
        agents_to_run.append((summary_name, summary_config))
    else:
        _log_skip(
            summary_name,
            "no_config",
            f"{summary_name} config not found",
            mode=target_schedule,
            day=day,
            segment=segment,
            **({"stream": stream} if stream else {}),
        )

    detection_name = "entities:detection"
    detection_config = _cfg(detection_name)
    if detection_config:
        agents_to_run.append((detection_name, detection_config))
    else:
        _log_skip(
            detection_name,
            "no_config",
            f"{detection_name} config not found",
            mode=target_schedule,
            day=day,
            segment=segment,
            **({"stream": stream} if stream else {}),
        )

    # Only fold-consumed segment events carry stream; not_recommended skips stay untagged.
    if recommend.get("screen_record"):
        screen_config = _cfg("screen")
        if screen_config:
            agents_to_run.append(("screen", screen_config))
        else:
            _log_skip(
                "screen",
                "no_config",
                "screen config not found",
                mode=target_schedule,
                day=day,
                segment=segment,
                **({"stream": stream} if stream else {}),
            )
    else:
        _log_skip(
            "screen",
            "not_recommended",
            "screen_record not recommended by sense",
            mode=target_schedule,
            day=day,
            segment=segment,
        )

    if recommend.get("speaker_attribution") and has_audio_embeddings:
        speaker_config = _cfg("speaker_attribution")
        if speaker_config:
            agents_to_run.append(("speaker_attribution", speaker_config))
        else:
            _log_skip(
                "speaker_attribution",
                "no_config",
                "speaker_attribution config not found",
                mode=target_schedule,
                day=day,
                segment=segment,
                **({"stream": stream} if stream else {}),
            )
    else:
        if not recommend.get("speaker_attribution"):
            _log_skip(
                "speaker_attribution",
                "not_recommended",
                "speaker_attribution not recommended by sense",
                mode=target_schedule,
                day=day,
                segment=segment,
            )
        elif not has_audio_embeddings:
            _log_skip(
                "speaker_attribution",
                "not_recommended",
                "no audio embeddings available",
                mode=target_schedule,
                day=day,
                segment=segment,
            )

    _update_status(agents_total=1 + len(agents_to_run))

    spawned: list[tuple[str, str, dict, str | None]] = []
    for agent_name, config in agents_to_run:
        use_id = _dispatch_agent(agent_name, config)
        if use_id is _SKIPPED:
            continue
        if use_id is None:
            _log_skip(
                agent_name,
                "send_failed",
                f"All cortex request attempts failed for {agent_name}",
                mode=target_schedule,
                day=day,
                segment=segment,
            )
            total_failed += 1
            all_failed_names.append(f"{agent_name} (send)")
            _update_status(agents_completed=total_success + total_failed)
            continue
        if isinstance(use_id, _NotClaimed):
            _record_request_lost(
                agent_name,
                use_id.use_id,
                mode=target_schedule,
                day=day,
                segment=segment,
                stream=stream,
            )
            total_failed += 1
            all_failed_names.append(f"{agent_name} (request_lost)")
            _update_status(agents_completed=total_success + total_failed)
            continue

        spawned.append((use_id, agent_name, config, None))
        emit(
            "talent_started",
            mode=target_schedule,
            day=day,
            segment=segment,
            name=agent_name,
            use_id=use_id,
        )
        _jsonl_log(
            "talent.dispatch",
            mode=target_schedule,
            day=day,
            segment=segment,
            name=agent_name,
            use_id=use_id,
            **({"stream": stream} if stream else {}),
        )

        if max_concurrency and len(spawned) >= max_concurrency:
            _update_status(current_agents=[name for _, name, _, _ in spawned])
            s, f, fn = _drain_priority_batch(
                spawned,
                target_schedule,
                day,
                segment,
                stream,
                timeout,
            )
            total_success += s
            total_failed += f
            all_failed_names.extend(fn)
            spawned = []
            _update_status(
                agents_completed=total_success + total_failed,
                current_agents=[],
            )

    if spawned:
        _update_status(current_agents=[name for _, name, _, _ in spawned])
        s, f, fn = _drain_priority_batch(
            spawned,
            target_schedule,
            day,
            segment,
            stream,
            timeout,
        )
        total_success += s
        total_failed += f
        all_failed_names.extend(fn)
        _update_status(
            agents_completed=total_success + total_failed,
            current_agents=[],
        )

    _run_activity_state_tail(
        state_machine,
        sense_json,
        segment,
        day,
        target_schedule,
        refresh=refresh,
        verbose=verbose,
        max_concurrency=max_concurrency,
        skip_activity_prompts=skip_activity_prompts,
    )

    duration_ms = int((time.time() - start_time) * 1000)
    emit(
        "completed",
        mode=target_schedule,
        day=day,
        segment=segment,
        success=total_success,
        failed=total_failed,
        failed_names=all_failed_names,
        duration_ms=duration_ms,
    )

    logging.info(
        "Segment sense completed: %s succeeded, %s failed",
        total_success,
        total_failed,
    )
    return (total_success, total_failed, all_failed_names)


def _apply_output_persistence(
    request_config: dict, config: dict, *, force_refresh: bool
) -> None:
    """Configure a dispatch request's persistent-output fields in place.

    Generators and cogitate talents that declare an explicit output format
    produce a persisted file. For those, set ``output`` (so prepare_config
    computes an output_path) and, when ``force_refresh`` is True, set
    ``refresh`` (so the output-exists guard in _run_talent is bypassed and the
    talent regenerates). Cogitate talents with no declared output are left
    untouched — they do not persist. ``refresh`` is left absent when not
    forcing, matching the existing dispatch-config representation. Talents that
    declare ``accumulate`` persist from their post-hook and suppress this
    single-file output path.
    """
    # Accumulate talents persist via their post-hook's day_accumulator.append_record
    # (chronicle/<day>/talents/<name>.jsonl). Suppress the framework's single-file
    # write by leaving request_config["output"] unset -> prepare_config computes no
    # output_path -> talent_emit_event skips _write_output. The talent still declares
    # output:json + schema: in frontmatter, so config validation passes and the JSON
    # schema still reaches the model. NOTE: this covers schedules that route through
    # _apply_output_persistence (cadence + daily/weekly); segment/activity/flush set
    # output directly and are not covered — intentional, no consumer needs them.
    if config.get("accumulate"):
        return

    is_generate = config["type"] == "generate"
    if is_generate or config.get("output"):
        request_config["output"] = config.get("output") or "md"
        if force_refresh:
            request_config["refresh"] = True


def run_daily_prompts(
    day: str,
    verbose: bool,
    max_concurrency: int = 2,
    stream: str | None = None,
    timeout: int | None = 610,
    *,
    from_scratch: bool = False,
) -> tuple[int, int, list[str], set[tuple[str, str | None]]]:
    """Run all daily scheduled prompts in priority order.

    Loads all daily prompts, groups by priority, and executes each group with
    bounded concurrency. Waits for completion before proceeding to the next
    priority group. For generators (prompts with output), runs incremental
    indexing after each completes.

    Args:
        day: Day in YYYYMMDD format
        verbose: Verbose logging
        max_concurrency: Max agents to run concurrently per priority group.
            0 means unlimited (all agents in a group run in parallel).

    Returns:
        Tuple of (success_count, fail_count, failed_names, applicable_units) where
        failed_names contains descriptions like "flow (error)" and
        applicable_units contains (name, facet) daily units that survived
        structural filters.
    """
    target_schedule = "daily"

    # Load ALL scheduled prompts (both generators and agents)
    all_prompts = get_talent_configs(schedule=target_schedule)

    if not all_prompts:
        logging.info(f"No prompts found for schedule: {target_schedule}")
        return (0, 0, [], set())

    completed_units = read_completed_units(day)
    deterministic_failures = read_daily_deterministic_failures(day)

    # Group prompts by priority
    priority_groups: dict[int, list[tuple[str, dict]]] = {}
    for name, config in all_prompts.items():
        priority = config["priority"]  # Required field, validated by get_talent_configs
        priority_groups.setdefault(priority, []).append((name, config))

    # Pre-compute shared data for multi-facet prompts
    day_formatted = iso_date(day)
    input_summary = day_input_summary(day)
    enabled_facets = get_enabled_facets()
    active_facets = get_active_facets(day)

    total_prompts = sum(len(prompts) for prompts in priority_groups.values())
    num_groups = len(priority_groups)
    _update_status(
        mode=target_schedule,
        day=day,
        stream=stream,
        agents_total=total_prompts,
        agents_completed=0,
        current_agents=[],
    )

    logging.info(
        f"Running {total_prompts} prompts for {day} in {num_groups} priority groups"
    )

    emit(
        "started",
        mode=target_schedule,
        day=day,
        count=total_prompts,
        groups=num_groups,
    )

    start_time = time.time()
    total_success = 0
    total_failed = 0
    all_failed_names: list[str] = []
    applicable_units: set[tuple[str, str | None]] = set()
    already_complete_skips = 0
    deterministic_skips = 0

    # Process each priority group in order
    for priority in sorted(priority_groups.keys()):
        prompts_list = priority_groups[priority]
        _update_status(current_group_priority=priority)
        logging.info(f"Starting priority {priority} ({len(prompts_list)} prompts)")

        emit(
            "group_started",
            mode=target_schedule,
            day=day,
            priority=priority,
            count=len(prompts_list),
        )
        _jsonl_log(
            "group.start",
            mode=target_schedule,
            day=day,
            priority=priority,
            count=len(prompts_list),
        )

        spawned: list[
            tuple[str, str, dict, str | None]
        ] = []  # (use_id, name, config, facet)
        group_success = 0
        group_failed = 0

        for prompt_name, config in prompts_list:
            is_generate = config["type"] == "generate"

            # Check exclude_streams filter
            exclude_patterns = config.get("exclude_streams")
            if exclude_patterns and stream:
                if any(fnmatch.fnmatch(stream, pat) for pat in exclude_patterns):
                    logging.info(
                        f"Skipping {prompt_name}: stream '{stream}' matches exclude_streams"
                    )
                    _log_skip(
                        prompt_name,
                        "stream_excluded",
                        f"stream '{stream}' matches exclude_streams",
                        mode=target_schedule,
                        day=day,
                    )
                    continue

            try:
                if config.get("multi_facet"):
                    always_run = config.get("always", False)

                    for facet_name in enabled_facets.keys():
                        if not always_run and facet_name not in active_facets:
                            logging.info(
                                f"Skipping {prompt_name} for {facet_name}: "
                                f"no activity on {day_formatted}"
                            )
                            _log_skip(
                                prompt_name,
                                "no_active_facets",
                                f"no activity on {iso_date(day)}",
                                mode=target_schedule,
                                day=day,
                                facet=facet_name,
                            )
                            continue

                        applicable_units.add((prompt_name, facet_name))
                        skip, reason = _check_daily_skip(
                            prompt_name,
                            facet_name,
                            mode=target_schedule,
                            completed=completed_units,
                            deterministic_failures=deterministic_failures,
                            retry_on_deterministic_failure=config.get(
                                "retry_on_deterministic_failure", False
                            ),
                            from_scratch=from_scratch,
                        )
                        if skip:
                            if reason == "deterministic_failure_no_retry":
                                failure = deterministic_failures[
                                    (prompt_name, facet_name)
                                ]
                                detail = (
                                    f"{failure.count} same-day deterministic failures "
                                    f"({failure.reason_code}); not re-dispatching"
                                )
                                deterministic_skips += 1
                            else:
                                reason = reason or "already_complete"
                                detail = "unit already complete in health log"
                                already_complete_skips += 1
                            _log_skip(
                                prompt_name,
                                reason,
                                detail,
                                mode=target_schedule,
                                day=day,
                                facet=facet_name,
                            )
                            logging.debug(
                                "Skipping %s for %s: %s",
                                prompt_name,
                                facet_name,
                                reason,
                            )
                            continue

                        logging.info(f"Spawning {prompt_name} for facet: {facet_name}")

                        # Always pass day for instructions.day context
                        request_config: dict = {"facet": facet_name, "day": day}
                        _apply_output_persistence(
                            request_config, config, force_refresh=True
                        )
                        env: dict[str, str] = {
                            "SOL_DAY": day,
                            "SOL_FACET": facet_name,
                        }
                        request_config["env"] = env
                        request_config["schedule"] = target_schedule

                        prompt = (
                            ""
                            if is_generate
                            else f"Processing facet '{facet_name}' for {day_formatted}: {input_summary}. Use get_facet('{facet_name}') to load context."
                        )

                        use_id = _dispatch_cortex_request(
                            prompt=prompt,
                            name=prompt_name,
                            config=request_config,
                        )
                        if use_id is None:
                            _log_skip(
                                prompt_name,
                                "send_failed",
                                f"All cortex request attempts failed for {prompt_name}",
                                mode=target_schedule,
                                day=day,
                                facet=facet_name,
                            )
                            group_failed += 1
                            all_failed_names.append(
                                f"{prompt_name}/{facet_name} (send)"
                            )
                            continue
                        if isinstance(use_id, _NotClaimed):
                            _record_request_lost(
                                prompt_name,
                                use_id.use_id,
                                mode=target_schedule,
                                day=day,
                                facet=facet_name,
                                stream=stream,
                            )
                            group_failed += 1
                            all_failed_names.append(
                                f"{prompt_name}/{facet_name} (request_lost)"
                            )
                            continue
                        spawned.append((use_id, prompt_name, config, facet_name))
                        emit(
                            "talent_started",
                            mode=target_schedule,
                            day=day,
                            name=prompt_name,
                            use_id=use_id,
                            facet=facet_name,
                        )
                        _jsonl_log(
                            "talent.dispatch",
                            mode=target_schedule,
                            day=day,
                            name=prompt_name,
                            use_id=use_id,
                            facet=facet_name,
                        )
                        logging.info(
                            f"Started {prompt_name} for {facet_name} (ID: {use_id})"
                        )

                        # Drain batch when concurrency limit reached
                        if max_concurrency and len(spawned) >= max_concurrency:
                            _update_status(
                                current_agents=[name for _, name, _, _ in spawned]
                            )
                            s, f, fn = _drain_priority_batch(
                                spawned,
                                target_schedule,
                                day,
                                None,
                                stream,
                                timeout,
                            )
                            group_success += s
                            group_failed += f
                            all_failed_names.extend(fn)
                            spawned = []
                            _update_status(
                                agents_completed=total_success
                                + total_failed
                                + group_success
                                + group_failed,
                                current_agents=[],
                            )
                else:
                    # Regular single-instance prompt
                    applicable_units.add((prompt_name, None))
                    skip, reason = _check_daily_skip(
                        prompt_name,
                        None,
                        mode=target_schedule,
                        completed=completed_units,
                        deterministic_failures=deterministic_failures,
                        retry_on_deterministic_failure=config.get(
                            "retry_on_deterministic_failure", False
                        ),
                        from_scratch=from_scratch,
                    )
                    if skip:
                        if reason == "deterministic_failure_no_retry":
                            failure = deterministic_failures[(prompt_name, None)]
                            detail = (
                                f"{failure.count} same-day deterministic failures "
                                f"({failure.reason_code}); not re-dispatching"
                            )
                            deterministic_skips += 1
                        else:
                            reason = reason or "already_complete"
                            detail = "unit already complete in health log"
                            already_complete_skips += 1
                        _log_skip(
                            prompt_name,
                            reason,
                            detail,
                            mode=target_schedule,
                            day=day,
                        )
                        logging.debug("Skipping %s: %s", prompt_name, reason)
                        continue

                    logging.info(f"Spawning {prompt_name}")

                    # Always pass day for instructions.day context
                    request_config: dict = {"day": day}
                    _apply_output_persistence(
                        request_config, config, force_refresh=True
                    )
                    env: dict[str, str] = {"SOL_DAY": day}
                    request_config["env"] = env
                    request_config["schedule"] = target_schedule

                    prompt = (
                        ""
                        if is_generate
                        else f"Running scheduled task for {day_formatted}: {input_summary}."
                    )

                    use_id = _dispatch_cortex_request(
                        prompt=prompt,
                        name=prompt_name,
                        config=request_config,
                    )
                    if use_id is None:
                        _log_skip(
                            prompt_name,
                            "send_failed",
                            f"All cortex request attempts failed for {prompt_name}",
                            mode=target_schedule,
                            day=day,
                        )
                        group_failed += 1
                        all_failed_names.append(f"{prompt_name} (send)")
                        continue
                    if isinstance(use_id, _NotClaimed):
                        _record_request_lost(
                            prompt_name,
                            use_id.use_id,
                            mode=target_schedule,
                            day=day,
                            stream=stream,
                        )
                        group_failed += 1
                        all_failed_names.append(f"{prompt_name} (request_lost)")
                        continue
                    spawned.append((use_id, prompt_name, config, None))
                    emit(
                        "talent_started",
                        mode=target_schedule,
                        day=day,
                        name=prompt_name,
                        use_id=use_id,
                    )
                    _jsonl_log(
                        "talent.dispatch",
                        mode=target_schedule,
                        day=day,
                        name=prompt_name,
                        use_id=use_id,
                    )
                    logging.info(f"Started {prompt_name} (ID: {use_id})")

                    # Drain batch when concurrency limit reached
                    if max_concurrency and len(spawned) >= max_concurrency:
                        _update_status(
                            current_agents=[name for _, name, _, _ in spawned]
                        )
                        s, f, fn = _drain_priority_batch(
                            spawned, target_schedule, day, None, stream, timeout
                        )
                        group_success += s
                        group_failed += f
                        all_failed_names.extend(fn)
                        spawned = []
                        _update_status(
                            agents_completed=total_success
                            + total_failed
                            + group_success
                            + group_failed,
                            current_agents=[],
                        )

            except Exception as e:
                logging.error(f"Failed to spawn {prompt_name}: {e}")
                group_failed += 1
                all_failed_names.append(f"{prompt_name} (spawn)")

        # Drain any remaining agents in this priority group
        _update_status(current_agents=[name for _, name, _, _ in spawned])
        s, f, fn = _drain_priority_batch(
            spawned, target_schedule, day, None, stream, timeout
        )
        group_success += s
        group_failed += f
        all_failed_names.extend(fn)
        _update_status(
            agents_completed=total_success
            + total_failed
            + group_success
            + group_failed,
            current_agents=[],
        )

        total_success += group_success
        total_failed += group_failed

        emit(
            "group_completed",
            mode=target_schedule,
            day=day,
            priority=priority,
            success=group_success,
            failed=group_failed,
        )
        _jsonl_log(
            "group.complete",
            mode=target_schedule,
            day=day,
            priority=priority,
            success=group_success,
            failed=group_failed,
        )

    if already_complete_skips:
        logging.info(
            "Daily idempotency: skipped %d already-complete unit(s)",
            already_complete_skips,
        )
    if deterministic_skips:
        logging.info(
            "Daily idempotency: skipped %d deterministic-failure unit(s) (no retry)",
            deterministic_skips,
        )

    duration_ms = int((time.time() - start_time) * 1000)
    emit(
        "completed",
        mode=target_schedule,
        day=day,
        success=total_success,
        failed=total_failed,
        failed_names=all_failed_names,
        duration_ms=duration_ms,
    )

    logging.info(f"Prompts completed: {total_success} succeeded, {total_failed} failed")
    return (total_success, total_failed, all_failed_names, applicable_units)


def run_weekly_prompts(
    day: str,
    refresh: bool,
    verbose: bool,
    max_concurrency: int = 2,
    stream: str | None = None,
    timeout: int | None = 610,
) -> tuple[int, int, list[str]]:
    """Run all weekly scheduled prompts in priority order.

    Loads all weekly prompts, groups by priority, and executes each group with
    bounded concurrency. Structurally identical to run_daily_prompts but for
    weekly-scheduled agents (e.g., partner profile).

    Args:
        day: Day in YYYYMMDD format (reference day for agent context)
        refresh: Whether to regenerate existing outputs
        verbose: Verbose logging
        max_concurrency: Max agents to run concurrently per priority group.
            0 means unlimited (all agents in a group run in parallel).

    Returns:
        Tuple of (success_count, fail_count, failed_names).
    """
    target_schedule = "weekly"
    owner_tz = get_owner_timezone()
    analysis_dt = datetime.strptime(day, "%Y%m%d")
    week_start = sunday_of_week(analysis_dt, owner_tz)
    weekly_reflection_path = (
        Path(get_journal()) / "reflections" / "weekly" / f"{week_start}.md"
    )

    # Load ALL scheduled prompts (both generators and agents)
    all_prompts = get_talent_configs(schedule=target_schedule)

    if not all_prompts:
        logging.info(f"No prompts found for schedule: {target_schedule}")
        return (0, 0, [])

    # Group prompts by priority
    priority_groups: dict[int, list[tuple[str, dict]]] = {}
    for name, config in all_prompts.items():
        priority = config["priority"]  # Required field, validated by get_talent_configs
        priority_groups.setdefault(priority, []).append((name, config))

    # Pre-compute shared data for multi-facet prompts
    day_formatted = iso_date(day)
    input_summary = day_input_summary(day)
    enabled_facets = get_enabled_facets()
    active_facets = get_active_facets(day)

    total_prompts = sum(len(prompts) for prompts in priority_groups.values())
    num_groups = len(priority_groups)
    _update_status(
        mode=target_schedule,
        day=day,
        stream=stream,
        agents_total=total_prompts,
        agents_completed=0,
        current_agents=[],
    )

    logging.info(
        f"Running {total_prompts} prompts for {day} in {num_groups} priority groups"
    )

    emit(
        "started",
        mode=target_schedule,
        day=day,
        count=total_prompts,
        groups=num_groups,
    )

    start_time = time.time()
    total_success = 0
    total_failed = 0
    all_failed_names: list[str] = []

    # Process each priority group in order
    for priority in sorted(priority_groups.keys()):
        prompts_list = priority_groups[priority]
        _update_status(current_group_priority=priority)
        logging.info(f"Starting priority {priority} ({len(prompts_list)} prompts)")

        emit(
            "group_started",
            mode=target_schedule,
            day=day,
            priority=priority,
            count=len(prompts_list),
        )
        _jsonl_log(
            "group.start",
            mode=target_schedule,
            day=day,
            priority=priority,
            count=len(prompts_list),
        )

        spawned: list[
            tuple[str, str, dict, str | None]
        ] = []  # (use_id, name, config, facet)
        group_success = 0
        group_failed = 0

        for prompt_name, config in prompts_list:
            is_generate = config["type"] == "generate"

            # Check exclude_streams filter
            exclude_patterns = config.get("exclude_streams")
            if exclude_patterns and stream:
                if any(fnmatch.fnmatch(stream, pat) for pat in exclude_patterns):
                    logging.info(
                        f"Skipping {prompt_name}: stream '{stream}' matches exclude_streams"
                    )
                    _log_skip(
                        prompt_name,
                        "stream_excluded",
                        f"stream '{stream}' matches exclude_streams",
                        mode=target_schedule,
                        day=day,
                    )
                    continue

            try:
                if config.get("multi_facet"):
                    always_run = config.get("always", False)

                    for facet_name in enabled_facets.keys():
                        if not always_run and facet_name not in active_facets:
                            logging.info(
                                f"Skipping {prompt_name} for {facet_name}: "
                                f"no activity on {day_formatted}"
                            )
                            _log_skip(
                                prompt_name,
                                "no_active_facets",
                                f"no activity on {iso_date(day)}",
                                mode=target_schedule,
                                day=day,
                                facet=facet_name,
                            )
                            continue

                        logging.info(f"Spawning {prompt_name} for facet: {facet_name}")

                        # Always pass day for instructions.day context
                        request_config: dict = {"facet": facet_name, "day": day}
                        _apply_output_persistence(
                            request_config, config, force_refresh=refresh
                        )
                        env: dict[str, str] = {
                            "SOL_DAY": day,
                            "SOL_FACET": facet_name,
                        }
                        if prompt_name == "weekly_reflection":
                            request_config["day"] = week_start
                            request_config["output"] = "md"
                            request_config["output_path"] = str(weekly_reflection_path)
                            env["SOL_DAY"] = week_start
                        request_config["env"] = env
                        request_config["schedule"] = target_schedule

                        prompt = (
                            ""
                            if is_generate
                            else (
                                f"Processing facet '{facet_name}' for {iso_date(week_start)}: "
                                f"{input_summary}. Use get_facet('{facet_name}') to load context."
                                if prompt_name == "weekly_reflection"
                                else f"Processing facet '{facet_name}' for {day_formatted}: {input_summary}. Use get_facet('{facet_name}') to load context."
                            )
                        )

                        use_id = _dispatch_cortex_request(
                            prompt=prompt,
                            name=prompt_name,
                            config=request_config,
                        )
                        if use_id is None:
                            _log_skip(
                                prompt_name,
                                "send_failed",
                                f"All cortex request attempts failed for {prompt_name}",
                                mode=target_schedule,
                                day=day,
                                facet=facet_name,
                            )
                            group_failed += 1
                            all_failed_names.append(
                                f"{prompt_name}/{facet_name} (send)"
                            )
                            continue
                        if isinstance(use_id, _NotClaimed):
                            _record_request_lost(
                                prompt_name,
                                use_id.use_id,
                                mode=target_schedule,
                                day=day,
                                facet=facet_name,
                                stream=stream,
                            )
                            group_failed += 1
                            all_failed_names.append(
                                f"{prompt_name}/{facet_name} (request_lost)"
                            )
                            continue
                        spawned.append((use_id, prompt_name, config, facet_name))
                        emit(
                            "talent_started",
                            mode=target_schedule,
                            day=day,
                            name=prompt_name,
                            use_id=use_id,
                            facet=facet_name,
                        )
                        _jsonl_log(
                            "talent.dispatch",
                            mode=target_schedule,
                            day=day,
                            name=prompt_name,
                            use_id=use_id,
                            facet=facet_name,
                        )
                        logging.info(
                            f"Started {prompt_name} for {facet_name} (ID: {use_id})"
                        )

                        # Drain batch when concurrency limit reached
                        if max_concurrency and len(spawned) >= max_concurrency:
                            _update_status(
                                current_agents=[name for _, name, _, _ in spawned]
                            )
                            s, f, fn = _drain_priority_batch(
                                spawned,
                                target_schedule,
                                day,
                                None,
                                stream,
                                timeout,
                            )
                            group_success += s
                            group_failed += f
                            all_failed_names.extend(fn)
                            spawned = []
                            _update_status(
                                agents_completed=total_success
                                + total_failed
                                + group_success
                                + group_failed,
                                current_agents=[],
                            )
                else:
                    # Regular single-instance prompt
                    logging.info(f"Spawning {prompt_name}")

                    # Always pass day for instructions.day context
                    request_config: dict = {"day": day}
                    _apply_output_persistence(
                        request_config, config, force_refresh=refresh
                    )
                    env: dict[str, str] = {"SOL_DAY": day}
                    if prompt_name == "weekly_reflection":
                        request_config["day"] = week_start
                        request_config["output"] = "md"
                        request_config["output_path"] = str(weekly_reflection_path)
                        env["SOL_DAY"] = week_start
                    request_config["env"] = env
                    request_config["schedule"] = target_schedule

                    prompt = (
                        ""
                        if is_generate
                        else (
                            f"Running scheduled weekly reflection for {iso_date(week_start)}: {input_summary}."
                            if prompt_name == "weekly_reflection"
                            else f"Running scheduled task for {day_formatted}: {input_summary}."
                        )
                    )

                    use_id = _dispatch_cortex_request(
                        prompt=prompt,
                        name=prompt_name,
                        config=request_config,
                    )
                    if use_id is None:
                        _log_skip(
                            prompt_name,
                            "send_failed",
                            f"All cortex request attempts failed for {prompt_name}",
                            mode=target_schedule,
                            day=day,
                        )
                        group_failed += 1
                        all_failed_names.append(f"{prompt_name} (send)")
                        continue
                    if isinstance(use_id, _NotClaimed):
                        _record_request_lost(
                            prompt_name,
                            use_id.use_id,
                            mode=target_schedule,
                            day=day,
                            stream=stream,
                        )
                        group_failed += 1
                        all_failed_names.append(f"{prompt_name} (request_lost)")
                        continue
                    spawned.append((use_id, prompt_name, config, None))
                    emit(
                        "talent_started",
                        mode=target_schedule,
                        day=day,
                        name=prompt_name,
                        use_id=use_id,
                    )
                    _jsonl_log(
                        "talent.dispatch",
                        mode=target_schedule,
                        day=day,
                        name=prompt_name,
                        use_id=use_id,
                    )
                    logging.info(f"Started {prompt_name} (ID: {use_id})")

                    # Drain batch when concurrency limit reached
                    if max_concurrency and len(spawned) >= max_concurrency:
                        _update_status(
                            current_agents=[name for _, name, _, _ in spawned]
                        )
                        s, f, fn = _drain_priority_batch(
                            spawned, target_schedule, day, None, stream, timeout
                        )
                        group_success += s
                        group_failed += f
                        all_failed_names.extend(fn)
                        spawned = []
                        _update_status(
                            agents_completed=total_success
                            + total_failed
                            + group_success
                            + group_failed,
                            current_agents=[],
                        )

            except Exception as e:
                logging.error(f"Failed to spawn {prompt_name}: {e}")
                group_failed += 1
                all_failed_names.append(f"{prompt_name} (spawn)")

        # Drain any remaining agents in this priority group
        _update_status(current_agents=[name for _, name, _, _ in spawned])
        s, f, fn = _drain_priority_batch(
            spawned, target_schedule, day, None, stream, timeout
        )
        group_success += s
        group_failed += f
        all_failed_names.extend(fn)
        _update_status(
            agents_completed=total_success
            + total_failed
            + group_success
            + group_failed,
            current_agents=[],
        )

        total_success += group_success
        total_failed += group_failed

        emit(
            "group_completed",
            mode=target_schedule,
            day=day,
            priority=priority,
            success=group_success,
            failed=group_failed,
        )
        _jsonl_log(
            "group.complete",
            mode=target_schedule,
            day=day,
            priority=priority,
            success=group_success,
            failed=group_failed,
        )

    duration_ms = int((time.time() - start_time) * 1000)
    emit(
        "completed",
        mode=target_schedule,
        day=day,
        success=total_success,
        failed=total_failed,
        failed_names=all_failed_names,
        duration_ms=duration_ms,
    )

    logging.info(f"Prompts completed: {total_success} succeeded, {total_failed} failed")
    return (total_success, total_failed, all_failed_names)


def run_cadence_prompts(
    day: str,
    refresh: bool,
    verbose: bool,
    max_concurrency: int = 2,
    stream: str | None = None,
    timeout: int | None = 610,
) -> tuple[int, int, list[str]]:
    """Run cadence-scheduled prompts whose completion gate is open."""
    all_prompts = get_talent_configs(schedule="cadence")
    if not all_prompts:
        logging.info("cadence: no cadence talents configured")
        return (0, 0, [])

    cadence_state = load_cadence_state()
    dirty = False
    total_success = 0
    total_failed = 0
    failed_names: list[str] = []
    fired = 0
    skipped = 0

    for name, config in sorted(
        all_prompts.items(), key=lambda item: (item[1]["priority"], item[0])
    ):
        now = now_ms()
        cadence_minutes = config.get("cadence_minutes", 5)
        last = cadence_state.get(name)
        if last is not None and now - last < cadence_minutes * 60_000:
            _log_skip(
                name,
                "interval_not_elapsed",
                f"{(now - last) // 1000}s since last < {cadence_minutes}m",
                mode="cadence",
                day=day,
            )
            skipped += 1
            continue

        since_ms = last or 0
        window = read_completed_since(day, since_ms)
        if not window.segments and not window.activities:
            _log_skip(
                name,
                "no_new_work",
                "no segment/activity completed since last cadence run",
                mode="cadence",
                day=day,
            )
            skipped += 1
            continue

        is_generate = config["type"] == "generate"
        request_config: dict = {
            "day": day,
            "schedule": "cadence",
            "env": {"SOL_DAY": day},
            "cadence_window": {
                "since_ms": since_ms,
                "segments": list(window.segments),
                "activities": list(window.activities),
            },
        }
        _apply_output_persistence(request_config, config, force_refresh=refresh)
        prompt = "" if is_generate else f"Running cadence task for {iso_date(day)}."

        use_id = _dispatch_cortex_request(
            prompt=prompt,
            name=name,
            config=request_config,
        )
        if use_id is None:
            _log_skip(
                name,
                "send_failed",
                f"All cortex request attempts failed for {name}",
                mode="cadence",
                day=day,
            )
            total_failed += 1
            failed_names.append(f"{name} (send)")
            continue
        if isinstance(use_id, _NotClaimed):
            _record_request_lost(
                name,
                use_id.use_id,
                mode="cadence",
                day=day,
                stream=stream,
            )
            total_failed += 1
            failed_names.append(f"{name} (request_lost)")
            continue

        emit("talent_started", mode="cadence", day=day, name=name, use_id=use_id)
        _jsonl_log(
            "talent.dispatch",
            mode="cadence",
            day=day,
            name=name,
            use_id=use_id,
        )
        s, f, fn = _drain_priority_batch(
            [(use_id, name, config, None)], "cadence", day, None, stream, timeout
        )
        total_success += s
        total_failed += f
        failed_names.extend(fn)
        if s == 1 and f == 0:
            cadence_state[name] = now
            dirty = True
            fired += 1

    if dirty:
        save_cadence_state(cadence_state)
    logging.info(
        "cadence: %d fired, %d skipped (no new work or interval), %d failed",
        fired,
        skipped,
        total_failed,
    )
    return (total_success, total_failed, failed_names)


def run_activity_prompts(
    day: str,
    activity_id: str,
    facet: str,
    refresh: bool = False,
    verbose: bool = False,
    max_concurrency: int = 2,
) -> bool:
    """Run activity-scheduled agents for a completed activity.

    Loads the activity record from the journal, filters agents whose
    schedule="activity" and whose 'activities' list matches the activity type
    (or contains "*"), then spawns each matching agent with the activity's
    segment span for transcript loading.

    Args:
        day: Day in YYYYMMDD format
        activity_id: Activity record ID (e.g., "coding_100000_300")
        facet: Facet name
        refresh: Whether to regenerate existing outputs
        verbose: Verbose logging
        max_concurrency: Max agents to run concurrently (0=unlimited)

    Returns:
        True if all agents succeeded, False if any failed
    """
    # Load activity record
    record = get_activity_record(facet, day, activity_id)

    if not record:
        logging.error(
            "Activity record not found: %s in facet '%s' on %s",
            activity_id,
            facet,
            day,
        )
        return False

    activity_type = record.get("activity", "")
    segments = record.get("segments", [])

    if record.get("source") in ("cogitate", "anticipated") or not segments:
        logging.info(
            "Skipping activity-scheduled generators for synthetic activity %s (source=%s)",
            activity_id,
            record.get("source"),
        )
        return True

    # Load activity-scheduled agents
    all_prompts = get_talent_configs(schedule="activity")

    if not all_prompts:
        logging.info("No activity-scheduled agents found")
        return True

    # Filter agents that match this activity type
    matching = {}
    for name, config in all_prompts.items():
        activities_filter = config.get("activities", [])
        if "*" in activities_filter or activity_type in activities_filter:
            matching[name] = config

    if not matching:
        logging.info(
            "No agents match activity type '%s' (checked %d agents)",
            activity_type,
            len(all_prompts),
        )
        return True

    # Group by priority
    priority_groups: dict[int, list[tuple[str, dict]]] = {}
    for name, config in matching.items():
        priority = config["priority"]
        priority_groups.setdefault(priority, []).append((name, config))

    total_prompts = sum(len(p) for p in priority_groups.values())
    num_groups = len(priority_groups)
    _update_status(
        mode="activity",
        day=day,
        activity=activity_id,
        facet=facet,
        agents_total=total_prompts,
        agents_completed=0,
        current_agents=[],
    )

    logging.info(
        "Running %d activity agents for %s (type=%s, %d segments) in %d groups",
        total_prompts,
        activity_id,
        activity_type,
        len(segments),
        num_groups,
    )

    emit(
        "started",
        mode="activity",
        day=day,
        activity=activity_id,
        facet=facet,
        count=total_prompts,
        groups=num_groups,
    )

    start_time = time.time()
    total_success = 0
    total_failed = 0

    day_formatted = iso_date(day)

    for priority in sorted(priority_groups.keys()):
        prompts_list = priority_groups[priority]
        _update_status(current_group_priority=priority)
        logging.info(f"Starting priority {priority} ({len(prompts_list)} agents)")

        emit(
            "group_started",
            mode="activity",
            day=day,
            activity=activity_id,
            facet=facet,
            priority=priority,
            count=len(prompts_list),
        )
        _jsonl_log(
            "group.start",
            mode="activity",
            day=day,
            activity=activity_id,
            facet=facet,
            priority=priority,
            count=len(prompts_list),
        )

        spawned: list[tuple[str, str, dict]] = []  # (use_id, name, config)
        group_success = 0
        group_failed = 0

        def _drain_activity_batch() -> None:
            """Wait for current batch of spawned activity agents."""
            nonlocal spawned, group_success, group_failed
            if not spawned:
                return

            agent_ids = [aid for aid, _, _ in spawned]
            logging.info(f"Waiting for {len(agent_ids)} agents...")

            completed, timed_out = wait_for_uses(agent_ids, timeout=610)

            if timed_out:
                logging.warning(f"{len(timed_out)} agents timed out")
                group_failed += len(timed_out)
                for use_id in timed_out:
                    timed_name = next(
                        (n for aid, n, _ in spawned if aid == use_id), "unknown"
                    )
                    state = _classify_timeout_state(use_id)
                    emit(
                        "talent_completed",
                        mode="activity",
                        day=day,
                        activity=activity_id,
                        facet=facet,
                        name=timed_name,
                        use_id=use_id,
                        state=state,
                    )
                    _jsonl_log(
                        "talent.fail",
                        mode="activity",
                        day=day,
                        activity=activity_id,
                        facet=facet,
                        name=timed_name,
                        use_id=use_id,
                        state=state,
                        **_provider_model_fields(use_id),
                    )

            for use_id, prompt_name, config in spawned:
                if use_id in timed_out:
                    continue

                end_state = completed.get(use_id, "unknown")
                if end_state == "finish":
                    finish_fields = _cache_fields(use_id)
                    logging.info(f"{prompt_name} completed successfully")
                    group_success += 1

                    # Incremental indexing for generators (skip JSON)
                    is_generate = config["type"] == "generate"
                    output_format = config.get("output", "md")
                    if is_generate and output_format != "json":
                        output_path = get_activity_output_path(
                            facet,
                            day,
                            activity_id,
                            prompt_name,
                            output_format=output_format,
                        )
                        _maybe_rescan_output(
                            output_path,
                            finish_fields["output_changed"],
                            day,
                        )
                else:
                    finish_fields = {}
                    logging.error(f"{prompt_name} ended with state: {end_state}")
                    group_failed += 1

                emit(
                    "talent_completed",
                    mode="activity",
                    day=day,
                    activity=activity_id,
                    facet=facet,
                    name=prompt_name,
                    use_id=use_id,
                    state=end_state,
                )
                _jsonl_log(
                    "talent.complete" if end_state == "finish" else "talent.fail",
                    mode="activity",
                    day=day,
                    activity=activity_id,
                    facet=facet,
                    name=prompt_name,
                    use_id=use_id,
                    state=end_state,
                    **(
                        _provider_model_fields(use_id)
                        if end_state != "finish"
                        else _cache_terminal_fields(finish_fields)
                    ),
                )

            spawned = []

        for prompt_name, config in prompts_list:
            is_generate = config["type"] == "generate"

            try:
                logging.info(f"Spawning {prompt_name} for activity {activity_id}")

                if prompt_name == "work" and activity_type in ("browsing", "reading"):
                    level_avg = float(record.get("level_avg", 0.0) or 0.0)
                    if level_avg < 0.4:
                        logging.info(
                            "skipping work talent for low-level %s activity %s (level_avg=%.2f)",
                            activity_type,
                            record.get("id"),
                            level_avg,
                        )
                        continue

                output_format = config.get("output", "md")
                request_config: dict = {
                    "facet": facet,
                    "day": day,
                    "span": segments,
                    "activity": record,
                    "output_path": str(
                        get_activity_output_path(
                            facet,
                            day,
                            activity_id,
                            prompt_name,
                            output_format=output_format,
                        )
                    ),
                    "env": {
                        "SOL_DAY": day,
                        "SOL_FACET": facet,
                        "SOL_ACTIVITY": activity_id,
                    },
                }
                request_config["schedule"] = "activity"
                if is_generate:
                    request_config["output"] = output_format
                    if refresh:
                        request_config["refresh"] = True

                prompt = (
                    ""
                    if is_generate
                    else f"Processing activity '{activity_id}' ({activity_type}) in facet '{facet}' for {day_formatted}."
                )

                use_id = _dispatch_cortex_request(
                    prompt=prompt,
                    name=prompt_name,
                    config=request_config,
                )
                if use_id is None:
                    _log_skip(
                        prompt_name,
                        "send_failed",
                        f"All cortex request attempts failed for {prompt_name}",
                        mode="activity",
                        day=day,
                        activity=activity_id,
                        facet=facet,
                    )
                    total_failed += 1
                    continue
                if isinstance(use_id, _NotClaimed):
                    _record_request_lost(
                        prompt_name,
                        use_id.use_id,
                        mode="activity",
                        day=day,
                        activity=activity_id,
                        facet=facet,
                    )
                    total_failed += 1
                    continue
                spawned.append((use_id, prompt_name, config))
                emit(
                    "talent_started",
                    mode="activity",
                    day=day,
                    activity=activity_id,
                    facet=facet,
                    name=prompt_name,
                    use_id=use_id,
                )
                _jsonl_log(
                    "talent.dispatch",
                    mode="activity",
                    day=day,
                    activity=activity_id,
                    facet=facet,
                    name=prompt_name,
                    use_id=use_id,
                )
                logging.info(f"Started {prompt_name} (ID: {use_id})")

                # Drain batch when concurrency limit reached
                if max_concurrency and len(spawned) >= max_concurrency:
                    _update_status(current_agents=[name for _, name, _ in spawned])
                    _drain_activity_batch()
                    _update_status(
                        agents_completed=total_success
                        + total_failed
                        + group_success
                        + group_failed,
                        current_agents=[],
                    )

            except Exception as e:
                logging.error(f"Failed to spawn {prompt_name}: {e}")
                total_failed += 1

        # Drain any remaining agents
        _update_status(current_agents=[name for _, name, _ in spawned])
        _drain_activity_batch()
        _update_status(
            agents_completed=total_success
            + total_failed
            + group_success
            + group_failed,
            current_agents=[],
        )

        total_success += group_success
        total_failed += group_failed

        emit(
            "group_completed",
            mode="activity",
            day=day,
            activity=activity_id,
            facet=facet,
            priority=priority,
            success=group_success,
            failed=group_failed,
        )
        _jsonl_log(
            "group.complete",
            mode="activity",
            day=day,
            activity=activity_id,
            facet=facet,
            priority=priority,
            success=group_success,
            failed=group_failed,
        )

    duration_ms = int((time.time() - start_time) * 1000)
    emit(
        "completed",
        mode="activity",
        day=day,
        activity=activity_id,
        facet=facet,
        success=total_success,
        failed=total_failed,
        duration_ms=duration_ms,
    )

    logging.info(
        f"Activity agents completed: {total_success} succeeded, {total_failed} failed"
    )

    msg = f"think --activity {activity_id}"
    if total_failed:
        msg += f" failed={total_failed}"
    day_log(day, msg)

    return total_failed == 0


def run_flush_prompts(
    day: str,
    segment: str,
    verbose: bool,
    stream: str | None = None,
) -> bool:
    """Run flush hooks for segment agents that declare flush support.

    Triggered by supervisor when no new segments arrive after a timeout.
    Only runs agents with hook.flush=true, passing flush=True so their
    pre-hooks can close out dangling state.

    Args:
        day: Day in YYYYMMDD format
        segment: Last observed segment key
        verbose: Verbose logging

    Returns:
        True if all flush agents succeeded, False if any failed
    """
    all_prompts = get_talent_configs(schedule="segment")

    # Filter to only agents with flush hooks
    flush_prompts = {
        name: config
        for name, config in all_prompts.items()
        if isinstance(config.get("hook"), dict) and config["hook"].get("flush")
    }

    if not flush_prompts:
        logging.info("No flush-eligible agents found")
        return True

    logging.info(
        f"Flushing {len(flush_prompts)} agents for {day}/{segment}: "
        f"{', '.join(flush_prompts.keys())}"
    )

    emit("started", mode="flush", day=day, segment=segment, count=len(flush_prompts))
    start_time = time.time()
    total_success = 0
    total_failed = 0

    spawned: list[tuple[str, str, dict]] = []  # (use_id, name, config)
    _update_status(
        mode="flush",
        day=day,
        segment=segment,
        stream=stream,
        agents_total=len(flush_prompts),
        agents_completed=0,
        current_agents=[],
    )

    for prompt_name, config in flush_prompts.items():
        is_generate = config["type"] == "generate"

        try:
            env: dict[str, str] = {
                "SOL_SEGMENT": segment,
                "SOL_DAY": day,
            }
            if stream:
                env["SOL_STREAM"] = stream
            request_config: dict = {
                "day": day,
                "segment": segment,
                "flush": True,
                "refresh": True,
                "env": env,
            }
            if stream:
                request_config["stream"] = stream
            request_config["schedule"] = "segment"
            if is_generate:
                request_config["output"] = config.get("output", "md")

            use_id = _dispatch_cortex_request(
                prompt="",
                name=prompt_name,
                config=request_config,
            )
            if use_id is None:
                _log_skip(
                    prompt_name,
                    "send_failed",
                    f"All cortex request attempts failed for {prompt_name}",
                    mode="flush",
                    day=day,
                    segment=segment,
                )
                total_failed += 1
                continue
            if isinstance(use_id, _NotClaimed):
                _record_request_lost(
                    prompt_name,
                    use_id.use_id,
                    mode="flush",
                    day=day,
                    segment=segment,
                )
                total_failed += 1
                continue
            spawned.append((use_id, prompt_name, config))
            emit(
                "talent_started",
                mode="flush",
                day=day,
                segment=segment,
                name=prompt_name,
                use_id=use_id,
            )
            _jsonl_log(
                "talent.dispatch",
                mode="flush",
                day=day,
                segment=segment,
                name=prompt_name,
                use_id=use_id,
            )
            logging.info(f"Started flush agent {prompt_name} (ID: {use_id})")

        except Exception as e:
            logging.error(f"Failed to spawn flush agent {prompt_name}: {e}")
            total_failed += 1

    if spawned:
        _update_status(current_agents=[name for _, name, _ in spawned])
        agent_ids = [aid for aid, _, _ in spawned]
        completed, timed_out = wait_for_uses(agent_ids, timeout=610)

        if timed_out:
            logging.warning(f"Flush: {len(timed_out)} agents timed out")
            total_failed += len(timed_out)
            for use_id in timed_out:
                timed_name = next(
                    (n for aid, n, _ in spawned if aid == use_id), "unknown"
                )
                state = _classify_timeout_state(use_id)
                _jsonl_log(
                    "talent.fail",
                    mode="flush",
                    day=day,
                    segment=segment,
                    name=timed_name,
                    use_id=use_id,
                    state=state,
                    **_provider_model_fields(use_id),
                )

        for use_id, prompt_name, config in spawned:
            if use_id in timed_out:
                continue
            end_state = completed.get(use_id, "unknown")
            if end_state == "finish":
                logging.info(f"Flush agent {prompt_name} completed")
                total_success += 1
            else:
                logging.error(
                    f"Flush agent {prompt_name} ended with state: {end_state}"
                )
                total_failed += 1

            emit(
                "talent_completed",
                mode="flush",
                day=day,
                segment=segment,
                name=prompt_name,
                use_id=use_id,
                state=end_state,
            )
            _jsonl_log(
                "talent.complete" if end_state == "finish" else "talent.fail",
                mode="flush",
                day=day,
                segment=segment,
                name=prompt_name,
                use_id=use_id,
                state=end_state,
                **(_provider_model_fields(use_id) if end_state != "finish" else {}),
            )
        _update_status(
            agents_completed=total_success + total_failed,
            current_agents=[],
        )
    if not spawned and total_failed:
        _update_status(agents_completed=total_failed, current_agents=[])

    duration_ms = int((time.time() - start_time) * 1000)
    emit(
        "completed",
        mode="flush",
        day=day,
        segment=segment,
        success=total_success,
        failed=total_failed,
        duration_ms=duration_ms,
    )

    logging.info(
        f"Flush completed in {duration_ms}ms: "
        f"{total_success} succeeded, {total_failed} failed"
    )

    msg = f"think --flush {segment}"
    if total_failed:
        msg += f" failed={total_failed}"
    day_log(day, msg)

    return total_failed == 0


def dry_run(
    day: str,
    *,
    segment: str | None = None,
    segments: bool = False,
    facet: str | None = None,
    activity: str | None = None,
    flush: bool = False,
    refresh: bool = False,
    stream: str | None = None,
    weekly: bool = False,
    cadence: bool = False,
) -> None:
    """Print what think would execute without spawning any agents."""
    day_formatted = iso_date(day)

    def _print_segment_orchestrator(
        prompts: dict[str, dict], target_segment: str | None
    ) -> None:
        print("Sense orchestrator (linear):")
        sense_cfg = prompts.get("sense")
        step = 1
        if sense_cfg:
            status = _output_status(
                day,
                "sense",
                target_segment,
                sense_cfg.get("output", "json"),
                stream=stream,
            )
            print(
                f"  {step}. sense (gen/{sense_cfg.get('output', 'json')}){status} — mandatory"
            )
            step += 1

        for name, label in [
            ("entities", "always for non-idle"),
            ("timeline:segment_summary", "always for non-idle"),
            ("screen", "if recommend.screen_record"),
            (
                "speaker_attribution",
                "if recommend.speaker_attribution + audio embeddings",
            ),
        ]:
            cfg = prompts.get(name)
            if not cfg:
                continue
            is_gen = cfg["type"] == "generate"
            type_label = "gen" if is_gen else "cog"
            fmt = cfg.get("output", "md") if is_gen else cfg.get("output", "")
            status = _output_status(
                day,
                name,
                target_segment,
                cfg.get("output") if is_gen else None,
                stream=stream,
            )
            print(f"  {step}. {name} ({type_label}/{fmt}){status} — {label}")
            step += 1
        print()
        print("  idle segments: write stubs + early return (unless --refresh)")
        print("  activity state machine: updates per segment")

    if activity:
        _dry_run_activity(day, day_formatted, activity, facet or "", refresh)
        return

    if flush:
        _dry_run_flush(day, segment or "")
        return

    if weekly:
        all_prompts = get_talent_configs(schedule="weekly")
        print(f"Day {day_formatted} — weekly agents\n")
        if not all_prompts:
            print("No prompts for schedule: weekly")
        else:
            _print_prompt_table(all_prompts, day, refresh=refresh, stream=stream)
        return

    if cadence:
        all_prompts = get_talent_configs(schedule="cadence")
        print(f"Day {day_formatted} — cadence agents\n")
        if not all_prompts:
            print("No prompts for schedule: cadence")
            return
        cadence_state = load_cadence_state()
        now = now_ms()
        for name, config in sorted(
            all_prompts.items(), key=lambda item: (item[1]["priority"], item[0])
        ):
            cadence_minutes = config.get("cadence_minutes", 5)
            last = cadence_state.get(name)
            if last is not None and now - last < cadence_minutes * 60_000:
                print(
                    f"  skip  {name} — interval not elapsed "
                    f"({(now - last) // 1000}s < {cadence_minutes}m)"
                )
                continue
            window = read_completed_since(day, last or 0)
            count = len(window.segments) + len(window.activities)
            if count == 0:
                print(f"  no-op {name} — no new work since last cadence run")
            else:
                print(
                    f"  fire  {name} — window: {len(window.segments)} segment(s), "
                    f"{len(window.activities)} activity(ies)"
                )
        return

    if segments:
        segs = cluster_segments(day)
        if not segs:
            print(f"No segments found for {day}")
            return
        print(f"Day {day_formatted} — re-process {len(segs)} segments\n")
        for i, seg in enumerate(segs, 1):
            seg_key = seg["key"]
            seg_stream = seg.get("stream")
            label = f"  [{i}/{len(segs)}] {seg_key} ({seg['start']}-{seg['end']})"
            if seg_stream:
                label += f" stream={seg_stream}"
            print(label)
        print()
        all_prompts = get_talent_configs(schedule="segment")
        if all_prompts:
            _print_segment_orchestrator(all_prompts, "<each>")
        return

    # Default: full daily or segment run
    target_schedule = "segment" if segment else "daily"
    all_prompts = get_talent_configs(schedule=target_schedule)

    header = f"Day {day_formatted}"
    if segment:
        header += f" segment {segment}"
    if refresh:
        header += " (refresh)"
    print(header + "\n")

    if not segment:
        print(
            "Pre-phase:  journal sense --day "
            + day
            + " -j "
            + str(fanout_policy.default_describe_jobs())
        )

    if not all_prompts:
        print(f"No prompts for schedule: {target_schedule}")
    elif segment:
        _print_segment_orchestrator(all_prompts, segment)
    else:
        _print_prompt_table(
            all_prompts, day, segment=segment, refresh=refresh, stream=stream
        )

    if not segment:
        print("Post-phase: journal indexer --rescan")
        print("Post-phase: journal journal-stats")


def _print_prompt_table(
    prompts: dict[str, dict],
    day: str,
    *,
    segment: str | None = None,
    refresh: bool = False,
    stream: str | None = None,
) -> None:
    """Print a grouped-by-priority table of prompts."""
    enabled_facets = get_enabled_facets()

    if segment and segment != "<each>":
        active_facets = set(
            f
            for f in load_segment_facets(day, segment, stream=stream)
            if f in enabled_facets
        )
    else:
        active_facets = get_active_facets(day)

    # Group by priority
    groups: dict[int, list[tuple[str, dict]]] = {}
    for name, config in prompts.items():
        pri = config["priority"]
        groups.setdefault(pri, []).append((name, config))

    total = 0
    for priority in sorted(groups.keys()):
        items = groups[priority]
        print(f"Priority {priority}:")
        for name, config in items:
            is_gen = config["type"] == "generate"
            type_label = "gen" if is_gen else "agent"
            output_fmt = config.get("output", "md") if is_gen else None

            if config.get("multi_facet"):
                always = config.get("always", False)
                target_facets = [
                    f for f in enabled_facets if always or f in active_facets
                ]
                skipped = [f for f in enabled_facets if f not in target_facets]
                for f in target_facets:
                    status = (
                        _output_status(
                            day, name, segment, output_fmt, facet=f, stream=stream
                        )
                        if is_gen
                        else ""
                    )
                    print(f"  {type_label}  {name}/{f}{status}")
                    total += 1
                if skipped:
                    print(f"  skip {name} — no activity: {', '.join(skipped)}")
            else:
                status = (
                    _output_status(day, name, segment, output_fmt, stream=stream)
                    if is_gen
                    else ""
                )
                print(f"  {type_label}  {name}{status}")
                total += 1
        print()

    print(f"Total: {total} agents")


def _output_status(
    day: str,
    name: str,
    segment: str | None,
    output_format: str | None,
    *,
    facet: str | None = None,
    stream: str | None = None,
) -> str:
    """Return a short status suffix for a generator output file."""
    if segment == "<each>":
        return ""
    path = get_output_path(
        day_path(day),
        name,
        segment=segment,
        output_format=output_format,
        facet=facet,
        stream=stream,
    )
    if path.exists():
        return " (exists)"
    return " (new)"


def _dry_run_activity(
    day: str, day_formatted: str, activity_id: str, facet: str, refresh: bool
) -> None:
    """Dry-run for --activity mode."""
    records = load_activity_records(facet, day)
    record = next((r for r in records if r.get("id") == activity_id), None)

    if not record:
        print(f"Activity not found: {activity_id} in facet '{facet}' on {day}")
        return

    activity_type = record.get("activity", "")
    segments = record.get("segments", [])

    print(
        f"Day {day_formatted} --activity {activity_id} --facet {facet}"
        + (" (refresh)" if refresh else "")
        + "\n"
    )
    print(f"  type:     {activity_type}")
    print(f"  segments: {len(segments)}")

    all_prompts = get_talent_configs(schedule="activity")
    matching = {
        n: c
        for n, c in all_prompts.items()
        if "*" in c.get("activities", []) or activity_type in c.get("activities", [])
    }

    if not matching:
        print(f"\n  No agents match activity type '{activity_type}'")
        return

    groups: dict[int, list[tuple[str, dict]]] = {}
    for n, c in matching.items():
        groups.setdefault(c["priority"], []).append((n, c))

    print()
    total = 0
    for priority in sorted(groups.keys()):
        items = groups[priority]
        print(f"Priority {priority}:")
        for n, c in items:
            is_gen = c["type"] == "generate"
            type_label = "gen" if is_gen else "agent"
            output_fmt = c.get("output", "md") if is_gen else None
            status = ""
            if is_gen:
                path = get_activity_output_path(
                    facet, day, activity_id, n, output_format=output_fmt
                )
                status = " (exists)" if path.exists() else " (new)"
            print(f"  {type_label}  {n}{status}")
            total += 1
        print()

    print(f"Total: {total} agents")


def _dry_run_flush(day: str, segment: str) -> None:
    """Dry-run for --flush mode."""
    all_prompts = get_talent_configs(schedule="segment")
    flush_prompts = {
        n: c
        for n, c in all_prompts.items()
        if isinstance(c.get("hook"), dict) and c["hook"].get("flush")
    }

    day_formatted = iso_date(day)
    print(f"Day {day_formatted} --flush segment {segment}\n")

    if not flush_prompts:
        print("  No flush-eligible agents")
        return

    for n, c in flush_prompts.items():
        type_label = "gen" if c["type"] == "generate" else "agent"
        print(f"  {type_label}  {n}")

    print(f"\nTotal: {len(flush_prompts)} agents")


def parse_args() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Run processing tasks on a journal day or segment"
    )
    parser.add_argument(
        "--day",
        help="Day folder in YYYYMMDD format (defaults to yesterday, or today with --cadence)",
    )
    parser.add_argument(
        "--segment",
        help="Segment key in HHMMSS_LEN format (processes segment agents only)",
    )
    parser.add_argument(
        "--refresh", action="store_true", help="Refresh existing outputs"
    )
    parser.add_argument(
        "--from-scratch",
        action="store_true",
        help="Re-run segment and daily units that already completed",
    )
    parser.add_argument(
        "--segments",
        action="store_true",
        help="Re-process all segments for the day (incompatible with --segment, --facet)",
    )
    parser.add_argument(
        "--facet",
        metavar="NAME",
        help="Target a specific facet (only used with --activity)",
    )
    parser.add_argument(
        "--activity",
        metavar="ID",
        help="Run activity-scheduled agents for a completed activity record (requires --facet and --day)",
    )
    parser.add_argument(
        "--stream",
        help="Stream name (e.g., 'archon', 'import.apple'). Passed to agents as SOL_STREAM env var.",
    )
    parser.add_argument(
        "--flush",
        action="store_true",
        help="Run flush hooks on segment agents to close out dangling state (requires --segment)",
    )
    parser.add_argument(
        "-j",
        "--jobs",
        type=int,
        default=2,
        metavar="N",
        help="Max concurrent agents per priority group (0=unlimited, default: 2)",
    )
    parser.add_argument(
        "--no-timeout",
        action="store_true",
        help="Disable per-batch agent wait timeout in --segments mode",
    )
    parser.add_argument(
        "--segment-workers",
        type=int,
        default=None,
        metavar="N",
        help=(
            "Max concurrent segment repair workers in --segments mode "
            "(default: half CPU capped at 8; valid 1-32)"
        ),
    )
    parser.add_argument(
        "--no-activity-prompts",
        action="store_true",
        help=(
            "Write realized activity records but skip per-activity cogitate runs "
            '(schedule="activity" talents). Used by realizer backfill to write '
            "activity records cheaply without firing per-activity prompts. "
            "Incompatible with --activity."
        ),
    )
    parser.add_argument(
        "--skip-talents",
        type=str,
        default="",
        help=(
            "Comma-separated segment-scheduled talent names to suppress during "
            "--segments/--segment runs (e.g., 'screen,speaker_attribution' for "
            "realizer-backfill speedup). Recognized: sense, entities, documents, "
            "screen, speaker_attribution. Skipping 'sense' "
            "relies on a cached talents/sense.json from a prior run."
        ),
    )
    parser.add_argument(
        "--live",
        action="store_true",
        help=(
            "Mark this run as a live, current-segment think (observe-triggered "
            "for a segment that just completed live observation). Talents "
            "declaring new_only in their frontmatter run ONLY when --live is "
            "set; without it the run is treated as historical/batch "
            "re-processing and new_only talents are skipped. Defaults off so "
            "manually re-thinking an old segment never rebuilds rolling state "
            "from stale data."
        ),
    )
    parser.add_argument(
        "--updated",
        action="store_true",
        help="List days with pending daily processing and exit",
    )
    parser.add_argument(
        "--weekly",
        action="store_true",
        help="Run weekly-scheduled agents (incompatible with --segment, --segments, --activity, --flush)",
    )
    parser.add_argument(
        "--cadence",
        action="store_true",
        help="Run cadence-scheduled agents on completed segments/activities (incompatible with --segment, --segments, --activity, --flush, --weekly)",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Show what would run without executing anything",
    )
    return parser


def main() -> None:
    global _callosum, _jsonl

    parser = parse_args()
    args = setup_cli(parser)
    require_solstone()

    from solstone.think.identity import ensure_identity_directory

    ensure_identity_directory()

    if args.updated:
        incompatible = []
        if args.day:
            incompatible.append("--day")
        if args.segment:
            incompatible.append("--segment")
        if args.facet:
            incompatible.append("--facet")
        if args.activity:
            incompatible.append("--activity")
        if args.flush:
            incompatible.append("--flush")
        if args.segments:
            incompatible.append("--segments")
        if args.cadence:
            incompatible.append("--cadence")
        if incompatible:
            parser.error(f"--updated is incompatible with {', '.join(incompatible)}")
        today = date.today().strftime("%Y%m%d")
        for d in updated_days(exclude={today}):
            print(d)
        sys.exit(0)

    day = args.day
    if day is None:
        day = (
            date.today().strftime("%Y%m%d")
            if args.cadence
            else (datetime.now() - timedelta(days=1)).strftime("%Y%m%d")
        )
    day_dir = day_path(day)

    if not day_dir.is_dir():
        parser.error(f"Day folder not found: {day_dir}")

    if args.facet and not args.activity:
        parser.error("--facet requires --activity")

    if args.activity and not args.facet:
        parser.error("--activity requires --facet")

    if args.activity and not args.day:
        parser.error("--activity requires --day")

    if args.no_activity_prompts and args.activity:
        parser.error("--no-activity-prompts cannot be combined with --activity")

    if args.segment_workers is not None and not (
        1 <= args.segment_workers <= SEGMENT_WORKERS_MAX
    ):
        parser.error(f"--segment-workers must be between 1 and {SEGMENT_WORKERS_MAX}")

    skip_talents: frozenset[str] = frozenset(
        name.strip() for name in (args.skip_talents or "").split(",") if name.strip()
    )

    if args.activity and (args.segment or args.segments or args.flush):
        parser.error(
            "--activity is incompatible with --segment, --segments, and --flush"
        )

    if args.flush and not args.segment:
        parser.error("--flush requires --segment")

    if args.flush and (args.segments or args.refresh):
        parser.error("--flush is incompatible with --segments and --refresh")

    if args.segments and (args.segment or args.facet):
        parser.error("--segments is incompatible with --segment and --facet")

    if args.weekly and (args.segment or args.segments or args.activity or args.flush):
        parser.error(
            "--weekly is incompatible with --segment, --segments, --activity, and --flush"
        )

    if args.cadence and (
        args.segment or args.segments or args.activity or args.flush or args.weekly
    ):
        parser.error(
            "--cadence is incompatible with --segment, --segments, --activity, --flush, and --weekly"
        )

    if args.segments:
        segment_workers = (
            args.segment_workers or fanout_policy.default_segment_workers()
        )
        if args.jobs == 0 and segment_workers > 1:
            parser.error(
                "--jobs 0 is incompatible with multi-worker --segments; "
                "set --jobs to a positive bound or --segment-workers 1"
            )

    if args.dry_run:
        dry_run(
            day,
            segment=args.segment,
            segments=args.segments,
            facet=args.facet,
            activity=args.activity,
            flush=args.flush,
            refresh=args.refresh,
            stream=args.stream,
            weekly=args.weekly,
            cadence=args.cadence,
        )
        sys.exit(0)

    if args.activity:
        _run_mode = "activity"
    elif args.flush:
        _run_mode = "flush"
    elif args.segments:
        _run_mode = "segment"
    elif args.weekly:
        _run_mode = "weekly"
    elif args.cadence:
        _run_mode = "cadence"
    elif args.segment:
        _run_mode = "segment"
    else:
        _run_mode = "daily"

    if args.cadence and not get_talent_configs(schedule="cadence"):
        logging.info("cadence: no cadence talents configured")
        sys.exit(0)

    _run_ref = str(now_ms())
    _run_start_time = time.time()
    _run_result = {"success": 0, "failed": 0}
    jsonl_path = str(day_path(day) / "health" / f"{_run_ref}_{_run_mode}.jsonl")
    _jsonl = ThinkingJSONLWriter(jsonl_path)

    # Start callosum connection
    _callosum = CallosumConnection(defaults={"rev": get_rev()})
    _callosum.start()
    _stop_status.clear()
    status_thread = threading.Thread(target=_emit_periodic_status, daemon=True)
    status_thread.start()
    _jsonl_log("run.start", mode=_run_mode, day=day, ref=_run_ref)

    try:
        # Handle activity-triggered execution mode
        if args.activity:
            success = run_activity_prompts(
                day=day,
                activity_id=args.activity,
                facet=args.facet,
                refresh=args.refresh,
                verbose=args.verbose,
                max_concurrency=args.jobs,
            )
            _run_result["success"] = 1 if success else 0
            _run_result["failed"] = 0 if success else 1
            sys.exit(0 if success else 1)

        # Handle flush mode
        if args.flush:
            if not check_callosum_available():
                logging.warning("Callosum socket not found - prompts may fail to spawn")
            success = run_flush_prompts(
                day=day,
                segment=args.segment,
                verbose=args.verbose,
                stream=args.stream,
            )
            _run_result["success"] = 1 if success else 0
            _run_result["failed"] = 0 if success else 1
            sys.exit(0 if success else 1)

        # Handle batch segment re-processing mode
        if args.segments:
            if not check_callosum_available():
                logging.warning("Callosum socket not found - prompts may fail to spawn")

            segments = cluster_segments(day)
            if not segments:
                logging.info(f"No segments found for {day}")
                sys.exit(0)

            total = len(segments)
            batch_start = time.time()
            force_all_segments = args.refresh or args.from_scratch
            try:
                selected_segments, repair_counts = _select_segment_repair_targets(
                    day,
                    segments,
                    force_all=force_all_segments,
                )
            except Exception:
                logging.exception("Failed to select segment repair targets for %s", day)
                _run_result["failed"] = 1
                sys.exit(1)

            selected = len(selected_segments)
            segment_workers = (
                args.segment_workers or fanout_policy.default_segment_workers()
            )
            logging.info(
                "Segment repair targets for %s: selected=%d complete=%d "
                "raw_blocked=%d total=%d workers=%d jobs=%s",
                day,
                selected,
                repair_counts["complete"],
                repair_counts["raw_blocked"],
                total,
                min(segment_workers, max(selected, 1)),
                args.jobs,
            )
            emit(
                "segments_started",
                day=day,
                count=selected,
                total=total,
                complete=repair_counts["complete"],
                raw_blocked=repair_counts["raw_blocked"],
                workers=min(segment_workers, max(selected, 1)),
            )
            _update_status(segments_total=selected, segments_completed=0)

            if selected == 0:
                duration_ms = int((time.time() - batch_start) * 1000)
                if repair_counts["raw_blocked"]:
                    logging.info(
                        "No runnable segment repair targets for %s: "
                        "%d complete, %d raw-media-blocked, %d total",
                        day,
                        repair_counts["complete"],
                        repair_counts["raw_blocked"],
                        total,
                    )
                else:
                    logging.info(
                        "All %d segment(s) already fully thought for %s", total, day
                    )
                emit(
                    "segments_completed",
                    day=day,
                    count=0,
                    total=total,
                    success=0,
                    failed=0,
                    duration_ms=duration_ms,
                )
                day_log(
                    day,
                    "think --segments selected=0 "
                    f"complete={repair_counts['complete']} "
                    f"raw_blocked={repair_counts['raw_blocked']} failed=0",
                )
                _run_result["success"] = 0
                _run_result["failed"] = 0
                sys.exit(0)

            batch_success, batch_failed = _run_segment_repair_batch(
                day=day,
                segments=selected_segments,
                refresh=args.refresh,
                verbose=args.verbose,
                max_concurrency=args.jobs,
                segment_workers=segment_workers,
                timeout=None if args.no_timeout else 610,
                skip_activity_prompts=args.no_activity_prompts,
                skip_talents=skip_talents,
            )

            _replay_activity_state_for_segments(
                day=day,
                segments=segments,
                refresh=args.refresh,
                verbose=args.verbose,
                max_concurrency=args.jobs,
                skip_activity_prompts=args.no_activity_prompts,
            )

            duration_ms = int((time.time() - batch_start) * 1000)
            logging.info(
                f"All segments completed in {duration_ms}ms: "
                f"{batch_success} succeeded, {batch_failed} failed across "
                f"{selected}/{total} selected segments"
            )
            emit(
                "segments_completed",
                day=day,
                count=selected,
                total=total,
                success=batch_success,
                failed=batch_failed,
                complete=repair_counts["complete"],
                raw_blocked=repair_counts["raw_blocked"],
                duration_ms=duration_ms,
            )

            if args.refresh:
                day_log(
                    day,
                    f"think --segments --refresh selected={selected} failed={batch_failed}",
                )
            elif args.from_scratch:
                day_log(
                    day,
                    "think --segments --from-scratch "
                    f"selected={selected} failed={batch_failed}",
                )
            else:
                day_log(
                    day,
                    f"think --segments selected={selected} "
                    f"complete={repair_counts['complete']} "
                    f"raw_blocked={repair_counts['raw_blocked']} failed={batch_failed}",
                )

            _run_result["success"] = batch_success
            _run_result["failed"] = batch_failed
            if batch_failed > 0:
                sys.exit(1)
            sys.exit(0)

        # Check callosum availability
        if not check_callosum_available():
            logging.warning("Callosum socket not found - prompts may fail to spawn")

        start_time = time.time()

        # Handle weekly mode — dispatch weekly agents, no pre/post phases
        if args.weekly:
            success_count, fail_count, failed_names = run_weekly_prompts(
                day=day,
                refresh=args.refresh,
                verbose=args.verbose,
                max_concurrency=args.jobs,
                stream=args.stream,
            )

            duration_ms = int((time.time() - start_time) * 1000)
            logging.info(
                f"Weekly think completed in {duration_ms}ms: "
                f"{success_count} succeeded, {fail_count} failed"
            )
            day_log(day, f"think --weekly failed={fail_count}")
            _run_result["success"] = success_count
            _run_result["failed"] = fail_count

            if fail_count > 0:
                names = ", ".join(failed_names)
                logging.error(f"{fail_count} weekly prompt(s) failed: {names}")
                sys.exit(1)
            sys.exit(0)

        # Handle cadence mode — dispatch only agents whose completion gate is open
        if args.cadence:
            success_count, fail_count, failed_names = run_cadence_prompts(
                day=day,
                refresh=args.refresh,
                verbose=args.verbose,
                max_concurrency=args.jobs,
                stream=args.stream,
            )

            duration_ms = int((time.time() - start_time) * 1000)
            logging.info(
                f"Cadence think completed in {duration_ms}ms: "
                f"{success_count} succeeded, {fail_count} failed"
            )
            day_log(day, f"think --cadence failed={fail_count}")
            _run_result["success"] = success_count
            _run_result["failed"] = fail_count

            if fail_count > 0:
                names = ", ".join(failed_names)
                logging.error(f"{fail_count} cadence prompt(s) failed: {names}")
                sys.exit(1)
            sys.exit(0)

        blocked_before_cycle = None
        blocked_after_cycle = None
        cleared_pre = None
        remaining_pre = None

        def _blocked_keys(day: str) -> set[tuple[str | None, str]] | None:
            try:
                return blocked_segment_keys(
                    cluster_segments(day),
                    read_segment_progress(day),
                )
            except Exception:
                logging.warning(
                    "Failed to measure blocked segment keys for %s", day, exc_info=True
                )
                return None

        def _yield_counts(
            before: set[tuple[str | None, str]] | None,
            after: set[tuple[str | None, str]] | None,
        ) -> tuple[int | None, int | None]:
            if before is None or after is None:
                return None, None
            return len(before - after), len(after)

        def _format_segment_repair_yield(
            cleared: int | None, remaining: int | None
        ) -> str:
            if cleared is None and remaining is None:
                return "unknown"
            cleared_text = str(cleared) if cleared is not None else "unknown"
            remaining_text = str(remaining) if remaining is not None else "unknown"
            return f"{cleared_text} cleared, {remaining_text} remaining"

        # PRE-PHASE: Run sense repair (daily only)
        if not args.segment:
            blocked_before_cycle = _blocked_keys(day)
            logging.info("Running pre-phase: sense repair")
            cmd = [
                "journal",
                "sense",
                "--day",
                day,
                "-j",
                str(fanout_policy.default_describe_jobs()),
            ]
            if args.verbose:
                cmd.append("-v")
            day_log(day, f"starting: {' '.join(cmd)}")
            _jsonl_log("phase.start", mode=_run_mode, day=day, phase="sense_repair")
            _phase_start = time.time()
            phase_ok, phase_timed_out = run_bounded_phase(
                cmd, day, DEFAULT_TASK_MAX_RUNTIME
            )
            phase_complete = {
                "mode": _run_mode,
                "day": day,
                "phase": "sense_repair",
                "success": phase_ok,
                "duration_ms": int((time.time() - _phase_start) * 1000),
            }
            if phase_timed_out:
                phase_complete.update(
                    reason_code="wall_clock_exceeded",
                    timeout_seconds=DEFAULT_TASK_MAX_RUNTIME,
                    bounded=True,
                )
            _jsonl_log("phase.complete", **phase_complete)
            if not phase_ok:
                if phase_timed_out:
                    logging.warning(
                        "Sense repair exceeded its %ss budget, continuing anyway",
                        DEFAULT_TASK_MAX_RUNTIME,
                    )
                else:
                    logging.warning("Sense repair failed, continuing anyway")

        # PRE-PHASE: Run segment-think batch repair (daily only)
        if not args.segment:
            logging.info("Running pre-phase: segment-think repair")
            cmd = ["journal", "think", "--segments", "--day", day]
            if args.verbose:
                cmd.append("-v")
            if args.from_scratch:
                cmd.append("--refresh")
            day_log(day, f"starting: {' '.join(cmd)}")
            _jsonl_log("phase.start", mode=_run_mode, day=day, phase="segment_think")
            _phase_start = time.time()
            record_segment_repair_attempt(day, started_at=_phase_start)
            blocked_before_pre = _blocked_keys(day)
            phase_ok, phase_timed_out = run_bounded_phase(
                cmd, day, DEFAULT_TASK_MAX_RUNTIME
            )
            blocked_after_pre = _blocked_keys(day)
            cleared_pre, remaining_pre = _yield_counts(
                blocked_before_pre, blocked_after_pre
            )
            phase_complete = {
                "mode": _run_mode,
                "day": day,
                "phase": "segment_think",
                "success": phase_ok,
                "duration_ms": int((time.time() - _phase_start) * 1000),
            }
            if phase_timed_out:
                phase_complete.update(
                    reason_code="wall_clock_exceeded",
                    timeout_seconds=DEFAULT_TASK_MAX_RUNTIME,
                    bounded=True,
                )
            if cleared_pre is not None:
                phase_complete["cleared"] = cleared_pre
            if remaining_pre is not None:
                phase_complete["remaining"] = remaining_pre
            _jsonl_log("phase.complete", **phase_complete)
            record_segment_repair_outcome(
                day,
                success=phase_ok,
                timed_out=phase_timed_out,
                timeout_seconds=DEFAULT_TASK_MAX_RUNTIME,
                ended_at=time.time(),
                cleared=cleared_pre,
                remaining=remaining_pre,
            )
            if not phase_ok:
                if phase_timed_out:
                    logging.warning(
                        "Segment-think repair exceeded its %ss budget, continuing anyway (yield: %s)",
                        DEFAULT_TASK_MAX_RUNTIME,
                        _format_segment_repair_yield(cleared_pre, remaining_pre),
                    )
                else:
                    logging.warning("Segment-think repair failed, continuing anyway")

        # MAIN PHASE: Run prompts
        resolved_stream = args.stream
        if args.segment and args.stream is None:
            matches = [(s, k) for s, k, _ in iter_segments(day) if k == args.segment]
            if not matches:
                parser.error(
                    f"Segment {args.segment} not found in any stream under {day_dir}"
                )
            resolved_stream = matches[0][0]

        if args.segment:
            success_count, fail_count, failed_names = run_segment_sense(
                day=day,
                segment=args.segment,
                refresh=args.refresh,
                verbose=args.verbose,
                max_concurrency=args.jobs,
                stream=resolved_stream,
                timeout=None if args.no_timeout else 610,
                state_machine=ActivityStateMachine(journal_root=Path(get_journal())),
                skip_activity_prompts=args.no_activity_prompts,
                skip_talents=skip_talents,
                live=args.live,
                predecessor=resolve_predecessor(day, resolved_stream, args.segment),
            )
        else:
            success_count, fail_count, failed_names, applicable_units = (
                run_daily_prompts(
                    day=day,
                    verbose=args.verbose,
                    max_concurrency=args.jobs,
                    stream=resolved_stream,
                    from_scratch=args.from_scratch,
                )
            )
        _run_result["success"] = success_count
        _run_result["failed"] = fail_count

        # POST-PHASE: Final indexing and stats (daily only)
        if not args.segment:
            logging.info("Running post-phase: indexer rescan")
            rescan_cmd = ["journal", "indexer", "--rescan"]
            if args.verbose:
                rescan_cmd.append("--verbose")
            _jsonl_log("phase.start", mode=_run_mode, day=day, phase="indexer_rescan")
            _phase_start = time.time()
            rescan_ok = run_queued_command(rescan_cmd, day, timeout=3600)
            _jsonl_log(
                "phase.complete",
                mode=_run_mode,
                day=day,
                phase="indexer_rescan",
                success=rescan_ok,
                duration_ms=int((time.time() - _phase_start) * 1000),
            )

            logging.info("Running post-phase: journal stats")
            stats_cmd = ["journal", "journal-stats"]
            if args.verbose:
                stats_cmd.append("--verbose")
            _jsonl_log("phase.start", mode=_run_mode, day=day, phase="journal_stats")
            _phase_start = time.time()
            stats_ok, stats_timed_out = run_bounded_phase(
                stats_cmd, day, JOURNAL_STATS_MAX_RUNTIME
            )
            phase_complete = {
                "mode": _run_mode,
                "day": day,
                "phase": "journal_stats",
                "success": stats_ok,
                "duration_ms": int((time.time() - _phase_start) * 1000),
            }
            if stats_timed_out:
                phase_complete.update(
                    reason_code="wall_clock_exceeded",
                    timeout_seconds=JOURNAL_STATS_MAX_RUNTIME,
                    bounded=True,
                )
            _jsonl_log("phase.complete", **phase_complete)
            if not stats_ok:
                if stats_timed_out:
                    logging.warning(
                        "Journal stats exceeded its %ss budget, continuing anyway",
                        JOURNAL_STATS_MAX_RUNTIME,
                    )
                else:
                    logging.warning("Journal stats failed, continuing anyway")

            # Check storage health and emit warnings
            try:
                from solstone.think.callosum import callosum_send
                from solstone.think.retention import (
                    check_storage_health,
                    compute_storage_summary,
                )

                storage_summary = compute_storage_summary()
                journal_path = get_journal()
                storage_warnings = check_storage_health(storage_summary, journal_path)
                for warning in storage_warnings:
                    callosum_send(
                        "storage",
                        "warning",
                        level=warning["level"],
                        type=warning["type"],
                        message=warning["message"],
                        current=warning["current"],
                        threshold=warning["threshold"],
                    )
                if storage_warnings:
                    callosum_send(
                        "notification",
                        "show",
                        title="Storage Warning",
                        message=storage_warnings[0]["message"],
                        action="/app/settings#storage",
                    )
            except Exception:
                logging.debug(
                    "Storage health check failed in post-phase", exc_info=True
                )

            # Touch daily.updated marker only after daily and segment work completes.
            completion_payload_fragment: dict[str, object] = {}
            try:
                completed = read_completed_units(day)
                deterministic_failures = read_daily_deterministic_failures(day)

                segments = cluster_segments(day)
                progress = read_segment_progress(day)
                completion = classify_segment_completion(segments, progress)
                blocked_after_cycle = blocked_segment_keys(segments, progress)
                verdict = evaluate_daily_completion(
                    applicable_units,
                    completed,
                    deterministic_failures,
                    completion.blockers,
                )
                completion_payload_fragment = finalize_day_completion(day, verdict)
            except Exception:
                logging.warning("Failed to update daily marker", exc_info=True)

            cleared_cycle, remaining_cycle = _yield_counts(
                blocked_before_cycle, blocked_after_cycle
            )
            daily_complete_payload = {
                "day": day,
                "success": success_count,
                "failed": fail_count,
                "duration_ms": int((time.time() - start_time) * 1000),
            }
            if cleared_cycle is not None and remaining_cycle is not None:
                daily_complete_payload["cleared"] = cleared_cycle
                record_daily_catchup_progress(
                    day, cleared=cleared_cycle, remaining=remaining_cycle
                )
            if remaining_cycle is not None:
                daily_complete_payload["remaining"] = remaining_cycle
            if completion_payload_fragment:
                daily_complete_payload.update(completion_payload_fragment)

            # Set first_daily_ready awareness flag after first daily analysis
            try:
                from solstone.think.awareness import get_current, update_state

                cur = get_current()
                if not cur.get("journal", {}).get("first_daily_ready"):
                    update_state(
                        "journal",
                        {
                            "first_daily_ready": True,
                            "first_daily_ready_at": datetime.now().strftime(
                                "%Y%m%dT%H:%M:%S"
                            ),
                        },
                    )
            except Exception:
                pass

            # Notify supervisor that daily think processing is complete
            emit(
                "daily_complete",
                **daily_complete_payload,
            )

        segment_repair_suffix = ""
        if not args.segment:
            segment_repair_suffix = (
                "; segment repairs: "
                f"{_format_segment_repair_yield(cleared_pre, remaining_pre)}"
            )

        # Build log message
        msg = "think"
        if args.refresh:
            msg += " --refresh"
        if fail_count:
            msg += f" failed={fail_count}"
        msg += segment_repair_suffix
        day_log(day, msg)

        duration_ms = int((time.time() - start_time) * 1000)
        logging.info(
            f"Think completed in {duration_ms}ms: "
            f"{success_count} succeeded, {fail_count} failed{segment_repair_suffix}"
        )

        if fail_count > 0:
            names = ", ".join(failed_names)
            logging.error(f"{fail_count} prompt(s) failed: {names}")
            sys.exit(1)

    finally:
        _clear_status()
        _stop_status.set()
        status_thread.join(timeout=2)
        _run_duration_ms = int((time.time() - _run_start_time) * 1000)
        _jsonl_log(
            "run.complete",
            mode=_run_mode,
            day=day,
            ref=_run_ref,
            success=_run_result["success"],
            failed=_run_result["failed"],
            skipped=_jsonl.skip_count if _jsonl else 0,
            duration_ms=_run_duration_ms,
        )
        if _jsonl:
            _jsonl.close()
            _jsonl = None
        if _callosum:
            _callosum.stop()


if __name__ == "__main__":
    main()
