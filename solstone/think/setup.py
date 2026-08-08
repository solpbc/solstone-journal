# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""User-runtime setup orchestration for solstone."""

from __future__ import annotations

import argparse
import contextlib
import io
import json
import logging
import os
import subprocess
import sys
import tempfile
import threading
import time
from dataclasses import asdict, dataclass
from datetime import datetime, timezone
from enum import Enum
from importlib.metadata import PackageNotFoundError
from importlib.metadata import version as _pkg_version
from pathlib import Path
from typing import Any, Callable, Literal

from solstone.think import parakeet_readiness
from solstone.think.journal_config import ensure_journal_config
from solstone.think.setup_events import (
    EVENT_TYPES,
    SOL_ALREADY_KEEPS_JOURNAL_REASON,
    JsonlEmitter,
    utc_now_iso,
)
from solstone.think.user_config import (
    config_path,
    default_journal,
    read_user_config,
    write_user_config,
)
from solstone.think.utils import get_project_root
from solstone.think.utils import is_source_checkout as source_checkout

MANIFEST_SCHEMA_VERSION = 1
# doctor is examine-only and must complete near-instantly; 30s is a generous
# backstop, not a work budget. If doctor approaches it, the cause is a layer
# leak (a heavy import) to fix at the source — do not raise this number.
DOCTOR_TIMEOUT_SECONDS = 30
DOCTOR_JSONL_EVENTS = frozenset(
    {"doctor.started", "check.completed", "doctor.completed"}
)
SOL_ALREADY_KEEPS_JOURNAL_NARRATION = (
    "sol on this Mac already keeps this journal, so setup did not install a background launcher.\n"
    "Run journal doctor if something looks wrong."
)

StepStatus = Literal["ok", "skipped", "failed", "warning"]
CleanUninstallState = Literal["removed", "already-absent", "skipped", "failed"]


def narrate(ctx: "SetupContext | None", *args: Any, **kwargs: Any) -> None:
    """Print to stdout/stderr unless JSONL mode is active."""
    if ctx is not None and ctx.jsonl:
        return
    print(*args, **kwargs)


class SetupMode(Enum):
    INTERACTIVE = "interactive"
    NON_INTERACTIVE = "non_interactive"
    DRY_RUN = "dry_run"
    EXPLAIN = "explain"


@dataclass
class SetupContext:
    mode: SetupMode
    project_root: Path
    is_source_checkout: bool
    journal_path: Path
    journal_source: str
    config_path: Path
    manifest_path: Path
    port: int
    port_source: str
    port_supplied: bool
    step_timeout_seconds: int
    variant: str
    variant_source: str
    yes: bool
    skip_models: bool
    skip_brain: bool
    skip_skills: bool
    skip_service: bool
    skip_wrapper: bool
    accept_existing_journal: bool
    force: bool
    stdin_is_tty: bool
    stdout_is_tty: bool
    args_resolved: dict[str, object]
    doctor_advisories: list[dict[str, Any]]
    jsonl: bool = False
    emitter: JsonlEmitter | None = None
    sol_already_keeps_journal: bool = False


@dataclass(frozen=True)
class StepResult:
    name: str
    status: StepStatus
    paths: list[str]
    started_at: str
    finished_at: str
    error: dict[str, object] | None
    reason: str | None = None


@dataclass(frozen=True)
class CleanUninstallContext:
    journal_path: Path
    service_path: Path
    wrapper_paths: tuple[Path, ...]
    config_path: Path
    manifest_path: Path
    yes: bool
    stdin_is_tty: bool
    curdir: Path


@dataclass(frozen=True)
class CleanUninstallStepResult:
    name: str
    state: CleanUninstallState
    path: Path | None
    reason: str | None = None


class SetupDeadEnd(Exception):
    def __init__(
        self,
        message: str,
        exit_code: int = 2,
        *,
        step_name: str | None = None,
        error_code: str | None = None,
    ) -> None:
        super().__init__(message)
        self.message = message
        self.exit_code = exit_code
        self.step_name = step_name
        self.error_code = error_code


def _journal_arg(value: str) -> Path:
    if not value or not value.strip():
        raise argparse.ArgumentTypeError("--journal must not be empty")
    path = Path(value).expanduser()
    try:
        resolved = path.resolve()
    except (OSError, RuntimeError) as exc:
        raise argparse.ArgumentTypeError(
            f"--journal could not be resolved: {exc}"
        ) from exc
    if resolved == Path.cwd().resolve():
        raise argparse.ArgumentTypeError("--journal must not be empty")
    return path


def _port_arg(value: str) -> int:
    try:
        port = int(value)
    except ValueError as exc:
        raise argparse.ArgumentTypeError(
            f"--port must be in 1024-65535 (got {value})"
        ) from exc
    if not 1024 <= port <= 65535:
        raise argparse.ArgumentTypeError(f"--port must be in 1024-65535 (got {port})")
    return port


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Set up solstone user-runtime artifacts and start the service.",
    )
    parser.add_argument(
        "--journal",
        metavar="PATH",
        type=_journal_arg,
        default=None,
        help="journal directory to persist in ~/.config/solstone/config.toml",
    )
    parser.add_argument(
        "--port",
        metavar="INT",
        type=_port_arg,
        default=5015,
        help="convey service port (default: 5015)",
    )
    parser.add_argument(
        "--variant",
        choices=("auto", "cpu", "cuda", "coreml"),
        default="auto",
        help="Parakeet model/runtime variant passed to journal install-models (default: auto)",
    )
    parser.add_argument(
        "--step-timeout-seconds",
        metavar="INT",
        type=int,
        default=1800,
        help=(
            "timeout for model, skill, and wrapper steps in seconds "
            "(default: 1800; doctor uses a separate 30s timeout)"
        ),
    )
    parser.add_argument(
        "-y",
        "--yes",
        "--non-interactive",
        dest="yes",
        action="store_true",
        help="run without prompts; fail with retry guidance when input is required",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="print the resolved plan and commands without changing files or services",
    )
    parser.add_argument(
        "--jsonl",
        action="store_true",
        help="Emit one-JSON-per-line events on stdout instead of human output.",
    )
    parser.add_argument(
        "--explain",
        action="store_true",
        help="print the setup steps and resolved defaults without running them",
    )
    parser.add_argument(
        "--skip-models",
        action="store_true",
        help="skip local model installation",
    )
    parser.add_argument(
        "--skip-brain",
        action="store_true",
        help="skip local brain bootstrap",
    )
    parser.add_argument(
        "--skip-skills",
        action="store_true",
        help="skip Claude Code skill installation",
    )
    parser.add_argument(
        "--skip-service",
        action="store_true",
        help="skip service installation, start, and health check",
    )
    parser.add_argument(
        "--skip-wrapper",
        action="store_true",
        help="skip provisioning the managed sol/journal wrappers",
    )
    parser.add_argument(
        "--accept-existing-journal",
        action="store_true",
        help="allow setup to use a non-empty existing journal directory",
    )
    parser.add_argument(
        "--force",
        action="store_true",
        help="re-run all steps unconditionally",
    )
    parser.add_argument(
        "--clean-uninstall",
        action="store_true",
        help="remove setup-managed runtime artifacts and exit",
    )
    return parser


def resolve_mode(args: argparse.Namespace) -> SetupMode:
    stdin_is_tty = sys.stdin.isatty()
    stdout_is_tty = sys.stdout.isatty()

    if args.jsonl:
        return SetupMode.NON_INTERACTIVE
    if args.explain:
        return SetupMode.EXPLAIN
    if args.dry_run:
        return SetupMode.DRY_RUN
    if args.yes:
        return SetupMode.NON_INTERACTIVE
    if stdin_is_tty and stdout_is_tty:
        return SetupMode.INTERACTIVE
    return SetupMode.NON_INTERACTIVE


def read_sol_already_keeps_journal() -> bool:
    """Return True when sol on this Mac already supervises this journal.

    Read-only and macOS-only. Fires on either signal: an installed app-owned
    child launcher, or sol running right now. Never writes.
    """
    if sys.platform != "darwin":
        return False
    try:
        from solstone.think import install_guard
        from solstone.think.service import inspect_supervisor_conflict

        for binary, alias in install_guard.alias_paths().items():
            if install_guard.is_live_app_owned_child_launcher(alias, binary):
                return True
        return inspect_supervisor_conflict().app_state == "running"
    except Exception as exc:
        print(
            "warning: setup could not determine whether sol already keeps this "
            f"journal; continuing with the normal install ({type(exc).__name__}: {exc})",
            file=sys.stderr,
        )
        return False


def resolve_context(
    args: argparse.Namespace,
    raw_argv: list[str],
    *,
    emitter: JsonlEmitter | None = None,
) -> SetupContext:
    mode = resolve_mode(args)
    project_root = Path(get_project_root())
    is_source_checkout = source_checkout()
    journal_path, journal_source = resolve_journal_path(args)
    cfg_path = config_path()
    manifest_path = journal_path / "health" / "setup-state.json"
    port_supplied = arg_supplied(raw_argv, "--port")
    step_timeout_supplied = arg_supplied(raw_argv, "--step-timeout-seconds")
    variant_supplied = arg_supplied(raw_argv, "--variant")
    skip_brain = bool(args.skip_brain or args.skip_models)
    if args.skip_brain:
        skip_brain_source = "cli:--skip-brain"
    elif args.skip_models:
        skip_brain_source = "cli:--skip-models"
    else:
        skip_brain_source = "default"

    args_resolved: dict[str, object] = {
        "journal": {
            "value": str(journal_path),
            "source": journal_source,
        },
        "port": {
            "value": args.port,
            "source": "cli" if port_supplied else "default",
        },
        "step_timeout_seconds": {
            "value": args.step_timeout_seconds,
            "source": "cli" if step_timeout_supplied else "default",
        },
        "variant": {
            "value": args.variant,
            "source": "cli" if variant_supplied else "default",
        },
        "yes": {"value": bool(args.yes), "source": "cli" if args.yes else "default"},
        "force": {
            "value": bool(args.force),
            "source": "cli" if args.force else "default",
        },
        "dry_run": {
            "value": bool(args.dry_run),
            "source": "cli" if args.dry_run else "default",
        },
        "jsonl": {
            "value": bool(args.jsonl),
            "source": "cli" if args.jsonl else "default",
        },
        "explain": {
            "value": bool(args.explain),
            "source": "cli" if args.explain else "default",
        },
        "skip_models": {
            "value": bool(args.skip_models),
            "source": "cli" if args.skip_models else "default",
        },
        "skip_brain": {
            "value": skip_brain,
            "source": skip_brain_source,
        },
        "skip_skills": {
            "value": bool(args.skip_skills),
            "source": "cli" if args.skip_skills else "default",
        },
        "skip_service": {
            "value": bool(args.skip_service),
            "source": "cli" if args.skip_service else "default",
        },
        "skip_wrapper": {
            "value": bool(args.skip_wrapper),
            "source": "cli" if args.skip_wrapper else "default",
        },
        "accept_existing_journal": {
            "value": bool(args.accept_existing_journal),
            "source": "cli" if args.accept_existing_journal else "default",
        },
        "journal_variant_env": {
            "value": os.environ.get("JOURNAL_VARIANT"),
            "source": "env",
        },
        "is_source_checkout": {
            "value": is_source_checkout,
            "source": "detected",
        },
    }

    ctx = SetupContext(
        mode=mode,
        project_root=project_root,
        is_source_checkout=is_source_checkout,
        journal_path=journal_path,
        journal_source=journal_source,
        config_path=cfg_path,
        manifest_path=manifest_path,
        port=args.port,
        port_source="cli" if port_supplied else "default",
        port_supplied=port_supplied,
        step_timeout_seconds=args.step_timeout_seconds,
        variant=args.variant,
        variant_source="cli" if variant_supplied else "default",
        yes=bool(args.yes),
        skip_models=bool(args.skip_models),
        skip_brain=skip_brain,
        skip_skills=bool(args.skip_skills),
        skip_service=bool(args.skip_service),
        skip_wrapper=bool(args.skip_wrapper),
        accept_existing_journal=bool(args.accept_existing_journal),
        force=bool(args.force),
        stdin_is_tty=sys.stdin.isatty(),
        stdout_is_tty=sys.stdout.isatty(),
        args_resolved=args_resolved,
        doctor_advisories=[],
        jsonl=bool(args.jsonl),
        emitter=emitter,
        sol_already_keeps_journal=read_sol_already_keeps_journal(),
    )
    return ctx


def resolve_journal_path(args: argparse.Namespace) -> tuple[Path, str]:
    if args.journal is not None:
        return expand_path(args.journal), "cli"

    env_path = os.environ.get("SOLSTONE_JOURNAL", "").strip()
    if env_path:
        return expand_path(env_path), "env"

    configured = read_user_config().get("journal", "").strip()
    if configured:
        return expand_path(configured), "config"

    return expand_path(default_journal()), "default"


def arg_supplied(raw_argv: list[str], flag: str) -> bool:
    return flag in raw_argv or any(item.startswith(f"{flag}=") for item in raw_argv)


def expand_path(path: str | Path) -> Path:
    return Path(path).expanduser().resolve()


def utc_now() -> str:
    return (
        datetime.now(timezone.utc)
        .replace(microsecond=0)
        .isoformat()
        .replace("+00:00", "Z")
    )


def absolute_string(path: Path) -> str:
    return str(path.expanduser().resolve())


def non_empty_journal(path: Path) -> bool:
    return path.is_dir() and (
        (path / "config").is_dir()
        or any(path.glob("*.jsonl"))
        or any(
            p.is_dir() and p.name.isdigit() and len(p.name) == 8 for p in path.iterdir()
        )
    )


def read_manifest(ctx: SetupContext) -> dict[str, Any] | None:
    try:
        return json.loads(ctx.manifest_path.read_text(encoding="utf-8"))
    except (FileNotFoundError, json.JSONDecodeError):
        return None


@dataclass(frozen=True)
class PriorRunStatus:
    state: str  # "none" | "clean" | "partial"
    timestamp: str | None
    failed_steps: tuple[str, ...]


def prior_run_status(ctx: SetupContext) -> PriorRunStatus:
    manifest = read_manifest(ctx)
    if manifest is None:
        return PriorRunStatus("none", None, ())
    steps = manifest.get("steps") or []
    failed = tuple(
        s.get("name", "<unknown>")
        for s in steps
        if s.get("status") not in ("ok", "skipped")
    )
    completed_at = manifest.get("completed_at")
    if completed_at and not failed:
        return PriorRunStatus("clean", completed_at, ())
    return PriorRunStatus("partial", manifest.get("started_at"), failed)


def prior_step_lookup(manifest: dict[str, Any]) -> dict[str, dict]:
    lookup = {}
    for step in manifest.get("steps", []):
        lookup[step["name"]] = step
    return lookup


def can_skip(prior_step: dict | None) -> bool:
    if prior_step is None or prior_step.get("status") != "ok":
        return False
    return all(Path(path).exists() for path in prior_step.get("paths", []))


def write_manifest(ctx: SetupContext, manifest: dict[str, Any]) -> None:
    try:
        ctx.manifest_path.parent.mkdir(parents=True, exist_ok=True)
        fd, tmp_name = tempfile.mkstemp(
            prefix=".tmp_setup_state",
            suffix=".json",
            dir=ctx.manifest_path.parent,
        )
        tmp_path = Path(tmp_name)
        try:
            with os.fdopen(fd, "w", encoding="utf-8") as handle:
                json.dump(manifest, handle, indent=2)
                handle.write("\n")
            os.replace(tmp_path, ctx.manifest_path)
        except Exception:
            tmp_path.unlink(missing_ok=True)
            raise
    except Exception as exc:
        logging.warning("could not write setup manifest: %s", exc)


def initial_manifest(ctx: SetupContext) -> dict[str, Any]:
    previous = read_manifest(ctx)
    if previous is not None:
        logging.debug("previous setup manifest found at %s", ctx.manifest_path)
    return {
        "schema_version": MANIFEST_SCHEMA_VERSION,
        "started_at": utc_now(),
        "completed_at": None,
        "mode": ctx.mode.value,
        "args_resolved": ctx.args_resolved,
        "steps": [],
    }


def append_step(manifest: dict[str, Any], result: StepResult) -> None:
    steps = manifest.setdefault("steps", [])
    steps.append(asdict(result))


def step_result(
    name: str,
    status: StepStatus,
    paths: list[Path | str],
    started_at: str,
    error: dict[str, object] | None = None,
    reason: str | None = None,
) -> StepResult:
    return StepResult(
        name=name,
        status=status,
        paths=[absolute_string(Path(path)) for path in paths],
        started_at=started_at,
        finished_at=utc_now(),
        error=error,
        reason=reason,
    )


def print_step_header(
    ctx: SetupContext, step_index: int, label: str, command: list[str] | None = None
) -> None:
    if command:
        narrate(
            ctx,
            f"[step {step_index}/{TOTAL_STEPS}] running {label}: {format_command(command)}",
        )
    else:
        narrate(ctx, f"[step {step_index}/{TOTAL_STEPS}] running {label}...")


def print_step_skipped(
    ctx: SetupContext, step_index: int, name: str, reason: str
) -> None:
    narrate(ctx, f"[step {step_index}/{TOTAL_STEPS}] skipped {name}: {reason}")


def format_command(command: list[str]) -> str:
    return " ".join(command)


def run_inherited(command: list[str], *, timeout: float | None = None) -> int:
    if timeout is None:
        result = subprocess.run(command, stdout=None, stderr=None, check=False)
        return int(result.returncode)
    proc = subprocess.Popen(command, stdout=None, stderr=None)
    try:
        return int(proc.wait(timeout=timeout))
    except subprocess.TimeoutExpired:
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait()
        raise


@dataclass(frozen=True)
class StepProcessResult:
    returncode: int
    stdout: str
    stderr: str
    timed_out: bool


def _timeout_output_text(value: str | bytes | None) -> str:
    if value is None:
        return ""
    if isinstance(value, bytes):
        return value.decode("utf-8", errors="replace")
    return value


def run_step_subprocess(
    ctx: SetupContext, command: list[str], *, timeout: float | None = None
) -> StepProcessResult:
    if not ctx.jsonl:
        try:
            rc = (
                run_inherited(command)
                if timeout is None
                else run_inherited(command, timeout=timeout)
            )
        except subprocess.TimeoutExpired:
            return StepProcessResult(1, "", "", True)
        return StepProcessResult(rc, "", "", False)
    try:
        proc = subprocess.run(
            command,
            capture_output=True,
            text=True,
            timeout=timeout,
            check=False,
        )
        return StepProcessResult(
            int(proc.returncode), proc.stdout or "", proc.stderr or "", False
        )
    except subprocess.TimeoutExpired as exc:
        return StepProcessResult(
            1,
            _timeout_output_text(exc.stdout),
            _timeout_output_text(exc.stderr),
            True,
        )


def tail_text(text: str, max_bytes: int = 8192) -> str:
    encoded = text.encode("utf-8", errors="replace")
    if len(encoded) <= max_bytes:
        return text
    return encoded[-max_bytes:].decode("utf-8", errors="replace")


def first_non_empty_line(text: str) -> str:
    for line in text.splitlines():
        stripped = line.strip()
        if stripped:
            return " ".join(stripped.split())
    return ""


def subprocess_error(
    step: str, result: StepProcessResult, *, timeout: float | None = None
) -> dict[str, object]:
    details_source = result.stderr or result.stdout
    if result.timed_out:
        message = (
            f"{step} step timed out after {timeout:g}s"
            if timeout is not None
            else f"{step} step timed out"
        )
        return {
            "code": "step_subprocess_timeout",
            "message": message,
            "details": tail_text(details_source),
            "exit_code": 1,
        }
    message = first_non_empty_line(result.stderr) or (
        f"{step} step exited with code {result.returncode}"
    )
    return {
        "code": "step_subprocess_failed",
        "message": message,
        "details": tail_text(details_source),
        "exit_code": int(result.returncode),
    }


@dataclass
class CappedTextBuffer:
    text: str = ""
    max_bytes: int = 8192

    def append(self, chunk: str) -> None:
        self.text = tail_text(self.text + chunk, self.max_bytes)


def setup_version() -> str:
    try:
        return _pkg_version("solstone")
    except PackageNotFoundError:
        return "0.0.0+source"


def elapsed_ms(started: float) -> int:
    return round((time.monotonic() - started) * 1000)


def require_emitter(ctx: SetupContext) -> JsonlEmitter:
    if ctx.emitter is None:
        raise RuntimeError("JSONL emitter is not configured")
    return ctx.emitter


def emit_step_started(
    ctx: SetupContext,
    step_name: str,
    step_index: int,
    *,
    command: list[str] | None = None,
) -> None:
    if not ctx.jsonl:
        return
    fields: dict[str, object] = {
        "step": step_name,
        "index": step_index,
        "total": TOTAL_STEPS,
    }
    if command is not None:
        fields["command"] = command
    require_emitter(ctx).emit("step.started", **fields)


def emit_step_result(
    ctx: SetupContext, result: StepResult, step_started: float
) -> None:
    if not ctx.jsonl:
        return
    emitter = require_emitter(ctx)
    duration_ms = elapsed_ms(step_started)
    if result.status in ("ok", "skipped", "warning"):
        fields: dict[str, object] = {
            "step": result.name,
            "outcome": "skipped" if result.status == "skipped" else "ok",
            "duration_ms": duration_ms,
        }
        if result.reason:
            fields["reason"] = result.reason
        emitter.emit("step.completed", **fields)
        if result.status == "warning":
            error = result.error or {}
            emitter.emit(
                "step.warning",
                step=result.name,
                text=str(error.get("message", "step warning"))[:512],
                fix_hint=str(error.get("fix_hint", "")),
            )
        return
    error = result.error or {}
    emitter.emit(
        "step.failed",
        step=result.name,
        duration_ms=duration_ms,
        error={
            "code": error.get("code", "setup_unhandled_exception"),
            "message": error.get("message", "step failed"),
            "details": error.get("details", ""),
            "exit_code": int(error.get("exit_code", 1)),
        },
    )


def doctor_command(ctx: SetupContext, *, jsonl: bool = False) -> list[str]:
    return [
        *journal_console_command(),
        "doctor",
        "--readiness",
        "--jsonl" if jsonl else "--json",
        "--port",
        str(ctx.port),
    ]


def journal_console_command() -> list[str]:
    return [str(Path(sys.executable).parent / "journal")]


def sol_console_command() -> list[str]:
    """Resolve the native `sol` console script beside the running interpreter.

    Journal-access commands (`skills`, `call`, ...) are owned by the sol surface.
    The Python journal dispatcher is retired. Setup shells out to the distinct
    native `sol` executable, exactly as `journal_console_command` selects the
    native journal reach for journal-service commands.
    """
    return [str(Path(sys.executable).parent / "sol")]


def install_models_command(ctx: SetupContext) -> list[str]:
    return [
        *journal_console_command(),
        "install-models",
        "--variant",
        ctx.variant,
    ]


def skills_user_command() -> list[str]:
    return [
        *sol_console_command(),
        "skills",
        "install",
        "--agent",
        "all",
    ]


def skills_journal_command(ctx: SetupContext) -> list[str]:
    return [
        *sol_console_command(),
        "skills",
        "install",
        "--project",
        str(ctx.journal_path),
        "--agent",
        "all",
    ]


def brain_bootstrap_command() -> list[str]:
    return [
        *sol_console_command(),
        "call",
        "thinking",
        "local",
        "bootstrap",
    ]


def brain_skip_reason(ctx: SetupContext) -> str:
    resolved = ctx.args_resolved.get("skip_brain")
    source = resolved.get("source") if isinstance(resolved, dict) else None
    if source == "cli:--skip-models":
        return "--skip-models implies --skip-brain"
    return "--skip-brain"


def service_install_command(ctx: SetupContext) -> list[str]:
    return [
        *journal_console_command(),
        "service",
        "install",
        "--port",
        str(ctx.port),
    ]


def _drain_stream_to_buffer(stream: Any, buffer: CappedTextBuffer) -> None:
    if stream is None:
        return
    for chunk in stream:
        buffer.append(str(chunk))


def _emit_dropped_doctor_line(ctx: SetupContext, line: str) -> None:
    require_emitter(ctx).emit(
        "step.warning",
        step="doctor",
        text=line[:512],
        fix_hint="",
    )


def _jsonl_advisory_to_doctor_payload(check: dict[str, Any]) -> dict[str, Any]:
    status = check.get("status")
    return {
        "name": check.get("name"),
        "severity": check.get("severity"),
        "status": "warn" if status == "warning" else status,
        "detail": check.get("detail", ""),
        "fix": check.get("fix", ""),
    }


def _process_doctor_stdout_line(
    ctx: SetupContext,
    line: str,
    advisories: list[dict[str, Any]],
    state: dict[str, Any],
) -> None:
    stripped = line.strip()
    if not stripped:
        return
    try:
        obj = json.loads(stripped)
    except json.JSONDecodeError:
        _emit_dropped_doctor_line(ctx, stripped)
        return
    event = obj.get("event")
    if event not in EVENT_TYPES or event not in DOCTOR_JSONL_EVENTS:
        _emit_dropped_doctor_line(ctx, stripped)
        return
    require_emitter(ctx).forward_line(line)
    if event == "doctor.completed":
        state["doctor_completed"] = obj
    elif (
        event == "check.completed"
        and obj.get("severity") == "advisory"
        and obj.get("status") in ("warning", "failed")
    ):
        advisories.append(obj)


def _drain_doctor_stdout(
    ctx: SetupContext,
    stream: Any,
    advisories: list[dict[str, Any]],
    state: dict[str, Any],
) -> None:
    if stream is None:
        return
    for line in stream:
        _process_doctor_stdout_line(ctx, str(line), advisories, state)


def step_doctor_jsonl(ctx: SetupContext, started_at: str) -> StepResult:
    command = doctor_command(ctx, jsonl=True)
    stderr_buffer = CappedTextBuffer()
    advisories: list[dict[str, Any]] = []
    state: dict[str, Any] = {"doctor_completed": None}
    try:
        proc = subprocess.Popen(
            command,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            bufsize=1,
        )
    except OSError as exc:
        return step_result(
            "doctor",
            "failed",
            [],
            started_at,
            {
                "code": "doctor_failed",
                "message": f"doctor failed to start: {exc}",
                "details": "",
                "exit_code": 1,
            },
        )

    stderr_thread = threading.Thread(
        target=_drain_stream_to_buffer,
        args=(proc.stderr, stderr_buffer),
        daemon=True,
    )
    stderr_thread.start()
    stdout_thread = threading.Thread(
        target=_drain_doctor_stdout,
        args=(ctx, proc.stdout, advisories, state),
        daemon=True,
    )
    stdout_thread.start()

    try:
        proc.wait(timeout=DOCTOR_TIMEOUT_SECONDS)
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.wait()
        stdout_thread.join(timeout=1)
        stderr_thread.join(timeout=1)
        return step_result(
            "doctor",
            "failed",
            [],
            started_at,
            {
                "code": "doctor_timeout",
                "message": f"doctor timed out after {DOCTOR_TIMEOUT_SECONDS}s",
                "details": stderr_buffer.text,
                "exit_code": 1,
            },
        )
    stdout_thread.join(timeout=1)
    stderr_thread.join(timeout=1)

    doctor_completed = state["doctor_completed"]
    if doctor_completed is None:
        return step_result(
            "doctor",
            "failed",
            [],
            started_at,
            {
                "code": "doctor_jsonl_incomplete",
                "message": "doctor JSONL stream ended without doctor.completed",
                "details": stderr_buffer.text,
                "exit_code": int(proc.returncode or 1),
            },
        )

    if doctor_completed.get("status") == "failed":
        return step_result(
            "doctor",
            "failed",
            [],
            started_at,
            {
                "code": "doctor_failed",
                "message": "doctor completed with status failed",
                "details": stderr_buffer.text,
                "exit_code": int(proc.returncode or 1),
            },
        )

    ctx.doctor_advisories[:] = [
        _jsonl_advisory_to_doctor_payload(item) for item in advisories
    ]
    for advisory in advisories:
        require_emitter(ctx).emit(
            "step.warning",
            step="doctor",
            text=advisory.get("detail") or advisory.get("name") or "",
            fix_hint=advisory.get("fix") or "",
        )
    return step_result("doctor", "ok", [], started_at)


def step_doctor(ctx: SetupContext, step_index: int) -> StepResult:
    started_at = utc_now()
    if ctx.jsonl:
        return step_doctor_jsonl(ctx, started_at)
    command = doctor_command(ctx)
    print_step_header(ctx, step_index, "doctor", command)
    try:
        result = subprocess.run(
            command,
            capture_output=True,
            text=True,
            check=False,
            timeout=DOCTOR_TIMEOUT_SECONDS,
        )
    except OSError as exc:
        return step_result(
            "doctor",
            "failed",
            [],
            started_at,
            {
                "code": "doctor_failed",
                "message": f"doctor failed to start: {exc}",
                "details": "",
                "exit_code": 1,
            },
        )
    except subprocess.TimeoutExpired:
        return step_result(
            "doctor",
            "failed",
            [],
            started_at,
            {
                "code": "doctor_timeout",
                "message": f"doctor timed out after {DOCTOR_TIMEOUT_SECONDS}s",
                "details": "",
                "exit_code": 1,
            },
        )
    if result.returncode != 0:
        if result.stdout:
            narrate(
                ctx, result.stdout, end="" if result.stdout.endswith("\n") else "\n"
            )
        if result.stderr:
            narrate(
                ctx,
                result.stderr,
                end="" if result.stderr.endswith("\n") else "\n",
                file=sys.stderr,
            )
        return step_result(
            "doctor",
            "failed",
            [],
            started_at,
            {
                "code": "doctor_failed",
                "message": "doctor blocker failed",
                "details": tail_text(result.stderr or result.stdout),
                "exit_code": int(result.returncode),
            },
        )

    try:
        payload = json.loads(result.stdout)
    except json.JSONDecodeError as exc:
        return step_result(
            "doctor",
            "failed",
            [],
            started_at,
            {
                "code": "doctor_jsonl_incomplete",
                "message": f"doctor JSON parse failed: {exc}",
                "details": tail_text(result.stdout),
                "exit_code": 1,
            },
        )

    checks = payload.get("checks", [])
    if isinstance(checks, list):
        ctx.doctor_advisories[:] = [
            check
            for check in checks
            if isinstance(check, dict)
            and check.get("severity") == "advisory"
            and check.get("status") in ("warn", "fail")
        ]
    narrate(ctx, f"[step {step_index}/{TOTAL_STEPS}] doctor passed")
    return step_result("doctor", "ok", [], started_at)


def step_journal(ctx: SetupContext, step_index: int) -> StepResult:
    started_at = utc_now()
    if ctx.journal_path.exists() and not ctx.journal_path.is_dir():
        dead_end_journal_is_file(ctx)
    print_step_header(ctx, step_index, "journal config")
    journal_config_path = ctx.journal_path / "config" / "journal.json"
    if ctx.mode is SetupMode.DRY_RUN:
        narrate(ctx, f"would materialize {journal_config_path}")
        return step_result(
            "journal",
            "ok",
            [ctx.config_path, ctx.journal_path, journal_config_path],
            started_at,
        )
    persisted = read_user_config().get("journal", "").strip()
    persisted_matches = bool(persisted) and expand_path(persisted) == ctx.journal_path
    if (
        non_empty_journal(ctx.journal_path)
        and not ctx.accept_existing_journal
        and not persisted_matches
    ):
        if ctx.mode is SetupMode.NON_INTERACTIVE:
            dead_end_existing_journal(ctx)
        if not prompt_accept_existing_journal(ctx.journal_path):
            raise SetupDeadEnd("setup aborted by user", 2)

    if not persisted_matches:
        ctx.journal_path.mkdir(parents=True, exist_ok=True)
        write_user_config(journal=str(ctx.journal_path))
        ensure_journal_config()
        narrate(ctx, f"[step {step_index}/{TOTAL_STEPS}] wrote {ctx.config_path}")
    else:
        narrate(
            ctx, f"[step {step_index}/{TOTAL_STEPS}] journal config already current"
        )
        ctx.journal_path.mkdir(parents=True, exist_ok=True)
        ensure_journal_config()
    return step_result(
        "journal",
        "ok",
        [ctx.config_path, ctx.journal_path, journal_config_path],
        started_at,
    )


def prompt_accept_existing_journal(path: Path) -> bool:
    answer = input(f"Use existing journal at {path}? [y/N]: ").strip().lower()
    return answer in {"y", "yes"}


def linux_model_paths(journal_path: Path) -> list[Path]:
    try:
        artifact_key = parakeet_readiness.parakeet_cpp_artifact_key()
    except RuntimeError:
        return []
    cache_root = parakeet_readiness.parakeet_cpp_cache_root(journal_path)
    return [
        parakeet_readiness.parakeet_cpp_binary_path(cache_root, artifact_key, "cpu"),
        parakeet_readiness.parakeet_cpp_binary_path(cache_root, artifact_key, "vulkan"),
        parakeet_readiness.parakeet_cpp_model_path(cache_root),
    ]


def mac_model_sentinel() -> Path:
    return (
        Path.home()
        / "Library"
        / "Application Support"
        / "solstone"
        / "parakeet"
        / "models"
        / ".install-complete"
    )


def model_paths(journal_path: Path | None = None) -> list[Path]:
    if sys.platform.startswith("linux"):
        return linux_model_paths(journal_path or default_journal())
    if sys.platform == "darwin":
        return [mac_model_sentinel()]
    return []


def step_install_models(ctx: SetupContext, step_index: int) -> StepResult:
    started_at = utc_now()
    if ctx.skip_models:
        print_step_skipped(ctx, step_index, "install_models", "--skip-models")
        return step_result(
            "install_models", "skipped", [], started_at, reason="--skip-models"
        )
    command = install_models_command(ctx)
    print_step_header(ctx, step_index, "install-models", command)
    result = run_step_subprocess(ctx, command, timeout=ctx.step_timeout_seconds)
    if result.timed_out:
        return step_result(
            "install_models",
            "failed",
            model_paths(ctx.journal_path),
            started_at,
            subprocess_error(
                "install_models", result, timeout=ctx.step_timeout_seconds
            ),
        )
    if result.returncode != 0:
        return step_result(
            "install_models",
            "failed",
            model_paths(ctx.journal_path),
            started_at,
            subprocess_error("install_models", result),
        )
    return step_result(
        "install_models", "ok", model_paths(ctx.journal_path), started_at
    )


def skills_user_paths() -> list[Path]:
    return [
        Path.home() / ".claude" / "skills" / "sol" / "SKILL.md",
        Path.home() / ".codex" / "skills" / "sol" / "SKILL.md",
        Path.home() / ".gemini" / "skills" / "sol" / "SKILL.md",
    ]


def skills_journal_paths(ctx: SetupContext) -> list[Path]:
    return [
        ctx.journal_path / ".claude" / "skills",
        ctx.journal_path / ".agents" / "skills",
    ]


def step_skills_user(ctx: SetupContext, step_index: int) -> StepResult:
    started_at = utc_now()
    paths = skills_user_paths()
    if ctx.skip_skills:
        print_step_skipped(ctx, step_index, "skills_user", "--skip-skills")
        return step_result(
            "skills_user", "skipped", [], started_at, reason="--skip-skills"
        )
    command = skills_user_command()
    print_step_header(ctx, step_index, "skills_user", command)
    result = run_step_subprocess(ctx, command, timeout=ctx.step_timeout_seconds)
    if result.timed_out:
        return step_result(
            "skills_user",
            "failed",
            paths,
            started_at,
            subprocess_error("skills_user", result, timeout=ctx.step_timeout_seconds),
        )
    if result.returncode != 0:
        return step_result(
            "skills_user",
            "failed",
            paths,
            started_at,
            subprocess_error("skills_user", result),
        )
    return step_result("skills_user", "ok", paths, started_at)


def step_skills_journal(ctx: SetupContext, step_index: int) -> StepResult:
    started_at = utc_now()
    paths = skills_journal_paths(ctx)
    if ctx.skip_skills:
        print_step_skipped(ctx, step_index, "skills_journal", "--skip-skills")
        return step_result(
            "skills_journal", "skipped", [], started_at, reason="--skip-skills"
        )
    command = skills_journal_command(ctx)
    print_step_header(ctx, step_index, "skills_journal", command)
    result = run_step_subprocess(ctx, command, timeout=ctx.step_timeout_seconds)
    if result.timed_out:
        return step_result(
            "skills_journal",
            "failed",
            paths,
            started_at,
            subprocess_error(
                "skills_journal", result, timeout=ctx.step_timeout_seconds
            ),
        )
    if result.returncode != 0:
        return step_result(
            "skills_journal",
            "failed",
            paths,
            started_at,
            subprocess_error("skills_journal", result),
        )
    return step_result("skills_journal", "ok", paths, started_at)


# OWNS alias repair (the mutation) via install_guard.provision_wrappers; doctor
# only reports stale aliases.
def step_wrapper(ctx: SetupContext, step_index: int) -> StepResult:
    started_at = utc_now()
    if ctx.skip_wrapper:
        print_step_skipped(ctx, step_index, "wrapper", "--skip-wrapper")
        return step_result(
            "wrapper", "skipped", [], started_at, reason="--skip-wrapper"
        )

    from solstone.think import install_guard

    aliases = install_guard.alias_paths()
    wrapper_paths = list(aliases.values())
    print_step_header(ctx, step_index, "wrapper")
    try:
        install_guard.provision_wrappers(Path(ctx.project_root), str(ctx.journal_path))
        for message in install_guard._ensure_user_bin_on_path(aliases["sol"].parent):
            narrate(ctx, message)
        return step_result("wrapper", "ok", wrapper_paths, started_at)
    except Exception as exc:
        message = (
            "could not provision the sol/journal wrappers at "
            f"{aliases['sol'].parent} ({type(exc).__name__}: {exc})"
        )
        fix_hint = (
            "fix permissions on ~/.local/bin and re-run `journal setup`, "
            "or invoke sol/journal directly from the runtime"
        )
        narrate(ctx, f"warning: {message} — {fix_hint}", file=sys.stderr)
        return step_result(
            "wrapper",
            "warning",
            wrapper_paths,
            started_at,
            {"message": message, "fix_hint": fix_hint},
        )


def service_artifact_path() -> Path | None:
    if sys.platform == "darwin":
        return Path.home() / "Library" / "LaunchAgents" / "org.solpbc.solstone.plist"
    if sys.platform.startswith("linux"):
        return Path.home() / ".config" / "systemd" / "user" / "solstone.service"
    return None


def step_service(ctx: SetupContext, step_index: int) -> StepResult:
    started_at = utc_now()
    artifact = service_artifact_path()
    paths = [artifact] if artifact is not None else []
    if ctx.skip_service:
        print_step_skipped(ctx, step_index, "service", "--skip-service")
        return step_result(
            "service", "skipped", [], started_at, reason="--skip-service"
        )
    # Escape hatch: journal service install remains available deliberately; this
    # guard only skips setup's automatic background launcher install.
    if ctx.sol_already_keeps_journal:
        print_step_skipped(ctx, step_index, "service", SOL_ALREADY_KEEPS_JOURNAL_REASON)
        narrate(ctx, SOL_ALREADY_KEEPS_JOURNAL_NARRATION)
        return step_result(
            "service",
            "skipped",
            [],
            started_at,
            reason=SOL_ALREADY_KEEPS_JOURNAL_REASON,
        )
    command = service_install_command(ctx)
    print_step_header(ctx, step_index, "service install", command)
    result = run_step_subprocess(ctx, command, timeout=None)
    if result.returncode != 0:
        return step_result(
            "service",
            "failed",
            paths,
            started_at,
            subprocess_error("service", result),
        )

    from solstone.think.service import _up

    narrate(ctx, f"[step {step_index}/{TOTAL_STEPS}] running service up...")
    up_rc = int(_up())
    if up_rc != 0:
        return step_result(
            "service",
            "failed",
            paths,
            started_at,
            {
                "code": "service_up_failed",
                "message": f"service up failed (exit {up_rc})",
                "details": "",
                "exit_code": 1,
            },
        )

    return step_result("service", "ok", paths, started_at)


def step_brain(ctx: SetupContext, step_index: int) -> StepResult:
    started_at = utc_now()
    from solstone.think.journal_config import (
        JournalConfigMutation,
        mutate_journal_config,
    )

    reason_malformed = "provider config is not in the expected shape"

    def apply(config: dict[str, Any]) -> JournalConfigMutation[str | None]:
        if "providers" in config and not isinstance(config["providers"], dict):
            return JournalConfigMutation(changed=False, value=reason_malformed)

        providers = config.get("providers", {})
        active = providers.get("active")
        if active is not None and not isinstance(active, dict):
            return JournalConfigMutation(changed=False, value=reason_malformed)

        owner_chose_other = isinstance(active, dict) and active.get("provider") not in {
            None,
            "local",
        }
        if owner_chose_other:
            return JournalConfigMutation(
                changed=False,
                value="a provider is already configured",
            )

        changed = False
        if "providers" not in config:
            config["providers"] = providers
            changed = True
        from solstone.think.models import LOCAL_MODEL

        local_active = {"provider": "local", "model": LOCAL_MODEL}
        if active != local_active:
            providers["active"] = local_active
            changed = True
        return JournalConfigMutation(changed=changed, value=None)

    result = mutate_journal_config(apply, journal_path=ctx.journal_path)
    if result.value is not None:
        print_step_skipped(ctx, step_index, "brain", result.value)
        return step_result("brain", "skipped", [], started_at, reason=result.value)

    if ctx.skip_brain:
        reason = brain_skip_reason(ctx)
        print_step_skipped(ctx, step_index, "brain", reason)
        return step_result("brain", "skipped", [], started_at, reason=reason)

    if ctx.skip_service:
        reason = "--skip-service"
        print_step_skipped(ctx, step_index, "brain", reason)
        return step_result("brain", "skipped", [], started_at, reason=reason)

    if ctx.sol_already_keeps_journal:
        reason = SOL_ALREADY_KEEPS_JOURNAL_REASON
        print_step_skipped(ctx, step_index, "brain", reason)
        return step_result("brain", "skipped", [], started_at, reason=reason)

    from solstone.think.check import build_check_report

    verdict = build_check_report()
    if verdict.report.overall == "blocked":
        reason = "local provider unavailable on this host"
        print_step_skipped(ctx, step_index, "brain", reason)
        return step_result("brain", "skipped", [], started_at, reason=reason)

    command = brain_bootstrap_command()
    print_step_header(ctx, step_index, "brain", command)
    result = run_step_subprocess(ctx, command, timeout=ctx.step_timeout_seconds)
    if result.returncode == 0 and not result.timed_out:
        return step_result("brain", "ok", [], started_at)

    from solstone.think.providers import local_install

    error = subprocess_error("brain", result, timeout=ctx.step_timeout_seconds)
    error["fix_hint"] = local_install.install_hint()
    return step_result(
        "brain",
        "warning",
        [],
        started_at,
        error,
        reason="local bootstrap did not start",
    )


CLEAN_UNINSTALL_TOTAL_STEPS = 4
CLEAN_UNINSTALL_STEP_NAMES = ("service", "wrapper", "config", "manifest")


def _service_path_for_uninstall() -> Path:
    from solstone.think.service import SERVICE_LABEL, SYSTEMD_UNIT

    if sys.platform == "darwin":
        return Path.home() / "Library" / "LaunchAgents" / f"{SERVICE_LABEL}.plist"
    if sys.platform.startswith("linux"):
        return Path.home() / ".config" / "systemd" / "user" / f"{SYSTEMD_UNIT}.service"
    raise RuntimeError(f"unsupported platform: {sys.platform}")


def _resolve_journal_for_uninstall() -> Path:
    env_path = os.environ.get("SOLSTONE_JOURNAL", "").strip()
    configured = read_user_config().get("journal", "").strip()
    return expand_path(env_path or configured or default_journal())


def resolve_clean_uninstall_context(args: argparse.Namespace) -> CleanUninstallContext:
    from solstone.think import install_guard

    journal_path = _resolve_journal_for_uninstall()
    return CleanUninstallContext(
        journal_path=journal_path,
        service_path=_service_path_for_uninstall(),
        wrapper_paths=tuple(install_guard.alias_paths().values()),
        config_path=config_path(),
        manifest_path=journal_path / "health" / "setup-state.json",
        yes=bool(args.yes),
        stdin_is_tty=sys.stdin.isatty(),
        curdir=Path.cwd().resolve(),
    )


def _run_service_uninstall() -> int:
    from solstone.think.service import _uninstall

    return int(_uninstall())


def _path_present_for_uninstall(path: Path) -> bool:
    return path.is_symlink() or path.exists()


def _existence_marker(path: Path) -> str:
    return "present" if _path_present_for_uninstall(path) else "absent"


def _clean_uninstall_paths(ctx: CleanUninstallContext) -> tuple[Path, ...]:
    return (
        ctx.service_path,
        *ctx.wrapper_paths,
        ctx.config_path,
        ctx.manifest_path,
    )


def _print_clean_confirmation(ctx: CleanUninstallContext) -> bool:
    print("journal setup --clean-uninstall will remove these runtime artifacts:")
    print()
    print(f"  [{_existence_marker(ctx.service_path):<7}] service: {ctx.service_path}")
    for wrapper_path in ctx.wrapper_paths:
        print(f"  [{_existence_marker(wrapper_path):<7}] wrapper: {wrapper_path}")
    print(f"  [{_existence_marker(ctx.config_path):<7}] config: {ctx.config_path}")
    print(
        f"  [{_existence_marker(ctx.manifest_path):<7}] manifest: {ctx.manifest_path}"
    )
    print()
    print("will not remove:")
    print(f"  - journal directory: {ctx.journal_path}")
    print("  - /Applications/solstone.app")
    print("  - ~/Library/Application Support/solstone/")
    print("  - macOS microphone or screen recording permissions")
    print("  - the python package")
    print()
    try:
        answer = input("proceed? [y/N]: ").strip().lower()
    except EOFError:
        print("cancelled")
        return False
    if answer not in {"y", "yes"}:
        print("cancelled")
        return False
    return True


def _print_clean_step_header(index: int, name: str) -> None:
    print(f"[step {index}/{CLEAN_UNINSTALL_TOTAL_STEPS}] running {name} uninstall...")


def _print_clean_step_result(index: int, result: CleanUninstallStepResult) -> None:
    prefix = (
        f"[step {index}/{CLEAN_UNINSTALL_TOTAL_STEPS}] {result.state} {result.name}"
    )
    if result.state in {"removed", "already-absent"}:
        print(f"{prefix}: {result.path}")
        return
    print(f"{prefix}: {result.reason or ''}")


def _clean_failed_reason(exc: Exception) -> str:
    message = str(exc)
    return f"{exc.__class__.__name__}: {message}" if message else exc.__class__.__name__


def _clean_uninstall_service(ctx: CleanUninstallContext) -> CleanUninstallStepResult:
    pre_exists = _path_present_for_uninstall(ctx.service_path)
    try:
        with (
            contextlib.redirect_stdout(io.StringIO()),
            contextlib.redirect_stderr(io.StringIO()),
        ):
            rc = _run_service_uninstall()
    except Exception as exc:
        return CleanUninstallStepResult(
            "service", "failed", ctx.service_path, _clean_failed_reason(exc)
        )
    if rc != 0:
        return CleanUninstallStepResult(
            "service", "failed", ctx.service_path, f"service uninstall exited {rc}"
        )
    if pre_exists:
        try:
            ctx.service_path.unlink(missing_ok=True)
        except OSError as exc:
            return CleanUninstallStepResult(
                "service", "failed", ctx.service_path, _clean_failed_reason(exc)
            )
    state: CleanUninstallState = "removed" if pre_exists else "already-absent"
    return CleanUninstallStepResult("service", state, ctx.service_path)


def _clean_uninstall_wrapper(ctx: CleanUninstallContext) -> CleanUninstallStepResult:
    from solstone.think import install_guard

    primary_path = ctx.wrapper_paths[0] if ctx.wrapper_paths else None
    pre_exists = any(_path_present_for_uninstall(path) for path in ctx.wrapper_paths)
    try:
        with (
            contextlib.redirect_stdout(io.StringIO()),
            contextlib.redirect_stderr(io.StringIO()),
        ):
            rc = install_guard.cmd_uninstall(ctx.curdir)
    except Exception as exc:
        return CleanUninstallStepResult(
            "wrapper", "failed", primary_path, _clean_failed_reason(exc)
        )
    if rc == 0:
        state: CleanUninstallState = "removed" if pre_exists else "already-absent"
        return CleanUninstallStepResult("wrapper", state, primary_path)

    state = install_guard.AliasState.ABSENT
    target = None
    blocked_path = primary_path
    for binary, path in zip(install_guard.alias_paths(), ctx.wrapper_paths):
        state, target = install_guard.check_alias(ctx.curdir, binary)
        if state not in {
            install_guard.AliasState.ABSENT,
            install_guard.AliasState.OWNED,
        }:
            blocked_path = path
            break
    if state is install_guard.AliasState.WORKTREE:
        return CleanUninstallStepResult(
            "wrapper",
            "skipped",
            blocked_path,
            "refusing to act from a git worktree",
        )
    if state is install_guard.AliasState.CROSS_REPO:
        return CleanUninstallStepResult(
            "wrapper",
            "skipped",
            blocked_path,
            f"alias points at {target}, not removing",
        )
    if state is install_guard.AliasState.DANGLING:
        return CleanUninstallStepResult(
            "wrapper",
            "skipped",
            blocked_path,
            f"alias is dangling (target {target} missing), not removing",
        )
    if state is install_guard.AliasState.FOREIGN:
        return CleanUninstallStepResult(
            "wrapper",
            "skipped",
            blocked_path,
            "alias is not a managed symlink, not removing",
        )
    return CleanUninstallStepResult(
        "wrapper", "failed", blocked_path, f"unexpected alias state: {state.name}"
    )


def _clean_uninstall_path(name: str, path: Path) -> CleanUninstallStepResult:
    if not _path_present_for_uninstall(path):
        return CleanUninstallStepResult(name, "already-absent", path)
    try:
        path.unlink()
    except OSError as exc:
        return CleanUninstallStepResult(name, "failed", path, _clean_failed_reason(exc))
    return CleanUninstallStepResult(name, "removed", path)


def run_clean_uninstall(ctx: CleanUninstallContext) -> int:
    if not any(
        _path_present_for_uninstall(path) for path in _clean_uninstall_paths(ctx)
    ):
        print("nothing to remove (all paths already absent)")
        return 0

    if not ctx.yes:
        if not ctx.stdin_is_tty:
            print(
                "not a tty; rerun with --yes to proceed non-interactively (cancelled)"
            )
            return 0
        if not _print_clean_confirmation(ctx):
            return 0

    results: list[CleanUninstallStepResult] = []
    for index, name in enumerate(CLEAN_UNINSTALL_STEP_NAMES, start=1):
        _print_clean_step_header(index, name)
        if name == "service":
            result = _clean_uninstall_service(ctx)
        elif name == "wrapper":
            result = _clean_uninstall_wrapper(ctx)
        elif name == "config":
            result = _clean_uninstall_path("config", ctx.config_path)
        else:
            result = _clean_uninstall_path("manifest", ctx.manifest_path)
        results.append(result)
        _print_clean_step_result(index, result)

    counts = {state: 0 for state in ("removed", "already-absent", "skipped", "failed")}
    for result in results:
        counts[result.state] += 1
    print(
        "clean uninstall complete: "
        f"{counts['removed']} removed, "
        f"{counts['already-absent']} already-absent, "
        f"{counts['skipped']} skipped, "
        f"{counts['failed']} failed"
    )
    return 1 if counts["failed"] else 0


def dead_end_existing_journal(ctx: SetupContext) -> None:
    message = "\n".join(
        [
            (
                "journal setup: cannot proceed in non-interactive mode - "
                f"{ctx.journal_path} already contains journal data."
            ),
            "Setup will not auto-claim an existing journal.",
            "",
            "Retry with one of:",
            "  journal setup --accept-existing-journal",
            "  journal setup --journal /path/to/new-journal --accept-existing-journal",
            "",
            "Interactive escape:",
            "  journal setup",
            "",
            "Run 'journal setup --explain' for full step list.",
        ]
    )
    raise SetupDeadEnd(
        message,
        2,
        step_name="journal",
        error_code="journal_existing_blocked",
    )


def dead_end_journal_is_file(ctx: SetupContext) -> None:
    message = (
        f"expected a directory at {ctx.journal_path}; got a regular file. "
        "Re-run with --journal <other-path>."
    )
    raise SetupDeadEnd(
        message,
        2,
        step_name="journal",
        error_code="journal_dir_invalid",
    )


def _print_plan_step_header(
    ctx: SetupContext,
    step_index: int,
    step: Callable[[SetupContext, int], StepResult],
) -> None:
    suffix = _plan_step_suffix(ctx, step)
    narrate(ctx, f"[step {step_index}/{TOTAL_STEPS}] {_STEP_NAME[step]}{suffix}")


def _plan_step_suffix(
    ctx: SetupContext,
    step: Callable[[SetupContext, int], StepResult],
) -> str:
    if step is step_skills_user:
        return " - installs the sol skill for claude / codex / gemini"
    if step is step_skills_journal:
        return (
            " - installs talent skills into "
            f"{ctx.journal_path}/.{{claude,agents}}/skills"
        )
    return ""


def _print_doctor_plan(ctx: SetupContext) -> None:
    narrate(ctx, f"  would run: {format_command(doctor_command(ctx))}")


def _print_journal_plan(ctx: SetupContext) -> None:
    narrate(ctx, f"  would write: {ctx.config_path}")
    narrate(ctx, f"  would use journal: {ctx.journal_path}")


def _print_install_models_plan(ctx: SetupContext) -> None:
    if ctx.skip_models:
        narrate(ctx, "  skipped: --skip-models")
    else:
        narrate(ctx, f"  would run: {format_command(install_models_command(ctx))}")


def _print_skills_user_plan(ctx: SetupContext) -> None:
    if ctx.skip_skills:
        narrate(ctx, "  skipped: --skip-skills")
    else:
        narrate(ctx, f"  would run: {format_command(skills_user_command())}")


def _print_skills_journal_plan(ctx: SetupContext) -> None:
    if ctx.skip_skills:
        narrate(ctx, "  skipped: --skip-skills")
    else:
        narrate(ctx, f"  would run: {format_command(skills_journal_command(ctx))}")


def _print_wrapper_plan(ctx: SetupContext) -> None:
    if ctx.skip_wrapper:
        narrate(ctx, "  skipped: --skip-wrapper")
    else:
        narrate(ctx, "  would provision managed sol and journal wrappers in-process")


def _print_service_plan(ctx: SetupContext) -> None:
    if ctx.skip_service:
        narrate(ctx, "  skipped: --skip-service")
    elif ctx.sol_already_keeps_journal:
        narrate(ctx, f"  would skip: {SOL_ALREADY_KEEPS_JOURNAL_REASON}")
    else:
        narrate(ctx, f"  would run: {format_command(service_install_command(ctx))}")
        narrate(ctx, "  would call: solstone.think.service._up()")


def _print_brain_plan(ctx: SetupContext) -> None:
    narrate(ctx, "  would set local provider lane if unset")
    if ctx.skip_brain:
        narrate(ctx, f"  skipped: {brain_skip_reason(ctx)}")
        return
    if ctx.skip_service:
        narrate(ctx, "  skipped: --skip-service")
        return
    if ctx.sol_already_keeps_journal:
        narrate(ctx, f"  would skip: {SOL_ALREADY_KEEPS_JOURNAL_REASON}")
        return
    narrate(ctx, "  would check local provider fit")
    narrate(ctx, f"  would run: {format_command(brain_bootstrap_command())}")


def print_plan(ctx: SetupContext, *, dry_run: bool) -> None:
    heading = "setup dry-run" if dry_run else "setup plan"
    narrate(ctx, f"{heading}:")
    narrate(ctx, f"  mode: {ctx.mode.value}")
    narrate(ctx, f"  journal: {ctx.journal_path} ({ctx.journal_source})")
    narrate(ctx, f"  port: {ctx.port} ({ctx.port_source})")
    narrate(ctx, f"  variant: {ctx.variant} ({ctx.variant_source})")
    timeout_resolved = ctx.args_resolved["step_timeout_seconds"]
    timeout_source = timeout_resolved["source"]
    narrate(
        ctx, f"  step_timeout_seconds: {ctx.step_timeout_seconds} ({timeout_source})"
    )
    narrate(ctx, f"  source checkout: {ctx.is_source_checkout}")
    narrate(ctx)
    for step_index, step in enumerate(_STEPS, start=1):
        renderer = _PLAN_BODY.get(step)
        if renderer is None:
            name = _STEP_NAME.get(step, getattr(step, "__name__", repr(step)))
            raise RuntimeError(f"missing setup plan body for {name}")
        _print_plan_step_header(ctx, step_index, step)
        renderer(ctx)


def print_failure(ctx: SetupContext, result: StepResult) -> None:
    error = result.error or {}
    message = error.get("message", "step failed")
    narrate(ctx, f"journal setup: {result.name} failed: {message}", file=sys.stderr)


def print_success_summary(ctx: SetupContext, manifest: dict[str, Any]) -> None:
    narrate(ctx)
    narrate(ctx, "solstone is set up.")
    narrate(ctx)
    steps = manifest.get("steps", [])
    n_skipped_prior = sum(1 for step in steps if step.get("reason") == "prior_run_ok")
    n_skipped_other = sum(
        1
        for step in steps
        if step.get("status") == "skipped" and step.get("reason") != "prior_run_ok"
    )
    n_ran = TOTAL_STEPS - n_skipped_prior - n_skipped_other
    narrate(ctx, f"{n_skipped_prior} of {TOTAL_STEPS} steps already done; ran {n_ran}")
    narrate(ctx)
    narrate(ctx, "artifacts:")
    paths = artifact_paths(ctx, manifest)
    if paths:
        for path in paths:
            narrate(ctx, f"  {path}")
    else:
        narrate(ctx, "  none")
    narrate(ctx)
    if ctx.doctor_advisories:
        narrate(ctx, "advisories from doctor:")
        for advisory in ctx.doctor_advisories:
            detail = advisory.get("detail")
            if detail:
                narrate(ctx, f"  - {detail}")
    else:
        narrate(ctx, "advisories from doctor: none")
    narrate(ctx)
    warning_steps = [step for step in steps if step.get("status") == "warning"]
    if warning_steps:
        narrate(ctx, "warnings:")
        for step in warning_steps:
            error = step.get("error") or {}
            message = error.get("message")
            fix_hint = error.get("fix_hint")
            if message and fix_hint:
                narrate(ctx, f"  - {message} — {fix_hint}")
            elif message:
                narrate(ctx, f"  - {message}")
        narrate(ctx)
    if not ctx.skip_service and not ctx.sol_already_keeps_journal:
        narrate(ctx, f"solstone is running at http://localhost:{ctx.port}")
        narrate(ctx)
    narrate(
        ctx,
        "next: install an observer — see INSTALL.md for the macOS app bundle or the pipx path for linux and tmux.",
    )


def artifact_paths(ctx: SetupContext, manifest: dict[str, Any]) -> list[str]:
    seen: set[str] = set()
    paths: list[str] = []
    for step in manifest.get("steps", []):
        if not isinstance(step, dict):
            continue
        for item in step.get("paths", []):
            if not isinstance(item, str) or item in seen:
                continue
            seen.add(item)
            paths.append(item)
    manifest_path = absolute_string(ctx.manifest_path)
    if (
        ctx.mode not in (SetupMode.DRY_RUN, SetupMode.EXPLAIN)
        and manifest_path not in seen
    ):
        paths.append(manifest_path)
    return paths


def print_prior_run_preface(ctx: SetupContext) -> None:
    status = prior_run_status(ctx)
    if status.state == "none":
        return
    if status.state == "clean":
        suffix = (
            "re-running all steps (--force)."
            if ctx.force
            else "verifying current state."
        )
        narrate(ctx, f"journal setup last ran cleanly on {status.timestamp}; {suffix}")
        if not ctx.force:
            narrate(ctx, "Use --force to re-run all steps unconditionally.")
        return
    narrate(
        ctx,
        f"journal setup last run on {status.timestamp} left these steps incomplete:",
    )
    for name in status.failed_steps:
        narrate(ctx, f"  - {name} (failed)")
    narrate(ctx, "Re-running will verify state and re-run incomplete steps.")


def _resume_service(
    ctx: SetupContext, step_index: int, prior_step: dict
) -> StepResult | None:
    started_at = utc_now()
    if ctx.sol_already_keeps_journal and not ctx.skip_service:
        print_step_skipped(ctx, step_index, "service", SOL_ALREADY_KEEPS_JOURNAL_REASON)
        narrate(ctx, SOL_ALREADY_KEEPS_JOURNAL_NARRATION)
        return step_result(
            "service",
            "skipped",
            [],
            started_at,
            reason=SOL_ALREADY_KEEPS_JOURNAL_REASON,
        )

    from solstone.think.service import service_is_installed

    if not service_is_installed():
        return None

    from solstone.think.health_cli import health_check

    paths = prior_step.get("paths", [])
    if health_check() == 0:
        return step_result(
            "service", "skipped", paths, started_at, reason="prior_run_ok"
        )

    narrate(
        ctx,
        f"[step {step_index}/{TOTAL_STEPS}] service installed but unhealthy; restarting...",
    )
    run_step_subprocess(
        ctx,
        [*journal_console_command(), "service", "restart"],
        timeout=None,
    )
    if health_check() == 0:
        return step_result(
            "service",
            "ok",
            paths,
            started_at,
            reason="resumed_after_restart",
        )
    return None


_STEP_NAME: dict[Callable[[SetupContext, int], StepResult], str] = {
    step_doctor: "doctor",
    step_journal: "journal",
    step_install_models: "install_models",
    step_skills_user: "skills_user",
    step_skills_journal: "skills_journal",
    step_wrapper: "wrapper",
    step_service: "service",
    step_brain: "brain",
}

_STEPS: tuple[Callable[[SetupContext, int], StepResult], ...] = (
    step_doctor,
    step_journal,
    step_install_models,
    step_skills_user,
    step_skills_journal,
    step_wrapper,
    step_service,
    step_brain,
)


TOTAL_STEPS = len(_STEPS)


_PLAN_BODY: dict[
    Callable[[SetupContext, int], StepResult], Callable[[SetupContext], None]
] = {
    step_doctor: _print_doctor_plan,
    step_journal: _print_journal_plan,
    step_install_models: _print_install_models_plan,
    step_skills_user: _print_skills_user_plan,
    step_skills_journal: _print_skills_journal_plan,
    step_wrapper: _print_wrapper_plan,
    step_service: _print_service_plan,
    step_brain: _print_brain_plan,
}


_CONTINUE_AFTER_FAILURE: frozenset[str] = frozenset({"skills_user", "skills_journal"})


def command_for_step(
    ctx: SetupContext, step: Callable[[SetupContext, int], StepResult]
) -> list[str] | None:
    if step is step_doctor:
        return doctor_command(ctx, jsonl=ctx.jsonl)
    if step is step_install_models:
        return install_models_command(ctx)
    if step is step_skills_user:
        return skills_user_command()
    if step is step_skills_journal:
        return skills_journal_command(ctx)
    if step is step_service:
        if ctx.sol_already_keeps_journal and not ctx.skip_service:
            return None
        return service_install_command(ctx)
    if step is step_brain:
        return brain_bootstrap_command()
    return None


def emit_setup_completed(
    ctx: SetupContext,
    *,
    status: str,
    started_monotonic: float,
    failed_step: str | None = None,
) -> None:
    if not ctx.jsonl:
        return
    fields: dict[str, object] = {
        "status": status,
        "duration_ms": elapsed_ms(started_monotonic),
    }
    if failed_step is not None:
        fields["failed_step"] = failed_step
    require_emitter(ctx).emit("setup.completed", **fields)


def run_setup(ctx: SetupContext, *, started_monotonic: float | None = None) -> int:
    setup_started = (
        started_monotonic if started_monotonic is not None else time.monotonic()
    )
    if ctx.jsonl:
        require_emitter(ctx).emit(
            "setup.started",
            started_at=utc_now_iso(),
            version=setup_version(),
            mode=ctx.mode.value,
            args_resolved=ctx.args_resolved,
        )
    explain = bool(ctx.args_resolved["explain"]["value"])
    dry_run = bool(ctx.args_resolved["dry_run"]["value"])
    if explain:
        if ctx.jsonl:
            emit_setup_completed(ctx, status="ok", started_monotonic=setup_started)
            return 0
        print_plan(ctx, dry_run=False)
        return 0
    if dry_run:
        if ctx.jsonl:
            emit_setup_completed(ctx, status="ok", started_monotonic=setup_started)
            return 0
        print_plan(ctx, dry_run=True)
        return 0

    if ctx.journal_path.exists() and not ctx.journal_path.is_dir():
        emit_step_started(ctx, "journal", 2)
        dead_end_journal_is_file(ctx)

    print_prior_run_preface(ctx)
    prior_manifest = read_manifest(ctx) or {}
    prior = {} if ctx.force else prior_step_lookup(prior_manifest)
    manifest = initial_manifest(ctx)
    aggregate_failed: list[StepResult] = []
    for index, step in enumerate(_STEPS, start=1):
        step_name = _STEP_NAME[step]
        prior_step = prior.get(step_name)
        started_at = utc_now()
        step_started = time.monotonic()
        emit_step_started(ctx, step_name, index, command=command_for_step(ctx, step))
        try:
            if can_skip(prior_step):
                if step is step_service:
                    result = _resume_service(ctx, index, prior_step)
                    if result is None:
                        result = step(ctx, index)
                else:
                    result = step_result(
                        step_name,
                        "skipped",
                        prior_step.get("paths", []),
                        started_at,
                        reason="prior_run_ok",
                    )
            else:
                result = step(ctx, index)
        except SetupDeadEnd:
            raise
        except Exception as exc:
            result = step_result(
                step_name,
                "failed",
                [],
                started_at,
                {
                    "code": "setup_unhandled_exception",
                    "message": first_non_empty_line(str(exc)) or exc.__class__.__name__,
                    "details": str(exc),
                    "exit_code": 1,
                },
            )
            append_step(manifest, result)
            write_manifest(ctx, manifest)
            emit_step_result(ctx, result, step_started)
            print_failure(ctx, result)
            if result.name in _CONTINUE_AFTER_FAILURE:
                aggregate_failed.append(result)
                continue
            emit_setup_completed(
                ctx,
                status="failed",
                started_monotonic=setup_started,
                failed_step=result.name,
            )
            return 1
        append_step(manifest, result)
        write_manifest(ctx, manifest)
        emit_step_result(ctx, result, step_started)
        if result.status == "failed":
            print_failure(ctx, result)
            error = result.error or {}
            if result.name in _CONTINUE_AFTER_FAILURE:
                aggregate_failed.append(result)
                continue
            emit_setup_completed(
                ctx,
                status="failed",
                started_monotonic=setup_started,
                failed_step=result.name,
            )
            return int(error.get("exit_code", 1))

    if aggregate_failed:
        emit_setup_completed(
            ctx,
            status="failed",
            started_monotonic=setup_started,
            failed_step=aggregate_failed[0].name,
        )
        return max(
            int((result.error or {}).get("exit_code", 1)) for result in aggregate_failed
        )

    manifest["completed_at"] = utc_now()
    write_manifest(ctx, manifest)
    print_success_summary(ctx, manifest)
    emit_setup_completed(ctx, status="ok", started_monotonic=setup_started)
    return 0


def main(argv: list[str] | None = None) -> int:
    raw_argv = list(argv) if argv is not None else sys.argv[1:]
    parser = build_parser()
    args = parser.parse_args(raw_argv)
    if args.clean_uninstall:
        if args.jsonl:
            parser.error(
                "JSONL output is not supported for --clean-uninstall in this version."
            )
        incompatible: list[str] = []
        if args.journal is not None:
            incompatible.append("--journal")
        for flag in ("--port", "--variant", "--step-timeout-seconds"):
            if arg_supplied(raw_argv, flag):
                incompatible.append(flag)
        if args.dry_run:
            incompatible.append("--dry-run")
        if args.explain:
            incompatible.append("--explain")
        if args.skip_models:
            incompatible.append("--skip-models")
        if args.skip_brain:
            incompatible.append("--skip-brain")
        if args.skip_skills:
            incompatible.append("--skip-skills")
        if args.skip_service:
            incompatible.append("--skip-service")
        if args.skip_wrapper:
            incompatible.append("--skip-wrapper")
        if args.accept_existing_journal:
            incompatible.append("--accept-existing-journal")
        if args.force:
            incompatible.append("--force")
        if incompatible:
            parser.error(
                "--clean-uninstall cannot be combined with " + ", ".join(incompatible)
            )
        return run_clean_uninstall(resolve_clean_uninstall_context(args))
    ctx: SetupContext | None = None
    started_monotonic = time.monotonic()
    try:
        emitter = JsonlEmitter(sys.stdout) if args.jsonl else None
        ctx = resolve_context(args, raw_argv, emitter=emitter)
        return run_setup(ctx, started_monotonic=started_monotonic)
    except SetupDeadEnd as exc:
        if ctx is not None and ctx.jsonl and exc.step_name and exc.error_code:
            require_emitter(ctx).emit(
                "step.failed",
                step=exc.step_name,
                duration_ms=0,
                error={
                    "code": exc.error_code,
                    "message": exc.message,
                    "details": "",
                    "exit_code": exc.exit_code,
                },
            )
            emit_setup_completed(
                ctx,
                status="failed",
                started_monotonic=started_monotonic,
                failed_step=exc.step_name,
            )
        else:
            print(exc.message, file=sys.stderr)
        return exc.exit_code


if __name__ == "__main__":
    sys.exit(main())
