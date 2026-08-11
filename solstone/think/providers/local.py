# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Local provider backed by bundled llama-server or a configured endpoint.

The module must remain importable before the local runtime or GGUF files exist.
Network clients and daemon startup are created only inside provider functions.
"""

from __future__ import annotations

import asyncio
import logging
import time
import traceback
import uuid
from collections.abc import Callable
from dataclasses import dataclass
from typing import Any

from solstone.think.models import LOCAL_MODEL
from solstone.think.providers.local_endpoint import (
    classify_byo_cogitate_error,
    local_endpoint_reason_copy,
    redact_local_endpoint_credential,
    resolve_local_endpoint,
)
from solstone.think.providers.shared import (
    PROVIDER_ERROR_TEXT_CAP_CHARS,
    classify_provider_error,
    safe_raw,
)

LOG = logging.getLogger(__name__)

_LOCAL_PREFIX = "local/"
# Qwen3.5-4B runaway repetition emits duplicate array entries until the context
# wall. llama.cpp's GBNF converter honors maxItems, so bounded arrays force
# grammar-friendly closure with finish_reason="stop" and valid (if bloated) JSON.
# 192 is 2.4x the largest observed sense.entities[] length (80, n=1128);
# downstream dedupe absorbs the slack.
# llama.cpp's GBNF converter turns string length limits into repetition counts
# that can exceed its grammar parser limit, and it mistranslates pattern anchors
# into literal characters. Drop these request-side only; canonical validation
# still enforces them after generation.
# Unknown x-* handling by llama.cpp, mlx-vlm, and llguidance is unproven. Strip
# Solstone annotations before a schema reaches any local structured-output path.
# Qwen3.5-4B model card sampling recommendations. The card explicitly warns
# against greedy / near-greedy decoding, which drives runaway repetition on
# entity-rich extractions. presence_penalty is the vendor-sanctioned
# anti-repetition lever; we do not touch repeat_penalty or enable DRY/XTC.
_LOCAL_CAPACITY_EXHAUSTED_MESSAGE = (
    "The local model was busy and could not finish this request. Try again in a moment."
)


@dataclass(frozen=True)
class LocalModelSpec:
    model_id: str
    repo: str
    filename: str
    revision: str
    sha256: str
    size_bytes: int
    min_ram_bytes: int
    mmproj_filename: str | None = None
    mmproj_sha256: str | None = None
    mmproj_size_bytes: int | None = None


LOCAL_MODEL_SPECS: dict[str, LocalModelSpec] = {
    LOCAL_MODEL: LocalModelSpec(
        model_id=LOCAL_MODEL,
        repo="unsloth/Qwen3.5-4B-GGUF",
        filename="Qwen3.5-4B-Q4_K_M.gguf",
        revision="main",
        sha256="00fe7986ff5f6b463e62455821146049db6f9313603938a70800d1fb69ef11a4",
        size_bytes=2740937888,
        min_ram_bytes=8 * 1024**3,
        mmproj_filename="mmproj-F16.gguf",
        mmproj_sha256="cd88edcf8d031894960bb0c9c5b9b7e1fea6ebee02b9f7ce925a00d12891f864",
        mmproj_size_bytes=672423616,
    ),
}


class LocalProviderError(RuntimeError):
    """Local provider failure with a recovery reason code."""

    def __init__(self, reason_code: str | None, message: str) -> None:
        super().__init__(message)
        self.reason_code = reason_code


class ContextBudgetExceeded(LocalProviderError):
    """Assembled local request cannot fit the bundled context window."""

    def __init__(self, message: str) -> None:
        super().__init__("context_budget_exceeded", message)


class LocalCapacityExhausted(LocalProviderError):
    """Bundled local server ran out of serving capacity after admission."""

    def __init__(self) -> None:
        super().__init__("local_capacity_exhausted", _LOCAL_CAPACITY_EXHAUSTED_MESSAGE)


def normalize_model_id(model: str | None) -> str:
    model_id = str(model or LOCAL_MODEL)
    if model_id.startswith("openai/"):
        model_id = model_id[len("openai/") :]
    if not model_id.startswith(_LOCAL_PREFIX):
        raise LocalProviderError(
            "unsupported_model",
            f"Local provider model must start with {_LOCAL_PREFIX!r}: {model_id}",
        )
    return LOCAL_MODEL


def _telemetry_record(
    *,
    request_id: str,
    kind: str,
    model: str,
    profile: str,
    capacity: int,
    capacity_source: str,
    started: float,
    queue_wait_ms: float,
    admission_slot: int | None,
    retry_index: int | None,
    outcome: str,
    finish_reason: str | None = None,
    reason_code: str | None = None,
) -> dict[str, Any]:
    record: dict[str, Any] = {
        "timestamp": time.time(),
        "request_id": request_id,
        "kind": kind,
        "provider": "local",
        "model": model,
        "profile": profile,
        "serving_capacity": capacity,
        "capacity_source": capacity_source,
        "admission_slot": admission_slot,
        "queue_wait_ms": round(queue_wait_ms, 3),
        "client_total_ms": round((time.monotonic() - started) * 1000.0, 3),
        "retry_index": retry_index,
        "outcome": outcome,
        "finish_reason": finish_reason,
        "reason_code": reason_code,
        "timed_out": outcome == "timeout",
        "cancelled": outcome == "cancelled",
    }
    return record


def _remaining_timeout(started: float, timeout_s: float) -> float:
    remaining = timeout_s - (time.monotonic() - started)
    if remaining <= 0:
        from solstone.think.providers.local_admission import LocalAdmissionTimeout

        raise LocalAdmissionTimeout(
            f"Local inference request exceeded its {timeout_s:.3f}s deadline."
        )
    return remaining


async def run_cogitate(
    config: dict[str, Any],
    on_event: Callable[[dict], None] | None = None,
) -> str:
    from solstone.think import cogitate_client
    from solstone.think.providers import local_server
    from solstone.think.providers.local_admission import (
        LocalAdmissionTimeout,
        LocalSlotLease,
        acquire_local_slot_async,
        record_local_inference,
    )

    config = {**config, "model": normalize_model_id(config.get("model", LOCAL_MODEL))}
    endpoint = resolve_local_endpoint()
    started = time.monotonic()
    request_id = uuid.uuid4().hex
    timeout = float(config.get("timeout_seconds", 600) or 600)
    server = None
    capacity = None
    slot_lease = None
    outcome = "success"
    reason_code: str | None = None
    try:
        if endpoint.is_bundled:
            server = local_server.connect()
            capacity = local_server.read_server_capacity()
            permit = await acquire_local_slot_async(
                capacity.parallel_slots,
                _remaining_timeout(started, timeout),
            )
            slot_lease = LocalSlotLease(
                capacity=capacity.parallel_slots,
                deadline=started + timeout,
                permit=permit,
            )
        elif endpoint.parallel_slots is not None:
            permit = await acquire_local_slot_async(
                endpoint.parallel_slots,
                _remaining_timeout(started, timeout),
            )
            slot_lease = LocalSlotLease(
                capacity=endpoint.parallel_slots,
                deadline=started + timeout,
                permit=permit,
            )
        context_window = None
        if not endpoint.is_bundled:
            from solstone.think.providers.local_endpoint import (
                resolve_endpoint_served_window,
            )

            context_window = resolve_endpoint_served_window(endpoint)
        # AC4: the lease remains held by this wrapper for the native-client run.
        return await cogitate_client.run_cogitate(
            config,
            on_event=on_event,
            context_window=context_window,
        )
    except asyncio.CancelledError:
        outcome = "cancelled"
        reason_code = "cancelled"
        raise
    except Exception as exc:
        outcome = (
            "timeout"
            if isinstance(exc, LocalAdmissionTimeout)
            or getattr(exc, "reason_code", None) == "wall_clock_exceeded"
            else "error"
        )
        from solstone.think.talents import TalentHookError

        if isinstance(exc, TalentHookError):
            raise

        if not endpoint.is_bundled:
            reason_code = classify_byo_cogitate_error(exc) or getattr(
                exc, "reason_code", None
            )
        reason_code = (
            reason_code
            or getattr(exc, "reason_code", None)
            or classify_provider_error(exc, "local")
        )
        if on_event and not getattr(exc, "_evented", False):
            error_text = str(exc)
            trace_text = traceback.format_exc()
            fixed_copy = local_endpoint_reason_copy(reason_code)
            if fixed_copy:
                error_text = fixed_copy
            if not endpoint.is_bundled:
                error_text = redact_local_endpoint_credential(error_text, endpoint)
                trace_text = redact_local_endpoint_credential(trace_text, endpoint)
                if not fixed_copy:
                    error_text = error_text[:PROVIDER_ERROR_TEXT_CAP_CHARS]
            on_event(
                {
                    "event": "error",
                    "error": error_text,
                    "reason_code": reason_code,
                    "provider": "local",
                    "trace": trace_text,
                    "raw": safe_raw([{"reason_code": reason_code}]),
                }
            )
            setattr(exc, "_evented", True)
        fixed_copy = local_endpoint_reason_copy(reason_code)
        if fixed_copy:
            wrapped = LocalProviderError(reason_code or "unknown", fixed_copy)
            setattr(wrapped, "_evented", getattr(exc, "_evented", False))
            raise wrapped from exc
        raise
    finally:
        if slot_lease is not None:
            slot_lease.close()
        if server is not None and capacity is not None:
            record_local_inference(
                _telemetry_record(
                    request_id=request_id,
                    kind="cogitate",
                    model=LOCAL_MODEL,
                    profile=capacity.profile,
                    capacity=capacity.parallel_slots,
                    capacity_source=capacity.source,
                    started=started,
                    queue_wait_ms=(
                        slot_lease.initial_queue_wait_ms
                        if slot_lease is not None
                        else (time.monotonic() - started) * 1000.0
                    ),
                    admission_slot=(
                        slot_lease.initial_slot_index
                        if slot_lease is not None
                        else None
                    ),
                    retry_index=None,
                    outcome=outcome,
                    finish_reason="stop" if outcome == "success" else None,
                    reason_code=reason_code,
                )
            )


def list_models(provider: str = "local") -> list[dict[str, Any]]:
    del provider
    return [
        {
            "name": spec.model_id,
            "model": spec.model_id,
            "repo": spec.repo,
            "filename": spec.filename,
            "size_bytes": spec.size_bytes,
            "min_ram_bytes": spec.min_ram_bytes,
        }
        for spec in LOCAL_MODEL_SPECS.values()
    ]


def validate_key(provider: str = "local", api_key: str = "") -> dict[str, Any]:
    del provider, api_key
    from solstone.think import generate_client

    try:
        generate_client.generate_with_result(
            "Say OK",
            "settings.local.validate_key",
            temperature=0,
            max_output_tokens=8,
            timeout_s=10,
        )
        return {"valid": True}
    except Exception as exc:
        return {
            "valid": False,
            "error": str(exc),
            "reason_code": getattr(exc, "reason_code", None),
        }


__all__ = [
    "LOCAL_MODEL_SPECS",
    "ContextBudgetExceeded",
    "LocalCapacityExhausted",
    "LocalModelSpec",
    "LocalProviderError",
    "normalize_model_id",
    "run_cogitate",
    "list_models",
    "validate_key",
]
