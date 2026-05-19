# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Shared helpers for observer installers."""

from __future__ import annotations

import json
import re
import socket
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Literal, Sequence

from solstone.apps.observer.utils import find_observer_by_name, list_observers
from solstone.think.utils import read_service_port

Platform = Literal["linux", "tmux", "macos", "unsupported"]

STREAM_RE = re.compile(r"^[a-z0-9][a-z0-9._-]*$")
SERVICE_UNITS = {
    "linux": "solstone-linux.service",
    "tmux": "solstone-tmux.service",
}


class InstallError(Exception):
    """Install failure with an optional operator hint."""

    def __init__(self, message: str, *, hint: str | None = None, code: int = 1):
        super().__init__(message)
        self.hint = hint
        self.code = code


@dataclass(frozen=True)
class ObserverRecord:
    record: dict
    key: str
    prefix: str


@dataclass(frozen=True)
class StepResult:
    process: subprocess.CompletedProcess | None
    skipped: bool = False


def detect_platform(override: str | None = None) -> Platform:
    """Detect the observer platform."""
    if override in {"linux", "tmux", "macos"}:
        return override  # type: ignore[return-value]
    if sys.platform == "darwin":
        return "macos"
    if sys.platform.startswith("linux"):
        return "linux"
    return "unsupported"


def default_server_url(explicit: str | None = None) -> str:
    """Return the explicit or locally discovered solstone server URL."""
    if explicit:
        return explicit
    port = read_service_port("convey")
    if port is None:
        raise InstallError(
            "could not determine solstone server URL",
            hint=(
                "start solstone with 'sol up' or pass --server-url "
                "http://127.0.0.1:<port>"
            ),
        )
    return f"http://127.0.0.1:{port}"


def default_stream(
    platform: Literal["linux", "tmux"], override: str | None = None
) -> str:
    """Return the normalized observer stream name."""
    raw = override or socket.gethostname()
    if override is None and platform == "tmux":
        raw = f"{raw}-tmux"
    return normalize_stream_name(raw)


def normalize_stream_name(value: str) -> str:
    """Normalize a stream name to the observer name regex."""
    normalized = re.sub(r"[^a-z0-9._-]+", "-", value.strip().lower())
    normalized = re.sub(r"^[^a-z0-9]+", "", normalized)
    if not normalized or not STREAM_RE.match(normalized):
        raise InstallError(
            f"invalid observer name: {value}",
            hint="use lowercase letters, numbers, dots, underscores, or hyphens",
        )
    return normalized


def install_root() -> Path:
    """Return the observer install root."""
    return Path.home() / ".local" / "share" / "solstone" / "observers"


def xdg_install_dir(install_name: str) -> Path:
    """Return the per-observer state directory that holds the install marker."""
    return install_root() / install_name


def marker_path(install_name: str) -> Path:
    """Return the marker path for an observer package."""
    return xdg_install_dir(install_name) / ".installed.json"


def read_marker(install_name: str) -> dict | None:
    """Read an observer install marker."""
    path = marker_path(install_name)
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (FileNotFoundError, json.JSONDecodeError, OSError):
        return None
    return data if isinstance(data, dict) else None


def write_marker(install_name: str, data: dict) -> None:
    """Write an observer install marker."""
    path = marker_path(install_name)
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp_path = path.with_suffix(".json.tmp")
    tmp_path.write_text(json.dumps(data, indent=2) + "\n", encoding="utf-8")
    tmp_path.replace(path)


def find_marker_for_observer(name: str) -> tuple[Path, dict] | None:
    """Find the marker associated with a stream name."""
    root = install_root()
    if not root.exists():
        return None
    for path in root.glob("*/.installed.json"):
        try:
            data = json.loads(path.read_text(encoding="utf-8"))
        except (json.JSONDecodeError, OSError):
            continue
        if isinstance(data, dict) and data.get("name") == name:
            return path, data
    return None


def _active_observers_by_name(name: str) -> list[dict]:
    return [
        observer
        for observer in list_observers()
        if observer.get("name") == name and not observer.get("revoked", False)
    ]


def create_or_reuse_registration(name: str, *, force: bool) -> ObserverRecord:
    """Return an active observer registration, creating one when needed."""
    from solstone.observe.observer_cli import (
        create_observer_record,
        revoke_observer_record,
    )

    if force:
        for observer in _active_observers_by_name(name):
            revoke_observer_record(observer.get("key", "")[:8])
        record, key, _ = create_observer_record(name, permit_duplicate_name=True)
        return ObserverRecord(record=record, key=key, prefix=key[:8])

    active = _active_observers_by_name(name)
    if active:
        key = active[0].get("key")
        if not key:
            raise InstallError(
                f"observer registration for {name} is missing its key",
                hint="run sol observer install --force to recreate it",
            )
        return ObserverRecord(record=active[0], key=key, prefix=key[:8])

    permit_duplicate = find_observer_by_name(name) is not None
    record, key, _ = create_observer_record(
        name, permit_duplicate_name=permit_duplicate
    )
    return ObserverRecord(record=record, key=key, prefix=key[:8])


def run_step(
    label: str,
    cmd: Sequence[str],
    *,
    cwd: Path | None = None,
    dry_run: bool = False,
    capture: bool = False,
    stream: bool = True,
    json_output: bool = False,
    check: bool = True,
) -> StepResult:
    """Run an install step with consistent human output."""
    if dry_run:
        if not json_output:
            print(f"would {label}")
        return StepResult(process=None, skipped=True)

    effective_capture = capture or json_output or not stream
    if not json_output:
        print(f"→ {label}")

    try:
        process = subprocess.run(
            list(cmd),
            cwd=cwd,
            check=False,
            capture_output=effective_capture,
            text=True,
        )
    except OSError as exc:
        if not json_output:
            print(f"✗ {label} failed", file=sys.stderr)
        raise InstallError(f"{label} failed", hint=str(exc)) from exc

    if check and process.returncode != 0:
        hint = _first_output_line(process.stderr) or _first_output_line(process.stdout)
        if not json_output:
            print(f"✗ {label} failed", file=sys.stderr)
        raise InstallError(f"{label} failed", hint=hint)

    if not json_output:
        print("✓ done")
    return StepResult(process=process)


def pipx_install(
    package_name: str,
    version: str,
    *,
    system_site_packages: bool,
    json_output: bool,
    dry_run: bool,
) -> StepResult:
    """Install or refresh a pipx package by exact version.

    Always passes ``--force`` so re-installing the same version is a no-op
    for pipx (it would otherwise error). On linux, ``system_site_packages``
    is required so the venv can see PyGObject / GStreamer from the distro.
    """
    cmd = ["pipx", "install", "--force"]
    if system_site_packages:
        cmd.append("--system-site-packages")
    cmd.append(f"{package_name}=={version}")
    return run_step(
        f"install {package_name}=={version}",
        cmd,
        json_output=json_output,
        dry_run=dry_run,
    )


def run_probe(
    cmd: Sequence[str], *, cwd: Path | None = None
) -> subprocess.CompletedProcess:
    """Run a predicate command without raising."""
    try:
        return subprocess.run(
            list(cmd),
            cwd=cwd,
            check=False,
            capture_output=True,
            text=True,
        )
    except OSError as exc:
        return subprocess.CompletedProcess(list(cmd), 127, "", str(exc))


def poll_status_until(
    name: str, timeout: float = 30.0, interval: float = 2.0
) -> Literal["connected", "disconnected", "revoked", "missing"]:
    """Poll existing observer status until connected or timeout."""
    from solstone.observe.observer_cli import _status_label

    deadline = time.monotonic() + timeout
    last_status: Literal["connected", "disconnected", "revoked", "missing"] = "missing"
    while True:
        observer = find_observer_by_name(name)
        if observer is None:
            last_status = "missing"
        else:
            status = _status_label(observer)
            if status in {"connected", "revoked"}:
                return status  # type: ignore[return-value]
            last_status = "disconnected"
        if time.monotonic() >= deadline:
            return last_status
        time.sleep(interval)


def print_summary(result: dict) -> None:
    """Print a human summary for an install result."""
    if result.get("status") == "error":
        print(f"Error: {result.get('error')}", file=sys.stderr)
        if result.get("hint"):
            print(result["hint"], file=sys.stderr)
        return

    if result.get("status") == "redirected":
        return

    if result.get("status") == "already_installed":
        print(f"{result.get('service_unit')} is already installed.")
    else:
        print("Observer install complete:")
    print(f"  Name:       {result.get('name')}")
    print(f"  Service:    {result.get('service_unit')}")
    print(f"  Config:     {result.get('config_path')}")
    print(f"  Marker:     {result.get('marker_path')}")
    print(f"  Status:     {result.get('status')}")
    print(f"  Version:    {result.get('version')}")


def emit_json(result: dict) -> None:
    """Emit a single JSON result object."""
    print(json.dumps(result, indent=2))


def _first_output_line(value: str | None) -> str | None:
    if not value:
        return None
    for line in value.splitlines():
        if line.strip():
            return line.strip()
    return None


def observer_key_prefix_from_config(config_path: Path) -> str | None:
    """Read the configured key prefix if a config file exists."""
    try:
        data = json.loads(config_path.read_text(encoding="utf-8"))
    except (FileNotFoundError, json.JSONDecodeError, OSError):
        return None
    key = data.get("key")
    return key[:8] if isinstance(key, str) and key else None
