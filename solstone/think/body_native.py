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
BODY_INGEST_TIMEOUT_SECONDS = 6 * 60 * 60
_REBUILD_RESULT_SCHEMA = "solstone.body.rebuild.result.v1"
_INGEST_RESULT_SCHEMA = "solstone.body.ingest.result.v1"


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
    if not isinstance(payload, dict) or payload.get("schema") != _REBUILD_RESULT_SCHEMA:
        raise BodyNativeError("native body rebuild returned an unknown result")
    for field in ("native_bundles", "legacy_bundles", "rows"):
        value = payload.get(field)
        if type(value) is not int or value < 0:
            raise BodyNativeError("native body rebuild returned invalid counts")
    return payload


def apple_health(
    source: str | Path,
    journal: str | Path,
    *,
    save: bool,
    confirm_body_save: bool = False,
    date_from: str | None = None,
    date_to: str | None = None,
    force: bool = False,
    handshake_checker: Callable[
        ..., core_handshake.CoreHandshakeResult
    ] = core_handshake.check_solstone_core_handshake,
    helper_locator: Callable[[], Path] = core_handshake.helper_path_for_executable,
    runner: Callable[..., subprocess.CompletedProcess[str]] = subprocess.run,
) -> dict[str, Any]:
    """Preview or save an Apple export through the native body owner."""

    handshake = handshake_checker()
    if handshake.status != "ok":
        detail = handshake.message or "native body owner is unavailable"
        raise BodyNativeError(detail)

    argv = [
        str(helper_locator()),
        "body",
        "apple",
        "--source",
        str(source),
        "--journal",
        str(journal),
        "--json",
    ]
    if date_from is not None:
        argv.extend(("--date-from", date_from))
    if date_to is not None:
        argv.extend(("--date-to", date_to))
    if force:
        argv.append("--force")
    if save:
        argv.append("--save")
        if confirm_body_save:
            argv.append("--confirm-body-save")
    try:
        completed = runner(
            argv,
            capture_output=True,
            text=True,
            timeout=BODY_INGEST_TIMEOUT_SECONDS,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        raise BodyNativeError("native Apple body import could not complete") from exc
    if completed.returncode != 0:
        detail = completed.stderr.strip()[:512] or f"exit {completed.returncode}"
        raise BodyNativeError(f"native Apple body import failed: {detail}")
    try:
        payload: Any = json.loads(completed.stdout)
    except (TypeError, json.JSONDecodeError) as exc:
        raise BodyNativeError("native Apple body import returned invalid JSON") from exc
    if (
        not isinstance(payload, dict)
        or payload.get("schema") != _INGEST_RESULT_SCHEMA
        or payload.get("source") != "apple_health"
        or payload.get("mode") != ("save" if save else "preview")
        or type(payload.get("rows")) is not int
        or payload["rows"] < 0
        or type(payload.get("skipped")) is not bool
        or not isinstance(payload.get("days"), list)
        or not all(isinstance(day, str) for day in payload["days"])
        or not (
            payload.get("bundle_id") is None
            or isinstance(payload.get("bundle_id"), str)
        )
    ):
        raise BodyNativeError("native Apple body import returned an unknown result")
    return payload


def detect_apple_health(
    source: str | Path,
    *,
    handshake_checker: Callable[
        ..., core_handshake.CoreHandshakeResult
    ] = core_handshake.check_solstone_core_handshake,
    helper_locator: Callable[[], Path] = core_handshake.helper_path_for_executable,
    runner: Callable[..., subprocess.CompletedProcess[str]] = subprocess.run,
) -> bool:
    """Classify an Apple ZIP through the native bounded archive reader."""

    payload = _run_body_helper(
        ["body", "apple", "--source", str(source), "--detect", "--json"],
        operation="Apple body source detection",
        handshake_checker=handshake_checker,
        helper_locator=helper_locator,
        runner=runner,
    )
    if (
        payload.get("schema") != "solstone.body.apple.detect.result.v1"
        or type(payload.get("apple_health")) is not bool
    ):
        raise BodyNativeError("native Apple body detection returned an unknown result")
    return payload["apple_health"]


def oura_connect(
    journal: str | Path,
    *,
    handshake_checker: Callable[
        ..., core_handshake.CoreHandshakeResult
    ] = core_handshake.check_solstone_core_handshake,
    helper_locator: Callable[[], Path] = core_handshake.helper_path_for_executable,
    runner: Callable[..., subprocess.CompletedProcess[str]] = subprocess.run,
) -> dict[str, Any]:
    """Run owner-present Oura authorization through the native body owner."""

    payload = _run_body_helper(
        ["body", "oura", "connect", "--journal", str(journal), "--json"],
        operation="Oura authorization",
        handshake_checker=handshake_checker,
        helper_locator=helper_locator,
        runner=runner,
    )
    if (
        payload.get("schema") != "solstone.body.oura.connect.result.v1"
        or payload.get("connected") is not True
        or not isinstance(payload.get("scopes"), list)
        or not all(isinstance(scope, str) for scope in payload["scopes"])
    ):
        raise BodyNativeError("native Oura authorization returned an unknown result")
    return payload


def oura_sync(
    journal: str | Path,
    *,
    save: bool,
    confirm_body_save: bool = False,
    scheduled: bool = False,
    window_days: int | None = None,
    handshake_checker: Callable[
        ..., core_handshake.CoreHandshakeResult
    ] = core_handshake.check_solstone_core_handshake,
    helper_locator: Callable[[], Path] = core_handshake.helper_path_for_executable,
    runner: Callable[..., subprocess.CompletedProcess[str]] = subprocess.run,
) -> dict[str, Any]:
    """Preview or save an Oura sync through the native body owner."""

    argv = ["body", "oura", "sync", "--journal", str(journal), "--json"]
    if window_days is not None:
        argv.extend(("--window-days", str(window_days)))
    if save:
        argv.append("--save")
        if confirm_body_save:
            argv.append("--confirm-body-save")
        if scheduled:
            argv.append("--scheduled")
    payload = _run_body_helper(
        argv,
        operation="Oura body sync",
        handshake_checker=handshake_checker,
        helper_locator=helper_locator,
        runner=runner,
    )
    if (
        payload.get("schema") != _INGEST_RESULT_SCHEMA
        or payload.get("source") != "oura_api"
        or payload.get("mode") != ("save" if save else "preview")
        or type(payload.get("rows")) is not int
        or payload["rows"] < 0
        or type(payload.get("pages")) is not int
        or payload["pages"] < 0
        or type(payload.get("skipped")) is not bool
        or not isinstance(payload.get("days"), list)
        or not all(isinstance(day, str) for day in payload["days"])
        or not isinstance(payload.get("endpoint_counts"), dict)
        or not all(
            isinstance(endpoint, str) and type(count) is int and count >= 0
            for endpoint, count in payload["endpoint_counts"].items()
        )
        or not isinstance(payload.get("issues"), list)
        or not all(
            isinstance(issue, dict)
            and isinstance(issue.get("endpoint"), str)
            and issue.get("kind") == "permission"
            for issue in payload["issues"]
        )
        or not (
            payload.get("bundle_id") is None
            or isinstance(payload.get("bundle_id"), str)
        )
    ):
        raise BodyNativeError("native Oura body sync returned an unknown result")
    return payload


def _run_body_helper(
    arguments: list[str],
    *,
    operation: str,
    handshake_checker: Callable[..., core_handshake.CoreHandshakeResult],
    helper_locator: Callable[[], Path],
    runner: Callable[..., subprocess.CompletedProcess[str]],
) -> dict[str, Any]:
    handshake = handshake_checker()
    if handshake.status != "ok":
        detail = handshake.message or "native body owner is unavailable"
        raise BodyNativeError(detail)
    try:
        completed = runner(
            [str(helper_locator()), *arguments],
            capture_output=True,
            text=True,
            timeout=BODY_INGEST_TIMEOUT_SECONDS,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        raise BodyNativeError(f"native {operation} could not complete") from exc
    if completed.returncode != 0:
        detail = completed.stderr.strip()[:512] or f"exit {completed.returncode}"
        raise BodyNativeError(f"native {operation} failed: {detail}")
    try:
        payload: Any = json.loads(completed.stdout)
    except (TypeError, json.JSONDecodeError) as exc:
        raise BodyNativeError(f"native {operation} returned invalid JSON") from exc
    if not isinstance(payload, dict):
        raise BodyNativeError(f"native {operation} returned an unknown result")
    return payload


__all__ = [
    "BODY_REBUILD_TIMEOUT_SECONDS",
    "BODY_INGEST_TIMEOUT_SECONDS",
    "BodyNativeError",
    "apple_health",
    "oura_connect",
    "oura_sync",
    "rebuild_body_store",
]
