# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import argparse
import asyncio
import concurrent.futures
import fcntl
import getpass
import json
import logging
import os
import platform
import signal
import socket
import stat
import subprocess
import sys
import tempfile
import threading
import time
import uuid
from collections import OrderedDict, deque
from dataclasses import dataclass, field, replace
from datetime import datetime, timezone
from logging.handlers import RotatingFileHandler
from pathlib import Path
from typing import Any, Callable, Iterable, Literal, NoReturn, cast

import psutil

from solstone.observe.transcribe.config import confidential_audio_enabled
from solstone.observe.transcribe.resource import (
    STT_SURFACE,
    local_stt_backend,
    resolve_stt_backend_choice,
    stt_local_floor_bytes,
)
from solstone.think import core_handshake, maintenance, scheduler
from solstone.think.app_supervised import FLAG, is_app_supervised, resolve_parent_fd
from solstone.think.backup.engine import BACKUP_MAX_RUNTIME, BACKUP_RUN_CMD
from solstone.think.callosum import CallosumConnection, CallosumServer
from solstone.think.catchup_state import (
    KIND_DAILY_CATCHUP,
    KIND_SEGMENT_REPAIR,
    STUCK_THRESHOLD,
    day_eligible_to_drain,
    read_catchup_state,
    reconcile_interrupted_attempts,
    record_attempt,
    record_outcome,
)
from solstone.think.display_powersave import (
    poll_display_powersave,
    reset_display_powersave_monitor,
)
from solstone.think.journal_config import read_journal_config
from solstone.think.maint import run_pending_tasks
from solstone.think.models import (
    LOCAL_MODEL,
    is_local_provider_needed,
    no_thinking_engine_chosen,
)
from solstone.think.processing import (
    DISPLAY_POWERSAVE_UNAVAILABLE,
    evaluate_drain_gate,
    load_processing_settings,
)
from solstone.think.providers.install_state import (
    IN_FLIGHT_STATES,
    InstallStatusMalformedError,
    canonical_fingerprint,
    fingerprint_sha256,
    read_install_status,
)
from solstone.think.providers.memory import read_available_bytes
from solstone.think.providers.mlx_server import MLX_SERVER_PROCESS_NAME
from solstone.think.providers.runtime_health import (
    ADMISSION_ONLY_REASON_CODES,
    ReasonCode,
    RuntimeHealthConflictError,
    RuntimeHealthMalformedError,
    RuntimeHealthRecord,
    RuntimeHealthUnavailableError,
    RuntimePhase,
    consume_retry_token,
    read_retry_token,
    read_runtime_health,
    request_retry_token,
    write_runtime_health,
)
from solstone.think.readiness import START_TIME_TOLERANCE_S, clear_ready, signal_ready
from solstone.think.runner import DEFAULT_TASK_MAX_RUNTIME, _command_partition
from solstone.think.runner import ManagedProcess as RunnerManagedProcess
from solstone.think.sync_check import (
    DEFAULT_INTERVAL_SECONDS,
    SyncCheckSnapshot,
    check_journal_sync,
    clear_self_heartbeat,
    format_conflict_message,
    write_self_heartbeat,
)
from solstone.think.utils import (
    EXIT_EMPTY,
    EXIT_TEMPFAIL,
    SOFT_RUNTIME_FRACTION,
    day_path,
    find_available_port,
    get_journal,
    get_journal_info,
    get_rev,
    is_solstone_up,
    now_ms,
    parse_duration_seconds,
    read_service_port,
    setup_cli,
    updated_days,
    write_service_port,
)

REACTIVE_TASK_CAPS = {
    "daily": 21600,  # 6h daily/from-scratch reprocess
    "segment": 4500,  # 75m per-segment think
    "indexer": 7200,  # 2h indexer, above the code's own 1h rescan allotment
    "importer": 3600,  # 1h importer
}
GATE_TICK_INTERVAL_S = 60
CATCHUP_RETRY_TICK_INTERVAL_S = 60
MAX_UPDATED_CATCHUP = 4
TEMPFAIL_DELAY = 15  # seconds to wait before retrying a tempfail exit
CONVEY_READY_WINDOW_SECONDS = 60.0
PROVIDER_STARTUP_GATE_WINDOW_SECONDS = CONVEY_READY_WINDOW_SECONDS
PROVIDER_STARTUP_GATE_CEILING_SECONDS = 330.0
REALISTIC_COLD_BIND_SECONDS = 40.0
HANDLE_SHUTDOWN_REAP_S = 3.0
APP_SUPERVISED_SHUTDOWN_CEILING_S = 10.0
APP_SUPERVISED_TASK_DRAIN_S = 2.0
APP_SUPERVISED_CHILD_STOP_S = 2.0
APP_SUPERVISED_CALLOSUM_JOIN_S = 2.0
PARENT_DEATH_POLL_INTERVAL_S = 1.0
STOPPED_TICKS_THRESHOLD = 2
LOCAL_SERVER_PROCESS_NAME = "llama-server"
LOCAL_WEDGE_THRESHOLD = 3
LOCAL_WEDGE_RECYCLE_GRACE_S = 120.0
LOCAL_WEDGE_PROVIDER_MAP_CAP = 512
LOCAL_SERVER_READY_TIMEOUT_S = 300.0
LOCAL_SERVER_HEALTH_POLL_INTERVAL_S = 1.0
PARAKEET_SERVER_PROCESS_NAME = "parakeet-server"
PARAKEET_SERVER_READY_TIMEOUT_S = 300.0
PARAKEET_SERVER_HEALTH_POLL_INTERVAL_S = 1.0
LOCAL_MODEL_WARMING_UP_COPY = "Local model is warming up..."
PROVIDER_RETRY_SCHEDULE_SECONDS = (0.0, 2.0, 4.0, 8.0, 16.0, 30.0)
PROVIDER_CLEANUP_RETRY_SCHEDULE_SECONDS = (2.0, 4.0, 8.0, 16.0, 30.0)
PROVIDER_ADMISSION_STOP_TIMEOUT_S = 5.0
PROVIDER_STABLE_READY_REFRESH_SECONDS = 60.0
PROVIDER_TRUTH_OBSERVATION_INTERVAL_SECONDS = GATE_TICK_INTERVAL_S
PROVIDER_PROBE_INTERVAL_SECONDS = GATE_TICK_INTERVAL_S
# supervisor.log is size-rotated with a bounded on-disk footprint.
# Hard ceiling = SUPERVISOR_LOG_MAX_BYTES * (SUPERVISOR_LOG_BACKUP_COUNT + 1)
#   = 16 MiB * 6 = 96 MiB. Older lines drop; the most-recent tail is kept.
SUPERVISOR_LOG_MAX_BYTES = 16 * 1024 * 1024
SUPERVISOR_LOG_BACKUP_COUNT = 5
logger = logging.getLogger(__name__)


def linux_stt_uses_parakeet_cpp() -> bool:
    """Return whether this host's effective STT path needs parakeet-server."""
    if not sys.platform.startswith("linux"):
        return False
    try:
        from solstone.think import parakeet_readiness

        parakeet_readiness.parakeet_cpp_artifact_key(
            "linux", platform.machine().lower()
        )
    except RuntimeError:
        return False

    config = read_journal_config()
    transcribe = config.get("transcribe", {})
    backend = transcribe.get("backend") if isinstance(transcribe, dict) else None
    from solstone.think.services import spp

    # Routing uses channel usability; dispatch refusal still keys on bare
    # confidential block presence to keep raw audio from accidental egress.
    confidential_channel_usable = spp.is_confidential_channel_usable(config)

    selected = resolve_stt_backend_choice(
        backend if isinstance(backend, str) else None,
        read_available_bytes(),
        floor_bytes=stt_local_floor_bytes(),
        local_backend=local_stt_backend(),
        confidential_lane_active=confidential_channel_usable,
        confidential_audio_enabled=confidential_audio_enabled(transcribe),
    )
    return selected in {"parakeet", "parakeet-cpp"}


def _configured_parakeet_device() -> str:
    config = read_journal_config()
    transcribe = config.get("transcribe", {})
    nested = transcribe.get("parakeet-cpp", {}) if isinstance(transcribe, dict) else {}
    device = nested.get("device") if isinstance(nested, dict) else None
    return device if device in {"auto", "cpu"} else "auto"


def _compact_log_if_oversized(log_path: Path, max_bytes: int) -> None:
    try:
        size = log_path.stat().st_size
    except FileNotFoundError:
        return
    except OSError as error:
        logger.warning("Could not stat supervisor log before compaction: %s", error)
        return

    if size <= max_bytes:
        return

    compact_path = log_path.with_name(log_path.name + ".compact")
    try:
        with log_path.open("rb") as handle:
            handle.seek(-max_bytes, os.SEEK_END)
            tail = handle.read(max_bytes)

        first_newline = tail.find(b"\n")
        kept = tail[first_newline + 1 :] if first_newline != -1 else b""

        with compact_path.open("wb") as handle:
            handle.write(kept)
        compact_path.rename(log_path)
    except OSError as error:
        logger.warning("Could not compact oversized supervisor log: %s", error)
        try:
            compact_path.unlink(missing_ok=True)
        except OSError:
            pass


def _configure_supervisor_logging(
    log_path: Path,
    level: int,
    max_bytes: int = SUPERVISOR_LOG_MAX_BYTES,
    backup_count: int = SUPERVISOR_LOG_BACKUP_COUNT,
) -> None:
    logging.getLogger().handlers = []
    _compact_log_if_oversized(log_path, max_bytes)
    logging.basicConfig(
        level=level,
        handlers=[
            RotatingFileHandler(
                log_path,
                maxBytes=max_bytes,
                backupCount=backup_count,
                encoding="utf-8",
            )
        ],
        format="%(asctime)s [supervisor:log] %(levelname)s %(message)s",
        datefmt="%Y-%m-%dT%H:%M:%S",
    )


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
_last_gate_tick: float = 0.0
_last_catchup_retry_tick: float = 0.0
# Wall-clock of the last catchup-retry evaluation; 0.0 means unseeded.
_catchup_retry_watermark: float = 0.0
_last_sync_snapshot: "SyncCheckSnapshot | None" = None
_sync_conflict_shutdown: bool = False
# Supervisor identity (set in main() once ref is assigned)
_supervisor_ref: str | None = None
_supervisor_start: float | None = None
_parent_death_sigterm_sent = threading.Event()


def app_supervised_graceful_budget_s() -> float:
    """Sum of configured app-supervised graceful shutdown step budgets.

    After the parent-death watcher self-SIGTERMs, the graceful path runs these
    configured caps in finally-block order: handle_shutdown's managed-child
    reap, task-queue drain, managed child-stop, and Callosum join.

    The assertion that this budget stays below APP_SUPERVISED_SHUTDOWN_CEILING_S
    guards that configured step budgets leave room under the hard parent-death
    backstop. It is not a guarantee that wall time can never exceed this sum.
    In the common non-D-state case, bounded terminate calls (timeout plus
    KILL_REAP_GRACE_S) keep the graceful path well under the ceiling. For
    pathological slow-to-reap children, a step may exceed its nominal cap; the
    parent-death backstop remains the hard guarantee by sleeping to the ceiling,
    SIGKILLing every child's process group, then calling os._exit(1).
    """
    return (
        HANDLE_SHUTDOWN_REAP_S
        + APP_SUPERVISED_TASK_DRAIN_S
        + APP_SUPERVISED_CHILD_STOP_S
        + APP_SUPERVISED_CALLOSUM_JOIN_S
    )


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


def _parent_fd_is_usable(fd: int) -> bool:
    try:
        mode = os.fstat(fd).st_mode
        flags = fcntl.fcntl(fd, fcntl.F_GETFL)
    except OSError:
        return False
    access_mode = flags & os.O_ACCMODE
    return stat.S_ISFIFO(mode) and access_mode in (os.O_RDONLY, os.O_RDWR)


def wait_until_parent_gone(
    parent_fd: int, *, poll_interval: float = PARENT_DEATH_POLL_INTERVAL_S
) -> str:
    if _parent_fd_is_usable(parent_fd):
        while True:
            try:
                data = os.read(parent_fd, 4096)
            except OSError:
                return "fd-error"
            if data == b"":
                return "eof"

    while True:
        if os.getppid() == 1:
            return "orphaned"
        time.sleep(poll_interval)


def enforce_parent_death_shutdown_deadline(
    reason: str,
    *,
    ceiling: float = APP_SUPERVISED_SHUTDOWN_CEILING_S,
    managed_procs: Iterable[RunnerManagedProcess] | None = None,
    task_procs: Iterable[RunnerManagedProcess] | None = None,
    sent_event: "threading.Event | None" = None,
    kill: Callable[[int, int], None] | None = None,
    killpg: Callable[[int, int], None] | None = None,
    getpgid: Callable[[int], int] | None = None,
    exit_now: Callable[[int], NoReturn] | None = None,
    sleep: Callable[[float], None] | None = None,
) -> None:
    del reason
    own_pid = os.getpid()
    own_pgid = os.getpgrp()
    kill_fn = kill or os.kill
    killpg_fn = killpg or os.killpg
    getpgid_fn = getpgid or os.getpgid
    exit_now_fn = exit_now or os._exit
    sleep_fn = sleep or time.sleep
    sigterm_sent = sent_event if sent_event is not None else _parent_death_sigterm_sent

    if not sigterm_sent.is_set():
        sigterm_sent.set()
        kill_fn(own_pid, signal.SIGTERM)

    sleep_fn(ceiling)

    procs = managed_procs if managed_procs is not None else _managed_procs
    if task_procs is not None:
        task_snapshot = task_procs
    elif _task_queue is not None:
        with _task_queue._lock:
            task_snapshot = list(_task_queue._active.values())
    else:
        task_snapshot = []

    def _kill_group(managed: RunnerManagedProcess) -> None:
        if not managed.is_running():
            return
        try:
            pgid = getpgid_fn(managed.process.pid)
        except (ProcessLookupError, OSError):
            logger.exception(
                "parent-death backstop: could not resolve pgid for %s",
                managed.name,
            )
            return

        if pgid == own_pgid or pgid == own_pid:
            logger.warning(
                "parent-death backstop: refusing to signal supervisor's own "
                "group (pgid=%s) for %s",
                pgid,
                managed.name,
            )
            return

        try:
            killpg_fn(pgid, signal.SIGKILL)
        except Exception:
            logger.exception(
                "parent-death backstop: SIGKILL failed for %s", managed.name
            )

    for managed in procs:
        try:
            _kill_group(managed)
        except Exception:
            logger.exception(
                "parent-death backstop: unexpected failure for %s",
                getattr(managed, "name", managed),
            )
    for managed in task_snapshot:
        try:
            _kill_group(managed)
        except Exception:
            logger.exception(
                "parent-death backstop: unexpected failure for %s",
                getattr(managed, "name", managed),
            )

    exit_now_fn(1)


def _parent_death_watcher_main(
    parent_fd: int,
    *,
    poll_interval: float = PARENT_DEATH_POLL_INTERVAL_S,
    ceiling: float = APP_SUPERVISED_SHUTDOWN_CEILING_S,
) -> None:
    reason = wait_until_parent_gone(parent_fd, poll_interval=poll_interval)
    logger.warning(
        "parent-death detected (%s); converging to graceful shutdown", reason
    )
    enforce_parent_death_shutdown_deadline(reason, ceiling=ceiling)


def start_parent_death_watcher(
    parent_fd: int | None = None,
    *,
    poll_interval: float = PARENT_DEATH_POLL_INTERVAL_S,
    ceiling: float = APP_SUPERVISED_SHUTDOWN_CEILING_S,
) -> threading.Thread:
    fd = parent_fd if parent_fd is not None else resolve_parent_fd()
    thread = threading.Thread(
        target=_parent_death_watcher_main,
        args=(fd,),
        kwargs={"poll_interval": poll_interval, "ceiling": ceiling},
        name="parent-death-watcher",
        daemon=True,
    )
    thread.start()
    return thread


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


# The long-lived journal proctitles set by setproctitle at sol_cli.py
# (f"{binary}:{cmd}"). setproctitle is in-process and persists until the
# process exits, so an orphaned service or task child still reports its title
# via proc.name() after the supervisor dies, which is what lets the sweep find
# it. Supervisor-owned provider servers report their own bare binary names
# (no colon prefix) and are included here so the sweep reaps them too.
# The mlx-vlm server is a Python process, but our launcher sets the same
# managed proctitle so proc.name() is stable for orphan sweeping.
_SWEEPABLE_PROVIDER_PROCTITLES = frozenset(
    {
        LOCAL_SERVER_PROCESS_NAME,
        PARAKEET_SERVER_PROCESS_NAME,
        MLX_SERVER_PROCESS_NAME,
    }
)


def _is_sweepable_orphan_name(name: str) -> bool:
    """True if proc.name() identifies a sweepable orphan of this install.

    Any `journal:*` proctitle - managed service or task-queue child - plus the
    bare provider-server binary names. A PPID-1, same-journal `journal:*` process
    is by definition an orphan of a dead supervisor. `solstone:*`/`sol:*` and a
    bare `journal` (no colon) are deliberately not matched because they cannot
    be positively classified as a sub-command of this install.
    """
    return name.startswith("journal:") or name in _SWEEPABLE_PROVIDER_PROCTITLES


def _sweep_orphaned_sol_processes(journal: Path, grace: float = 5.0) -> int:
    journal = journal.resolve()
    current_user = getpass.getuser()
    own_pid = os.getpid()
    targets: list[int] = []
    for proc in psutil.process_iter(["name", "ppid", "username", "pid"]):
        try:
            if not _is_sweepable_orphan_name(proc.name()):
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
        self._default_cap = DEFAULT_TASK_MAX_RUNTIME
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

    def _effective_cap(self, cmd_name: str) -> int:
        """Resolve the wall-clock cap for a partition: explicit override or default.

        Lock-free: reads only `self._caps` (atomic dict get) and the immutable
        `self._default_cap`, so it is safe whether or not the caller holds
        `self._lock`.
        """
        return self._caps.get(cmd_name) or self._default_cap

    def get_active_by_cmd_name(self, name: str) -> str | None:
        """Return the first active ref matching a command name."""
        with self._lock:
            for ref, managed in self._active.items():
                if self.get_command_name(managed.cmd) == name:
                    return ref
        return None

    def enforce_deadlines(self, now: float) -> None:
        """Enforce configured task runtime caps without blocking the supervisor tick.

        Phase A snapshots active tasks under the lock. Phase B does all psutil
        probing and outcome decisions with NO lock held. Phase C applies the state
        mutations under the lock. Phase D starts termination threads with NO lock
        held. Threads start only after Phase C so a cap/stopped-terminated ref is
        recorded in _cap_terminated before its process can exit (preserving the
        "timeout" exit-status labeling in _run_task's completion handler).
        """
        # Phase A: snapshot under lock.
        with self._lock:
            snapshot = [
                (ref, managed, self._effective_cap(self.get_command_name(managed.cmd)))
                for ref, managed in self._active.items()
            ]
            already_cap_terminated = set(self._cap_terminated)
            stopped_ticks = dict(self._stopped_ticks)

        # Phase B: probe + decide, no lock, no mutation, no thread starts.
        newly_cap_terminated: set[str] = set()
        to_terminate: list[tuple[str, RunnerManagedProcess, str]] = []
        stopped_updates: dict[str, int | None] = {}  # ref -> new count, or None to pop

        # Cap loop first (populates newly_cap_terminated for the stopped skip set).
        for ref, managed, cap in snapshot:
            elapsed = now - managed.start_time
            if elapsed <= cap:
                continue

            cmd_name = self.get_command_name(managed.cmd)
            logging.warning(
                "Task %s (cmd=%s, ref=%s) exceeded max_runtime of %ds "
                "(elapsed=%ds); terminating",
                cmd_name,
                " ".join(managed.cmd),
                ref,
                cap,
                int(elapsed),
            )
            newly_cap_terminated.add(ref)
            to_terminate.append((ref, managed, "cap"))

        # Stopped loop, skipping anything cap-terminated (prior ticks or this tick).
        skip = already_cap_terminated | newly_cap_terminated
        for ref, managed, _cap in snapshot:
            if ref in skip:
                continue
            try:
                state = psutil.Process(managed.process.pid).status()
            except (psutil.NoSuchProcess, psutil.AccessDenied):
                stopped_updates[ref] = None
                continue

            if state in (psutil.STATUS_STOPPED, psutil.STATUS_TRACING_STOP):
                ticks = stopped_ticks.get(ref, 0) + 1
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
                    newly_cap_terminated.add(ref)
                    to_terminate.append((ref, managed, "stopped"))
                    stopped_updates[ref] = None
                else:
                    stopped_updates[ref] = ticks
            else:
                stopped_updates[ref] = None

        # Phase C: apply state mutations under lock. Guard additions/sets with
        # `ref in self._active` so a task that completed during the unlocked probe
        # window (already popped its own _cap_terminated/_stopped_ticks entries in
        # _run_task) is not resurrected. Pops are unconditional.
        with self._lock:
            for ref in newly_cap_terminated:
                if ref in self._active:
                    self._cap_terminated.add(ref)
            for ref, val in stopped_updates.items():
                if val is None:
                    self._stopped_ticks.pop(ref, None)
                elif ref in self._active:
                    self._stopped_ticks[ref] = val

        # Phase D: start termination threads, no lock held.
        for ref, managed, reason in to_terminate:
            _start_termination_thread(ref, managed, timeout=2.0, reason=reason)

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
        attempt_recorded = False

        try:
            callosum.start()
            logging.info(f"Starting task {primary_ref}: {' '.join(cmd)}")

            managed = RunnerManagedProcess.spawn(
                cmd, ref=primary_ref, callosum=callosum, day=day
            )
            with self._lock:
                self._active[primary_ref] = managed
            started_at = time.time()
            record_attempt(cmd, day, primary_ref, started_at=started_at)
            attempt_recorded = True

            callosum.emit(
                "supervisor",
                "started",
                service=service,
                pid=managed.pid,
                ref=primary_ref,
            )

            exit_code = managed.wait()
            exit_status = _exit_status_for_code(exit_code)

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
            if attempt_recorded:
                try:
                    outcome_result = record_outcome(
                        cmd,
                        day,
                        primary_ref,
                        exit_status=exit_status,
                        ended_at=ended_at,
                    )
                    if outcome_result.entered_backoff:
                        _emit_catchup_backoff(
                            callosum,
                            day=outcome_result.day,
                            attempts=outcome_result.attempts,
                            consecutive=outcome_result.consecutive_non_completion,
                            last_outcome=outcome_result.last_outcome,
                        )
                    if (
                        outcome_result.recorded
                        and outcome_result.command_kind == KIND_DAILY_CATCHUP
                        and not outcome_result.completed
                    ):
                        _nudge_catchup_drain(exclude_today=True)
                except Exception:
                    logging.warning("catchup outcome writeback failed", exc_info=True)
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

    def collect_task_status(self) -> list[dict]:
        """Collect status of all running tasks for supervisor status."""
        now = time.time()
        with self._lock:
            snapshot = list(self._active.items())
        tasks = []
        for ref, managed in snapshot:
            if managed.is_running():
                duration = int(now - managed.start_time)
                cmd_name = TaskQueue.get_command_name(managed.cmd)
                cap = self._effective_cap(cmd_name)
                tasks.append(
                    {
                        "ref": ref,
                        "name": cmd_name,
                        "duration_seconds": duration,
                        "max_runtime_seconds": cap,
                        "slow": duration >= cap * SOFT_RUNTIME_FRACTION,
                        "stuck": duration > cap,
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

# State for daily processing (tracks day boundary for midnight think trigger)
_daily_state = {
    "last_day": None,  # Track which day we last processed
}

# State for local provider wedge detection
_wedge_state: dict[str, Any] = {
    "providers": OrderedDict(),
    "failures": set(),
    "cooldown_until": 0.0,
    "awaiting_recovery": False,
}

ProviderName = Literal["local", "parakeet"]
LaunchOutcomeStatus = Literal[
    "ready",
    "not-ready",
    "host-blocked",
    "exited",
    "warmup-timeout",
    "launch-failed",
]
StopCleanupStatus = Literal["stopped", "stop-deferred", "cleanup-failed", "cancelled"]
ProbeStatus = Literal["ready", "not-ready", "unavailable"]
LocalPlanBackend = Literal["mlx", "cuda", "vulkan"]


@dataclass(frozen=True)
class ProviderLaunchOutcome:
    status: LaunchOutcomeStatus
    reason_code: ReasonCode
    detail: dict[str, Any]
    managed: RunnerManagedProcess | None = None


@dataclass(frozen=True)
class ProviderStopCleanupRequest:
    managed: RunnerManagedProcess
    reason_code: ReasonCode
    detail: dict[str, Any]
    target_phase: RuntimePhase
    target_reason_code: ReasonCode | None
    target_detail: dict[str, Any]
    admission_exclusive: bool = False
    local_capacity: int | None = None


@dataclass(frozen=True)
class ProviderStopCleanupOutcome:
    status: StopCleanupStatus
    reason_code: ReasonCode
    detail: dict[str, Any]
    managed: RunnerManagedProcess | None = None


@dataclass(frozen=True)
class ProviderProbeOutcome:
    status: ProbeStatus
    reason_code: ReasonCode
    detail: dict[str, Any]


@dataclass(frozen=True)
class LocalServerLaunchPlan:
    backend: LocalPlanBackend
    desired_fingerprint_json: str
    desired_fingerprint_sha256: str
    binary_path: Path | None = None
    model_path: Path | None = None
    mmproj_path: Path | None = None
    lib_dir: Path | None = None
    model_id: str = LOCAL_MODEL
    runtime_dir: Path | None = None
    gpu_index: int | None = None
    gpu_name: str | None = None
    gpu_vram_mib: int | None = None
    vram_before_mib: int | None = None
    context_tokens: int | None = None
    parallel_slots: int | None = None
    prompt_cache_mib: int | None = None
    visible_devices_env: str | None = None
    visible_devices_value: str | None = None
    env_updates: dict[str, str] = field(default_factory=dict)
    backend_reason: str | None = None


@dataclass(frozen=True)
class ParakeetServerLaunchPlan:
    binary_backend: str
    env_updates: dict[str, str]
    gpu_index: int | None
    binary_path: Path
    model_path: Path
    threads: int
    desired_fingerprint_json: str
    desired_fingerprint_sha256: str
    placement: Literal["cpu", "gpu"]


@dataclass(frozen=True)
class ProviderTruthObservation:
    provider: ProviderName
    phase: RuntimePhase
    reason_code: ReasonCode | None
    detail: dict[str, Any]
    desired_fingerprint_json: str | None = None
    desired_fingerprint_sha256: str | None = None
    plan: LocalServerLaunchPlan | ParakeetServerLaunchPlan | None = None
    boot_required: bool = False


@dataclass(frozen=True)
class ProviderFence:
    incarnation: str
    generation: int
    fingerprint: str | None
    attempt: int


@dataclass
class ProviderRetryState:
    attempt_count: int = 0
    next_at: float = 0.0
    desired_fingerprint: str | None = None


@dataclass
class ProviderRecoveryState:
    down_generation: int | None = None
    nudged_generation: int | None = None


@dataclass
class ProviderRuntimeState:
    provider: ProviderName
    truth_future: concurrent.futures.Future | None = None
    truth_fence: ProviderFence | None = None
    start_future: concurrent.futures.Future | None = None
    start_fence: ProviderFence | None = None
    start_cancel_event: threading.Event | None = None
    stop_cleanup_future: concurrent.futures.Future | None = None
    stop_cleanup_fence: ProviderFence | None = None
    stop_cleanup_cancel_event: threading.Event | None = None
    pending_stop_request: ProviderStopCleanupRequest | None = None
    pending_stop_target_phase: RuntimePhase = "stopped"
    pending_stop_target_reason_code: ReasonCode | None = "cleanup-succeeded"
    pending_stop_target_detail: dict[str, Any] = field(default_factory=dict)
    pending_stop_admission_exclusive: bool = False
    cleanup_attempt_count: int = 0
    cleanup_next_at: float = 0.0
    probe_future: concurrent.futures.Future | None = None
    probe_fence: ProviderFence | None = None
    retry: ProviderRetryState = field(default_factory=ProviderRetryState)
    generation: int = 0
    desired_fingerprint: str | None = None
    latest_plan: LocalServerLaunchPlan | ParakeetServerLaunchPlan | None = None
    latest_phase: RuntimePhase = "stopped"
    replacement_artifact_not_ready_fingerprint: str | None = None
    boot_required: bool = False
    startup_terminal: bool = False
    next_truth_at: float = 0.0
    next_probe_at: float = 0.0
    parakeet_bootstrap_requested_fingerprint: str | None = None
    parakeet_bootstrap_future: concurrent.futures.Future | None = None
    parakeet_bootstrap_fingerprint: str | None = None


@dataclass
class ProviderStartupGate:
    started_at: float
    required: set[ProviderName]
    terminal: set[ProviderName]
    attempted: dict[ProviderName, LaunchOutcomeStatus]
    first_start_at: float | None
    released: bool


# State for provider recovery nudges. Only local consumes this to nudge catchup.
_recovery_state: dict[ProviderName, ProviderRecoveryState] = {
    "local": ProviderRecoveryState(),
    "parakeet": ProviderRecoveryState(),
}


class ReservedPort:
    """Supervisor-local port reservation held until immediate pre-spawn."""

    def __init__(self, sock: socket.socket) -> None:
        self._sock: socket.socket | None = sock
        self.port = int(sock.getsockname()[1])

    @classmethod
    def reserve(cls) -> "ReservedPort":
        sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        try:
            sock.bind(("127.0.0.1", 0))
            return cls(sock)
        except BaseException:
            sock.close()
            raise

    def release_for_spawn(self) -> int:
        port = self.port
        self.close()
        return port

    def close(self) -> None:
        sock = self._sock
        self._sock = None
        if sock is not None:
            sock.close()


_PROVIDER_INCARNATION = uuid.uuid4().hex
_provider_runtime_states: dict[ProviderName, ProviderRuntimeState] = {
    "local": ProviderRuntimeState("local"),
    "parakeet": ProviderRuntimeState("parakeet"),
}
_provider_runtime_executor: concurrent.futures.ThreadPoolExecutor | None = None
_provider_startup_gate: ProviderStartupGate | None = None
_parakeet_admission_retry_epoch = 0

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

    return abs(recorded_start - create_time) <= START_TIME_TOLERANCE_S


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


def describe_exit(returncode: int) -> str:
    """Render a process return code, decoding signals for negative codes."""
    if returncode >= 0:
        return f"exit {returncode}"
    try:
        name = signal.Signals(-returncode).name
    except ValueError:
        return f"exit {returncode} / signal {-returncode}"
    return f"exit {returncode} / {name}"


def _get_restart_policy(name: str) -> RestartPolicy:
    return _RESTART_POLICIES.setdefault(name, RestartPolicy())


def _launch_process(
    name: str,
    cmd: list[str],
    *,
    restart: bool = False,
    shutdown_timeout: int = 15,
    ref: str | None = None,
    env: dict[str, str] | None = None,
) -> RunnerManagedProcess:
    # NOTE: All child processes should include -v for verbose logging by default.
    # This ensures their output is captured in logs for debugging.
    """Launch process with automatic output logging and restart policy tracking."""
    policy = _get_restart_policy(name)

    # Generate ref if not provided
    ref = ref if ref else str(now_ms())

    # Use unified runner to spawn process (share supervisor's callosum)
    try:
        managed = RunnerManagedProcess.spawn(
            cmd, ref=ref, callosum=_supervisor_callosum, env=env
        )
    except RuntimeError as exc:
        logging.error(str(exc))
        raise

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


def _stop_process(
    managed: RunnerManagedProcess, *, timeout_cap: float | None = None
) -> None:
    timeout = _SERVICE_STATE.get(managed.name, {}).get("shutdown_timeout", 15)
    if timeout_cap is not None:
        timeout = min(timeout, timeout_cap)
    _terminate_managed(managed, timeout, reason="shutdown")
    managed.cleanup()


def _exit_status_for_code(exit_code: int) -> str:
    """Map a scheduled task's process exit code to a scheduler status label.

    0 -> "ok"; EXIT_EMPTY -> "empty" (a rollup ran over zero inputs - a distinct,
    non-error "nothing to do" outcome); any other non-zero code -> "error".
    Timeouts are mapped separately by the caller.
    """
    if exit_code == 0:
        return "ok"
    if exit_code == EXIT_EMPTY:
        return "empty"
    return "error"


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


def _emit_catchup_backoff(
    callosum,
    *,
    day: str | None,
    attempts: int,
    consecutive: int,
    last_outcome: str,
) -> None:
    if callosum is None:
        return
    message = f"day {day} stuck after {attempts} attempts, last outcome {last_outcome}"
    try:
        callosum.emit(
            "storage",
            "warning",
            level="warning",
            type="catchup_backoff",
            message=message,
            current=consecutive,
            threshold=STUCK_THRESHOLD,
        )
        callosum.emit(
            "notification",
            "show",
            title="Catchup stuck",
            message=message,
            action="/app/health",
        )
    except Exception:
        logging.warning(
            "Failed to emit catchup backoff notification for %s", day, exc_info=True
        )


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
                active_cmd = list(managed.cmd) if managed else None
                cap = _task_queue._effective_cap(cmd_name)
            if (
                message.get("queue_if_active_cmd_differs")
                and active_cmd is not None
                and active_cmd != cmd
            ):
                _task_queue.submit(cmd, ref, day=day, scheduler_name=scheduler_name)
                return
            runtime = time.time() - managed.start_time if managed else 0
            reason = "wedged" if runtime > 2 * cap else "still_running"
            logging.warning(
                "Refusing supervisor task request: cmd_name=%s ref=%s active_ref=%s "
                "reason=%s scheduler_name=%s cmd=%s",
                cmd_name,
                ref,
                active_ref,
                reason,
                scheduler_name,
                cmd,
            )
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
    else:
        logging.warning(
            "Refusing supervisor task request: task_queue_unavailable ref=%s "
            "scheduler_name=%s cmd=%s",
            ref,
            scheduler_name,
            cmd,
        )


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


def _handle_supervisor_drain(message: dict) -> None:
    """Handle incoming supervisor catchup drain requests."""
    if message.get("tract") != "supervisor" or message.get("event") != "drain":
        return
    if _is_remote_mode:
        return

    day = message.get("day")
    if day:
        run_catchup_drain(force_days={day})
    elif message.get("exclude_today"):
        today_str = datetime.now().date().strftime("%Y%m%d")
        run_catchup_drain(exclude={today_str})
    else:
        run_catchup_drain()


def _handle_cortex_outcome(message: dict) -> None:
    """Recycle a wedged local model server after sustained generation failures."""
    if message.get("tract") != "cortex":
        return
    event = message.get("event")
    if event not in {"start", "finish", "error"}:
        return
    if _is_remote_mode:
        return

    use_id = message.get("use_id")
    if not use_id:
        return

    if event == "start":
        providers = _wedge_state["providers"]
        providers[use_id] = message.get("provider")
        while len(providers) > LOCAL_WEDGE_PROVIDER_MAP_CAP:
            providers.popitem(last=False)
        return

    provider = _wedge_state["providers"].get(use_id)
    if provider != "local":
        return
    from solstone.think.providers.local_endpoint import resolve_local_endpoint

    if not resolve_local_endpoint().is_bundled:
        return

    if time.monotonic() < _wedge_state["cooldown_until"]:
        return

    failures = _wedge_state["failures"]
    if event == "finish":
        if _wedge_state["awaiting_recovery"]:
            logging.info("local server wedge: recovered after recycle")
            _wedge_state["awaiting_recovery"] = False
        failures.clear()
        return

    if message.get("reason_code") != "provider_unavailable":
        return

    failures.add(use_id)
    if len(failures) < LOCAL_WEDGE_THRESHOLD:
        return

    logging.warning(
        "local server wedge: declared after %d local provider_unavailable failures "
        "(use_ids=%s)",
        len(failures),
        sorted(failures),
    )
    port = read_service_port("local")
    if port is None:
        logging.warning(
            "local server wedge: recycle deferred; local service port unavailable"
        )
        failures.clear()
        return

    from solstone.think.providers import local_server

    state, _ = local_server._probe_health(port)
    if state != local_server.STATE_READY:
        logging.warning(
            "local server wedge: recycle deferred; health state=%s",
            state,
        )
        failures.clear()
        return

    if _request_provider_runtime_recycle(
        "local",
        reason_code="local-wedge-provider-unavailable",
        detail={
            "use_ids": sorted(failures),
            "port": port,
            "health_state": state,
        },
    ):
        logging.warning("local server wedge: requested local provider recycle")
        failures.clear()
        _wedge_state["awaiting_recovery"] = True
        _wedge_state["cooldown_until"] = time.monotonic() + LOCAL_WEDGE_RECYCLE_GRACE_S
    else:
        logging.warning("local server wedge: recycle request failed")
        failures.clear()


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


def _required_plan_path(path: Path | None, field_name: str) -> Path:
    if path is None:
        raise ValueError(f"launch plan is missing {field_name}")
    return path


def _required_plan_int(value: int | None, field_name: str) -> int:
    if value is None:
        raise ValueError(f"launch plan is missing {field_name}")
    return value


def _required_launch_plan_int(launch_plan: dict[str, Any], field_name: str) -> int:
    value = launch_plan.get(field_name)
    if not isinstance(value, int):
        raise ValueError(f"native launch plan is missing {field_name}")
    return value


def _request_local_launch_plan(
    plan: LocalServerLaunchPlan,
    port: int,
    *,
    mlx_interpreter_path: Path | None = None,
    handshake_checker=core_handshake.check_solstone_core_handshake,
    helper_locator=core_handshake.helper_path_for_executable,
    runner=subprocess.run,
) -> dict[str, Any]:
    """Render a supervisor-observed local plan through the native boundary."""
    handshake = handshake_checker()
    if handshake.status != "ok":
        raise RuntimeError(
            "local plan requires a usable solstone-core helper: "
            f"{handshake.message or 'unknown handshake failure'}"
        )

    is_cuda = plan.backend == "cuda"
    is_vulkan = plan.backend == "vulkan"
    if plan.backend == "mlx":
        if mlx_interpreter_path is None:
            raise ValueError("MLX launch requires an interpreter path")
        model_path = ""
    else:
        model_path = str(_required_plan_path(plan.model_path, "model_path"))

    nvidia_probe = None
    if is_cuda:
        nvidia_probe = {
            "schema": "solstone-local-nvidia-probe-v1",
            "detected": True,
            "gpu_index": plan.gpu_index,
            "gpu_name": plan.gpu_name,
            "compute_cap": None,
            "arch": None,
            "driver_cuda_major": None,
            "vram_mib": plan.gpu_vram_mib,
            "unified_memory_mib": None,
            "probe_error": None,
        }
    vulkan_devices = None
    if is_vulkan:
        gpu_index = _required_plan_int(plan.gpu_index, "gpu_index")
        gpu_name = plan.gpu_name
        gpu_vram_mib = plan.gpu_vram_mib
        if gpu_name is None or gpu_vram_mib is None:
            raise ValueError("launch plan is missing Vulkan GPU details")
        vulkan_devices = [
            {"index": gpu_index, "name": gpu_name, "vram_mib": gpu_vram_mib}
        ]

    payload = {
        "schema": "solstone-local-plan-input-v1",
        "platform": "darwin" if sys.platform == "darwin" else "linux",
        "backend_override": plan.backend,
        "bind_address": "127.0.0.1",
        "port": port,
        "desired_fingerprint_json": json.loads(plan.desired_fingerprint_json),
        "desired_fingerprint_sha256": plan.desired_fingerprint_sha256,
        "model_id": plan.model_id,
        "model_path": model_path,
        "mmproj_path": str(plan.mmproj_path) if plan.mmproj_path else None,
        "runtime_dir": str(plan.runtime_dir) if plan.runtime_dir else None,
        "mlx_interpreter_path": (
            str(mlx_interpreter_path) if mlx_interpreter_path is not None else None
        ),
        "cuda_binary_path": str(plan.binary_path) if is_cuda and plan.binary_path else None,
        "vulkan_binary_path": (
            str(plan.binary_path) if is_vulkan and plan.binary_path else None
        ),
        "lib_dir": str(plan.lib_dir) if plan.lib_dir else None,
        "inherited_ld_library_path": os.environ.get("LD_LIBRARY_PATH", ""),
        "nvidia_probe": nvidia_probe,
        "cuda_embedded_arch_set": [],
        "cuda_min_driver_version": None,
        "cuda_artifact_trust": None,
        "cuda_persisted_installed_cuda_target": None,
        "vulkan_devices": vulkan_devices,
        "vulkan_selected_gpu_index": plan.gpu_index if is_vulkan else None,
        "vulkan_selected_gpu_name": plan.gpu_name if is_vulkan else None,
        "vulkan_selected_vram_mib": plan.gpu_vram_mib if is_vulkan else None,
        "vram_before_mib": plan.vram_before_mib if is_vulkan else None,
    }
    try:
        completed = runner(
            [str(helper_locator()), "local", "plan"],
            input=json.dumps(payload),
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError as exc:
        raise RuntimeError(f"solstone-core local plan failed to launch: {exc}") from exc
    if completed.returncode != 0:
        raise RuntimeError(f"solstone-core local plan failed: {completed.stderr}")
    outcome = json.loads(completed.stdout)
    if outcome.get("outcome") == "rejected":
        raise RuntimeError(f"solstone-core local plan rejected: {outcome.get('reason')}")
    if outcome.get("outcome") != "launch":
        raise RuntimeError("solstone-core local plan returned an invalid outcome")
    return outcome


def _outcome(
    status: LaunchOutcomeStatus,
    reason_code: ReasonCode,
    detail: dict[str, Any],
    managed: RunnerManagedProcess | None = None,
) -> ProviderLaunchOutcome:
    return ProviderLaunchOutcome(
        status=status,
        reason_code=reason_code,
        detail=detail,
        managed=managed,
    )


def _terminate_cleanup_handle(
    managed: RunnerManagedProcess, *, reason: str, state_name: str | None = None
) -> None:
    service_name = state_name or managed.name
    timeout = _SERVICE_STATE.get(service_name, {}).get("shutdown_timeout", 15)
    _SERVICE_STATE.pop(service_name, None)
    _terminate_managed(managed, timeout, reason=reason)
    managed.cleanup()


def _launch_cancel_requested(cancel_event: threading.Event | None) -> bool:
    return cancel_event is not None and cancel_event.is_set()


def _wait_provider_launch_poll(
    cancel_event: threading.Event | None,
    interval_s: float,
) -> bool:
    if cancel_event is None:
        time.sleep(interval_s)
        return False
    return cancel_event.wait(interval_s)


def _cancelled_launch_outcome(
    provider: ProviderName,
    *,
    backend: str,
    port: int,
    managed: RunnerManagedProcess | None,
    reason: str,
) -> ProviderLaunchOutcome:
    cleanup_error: str | None = None
    if managed is not None:
        try:
            if provider == "parakeet":
                _cleanup_parakeet_launch(managed, reason)
            else:
                _terminate_cleanup_handle(managed, reason=reason)
        except Exception as exc:
            cleanup_error = str(exc)
            logger.exception(
                "%s provider launch cancellation cleanup failed; preserving handle",
                provider,
            )
    detail = {
        "backend": backend,
        "port": port,
        "cancelled": True,
        "cancel_reason": reason,
    }
    if cleanup_error is not None:
        detail |= {
            "cleanup_failed": True,
            "cleanup_error": cleanup_error,
            "cleanup_deferred_to": "cleanup-failed-reconciler",
        }
    return _outcome(
        "launch-failed",
        "launch-failed",
        detail,
        managed if cleanup_error is not None else None,
    )


def _start_mlx_local_server(
    plan: LocalServerLaunchPlan,
    reservation: ReservedPort | None = None,
    cancel_event: threading.Event | None = None,
) -> ProviderLaunchOutcome:
    """Launch the supervisor-owned mlx-vlm server from a captured plan."""
    from solstone.think.providers import local_server

    if plan.backend != "mlx":
        raise ValueError(f"MLX launch requires mlx plan, got {plan.backend!r}")
    runtime_dir = _required_plan_path(plan.runtime_dir, "runtime_dir")
    owned_reservation = (
        reservation if reservation is not None else ReservedPort.reserve()
    )
    port = owned_reservation.port
    script_path = Path(sys.executable).with_name(MLX_SERVER_PROCESS_NAME)

    logging.info("Starting mlx-vlm server for %s from %s", plan.model_id, runtime_dir)
    if _launch_cancel_requested(cancel_event):
        owned_reservation.close()
        return _cancelled_launch_outcome(
            "local",
            backend="mlx",
            port=port,
            managed=None,
            reason="provider launch cancelled before spawn",
        )
    try:
        owned_reservation.release_for_spawn()
        launch_plan = _request_local_launch_plan(
            plan,
            port,
            mlx_interpreter_path=script_path,
        )
        cmd = launch_plan["argv"]
        managed = _launch_process(MLX_SERVER_PROCESS_NAME, cmd)
    except Exception as exc:
        owned_reservation.close()
        logging.exception("MLX local server launch failed")
        return _outcome(
            "launch-failed",
            "launch-failed",
            {"backend": "mlx", "error": str(exc)},
        )
    print(f"  {LOCAL_MODEL_WARMING_UP_COPY}", flush=True)

    deadline = time.monotonic() + LOCAL_SERVER_READY_TIMEOUT_S
    while time.monotonic() < deadline:
        if _launch_cancel_requested(cancel_event):
            return _cancelled_launch_outcome(
                "local",
                backend="mlx",
                port=port,
                managed=managed,
                reason="provider launch cancelled during warmup",
            )
        if managed.process.poll() is not None:
            logging.warning(
                "mlx-vlm server exited during warmup with code %s",
                managed.process.returncode,
            )
            return _outcome(
                "exited",
                "process-exited",
                {
                    "backend": "mlx",
                    "returncode": managed.process.returncode,
                    "port": port,
                },
                managed,
            )
        if _launch_cancel_requested(cancel_event):
            return _cancelled_launch_outcome(
                "local",
                backend="mlx",
                port=port,
                managed=managed,
                reason="provider launch cancelled before health probe",
            )
        state, error = local_server._probe_health(port)
        if state == local_server.STATE_READY:
            if _launch_cancel_requested(cancel_event):
                return _cancelled_launch_outcome(
                    "local",
                    backend="mlx",
                    port=port,
                    managed=managed,
                    reason="provider launch cancelled before ready acceptance",
                )
            logging.info("mlx-vlm server ready on port %s", port)
            return _outcome(
                "ready",
                "probe-ready",
                {"backend": "mlx", "port": port},
                managed,
            )
        if state == local_server.STATE_FAILED and error:
            logging.debug("mlx-vlm server health probe failed during warmup: %s", error)
        if _wait_provider_launch_poll(
            cancel_event, LOCAL_SERVER_HEALTH_POLL_INTERVAL_S
        ):
            return _cancelled_launch_outcome(
                "local",
                backend="mlx",
                port=port,
                managed=managed,
                reason="provider launch cancelled during warmup wait",
            )

    if _launch_cancel_requested(cancel_event):
        return _cancelled_launch_outcome(
            "local",
            backend="mlx",
            port=port,
            managed=managed,
            reason="provider launch cancelled before timeout",
        )
    logging.warning(
        "mlx-vlm server did not become ready within %.0fs",
        LOCAL_SERVER_READY_TIMEOUT_S,
    )
    return _outcome(
        "warmup-timeout",
        "warmup-timeout",
        {"backend": "mlx", "port": port, "timeout_s": LOCAL_SERVER_READY_TIMEOUT_S},
        managed,
    )


def _format_vulkan_devices(devices: list[Any], local_vulkan: Any) -> str:
    if not devices:
        return "none"
    return "; ".join(
        (
            f"raw_index={device.index} name={device.name!r} "
            f"type={local_vulkan.classify(device)} vram_mib={device.vram_mib}"
        )
        for device in devices
    )


def _log_context_assertion(
    plan: LocalServerLaunchPlan, n_ctx: int | None, total_slots: int | None
) -> None:
    if plan.context_tokens is None or plan.parallel_slots is None:
        logging.info("llama-server context assertion skipped: launch plan incomplete")
        return
    expected_n_ctx = plan.context_tokens * plan.parallel_slots

    if n_ctx is None:
        logging.info(
            "llama-server context assertion skipped: n_ctx unavailable from /props"
        )
    else:
        context_matches = n_ctx == expected_n_ctx and (
            total_slots is None or total_slots == plan.parallel_slots
        )
        if context_matches:
            logging.info(
                "llama-server context OK: intended -c=%d parallel=%d actual n_ctx=%d",
                expected_n_ctx,
                plan.parallel_slots,
                n_ctx,
            )
        else:
            logging.warning(
                "llama-server context MISMATCH: intended -c=%d parallel=%d "
                "actual n_ctx=%d",
                expected_n_ctx,
                plan.parallel_slots,
                n_ctx,
            )

    if isinstance(total_slots, int):
        if total_slots == plan.parallel_slots:
            logging.info("llama-server slots OK: %d", total_slots)
        else:
            logging.warning(
                "llama-server slots MISMATCH: intended=%d actual=%d",
                plan.parallel_slots,
                total_slots,
            )
    else:
        logging.info("llama-server slot count not reported; skipped")


def parakeet_physical_thread_count() -> int:
    physical = psutil.cpu_count(logical=False)
    if isinstance(physical, int) and physical > 0:
        return physical
    return max(1, (os.cpu_count() or 2) // 2)


_HOST_READINESS_REASON_MAP: dict[str, ReasonCode] = {
    "platform_unsupported": "platform-unsupported",
    "package_unavailable": "package-unavailable",
    "binary_not_runnable": "package-unavailable",
    "openmp_runtime_unavailable": "openmp-runtime-unavailable",
    "ram_insufficient": "ram-insufficient",
    "gpu_probe_failed": "gpu-probe-failed",
    "gpu_unavailable": "gpu-unavailable",
}


def _target_fingerprint_pair(fingerprint: dict[str, Any]) -> tuple[str, str]:
    fingerprint_json = canonical_fingerprint(fingerprint)
    return fingerprint_json, fingerprint_sha256(fingerprint_json)


def _readiness_block_observation(
    *,
    provider: ProviderName,
    readiness: Any,
    fingerprint_json: str | None,
    fingerprint_sha256_value: str | None,
    boot_required: bool,
) -> ProviderTruthObservation | None:
    if readiness.status == "ready":
        return None
    if readiness.status == "missing-or-mismatched":
        install_state = readiness.install.get("install_state")
        return ProviderTruthObservation(
            provider=provider,
            phase="artifact-not-ready",
            reason_code="artifact-missing",
            desired_fingerprint_json=fingerprint_json,
            desired_fingerprint_sha256=fingerprint_sha256_value,
            boot_required=boot_required,
            detail={
                "readiness_status": readiness.status,
                "readiness_reason_code": readiness.reason_code,
                "install_state": install_state,
                "install_acquisition_allowed": install_state == "idle",
            },
        )
    if readiness.status == "proof-unavailable":
        return ProviderTruthObservation(
            provider=provider,
            phase="state-unavailable",
            reason_code="proof-observation-unavailable",
            desired_fingerprint_json=fingerprint_json,
            desired_fingerprint_sha256=fingerprint_sha256_value,
            boot_required=boot_required,
            detail={
                "readiness_status": readiness.status,
                "readiness_reason_code": readiness.reason_code,
            },
        )
    if readiness.status == "host-ineligible":
        reason = _HOST_READINESS_REASON_MAP.get(
            str(readiness.reason_code), "host-admission-blocked"
        )
        return ProviderTruthObservation(
            provider=provider,
            phase="host-blocked",
            reason_code=reason,
            desired_fingerprint_json=fingerprint_json,
            desired_fingerprint_sha256=fingerprint_sha256_value,
            boot_required=boot_required,
            detail={
                "readiness_status": readiness.status,
                "readiness_reason_code": readiness.reason_code,
                "host": readiness.host,
            },
        )
    return ProviderTruthObservation(
        provider=provider,
        phase="state-unavailable",
        reason_code="truth-observation-failed",
        desired_fingerprint_json=fingerprint_json,
        desired_fingerprint_sha256=fingerprint_sha256_value,
        boot_required=boot_required,
        detail={"readiness_status": readiness.status},
    )


def _local_install_progress_identity(
    *,
    desired_fingerprint_sha256: str | None,
    install_status: dict[str, Any],
) -> str:
    return canonical_fingerprint(
        {
            "desired_fingerprint_sha256": desired_fingerprint_sha256,
            "install_revision": install_status["revision"],
            "install_attempt_id": install_status["attempt_id"],
            "install_state": install_status["install_state"],
            "install_target_fingerprint_sha256": install_status[
                "target_fingerprint_sha256"
            ],
        }
    )


def _local_readiness_block_observation(
    *,
    readiness: Any,
    fingerprint_json: str | None,
    fingerprint_sha256_value: str | None,
    boot_required: bool,
    target_fingerprint: Callable[[], dict[str, Any]],
) -> ProviderTruthObservation | None:
    blocked = _readiness_block_observation(
        provider="local",
        readiness=readiness,
        fingerprint_json=fingerprint_json,
        fingerprint_sha256_value=fingerprint_sha256_value,
        boot_required=boot_required,
    )
    if blocked is None:
        return None
    if (
        blocked.phase != "artifact-not-ready"
        or blocked.reason_code != "artifact-missing"
    ):
        return blocked
    if readiness.install.get("install_state") not in IN_FLIGHT_STATES:
        return blocked

    before_status = read_install_status(name="local")
    if before_status["install_state"] not in IN_FLIGHT_STATES:
        return blocked
    attempt_id = before_status["attempt_id"]
    if not attempt_id:
        return blocked
    if before_status["target_fingerprint_sha256"] != fingerprint_sha256_value:
        return blocked

    from solstone.think.providers.install_lease import probe_install_lease_state

    try:
        lease_state = probe_install_lease_state("local")
    except OSError as exc:
        return ProviderTruthObservation(
            provider="local",
            phase="state-unavailable",
            reason_code="proof-observation-unavailable",
            desired_fingerprint_json=fingerprint_json,
            desired_fingerprint_sha256=fingerprint_sha256_value,
            boot_required=boot_required,
            detail={
                "readiness_status": readiness.status,
                "readiness_reason_code": readiness.reason_code,
                "error": str(exc),
            },
        )
    if lease_state != "held":
        return blocked

    after_status = read_install_status(name="local")
    _after_json, after_sha = _target_fingerprint_pair(target_fingerprint())
    before_identity = _local_install_progress_identity(
        desired_fingerprint_sha256=fingerprint_sha256_value,
        install_status=before_status,
    )
    after_identity = _local_install_progress_identity(
        desired_fingerprint_sha256=after_sha,
        install_status=after_status,
    )
    if before_identity != after_identity:
        return _observation_raced(
            "local",
            before_identity,
            after_identity,
            boot_required=boot_required,
        )

    return ProviderTruthObservation(
        provider="local",
        phase="artifact-not-ready",
        reason_code="install-in-progress",
        desired_fingerprint_json=fingerprint_json,
        desired_fingerprint_sha256=fingerprint_sha256_value,
        boot_required=boot_required,
        detail={
            "readiness_status": readiness.status,
            "readiness_reason_code": readiness.reason_code,
            "install_state": before_status["install_state"],
            "install_acquisition_allowed": False,
            "install_attempt_id": attempt_id,
            "install_revision": before_status["revision"],
        },
    )


def _observation_raced(
    provider: ProviderName,
    before: str,
    after: str,
    *,
    boot_required: bool,
) -> ProviderTruthObservation:
    return ProviderTruthObservation(
        provider=provider,
        phase="observing",
        reason_code="observation-raced",
        boot_required=boot_required,
        detail={"before": before, "after": after},
    )


def _local_plan_race_identity(plan: LocalServerLaunchPlan) -> dict[str, Any]:
    if plan.backend == "mlx":
        return {
            "backend": plan.backend,
            "runtime_dir": str(plan.runtime_dir),
            "model_id": plan.model_id,
        }
    if plan.backend == "cuda":
        from solstone.think.providers import local_cuda

        probe = local_cuda.probe_nvidia_gpu()
        return {
            "backend": plan.backend,
            "binary_path": str(plan.binary_path),
            "model_path": str(plan.model_path),
            "mmproj_path": str(plan.mmproj_path) if plan.mmproj_path else None,
            "gpu_index": probe.index if probe.index is not None else 0,
            "gpu_vram_mib": probe.vram_mib,
            "visible_devices_env": plan.visible_devices_env,
        }

    from solstone.think.providers import local_install, local_vulkan

    devices = local_vulkan.detect_gpus()
    selected = local_vulkan.select_device(
        devices, override_index=local_install.gpu_device_override()
    )
    return {
        "backend": plan.backend,
        "binary_path": str(plan.binary_path),
        "model_path": str(plan.model_path),
        "mmproj_path": str(plan.mmproj_path) if plan.mmproj_path else None,
        "gpu_index": selected.index if selected is not None else None,
        "gpu_name": selected.name if selected is not None else None,
        "gpu_vram_mib": selected.vram_mib if selected is not None else None,
        "visible_devices_env": plan.visible_devices_env,
    }


def _not_desired_observation(
    provider: ProviderName,
    reason: ReasonCode,
    *,
    detail: dict[str, Any] | None = None,
) -> ProviderTruthObservation:
    return ProviderTruthObservation(
        provider=provider,
        phase="not-desired",
        reason_code=reason,
        detail=detail or {},
        boot_required=False,
    )


def _parakeet_platform_can_host() -> bool:
    if not sys.platform.startswith("linux"):
        return False
    try:
        from solstone.think import parakeet_readiness

        parakeet_readiness.parakeet_cpp_artifact_key(
            "linux", platform.machine().lower()
        )
    except RuntimeError:
        return False
    return True


def _parakeet_stt_admission_input(
    transcribe: dict[str, Any], confidential_channel_usable: bool
) -> dict[str, Any]:
    backend = transcribe.get("backend") if isinstance(transcribe, dict) else None
    return {
        "platform": sys.platform,
        "machine": platform.machine().lower(),
        "backend": backend if isinstance(backend, str) else None,
        "local_backend": local_stt_backend(),
        "floor_bytes": stt_local_floor_bytes(),
        "confidential_lane_active": confidential_channel_usable,
        "confidential_audio_enabled": confidential_audio_enabled(transcribe),
    }


def _parakeet_stt_admission_latch(
    transcribe: dict[str, Any],
    confidential_channel_usable: bool,
) -> dict[str, Any]:
    global _parakeet_admission_retry_epoch

    admission_input = _parakeet_stt_admission_input(
        transcribe, confidential_channel_usable
    )
    input_json, input_sha = _target_fingerprint_pair(admission_input)
    try:
        current = read_runtime_health("parakeet")
    except (RuntimeHealthMalformedError, RuntimeHealthUnavailableError):
        raise
    existing = current["detail"].get("stt_admission_latch")
    if (
        isinstance(existing, dict)
        and existing.get("input_sha256") == input_sha
        and existing.get("retry_epoch") == _parakeet_admission_retry_epoch
        and isinstance(existing.get("desired"), bool)
        and isinstance(existing.get("blocked"), bool)
    ):
        return existing

    explicit_backend = admission_input["backend"]
    local_backend = admission_input["local_backend"]
    floor_bytes = admission_input["floor_bytes"]
    available_bytes = (
        None
        if explicit_backend in {"parakeet", "parakeet-cpp", "confidential"}
        or confidential_channel_usable
        else read_available_bytes()
    )
    choice = resolve_stt_backend_choice(
        explicit_backend if isinstance(explicit_backend, str) else None,
        available_bytes,
        floor_bytes=floor_bytes if isinstance(floor_bytes, int) else None,
        local_backend=local_backend if isinstance(local_backend, str) else None,
        confidential_lane_active=confidential_channel_usable,
        confidential_audio_enabled=bool(admission_input["confidential_audio_enabled"]),
    )
    desired = choice in {"parakeet", "parakeet-cpp"}
    ram_blocked = (
        choice == STT_SURFACE
        and explicit_backend is None
        and not confidential_channel_usable
        and local_backend in {"parakeet", "parakeet-cpp"}
        and floor_bytes is not None
    )
    return {
        "input_json": input_json,
        "input_sha256": input_sha,
        "retry_epoch": _parakeet_admission_retry_epoch,
        "choice": choice,
        "desired": desired,
        "blocked": ram_blocked,
        "reason_code": (
            "host-admission-blocked"
            if ram_blocked
            else (
                "confidential-backend-selected"
                if choice == "confidential"
                else "provider-not-needed"
            )
        ),
    }


def _observe_local_provider_truth() -> ProviderTruthObservation:
    if _is_remote_mode:
        return _not_desired_observation(
            "local", "provider-not-needed", detail={"remote_mode": True}
        )

    config = read_journal_config()
    active_local = is_local_provider_needed(config)
    from solstone.think.providers.local_endpoint import resolve_local_endpoint

    endpoint = resolve_local_endpoint()
    if not active_local:
        return _not_desired_observation(
            "local",
            "provider-not-needed",
            detail={
                "active_provider": config.get("providers", {})
                .get("active", {})
                .get("provider"),
                "projection": {"status": "reconciling"},
            },
        )
    if not endpoint.is_bundled:
        return _not_desired_observation(
            "local",
            "provider-not-needed",
            detail={"endpoint_mode": "byo", "projection": {"status": "reconciling"}},
        )

    try:
        if sys.platform == "darwin":
            return _observe_mlx_local_provider_truth()
        return _observe_linux_local_provider_truth()
    except InstallStatusMalformedError as exc:
        return ProviderTruthObservation(
            provider="local",
            phase="state-corrupt",
            reason_code="record-malformed",
            detail={"error": str(exc)},
            boot_required=True,
        )
    except OSError as exc:
        return ProviderTruthObservation(
            provider="local",
            phase="state-unavailable",
            reason_code="record-unavailable",
            detail={"error": str(exc)},
            boot_required=True,
        )
    except Exception as exc:
        logger.exception("local provider truth observation failed")
        return ProviderTruthObservation(
            provider="local",
            phase="state-unavailable",
            reason_code="truth-observation-failed",
            detail={"error": str(exc)},
            boot_required=True,
        )


def _observe_mlx_local_provider_truth() -> ProviderTruthObservation:
    from solstone.think.providers import mlx_install

    before_json, before_sha = _target_fingerprint_pair(mlx_install.target_fingerprint())
    readiness = mlx_install.inspect_readiness()
    blocked = _local_readiness_block_observation(
        readiness=readiness,
        fingerprint_json=before_json,
        fingerprint_sha256_value=before_sha,
        boot_required=True,
        target_fingerprint=mlx_install.target_fingerprint,
    )
    if blocked is not None:
        return blocked
    runtime_dir = Path(str(readiness.artifacts["runtime_dir"]))
    plan = LocalServerLaunchPlan(
        backend="mlx",
        desired_fingerprint_json=before_json,
        desired_fingerprint_sha256=before_sha,
        model_id=str(readiness.target["model_id"]),
        runtime_dir=runtime_dir,
    )
    after_json, after_sha = _target_fingerprint_pair(mlx_install.target_fingerprint())
    if before_sha != after_sha:
        return _observation_raced("local", before_sha, after_sha, boot_required=True)
    return ProviderTruthObservation(
        provider="local",
        phase="starting",
        reason_code="launch-requested",
        desired_fingerprint_json=after_json,
        desired_fingerprint_sha256=after_sha,
        plan=plan,
        boot_required=True,
        detail={"backend": "mlx", "model_id": plan.model_id},
    )


def _observe_linux_local_provider_truth() -> ProviderTruthObservation:
    from solstone.think.providers import local_cuda, local_install, local_server

    before_json, before_sha = _target_fingerprint_pair(
        local_install.target_fingerprint(LOCAL_MODEL)
    )
    readiness = local_install.inspect_readiness(LOCAL_MODEL)
    blocked = _local_readiness_block_observation(
        readiness=readiness,
        fingerprint_json=before_json,
        fingerprint_sha256_value=before_sha,
        boot_required=True,
        target_fingerprint=lambda: local_install.target_fingerprint(LOCAL_MODEL),
    )
    if blocked is not None:
        return blocked
    backend = str(readiness.host["backend"])
    binary_path = Path(str(readiness.artifacts["binary_path"]))
    model_path = Path(str(readiness.artifacts["model_path"]))
    mmproj = readiness.artifacts.get("mmproj_path")
    mmproj_path = Path(str(mmproj)) if mmproj else None
    if backend == "cuda":
        probe = local_cuda.probe_nvidia_gpu()
        gpu_index = probe.index if probe.index is not None else 0
        if probe.tiering_memory_mib is None:
            tier = local_server.select_server_tier(0)
        else:
            tier = local_server.select_server_tier(probe.tiering_memory_mib)
        lib_dir = local_install.cuda_binary_dir()
        plan = LocalServerLaunchPlan(
            backend="cuda",
            desired_fingerprint_json=before_json,
            desired_fingerprint_sha256=before_sha,
            binary_path=binary_path,
            model_path=model_path,
            mmproj_path=mmproj_path,
            lib_dir=lib_dir,
            gpu_index=gpu_index,
            gpu_vram_mib=probe.tiering_memory_mib,
            context_tokens=tier.context_tokens,
            parallel_slots=tier.parallel_slots,
            visible_devices_env=local_install.cuda_server_pin().visible_devices_env,
            backend_reason=str(readiness.host["backend_reason"]),
        )
    else:
        from solstone.think.providers import local_vulkan

        devices = local_vulkan.detect_gpus()
        override = local_install.gpu_device_override()
        selected = local_vulkan.select_device(devices, override_index=override)
        if selected is None:
            return ProviderTruthObservation(
                provider="local",
                phase="host-blocked",
                reason_code="gpu-unavailable",
                desired_fingerprint_json=before_json,
                desired_fingerprint_sha256=before_sha,
                boot_required=True,
                detail={
                    "readiness_status": "host-ineligible",
                    "readiness_reason_code": "gpu_unavailable",
                    "devices": _format_vulkan_devices(devices, local_vulkan),
                },
            )
        tier = local_server.select_server_tier(selected.vram_mib)
        plan = LocalServerLaunchPlan(
            backend="vulkan",
            desired_fingerprint_json=before_json,
            desired_fingerprint_sha256=before_sha,
            binary_path=binary_path,
            model_path=model_path,
            mmproj_path=mmproj_path,
            gpu_index=selected.index,
            gpu_name=selected.name,
            gpu_vram_mib=selected.vram_mib,
            vram_before_mib=local_vulkan.device_local_used_mib(selected.index),
            context_tokens=tier.context_tokens,
            parallel_slots=tier.parallel_slots,
            visible_devices_env="GGML_VK_VISIBLE_DEVICES",
            backend_reason=str(readiness.host["backend_reason"]),
        )
    before_identity_json, before_identity_sha = _target_fingerprint_pair(
        _local_plan_race_identity(plan)
    )
    after_json, after_sha = _target_fingerprint_pair(
        local_install.target_fingerprint(LOCAL_MODEL)
    )
    if before_sha != after_sha:
        return _observation_raced("local", before_sha, after_sha, boot_required=True)
    after_identity_json, after_identity_sha = _target_fingerprint_pair(
        _local_plan_race_identity(plan)
    )
    if before_identity_sha != after_identity_sha:
        return _observation_raced(
            "local",
            before_identity_json,
            after_identity_json,
            boot_required=True,
        )
    return ProviderTruthObservation(
        provider="local",
        phase="starting",
        reason_code="launch-requested",
        desired_fingerprint_json=after_json,
        desired_fingerprint_sha256=after_sha,
        plan=plan,
        boot_required=True,
        detail={"backend": plan.backend},
    )


def _observe_parakeet_provider_truth() -> ProviderTruthObservation:
    if _is_remote_mode:
        return _not_desired_observation(
            "parakeet", "provider-not-needed", detail={"remote_mode": True}
        )
    if not _parakeet_platform_can_host():
        return _not_desired_observation(
            "parakeet", "provider-not-needed", detail={"platform": sys.platform}
        )

    journal_path = Path(get_journal())
    try:
        from solstone.think.providers import local_vulkan, parakeet_install
        from solstone.think.providers.parakeet_placement import (
            decide_parakeet_auto_placement,
            discrete_hardware_gpu_count,
            is_discrete,
        )

        config = read_journal_config()
        transcribe = config.get("transcribe", {})
        if not isinstance(transcribe, dict):
            transcribe = {}
        from solstone.think.services import spp

        # Routing uses channel usability; dispatch refusal still keys on bare
        # confidential block presence to keep raw audio from accidental egress.
        confidential_channel_usable = spp.is_confidential_channel_usable(config)
        admission_latch = _parakeet_stt_admission_latch(
            transcribe, confidential_channel_usable
        )
        if admission_latch["blocked"]:
            return ProviderTruthObservation(
                provider="parakeet",
                phase="host-blocked",
                reason_code="host-admission-blocked",
                detail={"stt_admission_latch": admission_latch},
                boot_required=True,
            )
        if not admission_latch["desired"]:
            return _not_desired_observation(
                "parakeet",
                cast(ReasonCode, admission_latch["reason_code"]),
                detail={"stt_admission_latch": admission_latch},
            )

        before_json, before_sha = _target_fingerprint_pair(
            parakeet_install.target_fingerprint(journal_path=journal_path)
        )
        readiness = parakeet_install.inspect_readiness(journal_path)
        blocked = _readiness_block_observation(
            provider="parakeet",
            readiness=readiness,
            fingerprint_json=before_json,
            fingerprint_sha256_value=before_sha,
            boot_required=True,
        )
        if blocked is not None:
            return blocked

        config_device = _configured_parakeet_device()
        effective_device = config_device
        selected = None
        if config_device == "auto":
            devices = local_vulkan.detect_gpus()
            selected = local_vulkan.select_device(devices)
            selected_is_discrete = selected is not None and is_discrete(
                selected, local_vulkan
            )
            if selected is not None and selected_is_discrete:
                from solstone.think.providers import local_cuda
                from solstone.think.providers.local_endpoint import (
                    resolve_local_endpoint,
                )

                decision = decide_parakeet_auto_placement(
                    vram_mib=selected.vram_mib,
                    selected_device_is_discrete=selected_is_discrete,
                    discrete_hardware_gpu_count=discrete_hardware_gpu_count(
                        devices, local_vulkan
                    ),
                    unified_memory=(
                        local_cuda.probe_nvidia_gpu().memory_source
                        == local_cuda.MEMORY_SOURCE_SYSTEM_AVAILABLE
                    ),
                    brain_lane_active=(
                        is_local_provider_needed()
                        and resolve_local_endpoint().is_bundled
                    ),
                )
                if decision.force_cpu:
                    effective_device = "cpu"
        backend, env_updates, gpu_index = _resolve_parakeet_backend(
            effective_device, selected
        )
        binary_key = "binary_path_vulkan" if backend == "vulkan" else "binary_path_cpu"
        threads = parakeet_physical_thread_count()
        plan = ParakeetServerLaunchPlan(
            binary_backend=backend,
            env_updates=env_updates,
            gpu_index=gpu_index,
            binary_path=Path(str(readiness.artifacts[binary_key])),
            model_path=Path(str(readiness.artifacts["model_path"])),
            threads=threads,
            desired_fingerprint_json=before_json,
            desired_fingerprint_sha256=before_sha,
            placement="gpu" if backend == "vulkan" else "cpu",
        )
        after_json, after_sha = _target_fingerprint_pair(
            parakeet_install.target_fingerprint(journal_path=journal_path)
        )
        if before_sha != after_sha:
            return _observation_raced(
                "parakeet", before_sha, after_sha, boot_required=True
            )
        return ProviderTruthObservation(
            provider="parakeet",
            phase="starting",
            reason_code="launch-requested",
            desired_fingerprint_json=after_json,
            desired_fingerprint_sha256=after_sha,
            plan=plan,
            boot_required=True,
            detail={
                "backend": backend,
                "placement": plan.placement,
                "stt_admission_latch": admission_latch,
            },
        )
    except InstallStatusMalformedError as exc:
        return ProviderTruthObservation(
            provider="parakeet",
            phase="state-corrupt",
            reason_code="record-malformed",
            detail={"error": str(exc)},
            boot_required=True,
        )
    except OSError as exc:
        return ProviderTruthObservation(
            provider="parakeet",
            phase="state-unavailable",
            reason_code="record-unavailable",
            detail={"error": str(exc)},
            boot_required=True,
        )
    except Exception as exc:
        logger.exception("parakeet provider truth observation failed")
        return ProviderTruthObservation(
            provider="parakeet",
            phase="state-unavailable",
            reason_code="truth-observation-failed",
            detail={"error": str(exc)},
            boot_required=True,
        )


def _resolve_parakeet_backend(
    config_device: str, selected_gpu: Any
) -> tuple[str, dict[str, str], int | None]:
    if config_device not in {"auto", "cpu"}:
        raise ValueError(
            f"parakeet device must be 'auto' or 'cpu', got {config_device!r}"
        )
    if config_device == "cpu":
        return "cpu", {}, None
    if selected_gpu is not None:
        return (
            "vulkan",
            {"GGML_VK_VISIBLE_DEVICES": str(selected_gpu.index)},
            selected_gpu.index,
        )
    return "cpu", {}, None


def resolve_parakeet_server_launch_plan(
    config_device: str,
    selected_gpu: Any,
    *,
    binary_path: Path,
    model_path: Path,
    threads: int,
    desired_fingerprint_json: str = "",
    desired_fingerprint_sha256: str = "",
) -> ParakeetServerLaunchPlan:
    backend, env_updates, gpu_index = _resolve_parakeet_backend(
        config_device, selected_gpu
    )
    return ParakeetServerLaunchPlan(
        binary_backend=backend,
        env_updates=env_updates,
        gpu_index=gpu_index,
        binary_path=binary_path,
        model_path=model_path,
        threads=threads,
        desired_fingerprint_json=desired_fingerprint_json,
        desired_fingerprint_sha256=desired_fingerprint_sha256,
        placement="gpu" if backend == "vulkan" else "cpu",
    )


def _run_parakeet_bootstrap_worker(
    journal_path: Path | None = None,
    lease: Any | None = None,
    attempt_status: Any | None = None,
    ack: threading.Event | None = None,
    cancel: threading.Event | None = None,
    transfer_lock: threading.Lock | None = None,
) -> None:
    """Install parakeet.cpp artifacts in the background."""
    if ack is not None:
        if cancel is not None and transfer_lock is not None:
            with transfer_lock:
                if cancel.is_set():
                    return
                ack.set()
        else:
            ack.set()
    try:
        from solstone.think.providers import parakeet_install

        parakeet_install.install_parakeet(
            journal_path=journal_path,
            lease=lease,
            attempt_status=attempt_status,
        )
    except Exception:
        logging.exception("parakeet.cpp provider bootstrap failed")
    else:
        logging.info("parakeet.cpp provider bootstrap complete")
    finally:
        if lease is not None:
            lease.release()


def _parakeet_bootstrap_target_still_current(
    fence: ProviderFence | None,
    desired_fingerprint_sha256: str | None,
) -> bool:
    if fence is None:
        return True
    state = _provider_runtime_states["parakeet"]
    return (
        fence.incarnation == _PROVIDER_INCARNATION
        and fence.generation == state.generation
        and fence.fingerprint == state.desired_fingerprint
        and state.desired_fingerprint == desired_fingerprint_sha256
        and state.latest_phase in {"artifact-not-ready", "observing"}
    )


def _start_parakeet_bootstrap_if_needed(
    reason: str,
    fence: ProviderFence | None = None,
    desired_fingerprint_sha256: str | None = None,
) -> bool:
    """Start one non-blocking parakeet.cpp install worker when artifacts are absent."""
    from solstone.think.providers import parakeet_install
    from solstone.think.providers.install_lease import acquire_install_lease
    from solstone.think.providers.install_state import begin_or_replace_install_attempt

    try:
        readiness = parakeet_install.inspect_readiness()
    except Exception as exc:
        logging.info(
            "could not inspect parakeet.cpp readiness before bootstrap: %s", exc
        )
        readiness = None

    if readiness is not None and readiness.ready:
        return False

    journal_path = Path(get_journal())
    if not _parakeet_bootstrap_target_still_current(fence, desired_fingerprint_sha256):
        logging.info("parakeet.cpp provider bootstrap abandoned: target changed")
        return False

    lease = acquire_install_lease("parakeet", journal_path=journal_path)
    if lease is None:
        logging.info("parakeet.cpp provider bootstrap already running")
        return False
    try:
        fingerprint = parakeet_install.target_fingerprint(journal_path=journal_path)
        attempt_status = begin_or_replace_install_attempt(
            "parakeet",
            fingerprint,
            initial_state="downloading",
            owner={"entry": "supervisor_parakeet_bootstrap"},
            journal_path=journal_path,
        )
        ack = threading.Event()
        cancel = threading.Event()
        transfer_lock = threading.Lock()
        thread = threading.Thread(
            target=lambda: _run_parakeet_bootstrap_worker(
                journal_path,
                lease,
                attempt_status,
                ack,
                cancel,
                transfer_lock,
            ),
            name="parakeet-cpp-provider-bootstrap",
            daemon=True,
        )
    except Exception:
        lease.release()
        logging.exception("could not prepare parakeet.cpp provider bootstrap worker")
        return False

    logging.info(
        "Parakeet artifacts not ready; starting background provider install: %s",
        reason,
    )
    try:
        thread.start()
    except Exception:
        lease.release()
        logging.exception("could not start parakeet.cpp provider bootstrap worker")
        return False
    if not ack.wait(timeout=5.0):
        cancelled = False
        with transfer_lock:
            if not ack.is_set():
                cancel.set()
                cancelled = True
        if cancelled:
            lease.release()
            logging.error("parakeet.cpp provider bootstrap worker did not acknowledge")
        return False
    return True


def _build_parakeet_cmd(
    binary_path: Path, gguf_path: Path, port: int, threads: int
) -> list[str]:
    # Re-verify exact parakeet.cpp v0.5.0 CLI flag spellings at live bring-up.
    cmd = [
        str(binary_path),
        "--model",
        str(gguf_path),
        "--host",
        "127.0.0.1",
        "--port",
        str(port),
        "--threads",
        str(threads),
    ]
    if "0.0.0.0" in cmd:
        raise RuntimeError("parakeet server may not bind 0.0.0.0.")
    return cmd


def _start_llama_local_server(
    plan: LocalServerLaunchPlan,
    reservation: ReservedPort | None = None,
    cancel_event: threading.Event | None = None,
) -> ProviderLaunchOutcome:
    """Launch a CUDA/Vulkan llama-server from a captured plan."""
    from solstone.think.providers import local_server, local_vulkan

    if plan.backend not in {"cuda", "vulkan"}:
        raise ValueError(
            f"llama-server launch requires CUDA/Vulkan plan, got {plan.backend!r}"
        )
    owned_reservation = (
        reservation if reservation is not None else ReservedPort.reserve()
    )
    port = owned_reservation.port
    try:
        if _launch_cancel_requested(cancel_event):
            owned_reservation.close()
            return _cancelled_launch_outcome(
                "local",
                backend=plan.backend,
                port=port,
                managed=None,
                reason="provider launch cancelled before spawn",
            )
        owned_reservation.release_for_spawn()
        launch_plan = _request_local_launch_plan(plan, port)
        cmd = launch_plan["argv"]
        context_tokens = _required_launch_plan_int(launch_plan, "context_tokens")
        parallel_slots = _required_launch_plan_int(launch_plan, "parallel_slots")
        prompt_cache_mib = _required_launch_plan_int(launch_plan, "prompt_cache_mib")
        extra_env = launch_plan.get("extra_env")
        if not isinstance(extra_env, dict) or not all(
            isinstance(key, str) and isinstance(value, str)
            for key, value in extra_env.items()
        ):
            raise ValueError("native launch plan is missing extra_env")
        logging.info(
            "local server backend=%s context=%d parallel=%d cache=%d MiB",
            plan.backend,
            context_tokens,
            parallel_slots,
            prompt_cache_mib,
        )
        managed = _launch_process(
            LOCAL_SERVER_PROCESS_NAME,
            cmd,
            env=os.environ | extra_env,
        )
    except Exception as exc:
        owned_reservation.close()
        logging.exception("%s local server launch failed", plan.backend.upper())
        return _outcome(
            "launch-failed",
            "launch-failed",
            {"backend": plan.backend, "error": str(exc)},
        )
    print(f"  {LOCAL_MODEL_WARMING_UP_COPY}", flush=True)

    deadline = time.monotonic() + LOCAL_SERVER_READY_TIMEOUT_S
    while time.monotonic() < deadline:
        if _launch_cancel_requested(cancel_event):
            return _cancelled_launch_outcome(
                "local",
                backend=plan.backend,
                port=port,
                managed=managed,
                reason="provider launch cancelled during warmup",
            )
        if managed.process.poll() is not None:
            logging.warning(
                "%s local server exited during warmup with code %s",
                plan.backend.upper(),
                managed.process.returncode,
            )
            return _outcome(
                "exited",
                "process-exited",
                {
                    "backend": plan.backend,
                    "returncode": managed.process.returncode,
                    "port": port,
                },
                managed,
            )
        if _launch_cancel_requested(cancel_event):
            return _cancelled_launch_outcome(
                "local",
                backend=plan.backend,
                port=port,
                managed=managed,
                reason="provider launch cancelled before health probe",
            )
        state, error = local_server._probe_health(port)
        if state == local_server.STATE_READY:
            if _launch_cancel_requested(cancel_event):
                return _cancelled_launch_outcome(
                    "local",
                    backend=plan.backend,
                    port=port,
                    managed=managed,
                    reason="provider launch cancelled before ready acceptance",
                )
            if plan.backend == "vulkan" and plan.gpu_index is not None:
                vram_after_mib = local_vulkan.device_local_used_mib(plan.gpu_index)
                if plan.vram_before_mib is not None and vram_after_mib is not None:
                    logging.info(
                        "local GPU: %s — VRAM used %+d MiB after model load (%d -> %d MiB)",
                        plan.gpu_name,
                        vram_after_mib - plan.vram_before_mib,
                        plan.vram_before_mib,
                        vram_after_mib,
                    )
            props = local_server.fetch_props(port)
            n_ctx = local_server._extract_n_ctx(props) if props is not None else None
            total_slots = (
                local_server._extract_total_slots(props) if props is not None else None
            )
            _log_context_assertion(
                replace(
                    plan,
                    context_tokens=context_tokens,
                    parallel_slots=parallel_slots,
                ),
                n_ctx,
                total_slots,
            )
            logging.info("llama-server ready on port %s", port)
            return _outcome(
                "ready",
                "probe-ready",
                {"backend": plan.backend, "port": port},
                managed,
            )
        if state == local_server.STATE_FAILED and error:
            logging.debug("llama-server health probe failed during warmup: %s", error)
        if _wait_provider_launch_poll(
            cancel_event, LOCAL_SERVER_HEALTH_POLL_INTERVAL_S
        ):
            return _cancelled_launch_outcome(
                "local",
                backend=plan.backend,
                port=port,
                managed=managed,
                reason="provider launch cancelled during warmup wait",
            )

    if _launch_cancel_requested(cancel_event):
        return _cancelled_launch_outcome(
            "local",
            backend=plan.backend,
            port=port,
            managed=managed,
            reason="provider launch cancelled before timeout",
        )
    logging.warning(
        "llama-server did not become ready within %.0fs",
        LOCAL_SERVER_READY_TIMEOUT_S,
    )
    return _outcome(
        "warmup-timeout",
        "warmup-timeout",
        {
            "backend": plan.backend,
            "port": port,
            "timeout_s": LOCAL_SERVER_READY_TIMEOUT_S,
        },
        managed,
    )


def _start_cuda_local_server(
    plan: LocalServerLaunchPlan,
    reservation: ReservedPort | None = None,
    cancel_event: threading.Event | None = None,
) -> ProviderLaunchOutcome:
    """Launch the CUDA llama-server path from a captured plan."""
    if plan.backend != "cuda":
        raise ValueError(f"CUDA launch requires cuda plan, got {plan.backend!r}")
    return _start_llama_local_server(plan, reservation, cancel_event)


def start_local_server(
    plan: LocalServerLaunchPlan,
    reservation: ReservedPort | None = None,
    cancel_event: threading.Event | None = None,
) -> ProviderLaunchOutcome:
    """Launch the supervisor-owned bundled local runtime from a captured plan."""
    if plan.backend == "mlx":
        return _start_mlx_local_server(plan, reservation, cancel_event)
    if plan.backend == "cuda":
        return _start_cuda_local_server(plan, reservation, cancel_event)
    if plan.backend == "vulkan":
        return _start_llama_local_server(plan, reservation, cancel_event)
    raise ValueError(f"unknown local launch backend {plan.backend!r}")


def _launch_and_warm_parakeet(
    plan: ParakeetServerLaunchPlan,
    reservation: ReservedPort | None = None,
    cancel_event: threading.Event | None = None,
) -> ProviderLaunchOutcome:
    """Launch one parakeet-server backend from a captured plan and warm it."""
    from solstone.think.providers import parakeet_server
    from solstone.think.providers.parakeet_placement import (
        PARAKEET_ATT_CONTEXT_ENV,
        PARAKEET_ATT_CONTEXT_FRAMES,
    )

    owned_reservation = (
        reservation if reservation is not None else ReservedPort.reserve()
    )
    port = owned_reservation.port
    env = os.environ | plan.env_updates
    env = env | {PARAKEET_ATT_CONTEXT_ENV: str(PARAKEET_ATT_CONTEXT_FRAMES)}
    logging.info(
        "parakeet-server launch backend=%s attention=local att_context_frames=%d",
        plan.binary_backend,
        PARAKEET_ATT_CONTEXT_FRAMES,
    )
    try:
        cmd = _build_parakeet_cmd(plan.binary_path, plan.model_path, port, plan.threads)
        if _launch_cancel_requested(cancel_event):
            owned_reservation.close()
            return _cancelled_launch_outcome(
                "parakeet",
                backend=plan.binary_backend,
                port=port,
                managed=None,
                reason="provider launch cancelled before spawn",
            )
        owned_reservation.release_for_spawn()
        managed = _launch_process(PARAKEET_SERVER_PROCESS_NAME, cmd, env=env)
    except Exception as exc:
        owned_reservation.close()
        logging.exception("parakeet-server launch failed")
        return _outcome(
            "launch-failed",
            "launch-failed",
            {"backend": plan.binary_backend, "error": str(exc)},
        )

    deadline = time.monotonic() + PARAKEET_SERVER_READY_TIMEOUT_S
    while time.monotonic() < deadline:
        if _launch_cancel_requested(cancel_event):
            return _cancelled_launch_outcome(
                "parakeet",
                backend=plan.binary_backend,
                port=port,
                managed=managed,
                reason="provider launch cancelled during warmup",
            )
        if managed.process.poll() is not None:
            logging.warning(
                "parakeet-server %s exited during warmup with code %s",
                plan.binary_backend,
                managed.process.returncode,
            )
            return _outcome(
                "exited",
                "process-exited",
                {
                    "backend": plan.binary_backend,
                    "returncode": managed.process.returncode,
                    "port": port,
                },
                managed,
            )
        if _launch_cancel_requested(cancel_event):
            return _cancelled_launch_outcome(
                "parakeet",
                backend=plan.binary_backend,
                port=port,
                managed=managed,
                reason="provider launch cancelled before health probe",
            )
        state, error = parakeet_server._probe_health(port)
        if state == parakeet_server.STATE_READY:
            if _launch_cancel_requested(cancel_event):
                return _cancelled_launch_outcome(
                    "parakeet",
                    backend=plan.binary_backend,
                    port=port,
                    managed=managed,
                    reason="provider launch cancelled before ready acceptance",
                )
            logging.info("parakeet-server ready on port %s", port)
            return _outcome(
                "ready",
                "probe-ready",
                {
                    "backend": plan.binary_backend,
                    "placement": plan.placement,
                    "port": port,
                },
                managed,
            )
        if state == parakeet_server.STATE_FAILED and error:
            logging.debug(
                "parakeet-server health probe failed during warmup: %s", error
            )
        if _wait_provider_launch_poll(
            cancel_event, PARAKEET_SERVER_HEALTH_POLL_INTERVAL_S
        ):
            return _cancelled_launch_outcome(
                "parakeet",
                backend=plan.binary_backend,
                port=port,
                managed=managed,
                reason="provider launch cancelled during warmup wait",
            )
    if _launch_cancel_requested(cancel_event):
        return _cancelled_launch_outcome(
            "parakeet",
            backend=plan.binary_backend,
            port=port,
            managed=managed,
            reason="provider launch cancelled before timeout",
        )
    logging.warning(
        "parakeet-server %s did not become ready within %.0fs",
        plan.binary_backend,
        PARAKEET_SERVER_READY_TIMEOUT_S,
    )
    return _outcome(
        "warmup-timeout",
        "warmup-timeout",
        {
            "backend": plan.binary_backend,
            "placement": plan.placement,
            "port": port,
            "timeout_s": PARAKEET_SERVER_READY_TIMEOUT_S,
        },
        managed,
    )


def _cleanup_parakeet_launch(managed: RunnerManagedProcess, reason: str) -> None:
    _terminate_cleanup_handle(managed, reason=reason)


def start_parakeet_server(
    plan: ParakeetServerLaunchPlan,
    reservation: ReservedPort | None = None,
    cancel_event: threading.Event | None = None,
) -> ProviderLaunchOutcome:
    """Launch supervisor-owned parakeet-server from a captured plan."""
    return _launch_and_warm_parakeet(plan, reservation, cancel_event)


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
    convey_mp, *, timeout: float = CONVEY_READY_WINDOW_SECONDS, interval: float = 0.1
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


def stop_callosum_in_process(join_timeout: float = 5.0) -> None:
    """Stop the in-process Callosum server."""
    global _callosum_server, _callosum_thread

    if _callosum_server:
        logging.info("Stopping Callosum server...")
        _callosum_server.stop()

    if _callosum_thread:
        _callosum_thread.join(timeout=join_timeout)
        if _callosum_thread.is_alive():
            logging.warning("Callosum server thread did not stop cleanly")

    _callosum_server = None
    _callosum_thread = None


def start_cortex_server() -> RunnerManagedProcess:
    """Launch the Cortex WebSocket API server."""
    cmd = ["journal", "cortex", "-v"]
    return _launch_process("cortex", cmd, restart=True)


def start_spl_service() -> RunnerManagedProcess:
    """Launch the spl tunnel service."""
    cmd = ["journal", "spl", "-v"]
    return _launch_process("spl", cmd, restart=True)


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


def _record_provider_exit_for_reconciler(
    provider: ProviderName,
    managed: RunnerManagedProcess,
    *,
    returncode: int | None,
) -> None:
    state = _provider_runtime_states[provider]
    state.generation += 1
    state.retry = ProviderRetryState(desired_fingerprint=state.desired_fingerprint)
    state.latest_plan = None
    state.latest_phase = "stopped"
    state.next_truth_at = 0.0
    state.next_probe_at = 0.0
    _mark_provider_recovery_down(provider)
    _write_provider_runtime(
        state,
        phase="stopped",
        reason_code="process-exited",
        detail={
            "process_name": managed.name,
            "pid": managed.process.pid,
            "ref": managed.ref,
            "returncode": returncode,
        },
        process=None,
    )


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
        parts = []
        for m in sorted(exited, key=lambda managed: managed.name):
            policy = _get_restart_policy(m.name)
            if policy.last_start:
                uptime = f"up {time.time() - policy.last_start:.1f}s"
            else:
                uptime = "up unknown"
            parts.append(f"{m.name} ({describe_exit(m.process.returncode)}, {uptime})")
        msg = f"Runner process exited: {', '.join(parts)}"
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

        provider = _provider_for_process_name(managed.name)
        if provider is not None:
            _record_provider_exit_for_reconciler(
                provider,
                managed,
                returncode=returncode,
            )
            logging.info(
                "%s provider process exit reported to runtime reconciler",
                managed.name,
            )
            continue

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


def _nudge_catchup_drain(exclude_today: bool = False) -> None:
    """Ask the supervisor loopback path to drain pending catchup work."""
    if _supervisor_callosum is None:
        logging.warning("Cannot nudge catchup drain: supervisor callosum unavailable")
        return

    try:
        if exclude_today:
            _supervisor_callosum.emit("supervisor", "drain", exclude_today=True)
        else:
            _supervisor_callosum.emit("supervisor", "drain")
    except Exception as exc:
        logging.warning("Cannot nudge catchup drain: %s", exc)


_PROVIDER_STARTUP_TERMINAL_PHASES: frozenset[RuntimePhase] = frozenset(
    {
        "ready",
        "ready-proof-unavailable",
        "not-desired",
        "artifact-not-ready",
        "host-blocked",
        "failed",
        "state-corrupt",
        "state-unavailable",
    }
)
_PROVIDER_START_CANCEL_PHASES: frozenset[RuntimePhase] = frozenset(
    {
        "not-desired",
        "state-corrupt",
        "state-unavailable",
    }
)
_PROVIDER_TRUTH_PRESERVED_PHASES: frozenset[RuntimePhase] = frozenset(
    {
        "ready",
        "ready-proof-unavailable",
        "stop-deferred",
        "stopping",
        "cleanup-failed",
    }
)
_PROVIDER_PROCESS_NAMES: dict[ProviderName, frozenset[str]] = {
    "local": frozenset({LOCAL_SERVER_PROCESS_NAME, MLX_SERVER_PROCESS_NAME}),
    "parakeet": frozenset({PARAKEET_SERVER_PROCESS_NAME}),
}
_PROVIDER_PORT_SERVICES: dict[ProviderName, str] = {
    "local": "local",
    "parakeet": "parakeet-cpp",
}


def _runtime_updated_at() -> str:
    return datetime.now(timezone.utc).isoformat()


def _display_deadline_at(delay_s: float | None) -> str | None:
    if delay_s is None:
        return None
    deadline = time.time() + max(0.0, delay_s)
    return datetime.fromtimestamp(deadline, timezone.utc).isoformat()


def _provider_executor() -> concurrent.futures.ThreadPoolExecutor:
    global _provider_runtime_executor
    if _provider_runtime_executor is None:
        _provider_runtime_executor = concurrent.futures.ThreadPoolExecutor(
            max_workers=4,
            thread_name_prefix="provider-runtime",
        )
    return _provider_runtime_executor


def _provider_fence(state: ProviderRuntimeState, attempt: int) -> ProviderFence:
    return ProviderFence(
        incarnation=_PROVIDER_INCARNATION,
        generation=state.generation,
        fingerprint=state.desired_fingerprint,
        attempt=attempt,
    )


def _provider_fence_matches(state: ProviderRuntimeState, fence: ProviderFence) -> bool:
    return (
        fence.incarnation == _PROVIDER_INCARNATION
        and fence.generation == state.generation
        and fence.fingerprint == state.desired_fingerprint
        and fence.attempt == state.retry.attempt_count
    )


def _provider_running_process(
    provider: ProviderName, procs: list[RunnerManagedProcess]
) -> RunnerManagedProcess | None:
    names = _PROVIDER_PROCESS_NAMES[provider]
    for managed in procs:
        if managed.name in names and managed.is_running():
            return managed
    return None


def _provider_processes(
    provider: ProviderName, procs: list[RunnerManagedProcess]
) -> list[RunnerManagedProcess]:
    names = _PROVIDER_PROCESS_NAMES[provider]
    return [
        managed for managed in procs if managed.name in names and managed.is_running()
    ]


def _provider_for_process_name(name: str) -> ProviderName | None:
    for provider, names in _PROVIDER_PROCESS_NAMES.items():
        if name in names:
            return provider
    return None


def _remove_provider_process(
    procs: list[RunnerManagedProcess], managed: RunnerManagedProcess | None
) -> None:
    if managed is None:
        return
    try:
        procs.remove(managed)
    except ValueError:
        pass


def _runtime_record_matches_managed(
    record: RuntimeHealthRecord,
    managed: RunnerManagedProcess,
) -> bool:
    process = record["process"]
    if not isinstance(process, dict):
        return False
    return (
        process.get("name") == managed.name
        and process.get("pid") == managed.process.pid
        and process.get("ref") == managed.ref
    )


def _local_stop_capacity(state: ProviderRuntimeState) -> int:
    plan = state.latest_plan
    if isinstance(plan, LocalServerLaunchPlan) and plan.parallel_slots is not None:
        return max(1, int(plan.parallel_slots))
    return 1


def _mark_provider_recovery_down(
    provider: ProviderName,
    *,
    generation: int | None = None,
) -> None:
    state = _provider_runtime_states[provider]
    recovery = _recovery_state[provider]
    recovery.down_generation = state.generation if generation is None else generation


def _consume_local_recovery_nudge(state: ProviderRuntimeState) -> None:
    if state.provider != "local":
        return
    if _is_remote_mode:
        return
    recovery = _recovery_state["local"]
    if recovery.down_generation != state.generation:
        return
    if recovery.nudged_generation == state.generation:
        return
    recovery.nudged_generation = state.generation
    _nudge_catchup_drain()


def _provider_process_record(
    managed: RunnerManagedProcess | None,
    *,
    port: int | None = None,
) -> dict[str, Any] | None:
    if managed is None:
        return None
    return {
        "name": managed.name,
        "pid": managed.process.pid,
        "ref": managed.ref,
        "port": port,
    }


def _current_provider_process_record(provider: ProviderName) -> dict[str, Any] | None:
    try:
        current = read_runtime_health(provider)
    except (RuntimeHealthMalformedError, RuntimeHealthUnavailableError):
        return None
    process = current["process"]
    return process if isinstance(process, dict) else None


def _provider_port_path(provider: ProviderName) -> Path:
    service = _PROVIDER_PORT_SERVICES[provider]
    return Path(get_journal()) / "health" / f"{service}.port"


def _publish_provider_port(
    provider: ProviderName,
    *,
    port: int,
) -> None:
    write_service_port(_PROVIDER_PORT_SERVICES[provider], port)


def _write_provider_ready_side_effects(
    state: ProviderRuntimeState,
    outcome: ProviderLaunchOutcome,
) -> None:
    plan = state.latest_plan
    if state.provider == "local":
        if not isinstance(plan, LocalServerLaunchPlan):
            return
        from solstone.think.providers import local_server

        local_server.reset_parallel_slots_cache()
        if plan.backend in {"cuda", "vulkan"}:
            local_server.write_local_context_window(
                _required_plan_int(plan.context_tokens, "context_tokens")
            )
        elif plan.backend == "mlx":
            local_server.clear_local_context_window()
        expected_fingerprint = plan.desired_fingerprint_sha256
        if (
            _task_queue is not None
            and isinstance(expected_fingerprint, str)
            and expected_fingerprint
        ):
            _task_queue.submit(
                [
                    "journal",
                    "brain",
                    "refresh",
                    "--expected-fingerprint",
                    expected_fingerprint,
                ]
            )
        return

    if state.provider == "parakeet":
        if not isinstance(plan, ParakeetServerLaunchPlan):
            return
        from solstone.think.providers import parakeet_server

        placement = outcome.detail.get("placement")
        parakeet_server.write_parakeet_placement(
            placement if placement in {"cpu", "gpu"} else plan.placement
        )


def _runtime_detail_with_preserved_latch(
    state: ProviderRuntimeState,
    current: RuntimeHealthRecord,
    detail: dict[str, Any],
) -> dict[str, Any]:
    if state.provider != "parakeet":
        return detail
    if current["desired_fingerprint_sha256"] != state.desired_fingerprint:
        return detail
    if "stt_admission_latch" in detail:
        return detail
    existing = current["detail"].get("stt_admission_latch")
    if not isinstance(existing, dict):
        return detail
    return {**detail, "stt_admission_latch": existing}


def _clear_provider_port_if_owner_matches(
    state: ProviderRuntimeState,
    fence: ProviderFence | None,
    outcome: ProviderLaunchOutcome,
) -> None:
    if fence is None:
        return
    port = outcome.detail.get("port")
    if not isinstance(port, int):
        return
    try:
        current = read_runtime_health(state.provider)
    except (RuntimeHealthMalformedError, RuntimeHealthUnavailableError):
        return
    process = current["process"]
    if not isinstance(process, dict):
        return
    if (
        current["incarnation"] != fence.incarnation
        or current["generation"] != fence.generation
        or current["attempt"] != fence.attempt
        or process.get("port") != port
    ):
        return
    path = _provider_port_path(state.provider)
    try:
        if read_service_port(_PROVIDER_PORT_SERVICES[state.provider]) == port:
            path.unlink(missing_ok=True)
    except OSError as exc:
        logger.warning(
            "could not clear %s provider port file for generation %s attempt %s: %s",
            state.provider,
            fence.generation,
            fence.attempt,
            exc,
        )


def _write_provider_runtime(
    state: ProviderRuntimeState,
    *,
    phase: RuntimePhase,
    reason_code: ReasonCode | None,
    detail: dict[str, Any],
    attempt: int | None = None,
    process: dict[str, Any] | None = None,
    display_deadline_delay_s: float | None = None,
) -> RuntimeHealthRecord | None:
    try:
        current = read_runtime_health(state.provider)
        detail = _runtime_detail_with_preserved_latch(state, current, detail)
        record: RuntimeHealthRecord = {
            **current,
            "phase": phase,
            "reason_code": reason_code,
            "detail": detail,
            "desired_fingerprint_sha256": state.desired_fingerprint,
            "incarnation": _PROVIDER_INCARNATION,
            "generation": state.generation,
            "attempt": state.retry.attempt_count if attempt is None else attempt,
            "process": process,
            "updated_at": _runtime_updated_at(),
            "display_deadline_at": _display_deadline_at(display_deadline_delay_s),
            "owner": {"module": "solstone.think.supervisor"},
        }
        return write_runtime_health(record)
    except RuntimeHealthMalformedError as exc:
        state.latest_phase = "state-corrupt"
        logger.error("%s runtime health record is corrupt: %s", state.provider, exc)
    except RuntimeHealthUnavailableError as exc:
        state.latest_phase = "state-unavailable"
        logger.error("%s runtime health record is unavailable: %s", state.provider, exc)
    except RuntimeHealthConflictError as exc:
        logger.info("%s runtime health write lost race: %s", state.provider, exc)
    return None


def _signal_provider_start_cancel(
    state: ProviderRuntimeState,
    *,
    reason: str,
) -> None:
    event = state.start_cancel_event
    if event is None or event.is_set():
        return
    fence = state.start_fence
    logger.info(
        "cancelling %s provider start%s: %s",
        state.provider,
        (
            f" generation={fence.generation} attempt={fence.attempt}"
            if fence is not None
            else ""
        ),
        reason,
    )
    event.set()


def _cancel_all_provider_starts(reason: str) -> None:
    for state in _provider_runtime_states.values():
        _signal_provider_start_cancel(state, reason=reason)


def _signal_provider_stop_cancel(
    state: ProviderRuntimeState,
    *,
    reason: str,
) -> None:
    event = state.stop_cleanup_cancel_event
    if event is None or event.is_set():
        return
    fence = state.stop_cleanup_fence
    logger.info(
        "cancelling %s provider stop%s: %s",
        state.provider,
        (
            f" generation={fence.generation} attempt={fence.attempt}"
            if fence is not None
            else ""
        ),
        reason,
    )
    event.set()


def _cancel_all_provider_stops(reason: str) -> None:
    for state in _provider_runtime_states.values():
        _signal_provider_stop_cancel(state, reason=reason)


def _cleanup_provider_outcome_handle(
    state: ProviderRuntimeState,
    fence: ProviderFence | None,
    outcome: ProviderLaunchOutcome,
    *,
    reason: str,
) -> bool:
    if outcome.managed is None:
        return True
    try:
        _clear_provider_port_if_owner_matches(state, fence, outcome)
        _terminate_cleanup_handle(outcome.managed, reason=reason)
    except Exception as exc:
        logger.exception("%s provider cleanup failed: %s", state.provider, reason)
        _adopt_provider_cleanup_failed_handle(
            state,
            outcome.managed,
            detail={
                **outcome.detail,
                "error": str(exc),
                "cleanup_reason": reason,
                "cleanup_deferred_to": "cleanup-failed-reconciler",
            },
        )
        return False
    return True


def _stop_cleanup_outcome(
    status: StopCleanupStatus,
    reason_code: ReasonCode,
    detail: dict[str, Any],
    managed: RunnerManagedProcess | None = None,
) -> ProviderStopCleanupOutcome:
    return ProviderStopCleanupOutcome(
        status=status,
        reason_code=reason_code,
        detail=detail,
        managed=managed,
    )


def _set_provider_pending_stop_request(
    state: ProviderRuntimeState,
    request: ProviderStopCleanupRequest | None,
) -> None:
    state.pending_stop_request = request


def _clear_provider_pending_stop_request(
    state: ProviderRuntimeState,
    *,
    reason: str,
    resolved: bool,
    outcome: ProviderStopCleanupOutcome | None = None,
) -> bool:
    request = state.pending_stop_request
    if request is None:
        state.cleanup_attempt_count = 0
        state.cleanup_next_at = 0.0
        return True
    managed = (
        outcome.managed if outcome is not None and outcome.managed else request.managed
    )
    if not resolved:
        try:
            if not managed.is_running():
                managed.cleanup()
                _set_provider_pending_stop_request(state, None)
                state.cleanup_attempt_count = 0
                state.cleanup_next_at = 0.0
                return True
        except Exception as exc:
            detail = {
                **request.detail,
                "cleanup_clear_blocked": reason,
                "error": str(exc),
                "cleanup_deferred_to": "cleanup-failed-reconciler",
            }
        else:
            detail = {
                **request.detail,
                "cleanup_clear_blocked": reason,
                "cleanup_deferred_to": "cleanup-failed-reconciler",
            }
        if outcome is not None:
            detail["last_cleanup_status"] = outcome.status
            detail["last_cleanup_detail"] = outcome.detail
        _schedule_cleanup_failed_retry(
            state,
            request,
            _stop_cleanup_outcome(
                "cleanup-failed",
                "cleanup-attempt-failed",
                detail,
                managed,
            ),
        )
        return False
    _set_provider_pending_stop_request(state, None)
    state.cleanup_attempt_count = 0
    state.cleanup_next_at = 0.0
    return True


def _make_stop_request(
    state: ProviderRuntimeState,
    managed: RunnerManagedProcess,
    *,
    reason_code: ReasonCode,
    detail: dict[str, Any],
    target_phase: RuntimePhase = "stopped",
    target_reason_code: ReasonCode | None = "cleanup-succeeded",
    target_detail: dict[str, Any] | None = None,
    admission_exclusive: bool = False,
) -> ProviderStopCleanupRequest:
    return ProviderStopCleanupRequest(
        managed=managed,
        reason_code=reason_code,
        detail=detail,
        target_phase=target_phase,
        target_reason_code=target_reason_code,
        target_detail=target_detail or {},
        admission_exclusive=admission_exclusive,
        local_capacity=_local_stop_capacity(state) if admission_exclusive else None,
    )


def _provider_stop_cleanup_worker(
    provider: ProviderName,
    request: ProviderStopCleanupRequest,
    fence: ProviderFence,
    cancel_event: threading.Event,
) -> ProviderStopCleanupOutcome:
    del fence
    managed = request.managed
    permit = None
    try:
        if cancel_event.is_set():
            return _stop_cleanup_outcome(
                "cancelled",
                "stale-result-ignored",
                {"cancelled": True, "reason_code": request.reason_code},
                managed,
            )
        if request.admission_exclusive and provider == "local":
            from solstone.think.providers import local_admission

            try:
                permit = local_admission.acquire_local_slot(
                    request.local_capacity or 1,
                    PROVIDER_ADMISSION_STOP_TIMEOUT_S,
                    exclusive=True,
                    cancel_event=cancel_event,
                )
            except local_admission.LocalAdmissionTimeout as exc:
                return _stop_cleanup_outcome(
                    "stop-deferred",
                    "admission-exclusive-stop",
                    {
                        **request.detail,
                        "error": str(exc),
                        "capacity": request.local_capacity or 1,
                    },
                    managed,
                )
            except local_admission.LocalAdmissionCancelled:
                return _stop_cleanup_outcome(
                    "cancelled",
                    "stale-result-ignored",
                    {"cancelled": True, "reason_code": request.reason_code},
                    managed,
                )
        if cancel_event.is_set():
            return _stop_cleanup_outcome(
                "cancelled",
                "stale-result-ignored",
                {"cancelled": True, "reason_code": request.reason_code},
                managed,
            )
        if not managed.is_running():
            managed.cleanup()
            return _stop_cleanup_outcome(
                "stopped",
                "cleanup-succeeded",
                {**request.detail, "already_dead": True},
            )
        _terminate_cleanup_handle(managed, reason=str(request.reason_code))
        return _stop_cleanup_outcome(
            "stopped",
            "cleanup-succeeded",
            request.detail,
        )
    except Exception as exc:
        logger.exception("%s provider stop cleanup failed", provider)
        return _stop_cleanup_outcome(
            "cleanup-failed",
            "cleanup-attempt-failed",
            {**request.detail, "error": str(exc)},
            managed,
        )
    finally:
        if permit is not None:
            permit.release()


def _schedule_cleanup_failed_retry(
    state: ProviderRuntimeState,
    request: ProviderStopCleanupRequest,
    outcome: ProviderStopCleanupOutcome,
) -> None:
    _set_provider_pending_stop_request(
        state,
        ProviderStopCleanupRequest(
            managed=outcome.managed or request.managed,
            reason_code=request.reason_code,
            detail={**request.detail, "last_cleanup_detail": outcome.detail},
            target_phase=request.target_phase,
            target_reason_code=request.target_reason_code,
            target_detail=request.target_detail,
            admission_exclusive=request.admission_exclusive,
            local_capacity=request.local_capacity,
        ),
    )
    state.cleanup_attempt_count += 1
    delay = PROVIDER_CLEANUP_RETRY_SCHEDULE_SECONDS[
        min(
            state.cleanup_attempt_count - 1,
            len(PROVIDER_CLEANUP_RETRY_SCHEDULE_SECONDS) - 1,
        )
    ]
    state.cleanup_next_at = time.monotonic() + delay
    state.latest_phase = "cleanup-failed"
    _write_provider_runtime(
        state,
        phase="cleanup-failed",
        reason_code="cleanup-attempt-failed",
        detail={
            **outcome.detail,
            "cleanup_attempt": state.cleanup_attempt_count,
            "next_cleanup_attempt": state.cleanup_attempt_count + 1,
        },
        process=_current_provider_process_record(state.provider),
        display_deadline_delay_s=delay,
    )


def _adopt_provider_cleanup_failed_handle(
    state: ProviderRuntimeState,
    managed: RunnerManagedProcess,
    *,
    detail: dict[str, Any],
) -> None:
    request = _make_stop_request(
        state,
        managed,
        reason_code="cleanup-attempt-failed",
        detail=detail,
        target_phase="stopped",
        target_reason_code="cleanup-succeeded",
    )
    outcome = _stop_cleanup_outcome(
        "cleanup-failed",
        "cleanup-attempt-failed",
        detail,
        managed,
    )
    _schedule_cleanup_failed_retry(state, request, outcome)


def _duplicate_cleanup_request(
    state: ProviderRuntimeState,
    procs: list[RunnerManagedProcess],
) -> ProviderStopCleanupRequest | None:
    candidates = _provider_processes(state.provider, procs)
    if len(candidates) <= 1:
        return None
    keep: RunnerManagedProcess | None = None
    try:
        current = read_runtime_health(state.provider)
    except (RuntimeHealthMalformedError, RuntimeHealthUnavailableError):
        current = None
    if current is not None:
        matches = [
            managed
            for managed in candidates
            if _runtime_record_matches_managed(current, managed)
        ]
        if len(matches) == 1:
            keep = matches[0]
    stale = next((managed for managed in candidates if managed is not keep), None)
    if stale is None:
        return None
    return _make_stop_request(
        state,
        stale,
        reason_code="duplicate-owned-process",
        detail={"duplicates": len(candidates), "kept_ref": keep.ref if keep else None},
        target_phase=state.latest_phase,
        target_reason_code="duplicate-owned-process",
        target_detail={"duplicates": len(candidates)},
    )


def _stop_before_replace_request(
    state: ProviderRuntimeState,
    procs: list[RunnerManagedProcess],
) -> ProviderStopCleanupRequest | None:
    if state.latest_plan is None:
        return None
    if state.latest_phase not in {"starting", "backoff", "retry-requested", "stopped"}:
        return None
    managed = _provider_running_process(state.provider, procs)
    if managed is None:
        return None
    if state.provider == "local":
        _mark_provider_recovery_down("local")
    return _make_stop_request(
        state,
        managed,
        reason_code="target-changed",
        detail={"replacement_fingerprint": state.desired_fingerprint},
        target_phase="stopped",
        target_reason_code="cleanup-succeeded",
    )


def _deferred_stop_request(
    state: ProviderRuntimeState,
    procs: list[RunnerManagedProcess],
) -> ProviderStopCleanupRequest | None:
    managed = _provider_running_process(state.provider, procs)
    if managed is None:
        return None
    return _make_stop_request(
        state,
        managed,
        reason_code=(
            "admission-exclusive-stop"
            if state.pending_stop_admission_exclusive
            else "target-changed"
        ),
        detail={"target_phase": state.pending_stop_target_phase},
        target_phase=state.pending_stop_target_phase,
        target_reason_code=state.pending_stop_target_reason_code,
        target_detail=state.pending_stop_target_detail,
        admission_exclusive=state.pending_stop_admission_exclusive,
    )


def _submit_provider_stop_cleanup_if_needed(
    state: ProviderRuntimeState,
    procs: list[RunnerManagedProcess],
) -> bool:
    if state.stop_cleanup_future is not None:
        return True
    now = time.monotonic()
    request = state.pending_stop_request
    if request is not None:
        if state.cleanup_attempt_count > 0 and now < state.cleanup_next_at:
            return True
    elif state.latest_phase == "stop-deferred":
        request = _deferred_stop_request(state, procs)
        if request is None:
            state.latest_phase = state.pending_stop_target_phase
            _write_provider_runtime(
                state,
                phase=state.pending_stop_target_phase,
                reason_code=state.pending_stop_target_reason_code,
                detail=state.pending_stop_target_detail,
                process=None,
            )
            return False
    elif state.latest_phase == "cleanup-failed":
        if now < state.cleanup_next_at:
            return True
        return False
    else:
        request = _duplicate_cleanup_request(
            state, procs
        ) or _stop_before_replace_request(state, procs)
        if request is None:
            return False
    _set_provider_pending_stop_request(state, request)
    state.latest_phase = "stopping"
    fence = _provider_fence(state, state.retry.attempt_count)
    cancel_event = threading.Event()
    state.stop_cleanup_fence = fence
    state.stop_cleanup_cancel_event = cancel_event
    _write_provider_runtime(
        state,
        phase="stopping",
        reason_code=request.reason_code,
        detail={**request.detail, "fence": fence.__dict__},
        process=_current_provider_process_record(state.provider),
    )
    state.stop_cleanup_future = _provider_executor().submit(
        _provider_stop_cleanup_worker,
        state.provider,
        request,
        fence,
        cancel_event,
    )
    return True


def _handle_provider_stop_cleanup_result(
    state: ProviderRuntimeState,
    procs: list[RunnerManagedProcess],
) -> bool:
    future = state.stop_cleanup_future
    if future is None or not future.done():
        return False
    fence = state.stop_cleanup_fence
    request = state.pending_stop_request
    state.stop_cleanup_future = None
    state.stop_cleanup_fence = None
    state.stop_cleanup_cancel_event = None
    try:
        outcome = future.result()
    except Exception as exc:
        logger.exception("%s provider stop cleanup worker failed", state.provider)
        if request is None:
            return True
        outcome = _stop_cleanup_outcome(
            "cleanup-failed",
            "cleanup-attempt-failed",
            {"error": str(exc)},
            request.managed,
        )
    if request is None:
        return True
    if fence is not None and not _provider_fence_matches(state, fence):
        if outcome.status == "cleanup-failed" and outcome.managed is not None:
            _adopt_provider_cleanup_failed_handle(
                state,
                outcome.managed,
                detail={
                    **outcome.detail,
                    "stale_stop_fence": fence.__dict__,
                    "cleanup_deferred_to": "cleanup-failed-reconciler",
                },
            )
        return True
    if outcome.status == "cancelled":
        if _clear_provider_pending_stop_request(
            state,
            reason="provider stop cleanup cancelled",
            resolved=False,
            outcome=outcome,
        ):
            state.latest_phase = "observing"
        state.next_truth_at = 0.0
        return True
    if outcome.status == "stop-deferred":
        _set_provider_pending_stop_request(
            state,
            ProviderStopCleanupRequest(
                managed=outcome.managed or request.managed,
                reason_code=request.reason_code,
                detail=outcome.detail,
                target_phase=request.target_phase,
                target_reason_code=request.target_reason_code,
                target_detail=request.target_detail,
                admission_exclusive=request.admission_exclusive,
                local_capacity=request.local_capacity,
            ),
        )
        state.latest_phase = "stop-deferred"
        _write_provider_runtime(
            state,
            phase="stop-deferred",
            reason_code=outcome.reason_code,
            detail=outcome.detail,
            process=_current_provider_process_record(state.provider),
        )
        return True
    if outcome.status == "cleanup-failed":
        _schedule_cleanup_failed_retry(state, request, outcome)
        return True
    cleanup_outcome = _outcome(
        "launch-failed",
        "cleanup-succeeded",
        outcome.detail,
        outcome.managed,
    )
    _clear_provider_port_if_owner_matches(state, fence, cleanup_outcome)
    _remove_provider_process(procs, request.managed)
    _clear_provider_pending_stop_request(
        state,
        reason="provider stop cleanup succeeded",
        resolved=True,
        outcome=outcome,
    )
    state.latest_phase = request.target_phase
    process = None
    if request.target_phase in {"ready", "ready-proof-unavailable"}:
        try:
            process = read_runtime_health(state.provider)["process"]
        except (RuntimeHealthMalformedError, RuntimeHealthUnavailableError):
            process = None
    _write_provider_runtime(
        state,
        phase=request.target_phase,
        reason_code=request.target_reason_code,
        detail=request.target_detail,
        process=process,
    )
    return True


def _finish_provider_startup_condition(
    state: ProviderRuntimeState, phase: RuntimePhase
) -> None:
    if phase == "ready":
        now = time.monotonic()
        if state.next_probe_at <= now:
            state.next_probe_at = now + PROVIDER_PROBE_INTERVAL_SECONDS
        _consume_local_recovery_nudge(state)
    if phase in _PROVIDER_STARTUP_TERMINAL_PHASES:
        state.startup_terminal = True
        gate = _provider_startup_gate
        if gate is not None and state.provider in gate.required:
            gate.terminal.add(state.provider)


def _provider_startup_gate_in_flight_providers(
    gate: ProviderStartupGate,
) -> list[ProviderName]:
    providers: list[ProviderName] = []
    for provider in gate.required:
        future = _provider_runtime_states[provider].start_future
        if future is not None and not future.done():
            providers.append(provider)
    return sorted(providers)


def _provider_startup_gate_attempted_outcomes(gate: ProviderStartupGate) -> list[str]:
    return sorted(
        f"{provider}={gate.attempted[provider]}"
        for provider in gate.required
        if provider in gate.attempted
    )


def _release_provider_startup_gate_if_ready() -> None:
    gate = _provider_startup_gate
    if gate is None or gate.released or _task_queue is None:
        return
    attempted_names = set(gate.attempted.keys())
    satisfied = gate.terminal | attempted_names
    if gate.required.issubset(satisfied):
        gate.released = True
        _task_queue.set_ready()
        if not gate.required - gate.terminal:
            logger.info("provider startup gate released after terminal provider state")
        else:
            logger.info(
                "provider startup gate released after first launch attempts concluded: "
                "outcomes=%s terminal_providers=%s",
                _provider_startup_gate_attempted_outcomes(gate),
                sorted(gate.required & gate.terminal),
            )
        return
    if gate.first_start_at is not None:
        elapsed_since_first_start = time.monotonic() - gate.first_start_at
        if elapsed_since_first_start >= PROVIDER_STARTUP_GATE_CEILING_SECONDS:
            gate.released = True
            _task_queue.set_ready()
            logger.warning(
                "provider startup gate released after %.1fs provider launch ceiling "
                "with unsatisfied providers: %s; in_flight providers: %s; attempted "
                "outcomes: %s",
                PROVIDER_STARTUP_GATE_CEILING_SECONDS,
                sorted(gate.required - satisfied),
                _provider_startup_gate_in_flight_providers(gate),
                _provider_startup_gate_attempted_outcomes(gate),
            )
            return
    elapsed = time.monotonic() - gate.started_at
    if elapsed >= PROVIDER_STARTUP_GATE_WINDOW_SECONDS:
        if _provider_startup_gate_in_flight_providers(gate):
            return
        gate.released = True
        _task_queue.set_ready()
        logger.warning(
            "provider startup gate released after %.1fs window with pending providers: "
            "%s; launch_in_flight=false",
            PROVIDER_STARTUP_GATE_WINDOW_SECONDS,
            sorted(gate.required - satisfied),
        )


def _initialize_provider_startup_gate() -> None:
    global _provider_startup_gate
    required: set[ProviderName] = set()
    if not _is_remote_mode:
        try:
            config = read_journal_config()
            from solstone.think.providers.local_endpoint import resolve_local_endpoint

            if is_local_provider_needed(config) and resolve_local_endpoint().is_bundled:
                required.add("local")
        except Exception:
            logger.exception("could not determine local provider startup gate intent")
            required.add("local")
        if sys.platform.startswith("linux"):
            required.add("parakeet")
    _provider_startup_gate = ProviderStartupGate(
        started_at=time.monotonic(),
        required=required,
        terminal=set(),
        attempted={},
        first_start_at=None,
        released=False,
    )
    if not required:
        _release_provider_startup_gate_if_ready()


def _observe_provider_truth(provider: ProviderName) -> ProviderTruthObservation:
    if provider == "local":
        return _observe_local_provider_truth()
    return _observe_parakeet_provider_truth()


def _defer_provider_stop_for_observation(
    state: ProviderRuntimeState,
    observation: ProviderTruthObservation,
    *,
    reason_code: ReasonCode,
    admission_exclusive: bool,
) -> None:
    target_detail = {
        "target_phase": observation.phase,
        "target_reason_code": observation.reason_code,
        "target_detail": observation.detail,
        "admission_exclusive": admission_exclusive,
    }
    request = state.pending_stop_request
    if request is not None:
        _set_provider_pending_stop_request(
            state,
            ProviderStopCleanupRequest(
                managed=request.managed,
                reason_code=request.reason_code,
                detail=request.detail,
                target_phase="stop-deferred",
                target_reason_code=reason_code,
                target_detail=target_detail,
                admission_exclusive=request.admission_exclusive,
                local_capacity=request.local_capacity,
            ),
        )
    state.pending_stop_target_phase = observation.phase
    state.pending_stop_target_reason_code = observation.reason_code
    state.pending_stop_target_detail = observation.detail
    state.pending_stop_admission_exclusive = admission_exclusive
    if request is None:
        state.cleanup_attempt_count = 0
        state.cleanup_next_at = 0.0
    state.latest_phase = "stop-deferred"
    state.boot_required = observation.boot_required
    if state.provider == "local" and reason_code == "target-changed":
        _mark_provider_recovery_down("local")
    _write_provider_runtime(
        state,
        phase="stop-deferred",
        reason_code=reason_code,
        detail=target_detail,
        process=_current_provider_process_record(state.provider),
    )


def _submit_provider_truth_if_needed(state: ProviderRuntimeState) -> None:
    if state.truth_future is not None:
        return
    now = time.monotonic()
    if now < state.next_truth_at:
        return
    if (
        state.latest_phase in {"backoff", "retry-requested", "starting"}
        and state.latest_plan is not None
        and state.retry.attempt_count < len(PROVIDER_RETRY_SCHEDULE_SECONDS)
        and now >= state.retry.next_at
    ):
        return
    state.next_truth_at = now + PROVIDER_TRUTH_OBSERVATION_INTERVAL_SECONDS
    fence = _provider_fence(state, state.retry.attempt_count)
    state.truth_fence = fence
    # A truth refresh is speculative until its fenced result is accepted. Keep
    # an already-established live or cleanup transition authoritative while the
    # observation runs; replacing it with ``observing`` loses the context used
    # to distinguish a stable target from a replacement and can hide ownership
    # of a process that still needs cleanup.
    if (
        state.latest_phase not in _PROVIDER_TRUTH_PRESERVED_PHASES
        and state.pending_stop_request is None
        and state.stop_cleanup_future is None
    ):
        state.latest_phase = "observing"
        _write_provider_runtime(
            state,
            phase="observing",
            reason_code="truth-observation-started",
            detail={"fence": fence.__dict__},
        )
    state.truth_future = _provider_executor().submit(
        _observe_provider_truth,
        state.provider,
    )


def _maybe_start_parakeet_bootstrap(
    state: ProviderRuntimeState,
    observation: ProviderTruthObservation,
) -> None:
    if state.provider != "parakeet":
        return
    if state.parakeet_bootstrap_future is not None:
        if not state.parakeet_bootstrap_future.done():
            return
        fingerprint = state.parakeet_bootstrap_fingerprint
        future = state.parakeet_bootstrap_future
        state.parakeet_bootstrap_future = None
        state.parakeet_bootstrap_fingerprint = None
        try:
            started = bool(future.result())
        except Exception as exc:
            logging.warning("parakeet.cpp provider bootstrap helper failed: %s", exc)
            started = False
        if started:
            state.parakeet_bootstrap_requested_fingerprint = fingerprint
    if observation.phase != "artifact-not-ready":
        state.parakeet_bootstrap_requested_fingerprint = None
        return
    if observation.detail.get("install_acquisition_allowed") is not True:
        return
    fingerprint = observation.desired_fingerprint_sha256
    if state.parakeet_bootstrap_requested_fingerprint == fingerprint:
        return
    fence = _provider_fence(state, state.retry.attempt_count)
    future = _provider_executor().submit(
        _start_parakeet_bootstrap_if_needed,
        str(observation.reason_code or "artifact-not-ready"),
        fence,
        fingerprint,
    )
    state.parakeet_bootstrap_future = future
    state.parakeet_bootstrap_fingerprint = fingerprint
    if future.done():
        state.parakeet_bootstrap_future = None
        state.parakeet_bootstrap_fingerprint = None
        try:
            if bool(future.result()):
                state.parakeet_bootstrap_requested_fingerprint = fingerprint
        except Exception as exc:
            logging.warning("parakeet.cpp provider bootstrap helper failed: %s", exc)


def _handle_provider_truth_result(state: ProviderRuntimeState) -> bool:
    future = state.truth_future
    if future is None or not future.done():
        return False
    fence = state.truth_fence
    state.truth_future = None
    state.truth_fence = None
    try:
        observation = future.result()
    except Exception as exc:
        logger.exception("%s provider truth worker failed", state.provider)
        observation = ProviderTruthObservation(
            provider=state.provider,
            phase="state-unavailable",
            reason_code="truth-observation-failed",
            detail={"error": str(exc)},
        )
    if fence is not None and not _provider_fence_matches(state, fence):
        state.next_truth_at = 0.0
        _write_provider_runtime(
            state,
            phase=state.latest_phase,
            reason_code="stale-result-ignored",
            detail={"slot": "truth", "fence": fence.__dict__},
            process=_current_provider_process_record(state.provider),
        )
        return True
    if observation.provider != state.provider:
        logger.error(
            "provider truth worker returned %s for %s",
            observation.provider,
            state.provider,
        )
        return True
    if observation.phase != "artifact-not-ready":
        state.replacement_artifact_not_ready_fingerprint = None
    fingerprint_changed = (
        observation.desired_fingerprint_sha256 != state.desired_fingerprint
    )
    pending_stop_target_phase = (
        state.pending_stop_request.target_phase
        if state.pending_stop_request is not None
        else state.pending_stop_target_phase
    )
    if (
        not fingerprint_changed
        and state.latest_phase in {"stop-deferred", "stopping", "cleanup-failed"}
        and observation.phase == pending_stop_target_phase
    ):
        _write_provider_runtime(
            state,
            phase=state.latest_phase,
            reason_code="stale-result-ignored",
            detail={
                "slot": "truth",
                "pending_cleanup": True,
                "latched_phase": observation.phase,
                "latched_reason_code": observation.reason_code,
            },
            process=_current_provider_process_record(state.provider),
        )
        return True
    if (
        not fingerprint_changed
        and (
            (
                observation.phase == "starting"
                and state.latest_phase
                in {"starting", "ready", "ready-proof-unavailable"}
            )
            or (
                observation.phase == "host-blocked" and state.latest_phase == "starting"
            )
        )
        and state.latest_plan is not None
    ):
        _write_provider_runtime(
            state,
            phase=state.latest_phase,
            reason_code="stale-result-ignored",
            detail={
                "slot": "truth",
                "latched_phase": observation.phase,
                "latched_reason_code": observation.reason_code,
            },
            process=_current_provider_process_record(state.provider),
        )
        return True
    if state.stop_cleanup_future is not None and (
        fingerprint_changed or observation.phase == "starting"
    ):
        _signal_provider_stop_cancel(
            state,
            reason="provider desired state changed during stop",
        )
    if state.start_future is not None:
        if fingerprint_changed:
            _signal_provider_start_cancel(
                state,
                reason="provider desired fingerprint changed",
            )
        elif observation.phase in _PROVIDER_START_CANCEL_PHASES:
            _signal_provider_start_cancel(
                state,
                reason=f"provider became {observation.phase}",
            )
    if (
        fingerprint_changed
        and observation.phase == "starting"
        and state.latest_phase in {"ready", "ready-proof-unavailable"}
    ):
        state.generation += 1
        state.desired_fingerprint = observation.desired_fingerprint_sha256
        state.retry = ProviderRetryState(
            desired_fingerprint=observation.desired_fingerprint_sha256
        )
        state.latest_plan = observation.plan
        _defer_provider_stop_for_observation(
            state,
            observation,
            reason_code="target-changed",
            admission_exclusive=False,
        )
        return True
    if (
        fingerprint_changed
        and observation.phase == "artifact-not-ready"
        and state.latest_phase in {"ready", "ready-proof-unavailable"}
    ):
        state.replacement_artifact_not_ready_fingerprint = (
            observation.desired_fingerprint_sha256
        )
        _write_provider_runtime(
            state,
            phase=observation.phase,
            reason_code=observation.reason_code,
            detail=observation.detail,
            process=_current_provider_process_record(state.provider),
        )
        _finish_provider_startup_condition(state, observation.phase)
        return True
    if (
        not fingerprint_changed
        and observation.phase == "host-blocked"
        and observation.reason_code in ADMISSION_ONLY_REASON_CODES
        and state.latest_phase in {"ready", "ready-proof-unavailable"}
    ):
        # Available-RAM headroom is an admission gate, not a liveness signal: the
        # running provider's own resident footprint has already been subtracted
        # from the reading the floor is compared against. Re-applying it here would
        # make a successful model load the thing that evicts the model.
        _write_provider_runtime(
            state,
            phase=state.latest_phase,
            reason_code="stale-result-ignored",
            detail={
                "slot": "truth",
                "latched_phase": observation.phase,
                "latched_reason_code": observation.reason_code,
            },
            process=_current_provider_process_record(state.provider),
        )
        return True
    if observation.phase in {"not-desired", "host-blocked"} and state.latest_phase in {
        "ready",
        "ready-proof-unavailable",
    }:
        if fingerprint_changed:
            state.generation += 1
            state.desired_fingerprint = observation.desired_fingerprint_sha256
            state.retry = ProviderRetryState(
                desired_fingerprint=observation.desired_fingerprint_sha256
            )
        _defer_provider_stop_for_observation(
            state,
            observation,
            reason_code="admission-exclusive-stop",
            admission_exclusive=True,
        )
        return True
    if fingerprint_changed:
        state.generation += 1
        state.desired_fingerprint = observation.desired_fingerprint_sha256
        state.retry = ProviderRetryState(
            desired_fingerprint=observation.desired_fingerprint_sha256
        )
    state.latest_plan = observation.plan
    state.latest_phase = observation.phase
    state.boot_required = observation.boot_required
    _write_provider_runtime(
        state,
        phase=observation.phase,
        reason_code=observation.reason_code,
        detail=observation.detail,
    )
    _maybe_start_parakeet_bootstrap(state, observation)
    if (
        observation.phase == "observing"
        and observation.reason_code == "observation-raced"
    ):
        state.next_truth_at = 0.0
    _finish_provider_startup_condition(state, observation.phase)
    return True


def _provider_start_worker(
    provider: ProviderName,
    plan: LocalServerLaunchPlan | ParakeetServerLaunchPlan,
    fence: ProviderFence,
    cancel_event: threading.Event,
) -> ProviderLaunchOutcome:
    reservation = ReservedPort.reserve()
    logger.debug(
        "provider start worker reserved port %s for %s generation=%s attempt=%s",
        reservation.port,
        provider,
        fence.generation,
        fence.attempt,
    )
    if provider == "local":
        return start_local_server(
            cast(LocalServerLaunchPlan, plan),
            reservation,
            cancel_event,
        )
    return start_parakeet_server(
        cast(ParakeetServerLaunchPlan, plan),
        reservation,
        cancel_event,
    )


def _submit_provider_start_if_needed(
    state: ProviderRuntimeState, procs: list[RunnerManagedProcess]
) -> None:
    if (
        state.pending_stop_request is not None
        or state.stop_cleanup_future is not None
        or state.latest_phase
        in {
            "stop-deferred",
            "stopping",
            "cleanup-failed",
        }
    ):
        return
    if state.start_future is not None:
        return
    if state.latest_phase not in {"starting", "backoff", "retry-requested", "stopped"}:
        return
    if state.latest_plan is None:
        return
    if _provider_running_process(state.provider, procs) is not None:
        _write_provider_runtime(
            state,
            phase="ready",
            reason_code="ready-existing-owned-process",
            detail={"source": "process-list"},
        )
        state.latest_phase = "ready"
        _finish_provider_startup_condition(state, "ready")
        return
    if state.retry.attempt_count >= len(PROVIDER_RETRY_SCHEDULE_SECONDS):
        state.latest_phase = "failed"
        _write_provider_runtime(
            state,
            phase="failed",
            reason_code="launch-budget-exhausted",
            detail={"attempts": state.retry.attempt_count},
        )
        _finish_provider_startup_condition(state, "failed")
        return
    now = time.monotonic()
    if now < state.retry.next_at:
        return
    attempt = state.retry.attempt_count + 1
    state.retry.attempt_count = attempt
    fence = _provider_fence(state, attempt)
    cancel_event = threading.Event()
    state.start_fence = fence
    state.start_cancel_event = cancel_event
    state.latest_phase = "starting"
    _write_provider_runtime(
        state,
        phase="starting",
        reason_code="launch-requested",
        detail={"attempt": attempt, "fence": fence.__dict__},
        attempt=attempt,
    )
    state.start_future = _provider_executor().submit(
        _provider_start_worker,
        state.provider,
        state.latest_plan,
        fence,
        cancel_event,
    )
    gate = _provider_startup_gate
    if (
        gate is not None
        and state.provider in gate.required
        and gate.first_start_at is None
    ):
        gate.first_start_at = time.monotonic()


def _handle_provider_start_result(
    state: ProviderRuntimeState, procs: list[RunnerManagedProcess]
) -> bool:
    future = state.start_future
    if future is None or not future.done():
        return False
    fence = state.start_fence
    cancel_event = state.start_cancel_event
    state.start_future = None
    state.start_fence = None
    state.start_cancel_event = None
    try:
        outcome = future.result()
    except Exception as exc:
        logger.exception("%s provider start worker failed", state.provider)
        outcome = _outcome(
            "launch-failed",
            "launch-failed",
            {"error": str(exc)},
        )
    if fence is not None and not _provider_fence_matches(state, fence):
        if not _cleanup_provider_outcome_handle(
            state,
            fence,
            outcome,
            reason="provider launch result superseded",
        ):
            return True
        _write_provider_runtime(
            state,
            phase=state.latest_phase,
            reason_code="stale-result-ignored",
            detail={"slot": "start", "fence": fence.__dict__},
        )
        return True
    gate = _provider_startup_gate
    if (
        gate is not None
        and state.provider in gate.required
        and state.provider not in gate.attempted
    ):
        gate.attempted[state.provider] = outcome.status
    if (
        cancel_event is not None
        and cancel_event.is_set()
        and outcome.status == "ready"
        and outcome.managed is not None
    ):
        if not _cleanup_provider_outcome_handle(
            state,
            fence,
            outcome,
            reason="provider launch cancelled before ready publication",
        ):
            return True
        outcome = _outcome(
            "launch-failed",
            "launch-failed",
            {
                "last_outcome": outcome.status,
                "last_detail": outcome.detail,
                "cancelled": True,
            },
        )

    if outcome.status == "ready" and outcome.managed is not None:
        port = outcome.detail.get("port")
        if not isinstance(port, int):
            if not _cleanup_provider_outcome_handle(
                state,
                fence,
                outcome,
                reason="provider ready outcome missing port",
            ):
                return True
            outcome = _outcome(
                "launch-failed",
                "launch-failed",
                {
                    "last_outcome": outcome.status,
                    "last_detail": outcome.detail,
                    "error": "ready outcome missing port",
                },
            )
        else:
            process = _provider_process_record(outcome.managed, port=port)
            ownership = _write_provider_runtime(
                state,
                phase="ready",
                reason_code=outcome.reason_code,
                detail=outcome.detail,
                process=process,
            )
            if ownership is None:
                state.latest_phase = "state-unavailable"
                if not _cleanup_provider_outcome_handle(
                    state,
                    fence,
                    outcome,
                    reason="provider ready ownership publication failed",
                ):
                    return True
                _write_provider_runtime(
                    state,
                    phase="state-unavailable",
                    reason_code="record-unavailable",
                    detail={"error": "runtime ownership write failed", "port": port},
                    process=None,
                )
                _finish_provider_startup_condition(state, "state-unavailable")
                return True
            try:
                _write_provider_ready_side_effects(state, outcome)
                _publish_provider_port(state.provider, port=port)
            except Exception as exc:
                if not _cleanup_provider_outcome_handle(
                    state,
                    fence,
                    outcome,
                    reason="provider ready port publication failed",
                ):
                    return True
                state.latest_phase = "state-unavailable"
                _write_provider_runtime(
                    state,
                    phase="state-unavailable",
                    reason_code="record-unavailable",
                    detail={"error": str(exc), "port": port},
                    process=None,
                )
                _finish_provider_startup_condition(state, "state-unavailable")
                return True
            if outcome.managed not in procs:
                procs.append(outcome.managed)
            state.latest_phase = "ready"
            _finish_provider_startup_condition(state, "ready")
            return True

    if outcome.managed is not None:
        if outcome.detail.get("cleanup_deferred_to") == "cleanup-failed-reconciler":
            _adopt_provider_cleanup_failed_handle(
                state,
                outcome.managed,
                detail=outcome.detail,
            )
            return True
        if not _cleanup_provider_outcome_handle(
            state,
            fence,
            outcome,
            reason=f"provider launch outcome {outcome.status}",
        ):
            return True

    if state.retry.attempt_count >= len(PROVIDER_RETRY_SCHEDULE_SECONDS):
        state.latest_phase = "failed"
        _write_provider_runtime(
            state,
            phase="failed",
            reason_code="launch-budget-exhausted",
            detail={
                "last_outcome": outcome.status,
                "last_reason_code": outcome.reason_code,
                "last_detail": outcome.detail,
                "attempts": state.retry.attempt_count,
            },
            process=None,
        )
        _finish_provider_startup_condition(state, "failed")
        return True

    delay = PROVIDER_RETRY_SCHEDULE_SECONDS[state.retry.attempt_count]
    state.retry.next_at = time.monotonic() + delay
    state.latest_phase = "backoff"
    _write_provider_runtime(
        state,
        phase="backoff",
        reason_code="retry-scheduled",
        detail={
            "last_outcome": outcome.status,
            "last_reason_code": outcome.reason_code,
            "last_detail": outcome.detail,
            "next_attempt": state.retry.attempt_count + 1,
        },
        process=None,
        display_deadline_delay_s=delay,
    )
    return True


def _probe_outcome(
    status: ProbeStatus,
    reason_code: ReasonCode,
    detail: dict[str, Any],
) -> ProviderProbeOutcome:
    return ProviderProbeOutcome(
        status=status,
        reason_code=reason_code,
        detail=detail,
    )


def _provider_probe_worker(
    provider: ProviderName,
    port: int,
    fence: ProviderFence,
) -> ProviderProbeOutcome:
    del fence
    try:
        if provider == "local":
            from solstone.think.providers import local_server

            state, error = local_server._probe_health(port)
            if state == local_server.STATE_READY:
                return _probe_outcome(
                    "ready",
                    "probe-ready",
                    {"port": port, "health_state": state},
                )
            status: ProbeStatus = (
                "not-ready" if state == local_server.STATE_LOADING else "unavailable"
            )
            return _probe_outcome(
                status,
                "proof-observation-unavailable",
                {"port": port, "health_state": state, "error": error},
            )

        from solstone.think.providers import parakeet_server

        state, error = parakeet_server._probe_health(port)
        if state == parakeet_server.STATE_READY:
            return _probe_outcome(
                "ready",
                "probe-ready",
                {"port": port, "health_state": state},
            )
        return _probe_outcome(
            "unavailable",
            "proof-observation-unavailable",
            {"port": port, "health_state": state, "error": error},
        )
    except Exception as exc:
        logger.exception("%s provider probe worker failed", provider)
        return _probe_outcome(
            "unavailable",
            "proof-observation-unavailable",
            {"port": port, "error": str(exc)},
        )


def _handle_provider_probe_result(state: ProviderRuntimeState) -> bool:
    future = state.probe_future
    if future is None or not future.done():
        return False
    fence = state.probe_fence
    state.probe_future = None
    state.probe_fence = None
    try:
        outcome = future.result()
    except Exception as exc:
        logger.exception("%s provider probe future failed", state.provider)
        outcome = _probe_outcome(
            "unavailable",
            "proof-observation-unavailable",
            {"error": str(exc)},
        )
    if (
        state.replacement_artifact_not_ready_fingerprint is not None
        and outcome.status == "ready"
    ):
        # Keep the replacement artifact failure visible while the old child is
        # healthy. Probe submissions still run, and unhealthy probe outcomes
        # flow through below; exited children are surfaced by handle_runner_exits().
        state.next_probe_at = time.monotonic() + PROVIDER_PROBE_INTERVAL_SECONDS
        return True
    if fence is not None and not _provider_fence_matches(state, fence):
        _write_provider_runtime(
            state,
            phase=state.latest_phase,
            reason_code="stale-result-ignored",
            detail={"slot": "probe", "fence": fence.__dict__},
            process=_current_provider_process_record(state.provider),
        )
        return True

    process = _current_provider_process_record(state.provider)
    state.next_probe_at = time.monotonic() + PROVIDER_PROBE_INTERVAL_SECONDS
    if state.pending_stop_request is not None:
        _write_provider_runtime(
            state,
            phase=state.latest_phase,
            reason_code="stale-result-ignored",
            detail={
                "slot": "probe",
                "pending_cleanup": True,
                "probe_status": outcome.status,
                "probe_detail": outcome.detail,
            },
            process=process,
        )
        return True
    if outcome.status == "ready":
        state.latest_phase = "ready"
        _write_provider_runtime(
            state,
            phase="ready",
            reason_code=outcome.reason_code,
            detail=outcome.detail,
            process=process,
        )
        _finish_provider_startup_condition(state, "ready")
        return True

    state.latest_phase = "ready-proof-unavailable"
    _write_provider_runtime(
        state,
        phase="ready-proof-unavailable",
        reason_code=outcome.reason_code,
        detail=outcome.detail,
        process=process,
    )
    _finish_provider_startup_condition(state, "ready-proof-unavailable")
    return True


def _submit_provider_probe_if_needed(state: ProviderRuntimeState) -> None:
    if state.probe_future is not None:
        return
    if state.latest_phase not in {"ready", "ready-proof-unavailable"}:
        return
    now = time.monotonic()
    if now < state.next_probe_at:
        return
    port = read_service_port(_PROVIDER_PORT_SERVICES[state.provider])
    if port is None:
        state.latest_phase = "ready-proof-unavailable"
        state.next_probe_at = now + PROVIDER_PROBE_INTERVAL_SECONDS
        _write_provider_runtime(
            state,
            phase="ready-proof-unavailable",
            reason_code="proof-observation-unavailable",
            detail={"error": "service port unavailable"},
            process=_current_provider_process_record(state.provider),
        )
        _finish_provider_startup_condition(state, "ready-proof-unavailable")
        return
    fence = _provider_fence(state, state.retry.attempt_count)
    state.probe_fence = fence
    state.probe_future = _provider_executor().submit(
        _provider_probe_worker,
        state.provider,
        port,
        fence,
    )


def _handle_provider_retry_token(state: ProviderRuntimeState) -> None:
    global _parakeet_admission_retry_epoch
    try:
        token = read_retry_token(state.provider)
    except RuntimeHealthMalformedError:
        state.latest_phase = "state-corrupt"
        _finish_provider_startup_condition(state, "state-corrupt")
        return
    except RuntimeHealthUnavailableError:
        state.latest_phase = "state-unavailable"
        _finish_provider_startup_condition(state, "state-unavailable")
        return
    token_id = token["token_id"]
    if token_id is None:
        return
    if token["desired_fingerprint_sha256"] not in {None, state.desired_fingerprint}:
        return
    if state.latest_phase in {
        "not-desired",
        "artifact-not-ready",
        "host-blocked",
        "stopped",
        "failed",
        "backoff",
        "observing",
    }:
        retry_phase: RuntimePhase = "observing"
    else:
        retry_phase = "retry-requested"

    # Publish the nonterminal transition before clearing the token. Both writes
    # use the provider operation lock, so an owner retry can never observe a
    # cleared token while the durable health record still accepts another
    # terminal-failure request.
    state.latest_phase = retry_phase
    if (
        _write_provider_runtime(
            state,
            phase=retry_phase,
            reason_code="retry-token-requested",
            detail={"token_revision": token["revision"]},
        )
        is None
    ):
        return
    try:
        consume_retry_token(
            state.provider,
            token_id=token_id,
            revision=token["revision"],
            desired_fingerprint_sha256=token["desired_fingerprint_sha256"],
        )
    except RuntimeHealthConflictError:
        return
    except RuntimeHealthUnavailableError:
        state.latest_phase = "state-unavailable"
        _finish_provider_startup_condition(state, "state-unavailable")
        return
    if state.provider == "parakeet":
        _parakeet_admission_retry_epoch += 1
    state.retry = ProviderRetryState(desired_fingerprint=state.desired_fingerprint)
    state.latest_phase = retry_phase
    state.next_truth_at = 0.0


async def _reconcile_provider_runtime(
    provider: ProviderName, procs: list[RunnerManagedProcess]
) -> None:
    state = _provider_runtime_states[provider]
    _handle_provider_retry_token(state)
    _handle_provider_truth_result(state)
    _handle_provider_start_result(state, procs)
    stop_result_handled = _handle_provider_stop_cleanup_result(state, procs)
    _handle_provider_probe_result(state)
    if not stop_result_handled:
        _submit_provider_stop_cleanup_if_needed(state, procs)
        _submit_provider_start_if_needed(state, procs)
    _submit_provider_probe_if_needed(state)
    _submit_provider_truth_if_needed(state)


async def _reconcile_local_provider_runtime(
    procs: list[RunnerManagedProcess],
) -> None:
    await _reconcile_provider_runtime("local", procs)


async def _reconcile_parakeet_provider_runtime(
    procs: list[RunnerManagedProcess],
) -> None:
    await _reconcile_provider_runtime("parakeet", procs)


def _request_provider_runtime_recycle(
    provider: ProviderName,
    *,
    reason_code: ReasonCode,
    detail: dict[str, Any],
) -> bool:
    state = _provider_runtime_states[provider]
    try:
        token = request_retry_token(
            provider,
            desired_fingerprint_sha256=state.desired_fingerprint,
            reason_code=reason_code,
            owner={
                "module": "solstone.think.supervisor",
                "source": "provider-runtime-recycle",
            },
        )
    except RuntimeHealthMalformedError as exc:
        state.latest_phase = "state-corrupt"
        logger.error("%s recycle retry-token record is corrupt: %s", provider, exc)
        return False
    except RuntimeHealthUnavailableError as exc:
        state.latest_phase = "state-unavailable"
        logger.error("%s recycle retry-token record is unavailable: %s", provider, exc)
        return False

    state.generation += 1
    state.retry = ProviderRetryState(desired_fingerprint=state.desired_fingerprint)
    state.latest_phase = "retry-requested"
    state.next_truth_at = 0.0
    state.next_probe_at = 0.0
    if provider == "local":
        _mark_provider_recovery_down("local")
    _signal_provider_start_cancel(
        state,
        reason=f"{provider} provider recycle requested",
    )
    _write_provider_runtime(
        state,
        phase="retry-requested",
        reason_code=reason_code,
        detail={**detail, "token_revision": token["revision"]},
    )
    return True


def run_catchup_drain(
    force_days: Iterable[str] | None = None,
    *,
    exclude: set[str] | None = None,
) -> list[str]:
    """Submit catchup daily think tasks for pending, eligible days."""
    if no_thinking_engine_chosen():
        logging.info("No thinking engine selected; catchup drain held")
        return []

    all_updated = updated_days(exclude=exclude)

    def _eligible(day: str) -> bool:
        try:
            return day_eligible_to_drain(
                day, KIND_DAILY_CATCHUP
            ) and day_eligible_to_drain(day, KIND_SEGMENT_REPAIR)
        except Exception:
            logging.warning(
                "Catchup eligibility check failed for %s; treating as eligible",
                day,
            )
            return True

    eligible_natural = [day for day in all_updated if _eligible(day)]
    # AC3: force uses the same backoff gate. AC8: cap natural days after
    # eligibility; forced eligible days keep importer single-day intent.
    freshest = eligible_natural[-MAX_UPDATED_CATCHUP:]
    forced_eligible = [day for day in (force_days or []) if _eligible(day)]
    merged = set(freshest) | set(forced_eligible)
    if not merged:
        logging.info("no eligible days to process")
        return []

    if _task_queue is None:
        logging.warning("No task queue available for catchup drain: %s", sorted(merged))
        return []

    days = sorted(merged)
    logging.info("Queuing catchup drain for %d day(s): %s", len(days), days)
    for day_str in days:
        cmd = ["journal", "think", "-v", "--day", day_str]
        _task_queue.submit(cmd, day=day_str)
    return days


def _startup_catchup_drain() -> None:
    try:
        transitions = reconcile_interrupted_attempts()
        for transition in transitions:
            _emit_catchup_backoff(
                _supervisor_callosum,
                day=transition.day,
                attempts=transition.attempts,
                consecutive=transition.consecutive_non_completion,
                last_outcome=transition.last_outcome,
            )
    except Exception:
        logging.warning("Catchup reconciliation failed", exc_info=True)
    run_catchup_drain()


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
        run_catchup_drain(exclude={today_str})


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

    if load_processing_settings().mode == "deferred":
        logging.info(
            "Deferred mode: live segment %s/%s held for catchup drain; no live think",
            day,
            segment,
        )
        return

    if no_thinking_engine_chosen():
        logging.info(
            "No thinking engine selected: live segment %s/%s held; no live think",
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

    if load_processing_settings().mode == "deferred":
        return

    if no_thinking_engine_chosen():
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
    _handle_supervisor_drain(message)
    _handle_segment_observed(message)
    _handle_activity_recorded(message)
    _handle_think_daily_complete(message)
    _handle_segment_event_log(message)
    _handle_cortex_outcome(message)


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


def _run_gate_tick(now: float) -> None:
    global _last_gate_tick

    if now - _last_gate_tick < GATE_TICK_INTERVAL_S:
        return
    _last_gate_tick = now
    if _is_remote_mode:
        return
    settings = load_processing_settings()
    if settings.mode != "deferred":
        return
    reading = (
        poll_display_powersave(time.monotonic())
        if settings.gate.display_powersave.enabled
        else DISPLAY_POWERSAVE_UNAVAILABLE
    )
    gate = evaluate_drain_gate(settings, datetime.now(), reading)
    if not gate.open:
        return
    run_catchup_drain()


def _read_catchup_retry_expiries() -> list[float]:
    """Read the wall-clock times at which backed-off days become drain-eligible.

    Mirrors run_catchup_drain's gate, which requires a day to be drainable for
    both the daily-catchup and segment-repair kinds, so a day's eligibility time
    is the later of its two retry times. A day with an active record can never
    become eligible by expiry alone, and a day with no future retry time is
    already eligible; neither contributes a crossing.

    This reads catchup-state.json and nothing else -- in particular it never
    reaches read_raw_input_fingerprint, whose per-segment hashing is the IO tax
    this cheap gate exists to avoid.
    """
    entries = read_catchup_state()["entries"]
    blocked: set[str] = set()
    retries: dict[str, float] = {}

    for key, record in entries.items():
        day, _, kind = key.rpartition(":")
        if kind not in (KIND_DAILY_CATCHUP, KIND_SEGMENT_REPAIR):
            continue
        if record.get("active"):
            blocked.add(day)
            continue
        retry = float(record.get("next_retry_at") or 0)
        if retry > 0:
            retries[day] = max(retries.get(day, 0.0), retry)

    return [retry for day, retry in retries.items() if day not in blocked]


def _run_catchup_retry_tick(now: float) -> None:
    """Re-fire the catchup drain when a backed-off day's retry time passes.

    Live mode has no drain trigger between midnights, so a day that was still
    inside its backoff window at rollover waited ~24h for its next chance.
    Deferred mode is already served by _run_gate_tick.
    """
    global _last_catchup_retry_tick, _catchup_retry_watermark

    if now - _last_catchup_retry_tick < CATCHUP_RETRY_TICK_INTERVAL_S:
        return
    _last_catchup_retry_tick = now
    if _is_remote_mode:
        return
    if load_processing_settings().mode == "deferred":
        return

    expiries = _read_catchup_retry_expiries()

    # Seed on the first evaluation rather than firing: _startup_catchup_drain()
    # already covered every day whose retry expired before the supervisor came up.
    if _catchup_retry_watermark == 0.0:
        _catchup_retry_watermark = now
        return

    fired = any(_catchup_retry_watermark < expiry <= now for expiry in expiries)
    _catchup_retry_watermark = now
    if not fired:
        return

    # Today is perpetually "updated" while recording, and draining it mid-day
    # records a non-completion, pushing today into backoff churn and crowding the
    # freshest-days cap that the stuck older days need.
    today_str = datetime.now().date().strftime("%Y%m%d")
    run_catchup_drain(exclude={today_str})


_FATAL_TICK_EXCEPTIONS = (KeyboardInterrupt, asyncio.CancelledError, SystemExit)

# Consecutive-duplicate suppression for tick-step failure logging.
_last_tick_step_failure: tuple[str, str] | None = None


def _log_tick_step_failure(step: str, exc: Exception) -> None:
    """Log a guarded tick-step failure, debouncing identical consecutive repeats.

    The first occurrence of any distinct (step, message) always logs at ERROR
    with a full traceback; an immediately-repeated identical failure downgrades
    to DEBUG so a persistently-failing step cannot flood the log.
    """
    global _last_tick_step_failure
    signature = (step, f"{type(exc).__name__}: {exc}")
    if signature == _last_tick_step_failure:
        logging.debug("Supervision step %r still failing: %s", step, exc)
        return
    _last_tick_step_failure = signature
    logging.error("Supervision step %r failed; continuing", step, exc_info=True)


def _guarded_tick_step(step: str, fn: Callable[[], None]) -> None:
    """Run a synchronous tick step, swallowing non-fatal exceptions."""
    try:
        fn()
    except _FATAL_TICK_EXCEPTIONS:
        raise
    except Exception as exc:
        _log_tick_step_failure(step, exc)


async def _guarded_tick_step_async(step: str, coro_factory) -> None:
    """Run an awaited tick step, swallowing non-fatal exceptions."""
    try:
        await coro_factory()
    except _FATAL_TICK_EXCEPTIONS:
        raise
    except Exception as exc:
        _log_tick_step_failure(step, exc)


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
    global _last_gate_tick, _last_sync_tick
    global _last_catchup_retry_tick, _catchup_retry_watermark
    global _last_sync_snapshot, _sync_conflict_shutdown

    last_status_emit = 0.0
    _last_gate_tick = 0.0
    _last_catchup_retry_tick = 0.0
    _catchup_retry_watermark = 0.0
    reset_display_powersave_monitor()
    _last_sync_tick = 0.0
    _last_sync_snapshot = None
    _sync_conflict_shutdown = False

    try:
        while (
            not shutdown_requested
        ):  # pragma: no cover - loop checked via unit tests by patching
            if _task_queue:
                _guarded_tick_step(
                    "enforce_deadlines",
                    lambda: _task_queue.enforce_deadlines(time.time()),
                )

            # Check for runner exits first (immediate alert)
            if procs:
                await _guarded_tick_step_async(
                    "handle_runner_exits", lambda: handle_runner_exits(procs)
                )
                await _guarded_tick_step_async(
                    "reconcile_local_provider_runtime",
                    lambda: _reconcile_local_provider_runtime(procs),
                )
                await _guarded_tick_step_async(
                    "reconcile_parakeet_provider_runtime",
                    lambda: _reconcile_parakeet_provider_runtime(procs),
                )

            _guarded_tick_step(
                "release_provider_startup_gate",
                _release_provider_startup_gate_if_ready,
            )

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
            _guarded_tick_step("check_segment_flush", _check_segment_flush)

            # Check for journal sync conflicts (usually just heartbeat IO)
            if not _run_sync_tick(now):
                return

            # Check for daily processing (non-blocking, submits via task queue)
            if daily:
                _guarded_tick_step("handle_daily_tasks", handle_daily_tasks)
                _guarded_tick_step("run_gate_tick", lambda: _run_gate_tick(now))
                _guarded_tick_step(
                    "run_catchup_retry_tick", lambda: _run_catchup_retry_tick(now)
                )

            # Check periodic task schedules (non-blocking, submits via callosum)
            if schedule:
                _guarded_tick_step("scheduler_check", scheduler.check)

            # Sleep 1 second before next iteration (responsive to shutdown)
            await asyncio.sleep(1)
    finally:
        _cancel_all_provider_starts("supervisor shutdown")
        _cancel_all_provider_stops("supervisor shutdown")
        # Callosum cleanup happens in main().


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
        "--no-spl",
        action="store_true",
        help="Do not start the spl tunnel service",
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
        FLAG,
        action="store_true",
        help=(
            "App-supervised mode: skip all service-unit work and self-exit when "
            "the parent process dies (used by the macOS app)."
        ),
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
        _cancel_all_provider_starts("supervisor shutdown signal")
        _cancel_all_provider_stops("supervisor shutdown signal")
        live = [managed for managed in _managed_procs if managed.is_running()]
        if live:
            logger.info("shutdown: signaling %d managed child(ren)", len(live))
            for managed in live:
                try:
                    managed.process.terminate()
                except Exception:
                    logger.exception("shutdown: terminate failed for %s", managed.name)

            deadline = time.monotonic() + HANDLE_SHUTDOWN_REAP_S
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


def register_baseline_caps(queue: TaskQueue) -> None:
    """Register caps that must hold regardless of the schedule_enabled gate.

    Reactive partitions (daily/segment/indexer/importer) and on-demand backup
    can reach _active even under --no-schedule, so their caps cannot live behind
    the schedule-only registration block.
    """
    for name, seconds in REACTIVE_TASK_CAPS.items():
        queue.set_cap(name, seconds)
    queue.set_cap(
        TaskQueue.get_command_name(BACKUP_RUN_CMD),
        parse_duration_seconds(BACKUP_MAX_RUNTIME),
    )


def _register_scheduler_defaults() -> None:
    """Register built-in scheduler defaults, tolerating a malformed config file.

    scheduler.register_defaults() reads config/schedules.json with
    RAISE-on-malformed semantics; a hand-corrupted or truncated file must
    degrade to "no built-in defaults" rather than abort supervisor boot.
    """
    try:
        scheduler.register_defaults()
    except Exception:
        logging.error(
            "Failed to register scheduler defaults (malformed schedules.json?); "
            "continuing without built-in defaults",
            exc_info=True,
        )


def main() -> None:
    parser = parse_args()

    # Capture journal info before setup_cli() hydrates os.environ from journal
    # config and strips shell-only managed provider keys.
    journal_info = get_journal_info()

    args = setup_cli(parser)
    app_supervised = is_app_supervised(sys.argv)
    _ensure_venv_bin_on_path()

    journal_path = _get_journal_path()

    log_level = logging.DEBUG if args.debug else logging.INFO
    log_path = journal_path / "health" / "supervisor.log"
    log_path.parent.mkdir(parents=True, exist_ok=True)
    _configure_supervisor_logging(log_path, log_level)

    core_result = core_handshake.check_solstone_core_handshake()
    if core_result.status == "skip":
        print(core_result.message)
        logging.info(core_result.message)
    elif core_result.status == "fail":
        print(core_result.message, file=sys.stderr)
        logging.error(core_result.message)
        sys.exit(core_handshake.EX_CONFIG)

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

    pid_path.write_text(str(os.getpid()))
    start_time_path = health_dir / "supervisor.start_time"
    # Stamp THIS process's kernel create_time() so the recorded value equals what
    # is_supervisor_up()/_valid_marker() later read via psutil for this pid —
    # eliminating (not minimizing) drift from a wall-clock time.time() stamp.
    start_time_path.write_text(str(psutil.Process().create_time()))
    logging.info("Singleton lock acquired (PID %d)", os.getpid())
    _sweep_orphaned_sol_processes(journal_path)

    from solstone.think.speakers_analyze_installation import (
        enter_speakers_analyze_generation,
    )

    try:
        speakers_generation = enter_speakers_analyze_generation(
            journal_path=journal_path
        )
    except Exception as exc:
        message = str(exc)
        print(message, file=sys.stderr)
        logging.error(message)
        sys.exit(core_handshake.EX_CONFIG)

    try:
        write_self_heartbeat(journal=journal_path)

        # Set up signal handlers
        signal.signal(signal.SIGINT, handle_shutdown)
        signal.signal(signal.SIGTERM, handle_shutdown)

        # Show journal path and source on startup
        path, source = journal_info
        print(f"Journal: {path} (from {source})")
        logging.info("Supervisor starting...")

        global _managed_procs, _supervisor_callosum, _is_remote_mode
        global _task_queue
        procs: list[RunnerManagedProcess] = []
        convey_port = None
        convey_proc = None

        # Remote mode: run sync instead of local processing
        _is_remote_mode = bool(args.remote)

        # Run pending journal-maintenance tasks before spawning any writer children.
        # Callosum isn't up yet (emit_fn=None); migrations log through supervisor's logger only.
        try:
            maint_results = run_pending_tasks(journal_path, emit_fn=None)
            ran = len(maint_results)
            succeeded = sum(1 for result in maint_results if result.success)
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
                    blocking_failures = [
                        result
                        for result in maint_results
                        if not result.success and result.task.blocks_supervisor_start
                    ]
                    if blocking_failures:
                        failure = blocking_failures[0]
                        message = (
                            "Startup blocked by maintenance task "
                            f"{failure.task.qualified_name} "
                            f"(exit {failure.exit_code}). Log: {failure.state_file}. "
                            "This task is retry-on-next-start; fix the error and start "
                            "the supervisor again."
                        )
                        logging.error(message)
                        print(f"  {message}", file=sys.stderr, flush=True)
                        sys.exit(1)
        except Exception:
            logging.exception("Maintenance runner raised; continuing startup")

        try:
            from solstone.think.importers.journal_archive import (
                sweep_stale_extract_dirs,
            )

            swept = sweep_stale_extract_dirs()
            if swept > 0:
                logging.info("Swept %d stale journal-archive extract dir(s)", swept)
        except Exception:
            logging.exception(
                "Journal archive extract sweep raised; continuing startup"
            )

        try:
            from solstone.observe.transcribe.speakers_analyze_adapter import (
                sweep_stale_speakers_analyze_dirs,
            )

            swept = sweep_stale_speakers_analyze_dirs()
            if swept > 0:
                logging.info("Swept %d stale speakers-analyze temp dir(s)", swept)
        except Exception:
            logging.exception("Speakers-analyze temp sweep raised; continuing startup")

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
        register_baseline_caps(_task_queue)

        # Now start other services (their startup events will be captured)
        if _is_remote_mode:
            # Remote mode: transfer send will be added here
            pass
        else:
            # Local mode: convey first, then sense for file processing
            os.environ["SOL_SUPERVISOR_SPAWNED"] = "1"
            if not args.no_convey:
                print(f"  Starting convey on port {args.port}...", flush=True)
                convey_proc, convey_port = start_convey_server(
                    verbose=args.verbose, debug=args.debug, port=args.port
                )
                procs.append(convey_proc)
                wait_for_convey_ready(convey_proc)
                print("  Convey ready", flush=True)
            # Sense handles file processing
            print("  Starting sense...", flush=True)
            procs.append(start_sense())
            # Cortex for agent execution
            if not args.no_cortex:
                print("  Starting cortex...", flush=True)
                procs.append(start_cortex_server())
            # spl tunnel service (opt-out via --no-spl)
            if not args.no_spl:
                print("  Starting spl...", flush=True)
                procs.append(start_spl_service())

        # Make procs accessible to restart handler
        _managed_procs = procs
        _initialize_provider_startup_gate()

        # Initialize daily state to today - think only triggers at midnight when day changes
        _daily_state["last_day"] = datetime.now().date()

        # Initialize periodic task scheduler
        schedule_enabled = not args.no_schedule and not _is_remote_mode
        if schedule_enabled and _supervisor_callosum:
            try:
                maintenance.register_maintenance_schedules()
            except Exception:
                logging.error("Failed to register maintenance schedules", exc_info=True)
            scheduler.init(_supervisor_callosum)
            _register_scheduler_defaults()
            if _task_queue:
                for cmd, seconds in scheduler.collect_runtime_caps():
                    cmd_name = TaskQueue.get_command_name(cmd)
                    _task_queue.set_cap(cmd_name, seconds)
                    logging.info(
                        "Registered max_runtime cap for %s: %ss",
                        cmd_name,
                        seconds,
                    )

        # Show Convey URL if running
        if convey_port:
            print(f"Convey: http://localhost:{convey_port}/")

        logging.info(f"Started {len(procs)} processes, entering supervision loop")
        daily_enabled = not args.no_daily and not _is_remote_mode
        if daily_enabled:
            logging.info("Daily processing scheduled for midnight")

        # Startup catchup: submit thinks for days with pending stream data
        if daily_enabled:
            _startup_catchup_drain()

        # Startup catch-up: submit overdue schedule entries missed while down
        if schedule_enabled and _supervisor_callosum:
            scheduler.catch_up()

        try:
            convey_accepting = convey_proc is None or is_solstone_up(timeout=1.0)
            if convey_accepting:
                print("  Supervisor ready", flush=True)
                _sd_notify("READY=1")
                signal_ready()
            else:
                logging.error(
                    "Convey is not accepting on :%s; withholding readiness marker, "
                    "continuing into supervise loop",
                    read_service_port("convey"),
                )
            if app_supervised:
                start_parent_death_watcher()
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
            _cancel_all_provider_starts("supervisor shutdown")
            _cancel_all_provider_stops("supervisor shutdown")
            try:
                clear_ready()
            except Exception as exc:
                logging.warning(
                    "Failed to clear readiness marker during shutdown: %s", exc
                )
            try:
                if not _sync_conflict_shutdown:
                    clear_self_heartbeat()
            except Exception as exc:
                logging.warning(
                    "Failed to clear sync heartbeat during shutdown: %s", exc
                )

            logging.info("Stopping all processes...")
            print("\nShutting down gracefully (this may take a moment)...", flush=True)

            if _task_queue:
                task_drain_timeout = (
                    APP_SUPERVISED_TASK_DRAIN_S if app_supervised else 10
                )
                _task_queue.shutdown(timeout=task_drain_timeout)

            # Stop services in reverse order
            child_stop_timeout = APP_SUPERVISED_CHILD_STOP_S if app_supervised else None
            for managed in reversed(procs):
                _stop_process(managed, timeout_cap=child_stop_timeout)

            # Disconnect supervisor's Callosum connection
            if _supervisor_callosum:
                _supervisor_callosum.stop()
                logging.info("Supervisor disconnected from Callosum")

            # Stop in-process Callosum server last
            callosum_join_timeout = (
                APP_SUPERVISED_CALLOSUM_JOIN_S if app_supervised else 5.0
            )
            stop_callosum_in_process(join_timeout=callosum_join_timeout)

            logging.info("Supervisor shutdown complete.")
            print("Shutdown complete.", flush=True)

        if _sync_conflict_shutdown:
            sys.exit(2)

    finally:
        speakers_generation.release()


if __name__ == "__main__":
    main()
