#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Event-based processor dispatcher for observe subsystem.

Listens for observe.observing Callosum events and spawns appropriate handler
processes, capturing their stdout/stderr to log files like supervisor.py does
for runners. Batch mode (--day) uses file-based scanning for historical days.
"""

from __future__ import annotations

import argparse
import json
import logging
import os
import signal
import subprocess
import threading
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path
from typing import Any, Dict, List, Optional

from solstone.observe.utils import AUDIO_EXTENSIONS, VIDEO_EXTENSIONS
from solstone.think.callosum import CallosumConnection
from solstone.think.runner import ManagedProcess as RunnerManagedProcess
from solstone.think.utils import (
    CHRONICLE_DIR,
    DATE_RE,
    STREAM_RE,
    day_path,
    get_config,
    get_journal,
    get_rev,
    iter_segments,
    journal_relative_path,
    now_ms,
    require_solstone,
    resolve_journal_path,
    setup_cli,
)

logger = logging.getLogger(__name__)

# Handlers with serialized worker pools. Add a new entry here when registering one in main().
HANDLER_NAMES = ("describe", "transcribe")


class QueuedItem:
    """Item in a handler queue with context for deferred processing."""

    __slots__ = ("file_path", "queued_at", "observer", "meta")

    def __init__(
        self,
        file_path: Path,
        queued_at: Optional[float] = None,
        observer: Optional[str] = None,
        meta: Optional[Dict[str, Any]] = None,
    ):
        self.file_path = file_path
        self.queued_at = queued_at if queued_at is not None else time.time()
        self.observer = observer
        self.meta = meta


class HandlerProcess:
    """Manages a running handler subprocess with RunnerManagedProcess."""

    def __init__(
        self,
        file_path: Path,
        managed: RunnerManagedProcess,
        handler_name: str,
    ):
        self.file_path = file_path
        self.managed = managed
        self.process = managed.process
        self.handler_name = handler_name
        self.started_at = time.time()

    def cleanup(self):
        self.managed.cleanup()


class FileSensor:
    """Event-driven sensor that spawns handler processes for media files."""

    def __init__(self, journal_dir: Path, verbose: bool = False, debug: bool = False):
        self.journal_dir = journal_dir
        self.verbose = verbose
        self.debug = debug

        # Registry: {glob_pattern: (handler_name, command_template)}
        self.handlers: Dict[str, tuple[str, List[str]]] = {}

        self._stopping = threading.Event()
        self.lock = threading.Lock()

        self.handler_pools: dict[str, ThreadPoolExecutor] = {
            name: ThreadPoolExecutor(
                max_workers=self._resolve_concurrency(name),
                thread_name_prefix=f"{name}-worker",
            )
            for name in HANDLER_NAMES
        }
        self.running_handlers: dict[str, list[HandlerProcess]] = {
            name: [] for name in HANDLER_NAMES
        }
        self.queued_handlers: dict[str, list[QueuedItem]] = {
            name: [] for name in HANDLER_NAMES
        }

        self.running_flag = True

        # Callosum connection for receiving events and emitting status
        self.callosum: Optional[CallosumConnection] = None

        # Track last status emission time
        self.last_status_emit = 0.0

        # Track segment processing: {segment_key: {pending_files}}
        self.segment_files: Dict[str, set[Path]] = {}
        # Track segment start times: {segment_key: start_timestamp}
        self.segment_start_time: Dict[str, float] = {}
        # Track segment day: {segment_key: day_string}
        self.segment_day: Dict[str, str] = {}
        # Track batch origin: {segment_key: True} for segments from batch mode
        self.segment_batch: Dict[str, bool] = {}
        # Track observer origin: {segment_key: observer_name} for observer segments
        self.segment_observer: Dict[str, str] = {}
        # Track handler errors per segment: {segment_key: [error_strings]}
        self.segment_errors: Dict[str, list[str]] = {}
        # Track stream identity per segment: {segment_key: stream_name}
        self.segment_stream: Dict[str, str] = {}

    def _resolve_concurrency(self, handler_name: str) -> int:
        cfg = get_config()
        raw = cfg.get(handler_name, {}).get("max_concurrent", 1)
        if not isinstance(raw, int) or isinstance(raw, bool) or raw < 1:
            logger.warning(
                "Invalid %s.max_concurrent in journal config: %r — defaulting to 1",
                handler_name,
                raw,
            )
            return 1
        return raw

    def register(self, pattern: str, handler_name: str, command: List[str]):
        """
        Register a handler for a file pattern.

        Args:
            pattern: Glob pattern (e.g., "*.webm", "*.flac")
            handler_name: Name for logging (e.g., "describe", "transcribe")
            command: Command list where "{file}" will be replaced with file path
        """
        self.handlers[pattern] = (handler_name, command)
        logger.info(f"Registered handler '{handler_name}' for pattern '{pattern}'")

    def _segment_relative_path(self, file_path: Path) -> Optional[Path]:
        """Return day/stream/segment/file relative path for journal media files."""
        roots = [self.journal_dir / CHRONICLE_DIR, self.journal_dir]
        for root in roots:
            try:
                rel_path = file_path.relative_to(root)
            except ValueError:
                continue
            if len(rel_path.parts) == 4 and DATE_RE.fullmatch(rel_path.parts[0]):
                return rel_path
        return None

    def _match_pattern(self, file_path: Path) -> Optional[tuple[str, List[str]]]:
        """Check if file matches any registered pattern."""
        # Ignore hidden files (temp recordings with dot prefix)
        if file_path.name.startswith("."):
            return None

        # Files should be in segment directories:
        # journal_dir/chronicle/YYYYMMDD/stream/HHMMSS_LEN/file.ext
        # Expected structure after stripping journal_dir[/chronicle]: 4 parts
        if self._segment_relative_path(file_path) is None:
            return None

        for pattern, handler_info in self.handlers.items():
            if file_path.match(pattern):
                return handler_info
        return None

    def _spawn_managed_process(
        self,
        cmd: list[str],
        file_path: Path,
        ref: str,
        segment: Optional[str],
        observer: Optional[str],
        meta: Optional[Dict[str, Any]],
        day: Optional[str] = None,
    ) -> RunnerManagedProcess | None:
        """Spawn the managed process for a handler invocation."""
        if self.callosum and "--cpu" not in cmd:
            try:
                rel_file = file_path.relative_to(self.journal_dir)
            except ValueError:
                rel_file = file_path

            handler_name = (
                cmd[1]
                if cmd[0] in ("sol", "journal") and len(cmd) > 1
                else Path(cmd[0]).name
            )
            event_fields = {
                "file": str(rel_file),
                "handler": handler_name,
                "ref": ref,
            }
            if day:
                event_fields["day"] = day
            if segment:
                event_fields["segment"] = segment
            if observer:
                event_fields["observer"] = observer
            if segment and segment in self.segment_stream:
                event_fields["stream"] = self.segment_stream[segment]
            self.callosum.emit("observe", "detected", **event_fields)

        env = os.environ.copy()
        if segment:
            env["SOL_SEGMENT"] = segment
        if observer:
            env["OBSERVER_NAME"] = observer
        if meta:
            env["SEGMENT_META"] = json.dumps(meta)

        try:
            managed = RunnerManagedProcess.spawn(
                cmd, ref=ref, callosum=self.callosum, env=env, day=day
            )
        except RuntimeError as exc:
            logger.error(str(exc))
            return None
        return managed

    def _remove_running_handler(
        self, handler_name: str, handler_proc: HandlerProcess
    ) -> None:
        with self.lock:
            handlers = self.running_handlers.get(handler_name)
            if handlers and handler_proc in handlers:
                handlers.remove(handler_proc)

    def _terminate_handler_process(
        self, handler_proc: HandlerProcess, deadline: Optional[float] = None
    ) -> None:
        try:
            handler_proc.process.terminate()
            logger.debug(
                f"Sent SIGTERM to {handler_proc.handler_name} for {handler_proc.file_path.name}"
            )
        except Exception as exc:
            logger.warning(
                f"Failed to terminate {handler_proc.handler_name} for {handler_proc.file_path.name}: {exc}"
            )

        try:
            timeout = 5 if deadline is None else max(0.1, deadline - time.time())
            handler_proc.process.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            logger.warning(
                f"Force killing {handler_proc.handler_name} for {handler_proc.file_path.name}"
            )
            handler_proc.process.kill()
            handler_proc.process.wait()

        handler_proc.cleanup()

    def _run_handler(
        self,
        queued_item: QueuedItem,
        handler_name: str,
        command: list[str],
        segment: str,
        day: Optional[str] = None,
        batch: bool = False,
    ) -> None:
        """Run one handler invocation from spawn through cleanup."""
        try:
            with self.lock:
                queued = self.queued_handlers.get(handler_name)
                if queued and queued_item in queued:
                    queued.remove(queued_item)

            if self._stopping.is_set():
                return

            file_path = queued_item.file_path
            cpu_fallback = False

            while True:
                ref = str(now_ms())
                cmd = [str(file_path) if arg == "{file}" else arg for arg in command]
                if cpu_fallback and handler_name == "transcribe":
                    cmd.append("--cpu")
                if self.debug:
                    cmd.append("-d")
                elif self.verbose:
                    cmd.append("-v")

                fallback_note = " with CPU fallback" if cpu_fallback else ""
                logger.info(
                    f"Spawning {handler_name}{fallback_note} for {file_path.name}: {' '.join(cmd)}"
                )

                managed = self._spawn_managed_process(
                    cmd,
                    file_path,
                    ref,
                    segment,
                    queued_item.observer,
                    queued_item.meta,
                    day,
                )
                if managed is None:
                    self._check_segment_observed(
                        file_path, error=f"{handler_name} spawn failed"
                    )
                    return

                handler_proc = HandlerProcess(file_path, managed, handler_name)
                with self.lock:
                    self.running_handlers[handler_name].append(handler_proc)
                    terminate_due_to_stop = self._stopping.is_set()

                if terminate_due_to_stop:
                    self._terminate_handler_process(handler_proc)
                    self._remove_running_handler(handler_name, handler_proc)
                    return

                try:
                    exit_code = managed.process.wait()
                except Exception:
                    handler_proc.cleanup()
                    self._remove_running_handler(handler_name, handler_proc)
                    raise

                elapsed = time.time() - handler_proc.started_at

                if (
                    exit_code == 134
                    and handler_name == "transcribe"
                    and not cpu_fallback
                ):
                    logger.warning(
                        f"Transcribe crashed (exit 134, likely GPU/cuDNN issue) for "
                        f"{file_path.name}, retrying with --cpu"
                    )
                    handler_proc.cleanup()
                    self._remove_running_handler(handler_name, handler_proc)
                    cpu_fallback = True
                    continue

                if exit_code == 0:
                    logger.info(
                        f"Handler completed successfully for {file_path.name} "
                        f"({elapsed:.1f}s)"
                    )
                    self._check_segment_observed(file_path)
                    handler_proc.cleanup()
                    self._remove_running_handler(handler_name, handler_proc)
                    return

                try:
                    log_rel = handler_proc.managed.log_writer.path.relative_to(
                        self.journal_dir
                    )
                except ValueError:
                    log_rel = handler_proc.managed.log_writer.path

                error_msg = f"{handler_name} failed with exit {exit_code}"
                logger.error(
                    f"{error_msg} for {file_path.name} ({elapsed:.1f}s) - see log {log_rel}"
                )

                if self.callosum:
                    icon = "🤖"
                    if handler_name == "transcribe":
                        icon = "🎙️"
                    elif handler_name == "describe":
                        icon = "👁️"
                    self.callosum.emit(
                        "notification",
                        "show",
                        message=f"{handler_name.capitalize()} failed for {file_path.name}",
                        title=f"{handler_name.capitalize()} Error",
                        icon=icon,
                        app="sense",
                        action=f"/app/health?log={log_rel}",
                    )

                self._check_segment_observed(
                    file_path,
                    error=f"{handler_name} exit {exit_code}",
                )
                handler_proc.cleanup()
                self._remove_running_handler(handler_name, handler_proc)
                return
        except Exception:
            logger.exception(
                f"Unhandled exception in handler worker for {queued_item.file_path}"
            )

    def _emit_segment_observed(self, segment: str, note: str = ""):
        """Emit observe.observed event and cleanup segment tracking.

        Must be called while holding self.lock.

        Args:
            segment: Segment key (HHMMSS_LEN format)
            note: Optional note for log message (e.g., "no handlers")
        """
        duration = int(time.time() - self.segment_start_time[segment])
        day = self.segment_day.get(segment)
        batch = self.segment_batch.get(segment, False)
        observer = self.segment_observer.get(segment)
        errors = self.segment_errors.get(segment)
        stream = self.segment_stream.get(segment)

        if self.callosum:
            event_fields = {
                "segment": segment,
                "day": day,
                "duration": duration,
            }
            if batch:
                event_fields["batch"] = True
            if observer:
                event_fields["observer"] = observer
            if stream:
                event_fields["stream"] = stream
            if errors:
                event_fields["error"] = True
                event_fields["errors"] = errors
            self.callosum.emit("observe", "observed", **event_fields)

        if errors:
            logger.warning(
                f"Segment observed with errors: {day}/{segment} ({duration}s) - {errors}"
            )
        else:
            note_str = f" ({note})" if note else ""
            logger.info(
                f"Segment fully observed{note_str}: {day}/{segment} ({duration}s)"
            )

        # Touch stream.updated marker for downstream consumers
        if day:
            try:
                health_dir = day_path(day) / "health"
                health_dir.mkdir(parents=True, exist_ok=True)
                (health_dir / "stream.updated").touch()
            except Exception:
                pass

        # Cleanup segment tracking
        del self.segment_files[segment]
        del self.segment_start_time[segment]
        if segment in self.segment_day:
            del self.segment_day[segment]
        if segment in self.segment_batch:
            del self.segment_batch[segment]
        if segment in self.segment_observer:
            del self.segment_observer[segment]
        if segment in self.segment_stream:
            del self.segment_stream[segment]
        if segment in self.segment_errors:
            del self.segment_errors[segment]

    def _check_segment_observed(self, file_path: Path, error: str | None = None):
        """Check if all files for this segment have completed processing.

        Args:
            file_path: Path to the file that finished processing
            error: Optional error string if the handler failed
        """
        from solstone.observe.utils import get_segment_key

        segment = get_segment_key(file_path)
        if not segment:
            return

        with self.lock:
            if segment in self.segment_files:
                if error:
                    if segment not in self.segment_errors:
                        self.segment_errors[segment] = []
                    self.segment_errors[segment].append(error)

                self.segment_files[segment].discard(file_path)

                # If no more pending files, emit observed event
                if not self.segment_files[segment]:
                    self._emit_segment_observed(segment)

    def _handle_file(
        self,
        file_path: Path,
        segment: Optional[str] = None,
        observer: Optional[str] = None,
        meta: Optional[Dict[str, Any]] = None,
    ):
        """Route file to appropriate handler.

        Args:
            file_path: Path to the file to process
            segment: Optional segment key for tracking
            observer: Optional observer name for OBSERVER_NAME env var
            meta: Optional metadata dict for SEGMENT_META env var
        """
        if not file_path.exists():
            logger.warning(f"File not found, skipping: {file_path}")
            return

        queue_size = 0
        with self.lock:
            if self._stopping.is_set():
                return

            handler_info = self._match_pattern(file_path)
            if not handler_info:
                return

            handler_name, command = handler_info
            running = self.running_handlers[handler_name]
            queued = self.queued_handlers[handler_name]
            if any(proc.file_path == file_path for proc in running) or any(
                item.file_path == file_path for item in queued
            ):
                logger.debug(f"File {file_path.name} already being processed")
                return

            rel_path = self._segment_relative_path(file_path)
            day = rel_path.parts[0] if rel_path is not None else None
            if segment is None and rel_path is not None:
                segment = rel_path.parts[2]
            if segment is None:
                return

            if segment not in self.segment_files:
                self.segment_files[segment] = set()
                self.segment_start_time[segment] = time.time()
                if day:
                    self.segment_day[segment] = day
            self.segment_files[segment].add(file_path)

            queued_item = QueuedItem(file_path, time.time(), observer, meta)
            queued.append(queued_item)
            queue_size = len(queued) + len(running)
            self.handler_pools[handler_name].submit(
                self._run_handler,
                queued_item,
                handler_name,
                command,
                segment,
                day,
                False,
            )

        if queue_size > 1:
            logger.info(
                f"Queueing {file_path.name} for {handler_name} "
                f"(queue size: {queue_size})"
            )

    def _handle_callosum_message(self, message: Dict[str, Any]):
        """Handle incoming Callosum messages, filtering for observe.observing events."""
        tract = message.get("tract")
        event = message.get("event")

        if tract != "observe" or event != "observing":
            return

        # Extract event fields
        day = message.get("day")
        segment = message.get("segment")
        files = message.get("files", [])
        observer = message.get("observer")  # Optional: set for observer uploads
        meta = message.get("meta")  # Optional: metadata dict (facet, setting, etc.)
        stream = message.get("stream")  # Optional: stream identity from observer

        if not day or not segment or not files:
            logger.warning(
                f"Invalid observing event: missing day/segment/files: {message}"
            )
            return

        logger.info(f"Received observing event: {day}/{segment} ({len(files)} files)")

        # Track stream identity for this segment
        if stream and segment:
            self.segment_stream[segment] = stream
            # Merge stream into meta so handlers get it via SEGMENT_META
            if meta is None:
                meta = {}
            meta["stream"] = stream

        # Build full paths for all files in this segment.
        rel_segment = f"{day}/{stream}/{segment}" if stream else f"{day}/{segment}"
        segment_dir = resolve_journal_path(self.journal_dir, rel_segment)
        file_paths = [segment_dir / filename for filename in files]

        # Pre-register segment tracking with complete file list
        # This ensures segment completion is tracked correctly even if some files
        # don't match patterns or fail to process
        with self.lock:
            if segment not in self.segment_files:
                self.segment_files[segment] = set()
                self.segment_start_time[segment] = time.time()
                self.segment_day[segment] = day
                if message.get("batch"):
                    self.segment_batch[segment] = True
                if observer:
                    self.segment_observer[segment] = observer
            for file_path in file_paths:
                # Only track files that will be processed (match a pattern)
                if self._match_pattern(file_path):
                    self.segment_files[segment].add(file_path)

        # Process each file (pass segment context for env vars)
        for file_path in file_paths:
            self._handle_file(file_path, segment=segment, observer=observer, meta=meta)

        # If no files matched any handler patterns, emit observed immediately
        # (e.g., tmux-only segments with just .jsonl files)
        with self.lock:
            if segment in self.segment_files and not self.segment_files[segment]:
                self._emit_segment_observed(segment, note="no handlers")

    def _emit_status(self):
        """Emit observe.status event with current processing state (only when active)."""
        if not self.callosum:
            return

        with self.lock:
            running_snapshot = {
                name: list(handlers) for name, handlers in self.running_handlers.items()
            }
            queued_snapshot = {
                name: list(items) for name, items in self.queued_handlers.items()
            }

        has_running = any(running_snapshot.values())
        has_queued = any(queued_snapshot.values())
        if not has_running and not has_queued:
            return

        # Build status object
        status = {}

        # Get journal path for relative paths
        journal_path = Path(get_journal())
        now = time.time()

        # Build status for each serialized handler queue
        for handler_name in running_snapshot:
            handler_status = {}

            # Current running processes
            if running_snapshot[handler_name]:
                running_list = []
                for handler_proc in running_snapshot[handler_name]:
                    try:
                        rel_file = journal_relative_path(
                            journal_path, handler_proc.file_path
                        )
                    except ValueError:
                        rel_file = str(handler_proc.file_path)

                    running_list.append(
                        {
                            "file": rel_file,
                            "ref": handler_proc.managed.ref,
                            "duration_seconds": int(now - handler_proc.started_at),
                        }
                    )
                handler_status["running"] = running_list

            # Queued items with age
            if queued_snapshot[handler_name]:
                queued_list = []
                for item in queued_snapshot[handler_name]:
                    try:
                        rel_file = journal_relative_path(journal_path, item.file_path)
                    except ValueError:
                        rel_file = str(item.file_path)

                    queued_list.append(
                        {"file": rel_file, "age_seconds": int(now - item.queued_at)}
                    )
                handler_status["queued"] = queued_list

            if queued_snapshot[handler_name]:
                handler_status["max_age_seconds"] = int(
                    now - min(item.queued_at for item in queued_snapshot[handler_name])
                )
            elif handler_status:
                handler_status["max_age_seconds"] = 0

            # Add section if any activity for this handler
            if handler_status:
                status[handler_name] = handler_status

        # Only emit if we have something to report
        if status:
            self.callosum.emit("observe", "status", **status)

    def start(self):
        """Start listening for observe.observing Callosum events."""

        # Start Callosum connection with callback for receiving events
        self.callosum = CallosumConnection(defaults={"rev": get_rev()})
        self.callosum.start(callback=self._handle_callosum_message)
        logger.info("Listening for observe.observing events via Callosum")

        while self.running_flag:
            # Emit status every 5 seconds if there's activity
            now = time.time()
            if now - self.last_status_emit >= 5:
                self._emit_status()
                self.last_status_emit = now

            time.sleep(1)

    def stop(self):
        """Stop listening and cleanup running processes."""
        self._stopping.set()
        self.running_flag = False

        # Stop Callosum connection
        if self.callosum:
            self.callosum.stop()

        for pool in self.handler_pools.values():
            pool.shutdown(wait=False, cancel_futures=True)

        with self.lock:
            running_handlers = [
                handler_proc
                for handlers in self.running_handlers.values()
                for handler_proc in handlers
            ]
        if running_handlers:
            logger.info(f"Terminating {len(running_handlers)} running handler(s)...")

        deadline = time.time() + 5
        for handler_proc in running_handlers:
            self._terminate_handler_process(handler_proc, deadline=deadline)
            self._remove_running_handler(handler_proc.handler_name, handler_proc)

        for pool in self.handler_pools.values():
            pool.shutdown(wait=True)

        with self.lock:
            for handlers in self.running_handlers.values():
                handlers.clear()
            for queued in self.queued_handlers.values():
                queued.clear()

    def scan_unprocessed(
        self,
        day: str,
        segment_filter: Optional[str] = None,
        *,
        stream_filter: Optional[str] = None,
        modality_filter: Optional[str] = None,
    ) -> tuple[list[tuple[Path, str, List[str]]], Dict[str, Optional[Dict[str, Any]]]]:
        """Scan a day and return matching unprocessed files and segment stream metadata."""
        day_dir = day_path(day)

        # Find all matching unprocessed files in segment directories
        from solstone.think.streams import read_segment_stream

        to_process = []
        segment_meta_cache: Dict[str, Optional[Dict[str, Any]]] = {}
        for stream_name, seg_key, seg_path in iter_segments(day_dir):
            if stream_filter and seg_path.parent.name != stream_filter:
                continue

            # Apply segment filter if specified
            if segment_filter and seg_key != segment_filter:
                continue

            # Read stream.json for batch-processed segments
            if seg_key not in segment_meta_cache:
                stream_info = read_segment_stream(seg_path)
                if stream_info and stream_info.get("stream"):
                    segment_meta_cache[seg_key] = {"stream": stream_info["stream"]}
                else:
                    segment_meta_cache[seg_key] = None

            for file_path in seg_path.iterdir():
                if not file_path.is_file():
                    continue

                suffix = file_path.suffix.lower()
                if modality_filter == "audio" and suffix not in AUDIO_EXTENSIONS:
                    continue
                if modality_filter == "screen" and suffix not in VIDEO_EXTENSIONS:
                    continue

                # Check if output JSONL exists (already processed)
                output_path = file_path.with_suffix(".jsonl")
                if output_path.exists():
                    continue

                handler_info = self._match_pattern(file_path)
                if handler_info:
                    handler_name, command = handler_info
                    to_process.append((file_path, handler_name, command))

        return to_process, segment_meta_cache

    def process_day(
        self,
        day: str,
        max_jobs: int = 1,
        segment_filter: Optional[str] = None,
        *,
        stream_filter: Optional[str] = None,
        modality_filter: Optional[str] = None,
    ):
        """Process all matching unprocessed files from a specific day directory.

        Files are in segment directories (HHMMSS_LEN/). A file is considered
        unprocessed if it has no corresponding .jsonl output file.

        Args:
            day: Day in YYYYMMDD format
            max_jobs: Maximum number of concurrent processing jobs
            segment_filter: Optional segment key to filter (HHMMSS_LEN format)
        """
        day_dir = day_path(day)
        if not day_dir.exists():
            logger.error(f"Day directory not found: {day_dir}")
            return

        to_process, segment_meta_cache = self.scan_unprocessed(
            day,
            segment_filter=segment_filter,
            stream_filter=stream_filter,
            modality_filter=modality_filter,
        )

        if not to_process:
            logger.info(f"No unprocessed files found in {day_dir}")
            return

        # Count files by extension
        ext_counts = {}
        for file_path, handler_name, command in to_process:
            ext = file_path.suffix.lower()  # e.g., ".webm", ".flac", ".jsonl"
            ext_counts[ext] = ext_counts.get(ext, 0) + 1

        # Format breakdown: "21 files (10 .webm, 8 .flac, 3 .jsonl)"
        breakdown = ", ".join(
            f"{count} {ext}" for ext, count in sorted(ext_counts.items())
        )
        logger.info(
            f"Found {len(to_process)} unprocessed files to process ({breakdown})"
        )

        temp_pools = {}
        futures = {}
        try:
            for file_path, handler_name, command in to_process:
                seg_name = file_path.parent.name
                meta = segment_meta_cache.get(seg_name)
                if max_jobs > self._resolve_concurrency(handler_name):
                    if handler_name not in temp_pools:
                        temp_pools[handler_name] = ThreadPoolExecutor(
                            max_workers=max_jobs,
                            thread_name_prefix=f"{handler_name}-batch",
                        )
                    executor = temp_pools[handler_name]
                else:
                    executor = self.handler_pools[handler_name]

                with self.lock:
                    if seg_name not in self.segment_files:
                        self.segment_files[seg_name] = set()
                        self.segment_start_time[seg_name] = time.time()
                        self.segment_day[seg_name] = day
                        self.segment_batch[seg_name] = True
                    self.segment_files[seg_name].add(file_path)

                queued_item = QueuedItem(file_path, time.time(), meta=meta)
                future = executor.submit(
                    self._run_handler,
                    queued_item,
                    handler_name,
                    command,
                    seg_name,
                    day,
                    True,
                )
                futures[future] = file_path

            for future in as_completed(futures):
                file_path = futures[future]
                try:
                    future.result()
                except Exception:
                    logger.exception(f"Batch worker failed for {file_path}")
        finally:
            for executor in temp_pools.values():
                executor.shutdown(wait=True)

        logger.info("Batch processing complete")


def delete_outputs(
    day_dir: Path,
    reprocess_type: str,
    segment_filter: Optional[str] = None,
    *,
    stream_filter: Optional[str] = None,
    dry_run: bool = False,
) -> list[Path]:
    """Delete existing output files to force reprocessing.

    Args:
        day_dir: Path to day directory (YYYYMMDD)
        reprocess_type: Type of outputs to delete ("screen", "audio", or "all")
        segment_filter: Optional segment key to filter (HHMMSS_LEN format)
        dry_run: If True, don't delete, just return what would be deleted

    Returns:
        List of paths that were (or would be) deleted
    """
    deleted = []

    if not day_dir.exists():
        return deleted

    for _stream_name, seg_key, seg_path in iter_segments(day_dir):
        if stream_filter and seg_path.parent.name != stream_filter:
            continue

        # Apply segment filter if specified
        if segment_filter and seg_key != segment_filter:
            continue

        for file_path in seg_path.iterdir():
            if not file_path.is_file() or file_path.suffix != ".jsonl":
                continue

            stem = file_path.stem.lower()

            # Determine if this output matches the reprocess type
            should_delete = False
            if reprocess_type == "all":
                # Delete all outputs that have a corresponding source file
                # Check for video source
                for ext in VIDEO_EXTENSIONS:
                    if (seg_path / f"{file_path.stem}{ext}").exists():
                        should_delete = True
                        break
                # Check for audio source
                for ext in AUDIO_EXTENSIONS:
                    if (seg_path / f"{file_path.stem}{ext}").exists():
                        should_delete = True
                        break
            elif reprocess_type == "screen":
                # Screen outputs end with _screen or are just screen
                if stem.endswith("_screen") or stem == "screen":
                    should_delete = True
            elif reprocess_type == "audio":
                # Audio outputs end with _audio or are just audio
                if stem.endswith("_audio") or stem == "audio":
                    should_delete = True

            if should_delete:
                deleted.append(file_path)
                if not dry_run:
                    file_path.unlink()
                    logger.info(f"Deleted: {file_path.relative_to(day_dir.parent)}")

    return deleted


def scan_day(day_dir: Path) -> dict:
    """Scan a day directory for processed and unprocessed files.

    Files are in segment directories (stream/HHMMSS_LEN/). A file is considered
    processed if it has a corresponding .jsonl output file.

    Args:
        day_dir: Path to day directory (YYYYMMDD)

    Returns:
        Dictionary with:
        - "processed": List of JSONL output files in segments (stream/HHMMSS_LEN/audio.jsonl, etc)
        - "unprocessed": List of unprocessed source media files in segments
        - "pending_segments": Count of unique segments with pending files
    """
    processed = []
    unprocessed = []
    pending_segment_keys = set()

    if not day_dir.exists():
        return {"processed": [], "unprocessed": [], "pending_segments": 0}

    for stream_name, seg_key, seg_path in iter_segments(day_dir):
        # Check each file in the segment
        for file_path in seg_path.iterdir():
            if not file_path.is_file():
                continue

            # JSONL files are outputs
            if file_path.suffix == ".jsonl":
                processed.append(f"{stream_name}/{seg_key}/{file_path.name}")
                continue

            # Check if media file has corresponding JSONL (processed)
            if (
                file_path.suffix.lower() in VIDEO_EXTENSIONS
                or file_path.suffix.lower() in AUDIO_EXTENSIONS
            ):
                output_path = file_path.with_suffix(".jsonl")
                if not output_path.exists():
                    unprocessed.append(f"{stream_name}/{seg_key}/{file_path.name}")
                    pending_segment_keys.add(seg_key)

    processed.sort()
    unprocessed.sort()

    return {
        "processed": processed,
        "unprocessed": unprocessed,
        "pending_segments": len(pending_segment_keys),
    }


def _install_sigterm_handler(sensor: FileSensor) -> None:
    def handle_sigterm(_signum, _frame) -> None:
        logger.info("SIGTERM received, shutting down observe sensor")
        sensor.stop()

    signal.signal(signal.SIGTERM, handle_sigterm)


def main():
    """CLI entry point."""
    parser = argparse.ArgumentParser(description="Unified observe file processor")
    parser.add_argument(
        "--day",
        type=str,
        help="Process files from specific day (YYYYMMDD format) instead of watching",
    )
    parser.add_argument(
        "-j",
        "--jobs",
        type=int,
        default=1,
        help="Max concurrent processing jobs when using --day (default: 1)",
    )
    parser.add_argument(
        "--reprocess",
        type=str,
        choices=["screen", "audio", "all"],
        help="Delete existing outputs and reprocess (requires --day)",
    )
    parser.add_argument(
        "--segment",
        type=str,
        help="Filter to specific segment (HHMMSS_LEN format, requires --day)",
    )
    parser.add_argument(
        "--stream",
        type=str,
        help="Filter to specific stream (requires --day)",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Show what would be processed (or deleted with --reprocess) without making changes",
    )
    args = setup_cli(parser)
    require_solstone()

    journal = Path(get_journal())

    # Validate argument combinations
    if args.reprocess and not args.day:
        parser.error("--reprocess requires --day")
    if args.segment and not args.day:
        parser.error("--segment requires --day")
    if args.stream and not args.day:
        parser.error("--stream requires --day")
    if args.dry_run and not args.day:
        parser.error("--dry-run requires --day")

    # Validate segment format if provided
    if args.segment:
        from solstone.think.utils import segment_key

        if not segment_key(args.segment):
            parser.error(f"--segment must be HHMMSS_LEN format, got: {args.segment}")

    if args.stream and not STREAM_RE.fullmatch(args.stream):
        parser.error(f"--stream must match stream name format, got: {args.stream}")

    sensor = FileSensor(journal, verbose=args.verbose, debug=args.debug)

    # Register handlers - match by extension
    # Audio files in segment directories
    for ext in AUDIO_EXTENSIONS:
        sensor.register(f"*{ext}", "transcribe", ["journal", "transcribe", "{file}"])

    # Video files in segment directories
    for ext in VIDEO_EXTENSIONS:
        sensor.register(f"*{ext}", "describe", ["journal", "describe", "{file}"])

    if args.day:
        day_dir = day_path(args.day)

        # Handle reprocess mode
        if args.reprocess:
            deleted = delete_outputs(
                day_dir,
                args.reprocess,
                segment_filter=args.segment,
                stream_filter=args.stream,
                dry_run=args.dry_run,
            )

            if args.dry_run:
                if deleted:
                    logger.info(f"Would delete {len(deleted)} output file(s):")
                    for path in deleted:
                        logger.info(f"  {journal_relative_path(Path(journal), path)}")
                else:
                    logger.info("No files to delete")
                return
            else:
                logger.info(f"Deleted {len(deleted)} output file(s)")

        # Standalone dry-run: show what would be processed
        if args.dry_run:
            modality_filter = (
                args.reprocess if args.reprocess in {"audio", "screen"} else None
            )
            to_process, _ = sensor.scan_unprocessed(
                args.day,
                segment_filter=args.segment,
                stream_filter=args.stream,
                modality_filter=modality_filter,
            )
            if to_process:
                ext_counts = {}
                for file_path, handler_name, command in to_process:
                    ext = file_path.suffix.lower()
                    ext_counts[ext] = ext_counts.get(ext, 0) + 1
                breakdown = ", ".join(
                    f"{count} {ext}" for ext, count in sorted(ext_counts.items())
                )
                logger.info(f"Would process {len(to_process)} file(s) ({breakdown}):")
                for file_path, handler_name, command in to_process:
                    logger.info(f"  {journal_relative_path(Path(journal), file_path)}")
            else:
                logger.info("No unprocessed files found")
            return

        # Batch mode: process specific day
        segment_msg = f" (segment: {args.segment})" if args.segment else ""
        logger.info(
            f"Processing files from day {args.day}{segment_msg} "
            f"with {args.jobs} concurrent jobs"
        )
        modality_filter = (
            args.reprocess if args.reprocess in {"audio", "screen"} else None
        )
        sensor.process_day(
            args.day,
            max_jobs=args.jobs,
            segment_filter=args.segment,
            stream_filter=args.stream,
            modality_filter=modality_filter,
        )
    else:
        # Event mode: listen for Callosum events
        logger.info("Starting observe sensor in event mode...")
        _install_sigterm_handler(sensor)
        try:
            sensor.start()
        except KeyboardInterrupt:
            logger.info("Shutting down...")
            sensor.stop()


if __name__ == "__main__":
    main()
