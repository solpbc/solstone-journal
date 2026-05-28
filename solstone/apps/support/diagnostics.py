# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Diagnostic collector for support tickets.

Gathers system state — version, OS, active services, recent errors, and
configuration (secrets stripped) — for the ``user_context`` field on support
tickets.  All collection is local; nothing is transmitted.
"""

from __future__ import annotations

import json
import logging
import os
import platform
from datetime import datetime, timedelta
from pathlib import Path
from typing import Any

logger = logging.getLogger(__name__)

# 168h = 7 days.
RECENCY_WINDOW_HOURS = 168

# Config keys that must never leave the device.
_SECRET_KEYS = frozenset(
    {
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
        "GOOGLE_API_KEY",
        "REVAI_ACCESS_TOKEN",
        "PLAUD_ACCESS_TOKEN",
        "password",
        "secret",
        "token",
        "key",
    }
)


def _is_secret_key(key: str) -> bool:
    """Return True if *key* looks like it holds sensitive data."""
    lower = key.lower()
    return any(s in lower for s in ("key", "token", "secret", "password"))


def _strip_secrets(obj: Any) -> Any:
    """Recursively redact values whose keys look secret."""
    if isinstance(obj, dict):
        return {
            k: "***" if _is_secret_key(k) else _strip_secrets(v) for k, v in obj.items()
        }
    if isinstance(obj, list):
        return [_strip_secrets(v) for v in obj]
    return obj


# -- Individual collectors ---------------------------------------------------


def collect_version() -> str | None:
    """Return the installed solstone version string."""
    try:
        from importlib.metadata import version

        return version("solstone")
    except Exception:
        return None


def collect_platform() -> dict[str, str]:
    """Return OS / platform info."""
    return {
        "system": platform.system(),
        "release": platform.release(),
        "machine": platform.machine(),
        "python": platform.python_version(),
    }


def collect_services() -> dict[str, str]:
    """Check which solstone services are running.

    Looks at PID files under ``journal/health/``.
    """
    from solstone.think.utils import get_journal

    journal = get_journal()
    health_dir = Path(journal) / "health"
    if not health_dir.is_dir():
        return {}

    statuses: dict[str, str] = {}
    for pid_file in health_dir.glob("*.pid"):
        service = pid_file.stem
        try:
            pid = int(pid_file.read_text().strip())
            # Check if process is alive
            os.kill(pid, 0)
            statuses[service] = "running"
        except (ValueError, ProcessLookupError, PermissionError):
            statuses[service] = "stopped"
        except OSError:
            statuses[service] = "unknown"

    return statuses


def collect_recent_errors(limit: int = 10) -> list[dict[str, Any]]:
    """Return recent ERROR log lines within the recency window, newest-first."""
    from solstone.think.utils import get_journal

    journal = get_journal()
    health_dir = Path(journal) / "health"
    if not health_dir.is_dir():
        return []

    cutoff = datetime.now() - timedelta(hours=RECENCY_WINDOW_HOURS)
    candidates: list[tuple[datetime, dict[str, Any]]] = []
    for log_file in health_dir.glob("*.log"):
        file_mtime: datetime | None = None
        try:
            lines = log_file.read_text(errors="replace").splitlines()
            for line in lines:
                if "ERROR" not in line:
                    continue

                parts = line.split(maxsplit=1)
                try:
                    if not parts:
                        raise ValueError
                    dt = datetime.fromisoformat(parts[0])
                    approx = False
                    # Head slice is intentional: the prefix carries ERROR details.
                    message = (parts[1] if len(parts) > 1 else "").strip()[:500]
                except ValueError:
                    if file_mtime is None:
                        file_mtime = datetime.fromtimestamp(log_file.stat().st_mtime)
                    dt = file_mtime
                    approx = True
                    message = line.strip()[:500]

                if dt < cutoff:
                    continue

                candidates.append(
                    (
                        dt,
                        {
                            "service": log_file.stem,
                            "message": message,
                            "time": dt.isoformat(timespec="seconds"),
                            "time_approximate": approx,
                        },
                    )
                )
        except OSError:
            continue

    candidates.sort(key=lambda c: c[0], reverse=True)
    return [entry for _, entry in candidates[:limit]]


def collect_config() -> dict[str, Any]:
    """Return journal config with secrets stripped."""
    from solstone.think.utils import get_journal

    journal = get_journal()
    config_path = Path(journal) / "config" / "config.json"
    if not config_path.is_file():
        return {}

    try:
        config = json.loads(config_path.read_text())
        return _strip_secrets(config)
    except (json.JSONDecodeError, OSError):
        return {}


# -- Public API --------------------------------------------------------------


def collect_all() -> dict[str, Any]:
    """Gather all diagnostics and return as a JSON-serialisable dict.

    This is the value for the ``user_context`` field on support tickets.
    The user sees *exactly* this dict before approving submission.
    """
    diagnostics: dict[str, Any] = {}

    try:
        diagnostics["version"] = collect_version()
    except Exception as exc:
        logger.debug("version collection failed: %s", exc)

    try:
        diagnostics["platform"] = collect_platform()
    except Exception as exc:
        logger.debug("platform collection failed: %s", exc)

    try:
        diagnostics["services"] = collect_services()
    except Exception as exc:
        logger.debug("service collection failed: %s", exc)

    try:
        diagnostics["recent_errors"] = collect_recent_errors()
    except Exception as exc:
        logger.debug("error collection failed: %s", exc)

    try:
        diagnostics["config"] = collect_config()
    except Exception as exc:
        logger.debug("config collection failed: %s", exc)

    return diagnostics


def collect_all_json() -> str:
    """Convenience: return :func:`collect_all` as a formatted JSON string."""
    return json.dumps(collect_all(), indent=2, default=str)
