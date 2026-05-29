#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Unified process spawning and lifecycle management utilities.

All subprocess output is automatically logged to:
    journal/chronicle/{YYYYMMDD}/health/{ref}_{process_name}.log

Where process_name is derived from cmd[0] basename, and ref is a unique correlation ID.

Symlinks provide stable access paths:
    journal/chronicle/{YYYYMMDD}/health/{process_name}.log (day-level symlink)
    journal/health/{process_name}.log (journal-level symlink)

Logs automatically roll over at midnight for long-running processes.
"""

from __future__ import annotations

import logging
import os
import signal
import subprocess
import sys
import threading
import time
from collections.abc import Sequence
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path

from solstone.think.callosum import CallosumConnection
from solstone.think.utils import CHRONICLE_DIR, get_journal, now_ms

logger = logging.getLogger(__name__)


def _set_pdeathsig_on_linux() -> None:
    """Best-effort: ask the kernel to SIGTERM this child if its parent dies.

    Linux-only. Uses prctl(PR_SET_PDEATHSIG, SIGTERM). Silent on failure —
    a missing PDEATHSIG must not block process spawn.
    """
    if sys.platform != "linux":
        return
    try:
        import ctypes

        PR_SET_PDEATHSIG = 1
        libc = ctypes.CDLL("libc.so.6", use_errno=True)
        libc.prctl(PR_SET_PDEATHSIG, signal.SIGTERM, 0, 0, 0)
    except Exception:
        # Best effort. Failure to set PDEATHSIG must not block spawn.
        pass


def _get_journal_path() -> Path:
    """Return the journal path (auto-creates if needed)."""
    return Path(get_journal())


def _current_day() -> str:
    """Get current day in YYYYMMDD format."""
    return datetime.now().strftime("%Y%m%d")


def _day_health_log_path(journal_root: Path, day: str, ref: str, name: str) -> Path:
    """Build path to day health log.

    Returns: journal/chronicle/{day}/health/{ref}_{name}.log
    """
    return journal_root / CHRONICLE_DIR / day / "health" / f"{ref}_{name}.log"


def _atomic_symlink(link_path: Path, target: str) -> None:
    """Create or update symlink atomically.

    Args:
        link_path: Path where symlink should be created
        target: Target path (can be relative or absolute)
    """
    link_path.parent.mkdir(parents=True, exist_ok=True)
    tmp_link = link_path.with_suffix(f".tmp{os.getpid()}_{threading.get_ident()}")
    try:
        tmp_link.symlink_to(target)
        tmp_link.replace(link_path)
    finally:
        # Clean up temp file if it still exists
        if tmp_link.exists() or tmp_link.is_symlink():
            tmp_link.unlink(missing_ok=True)


def _format_log_line(prefix: str, stream: str, line: str) -> str:
    """Format log line with ISO timestamp and labels.

    Args:
        prefix: Process identifier (e.g., "observer" or "describe:file.webm")
        stream: "stdout" or "stderr"
        line: Output line from process

    Returns:
        Formatted line: "2024-11-01T10:30:45 [prefix:stream] line\\n"
    """
    timestamp = datetime.now().isoformat(timespec="seconds")
    clean_line = line.rstrip("\n")
    return f"{timestamp} [{prefix}:{stream}] {clean_line}\n"


class DailyLogWriter:
    """Thread-safe log writer that automatically rolls over at midnight.

    When ``day`` is provided, the writer is pinned to that day directory
    and midnight rollover is disabled (batch processing of historical days).

    Writes to: journal/chronicle/{YYYYMMDD}/health/{ref}_{name}.log

    Creates and maintains symlinks:
    - journal/chronicle/{YYYYMMDD}/health/{name}.log -> {ref}_{name}.log (day-level)
    - journal/health/{name}.log -> chronicle/{YYYYMMDD}/health/{ref}_{name}.log (journal-level)

    When the day changes, automatically closes old file, opens new file, and updates symlinks.
    The journal root is resolved once at construction time and pinned for the
    lifetime of the writer.
    """

    def __init__(self, ref: str, name: str, day: str | None = None):
        self._ref = ref
        self._name = name
        self._journal_root: Path = _get_journal_path()
        self._pinned = day is not None
        self._lock = threading.Lock()
        self._current_day = day or _current_day()
        self._fh = self._open_log()
        self._update_symlinks()

    def _open_log(self):
        """Open log file for current day."""
        log_path = _day_health_log_path(
            self._journal_root, self._current_day, self._ref, self._name
        )
        log_path.parent.mkdir(parents=True, exist_ok=True)
        return log_path.open("a", encoding="utf-8")

    def _update_symlinks(self) -> None:
        """Update day-level and journal-level symlinks to point to current log."""
        journal = self._journal_root
        day_health = journal / CHRONICLE_DIR / self._current_day / "health"
        log_filename = f"{self._ref}_{self._name}.log"

        # Day-level symlink: chronicle/{YYYYMMDD}/health/{name}.log -> {ref}_{name}.log
        day_symlink = day_health / f"{self._name}.log"
        _atomic_symlink(day_symlink, log_filename)

        # Journal-level symlink: health/{name}.log -> ../chronicle/{YYYYMMDD}/health/{ref}_{name}.log
        # Relative from journal/health/ to journal/chronicle/{YYYYMMDD}/health/
        journal_symlink = journal / "health" / f"{self._name}.log"
        relative_target = (
            f"../{CHRONICLE_DIR}/{self._current_day}/health/{log_filename}"
        )
        _atomic_symlink(journal_symlink, relative_target)

    def write(self, message: str) -> None:
        """Write message to log, handling day rollover."""
        with self._lock:
            if not self._pinned:
                # Check for day change
                day_now = _current_day()
                if day_now != self._current_day:
                    # Close old log
                    if not self._fh.closed:
                        self._fh.close()
                    # Open new log for new day — keep old handle on failure
                    try:
                        self._fh = self._open_log()
                        self._current_day = day_now
                        self._update_symlinks()
                    except OSError:
                        pass

            # Write and flush — swallow disk-full so output threads survive
            try:
                self._fh.write(message)
                self._fh.flush()
            except OSError:
                pass

    def close(self) -> None:
        """Close log file."""
        with self._lock:
            if not self._fh.closed:
                self._fh.close()

    @property
    def path(self) -> Path:
        """Get current log file path."""
        return _day_health_log_path(
            self._journal_root, self._current_day, self._ref, self._name
        )


def _command_partition(cmd: Sequence[str]) -> str:
    """Return the queue/log partition name for a managed-process cmd.

    Think tasks partition by bare mode name (daily/segment/flush/activity/weekly);
    everything else uses sol/journal subcommand or process basename.
    """
    if cmd and cmd[0] in ("sol", "journal") and len(cmd) > 1:
        name = cmd[1]
        if name == "think":
            for flag, mode in [
                ("--activity", "activity"),
                ("--flush", "flush"),
                ("--segments", "segment"),
                ("--weekly", "weekly"),
                ("--segment", "segment"),
            ]:
                if flag in cmd:
                    name = mode
                    break
            else:
                name = "daily"
    else:
        name = Path(cmd[0]).name if cmd else "unknown"
    return name


@dataclass
class ManagedProcess:
    """Subprocess wrapper with automatic output logging and lifecycle management.

        All output is automatically logged to:
            journal/chronicle/{YYYYMMDD}/health/{ref}_{name}.log

    Where name is derived from cmd[0] basename, and ref is a unique correlation ID.

        Symlinks are automatically created and maintained:
            journal/chronicle/{YYYYMMDD}/health/{name}.log -> {ref}_{name}.log (day-level)
            journal/health/{name}.log -> chronicle/{YYYYMMDD}/health/{ref}_{name}.log (journal-level)

    Logs roll over automatically at midnight for long-running processes.

    Process lifecycle events are broadcast via Callosum logs tract.
    """

    process: subprocess.Popen
    name: str
    log_writer: DailyLogWriter
    cmd: list[str]
    _threads: list[threading.Thread]
    ref: str
    _start_time: float
    _callosum: CallosumConnection | None
    _owns_callosum: bool = True

    @property
    def start_time(self) -> float:
        """Epoch timestamp when this process was spawned."""
        return self._start_time

    @classmethod
    def spawn(
        cls,
        cmd: list[str],
        *,
        env: dict | None = None,
        ref: str | None = None,
        callosum: CallosumConnection | None = None,
        day: str | None = None,
        nice: int | None = None,
    ) -> "ManagedProcess":
        """Spawn process with automatic output logging to daily health directory.

        Args:
            cmd: Command and arguments
            env: Optional environment variables (inherits parent env if not provided)
            ref: Optional correlation ID (auto-generated if not provided)
            callosum: Optional shared CallosumConnection (creates new one if not provided)
            day: Optional day override (YYYYMMDD). When provided, logs are placed
                in that day's health directory instead of today's.
            nice: Optional nice increment to apply in the child process before exec.

        Returns:
            ManagedProcess instance

        Raises:
            RuntimeError: If process fails to spawn

        Example:
            managed = ManagedProcess.spawn(["observer", "-v"])
            # Logs to: {JOURNAL}/{YYYYMMDD}/health/{ref}_observer.log
            # Symlinks: {YYYYMMDD}/health/observer.log (day-level)
            #           health/observer.log (journal-level)

            # With explicit correlation ID:
            managed = ManagedProcess.spawn(
                ["sol", "indexer", "--rescan"],
                ref="1730476800000",
            )
            # Logs to: {JOURNAL}/{YYYYMMDD}/health/1730476800000_indexer.log

        Caller contract:
            This method installs a subprocess.Popen preexec_fn that always calls
            _set_pdeathsig_on_linux; when nice is provided, it then applies
            os.nice(nice). _set_pdeathsig_on_linux calls
            prctl(PR_SET_PDEATHSIG, SIGTERM), and man 2 prctl defines
            PR_SET_PDEATHSIG relative to the calling task's TID: the thread
            that called Popen, not just the thread-group leader. If that thread
            exits before the child does, the kernel delivers SIGTERM to the
            child when the calling task terminates. Never call spawn() from a
            daemon monitor thread that returns immediately after this call. Use
            a long-lived worker thread that blocks in process.wait() for the
            lifetime of the child.
        """
        name = _command_partition(cmd)

        # Generate correlation ID (use provided ref, else timestamp)
        ref = ref if ref else str(now_ms())
        start_time = time.time()

        # Use provided callosum or create new one
        owns_callosum = callosum is None
        if owns_callosum:
            callosum = CallosumConnection()
            callosum.start()

        log_writer = DailyLogWriter(ref, name, day=day)

        logger.info(f"Starting {name}: {' '.join(cmd)}")

        preexec_fn = _set_pdeathsig_on_linux
        if nice is not None:

            def _preexec_with_nice() -> None:
                _set_pdeathsig_on_linux()
                os.nice(nice)

            preexec_fn = _preexec_with_nice

        try:
            proc = subprocess.Popen(
                cmd,
                stdin=subprocess.DEVNULL,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
                bufsize=1,
                env=env,
                process_group=0,
                preexec_fn=preexec_fn,
            )
        except Exception as exc:
            log_writer.close()
            if owns_callosum and callosum:
                callosum.stop()
            raise RuntimeError(f"Failed to spawn {name}: {exc}") from exc

        logger.info(f"Started {name} with PID {proc.pid}")

        # Emit exec event
        if callosum:
            callosum.emit(
                "logs",
                "exec",
                ref=ref,
                name=name,
                pid=proc.pid,
                cmd=list(cmd),
                log_path=str(log_writer.path),
            )

        # Start output streaming threads
        def stream_output(pipe, stream_label: str):
            if pipe is None:
                return
            with pipe:
                for line in pipe:
                    formatted = _format_log_line(name, stream_label, line)
                    log_writer.write(formatted)

                    # Emit line event
                    if callosum:
                        callosum.emit(
                            "logs",
                            "line",
                            ref=ref,
                            name=name,
                            pid=proc.pid,
                            stream=stream_label,
                            line=line.rstrip("\n"),
                        )

        threads = [
            threading.Thread(
                target=stream_output,
                args=(proc.stdout, "stdout"),
                daemon=True,
            ),
            threading.Thread(
                target=stream_output,
                args=(proc.stderr, "stderr"),
                daemon=True,
            ),
        ]
        for thread in threads:
            thread.start()

        return cls(
            process=proc,
            name=name,
            log_writer=log_writer,
            cmd=list(cmd),
            _threads=threads,
            ref=ref,
            _start_time=start_time,
            _callosum=callosum,
            _owns_callosum=owns_callosum,
        )

    def wait(self, timeout: float | None = None) -> int:
        """Wait for process completion, return exit code.

        Args:
            timeout: Optional timeout in seconds

        Returns:
            Exit code

        Raises:
            subprocess.TimeoutExpired: If timeout exceeded
        """
        return self.process.wait(timeout=timeout)

    def poll(self) -> int | None:
        """Check if process has terminated.

        Returns:
            Exit code if terminated, None if still running
        """
        return self.process.poll()

    def is_running(self) -> bool:
        """Check if process is still running."""
        return self.process.poll() is None

    def terminate(self, timeout: float = 15) -> int:
        """Terminate the managed process and its session group.

        Sends SIGTERM to the immediate child and the process group. Waits up to
        `timeout` seconds for graceful exit. If the process is still alive after
        `timeout`, escalates to SIGKILL on the group and child, then re-raises
        `subprocess.TimeoutExpired` after the kill completes.

        Args:
            timeout: Seconds to wait after SIGTERM before SIGKILL (default: 15).

        Returns:
            Exit code on graceful termination (may be negative for signals,
            e.g., -15 for SIGTERM).

        Raises:
            subprocess.TimeoutExpired: Re-raised after SIGKILL when graceful
                shutdown did not complete within `timeout`.
        """
        logger.debug(f"Terminating {self.name} (PID {self.pid})...")
        try:
            pgid = os.getpgid(self.process.pid)
        except (ProcessLookupError, OSError):
            pgid = None

        try:
            try:
                self.process.terminate()
            except (ProcessLookupError, OSError):
                pass
            if pgid is not None:
                try:
                    os.killpg(pgid, signal.SIGTERM)
                except (ProcessLookupError, OSError):
                    pass
            exit_code = self.process.wait(timeout=timeout)
            logger.debug(f"{self.name} terminated gracefully with code {exit_code}")
            return exit_code
        except subprocess.TimeoutExpired:
            logger.warning(
                f"{self.name} did not terminate after {timeout}s, force killing..."
            )
            if pgid is not None:
                try:
                    os.killpg(pgid, signal.SIGKILL)
                except (ProcessLookupError, OSError):
                    pass
            try:
                self.process.kill()
            except (ProcessLookupError, OSError):
                pass
            self.process.wait()
            logger.debug(f"{self.name} killed with code {self.process.returncode}")
            raise

    def cleanup(self) -> None:
        """Wait for output threads to finish and close log file.

        Call this after process exits to clean up resources.
        Each step is isolated so one failure doesn't block the rest.
        """
        for thread in self._threads:
            try:
                thread.join(timeout=1)
            except Exception:
                pass

        try:
            self.log_writer.close()
        except Exception:
            pass

        # Emit exit event
        if self._callosum:
            try:
                duration_ms = int((time.time() - self._start_time) * 1000)
                self._callosum.emit(
                    "logs",
                    "exit",
                    ref=self.ref,
                    name=self.name,
                    pid=self.pid,
                    exit_code=self.returncode,
                    duration_ms=duration_ms,
                    cmd=self.cmd,
                    log_path=str(self.log_writer.path),
                )
            except Exception:
                pass
            # Only stop callosum if we created it (not shared)
            if self._owns_callosum:
                try:
                    self._callosum.stop()
                except Exception:
                    pass

    @property
    def pid(self) -> int:
        """Process ID."""
        return self.process.pid

    @property
    def returncode(self) -> int | None:
        """Return code if process has exited, None otherwise."""
        return self.process.returncode


def run_task(
    cmd: list[str],
    *,
    timeout: float | None = None,
    env: dict | None = None,
    ref: str | None = None,
    callosum: CallosumConnection | None = None,
    day: str | None = None,
) -> tuple[bool, int, Path]:
    """Run a task to completion with automatic logging (blocking).

    Spawns process, waits for completion, cleans up resources.
    Output is automatically logged to: journal/{YYYYMMDD}/health/{ref}_{name}.log
    where name is derived from cmd[0] basename.

    Args:
        cmd: Command and arguments
        timeout: Optional timeout in seconds
        env: Optional environment variables
        ref: Optional correlation ID (auto-generated if not provided)
        callosum: Optional shared CallosumConnection (creates new one if not provided)
        day: Optional day override (YYYYMMDD). When provided, logs are placed
            in that day's health directory instead of today's.

    Returns:
        (success, exit_code, log_path) tuple where success = (exit_code == 0)
        and log_path points to the process output log file.

    Example:
        success, code, log = run_task(
            ["sol", "generate", "20241101", "-f", "flow"],
            timeout=300,
        )
        # Logs to: {JOURNAL}/{YYYYMMDD}/health/{ref}_generate.log

        # With explicit correlation ID:
        success, code, log = run_task(
            ["sol", "indexer", "--rescan"],
            ref="1730476800000",
        )
        # Logs to: {JOURNAL}/{YYYYMMDD}/health/1730476800000_indexer.log
    """
    managed = ManagedProcess.spawn(cmd, env=env, ref=ref, callosum=callosum, day=day)
    log_path = managed.log_writer.path
    try:
        exit_code = managed.wait(timeout=timeout)
    except subprocess.TimeoutExpired:
        logger.error(f"{managed.name} timed out after {timeout}s, terminating...")
        exit_code = managed.terminate()
    finally:
        managed.cleanup()

    if exit_code != 0:
        logger.warning(f"{managed.name} exited with code {exit_code}")

    return (exit_code == 0, exit_code, log_path)
