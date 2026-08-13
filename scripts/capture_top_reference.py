#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Capture and independently verify retained ``journal top`` behavior.

This is a build-time evidence tool. It executes one SHA-pinned Python source
file under deterministic pre-import modules. It is never imported by product
runtime code and never starts a process, socket, TTY, service, or network call.
"""

from __future__ import annotations

import argparse
import asyncio as _real_asyncio
import copy
import datetime as _real_datetime
import hashlib
import importlib.util
import json
import os
import subprocess
import sys
import types
from pathlib import Path
from typing import Any, NoReturn

ROOT = Path(__file__).resolve().parents[1]
TOOL_PATH = Path("scripts/capture_top_reference.py")
SOURCE_PATH = Path("solstone/think/top.py")
DEFAULT_OUTPUT = ROOT / "core/fixtures/top_reference.json"

PRODUCT_HEAD = "f7e880da44d34ac3ed583d098603d2c513a9299e"
SOURCE_BLOB = "a6690540717cb7da51ee732c75f57c72d9cabb1d"
SOURCE_SHA256 = "f23a857ecce0bcdf01668490445e5dd656102633f5fbccb54a0681664aafb062"
PYTHON_SHA256 = "255e900f44ce87c630e83b637a79435f9ae7778dd72f6e2a2f18a486e501d016"
PYTHON_VERSION = "3.14.6 (main, Jun 23 2026, 15:18:23) [Clang 22.1.3 ]"
ALLOWED_EVIDENCE_PATHS = {
    "core/fixtures/top_reference.json",
    "scripts/capture_top_reference.py",
    "scripts/check_top_reference.py",
}
EXPECTED_EVENT_SEQUENCE = [
    "supervisor/status",
    "supervisor/status",
    "supervisor/restarting",
    "supervisor/started",
    "supervisor/stopped",
    "supervisor/queue",
    "supervisor/queue",
    "logs/line",
    "logs/exec",
    "logs/line",
    "logs/exit",
    "logs/exit",
    "observe/status",
    "observe/status",
    "observe/observed",
    "observe/observed",
    "observe/observed",
    "observe/observed",
    "think/started",
    "think/status",
    "think/completed",
]
EXPECTED_MALFORMED_CASES = [
    "unknown-tract",
    "supervisor-status-defaults",
    "supervisor-service-missing-pid",
    "supervisor-services-wrong-type",
    "supervisor-queues-wrong-type",
    "queue-missing-command",
    "queue-wrong-count",
    "logs-exec-missing-pid",
    "logs-line-missing-ref",
    "logs-line-wrong-stream",
    "observe-missing-day",
    "observe-duration-wrong-type",
    "think-completed-defaults",
]
EXPECTED_RENDER_CASES = [
    "empty",
    "one",
    "full",
    "wide",
    "think-failed",
    "brain-supplied",
    "observe-idle",
    "observe-tmux-yellow",
    "observe-tmux-yellow-upper",
    "last-selected",
]
EXPECTED_ACTION_CASES = [
    "empty",
    "supervisor",
    "ordinary",
    "first-selected",
    "last-selected",
    "below-bound",
    "above-bound",
]
EXPECTED_LOOP_CASES = [
    "normal-periodic",
    "normal-q",
    "normal-ctrl-c",
    "normal-ctrl-d",
    "key-up",
    "key-down",
    "key-restart",
    "event-success",
    "context-error",
    "initial-render-error",
    "input-error",
    "event-error",
    "cleanup-error",
    "later-render-error",
    "sleep-error",
    "stop-error",
]


class VerificationError(RuntimeError):
    """Stable named fixture-verification failure."""

    def __init__(self, code: str, detail: str):
        super().__init__(f"{code}: {detail}")
        self.code = code


def reject(code: str, detail: str) -> NoReturn:
    raise VerificationError(code, detail)


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def git(*arguments: str) -> bytes:
    environment = os.environ.copy()
    for name in [
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_COMMON_DIR",
        "GIT_CONFIG_COUNT",
        "GIT_DIR",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_WORK_TREE",
    ]:
        environment.pop(name, None)
    environment["GIT_CONFIG_NOSYSTEM"] = "1"
    environment["HOME"] = "/nonexistent/top-reference-git-home"
    environment["XDG_CONFIG_HOME"] = "/nonexistent/top-reference-git-xdg"
    try:
        return subprocess.run(
            ["/usr/bin/git", *arguments],
            cwd=ROOT,
            check=True,
            capture_output=True,
            env=environment,
        ).stdout
    except subprocess.CalledProcessError as error:
        reject("git", f"git {' '.join(arguments)} failed: {error}")


def committed_blob(path: Path) -> tuple[str, str]:
    line = git("ls-tree", "HEAD", "--", path.as_posix()).decode().strip()
    fields = line.split(None, 3)
    if len(fields) != 4 or fields[1] != "blob" or fields[3] != path.as_posix():
        reject("source-git", f"{path} is not one committed blob at HEAD")
    committed = git("show", f"HEAD:{path.as_posix()}")
    current = (ROOT / path).read_bytes()
    if committed != current:
        reject("source-dirty", f"{path} differs from its committed blob")
    return fields[2], sha256_bytes(current)


def checked_environment() -> dict[str, Any]:
    head = git("rev-parse", "HEAD").decode().strip()
    try:
        git("merge-base", "--is-ancestor", PRODUCT_HEAD, head)
    except VerificationError:
        reject("source-head", f"{PRODUCT_HEAD} is not an ancestor of {head}")
    changed_paths = set(
        git("diff", "--name-only", f"{PRODUCT_HEAD}..{head}").decode().splitlines()
    )
    if not changed_paths or not changed_paths <= ALLOWED_EVIDENCE_PATHS:
        reject("source-head", f"unexpected post-ground paths {sorted(changed_paths)}")
    if git("status", "--porcelain").strip():
        reject("source-dirty", "capture requires a clean worktree")
    if sys.version != PYTHON_VERSION:
        reject("runtime-python", f"expected {PYTHON_VERSION!r}, got {sys.version!r}")
    executable_sha = sha256_bytes(Path(sys.executable).read_bytes())
    if executable_sha != PYTHON_SHA256:
        reject("runtime-executable", f"unexpected interpreter digest {executable_sha}")

    source_blob, source_sha = committed_blob(SOURCE_PATH)
    if source_blob != SOURCE_BLOB or source_sha != SOURCE_SHA256:
        reject("source-top", f"unexpected top source {source_blob}/{source_sha}")
    tool_blob, tool_sha = committed_blob(TOOL_PATH)
    return {
        "capture_tool": {
            "git_blob": tool_blob,
            "path": TOOL_PATH.as_posix(),
            "sha256": tool_sha,
        },
        "retained_source": {
            "git_blob": source_blob,
            "path": SOURCE_PATH.as_posix(),
            "sha256": source_sha,
        },
        "runtime": {
            "executable_sha256": executable_sha,
            "python": sys.version,
        },
        "source_ground": PRODUCT_HEAD,
    }


class Clock:
    def __init__(self, wall: float = 1_800_000_000.0):
        self.wall = wall

    def set(self, wall: float) -> None:
        self.wall = wall


CLOCK = Clock()
PROCESS_ROWS: dict[int, dict[str, Any]] = {}
RUN_TRACE: list[Any] = []
SLEEP_ERROR: str | None = None


class NoSuchProcess(Exception):
    pass


class AccessDenied(Exception):
    pass


class FakeProcess:
    def __init__(self, pid: int):
        self.pid = pid
        row = PROCESS_ROWS.get(pid, {"state": "missing"})
        state = row.get("state")
        if state == "missing":
            raise NoSuchProcess(pid)
        if state == "denied":
            raise AccessDenied(pid)
        self.row = row

    def cpu_percent(self, interval: Any = None) -> float:
        RUN_TRACE.append(["cpu", self.pid, interval])
        if self.row.get("cpu_error") == "missing":
            raise NoSuchProcess(self.pid)
        if self.row.get("cpu_error") == "denied":
            raise AccessDenied(self.pid)
        return float(self.row.get("cpu", 0.0))

    def memory_info(self) -> Any:
        if self.row.get("memory_error") == "missing":
            raise NoSuchProcess(self.pid)
        if self.row.get("memory_error") == "denied":
            raise AccessDenied(self.pid)
        return types.SimpleNamespace(rss=int(self.row.get("rss", 0)))

    def status(self) -> str:
        if self.row.get("status_error") == "missing":
            raise NoSuchProcess(self.pid)
        if self.row.get("status_error") == "denied":
            raise AccessDenied(self.pid)
        return str(self.row.get("state", "live"))


class FakeKey:
    def __init__(
        self, text: str = "", *, name: str | None = None, code: int | None = None
    ):
        self.text = text
        self.name = name
        self.code = code

    def __bool__(self) -> bool:
        return bool(self.text or self.name or self.code is not None)

    def lower(self) -> str:
        return self.text.lower()


class FakeContext:
    def __init__(self, name: str, error: str | None = None):
        self.name = name
        self.error = error

    def __enter__(self) -> FakeContext:
        RUN_TRACE.append(["enter", self.name])
        if self.error:
            raise RuntimeError(self.error)
        return self

    def __exit__(self, kind: Any, value: Any, traceback: Any) -> bool:
        RUN_TRACE.append(["exit", self.name, kind.__name__ if kind else None])
        return False


class FakeTerminal:
    TOKENS = {
        "home": "<HOME>",
        "clear": "<CLEAR>",
        "normal": "<NORMAL>",
        "bold": "<BOLD>",
        "cyan": "<CYAN>",
        "red": "<RED>",
        "yellow": "<YELLOW>",
        "green": "<GREEN>",
        "magenta": "<MAGENTA>",
        "dim": "<DIM>",
        "black_on_white": "<SELECT>",
    }

    def __init__(self, width: int = 80):
        self.width = width
        self.keys: list[FakeKey] = []
        self.input_error: str | None = None
        self.context_error: str | None = None
        for name, token in self.TOKENS.items():
            if name == "black_on_white":
                setattr(
                    self, name, lambda value, token=token: token + value + "</SELECT>"
                )
            else:
                setattr(self, name, token)

    def cbreak(self) -> FakeContext:
        return FakeContext("cbreak", self.context_error)

    def hidden_cursor(self) -> FakeContext:
        return FakeContext("hidden_cursor")

    def inkey(self, timeout: float) -> FakeKey:
        RUN_TRACE.append(["input", timeout])
        if self.input_error:
            raise RuntimeError(self.input_error)
        if self.keys:
            return self.keys.pop(0)
        return FakeKey()


class FakeConnection:
    def __init__(self):
        self.emissions: list[dict[str, Any]] = []
        self.stop_error: str | None = None

    def start(self, callback: Any) -> None:
        RUN_TRACE.append(["connection", "start"])
        self.callback = callback

    def stop(self) -> None:
        RUN_TRACE.append(["connection", "stop"])
        if self.stop_error:
            raise RuntimeError(self.stop_error)

    def emit(self, tract: str, event: str, **fields: Any) -> None:
        self.emissions.append({"event": event, **fields, "tract": tract})


class FakeDateTime(_real_datetime.datetime):
    @classmethod
    def now(cls, tz: Any = None) -> _real_datetime.datetime:
        instant = _real_datetime.datetime.fromtimestamp(
            CLOCK.wall, tz=_real_datetime.timezone.utc
        )
        if tz is not None:
            return instant.astimezone(tz)
        return instant.replace(tzinfo=None)


async def fake_sleep(seconds: float) -> None:
    RUN_TRACE.append(["sleep", seconds])
    if SLEEP_ERROR:
        raise RuntimeError(SLEEP_ERROR)


def fake_asyncio_run(awaitable: Any) -> Any:
    return _real_asyncio.run(awaitable)


def make_modules() -> dict[str, types.ModuleType]:
    solstone = types.ModuleType("solstone")
    solstone.__path__ = []  # type: ignore[attr-defined]
    think = types.ModuleType("solstone.think")
    think.__path__ = []  # type: ignore[attr-defined]

    psutil = types.ModuleType("psutil")
    psutil.NoSuchProcess = NoSuchProcess
    psutil.AccessDenied = AccessDenied
    psutil.STATUS_ZOMBIE = "zombie"
    psutil.Process = FakeProcess

    blessed = types.ModuleType("blessed")
    blessed.Terminal = FakeTerminal

    argparse_module = types.ModuleType("argparse")

    class FakeArgumentParser:
        def __init__(self, *args: Any, **kwargs: Any):
            self.prog = str(kwargs.get("prog", "top-reference"))

        def add_argument(self, *args: Any, **kwargs: Any) -> None:
            return None

        def parse_args(self) -> types.SimpleNamespace:
            return types.SimpleNamespace()

    argparse_module.ArgumentParser = FakeArgumentParser

    queue_module = types.ModuleType("queue")

    class Empty(Exception):
        pass

    class FakeQueue:
        def __init__(self):
            self.items: list[Any] = []

        def put_nowait(self, value: Any) -> None:
            self.items.append(value)

        def get_nowait(self) -> Any:
            if not self.items:
                raise Empty
            return self.items.pop(0)

    queue_module.Empty = Empty
    queue_module.Queue = FakeQueue

    time_module = types.ModuleType("time")
    time_module.time = lambda: CLOCK.wall

    datetime_module = types.ModuleType("datetime")
    datetime_module.datetime = FakeDateTime
    datetime_module.timedelta = _real_datetime.timedelta
    datetime_module.timezone = _real_datetime.timezone

    asyncio_module = types.ModuleType("asyncio")
    asyncio_module.sleep = fake_sleep
    asyncio_module.run = fake_asyncio_run

    brain = types.ModuleType("solstone.think.brain_health")
    brain.build_brain_snapshot = lambda now, surface: {
        "now": now.isoformat(),
        "surface": surface,
    }
    brain.render_brain_health_lines = lambda value: list(value.get("lines", []))

    callosum = types.ModuleType("solstone.think.callosum")
    callosum.CallosumConnection = FakeConnection

    utils = types.ModuleType("solstone.think.utils")
    utils.setup_cli = lambda parser: RUN_TRACE.append(["setup_cli", parser.prog])

    return {
        "argparse": argparse_module,
        "asyncio": asyncio_module,
        "blessed": blessed,
        "datetime": datetime_module,
        "psutil": psutil,
        "queue": queue_module,
        "solstone": solstone,
        "solstone.think": think,
        "solstone.think.brain_health": brain,
        "solstone.think.callosum": callosum,
        "solstone.think.utils": utils,
        "time": time_module,
    }


def load_retained_module() -> types.ModuleType:
    source = ROOT / SOURCE_PATH
    if sha256_bytes(source.read_bytes()) != SOURCE_SHA256:
        reject("source-top", "retained source bytes changed")
    replacements = make_modules()
    previous = {name: sys.modules.get(name) for name in replacements}
    module_name = "_solstone_retained_top_reference"
    old_module = sys.modules.get(module_name)
    try:
        sys.modules.update(replacements)
        spec = importlib.util.spec_from_file_location(module_name, source)
        if spec is None or spec.loader is None:
            reject("source-load", "cannot create retained module spec")
        module = importlib.util.module_from_spec(spec)
        sys.modules[module_name] = module
        spec.loader.exec_module(module)
        if Path(module.__file__).resolve() != source.resolve():
            reject("source-load", f"executed unexpected source {module.__file__}")
        if sha256_bytes(Path(module.__file__).read_bytes()) != SOURCE_SHA256:
            reject("source-load", "executed source digest changed")
        return module
    finally:
        for name, value in previous.items():
            if value is None:
                sys.modules.pop(name, None)
            else:
                sys.modules[name] = value
        if old_module is None:
            sys.modules.pop(module_name, None)
        else:
            sys.modules[module_name] = old_module


def normalize(value: Any) -> Any:
    if isinstance(value, _real_datetime.datetime):
        return {"datetime": value.isoformat()}
    if isinstance(value, tuple):
        return [normalize(item) for item in value]
    if isinstance(value, list):
        return [normalize(item) for item in value]
    if isinstance(value, dict):
        return {
            str(key): normalize(item)
            for key, item in sorted(value.items(), key=lambda row: str(row[0]))
        }
    if isinstance(value, (str, int, float, bool)) or value is None:
        return value
    if isinstance(value, FakeProcess):
        return {"process": value.pid}
    return {"type": type(value).__name__}


def manager_state(manager: Any) -> dict[str, Any]:
    return normalize(
        {
            "command_queues": manager.command_queues,
            "brain_health": manager.brain_health,
            "brain_health_ts": manager.brain_health_ts,
            "cpu_cache": manager.cpu_cache,
            "cpu_pids": sorted(manager.cpu_procs),
            "crashed": manager.crashed,
            "displayed_mode": manager.displayed_mode,
            "finished_tasks": manager.finished_tasks,
            "last_active_ts": manager.last_active_ts,
            "last_log_lines": manager.last_log_lines,
            "observe_last_ts": manager.observe_last_ts,
            "observe_status": manager.observe_status,
            "recent_segments": manager.recent_segments,
            "running_tasks": manager.running_tasks,
            "selected": manager.selected,
            "service_status": manager.service_status,
            "services": manager.services,
            "think_last_completed": manager.think_last_completed,
            "think_running": manager.think_running,
            "think_status": manager.think_status,
        }
    )


def reset_processes() -> None:
    PROCESS_ROWS.clear()
    PROCESS_ROWS.update(
        {
            101: {"cpu": 12.4, "rss": 10_485_760, "state": "live"},
            102: {"state": "denied"},
            201: {"cpu": 8.6, "rss": 5_767_168, "state": "live"},
            202: {"state": "missing"},
            203: {"rss": 1_048_576, "state": "zombie"},
            204: {"rss": 2_097_152, "state": "denied"},
        }
    )


def new_manager(module: types.ModuleType, width: int = 80) -> Any:
    reset_processes()
    manager = module.ServiceManager()
    manager.term = FakeTerminal(width)
    manager.callosum = FakeConnection()
    return manager


def process_event(
    module: types.ModuleType, manager: Any, event: dict[str, Any]
) -> None:
    _real_asyncio.run(manager._process_event(event))


def event_program(module: types.ModuleType) -> list[dict[str, Any]]:
    manager = new_manager(module)
    manager.selected = 9
    events = [
        {
            "tract": "supervisor",
            "event": "status",
            "services": [
                {
                    "name": "supervisor",
                    "pid": 101,
                    "ref": "svc-a",
                    "uptime_seconds": 59,
                },
                {"name": "local", "pid": 102, "ref": "svc-b", "uptime_seconds": 3660},
            ],
            "crashed": [{"name": "old", "restart_attempts": 2}],
            "queues": {"health": 2},
        },
        {
            "tract": "supervisor",
            "event": "status",
            "services": [
                {
                    "name": "supervisor",
                    "pid": 101,
                    "ref": "svc-a",
                    "uptime_seconds": 60,
                },
                {"name": "local", "pid": 102, "ref": "svc-b", "uptime_seconds": 3661},
            ],
            "crashed": [],
            "queues": {"health": 1, "logs": 3},
        },
        {"tract": "supervisor", "event": "restarting", "service": "local"},
        {"tract": "supervisor", "event": "started", "service": "local"},
        {"tract": "supervisor", "event": "stopped", "service": "supervisor"},
        {"tract": "supervisor", "event": "queue", "command": "logs", "queued": 0},
        {"tract": "supervisor", "event": "queue", "command": "backup", "queued": 4},
        {
            "tract": "logs",
            "event": "line",
            "ref": "task-missed",
            "name": "missed",
            "pid": 201,
            "stream": "stderr",
            "line": "before exec",
        },
        {
            "tract": "logs",
            "event": "exec",
            "ref": "task-b",
            "name": "backup",
            "pid": 203,
            "cmd": ["backup", "now"],
        },
        {
            "tract": "logs",
            "event": "line",
            "ref": "task-b",
            "name": "backup",
            "pid": 203,
            "line": "done",
            "stream": "stdout",
        },
        {"tract": "logs", "event": "exit", "ref": "task-b", "exit_code": 7},
        {
            "tract": "logs",
            "event": "exit",
            "ref": "unknown-ref",
            "name": "unknown-name",
            "exit_code": 0,
        },
        {
            "tract": "observe",
            "event": "status",
            "mode": "screencast",
            "stream": "display",
            "audio": {"threshold_hits": 2, "will_save": True},
        },
        {
            "tract": "observe",
            "event": "status",
            "mode": "idle",
            "describe": {"running": ["a"], "queued": ["b", "c"]},
        },
        {
            "tract": "observe",
            "event": "observed",
            "day": "260810",
            "segment": "001",
            "duration": 1,
        },
        {
            "tract": "observe",
            "event": "observed",
            "day": "260810",
            "segment": "002",
            "duration": 60,
        },
        {
            "tract": "observe",
            "event": "observed",
            "day": "260810",
            "segment": "003",
            "duration": 121,
        },
        {
            "tract": "observe",
            "event": "observed",
            "day": "260810",
            "segment": "004",
            "duration": 180,
        },
        {"tract": "think", "event": "started"},
        {
            "tract": "think",
            "event": "status",
            "mode": "batch",
            "day": "260810",
            "segment": "003",
            "agents_completed": 1,
            "agents_total": 3,
            "current_agents": ["b", "a"],
            "segments_total": 4,
            "segments_completed": 2,
        },
        {
            "tract": "think",
            "event": "completed",
            "success": 2,
            "failed": 1,
            "failed_names": ["x"],
            "duration_ms": 61234,
        },
    ]
    results = []
    for index, event in enumerate(events):
        CLOCK.set(1_800_000_000.0 + index * 7)
        process_event(module, manager, event)
        results.append({"event": normalize(event), "state": manager_state(manager)})
    return results


def malformed_event_cases(module: types.ModuleType) -> list[dict[str, Any]]:
    cases = [
        {"name": "unknown-tract", "event": {"tract": "other", "event": "status"}},
        {
            "name": "supervisor-status-defaults",
            "event": {"tract": "supervisor", "event": "status"},
        },
        {
            "name": "supervisor-service-missing-pid",
            "event": {
                "tract": "supervisor",
                "event": "status",
                "services": [{"name": "broken", "ref": "x", "uptime_seconds": 1}],
            },
        },
        {
            "name": "supervisor-services-wrong-type",
            "event": {
                "tract": "supervisor",
                "event": "status",
                "services": "not-a-list",
            },
        },
        {
            "name": "supervisor-queues-wrong-type",
            "event": {
                "tract": "supervisor",
                "event": "status",
                "queues": 7,
            },
        },
        {
            "name": "queue-missing-command",
            "event": {"tract": "supervisor", "event": "queue", "queued": 2},
        },
        {
            "name": "queue-wrong-count",
            "event": {
                "tract": "supervisor",
                "event": "queue",
                "command": "x",
                "queued": "two",
            },
        },
        {
            "name": "logs-exec-missing-pid",
            "event": {"tract": "logs", "event": "exec", "ref": "r", "name": "n"},
        },
        {
            "name": "logs-line-missing-ref",
            "event": {
                "tract": "logs",
                "event": "line",
                "name": "n",
                "pid": 201,
                "line": "x",
            },
        },
        {
            "name": "logs-line-wrong-stream",
            "event": {
                "tract": "logs",
                "event": "line",
                "ref": "r",
                "stream": 7,
                "line": ["not", "text"],
            },
        },
        {
            "name": "observe-missing-day",
            "event": {"tract": "observe", "event": "observed", "segment": "1"},
        },
        {
            "name": "observe-duration-wrong-type",
            "event": {
                "tract": "observe",
                "event": "observed",
                "day": "260810",
                "segment": "1",
                "duration": "sixty",
            },
        },
        {
            "name": "think-completed-defaults",
            "event": {"tract": "think", "event": "completed"},
        },
    ]
    output = []
    for case in cases:
        manager = new_manager(module)
        try:
            process_event(module, manager, case["event"])
            outcome: dict[str, Any] = {"return": None, "state": manager_state(manager)}
        except Exception as error:
            outcome = {"error": type(error).__name__, "message": str(error)}
        output.append({**case, "outcome": outcome})
    return output


def formatting_cases(module: types.ModuleType) -> dict[str, Any]:
    manager = new_manager(module)
    CLOCK.set(1_800_000_000.0)
    uptime_values = [0, 5, 59, 60, 3599, 3600, 3660, 86399, 86400, 86460]
    runtime_values = [
        0,
        5,
        59,
        60,
        61,
        3599,
        3600,
        3660,
        86399,
        86400,
        86460,
    ]
    queue_values = [
        {},
        {"running": [], "queued": []},
        {"running": ["a"], "queued": []},
        {"running": [], "queued": ["a", "b"]},
        {"running": ["a", "b"], "queued": ["c", "d", "e"]},
    ]
    outputs: dict[str, Any] = {
        "uptime": [[value, manager.format_uptime(value)] for value in uptime_values],
        "runtime": [],
        "log_age": [],
        "queue": [
            [value, manager.format_queue_status(value)] for value in queue_values
        ],
        "memory": [],
        "cpu": [],
        "log_width": [],
        "mode": [],
        "service_icons": [],
    }
    for value in runtime_values:
        start = FakeDateTime.now() - _real_datetime.timedelta(seconds=value)
        outputs["runtime"].append([value, manager.format_runtime(start)])
        outputs["log_age"].append([value, manager.format_log_age(start)])
    for pid in [101, 102, 202, 203, 204]:
        outputs["memory"].append([pid, manager.get_memory_mb(pid)])
    manager.cpu_cache = {101: 12.4, 201: 8.6, 203: -0.4}
    for pid in [101, 201, 202, 203]:
        outputs["cpu"].append([pid, manager.get_cpu_percent(pid)])
    line = "αβ\x1b[31m-control-" + "x" * 100
    manager.last_log_lines["r"] = (FakeDateTime.now(), "stderr", line)
    for width in [40, 62, 63, 64, 80, 120]:
        manager.term = FakeTerminal(width)
        outputs["log_width"].append([width, normalize(manager.get_log_display("r"))])
    manager.observe_status = {"mode": "idle"}
    manager.displayed_mode = "tmux"
    manager.last_active_ts = CLOCK.wall - 9
    outputs["mode"].append(["idle-9", manager.get_displayed_mode()])
    manager.last_active_ts = CLOCK.wall - 10
    outputs["mode"].append(["idle-10", manager.get_displayed_mode()])
    manager.observe_status = {"mode": "screencast"}
    outputs["mode"].append(["active", manager.get_displayed_mode()])
    for status in ["requested", "restarting", "started", "stopped", "other"]:
        manager.service_status = {"service": (status, CLOCK.wall - 5)}
        outputs["service_icons"].append(
            [status, "at-five", list(manager.get_service_icon("service"))]
        )
        manager.service_status = {"service": (status, CLOCK.wall - 5.001)}
        outputs["service_icons"].append(
            [status, "after-five", list(manager.get_service_icon("service"))]
        )
    outputs["service_icons"].append(
        ["missing", "none", list(manager.get_service_icon("missing"))]
    )
    return outputs


def process_matrix(module: types.ModuleType) -> dict[str, Any]:
    manager = new_manager(module)
    CLOCK.set(1_800_000_000.0)
    started = FakeDateTime.now() - _real_datetime.timedelta(seconds=61)
    manager.running_tasks = {
        "live": {
            "ref": "live",
            "name": "live",
            "pid": 201,
            "cmd": [],
            "start_time": started,
        },
        "missing": {
            "ref": "missing",
            "name": "missing",
            "pid": 202,
            "cmd": [],
            "start_time": started,
        },
        "zombie": {
            "ref": "zombie",
            "name": "zombie",
            "pid": 203,
            "cmd": [],
            "start_time": started,
        },
        "denied": {
            "ref": "denied",
            "name": "denied",
            "pid": 204,
            "cmd": [],
            "start_time": started,
        },
    }
    manager.last_log_lines = {
        ref: (FakeDateTime.now(), "stdout", f"last-{ref}")
        for ref in manager.running_tasks
    }
    manager.cpu_procs = {201: FakeProcess(201), 203: FakeProcess(203)}
    manager.cpu_cache = {201: 8.6, 203: 2.0}
    manager.cleanup_dead_tasks()
    after_cleanup = manager_state(manager)
    at_five = manager.render_tasks_table()
    CLOCK.set(CLOCK.wall + 5.001)
    after_five = manager.render_tasks_table()
    return {
        "after_cleanup": after_cleanup,
        "ghosts_at_five": at_five,
        "ghosts_after_five": after_five,
    }


def render_scenarios(module: types.ModuleType) -> list[dict[str, Any]]:
    outputs = []
    for name, width in [("empty", 40), ("one", 64), ("full", 80), ("wide", 120)]:
        manager = new_manager(module, width)
        manager.BRAIN_HEALTH_INTERVAL = 10**9
        manager.brain_health_ts = CLOCK.wall
        if name != "empty":
            manager.services = [
                {
                    "name": "supervisor",
                    "pid": 101,
                    "ref": "svc-a",
                    "uptime_seconds": 3660,
                }
            ]
            manager.selected = 0
            manager.service_status["supervisor"] = ("started", CLOCK.wall)
            manager.last_log_lines["svc-a"] = (
                FakeDateTime.now(),
                "stdout",
                "service α line",
            )
        if name in ("full", "wide"):
            manager.services.append(
                {
                    "name": "local-service-name",
                    "pid": 102,
                    "ref": "svc-b",
                    "uptime_seconds": 86460,
                }
            )
            manager.crashed = [{"name": "crash\x1b", "restart_attempts": 3}]
            manager.running_tasks = {
                "task-a": {
                    "ref": "task-a",
                    "name": "backup",
                    "pid": 201,
                    "cmd": ["backup"],
                    "start_time": FakeDateTime.now()
                    - _real_datetime.timedelta(seconds=61),
                },
                "task-supervised": {
                    "ref": "task-supervised",
                    "name": "supervisor-copy",
                    "pid": 101,
                    "cmd": [],
                    "start_time": FakeDateTime.now(),
                },
            }
            manager.last_log_lines["task-a"] = (
                FakeDateTime.now(),
                "stderr",
                "task error " + "z" * 90,
            )
            manager.finished_tasks = {
                "ghost-ok": {
                    "name": "done",
                    "exit_code": 0,
                    "last_log": "ok",
                    "finished_at": CLOCK.wall - 1,
                },
                "ghost-bad": {
                    "name": "bad",
                    "exit_code": 4,
                    "last_log": "failed",
                    "finished_at": CLOCK.wall - 2,
                },
                "ghost-unknown": {
                    "name": "lost",
                    "exit_code": None,
                    "last_log": "gone",
                    "finished_at": CLOCK.wall - 3,
                },
            }
            manager.command_queues = {"backup": 2, "health": 3}
            manager.observe_status = {
                "mode": "screencast",
                "stream": "display",
                "screencast": {"window_elapsed_seconds": 60},
                "audio": {"threshold_hits": 2, "will_save": True},
                "describe": {"running": ["a"], "queued": ["b"]},
                "transcribe": {"queued": ["a", "b"]},
            }
            manager.displayed_mode = "screencast"
            manager.observe_last_ts = CLOCK.wall - (29 if name == "full" else 60)
            manager.recent_segments = [("260810", "003", 1), ("260810", "002", 120)]
            manager.think_running = True
            manager.think_status = {
                "mode": "batch",
                "day": "260810",
                "segment": "003",
                "agents_completed": 1,
                "agents_total": 3,
                "current_agents": ["b", "a"],
                "segments_total": 4,
                "segments_completed": 2,
            }
            manager.brain_health = {"lines": ["Brain Health — OK", "  memory good"]}
        outputs.append({"name": name, "width": width, "render": manager.render()})

    manager = new_manager(module, 80)
    manager.BRAIN_HEALTH_INTERVAL = 10**9
    manager.brain_health_ts = CLOCK.wall
    manager.think_last_completed = {
        "success": 2,
        "failed": 1,
        "duration_ms": 61234,
        "failed_names": ["agent-x"],
    }
    outputs.append({"name": "think-failed", "width": 80, "render": manager.render()})
    manager.brain_health = {"lines": ["Brain Health — DEGRADED", "  item"]}
    outputs.append({"name": "brain-supplied", "width": 80, "render": manager.render()})
    for name, mode, age in [
        ("observe-idle", "idle", 30),
        ("observe-tmux-yellow", "tmux", 30),
        ("observe-tmux-yellow-upper", "tmux", 59),
    ]:
        manager = new_manager(module, 80)
        manager.BRAIN_HEALTH_INTERVAL = 10**9
        manager.brain_health_ts = CLOCK.wall
        manager.observe_status = {
            "mode": mode,
            "tmux": {"captures": 2},
            "activity": {"screen_locked": True},
        }
        manager.displayed_mode = mode
        manager.last_active_ts = CLOCK.wall - 10
        manager.observe_last_ts = CLOCK.wall - age
        outputs.append({"name": name, "width": 80, "render": manager.render()})
    manager = new_manager(module, 80)
    manager.BRAIN_HEALTH_INTERVAL = 10**9
    manager.brain_health_ts = CLOCK.wall
    manager.services = [
        {"name": "first", "pid": 101, "ref": "first", "uptime_seconds": 1},
        {"name": "last", "pid": 201, "ref": "last", "uptime_seconds": 2},
    ]
    manager.selected = 1
    outputs.append({"name": "last-selected", "width": 80, "render": manager.render()})
    return outputs


def action_cases(module: types.ModuleType) -> list[dict[str, Any]]:
    rows = []
    service_sets = [
        ("empty", [], 0),
        (
            "supervisor",
            [{"name": "supervisor", "pid": 101, "ref": "a", "uptime_seconds": 1}],
            0,
        ),
        (
            "ordinary",
            [{"name": "local", "pid": 101, "ref": "a", "uptime_seconds": 1}],
            0,
        ),
        (
            "first-selected",
            [
                {"name": "one", "pid": 101, "ref": "a", "uptime_seconds": 1},
                {"name": "two", "pid": 201, "ref": "b", "uptime_seconds": 2},
            ],
            0,
        ),
        (
            "last-selected",
            [
                {"name": "one", "pid": 101, "ref": "a", "uptime_seconds": 1},
                {"name": "two", "pid": 201, "ref": "b", "uptime_seconds": 2},
            ],
            1,
        ),
        (
            "below-bound",
            [
                {"name": "one", "pid": 101, "ref": "a", "uptime_seconds": 1},
                {"name": "two", "pid": 201, "ref": "b", "uptime_seconds": 2},
            ],
            -1,
        ),
        (
            "above-bound",
            [
                {"name": "one", "pid": 101, "ref": "a", "uptime_seconds": 1},
                {"name": "two", "pid": 201, "ref": "b", "uptime_seconds": 2},
            ],
            9,
        ),
    ]
    for name, services, selected in service_sets:
        manager = new_manager(module)
        manager.services = services
        manager.selected = selected
        manager.send_restart()
        rows.append(
            {
                "name": name,
                "emissions": normalize(manager.callosum.emissions),
                "state": manager_state(manager),
            }
        )
    return rows


def run_case(module: types.ModuleType, name: str) -> dict[str, Any]:
    global SLEEP_ERROR
    RUN_TRACE.clear()
    SLEEP_ERROR = None
    manager = new_manager(module)
    manager.BRAIN_HEALTH_INTERVAL = 10**9
    manager.brain_health_ts = CLOCK.wall
    manager.term.keys = [FakeKey("x") for _ in range(16)] + [FakeKey("q")]
    if name == "normal-q":
        manager.term.keys = [FakeKey("q")]
    elif name == "normal-ctrl-c":
        manager.term.keys = [FakeKey(code=3)]
    elif name == "normal-ctrl-d":
        manager.term.keys = [FakeKey(code=4)]
    elif name == "key-up":
        manager.services = [
            {"name": "one", "pid": 101, "ref": "a", "uptime_seconds": 1},
            {"name": "two", "pid": 201, "ref": "b", "uptime_seconds": 1},
        ]
        manager.selected = 1
        manager.term.keys = [FakeKey(name="KEY_UP"), FakeKey("q")]
    elif name == "key-down":
        manager.services = [
            {"name": "one", "pid": 101, "ref": "a", "uptime_seconds": 1},
            {"name": "two", "pid": 201, "ref": "b", "uptime_seconds": 1},
        ]
        manager.term.keys = [FakeKey(name="KEY_DOWN"), FakeKey("q")]
    elif name == "key-restart":
        manager.services = [
            {"name": "ordinary", "pid": 101, "ref": "a", "uptime_seconds": 1}
        ]
        manager.term.keys = [FakeKey("r"), FakeKey("q")]
    elif name == "event-success":
        manager.event_queue.put_nowait(
            {
                "tract": "supervisor",
                "event": "queue",
                "command": "health",
                "queued": 2,
            }
        )
        manager.term.keys = [FakeKey("q")]
    original_render = manager.render
    original_process = manager._process_event
    original_cleanup = manager.cleanup_dead_tasks
    render_calls = 0

    def render() -> str:
        nonlocal render_calls
        render_calls += 1
        RUN_TRACE.append(["render", render_calls])
        if name == "initial-render-error" and render_calls == 1:
            raise RuntimeError(name)
        if name == "later-render-error" and render_calls == 2:
            raise RuntimeError(name)
        return original_render()

    async def process(message: dict[str, Any]) -> None:
        RUN_TRACE.append(["event", message.get("event")])
        if name == "event-error":
            raise RuntimeError(name)
        await original_process(message)

    def cleanup() -> None:
        RUN_TRACE.append(["cleanup"])
        if name == "cleanup-error":
            raise RuntimeError(name)
        original_cleanup()

    manager.render = render
    manager._process_event = process
    manager.cleanup_dead_tasks = cleanup
    if name == "context-error":
        manager.term.context_error = name
    if name == "input-error":
        manager.term.input_error = name
    if name == "event-error":
        manager.event_queue.put_nowait(
            {"tract": "supervisor", "event": "queue", "command": "x", "queued": 1}
        )
        manager.term.keys = [FakeKey("x")]
    if name == "cleanup-error":
        manager.term.keys = [FakeKey("x") for _ in range(17)]
    if name == "sleep-error":
        SLEEP_ERROR = name
        manager.term.keys = [FakeKey("x")]
    if name == "stop-error":
        manager.callosum.stop_error = name
        manager.term.keys = [FakeKey("q")]
    printed: list[str] = []
    old_print = module.__dict__.get("print", None)
    module.__dict__["print"] = lambda value="", **kwargs: printed.append(str(value))
    try:
        _real_asyncio.run(manager.run())
        outcome: dict[str, Any] = {"return": None}
    except Exception as error:
        outcome = {"error": type(error).__name__, "message": str(error)}
    finally:
        SLEEP_ERROR = None
        if old_print is None:
            module.__dict__.pop("print", None)
        else:
            module.__dict__["print"] = old_print
    return {
        "name": name,
        "outcome": outcome,
        "printed": printed,
        "running": manager.running,
        "selected": manager.selected,
        "emissions": normalize(manager.callosum.emissions),
        "state": manager_state(manager),
        "trace": normalize(RUN_TRACE),
    }


def loop_cases(module: types.ModuleType) -> list[dict[str, Any]]:
    return [
        run_case(module, name)
        for name in [
            "normal-periodic",
            "normal-q",
            "normal-ctrl-c",
            "normal-ctrl-d",
            "key-up",
            "key-down",
            "key-restart",
            "event-success",
            "context-error",
            "initial-render-error",
            "input-error",
            "event-error",
            "cleanup-error",
            "later-render-error",
            "sleep-error",
            "stop-error",
        ]
    ]


def validate_case_manifest(value: dict[str, Any]) -> None:
    event_sequence = [
        f"{row['event'].get('tract')}/{row['event'].get('event')}"
        for row in value["events"]
    ]
    named_sets = [
        ("event-sequence", event_sequence, EXPECTED_EVENT_SEQUENCE),
        (
            "malformed-cases",
            [row["name"] for row in value["malformed_events"]],
            EXPECTED_MALFORMED_CASES,
        ),
        (
            "render-cases",
            [row["name"] for row in value["renders"]],
            EXPECTED_RENDER_CASES,
        ),
        (
            "action-cases",
            [row["name"] for row in value["actions"]],
            EXPECTED_ACTION_CASES,
        ),
        (
            "loop-cases",
            [row["name"] for row in value["loop"]],
            EXPECTED_LOOP_CASES,
        ),
    ]
    for label, actual, expected in named_sets:
        if actual != expected:
            reject("case-manifest", f"{label} differs: {actual!r}")
    formatting = value["formatting"]
    boundaries = {
        "uptime": [0, 5, 59, 60, 3599, 3600, 3660, 86399, 86400, 86460],
        "runtime": [0, 5, 59, 60, 61, 3599, 3600, 3660, 86399, 86400, 86460],
        "log_age": [0, 5, 59, 60, 61, 3599, 3600, 3660, 86399, 86400, 86460],
        "log_width": [40, 62, 63, 64, 80, 120],
    }
    for label, expected in boundaries.items():
        actual = [row[0] for row in formatting[label]]
        if actual != expected:
            reject("case-manifest", f"{label} boundaries differ: {actual!r}")
    if len(formatting["service_icons"]) != 11:
        reject("case-manifest", "service icon denominator differs")


def build_reference() -> dict[str, Any]:
    provenance = checked_environment()
    module = load_retained_module()
    CLOCK.set(1_800_000_000.0)
    reference = {
        "actions": action_cases(module),
        "events": event_program(module),
        "formatting": formatting_cases(module),
        "loop": loop_cases(module),
        "malformed_events": malformed_event_cases(module),
        "process_matrix": process_matrix(module),
        "provenance": provenance,
        "renders": render_scenarios(module),
        "schema": 1,
    }
    validate_case_manifest(reference)
    return reference


def strict_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    output: dict[str, Any] = {}
    for key, value in pairs:
        if key in output:
            reject("duplicate-field", f"duplicate JSON field {key!r}")
        output[key] = value
    return output


def load_json(data: bytes) -> dict[str, Any]:
    try:
        value = json.loads(data.decode("utf-8"), object_pairs_hook=strict_object)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        reject("json", str(error))
    if not isinstance(value, dict):
        reject("schema", "top-level value must be an object")
    return value


def compare_shapes(actual: Any, expected: Any, path: str = "$") -> None:
    if type(actual) is not type(expected):
        reject("schema", f"{path} type differs")
    if isinstance(actual, dict):
        if set(actual) != set(expected):
            reject(
                "schema", f"{path} keys differ: {sorted(set(actual) ^ set(expected))}"
            )
        for key in sorted(actual):
            compare_shapes(actual[key], expected[key], f"{path}.{key}")
    elif isinstance(actual, list):
        if len(actual) != len(expected):
            reject("schema", f"{path} length differs")
        for index, (left, right) in enumerate(zip(actual, expected, strict=True)):
            compare_shapes(left, right, f"{path}[{index}]")


def semantic_verify(value: dict[str, Any]) -> None:
    expected = build_reference()
    compare_shapes(value, expected)
    for key in [
        "provenance",
        "events",
        "malformed_events",
        "process_matrix",
        "formatting",
        "renders",
        "actions",
        "loop",
    ]:
        if value[key] != expected[key]:
            reject(key.replace("_", "-"), f"{key} outcomes differ")
    if value["schema"] != 1:
        reject("schema", "schema differs")


def canonical_bytes(value: dict[str, Any]) -> bytes:
    return (
        json.dumps(value, ensure_ascii=True, sort_keys=True, separators=(",", ":"))
        + "\n"
    ).encode()


def expect_failure(code: str, action: Any) -> None:
    try:
        action()
    except VerificationError as error:
        if error.code != code:
            reject("self-test", f"expected {code}, got {error.code}: {error}")
    else:
        reject("self-test", f"expected {code} failure")


def representative_object_paths(value: Any) -> list[tuple[Any, ...]]:
    paths: dict[tuple[str, ...], tuple[Any, ...]] = {}

    def visit(current: Any, path: tuple[Any, ...]) -> None:
        if isinstance(current, dict):
            signature = tuple(sorted(current))
            paths.setdefault(signature, path)
            for key, item in current.items():
                visit(item, (*path, key))
        elif isinstance(current, list):
            for index, item in enumerate(current):
                visit(item, (*path, index))

    visit(value, ())
    return list(paths.values())


def value_at_path(value: Any, path: tuple[Any, ...]) -> Any:
    current = value
    for part in path:
        current = current[part]
    return current


def self_test() -> None:
    reference = build_reference()
    raw = canonical_bytes(reference)
    semantic_verify(load_json(raw))

    expect_failure(
        "duplicate-field",
        lambda: load_json(
            raw.replace(b'{"actions":', b'{"schema":1,"schema":1,"actions":', 1)
        ),
    )
    unknown = copy.deepcopy(reference)
    unknown["unknown"] = True
    expect_failure("schema", lambda: semantic_verify(unknown))
    missing = copy.deepcopy(reference)
    del missing["actions"]
    expect_failure("schema", lambda: semantic_verify(missing))
    for path in representative_object_paths(reference):
        changed = copy.deepcopy(reference)
        target = value_at_path(changed, path)
        target["__unknown__"] = True
        expect_failure(
            "schema", lambda changed=changed: compare_shapes(changed, reference)
        )
        original = value_at_path(reference, path)
        if original:
            changed = copy.deepcopy(reference)
            target = value_at_path(changed, path)
            del target[sorted(target)[0]]
            expect_failure(
                "schema", lambda changed=changed: compare_shapes(changed, reference)
            )
    for family in ["events", "malformed_events", "renders", "actions", "loop"]:
        changed = copy.deepcopy(reference)
        changed[family].pop()
        expect_failure(
            "case-manifest", lambda changed=changed: validate_case_manifest(changed)
        )
    mutations = [
        (
            "provenance",
            lambda value: value["provenance"]["runtime"].__setitem__("python", "wrong"),
        ),
        (
            "events",
            lambda value: value["events"][0]["state"].__setitem__("selected", 999),
        ),
        (
            "malformed-events",
            lambda value: value["malformed_events"][0]["event"].__setitem__(
                "tract", "other-mutated"
            ),
        ),
        (
            "process-matrix",
            lambda value: value["process_matrix"]["ghosts_after_five"].__setitem__(
                0, "wrong"
            ),
        ),
        (
            "formatting",
            lambda value: value["formatting"]["uptime"][0].__setitem__(1, "wrong"),
        ),
        ("renders", lambda value: value["renders"][0].__setitem__("render", "wrong")),
        (
            "actions",
            lambda value: value["actions"][2]["emissions"][0].__setitem__(
                "service", "wrong"
            ),
        ),
        (
            "loop",
            lambda value: value["loop"][0]["trace"][0].__setitem__(0, "wrong"),
        ),
    ]
    for code, mutation in mutations:
        changed = copy.deepcopy(reference)
        mutation(changed)
        expect_failure(code, lambda changed=changed: semantic_verify(changed))

    saved = {
        name: os.environ.get(name)
        for name in ["HOME", "PYTHONPATH", "TZ", "LANG", "SOLSTONE_JOURNAL"]
    }
    try:
        os.environ.update(
            {
                "HOME": "/tmp/top-poison-home",
                "PYTHONPATH": "/tmp/top-poison-python",
                "TZ": "Pacific/Kiritimati",
                "LANG": "C",
                "SOLSTONE_JOURNAL": "/tmp/top-poison-journal",
            }
        )
        if build_reference() != reference:
            reject("environment", "caller environment altered outcomes")
    finally:
        for name, value in saved.items():
            if value is None:
                os.environ.pop(name, None)
            else:
                os.environ[name] = value


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument("--check", action="store_true")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()

    if args.self_test:
        self_test()
        return
    built = canonical_bytes(build_reference())
    if args.check:
        existing = args.output.read_bytes()
        semantic_verify(load_json(existing))
        if existing != built:
            reject("canonical", f"{args.output} differs from fresh capture")
    else:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_bytes(built)


if __name__ == "__main__":
    main()
