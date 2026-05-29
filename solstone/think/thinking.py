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
import sys
import threading
import time
from datetime import date, datetime, timedelta
from pathlib import Path

from solstone.think.activities import (
    append_activity_record,
    get_activity_output_path,
    get_activity_record,
    load_activity_records,
)
from solstone.think.activity_state_machine import ActivityStateMachine
from solstone.think.callosum import CallosumConnection
from solstone.think.cluster import cluster_segments
from solstone.think.cortex_client import (
    CortexSpawnUnavailable,
    cortex_request,
    wait_for_uses,
)
from solstone.think.facets import (
    get_active_facets,
    get_enabled_facets,
    load_segment_facets,
)
from solstone.think.pipeline_health import read_completed_units
from solstone.think.runner import run_task
from solstone.think.sense_splitter import write_idle_stubs, write_sense_outputs
from solstone.think.talent import get_output_path, get_talent_configs
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
            try:
                self.file.close()
            except OSError as exc:
                logging.warning(
                    "Failed to close think JSONL sidecar %s: %s", self.file.name, exc
                )


_jsonl: ThinkingJSONLWriter | None = None


def _jsonl_log(event: str, **fields) -> None:
    """Write a JSONL event if the writer is active."""
    if _jsonl:
        _jsonl.log(event, **fields)


def _log_skip(name: str, reason: str, detail: str, **extra) -> None:
    """Emit an talent.skip JSONL event."""
    _jsonl_log("talent.skip", name=name, reason=reason, detail=detail, **extra)


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


def run_command(cmd: list[str], day: str) -> bool:
    """Run a shell command synchronously."""
    logging.info("==> %s", " ".join(cmd))
    cmd_name = cmd[1] if cmd[0] in ("sol", "journal") and len(cmd) > 1 else cmd[0]
    cmd_name = cmd_name.replace("-", "_")

    try:
        success, exit_code, _log_path = run_task(cmd, day=day)
        if not success:
            logging.error(
                "Command failed with exit code %s: %s", exit_code, " ".join(cmd)
            )
            day_log(day, f"{cmd_name} error {exit_code}")
            return False
        return True
    except Exception as e:
        logging.error("Command exception: %s: %s", e, " ".join(cmd))
        day_log(day, f"{cmd_name} exception")
        return False


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
NEVER_SKIP_DAILY = frozenset({"pulse", "awareness_tender"})
_SEND_RETRY_DELAYS = (0.5, 1.0)  # seconds between retries (3 attempts total)


def _cortex_request_with_retry(**kwargs) -> str | None:
    """Call cortex_request with retries on Callosum send failure.

    Retries up to len(_SEND_RETRY_DELAYS) times with short sleeps in between.
    Returns the use_id on success, or None if all attempts failed.
    """
    try:
        use_id = cortex_request(**kwargs)
    except CortexSpawnUnavailable as exc:
        logging.info("cortex_request unavailable: %s", exc.detail or "unknown")
        return None
    if use_id is not None:
        return use_id

    name = kwargs.get("name", "unknown")
    for i, delay in enumerate(_SEND_RETRY_DELAYS, 1):
        logging.warning("Retrying cortex request for '%s' (attempt %d)", name, i + 1)
        time.sleep(delay)
        try:
            use_id = cortex_request(**kwargs)
        except CortexSpawnUnavailable as exc:
            logging.info("cortex_request unavailable: %s", exc.detail or "unknown")
            return None
        if use_id is not None:
            return use_id

    logging.error("All cortex request attempts failed for '%s'", name)
    return None


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
        failed_names contains descriptions like "digest (error)" or
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
            failed_names.append(f"{label} (timeout)")
            emit(
                "talent_completed",
                mode=target_schedule,
                day=day,
                segment=segment,
                name=timed_name,
                use_id=use_id,
                state="timeout",
                **({"facet": timed_facet} if timed_facet else {}),
            )
            _jsonl_log(
                "talent.fail",
                mode=target_schedule,
                day=day,
                segment=segment,
                name=timed_name,
                use_id=use_id,
                state="timeout",
                **({"facet": timed_facet} if timed_facet else {}),
            )

    for use_id, prompt_name, config, agent_facet in spawned:
        if use_id in timed_out:
            continue

        end_state = completed.get(use_id, "unknown")
        if end_state == "finish":
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
                **({"facet": agent_facet} if agent_facet else {}),
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

                if output_path.exists():
                    logging.debug(f"Indexing {output_path}")
                    run_queued_command(
                        ["sol", "indexer", "--rescan-file", str(output_path)],
                        day,
                        timeout=60,
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
                **({"facet": agent_facet} if agent_facet else {}),
            )

    return (success, failed, failed_names)


def _segment_dir(day: str, segment: str, stream: str | None) -> Path:
    """Return the expected segment directory without creating it."""
    return day_path(day) / (stream or "default") / segment


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


def _write_json_atomic(path: Path, data: object) -> None:
    """Atomically write JSON data to a file."""
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(f"{path.suffix}.tmp")
    tmp.write_text(json.dumps(data), encoding="utf-8")
    tmp.replace(path)


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
    never_skip: frozenset[str],
) -> tuple[bool, str | None]:
    if mode != "daily":
        return (False, None)
    if name in never_skip:
        return (False, None)
    if (mode, name, facet) in completed:
        return (True, "already_complete")
    return (False, None)


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
) -> tuple[int, int, list[str]]:
    """Run Sense-first linear orchestrator for a single segment.

    Dispatches the Sense agent first, parses its output to determine segment
    density and conditional agent recommendations, then dispatches remaining
    agents based on Sense output.
    """
    target_schedule = "segment"
    all_prompts = get_talent_configs(schedule="segment")
    if not all_prompts:
        logging.info("No prompts found for schedule: segment")
        return (0, 0, [])

    def _cfg(name: str) -> dict | None:
        return all_prompts.get(name)

    def _dispatch_agent(name: str, config: dict) -> str | None | object:
        if name in skip_talents:
            _log_skip(
                name,
                "skip_talents_flag",
                "Skipped by --skip-talents",
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
        return _cortex_request_with_retry(
            prompt=prompt, name=name, config=request_config
        )

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
        )
        return (0, 1, ["sense (not_configured)"])

    day_dir = day_path(day)
    seg_dir = _segment_dir(day, segment, stream)
    pulse_config = _cfg("pulse")

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

    write_sense_outputs(sense_json, seg_dir, stream=stream)
    density = sense_json["density"]
    _jsonl_log(
        "sense.complete",
        mode=target_schedule,
        day=day,
        segment=segment,
        density=density,
        recommend=sense_json.get("recommend") or {},
    )

    if density == "idle" and not refresh:
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
            completed_lookup = {}
            for rec in state_machine.get_completed_activities():
                completed_lookup.setdefault(rec["id"], rec)
            for activity_id, facet, change in ended_triples:
                _jsonl_log(
                    "activity.detected",
                    mode=target_schedule,
                    day=day,
                    segment=segment,
                    activity=str(activity_id),
                    facet=str(facet),
                    state="ended",
                    change=change,
                )
                rec = completed_lookup.get(activity_id)
                if rec:
                    append_activity_record(facet, routing_day, rec)
                    _jsonl_log(
                        "activity.persisted",
                        mode=target_schedule,
                        day=day,
                        segment=segment,
                        activity=str(activity_id),
                        facet=str(facet),
                        change=change,
                    )
            # Run activity agents for completed activities
            for activity_id, facet, _change in ended_triples:
                logging.info(
                    "Activity completed (idle): %s facet=%s, running activity agents",
                    activity_id,
                    facet,
                )
                if skip_activity_prompts:
                    _jsonl_log(
                        "activity.prompts_skipped",
                        day=day,
                        segment=segment,
                        activity=str(activity_id),
                        facet=str(facet),
                        mode=target_schedule,
                        reason="--no-activity-prompts",
                    )
                    continue
                run_activity_prompts(
                    day=routing_day,
                    activity_id=str(activity_id),
                    facet=str(facet),
                    refresh=refresh,
                    verbose=verbose,
                    max_concurrency=max_concurrency,
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
                    _write_json_atomic(
                        state_machine.journal_root
                        / "awareness"
                        / "activity_state.json",
                        snapshot,
                    )
                except Exception:
                    logging.debug(
                        "Failed to write activity state snapshot", exc_info=True
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

    entities_config = _cfg("entities")
    if entities_config:
        agents_to_run.append(("entities", entities_config))
    else:
        _log_skip(
            "entities",
            "no_config",
            "entities config not found",
            mode=target_schedule,
            day=day,
            segment=segment,
        )

    documents_config = _cfg("documents")
    if documents_config:
        agents_to_run.append(("documents", documents_config))
    else:
        _log_skip(
            "documents",
            "no_config",
            "documents config not found",
            mode=target_schedule,
            day=day,
            segment=segment,
        )

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

    total_expected = 1 + len(agents_to_run)
    if recommend.get("pulse_update") and pulse_config:
        total_expected += 1
    _update_status(agents_total=total_expected)

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

    if state_machine is not None:
        routing_day = state_machine.last_segment_day or day
        changes = state_machine.update(sense_json, segment, day)
        # Persist completed activity records before running activity agents
        ended_triples = [
            (c["id"], c["facet"], c.get("_change"))
            for c in changes
            if c.get("state") == "ended"
        ]
        completed_lookup = {}
        for rec in state_machine.get_completed_activities():
            completed_lookup.setdefault(rec["id"], rec)
        for activity_id, facet, change in ended_triples:
            _jsonl_log(
                "activity.detected",
                mode=target_schedule,
                day=day,
                segment=segment,
                activity=str(activity_id),
                facet=str(facet),
                state="ended",
                change=change,
            )
            rec = completed_lookup.get(activity_id)
            if rec:
                append_activity_record(facet, routing_day, rec)
                _jsonl_log(
                    "activity.persisted",
                    mode=target_schedule,
                    day=day,
                    segment=segment,
                    activity=str(activity_id),
                    facet=str(facet),
                    change=change,
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
                _write_json_atomic(
                    state_machine.journal_root / "awareness" / "activity_state.json",
                    snapshot,
                )
            except Exception:
                logging.debug("Failed to write activity state snapshot", exc_info=True)
        for change in changes:
            if change.get("state") != "ended":
                continue
            facet = change.get("facet")
            activity_id = change.get("id")
            if not facet or not activity_id:
                continue
            logging.info(
                "Activity completed: %s facet=%s, running activity agents",
                activity_id,
                facet,
            )
            if skip_activity_prompts:
                _jsonl_log(
                    "activity.prompts_skipped",
                    day=day,
                    segment=segment,
                    activity=str(activity_id),
                    facet=str(facet),
                    mode=target_schedule,
                    reason="--no-activity-prompts",
                )
                continue
            run_activity_prompts(
                day=routing_day,
                activity_id=str(activity_id),
                facet=str(facet),
                refresh=refresh,
                verbose=verbose,
                max_concurrency=max_concurrency,
            )

    awareness_tender_config = _cfg("awareness_tender")
    if awareness_tender_config:
        at_agent_id = _dispatch_agent("awareness_tender", awareness_tender_config)
        if at_agent_id is None:
            _log_skip(
                "awareness_tender",
                "send_failed",
                "All cortex request attempts failed for awareness_tender",
                mode=target_schedule,
                day=day,
                segment=segment,
            )
            total_failed += 1
            all_failed_names.append("awareness_tender (send)")
            _update_status(agents_completed=total_success + total_failed)
        elif at_agent_id is not _SKIPPED:
            emit(
                "talent_started",
                mode=target_schedule,
                day=day,
                segment=segment,
                name="awareness_tender",
                use_id=at_agent_id,
            )
            _jsonl_log(
                "talent.dispatch",
                mode=target_schedule,
                day=day,
                segment=segment,
                name="awareness_tender",
                use_id=at_agent_id,
            )
            _update_status(current_agents=["awareness_tender"])
            s, f, fn = _drain_priority_batch(
                [(at_agent_id, "awareness_tender", awareness_tender_config, None)],
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

    if recommend.get("pulse_update") and pulse_config:
        pulse_agent_id = _dispatch_agent("pulse", pulse_config)
        if pulse_agent_id is None:
            _log_skip(
                "pulse",
                "send_failed",
                "All cortex request attempts failed for pulse",
                mode=target_schedule,
                day=day,
                segment=segment,
            )
            total_failed += 1
            all_failed_names.append("pulse (send)")
            _update_status(agents_completed=total_success + total_failed)
        elif pulse_agent_id is not _SKIPPED:
            emit(
                "talent_started",
                mode=target_schedule,
                day=day,
                segment=segment,
                name="pulse",
                use_id=pulse_agent_id,
            )
            _jsonl_log(
                "talent.dispatch",
                mode=target_schedule,
                day=day,
                segment=segment,
                name="pulse",
                use_id=pulse_agent_id,
            )
            _update_status(current_agents=["pulse"])
            s, f, fn = _drain_priority_batch(
                [(pulse_agent_id, "pulse", pulse_config, None)],
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
    elif not recommend.get("pulse_update"):
        _log_skip(
            "pulse",
            "not_recommended",
            "pulse_update not recommended by sense",
            mode=target_schedule,
            day=day,
            segment=segment,
        )
    elif not pulse_config:
        _log_skip(
            "pulse",
            "no_config",
            "pulse config not found",
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


def run_daily_prompts(
    day: str,
    verbose: bool,
    max_concurrency: int = 2,
    stream: str | None = None,
    timeout: int | None = 610,
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
        failed_names contains descriptions like "digest (error)" and
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
                            never_skip=NEVER_SKIP_DAILY,
                        )
                        if skip:
                            reason = reason or "already_complete"
                            _log_skip(
                                prompt_name,
                                reason,
                                "unit already complete in health log",
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
                            already_complete_skips += 1
                            continue

                        logging.info(f"Spawning {prompt_name} for facet: {facet_name}")

                        # Always pass day for instructions.day context
                        request_config: dict = {"facet": facet_name, "day": day}
                        if is_generate:
                            request_config["output"] = config.get("output", "md")
                            request_config["refresh"] = True
                        elif config.get("output"):
                            # Cogitate agents with explicit output get auto-persisted
                            request_config["output"] = config["output"]
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

                        use_id = _cortex_request_with_retry(
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
                        never_skip=NEVER_SKIP_DAILY,
                    )
                    if skip:
                        reason = reason or "already_complete"
                        _log_skip(
                            prompt_name,
                            reason,
                            "unit already complete in health log",
                            mode=target_schedule,
                            day=day,
                        )
                        logging.debug("Skipping %s: %s", prompt_name, reason)
                        already_complete_skips += 1
                        continue

                    logging.info(f"Spawning {prompt_name}")

                    # Always pass day for instructions.day context
                    request_config: dict = {"day": day}
                    if is_generate:
                        request_config["output"] = config.get("output", "md")
                        request_config["refresh"] = True
                    env: dict[str, str] = {"SOL_DAY": day}
                    request_config["env"] = env
                    request_config["schedule"] = target_schedule

                    prompt = (
                        ""
                        if is_generate
                        else f"Running scheduled task for {day_formatted}: {input_summary}."
                    )

                    use_id = _cortex_request_with_retry(
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
                        if is_generate:
                            request_config["output"] = config.get("output", "md")
                            if refresh:
                                request_config["refresh"] = True
                        elif config.get("output"):
                            # Cogitate agents with explicit output get auto-persisted
                            request_config["output"] = config["output"]
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

                        use_id = _cortex_request_with_retry(
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
                    if is_generate:
                        request_config["output"] = config.get("output", "md")
                        if refresh:
                            request_config["refresh"] = True
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

                    use_id = _cortex_request_with_retry(
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
                    emit(
                        "talent_completed",
                        mode="activity",
                        day=day,
                        activity=activity_id,
                        facet=facet,
                        name=timed_name,
                        use_id=use_id,
                        state="timeout",
                    )
                    _jsonl_log(
                        "talent.fail",
                        mode="activity",
                        day=day,
                        activity=activity_id,
                        facet=facet,
                        name=timed_name,
                        use_id=use_id,
                        state="timeout",
                    )

            for use_id, prompt_name, config in spawned:
                if use_id in timed_out:
                    continue

                end_state = completed.get(use_id, "unknown")
                if end_state == "finish":
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
                        if output_path.exists():
                            logging.debug(f"Indexing {output_path}")
                            run_queued_command(
                                ["sol", "indexer", "--rescan-file", str(output_path)],
                                day,
                                timeout=60,
                            )
                else:
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

                use_id = _cortex_request_with_retry(
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

            use_id = _cortex_request_with_retry(
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
                _jsonl_log(
                    "talent.fail",
                    mode="flush",
                    day=day,
                    segment=segment,
                    name=timed_name,
                    use_id=use_id,
                    state="timeout",
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
            ("screen", "if recommend.screen_record"),
            (
                "speaker_attribution",
                "if recommend.speaker_attribution + audio embeddings",
            ),
            ("pulse", "if recommend.pulse_update"),
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
        print("Pre-phase:  journal sense --day " + day)

    if not all_prompts:
        print(f"No prompts for schedule: {target_schedule}")
    elif segment:
        _print_segment_orchestrator(all_prompts, segment)
    else:
        _print_prompt_table(
            all_prompts, day, segment=segment, refresh=refresh, stream=stream
        )

    if not segment:
        print("Post-phase: sol indexer --rescan")
        print("Post-phase: sol journal-stats")


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
        help="Day folder in YYYYMMDD format (defaults to yesterday)",
    )
    parser.add_argument(
        "--segment",
        help="Segment key in HHMMSS_LEN format (processes segment agents only)",
    )
    parser.add_argument(
        "--refresh", action="store_true", help="Refresh existing outputs"
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
            "--segments/--segment runs (e.g., 'awareness_tender,pulse' for "
            "realizer-backfill speedup). Recognized: sense, entities, documents, "
            "screen, speaker_attribution, awareness_tender, pulse. Skipping 'sense' "
            "relies on a cached talents/sense.json from a prior run."
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
        if incompatible:
            parser.error(f"--updated is incompatible with {', '.join(incompatible)}")
        today = date.today().strftime("%Y%m%d")
        for d in updated_days(exclude={today}):
            print(d)
        sys.exit(0)

    day = args.day
    if day is None:
        day = (datetime.now() - timedelta(days=1)).strftime("%Y%m%d")
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
    elif args.segment:
        _run_mode = "segment"
    else:
        _run_mode = "daily"

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
            logging.info(f"Processing {total} segments for {day}")
            emit("segments_started", day=day, count=total)
            _update_status(segments_total=total, segments_completed=0)

            batch_start = time.time()
            batch_success = 0
            batch_failed = 0
            batch_state_machine = ActivityStateMachine()

            for i, seg in enumerate(segments, 1):
                seg_key = seg["key"]
                seg_stream = seg.get("stream")
                logging.info(
                    f"Processing segment {i}/{total}: {seg_key} ({seg['start']}-{seg['end']})"
                )
                try:
                    success, failed, _fn = run_segment_sense(
                        day=day,
                        segment=seg_key,
                        refresh=args.refresh,
                        verbose=args.verbose,
                        max_concurrency=args.jobs,
                        stream=seg_stream,
                        timeout=None if args.no_timeout else 610,
                        state_machine=batch_state_machine,
                        skip_activity_prompts=args.no_activity_prompts,
                        skip_talents=skip_talents,
                    )
                    # Touch stream.updated marker after each segment
                    try:
                        health_dir = day_path(day) / "health"
                        health_dir.mkdir(parents=True, exist_ok=True)
                        (health_dir / "stream.updated").touch()
                    except Exception:
                        pass
                    batch_success += success
                    batch_failed += failed
                    _update_status(segments_completed=i, segments_total=total)
                except Exception:
                    logging.exception(f"Segment {seg_key} failed with exception")
                    batch_failed += 1
                    _update_status(segments_completed=i, segments_total=total)

            duration_ms = int((time.time() - batch_start) * 1000)
            logging.info(
                f"All segments completed in {duration_ms}ms: "
                f"{batch_success} succeeded, {batch_failed} failed across {total} segments"
            )
            emit(
                "segments_completed",
                day=day,
                count=total,
                success=batch_success,
                failed=batch_failed,
                duration_ms=duration_ms,
            )

            if args.refresh:
                day_log(day, f"think --segments --refresh failed={batch_failed}")
            else:
                day_log(day, f"think --segments failed={batch_failed}")

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

        # PRE-PHASE: Run sense repair (daily only)
        if not args.segment:
            logging.info("Running pre-phase: sense repair")
            cmd = ["journal", "sense", "--day", day]
            if args.verbose:
                cmd.append("-v")
            day_log(day, f"starting: {' '.join(cmd)}")
            _jsonl_log("phase.start", mode=_run_mode, day=day, phase="sense_repair")
            _phase_start = time.time()
            phase_ok = run_command(cmd, day)
            _jsonl_log(
                "phase.complete",
                mode=_run_mode,
                day=day,
                phase="sense_repair",
                success=phase_ok,
                duration_ms=int((time.time() - _phase_start) * 1000),
            )
            if not phase_ok:
                logging.warning("Sense repair failed, continuing anyway")

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
            )
        else:
            success_count, fail_count, failed_names, applicable_units = (
                run_daily_prompts(
                    day=day,
                    verbose=args.verbose,
                    max_concurrency=args.jobs,
                    stream=resolved_stream,
                )
            )
        _run_result["success"] = success_count
        _run_result["failed"] = fail_count

        # Touch stream.updated marker after segment processing
        if args.segment:
            try:
                health_dir = day_path(day) / "health"
                health_dir.mkdir(parents=True, exist_ok=True)
                (health_dir / "stream.updated").touch()
            except Exception:
                pass

        # POST-PHASE: Final indexing and stats (daily only)
        if not args.segment:
            logging.info("Running post-phase: indexer rescan")
            rescan_cmd = ["sol", "indexer", "--rescan"]
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
            stats_cmd = ["sol", "journal-stats"]
            if args.verbose:
                stats_cmd.append("--verbose")
            _jsonl_log("phase.start", mode=_run_mode, day=day, phase="journal_stats")
            _phase_start = time.time()
            stats_ok = run_command(stats_cmd, day)
            _jsonl_log(
                "phase.complete",
                mode=_run_mode,
                day=day,
                phase="journal_stats",
                success=stats_ok,
                duration_ms=int((time.time() - _phase_start) * 1000),
            )

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
                        icon="💾",
                        action="/app/settings#storage",
                    )
            except Exception:
                logging.debug(
                    "Storage health check failed in post-phase", exc_info=True
                )

            # Touch daily.updated marker only after applicable daily units complete.
            try:
                completed = read_completed_units(day)
                all_done = all(
                    ("daily", name, facet) in completed
                    for name, facet in applicable_units
                )
                if all_done:
                    health_dir = day_path(day) / "health"
                    health_dir.mkdir(parents=True, exist_ok=True)
                    (health_dir / "daily.updated").touch()
                    logging.info("Day %s fully complete; wrote daily.updated", day)
                else:
                    logging.info(
                        "Day %s has incomplete daily unit(s); withholding daily.updated",
                        day,
                    )
            except Exception:
                logging.warning("Failed to update daily marker", exc_info=True)

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
                day=day,
                success=success_count,
                failed=fail_count,
                duration_ms=int((time.time() - start_time) * 1000),
            )

        # Build log message
        msg = "think"
        if args.refresh:
            msg += " --refresh"
        if fail_count:
            msg += f" failed={fail_count}"
        day_log(day, msg)

        duration_ms = int((time.time() - start_time) * 1000)
        logging.info(
            f"Think completed in {duration_ms}ms: {success_count} succeeded, {fail_count} failed"
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
