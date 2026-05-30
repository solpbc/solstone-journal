# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import argparse
import asyncio
import concurrent.futures
import getpass
import json
import logging
import os
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time
from collections import deque
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

import psutil

from solstone.think import routines, scheduler
from solstone.think.callosum import CallosumConnection, CallosumServer
from solstone.think.maint import run_pending_tasks
from solstone.think.models import LOCAL_MODEL, is_local_provider_needed
from solstone.think.readiness import clear_ready, signal_ready
from solstone.think.runner import ManagedProcess as RunnerManagedProcess
from solstone.think.runner import _command_partition
from solstone.think.sync_check import (
    DEFAULT_INTERVAL_SECONDS,
    SyncCheckSnapshot,
    check_journal_sync,
    clear_self_heartbeat,
    format_conflict_message,
    write_self_heartbeat,
)
from solstone.think.utils import (
    EXIT_TEMPFAIL,
    day_path,
    find_available_port,
    get_journal,
    get_journal_info,
    get_rev,
    is_solstone_up,
    now_ms,
    read_service_port,
    setup_cli,
    updated_days,
    write_service_port,
)

DEFAULT_THRESHOLD = 60
CHECK_INTERVAL = 30
MAX_UPDATED_CATCHUP = 4
TEMPFAIL_DELAY = 15  # seconds to wait before retrying a tempfail exit
STOPPED_TICKS_THRESHOLD = 2
LOCAL_SERVER_READY_TIMEOUT_S = 300.0
LOCAL_SERVER_HEALTH_POLL_INTERVAL_S = 1.0
LOCAL_MODEL_WARMING_UP_COPY = "Local model is warming up..."
logger = logging.getLogger(__name__)
_SERVICE_LIFECYCLE_VERBS = {
    "start",
    "stop",
    "restart",
    "status",
    "install",
    "uninstall",
    "logs",
}

# Global shutdown flag
shutdown_requested = False
_last_sync_tick: float = 0.0
_last_sync_snapshot: "SyncCheckSnapshot | None" = None
_sync_conflict_shutdown: bool = False
# Supervisor identity (set in main() once ref is assigned)
_supervisor_ref: str | None = None
_supervisor_start: float | None = None


def _sd_notify(state: str) -> None:
    addr = os.environ.get("NOTIFY_SOCKET")
    if not addr:
        return
    if addr.startswith("@"):
        addr = "\0" + addr[1:]
    try:
        with socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM) as s:
            s.sendto(state.encode(), addr)
    except OSError as exc:
        logging.warning("sd_notify failed: %s", exc)


def _candidate_journal(proc: "psutil.Process") -> Path | None:
    """Return the resolved SOLSTONE_JOURNAL of ``proc``, or None on any failure.

    Used by the orphan sweep to skip candidates we cannot positively classify
    as belonging to the caller's journal. Conservative on unknown: any failure
    to read or parse the env value returns None so the candidate is skipped.
    """
    try:
        env = proc.environ()
    except (psutil.AccessDenied, psutil.NoSuchProcess, OSError):
        return None
    raw = env.get("SOLSTONE_JOURNAL")
    if not raw:
        return None
    try:
        return Path(raw).resolve()
    except (OSError, RuntimeError, ValueError):
        return None


# The long-lived managed-service proctitles set by setproctitle at
# sol_cli.py (f"{binary}:{cmd}"). setproctitle is in-process and persists
# until the process exits, so an orphaned service still reports its title
# via proc.name() after the supervisor dies — which is what lets the sweep
# find it. The supervisor-owned `llama-server` reports its own bare binary
# name (no colon prefix) and is included here so the sweep reaps it too.
_MANAGED_SERVICE_PROCTITLES = frozenset(
    {
        "journal:sense",
        "journal:cortex",
        "journal:convey",
        "sol:link",
        "llama-server",
    }
)


def _sweep_orphaned_sol_processes(journal: Path, grace: float = 5.0) -> int:
    journal = journal.resolve()
    current_user = getpass.getuser()
    own_pid = os.getpid()
    targets: list[int] = []
    for proc in psutil.process_iter(["name", "ppid", "username", "pid"]):
        try:
            if proc.name() not in _MANAGED_SERVICE_PROCTITLES:
                continue
            if proc.ppid() != 1:
                continue
            if proc.username() != current_user:
                continue
            if proc.pid == own_pid:
                continue
            candidate_journal = _candidate_journal(proc)
            if candidate_journal != journal:
                continue
            targets.append(proc.pid)
        except (psutil.NoSuchProcess, psutil.AccessDenied):
            continue

    if not targets:
        return 0

    logger.info(
        "orphan sweep: terminating %d sol process(es) in journal %s",
        len(targets),
        journal,
    )
    for pid in targets:
        logger.debug("orphan sweep: SIGTERM pid=%d", pid)
        try:
            os.kill(pid, signal.SIGTERM)
        except ProcessLookupError:
            pass

    deadline = time.time() + grace
    while time.time() < deadline:
        if not any(psutil.pid_exists(pid) for pid in targets):
            break
        time.sleep(0.1)

    survivors = [pid for pid in targets if psutil.pid_exists(pid)]
    for pid in survivors:
        logger.debug("orphan sweep: SIGKILL pid=%d", pid)
        try:
            os.kill(pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
    return len(targets)


class CallosumLogHandler(logging.Handler):
    """Logging handler that emits log records as callosum ``logs`` tract events.

    Silently drops events on any error — callosum mirroring is best-effort.
    """

    def __init__(self, conn: CallosumConnection, ref: str):
        super().__init__()
        self._conn = conn
        self._ref = ref
        self._pid = os.getpid()
        self._emitting = False

    def emit(self, record: logging.LogRecord) -> None:
        if self._emitting:
            return
        self._emitting = True
        try:
            self._conn.emit(
                "logs",
                "line",
                ref=self._ref,
                name="supervisor",
                pid=self._pid,
                stream="log",
                line=self.format(record),
            )
        except Exception:
            pass
        finally:
            self._emitting = False


class SupervisorArgumentParser(argparse.ArgumentParser):
    def error(self, message: str) -> None:
        mistaken = next(
            (arg for arg in sys.argv[1:] if arg in _SERVICE_LIFECYCLE_VERBS),
            None,
        )
        if mistaken:
            self.exit(
                2,
                "journal supervisor is the server-launch command (takes a port). "
                "For lifecycle, use: journal service <verb>. "
                f"Did you mean: journal service {mistaken} ?\n",
            )
        super().error(message)


class TaskQueue:
    """Manages on-demand task execution with per-command serialization.

    Tasks are serialized by command name - only one task per command runs at a time.
    Additional requests for the same command are queued (deduped by exact cmd match).
    Multiple callers requesting the same work have their refs coalesced so all get
    notified when the task completes.

    The lock only protects state mutations, never held during I/O operations.
    """

    def __init__(self, on_queue_change: callable = None, ready: bool = True):
        """Initialize task queue.

        Args:
            on_queue_change: Optional callback(cmd_name, running_ref, queue_entries)
                            called after queue state changes. Called outside lock.
        """
        self._running: dict[
            str, dict
        ] = {}  # command_name -> {"ref": str, "thread": Thread}
        self._queues: dict[str, list] = {}  # command_name -> list of {refs, cmd} dicts
        self._active: dict[str, RunnerManagedProcess] = {}  # ref -> process
        self._history: deque[dict[str, Any]] = deque(maxlen=100)
        self._cap_terminated: set[str] = set()
        self._stopped_ticks: dict[str, int] = {}
        self._caps: dict[str, int] = {}
        self._pending: list[dict] = []
        self._ready = ready
        self._lock = threading.Lock()
        self._on_queue_change = on_queue_change

    @staticmethod
    def get_command_name(cmd: list[str]) -> str:
        """Return the canonical queue/log partition for a command."""
        return _command_partition(cmd)

    def _notify_queue_change(self, cmd_name: str) -> None:
        """Notify listener of queue state change (called outside lock)."""
        if not self._on_queue_change:
            return

        with self._lock:
            if cmd_name == "pending":
                queue = list(self._pending)
                running_ref = None
            else:
                queue = list(self._queues.get(cmd_name, []))
                entry = self._running.get(cmd_name)
                running_ref = entry["ref"] if entry else None

        self._on_queue_change(cmd_name, running_ref, queue)

    def submit(
        self,
        cmd: list[str],
        ref: str | None = None,
        day: str | None = None,
        scheduler_name: str | None = None,
    ) -> str | None:
        """Submit a task for execution.

        If no task of this command type is running, starts immediately.
        Otherwise queues (deduped by exact cmd match, refs coalesced).

        Args:
            cmd: Command to execute
            ref: Optional caller-provided ref for tracking
            day: Optional day override (YYYYMMDD) for log placement

        Returns:
            ref if task was started/queued, None if already tracked (no change)
        """
        ref = ref or str(now_ms())
        cmd_name = self.get_command_name(cmd)

        with self._lock:
            if not self._ready:
                self._pending.append(
                    {
                        "refs": [ref],
                        "cmd": cmd,
                        "day": day,
                        "scheduler_name": scheduler_name,
                    }
                )
                should_notify_pending = True
            else:
                should_notify_pending = False

        if should_notify_pending:
            self._notify_queue_change("pending")
            return ref

        should_notify = False
        should_start = False

        with self._lock:
            # Detect stale running state (task thread died without clearing queue)
            if cmd_name in self._running:
                stale = self._running[cmd_name]
                if stale["thread"] is not None and not stale["thread"].is_alive():
                    logging.warning(
                        f"Clearing stale {cmd_name} queue "
                        f"(thread dead, ref={stale['ref']})"
                    )
                    self._running.pop(cmd_name)

            if cmd_name in self._running:
                # Command already running - queue or coalesce
                queue = self._queues.setdefault(cmd_name, [])
                existing = next((q for q in queue if q["cmd"] == cmd), None)
                if existing:
                    if ref not in existing["refs"]:
                        existing["refs"].append(ref)
                        logging.info(
                            f"Added ref {ref} to queued task {cmd_name} "
                            f"(refs: {len(existing['refs'])})"
                        )
                        should_notify = True
                    else:
                        logging.debug(f"Ref already tracked for queued task: {ref}")
                        return None
                else:
                    queue.append(
                        {
                            "refs": [ref],
                            "cmd": cmd,
                            "day": day,
                            "scheduler_name": scheduler_name,
                        }
                    )
                    logging.info(
                        f"Queued task {cmd_name}: {' '.join(cmd)} ref={ref} "
                        f"(queue: {len(queue)})"
                    )
                    should_notify = True
            else:
                # Not running - mark as running and start
                # Thread is set to None here; _run_task registers it on entry
                self._running[cmd_name] = {
                    "ref": ref,
                    "thread": None,
                    "scheduler_name": scheduler_name,
                }
                should_start = True

        # Notify outside lock
        if should_notify:
            self._notify_queue_change(cmd_name)
            return ref

        # Start task outside lock
        if should_start:
            threading.Thread(
                target=self._run_task,
                args=([ref], cmd, cmd_name, day, scheduler_name),
                daemon=True,
            ).start()
            return ref

        return None

    def set_cap(self, cmd_name: str, seconds: int) -> None:
        """Set a max runtime cap in seconds for a queued command name."""
        with self._lock:
            self._caps[cmd_name] = seconds

    def get_active_by_cmd_name(self, name: str) -> str | None:
        """Return the first active ref matching a command name."""
        with self._lock:
            for ref, managed in self._active.items():
                if self.get_command_name(managed.cmd) == name:
                    return ref
        return None

    def enforce_deadlines(self, now: float) -> None:
        """Enforce configured task runtime caps without blocking the supervisor tick."""
        with self._lock:
            for ref, managed in list(self._active.items()):
                cmd_name = self.get_command_name(managed.cmd)
                cap = self._caps.get(cmd_name)
                if not cap:
                    continue

                elapsed = now - managed.start_time
                if elapsed <= cap:
                    continue

                elapsed_seconds = int(elapsed)
                logging.warning(
                    "Task %s (cmd=%s, ref=%s) exceeded max_runtime of %ds "
                    "(elapsed=%ds); terminating",
                    cmd_name,
                    " ".join(managed.cmd),
                    ref,
                    cap,
                    elapsed_seconds,
                )
                self._cap_terminated.add(ref)
                _start_termination_thread(ref, managed, timeout=2.0, reason="cap")

            for ref, managed in list(self._active.items()):
                if ref in self._cap_terminated:
                    continue

                try:
                    state = psutil.Process(managed.process.pid).status()
                except (psutil.NoSuchProcess, psutil.AccessDenied):
                    self._stopped_ticks.pop(ref, None)
                    continue

                if state in (psutil.STATUS_STOPPED, psutil.STATUS_TRACING_STOP):
                    ticks = self._stopped_ticks.get(ref, 0) + 1
                    self._stopped_ticks[ref] = ticks
                    if ticks >= STOPPED_TICKS_THRESHOLD:
                        cmd_name = self.get_command_name(managed.cmd)
                        logging.warning(
                            "Task %s (cmd=%s, ref=%s) was stopped (state=%s) "
                            "for %d consecutive ticks; terminating",
                            cmd_name,
                            " ".join(managed.cmd),
                            ref,
                            state,
                            ticks,
                        )
                        self._cap_terminated.add(ref)
                        _start_termination_thread(
                            ref, managed, timeout=2.0, reason="stopped"
                        )
                        self._stopped_ticks.pop(ref, None)
                else:
                    self._stopped_ticks.pop(ref, None)

    def set_ready(self) -> None:
        """Allow buffered tasks to start dispatching through the normal queue path."""
        with self._lock:
            if self._ready:
                return
            self._ready = True
            pending = list(self._pending)
            self._pending.clear()

        if pending:
            self._notify_queue_change("pending")
        for entry in pending:
            self.submit(
                entry["cmd"],
                ref=entry["refs"][0],
                day=entry.get("day"),
                scheduler_name=entry.get("scheduler_name"),
            )

    def _run_task(
        self,
        refs: list[str],
        cmd: list[str],
        cmd_name: str,
        day: str | None = None,
        scheduler_name: str | None = None,
    ) -> None:
        """Execute a task and handle completion.

        Args:
            refs: List of refs to notify on completion
            cmd: Command to execute
            cmd_name: Command name for queue management
            day: Optional day override (YYYYMMDD) for log placement
        """
        # Register this thread for stale-queue detection
        with self._lock:
            if cmd_name in self._running and self._running[cmd_name]["ref"] == refs[0]:
                self._running[cmd_name]["thread"] = threading.current_thread()

        callosum = CallosumConnection()
        managed = None
        primary_ref = refs[0]
        service = cmd_name
        exit_status = "error"

        try:
            callosum.start()
            logging.info(f"Starting task {primary_ref}: {' '.join(cmd)}")

            managed = RunnerManagedProcess.spawn(
                cmd, ref=primary_ref, callosum=callosum, day=day
            )
            with self._lock:
                self._active[primary_ref] = managed

            callosum.emit(
                "supervisor",
                "started",
                service=service,
                pid=managed.pid,
                ref=primary_ref,
            )

            exit_code = managed.wait()
            exit_status = "ok" if exit_code == 0 else "error"

            for ref in refs:
                callosum.emit(
                    "supervisor",
                    "stopped",
                    service=service,
                    pid=managed.pid,
                    ref=ref,
                    exit_code=exit_code,
                )

            if exit_code == 0:
                logging.info(f"Task {cmd_name} ({primary_ref}) finished successfully")
            else:
                logging.warning(
                    f"Task {cmd_name} ({primary_ref}) failed with exit code {exit_code}"
                )

        except Exception as e:
            if isinstance(e, subprocess.TimeoutExpired):
                exit_status = "timeout"
            logging.exception(
                f"Task {cmd_name} ({primary_ref}) encountered exception: {e}"
            )
            for ref in refs:
                callosum.emit(
                    "supervisor",
                    "stopped",
                    service=service,
                    pid=managed.pid if managed else 0,
                    ref=ref,
                    exit_code=-1,
                )
        finally:
            try:
                if managed:
                    managed.cleanup()
            except Exception:
                logging.exception(f"Task {cmd_name} ({primary_ref}): cleanup failed")
            with self._lock:
                self._active.pop(primary_ref, None)
                if primary_ref in self._cap_terminated:
                    exit_status = "timeout"
                self._cap_terminated.discard(primary_ref)
                self._stopped_ticks.pop(primary_ref, None)
                ended_at = time.time()
                self._history.append(
                    {
                        "name": cmd_name,
                        "cmd": list(cmd),
                        "ref": primary_ref,
                        "ended_at": ended_at,
                        "exit_status": exit_status,
                        "scheduler_name": scheduler_name,
                    }
                )
            if scheduler_name:
                try:
                    _record_scheduler_completion(
                        scheduler_name,
                        ended_at=ended_at,
                        exit_status=exit_status,
                        ref=primary_ref,
                        cmd=cmd,
                    )
                except Exception as exc:
                    logger.warning("scheduler completion writeback failed: %s", exc)
            try:
                callosum.stop()
            except Exception:
                logging.exception(
                    f"Task {cmd_name} ({primary_ref}): callosum stop failed"
                )
            self._process_next(cmd_name)

    def _process_next(self, cmd_name: str) -> None:
        """Process next queued task after completion."""
        next_cmd = None
        refs = None
        day = None
        scheduler_name = None

        with self._lock:
            queue = self._queues.get(cmd_name, [])
            if queue:
                entry = queue.pop(0)
                refs = entry["refs"]
                next_cmd = entry["cmd"]
                day = entry.get("day")
                scheduler_name = entry.get("scheduler_name")
                # Thread is set to None here; _run_task registers it on entry
                self._running[cmd_name] = {
                    "ref": refs[0],
                    "thread": None,
                    "scheduler_name": scheduler_name,
                }
                logging.info(
                    f"Dequeued task {cmd_name}: {' '.join(next_cmd)} refs={refs} "
                    f"(remaining: {len(queue)})"
                )
            else:
                self._running.pop(cmd_name, None)

        # Notify and spawn outside lock
        self._notify_queue_change(cmd_name)
        if next_cmd:
            threading.Thread(
                target=self._run_task,
                args=(refs, next_cmd, cmd_name, day, scheduler_name),
                daemon=True,
            ).start()

    def cancel(self, ref: str) -> bool:
        """Cancel a running task.

        Returns:
            True if task was found and terminated, False otherwise
        """
        if ref not in self._active:
            logging.warning(f"Cannot cancel task {ref}: not found")
            return False

        managed = self._active[ref]
        if not managed.is_running():
            logging.debug(f"Task {ref} already finished")
            return False

        logging.info(f"Cancelling task {ref}...")
        managed.terminate()
        return True

    def shutdown(self, timeout: float = 10.0) -> int:
        with self._lock:
            active = list(self._active.items())
        if not active:
            return 0

        def _terminate(item: tuple[str, RunnerManagedProcess]) -> None:
            ref, managed = item
            try:
                managed.terminate(timeout=timeout)
            except subprocess.TimeoutExpired:
                logger.warning(
                    "task %s did not exit within %ss; KILL sent", ref, timeout
                )
            except OSError as exc:
                logger.warning("task %s terminate raised: %s", ref, exc)

        with concurrent.futures.ThreadPoolExecutor(max_workers=len(active)) as executor:
            list(executor.map(_terminate, active))
        return len(active)

    def get_status(self, ref: str) -> dict:
        """Get status of a task."""
        if ref not in self._active:
            return {"status": "not_found"}

        managed = self._active[ref]
        return {
            "status": "running" if managed.is_running() else "finished",
            "pid": managed.pid,
            "returncode": managed.returncode,
            "log_path": str(managed.log_writer.path),
            "cmd": managed.cmd,
        }

    def collect_task_status(self) -> list[dict]:
        """Collect status of all running tasks for supervisor status."""
        now = time.time()
        tasks = []
        for ref, managed in self._active.items():
            if managed.is_running():
                duration = int(now - managed.start_time)
                cmd_name = TaskQueue.get_command_name(managed.cmd)
                cap = self._caps.get(cmd_name)
                tasks.append(
                    {
                        "ref": ref,
                        "name": cmd_name,
                        "duration_seconds": duration,
                        "max_runtime_seconds": cap,
                        "stuck": cap is not None and duration > cap,
                    }
                )
        return tasks

    def collect_queue_counts(self) -> dict[str, int]:
        """Snapshot per-command queue depths for status reporting."""
        with self._lock:
            counts = {
                cmd_name: len(queue)
                for cmd_name, queue in self._queues.items()
                if queue
            }
            if self._pending:
                counts["pending"] = len(self._pending)
            return counts


# Global task queue instance (initialized in main())
_task_queue: TaskQueue | None = None

# Global supervisor callosum connection for event emissions
_supervisor_callosum: CallosumConnection | None = None

# Global reference to managed processes for restart control
_managed_procs: list[RunnerManagedProcess] = []
_SERVICE_STATE: dict[str, dict[str, Any]] = {}
_termination_threads: dict[str, threading.Thread] = {}
_termination_threads_lock = threading.Lock()
_SCHEDULER_JSON_LOCK = threading.Lock()

# Global reference to in-process Callosum server
_callosum_server: CallosumServer | None = None
_callosum_thread: threading.Thread | None = None

# Track whether running in remote mode (upload-only, no local processing)
_is_remote_mode: bool = False
_digest_submitted_this_boot = False

# State for daily processing (tracks day boundary for midnight think trigger)
_daily_state = {
    "last_day": None,  # Track which day we last processed
}

# Timeout before flushing stale segments (seconds)
FLUSH_TIMEOUT = 3600

# State for segment flush (close out dangling agent state after inactivity)
_flush_state: dict = {
    "last_segment_ts": 0.0,  # Wall-clock time of last observe.observed event
    "day": None,  # Day of last observed segment
    "segment": None,  # Last observed segment key
    "flushed": False,  # Whether flush has already run for current segment
}


def _get_journal_path() -> Path:
    return Path(get_journal())


def is_supervisor_up() -> bool:
    """Return True when supervisor.pid and supervisor.start_time identify a live supervisor process for the current journal."""
    health_dir = Path(get_journal()) / "health"
    pid_path = health_dir / "supervisor.pid"
    try:
        pid = int(pid_path.read_text().strip())
    except FileNotFoundError:
        return False
    except (OSError, ValueError):
        return False

    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return False
    except OSError:
        return False

    start_time_path = health_dir / "supervisor.start_time"
    try:
        recorded_start = float(start_time_path.read_text().strip())
    except FileNotFoundError:
        return False
    except (OSError, ValueError):
        return False

    try:
        create_time = psutil.Process(pid).create_time()
    except psutil.NoSuchProcess:
        return False
    except psutil.Error:
        return False

    tolerance = 1.5  # drift between time.time() and psutil create_time()
    return abs(recorded_start - create_time) <= tolerance


class RestartPolicy:
    """Track restart attempts and compute backoff delays."""

    _SCHEDULE = (0, 1, 5)

    def __init__(self) -> None:
        self.attempts = 0
        self.last_start = 0.0

    def record_start(self) -> None:
        self.last_start = time.time()

    def reset_attempts(self) -> None:
        self.attempts = 0

    def next_delay(self) -> int:
        delay = self._SCHEDULE[min(self.attempts, len(self._SCHEDULE) - 1)]
        self.attempts += 1
        return delay


_RESTART_POLICIES: dict[str, RestartPolicy] = {}


def _get_restart_policy(name: str) -> RestartPolicy:
    return _RESTART_POLICIES.setdefault(name, RestartPolicy())


def _launch_process(
    name: str,
    cmd: list[str],
    *,
    restart: bool = False,
    shutdown_timeout: int = 15,
    ref: str | None = None,
) -> RunnerManagedProcess:
    # NOTE: All child processes should include -v for verbose logging by default.
    # This ensures their output is captured in logs for debugging.
    """Launch process with automatic output logging and restart policy tracking."""
    policy: RestartPolicy | None = None
    if restart:
        policy = _get_restart_policy(name)

    # Generate ref if not provided
    ref = ref if ref else str(now_ms())

    # Use unified runner to spawn process (share supervisor's callosum)
    try:
        managed = RunnerManagedProcess.spawn(
            cmd, ref=ref, callosum=_supervisor_callosum
        )
    except RuntimeError as exc:
        logging.error(str(exc))
        raise

    if policy:
        policy.record_start()
    _SERVICE_STATE[name] = {
        "restart": restart,
        "shutdown_timeout": shutdown_timeout,
    }

    # Emit started event
    if _supervisor_callosum:
        _supervisor_callosum.emit(
            "supervisor",
            "started",
            service=name,
            pid=managed.process.pid,
            ref=managed.ref,
        )

    return managed


def _terminate_managed(
    managed: RunnerManagedProcess, timeout: float, *, reason: str
) -> None:
    logger.info("Terminating %s for %s", managed.name, reason)
    try:
        managed.terminate(timeout=timeout)
    except subprocess.TimeoutExpired:
        logger.warning(
            "%s did not terminate within %.1fs for %s",
            managed.name,
            timeout,
            reason,
        )


def _start_termination_thread(
    key: str, managed: RunnerManagedProcess, timeout: float, reason: str
) -> None:
    def run() -> None:
        try:
            _terminate_managed(managed, timeout, reason=reason)
        finally:
            with _termination_threads_lock:
                if _termination_threads.get(key) is threading.current_thread():
                    _termination_threads.pop(key, None)

    with _termination_threads_lock:
        existing = _termination_threads.get(key)
        if existing and existing.is_alive():
            return

        thread = threading.Thread(
            target=run,
            daemon=True,
            name=f"terminate-{key}",
        )
        _termination_threads[key] = thread
        thread.start()


def _stop_process(managed: RunnerManagedProcess) -> None:
    timeout = _SERVICE_STATE.get(managed.name, {}).get("shutdown_timeout", 15)
    _terminate_managed(managed, timeout, reason="shutdown")
    managed.cleanup()


def _record_scheduler_completion(
    scheduler_name: str,
    *,
    ended_at: float,
    exit_status: str,
    ref: str,
    cmd: list[str],
) -> None:
    health_dir = Path(get_journal()) / "health"
    health_dir.mkdir(parents=True, exist_ok=True)
    state_path = health_dir / "scheduler.json"
    with _SCHEDULER_JSON_LOCK:
        try:
            with open(state_path, "r", encoding="utf-8") as file:
                state = json.load(file)
        except FileNotFoundError:
            state = {}
        except (json.JSONDecodeError, OSError) as exc:
            logger.warning(
                "Failed to load scheduler state for completion write: %s", exc
            )
            state = {}

        current = state.get(scheduler_name)
        if not isinstance(current, dict):
            current = {}
        current.update(
            {
                "last_run": ended_at,
                "last_status": exit_status,
                "last_ref": ref,
            }
        )
        state[scheduler_name] = current

        fd, tmp_path = tempfile.mkstemp(
            dir=health_dir, suffix=".tmp", prefix=".scheduler_"
        )
        tmp_file = Path(tmp_path)
        try:
            with open(fd, "w", encoding="utf-8") as file:
                json.dump(state, file, indent=2)
            tmp_file.replace(state_path)
        except BaseException:
            tmp_file.unlink(missing_ok=True)
            raise


def _emit_queue_event(cmd_name: str, running_ref: str, queue: list) -> None:
    """Emit supervisor.queue event with current queue state for a command.

    This is the callback passed to TaskQueue for queue change notifications.
    """
    if not _supervisor_callosum:
        return

    _supervisor_callosum.emit(
        "supervisor",
        "queue",
        command=cmd_name,
        running=running_ref,
        queued=len(queue),
        queue=queue,
    )


def _maybe_submit_startup_digest(*, no_cortex: bool) -> None:
    """Submit the startup digest once when a local cortex substrate exists."""
    global _digest_submitted_this_boot

    if (
        _digest_submitted_this_boot
        or no_cortex
        or _is_remote_mode
        or _task_queue is None
    ):
        return

    _task_queue.submit(["sol", "call", "identity", "digest"])
    _digest_submitted_this_boot = True
    logging.info("startup: submitted identity digest")


def _handle_task_request(message: dict) -> None:
    """Handle incoming task request from Callosum."""
    if message.get("tract") != "supervisor" or message.get("event") != "request":
        return

    cmd = message.get("cmd")
    if not cmd:
        logging.error(f"Invalid task request: missing cmd: {message}")
        return

    ref = message.get("ref") or str(now_ms())
    day = message.get("day")
    scheduler_name = message.get("scheduler_name")
    if _task_queue:
        cmd_name = TaskQueue.get_command_name(cmd)
        active_ref = _task_queue.get_active_by_cmd_name(cmd_name)
        if active_ref:
            with _task_queue._lock:
                managed = _task_queue._active.get(active_ref)
                cap = _task_queue._caps.get(cmd_name, 0)
            runtime = time.time() - managed.start_time if managed else 0
            reason = "wedged" if cap and runtime > 2 * cap else "still_running"
            if _supervisor_callosum:
                _supervisor_callosum.emit(
                    "supervisor",
                    "skipped",
                    reason=reason,
                    ref=ref,
                    active_ref=active_ref,
                    cmd=cmd,
                    scheduler_name=scheduler_name,
                )
            return
        _task_queue.submit(cmd, ref, day=day, scheduler_name=scheduler_name)


def _restart_service(service: str) -> bool:
    """Terminate a managed service to trigger graceful restart.

    Returns True if the service was found and running, False if not found
    or already exited.
    """
    for proc in _managed_procs:
        if proc.name == service:
            if proc.process.poll() is not None:
                logging.debug(
                    f"Ignoring restart for {service}: already exited, awaiting auto-restart"
                )
                return False

            state = _SERVICE_STATE.setdefault(service, {})
            state["restart"] = True
            timeout = state.get("shutdown_timeout", 15)

            logging.info(f"Restart requested for {service}, terminating...")

            if _supervisor_callosum:
                _supervisor_callosum.emit(
                    "supervisor",
                    "restarting",
                    service=service,
                    pid=proc.process.pid,
                    ref=proc.ref,
                )

            _start_termination_thread(service, proc, timeout=timeout, reason="restart")
            return True

    logging.warning(f"Cannot restart {service}: not found in managed processes")
    return False


def _handle_supervisor_request(message: dict) -> None:
    """Handle incoming supervisor control messages."""
    if message.get("tract") != "supervisor" or message.get("event") != "restart":
        return

    service = message.get("service")
    if not service:
        logging.error("Invalid restart request: missing service")
        return
    if service == "supervisor":
        logging.debug("Ignoring restart request for supervisor itself")
        return

    _restart_service(service)


def get_task_status(ref: str) -> dict:
    """Get status of a task.

    Args:
        ref: Task correlation ID

    Returns:
        Dict with status info, or {"status": "not_found"} if task doesn't exist
    """
    if _task_queue:
        return _task_queue.get_status(ref)
    return {"status": "not_found"}


def collect_status(procs: list[RunnerManagedProcess]) -> dict:
    """Collect current supervisor status for broadcasting."""
    now = time.time()

    # Running services
    services = []
    running_names = set()
    for proc in procs:
        if proc.process.poll() is None:  # Still running
            policy = _get_restart_policy(proc.name)
            uptime = int(now - policy.last_start) if policy.last_start else 0
            services.append(
                {
                    "name": proc.name,
                    "ref": proc.ref,
                    "pid": proc.process.pid,
                    "uptime_seconds": uptime,
                }
            )
            running_names.add(proc.name)

    # Prepend supervisor itself
    if _supervisor_ref and _supervisor_start:
        services.insert(
            0,
            {
                "name": "supervisor",
                "ref": _supervisor_ref,
                "pid": os.getpid(),
                "uptime_seconds": int(now - _supervisor_start),
            },
        )

    # Crashed services (in restart backoff)
    crashed = []
    for name, policy in _RESTART_POLICIES.items():
        if name not in running_names and policy.attempts > 0:
            crashed.append(
                {
                    "name": name,
                    "restart_attempts": policy.attempts,
                }
            )

    # Running tasks
    tasks = _task_queue.collect_task_status() if _task_queue else []
    queues = _task_queue.collect_queue_counts() if _task_queue else {}

    # Scheduled tasks
    schedules = scheduler.collect_status()
    # Connected callosum clients
    callosum_clients = _callosum_server.client_count() if _callosum_server else 0

    return {
        "services": services,
        "crashed": crashed,
        "tasks": tasks,
        "queues": queues,
        "stale_heartbeats": [],
        "schedules": schedules,
        "callosum_clients": callosum_clients,
    }


def start_sense() -> RunnerManagedProcess:
    """Launch journal sense with output logging."""
    return _launch_process("sense", ["journal", "sense", "-v"], restart=True)


def start_local_server() -> RunnerManagedProcess | None:
    """Launch the supervisor-owned local llama-server when artifacts are present."""
    from solstone.think.providers import local_install, local_server

    try:
        binary_path, gguf_path, mmproj_path = local_install.ensure_artifacts_installed(
            LOCAL_MODEL
        )
        # Defense in depth: refuse to launch a gguf/mmproj pair that does not
        # belong to the selected model, even if readiness ever regresses. A
        # mixed pair (e.g. a stale gguf from a prior model + the current mmproj)
        # aborts llama-server with an n_embd mismatch, so skip startup instead.
        expected_dir = local_install.model_dir(LOCAL_MODEL)
        if gguf_path.parent != expected_dir or (
            mmproj_path is not None and mmproj_path.parent != expected_dir
        ):
            raise RuntimeError(
                f"local model artifacts are not under {expected_dir} "
                f"(gguf={gguf_path}, mmproj={mmproj_path}); refusing to launch "
                "a mismatched gguf/mmproj pair"
            )
    except Exception as exc:
        logging.info("Local model not ready; skipping llama-server startup: %s", exc)
        return None

    port = find_available_port()
    write_service_port("local", port)
    cmd = [
        str(binary_path),
        "-m",
        str(gguf_path),
        "--alias",
        LOCAL_MODEL,
        "--host",
        "127.0.0.1",
        "--port",
        str(port),
        "--jinja",
    ]
    if mmproj_path is not None:
        cmd.extend(["--mmproj", str(mmproj_path)])
    if "0.0.0.0" in cmd:
        raise RuntimeError("Local server may not bind 0.0.0.0.")

    managed = _launch_process("llama-server", cmd, restart=True)
    print(f"  {LOCAL_MODEL_WARMING_UP_COPY}", flush=True)

    deadline = time.monotonic() + LOCAL_SERVER_READY_TIMEOUT_S
    while time.monotonic() < deadline:
        if managed.process.poll() is not None:
            logging.warning(
                "llama-server exited during warmup with code %s",
                managed.process.returncode,
            )
            return managed
        state, error = local_server._probe_health(port)
        if state == local_server.STATE_READY:
            logging.info("llama-server ready on port %s", port)
            return managed
        if state == local_server.STATE_FAILED and error:
            logging.debug("llama-server health probe failed during warmup: %s", error)
        time.sleep(LOCAL_SERVER_HEALTH_POLL_INTERVAL_S)

    logging.warning(
        "llama-server did not become ready within %.0fs; continuing startup",
        LOCAL_SERVER_READY_TIMEOUT_S,
    )
    return managed


def start_callosum_in_process() -> CallosumServer:
    """Start Callosum message bus server in-process.

    Runs the server in a background thread and waits for socket to be ready.

    Returns:
        CallosumServer instance
    """
    global _callosum_server, _callosum_thread

    server = CallosumServer()
    _callosum_server = server

    # Pre-delete stale socket to avoid race condition where the ready check
    # passes due to an old socket file before the server thread deletes it
    socket_path = server.socket_path
    socket_path.parent.mkdir(parents=True, exist_ok=True)
    if socket_path.exists():
        socket_path.unlink()

    # Start server in background thread (server.start() is blocking)
    thread = threading.Thread(target=server.start, daemon=False, name="callosum-server")
    thread.start()
    _callosum_thread = thread

    # Wait for socket to be ready (with timeout)
    for _ in range(50):  # Wait up to 500ms
        if socket_path.exists():
            logging.info(f"Callosum server started on {socket_path}")
            return server
        time.sleep(0.01)

    raise RuntimeError("Callosum server failed to create socket within 500ms")


def wait_for_convey_ready(
    convey_mp, *, timeout: float = 30.0, interval: float = 0.1
) -> bool:
    """Poll until Convey accepts TCP connections, or fail fast on death/timeout."""
    start = time.monotonic()
    deadline = start + timeout
    while time.monotonic() < deadline:
        rc = convey_mp.process.poll()
        if rc is not None:
            logging.error(
                "Convey process exited during startup (rc=%d); continuing into supervise loop",
                rc,
            )
            return False
        if is_solstone_up(timeout=0.1):
            logging.info("Convey ready after %.1fs", time.monotonic() - start)
            return True
        time.sleep(interval)
    alive = convey_mp.process.poll() is None
    logging.error(
        "Convey not ready after %.1fs (port=%s, pid alive=%s); continuing into supervise loop",
        time.monotonic() - start,
        read_service_port("convey"),
        alive,
    )
    return False


def stop_callosum_in_process() -> None:
    """Stop the in-process Callosum server."""
    global _callosum_server, _callosum_thread

    if _callosum_server:
        logging.info("Stopping Callosum server...")
        _callosum_server.stop()

    if _callosum_thread:
        _callosum_thread.join(timeout=5)
        if _callosum_thread.is_alive():
            logging.warning("Callosum server thread did not stop cleanly")

    _callosum_server = None
    _callosum_thread = None


def start_cortex_server() -> RunnerManagedProcess:
    """Launch the Cortex WebSocket API server."""
    cmd = ["journal", "cortex", "-v"]
    return _launch_process("cortex", cmd, restart=True)


def start_link_server() -> RunnerManagedProcess:
    """Launch the link tunnel service (spl home-side endpoint)."""
    cmd = ["sol", "link", "-v"]
    return _launch_process("link", cmd, restart=True)


def start_convey_server(
    verbose: bool, debug: bool = False, port: int = 0
) -> tuple[RunnerManagedProcess, int]:
    """Launch the Convey web application with optional verbose and debug logging.

    Returns:
        Tuple of (RunnerManagedProcess, resolved_port) where resolved_port is the
        actual port being used (auto-selected if port was 0).
    """
    # Resolve port 0 to an available port before launching
    resolved_port = port if port != 0 else find_available_port()

    cmd = ["journal", "convey", "--port", str(resolved_port)]
    if debug:
        cmd.append("-d")
    elif verbose:
        cmd.append("-v")
    return _launch_process("convey", cmd, restart=True), resolved_port


def check_runner_exits(
    procs: list[RunnerManagedProcess],
) -> list[RunnerManagedProcess]:
    """Return managed processes that have exited."""

    exited: list[RunnerManagedProcess] = []
    for managed in procs:
        if managed.process.poll() is not None:
            exited.append(managed)
    return exited


async def handle_runner_exits(procs: list[RunnerManagedProcess]) -> None:
    """Check for and handle exited processes with restart policy."""
    exited = check_runner_exits(procs)
    if not exited:
        return

    exited_names = [managed.name for managed in exited]

    # Check if all exits are tempfail (session not ready)
    all_tempfail = all(m.process.returncode == EXIT_TEMPFAIL for m in exited)

    if all_tempfail:
        logging.info("Runner waiting for session: %s", ", ".join(sorted(exited_names)))
    else:
        msg = f"Runner process exited: {', '.join(sorted(exited_names))}"
        logging.error(msg)

    for managed in exited:
        returncode = managed.process.returncode
        is_tempfail = returncode == EXIT_TEMPFAIL
        logging.info("%s exited with code %s", managed.name, returncode)

        # Emit stopped event
        if _supervisor_callosum:
            _supervisor_callosum.emit(
                "supervisor",
                "stopped",
                service=managed.name,
                pid=managed.process.pid,
                ref=managed.ref,
                exit_code=returncode,
            )

        # Remove from procs list
        try:
            procs.remove(managed)
        except ValueError:
            pass

        managed.cleanup()

        # Handle restart if needed
        restart = _SERVICE_STATE.get(managed.name, {}).get("restart", False)
        if restart and not shutdown_requested:
            # Tempfail: use fixed longer delay, don't burn through backoff
            if is_tempfail:
                delay = TEMPFAIL_DELAY
            else:
                policy = _get_restart_policy(managed.name)
                uptime = time.time() - policy.last_start if policy.last_start else 0
                if uptime >= 60:
                    policy.reset_attempts()
                delay = policy.next_delay()
            if delay:
                logging.info("Waiting %ss before restarting %s", delay, managed.name)
                for _ in range(delay):
                    if shutdown_requested:
                        break
                    await asyncio.sleep(1)
            if shutdown_requested:
                continue
            logging.info("Restarting %s...", managed.name)
            try:
                state = _SERVICE_STATE.get(managed.name, {})
                new_proc = _launch_process(
                    managed.name,
                    managed.cmd,
                    restart=True,
                    shutdown_timeout=state.get("shutdown_timeout", 15),
                )
            except Exception as exc:
                logging.exception("Failed to restart %s: %s", managed.name, exc)
                continue

            procs.append(new_proc)
            logging.info("Restarted %s after exit code %s", managed.name, returncode)
        else:
            logging.info("Not restarting %s", managed.name)


def handle_daily_tasks() -> None:
    """Check for day change and submit daily think for updated days (non-blocking).

    Triggers once when the day rolls over at midnight.  Queries ``updated_days()``
    for journal days that have new stream data but haven't completed a daily
    think yet, then submits up to ``MAX_UPDATED_CATCHUP`` thinks in chronological
    order (oldest first, yesterday last) via the TaskQueue.

    Think auto-detects updated state and enables ``--refresh`` internally, so we
    don't pass it here.

    Skipped in remote mode (no local data to process).
    """
    # Remote mode: no local processing, data is on the server
    if _is_remote_mode:
        return

    today = datetime.now().date()

    # Only trigger when day actually changes (at midnight)
    if today != _daily_state["last_day"]:
        # The day that just ended is what we process
        prev_day = _daily_state["last_day"]

        # Guard against None (e.g., module reloaded without going through main())
        if prev_day is None:
            logging.warning("Daily state not initialized, skipping daily processing")
            _daily_state["last_day"] = today
            return

        prev_day_str = prev_day.strftime("%Y%m%d")

        # Update state for new day
        _daily_state["last_day"] = today

        # Flush any dangling segment state from the previous day before daily think
        if not _flush_state["flushed"] and _flush_state["day"] == prev_day_str:
            _check_segment_flush(force=True)

        today_str = today.strftime("%Y%m%d")
        all_updated = updated_days(exclude={today_str})

        if not all_updated:
            logging.info("Day changed to %s, no updated days to process", today)
            return

        # Take the newest MAX_UPDATED_CATCHUP days (already sorted ascending)
        days_to_process = all_updated[-MAX_UPDATED_CATCHUP:]
        skipped = len(all_updated) - len(days_to_process)

        if skipped:
            logging.warning(
                "Skipping %d older updated days (max catchup %d): %s",
                skipped,
                MAX_UPDATED_CATCHUP,
                all_updated[:skipped],
            )

        logging.info(
            "Day changed to %s, queuing daily think for %d updated day(s): %s",
            today,
            len(days_to_process),
            days_to_process,
        )

        # Submit oldest-first so yesterday is processed last
        for day_str in days_to_process:
            cmd = ["journal", "think", "-v", "--day", day_str]
            if _task_queue:
                _task_queue.submit(cmd, day=day_str)
                logging.debug("Submitted daily think for %s", day_str)
            else:
                logging.warning(
                    "No task queue available for daily processing: %s", day_str
                )


def _handle_segment_observed(message: dict) -> None:
    """Handle segment completion events (from live observation or imports).

    Submits journal think in segment mode via task queue, which handles both
    generators and segment agents. Also updates flush state to track
    segment recency.
    """
    if message.get("tract") != "observe" or message.get("event") != "observed":
        return

    segment = message.get("segment")  # e.g., "163045_300"
    if not segment:
        logging.warning("observed event missing segment field")
        return

    # Use day from event payload, fallback to today (for live observation)
    day = message.get("day") or datetime.now().strftime("%Y%m%d")

    # Batch/historical re-sensing heals deterministically via daily catchup's
    # segment-think pre-phase. A lone volatile segment think for a re-sensed
    # past segment can rewind live activity-timeline state, so submit nothing;
    # also leave flush state untouched so stale segments cannot reset
    # _check_segment_flush or pollute handle_daily_tasks' force-flush gate.
    if message.get("batch"):
        logging.info(
            "Batch observed segment deferred to daily catchup; "
            "no volatile segment think submitted: %s/%s",
            day,
            segment,
        )
        return

    stream = message.get("stream")

    # Update flush state — new segment resets the flush timer
    _flush_state["last_segment_ts"] = time.time()
    _flush_state["day"] = day
    _flush_state["segment"] = segment
    _flush_state["stream"] = stream
    _flush_state["flushed"] = False

    logging.info(f"Segment observed: {day}/{segment}, submitting processing...")

    # Submit via task queue — serializes with other think invocations
    cmd = ["journal", "think", "-v", "--day", day, "--segment", segment]
    if stream:
        cmd.extend(["--stream", stream])
    if not message.get("batch"):
        cmd.append("--live")
    if _task_queue:
        _task_queue.submit(cmd, day=day)
    else:
        logging.warning(
            "No task queue available for segment processing: %s/%s", day, segment
        )


def _check_segment_flush(force: bool = False) -> None:
    """Check if the last observed segment needs flushing.

    If no new segments have arrived within FLUSH_TIMEOUT seconds, runs
    ``journal think --flush`` on the last segment to let flush-enabled agents
    close out dangling state (e.g., end active activities).

    Args:
        force: Skip timeout check (used at day boundary to flush
               before daily think regardless of elapsed time).

    Skipped in remote mode (no local processing).
    """
    if _is_remote_mode:
        return

    last_ts = _flush_state["last_segment_ts"]
    if not last_ts or _flush_state["flushed"]:
        return

    if not force and time.time() - last_ts < FLUSH_TIMEOUT:
        return

    day = _flush_state["day"]
    segment = _flush_state["segment"]
    if not day or not segment:
        return

    _flush_state["flushed"] = True

    stream = _flush_state.get("stream")
    cmd = ["journal", "think", "-v", "--day", day, "--segment", segment, "--flush"]
    if stream:
        cmd.extend(["--stream", stream])
    if _task_queue:
        _task_queue.submit(cmd, day=day)
        logging.info(f"Queued segment flush: {day}/{segment}")
    else:
        logging.warning(
            "No task queue available for segment flush: %s/%s", day, segment
        )


def _handle_segment_event_log(message: dict) -> None:
    """Log observe, think, and activity events with day+segment to segment/events.jsonl.

    Any observe, think, or activity tract message with both day and segment fields
    gets logged to journal/day/segment/events.jsonl if that directory exists.
    """
    if message.get("tract") not in {"observe", "think", "activity"}:
        return

    day = message.get("day")
    segment = message.get("segment")

    if not day or not segment:
        return

    stream = message.get("stream")

    try:
        if stream:
            segment_dir = day_path(day, create=False) / stream / segment
        else:
            segment_dir = day_path(day, create=False) / segment

        # Only log if segment directory exists
        if not segment_dir.is_dir():
            return

        events_file = segment_dir / "events.jsonl"

        # Append event as JSON line
        with open(events_file, "a", encoding="utf-8") as f:
            f.write(json.dumps(message, ensure_ascii=False) + "\n")

    except Exception as e:
        logging.debug(f"Failed to log segment event: {e}")


def _handle_activity_recorded(message: dict) -> None:
    """Queue a per-activity think task when an activity is recorded.

    Listens for activity.recorded events and submits a queued think task
    for per-activity agent processing (serialized via TaskQueue).
    """
    if message.get("tract") != "activity" or message.get("event") != "recorded":
        return

    record_id = message.get("id")
    facet = message.get("facet")
    day = message.get("day")

    if not record_id or not facet or not day:
        logging.warning("activity.recorded event missing required fields")
        return

    cmd = ["journal", "think", "--activity", record_id, "--facet", facet, "--day", day]

    if _task_queue:
        _task_queue.submit(cmd, day=day)
        logging.info(f"Queued activity think: {record_id} for #{facet}")
    else:
        logging.warning("No task queue available for activity think: %s", record_id)


def _handle_think_daily_complete(message: dict) -> None:
    """Submit a heartbeat task after daily think processing completes.

    Listens for think.daily_complete events. Skips if a heartbeat process
    is already running (PID file guard).
    """
    if message.get("tract") != "think" or message.get("event") != "daily_complete":
        return

    # Check if heartbeat is already running via PID file
    pid_file = Path(get_journal()) / "health" / "heartbeat.pid"
    if pid_file.exists():
        try:
            existing_pid = int(pid_file.read_text().strip())
            os.kill(existing_pid, 0)
            logging.info("Heartbeat already running (pid=%d), skipping", existing_pid)
            return
        except ProcessLookupError:
            pass  # Stale PID file, proceed
        except PermissionError:
            logging.info(
                "Heartbeat running under different user (pid file exists), skipping"
            )
            return
        except ValueError:
            pass  # Corrupt PID file, proceed

    cmd = ["journal", "heartbeat"]
    if _task_queue:
        _task_queue.submit(cmd)
        logging.info("Queued heartbeat after daily think completion")
    else:
        logging.warning("No task queue available for heartbeat submission")


def _handle_callosum_message(message: dict) -> None:
    """Dispatch incoming Callosum messages to appropriate handlers."""
    _handle_task_request(message)
    _handle_supervisor_request(message)
    _handle_segment_observed(message)
    _handle_activity_recorded(message)
    _handle_think_daily_complete(message)
    _handle_segment_event_log(message)


def _run_sync_tick(now: float) -> bool:
    """Write this supervisor's heartbeat and stop on live foreign writers."""
    global _last_sync_tick, _last_sync_snapshot, _sync_conflict_shutdown
    global shutdown_requested

    if now - _last_sync_tick < DEFAULT_INTERVAL_SECONDS:
        return True

    try:
        write_self_heartbeat()
        result = check_journal_sync(previous=_last_sync_snapshot)
        _last_sync_snapshot = result.snapshot
        _last_sync_tick = now
        if not result.is_conflict:
            return True

        primary = result.primary_conflict
        if primary is None:
            return True

        machine_prefix = primary.machine_id[:8] if primary.machine_id else "(unknown)"
        logging.error(
            "Another solstone instance is writing to this journal "
            "(host=%s pid=%s machine=%s) - shutting down.",
            primary.display_hostname,
            primary.pid,
            machine_prefix,
        )
        if _supervisor_callosum:
            try:
                _supervisor_callosum.emit(
                    "supervisor",
                    "sync_conflict",
                    hostname=primary.display_hostname,
                    journal_path=primary.journal_path,
                    pid=primary.pid,
                    machine_id_prefix=primary.machine_id[:8]
                    if primary.machine_id
                    else "",
                    wall_time=datetime.now(timezone.utc)
                    .isoformat(timespec="seconds")
                    .replace("+00:00", "Z"),
                )
            except Exception:
                logging.exception("Failed to emit sync_conflict event")
        shutdown_requested = True
        _sync_conflict_shutdown = True
        return False
    except Exception:
        logging.exception("Sync conflict check failed (continuing)")
        return True


async def supervise(
    *,
    daily: bool = True,
    schedule: bool = True,
    procs: list[RunnerManagedProcess] | None = None,
) -> None:
    """Main supervision loop. Runs at 1-second intervals for responsiveness.

    Monitors runner health, emits status, triggers daily processing,
    and checks scheduled agents.
    """
    global _last_sync_tick, _last_sync_snapshot, _sync_conflict_shutdown

    last_status_emit = 0.0
    _last_sync_tick = 0.0
    _last_sync_snapshot = None
    _sync_conflict_shutdown = False

    try:
        while (
            not shutdown_requested
        ):  # pragma: no cover - loop checked via unit tests by patching
            if _task_queue:
                _task_queue.enforce_deadlines(time.time())

            # Check for runner exits first (immediate alert)
            if procs:
                await handle_runner_exits(procs)

            # Emit status every 5 seconds
            now = time.time()
            if now - last_status_emit >= 5:
                if _supervisor_callosum and procs:
                    try:
                        status = collect_status(procs)
                        _supervisor_callosum.emit("supervisor", "status", **status)
                    except Exception as e:
                        logging.debug(f"Status emission failed: {e}")
                last_status_emit = now

            # Check for segment flush (non-blocking, submits via task queue)
            _check_segment_flush()

            # Check for journal sync conflicts (usually just heartbeat IO)
            if not _run_sync_tick(now):
                return

            # Check for daily processing (non-blocking, submits via task queue)
            if daily:
                handle_daily_tasks()

            # Check periodic task schedules (non-blocking, submits via callosum)
            if schedule:
                scheduler.check()
                routines.check()

            # Sleep 1 second before next iteration (responsive to shutdown)
            await asyncio.sleep(1)
    finally:
        pass  # Callosum cleanup happens in main()


def parse_args() -> argparse.ArgumentParser:
    parser = SupervisorArgumentParser(description="Monitor journaling health")
    parser.add_argument(
        "port",
        nargs="?",
        type=int,
        default=0,
        help="Convey port (0 = auto-select available port)",
    )
    parser.add_argument(
        "--threshold",
        type=int,
        default=DEFAULT_THRESHOLD,
        help="Seconds before heartbeat considered stale",
    )
    parser.add_argument(
        "--interval", type=int, default=CHECK_INTERVAL, help="Polling interval seconds"
    )
    parser.add_argument(
        "--no-daily",
        action="store_true",
        help="Disable daily processing run at midnight",
    )
    parser.add_argument(
        "--no-cortex",
        action="store_true",
        help="Do not start the Cortex server (run it manually for debugging)",
    )
    parser.add_argument(
        "--no-link",
        action="store_true",
        help="Do not start the link tunnel service",
    )
    parser.add_argument(
        "--no-convey",
        action="store_true",
        help="Do not start the Convey web application",
    )
    parser.add_argument(
        "--no-schedule",
        action="store_true",
        help="Disable periodic task scheduler",
    )
    parser.add_argument(
        "--remote",
        type=str,
        help="Remote mode: URL for segment transfer (not yet implemented)",
    )
    return parser


def handle_shutdown(signum, frame):
    """Handle shutdown signals gracefully."""
    global shutdown_requested
    if not shutdown_requested:
        shutdown_requested = True
        logger.info("shutdown requested via signal %d", signum)
        live = [managed for managed in _managed_procs if managed.is_running()]
        if live:
            logger.info("shutdown: signaling %d managed child(ren)", len(live))
            for managed in live:
                try:
                    managed.process.terminate()
                except Exception:
                    logger.exception("shutdown: terminate failed for %s", managed.name)

            deadline = time.monotonic() + 3.0
            while time.monotonic() < deadline:
                if all(not managed.is_running() for managed in live):
                    break
                time.sleep(0.05)

            kills = 0
            for managed in live:
                if managed.is_running():
                    try:
                        managed.process.kill()
                        logger.warning(
                            "shutdown: SIGKILL pid=%s name=%s",
                            managed.process.pid,
                            managed.name,
                        )
                        kills += 1
                    except Exception:
                        logger.exception("shutdown: kill failed for %s", managed.name)

            cleanly = len(live) - kills
            logger.info(
                "shutdown: reap complete (%d exited cleanly, %d SIGKILL'd)",
                cleanly,
                kills,
            )
        raise KeyboardInterrupt
    # Second signal during shutdown: cleanup is already in progress.


def _ensure_venv_bin_on_path() -> None:
    """Prepend the venv bin dir (sibling of sys.executable) to PATH if absent.

    Idempotent — safe to call repeatedly. Lets the supervisor spawn `sol` and
    other venv-installed entry points even when the operator's shell PATH does
    not include the venv bin dir.
    """
    venv_bin = os.path.dirname(sys.executable)
    parts = os.environ.get("PATH", "").split(os.pathsep)
    if parts and parts[0] == venv_bin:
        return
    parts = [venv_bin] + [p for p in parts if p != venv_bin]
    os.environ["PATH"] = os.pathsep.join(parts)


def main() -> None:
    parser = parse_args()

    # Capture journal info BEFORE setup_cli() loads .env and pollutes os.environ
    journal_info = get_journal_info()

    args = setup_cli(parser)
    _ensure_venv_bin_on_path()

    journal_path = _get_journal_path()

    log_level = logging.DEBUG if args.debug else logging.INFO
    log_path = journal_path / "health" / "supervisor.log"
    log_path.parent.mkdir(parents=True, exist_ok=True)
    logging.getLogger().handlers = []
    logging.basicConfig(
        level=log_level,
        handlers=[logging.FileHandler(log_path, encoding="utf-8")],
        format="%(asctime)s [supervisor:log] %(levelname)s %(message)s",
        datefmt="%Y-%m-%dT%H:%M:%S",
    )

    if args.verbose or args.debug:
        console_handler = logging.StreamHandler()
        console_handler.setLevel(log_level)
        console_handler.setFormatter(
            logging.Formatter("%(asctime)s %(levelname)s %(message)s")
        )
        logging.getLogger().addHandler(console_handler)

    # Singleton guard: only one supervisor per journal
    health_dir = journal_path / "health"
    lock_path = health_dir / "supervisor.lock"
    pid_path = health_dir / "supervisor.pid"
    import fcntl

    lock_fd = open(lock_path, "w")
    try:
        fcntl.flock(lock_fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except OSError:
        lock_fd.close()
        pid_str = ""
        try:
            pid_str = pid_path.read_text().strip()
        except OSError:
            pass
        pid_msg = f" (PID {pid_str})" if pid_str else ""
        if os.environ.get("INVOCATION_ID"):
            holder_pid = pid_str or "unknown"
            print(
                "Supervisor already running "
                f"(PID {holder_pid}) - exiting cleanly under systemd activation"
            )
            sys.exit(0)
        sock_path = health_dir / "callosum.sock"
        if sock_path.exists():
            try:
                from solstone.think.health_cli import health_check

                print(f"Supervisor already running{pid_msg}\n")
                health_check()
            except Exception:
                print(f"Supervisor already running{pid_msg}")
        else:
            print(f"Supervisor already running{pid_msg}")
        sys.exit(1)

    print(
        "Checking for other active solstone instances on this journal...",
        flush=True,
    )
    snapshot = check_journal_sync(journal=journal_path)
    if snapshot.is_conflict:
        print(format_conflict_message(snapshot), file=sys.stderr)
        try:
            lock_fd.close()
        except Exception:
            pass
        sys.exit(1)
    write_self_heartbeat(journal=journal_path)

    pid_path.write_text(str(os.getpid()))
    start_time_path = health_dir / "supervisor.start_time"
    # Written here, not at _supervisor_start, to minimize drift from psutil create_time().
    start_time_path.write_text(str(time.time()))
    logging.info("Singleton lock acquired (PID %d)", os.getpid())
    _sweep_orphaned_sol_processes(journal_path)

    # Set up signal handlers
    signal.signal(signal.SIGINT, handle_shutdown)
    signal.signal(signal.SIGTERM, handle_shutdown)

    # Show journal path and source on startup
    path, source = journal_info
    print(f"Journal: {path} (from {source})")
    logging.info("Supervisor starting...")

    global _managed_procs, _supervisor_callosum, _is_remote_mode
    global _digest_submitted_this_boot
    global _task_queue
    procs: list[RunnerManagedProcess] = []
    convey_port = None

    # Remote mode: run sync instead of local processing
    _is_remote_mode = bool(args.remote)
    _digest_submitted_this_boot = False

    # Run pending journal-maintenance tasks before spawning any writer children.
    # Callosum isn't up yet (emit_fn=None); migrations log through supervisor's logger only.
    try:
        ran, succeeded = run_pending_tasks(journal_path, emit_fn=None)
        if ran > 0:
            print(f"  Ran {ran} maintenance task(s)", flush=True)
            if ran == succeeded:
                logging.info("Completed %d/%d maintenance task(s)", succeeded, ran)
            else:
                logging.error(
                    "Maintenance tasks completed with failures: %d/%d succeeded",
                    succeeded,
                    ran,
                )
    except Exception:
        logging.exception("Maintenance runner raised; continuing startup")

    try:
        from solstone.think.importers.journal_archive import sweep_stale_extract_dirs

        swept = sweep_stale_extract_dirs()
        if swept > 0:
            logging.info("Swept %d stale journal-archive extract dir(s)", swept)
    except Exception:
        logging.exception("Journal archive extract sweep raised; continuing startup")

    # Start Callosum in-process first - it's the message bus that other services depend on
    try:
        print("  Starting Callosum bus...", flush=True)
        start_callosum_in_process()
    except RuntimeError as e:
        logging.error(f"Failed to start Callosum server: {e}")
        parser.error(f"Failed to start Callosum server: {e}")
        return

    # Connect supervisor's Callosum client to capture startup events from other services
    try:
        _supervisor_callosum = CallosumConnection(defaults={"rev": get_rev()})
        _supervisor_callosum.start(callback=_handle_callosum_message)
        logging.info("Supervisor connected to Callosum")
    except Exception as e:
        logging.warning(f"Failed to start Callosum connection: {e}")

    # Mirror supervisor log output to callosum logs tract (best-effort)
    supervisor_ref = str(now_ms())
    global _supervisor_ref, _supervisor_start
    _supervisor_ref = supervisor_ref
    _supervisor_start = time.time()
    if _supervisor_callosum:
        try:
            handler = CallosumLogHandler(_supervisor_callosum, supervisor_ref)
            handler.setFormatter(
                logging.Formatter("%(asctime)s %(levelname)s %(message)s")
            )
            logging.getLogger().addHandler(handler)
        except Exception:
            pass

    # Initialize task queue with callosum event callback
    _task_queue = TaskQueue(on_queue_change=_emit_queue_event, ready=False)

    # Now start other services (their startup events will be captured)
    if _is_remote_mode:
        # Remote mode: transfer send will be added here
        pass
    else:
        # Local mode: convey first, then sense for file processing
        os.environ["SOL_SUPERVISOR_SPAWNED"] = "1"
        if not args.no_convey:
            print(f"  Starting convey on port {args.port}...", flush=True)
            proc, convey_port = start_convey_server(
                verbose=args.verbose, debug=args.debug, port=args.port
            )
            procs.append(proc)
            wait_for_convey_ready(proc)
            print("  Convey ready", flush=True)
        if is_local_provider_needed():
            proc = start_local_server()
            if proc is not None:
                procs.append(proc)
        # Sense handles file processing
        print("  Starting sense...", flush=True)
        procs.append(start_sense())
        # Cortex for agent execution
        if not args.no_cortex:
            print("  Starting cortex...", flush=True)
            procs.append(start_cortex_server())
        # Link tunnel service (opt-out via --no-link)
        if not args.no_link:
            print("  Starting link...", flush=True)
            procs.append(start_link_server())

    # Make procs accessible to restart handler
    _managed_procs = procs

    # Initialize daily state to today - think only triggers at midnight when day changes
    _daily_state["last_day"] = datetime.now().date()

    # Initialize periodic task scheduler
    schedule_enabled = not args.no_schedule and not _is_remote_mode
    if schedule_enabled and _supervisor_callosum:
        scheduler.init(_supervisor_callosum)
        scheduler.register_defaults()
        if _task_queue:
            for cmd, seconds in scheduler.collect_runtime_caps():
                cmd_name = TaskQueue.get_command_name(cmd)
                _task_queue.set_cap(cmd_name, seconds)
                logging.info(
                    "Registered max_runtime cap for %s: %ss",
                    cmd_name,
                    seconds,
                )
        routines.init(_supervisor_callosum)

    if _task_queue:
        _task_queue.set_ready()
        _maybe_submit_startup_digest(no_cortex=args.no_cortex)

    # Show Convey URL if running
    if convey_port:
        print(f"Convey: http://localhost:{convey_port}/")

    logging.info(f"Started {len(procs)} processes, entering supervision loop")
    daily_enabled = not args.no_daily and not _is_remote_mode
    if daily_enabled:
        logging.info("Daily processing scheduled for midnight")

    # Startup catchup: submit thinks for days with pending stream data
    if daily_enabled:
        all_updated = updated_days()
        if all_updated:
            days_to_process = all_updated[-MAX_UPDATED_CATCHUP:]
            skipped = len(all_updated) - len(days_to_process)

            if skipped:
                logging.warning(
                    "Startup catchup: skipping %d older updated days (max %d): %s",
                    skipped,
                    MAX_UPDATED_CATCHUP,
                    all_updated[:skipped],
                )

            logging.info(
                "Startup catchup: submitted %d day(s) with pending stream data: %s",
                len(days_to_process),
                days_to_process,
            )

            for day_str in days_to_process:
                cmd = ["journal", "think", "-v", "--day", day_str]
                if _task_queue:
                    _task_queue.submit(cmd, day=day_str)
                    logging.debug("Startup catchup: submitted think for %s", day_str)
                else:
                    logging.warning(
                        "No task queue available for startup catchup: %s", day_str
                    )

    try:
        print("  Supervisor ready", flush=True)
        _sd_notify("READY=1")
        signal_ready()
        asyncio.run(
            supervise(
                daily=daily_enabled,
                schedule=schedule_enabled,
                procs=procs if procs else None,
            )
        )
    except KeyboardInterrupt:
        logging.info("Caught KeyboardInterrupt, shutting down...")
    finally:
        try:
            clear_ready()
        except Exception as exc:
            logging.warning("Failed to clear readiness marker during shutdown: %s", exc)
        try:
            if not _sync_conflict_shutdown:
                clear_self_heartbeat()
        except Exception as exc:
            logging.warning("Failed to clear sync heartbeat during shutdown: %s", exc)

        logging.info("Stopping all processes...")
        print("\nShutting down gracefully (this may take a moment)...", flush=True)

        if _task_queue:
            _task_queue.shutdown(timeout=10)

        # Stop services in reverse order
        for managed in reversed(procs):
            _stop_process(managed)

        if schedule_enabled:
            try:
                routines.save_state()
            except Exception as exc:
                logging.warning("Failed to save routines state on shutdown: %s", exc)

        # Disconnect supervisor's Callosum connection
        if _supervisor_callosum:
            _supervisor_callosum.stop()
            logging.info("Supervisor disconnected from Callosum")

        # Stop in-process Callosum server last
        stop_callosum_in_process()

        logging.info("Supervisor shutdown complete.")
        print("Shutdown complete.", flush=True)

    if _sync_conflict_shutdown:
        sys.exit(2)


if __name__ == "__main__":
    main()
