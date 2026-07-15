# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

from dataclasses import dataclass
from typing import Any

from solstone.think.providers.local_endpoint import (
    LOCAL_ENDPOINT_CONTRACT_COPY,
    LOCAL_ENDPOINT_UNREACHABLE_COPY,
)
from solstone.think.providers.state import ProviderState


@dataclass(frozen=True)
class RecoveryAction:
    label: str
    target: str


@dataclass(frozen=True)
class ReadinessView:
    semantic_key: str
    work_key: str | None
    status: str
    severity: str
    reason_code: str
    provider: str
    model: str | None
    context: str | None
    interface: str | None
    summary: str
    detail: str
    recovery_action: RecoveryAction | None
    operator_detail: str


@dataclass(frozen=True)
class _Entry:
    klass: str
    summary: str
    detail: str
    recovery_action: RecoveryAction | None = None


DISPLAY_NAMES: dict[str, str] = {
    "google": "Gemini",
    "openai": "OpenAI",
    "anthropic": "Anthropic",
    "local": "Local",
}

PROVIDER_LEVEL_CODES = frozenset(
    {
        "thinking_engine_not_chosen",
        "provider_key_missing",
        "provider_key_invalid",
        "provider_quota_exceeded",
        "provider_unavailable",
        "network_unreachable",
        "local_endpoint_unreachable",
        "chat_timeout",
        "chat_pipeline_unavailable",
        "unknown",
        "no_output",
    }
)

_THINKING_ACTION = RecoveryAction(
    label="Open Thinking",
    target="/app/thinking/#main",
)
_LOCAL_SETUP_ACTION = RecoveryAction(
    label="Open Local Model Setup",
    target="/app/thinking/#local-setup",
)

_LOCAL_SETUP_DETAIL = "Finish local model setup, then try the request again."
_LOCAL_VERIFY_DETAIL = (
    "Local model setup could not be verified. Re-run setup before trying again."
)

_ENTRIES: dict[str, _Entry] = {
    "thinking_engine_not_chosen": _Entry(
        klass="setup",
        summary="no thinking engine is chosen yet",
        detail="Open Thinking to choose how sol thinks, then try again.",
        recovery_action=_THINKING_ACTION,
    ),
    "provider_key_missing": _Entry(
        klass="setup",
        summary="{provider} needs credentials before it can read your screen descriptions",
        detail="Open provider setup and add credentials, then try again.",
        recovery_action=_THINKING_ACTION,
    ),
    "ram_insufficient": _Entry(
        klass="setup",
        summary="the local model needs more memory than this machine has",
        detail=(
            "Choose a smaller local model or use a cloud provider for screen "
            "descriptions and journal interpretation."
        ),
        recovery_action=_LOCAL_SETUP_ACTION,
    ),
    "gpu_unavailable": _Entry(
        klass="setup",
        summary="local models need GPU acceleration on this computer",
        detail=(
            "Local models require a supported GPU. This computer has no GPU "
            "acceleration available."
        ),
        recovery_action=_THINKING_ACTION,
    ),
    "gpu_probe_failed": _Entry(
        klass="setup",
        summary="local GPU check couldn't finish",
        detail=(
            "Local model setup couldn't confirm GPU acceleration — try again, "
            "or use a cloud provider if it keeps failing."
        ),
        recovery_action=_THINKING_ACTION,
    ),
    "local_model_missing": _Entry(
        klass="setup",
        summary="local model setup is not finished",
        detail=_LOCAL_SETUP_DETAIL,
        recovery_action=_LOCAL_SETUP_ACTION,
    ),
    "model_missing": _Entry(
        klass="setup",
        summary="local model setup is not finished",
        detail=_LOCAL_SETUP_DETAIL,
        recovery_action=_LOCAL_SETUP_ACTION,
    ),
    "binary_missing": _Entry(
        klass="setup",
        summary="local model setup is not finished",
        detail=_LOCAL_SETUP_DETAIL,
        recovery_action=_LOCAL_SETUP_ACTION,
    ),
    "local_model_installing": _Entry(
        klass="setup",
        summary="local model setup is finishing",
        detail="The local model is still installing. Try again shortly.",
        recovery_action=None,
    ),
    "local_model_loading": _Entry(
        klass="setup",
        summary="the local model is starting up",
        detail="The local model is not ready yet. Try again shortly.",
        recovery_action=None,
    ),
    "local_model_not_ready": _Entry(
        klass="setup",
        summary="the local model is starting up",
        detail="The local model is not ready yet. Try again shortly.",
        recovery_action=None,
    ),
    "local_server_unhealthy": _Entry(
        klass="provider",
        summary="the local model isn't responding",
        detail="Restart local model setup or try a cloud provider.",
        recovery_action=_LOCAL_SETUP_ACTION,
    ),
    "local_endpoint_unreachable": _Entry(
        klass="provider",
        summary=LOCAL_ENDPOINT_UNREACHABLE_COPY,
        detail="Check the endpoint URL, confirm the server is running, then retry.",
        recovery_action=_THINKING_ACTION,
    ),
    "local_endpoint_contract_failed": _Entry(
        klass="generic",
        summary=LOCAL_ENDPOINT_CONTRACT_COPY,
        detail=(
            "Confirm the endpoint serves /v1/chat/completions with vision and "
            "JSON-schema response_format support."
        ),
        recovery_action=_THINKING_ACTION,
    ),
    "unsupported_platform": _Entry(
        klass="setup",
        summary="this machine is not supported for local model setup",
        detail="Use a cloud provider or try local model setup on a supported machine.",
        recovery_action=_LOCAL_SETUP_ACTION,
    ),
    "host_unfit": _Entry(
        klass="setup",
        summary="this computer doesn't meet the local model's requirements",
        detail=(
            "A pre-download check found this computer can't run the selected "
            "local model. Choose a smaller local model or use a cloud provider "
            "for screen descriptions and journal interpretation."
        ),
        recovery_action=_LOCAL_SETUP_ACTION,
    ),
    "unsupported_model": _Entry(
        klass="setup",
        summary="this local model is not supported",
        detail="Choose a supported local model, then try again.",
        recovery_action=_LOCAL_SETUP_ACTION,
    ),
    "sha256_mismatch": _Entry(
        klass="setup",
        summary="local model setup could not be verified",
        detail=_LOCAL_VERIFY_DETAIL,
        recovery_action=_LOCAL_SETUP_ACTION,
    ),
    "archive_path_traversal": _Entry(
        klass="setup",
        summary="local model setup could not be verified",
        detail=_LOCAL_VERIFY_DETAIL,
        recovery_action=_LOCAL_SETUP_ACTION,
    ),
    "provider_key_invalid": _Entry(
        klass="provider",
        summary="your {provider} key didn't validate",
        detail="Open provider setup and check the saved credentials.",
        recovery_action=_THINKING_ACTION,
    ),
    "provider_quota_exceeded": _Entry(
        klass="provider",
        summary="your {provider} quota is spent",
        detail="Wait for provider quota to reset or choose another provider.",
        recovery_action=None,
    ),
    "network_unreachable": _Entry(
        klass="generic",
        summary="I couldn't reach the network",
        detail="Check the network connection, then try again.",
        recovery_action=None,
    ),
    "provider_response_invalid": _Entry(
        klass="generic",
        summary="{provider}'s response didn't match the expected shape — try rephrasing or asking something more specific.",
        detail="Try a narrower request or choose another provider.",
        recovery_action=None,
    ),
    "provider_unavailable": _Entry(
        klass="provider",
        summary="{provider} is having trouble right now",
        detail="Try again later or choose another provider.",
        recovery_action=None,
    ),
    "chat_pipeline_unavailable": _Entry(
        klass="generic",
        summary="the chat pipeline isn't ready yet",
        detail="Try again once chat finishes starting.",
        recovery_action=None,
    ),
    "chat_timeout": _Entry(
        klass="generic",
        summary="chat took too long",
        detail="Try again with a shorter request.",
        recovery_action=None,
    ),
    "local_queue_timeout": _Entry(
        klass="generic",
        summary="the local model was busy and couldn't start in time",
        detail=(
            "Your computer was already running as many local requests as it can "
            "at once. Try again in a moment."
        ),
        recovery_action=None,
    ),
    "local_capacity_exhausted": _Entry(
        klass="generic",
        summary="the local model was busy and could not finish this request",
        detail="Try again in a moment.",
        recovery_action=None,
    ),
    "context_window_exceeded": _Entry(
        klass="generic",
        summary="the conversation grew too long to finish",
        detail="Try a shorter or more focused request.",
        recovery_action=None,
    ),
    "context_budget_exceeded": _Entry(
        klass="generic",
        summary="the request was too long for the local model",
        detail=(
            "The request didn't fit the local model's context window. Try a "
            "shorter or more focused request, or choose another provider."
        ),
        recovery_action=None,
    ),
    "incomplete_json_length": _Entry(
        klass="generic",
        summary="the answer ran out of room before it finished",
        detail=(
            "The reply hit its length limit before it could finish. On your own "
            "machine sol tries once more with different settings; if it still runs "
            "long, ask for less at once or choose another provider."
        ),
        recovery_action=None,
    ),
    "incomplete_text_length": _Entry(
        klass="generic",
        summary="the answer ran out of room before it finished",
        detail=(
            "The reply hit its length limit before it could finish. Try again "
            "with less at once or choose another provider."
        ),
        recovery_action=None,
    ),
    "max_turns_exhausted": _Entry(
        klass="generic",
        summary="this took too many steps to finish",
        detail="Try again or simplify the request.",
        recovery_action=None,
    ),
    "no_output": _Entry(
        klass="generic",
        summary="I didn't get a response",
        detail="Try again or choose another provider.",
        recovery_action=None,
    ),
    "token_budget_exceeded": _Entry(
        klass="generic",
        summary="this run reached its resource budget before finishing",
        detail="Try a shorter or more focused request.",
        recovery_action=None,
    ),
    "wall_clock_exceeded": _Entry(
        klass="generic",
        summary="this run took too long to finish",
        detail="Try a shorter or more focused request.",
        recovery_action=None,
    ),
    "unknown": _Entry(
        klass="generic",
        summary="chat had trouble",
        detail="Try again or choose another provider.",
        recovery_action=None,
    ),
}

_STARTUP_REASON_CODES = frozenset(
    {
        "local_model_installing",
        "local_model_loading",
        "local_model_not_ready",
    }
)

_READY_ENTRY = _Entry(
    klass="generic",
    summary="{provider} is ready",
    detail="Provider readiness is clear.",
    recovery_action=None,
)

_FALLBACK_ENTRY = _Entry(
    klass="generic",
    summary="couldn't determine provider readiness — check your provider or local model setup",
    detail="Check provider setup and local model setup, then try again.",
    recovery_action=_THINKING_ACTION,
)

_NEUTRAL_SUMMARY = (
    "{provider} is set up — readiness will be confirmed when it's next used"
)
_NEUTRAL_DETAIL = "No action needed right now."


def mapped_reason_codes() -> frozenset[str]:
    return frozenset(_ENTRIES)


def is_blocking_reason(reason_code: str) -> bool:
    entry = _ENTRIES.get(reason_code)
    return bool(entry and entry.klass in {"setup", "provider"})


def backlog_reason_category(reason_code: str | None) -> str:
    """Coarse category for the stuck-day backlog surface.

    Orthogonal to `_Entry.klass`: the startup carve-out lets transient
    local-model states map to a "try again" sentence instead of the
    missing-setting one. Codes absent from the taxonomy (corrupt_raw,
    catchup_backoff, unknown) fall through to "generic".
    """
    if reason_code in _STARTUP_REASON_CODES:
        return "startup"
    entry = _ENTRIES.get(reason_code)
    return entry.klass if entry is not None else "generic"


def semantic_key_for(reason_code: str, provider: str, model: str | None = None) -> str:
    model_part = "" if reason_code in PROVIDER_LEVEL_CODES else model or ""
    return f"{reason_code}:{provider}:{model_part}"


def chat_reason_projection() -> dict[str, dict[str, Any]]:
    return {
        code: {
            "template": entry.summary,
            "action": _action_projection(entry.recovery_action),
        }
        for code, entry in _ENTRIES.items()
    }


def view_to_dict(view: ReadinessView) -> dict[str, Any]:
    return {
        "semantic_key": view.semantic_key,
        "work_key": view.work_key,
        "status": view.status,
        "severity": view.severity,
        "reason_code": view.reason_code,
        "provider": view.provider,
        "model": view.model,
        "context": view.context,
        "interface": view.interface,
        "summary": view.summary,
        "detail": view.detail,
        "recovery_action": _action_projection(view.recovery_action),
        "operator_detail": view.operator_detail,
    }


def present_readiness(
    state: ProviderState, *, work_key: str | None = None
) -> ReadinessView:
    reason_code = state.reason_code or "ready"
    return _build_view(
        reason_code,
        provider=state.provider,
        model=state.model,
        status=state.status,
        context=state.context,
        interface=state.interface,
        message=state.message,
        reset_at_ms=state.reset_at_ms,
        work_key=work_key,
    )


def present_for_reason(
    reason_code: str,
    *,
    provider: str = "",
    model: str | None = None,
    status: str = "unknown",
    context: str | None = None,
    interface: str | None = None,
    message: str | None = None,
    reset_at_ms: int | None = None,
    work_key: str | None = None,
) -> ReadinessView:
    return _build_view(
        reason_code,
        provider=provider,
        model=model,
        status=status,
        context=context,
        interface=interface,
        message=message,
        reset_at_ms=reset_at_ms,
        work_key=work_key,
    )


def chat_view(code: str, provider: str) -> dict[str, Any]:
    entry = _ENTRIES.get(code)
    if entry is None:
        return {"code": code, "message": code, "action": None}

    if code == "unknown":
        display_name = DISPLAY_NAMES.get(provider)
        message = (
            f"something went wrong with {display_name}"
            if display_name
            else entry.summary
        )
        return {"code": code, "message": message, "action": None}

    display_name = DISPLAY_NAMES.get(provider, provider)
    message = _render_template(entry.summary, display_name)
    return {
        "code": code,
        "message": message,
        "action": _action_projection(entry.recovery_action),
    }


def _build_view(
    reason_code: str,
    *,
    provider: str,
    model: str | None,
    status: str,
    context: str | None,
    interface: str | None,
    message: str | None,
    reset_at_ms: int | None,
    work_key: str | None,
) -> ReadinessView:
    if reason_code == "ready":
        entry = _READY_ENTRY
        mapped = True
    else:
        entry = _ENTRIES.get(reason_code, _FALLBACK_ENTRY)
        mapped = reason_code in _ENTRIES
    display_name = _provider_display_name(provider)
    severity = _severity(status, entry.klass, mapped=mapped)
    if severity == "neutral":
        summary = _render_template(_NEUTRAL_SUMMARY, display_name)
        detail = _NEUTRAL_DETAIL
        recovery_action = None
    else:
        summary = _render_template(entry.summary, display_name)
        detail = _render_template(entry.detail, display_name)
        recovery_action = entry.recovery_action
    return ReadinessView(
        semantic_key=semantic_key_for(reason_code, provider, model),
        work_key=work_key,
        status=status,
        severity=severity,
        reason_code=reason_code,
        provider=provider,
        model=model,
        context=context,
        interface=interface,
        summary=summary,
        detail=detail,
        recovery_action=recovery_action,
        operator_detail=_operator_detail(
            reason_code=reason_code,
            provider=provider,
            model=model,
            status=status,
            context=context,
            interface=interface,
            reset_at_ms=reset_at_ms,
            message=message,
        ),
    )


def _severity(status: str, klass: str, *, mapped: bool) -> str:
    if not mapped:
        return "neutral" if status == "unknown" else "attention"
    if status == "ready":
        return "ok"
    if status == "unknown":
        return "neutral"
    if status in {"blocked", "unhealthy"}:
        return "blocker" if klass == "setup" else "attention"
    return "attention"


def _provider_display_name(provider: str) -> str:
    return DISPLAY_NAMES.get(provider, "provider" if not provider else provider)


def _render_template(template: str, provider: str) -> str:
    return template.replace("{provider}", provider)


def _action_projection(action: RecoveryAction | None) -> dict[str, str] | None:
    if action is None:
        return None
    return {"label": action.label, "href": action.target}


def _operator_detail(
    *,
    reason_code: str,
    provider: str,
    model: str | None,
    status: str,
    context: str | None,
    interface: str | None,
    reset_at_ms: int | None,
    message: str | None,
) -> str:
    parts = [
        f"reason_code={reason_code}",
        f"provider={provider or '<unset>'}",
        f"status={status}",
    ]
    if model:
        parts.append(f"model={model}")
    if interface:
        parts.append(f"interface={interface}")
    if context:
        parts.append(f"context={context}")
    if reset_at_ms is not None:
        parts.append(f"reset_at_ms={reset_at_ms}")
    if message:
        parts.append(f"message={_bounded(message)}")
    return "; ".join(parts)


def _bounded(value: str, limit: int = 240) -> str:
    clean = " ".join(value.split())
    if len(clean) <= limit:
        return clean
    return clean[: limit - 1] + "…"
