# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Process adapter for the native body-history owner."""

from __future__ import annotations

import json
import subprocess
from collections.abc import Callable
from pathlib import Path
from typing import Any

from solstone.think import core_handshake

BODY_REBUILD_TIMEOUT_SECONDS = 6 * 60 * 60
_RESULT_SCHEMA = "solstone.body.rebuild.result.v1"


class BodyNativeError(RuntimeError):
    """The native body owner could not complete a requested operation."""


def rebuild_body_store(
    journal: str | Path,
    *,
    handshake_checker: Callable[
        ..., core_handshake.CoreHandshakeResult
    ] = core_handshake.check_solstone_core_handshake,
    helper_locator: Callable[[], Path] = core_handshake.helper_path_for_executable,
    runner: Callable[..., subprocess.CompletedProcess[str]] = subprocess.run,
) -> dict[str, int | str]:
    """Reconstruct body dedupe state through ``solstone-core body rebuild``."""

    handshake = handshake_checker()
    if handshake.status != "ok":
        detail = handshake.message or "native body owner is unavailable"
        raise BodyNativeError(detail)

    argv = [
        str(helper_locator()),
        "body",
        "rebuild",
        "--journal",
        str(journal),
        "--json",
    ]
    try:
        completed = runner(
            argv,
            capture_output=True,
            text=True,
            timeout=BODY_REBUILD_TIMEOUT_SECONDS,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        raise BodyNativeError("native body rebuild could not complete") from exc
    if completed.returncode != 0:
        detail = completed.stderr.strip()[:512] or f"exit {completed.returncode}"
        raise BodyNativeError(f"native body rebuild failed: {detail}")
    try:
        payload: Any = json.loads(completed.stdout)
    except (TypeError, json.JSONDecodeError) as exc:
        raise BodyNativeError("native body rebuild returned invalid JSON") from exc
    if not isinstance(payload, dict) or payload.get("schema") != _RESULT_SCHEMA:
        raise BodyNativeError("native body rebuild returned an unknown result")
    for field in ("native_bundles", "legacy_bundles", "rows"):
        value = payload.get(field)
        if type(value) is not int or value < 0:
            raise BodyNativeError("native body rebuild returned invalid counts")
    return payload


__all__ = [
    "BODY_REBUILD_TIMEOUT_SECONDS",
    "BodyNativeError",
    "rebuild_body_store",
]
