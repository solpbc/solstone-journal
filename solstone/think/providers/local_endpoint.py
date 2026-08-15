# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""BYO OpenAI-compatible endpoint resolution for the local provider."""

from __future__ import annotations

import json
import logging
import time
from collections.abc import Callable, Iterator
from dataclasses import dataclass
from typing import Any

from solstone.think.journal_config import read_journal_config
from solstone.think.providers.shared import (
    _CONTEXT_WINDOW_PATTERNS,
    PROVIDER_ERROR_TEXT_CAP_CHARS,
    _contains_any,
)

LOG = logging.getLogger(__name__)

# COPY REVIEW: approved owner-facing copy; keep in sync with convey reason UI.
LOCAL_ENDPOINT_UNREACHABLE_COPY = (
    "The inference endpoint you configured could not be reached."
)
# COPY REVIEW: approved owner-facing copy; keep in sync with convey reason UI.
LOCAL_ENDPOINT_CONTRACT_COPY = (
    "The configured endpoint did not respond in the expected format."
)

_REASON_COPY_BY_CODE = {
    "local_endpoint_unreachable": LOCAL_ENDPOINT_UNREACHABLE_COPY,
    "local_endpoint_contract_failed": LOCAL_ENDPOINT_CONTRACT_COPY,
}
_DEFAULT_BYO_PARALLEL_SLOTS = 2
ENDPOINT_SERVED_CONTEXT_WINDOW_CONFIG_KEY = "served_context_window"
ENDPOINT_SERVED_CONTEXT_WINDOW_MIN_TOKENS = 2048
ENDPOINT_SERVED_WINDOW_CACHE_TTL_S = 300.0
ENDPOINT_MODELS_TIMEOUT_S = 2.5

_SERVED_WINDOW_CACHE: dict[tuple[str, str], tuple[float, int | None]] = {}


@dataclass(frozen=True)
class LocalEndpoint:
    """Resolved local endpoint and optional client-side admission capacity.

    Bundled endpoints always carry ``parallel_slots=None``; bundled capacity is
    live server state and this field is inert. Confidential BYO endpoints also
    carry ``None`` and are ungoverned. Non-confidential BYO endpoints carry the
    resolved ``int >= 1`` client-side slot count. ``is_confidential`` marks BYO
    endpoints routed through the confidential forwarder; bundled endpoints are
    not confidential endpoints even when confidential service config is present.
    """

    base_url: str
    served_model_id: str
    credential: str | None
    is_bundled: bool
    parallel_slots: int | None = None
    is_confidential: bool = False


def normalize_local_endpoint_url(raw_url: str) -> str:
    """Return the endpoint root without a trailing slash or single /v1 segment."""

    root = raw_url.strip().rstrip("/")
    if root.endswith("/v1"):
        root = root[: -len("/v1")].rstrip("/")
    return root


def confidential_provenance_block(config: dict[str, Any]) -> dict[str, Any] | None:
    """Return the services.confidential block from an already-read journal config."""

    services = config.get("services")
    if not isinstance(services, dict):
        return None
    provenance = services.get("confidential")
    return provenance if isinstance(provenance, dict) else None


_CONFIDENTIAL_RESTORE_ONLY_FIELDS = frozenset(
    {
        "prior_active",
        "prior_local_endpoint",
    }
)


def confidential_fingerprint_provenance_block(
    config: dict[str, Any],
) -> dict[str, Any] | None:
    """Return confidential provenance fields that affect active SPP execution."""

    provenance = confidential_provenance_block(config)
    if provenance is None:
        return None
    return {
        key: value
        for key, value in provenance.items()
        if key not in _CONFIDENTIAL_RESTORE_ONLY_FIELDS
    }


def _configured_byo_parallel_slots(local_config: dict[str, Any]) -> int:
    if "parallel_slots" not in local_config:
        return _DEFAULT_BYO_PARALLEL_SLOTS

    raw = local_config.get("parallel_slots")
    if not isinstance(raw, int) or isinstance(raw, bool) or raw < 1:
        LOG.warning(
            "Invalid providers.local.parallel_slots in journal config: %r - "
            "defaulting to %d",
            raw,
            _DEFAULT_BYO_PARALLEL_SLOTS,
        )
        return _DEFAULT_BYO_PARALLEL_SLOTS
    return raw


def _local_provider_config(config: dict[str, Any]) -> dict[str, Any]:
    providers_config = config.get("providers", {})
    local_config: Any = {}
    if isinstance(providers_config, dict):
        local_config = providers_config.get("local", {})
    return local_config if isinstance(local_config, dict) else {}


def _configured_served_context_window(local_config: dict[str, Any]) -> int | None:
    if ENDPOINT_SERVED_CONTEXT_WINDOW_CONFIG_KEY not in local_config:
        return None

    raw = local_config.get(ENDPOINT_SERVED_CONTEXT_WINDOW_CONFIG_KEY)
    if (
        not isinstance(raw, int)
        or isinstance(raw, bool)
        or raw < ENDPOINT_SERVED_CONTEXT_WINDOW_MIN_TOKENS
    ):
        LOG.warning(
            "Invalid providers.local.%s in journal config: %r - falling through "
            "to endpoint discovery",
            ENDPOINT_SERVED_CONTEXT_WINDOW_CONFIG_KEY,
            raw,
        )
        return None
    return raw


def resolve_local_endpoint_from_config(config: dict[str, Any]) -> LocalEndpoint:
    """Resolve local provider traffic from an already-read journal config."""

    local_config = _local_provider_config(config)

    endpoint_url = str(local_config.get("endpoint_url") or "").strip()
    served_model_id = str(local_config.get("served_model_id") or "").strip()
    if endpoint_url and served_model_id:
        credential = local_config.get("credential") or None
        is_confidential = confidential_provenance_block(config) is not None
        return LocalEndpoint(
            base_url=normalize_local_endpoint_url(endpoint_url),
            served_model_id=served_model_id,
            credential=str(credential) if credential is not None else None,
            is_bundled=False,
            parallel_slots=(
                None
                if is_confidential
                else _configured_byo_parallel_slots(local_config)
            ),
            is_confidential=is_confidential,
        )
    return LocalEndpoint("", "", None, is_bundled=True, is_confidential=False)


def resolve_local_endpoint() -> LocalEndpoint:
    """Resolve whether local provider traffic uses bundled runtime or BYO endpoint."""

    return resolve_local_endpoint_from_config(read_journal_config())


def reset_endpoint_served_window_cache() -> None:
    """Clear the process-local endpoint served-window discovery cache."""

    _SERVED_WINDOW_CACHE.clear()


def resolve_endpoint_served_window(endpoint: LocalEndpoint) -> int | None:
    """Resolve the endpoint's served context window without mutating journal state."""

    if endpoint.is_bundled:
        return None

    local_config = _local_provider_config(read_journal_config())
    override = _configured_served_context_window(local_config)
    if override is not None:
        return override

    now = time.monotonic()
    cache_key = (endpoint.base_url, endpoint.served_model_id)
    cached = _SERVED_WINDOW_CACHE.get(cache_key)
    if cached is not None:
        cached_at, value = cached
        if now - cached_at < ENDPOINT_SERVED_WINDOW_CACHE_TTL_S:
            return value

    value = _discover_endpoint_served_window(endpoint)
    _SERVED_WINDOW_CACHE[cache_key] = (now, value)
    if endpoint.is_confidential and value is None:
        LOG.warning(
            "Could not resolve served context window for confidential local "
            "endpoint %s",
            endpoint.served_model_id,
        )
    return value


def _discover_endpoint_served_window(endpoint: LocalEndpoint) -> int | None:
    import httpx

    from solstone.think.services.spp_transport import confidential_egress_base_url

    try:
        base_url = (
            confidential_egress_base_url(endpoint.base_url)
            if endpoint.is_confidential
            else endpoint.base_url
        )
        kwargs: dict[str, Any] = {"timeout": ENDPOINT_MODELS_TIMEOUT_S}
        if endpoint.credential:
            kwargs["headers"] = {"Authorization": f"Bearer {endpoint.credential}"}
        response = httpx.get(f"{base_url}/v1/models", **kwargs)
        response.raise_for_status()
        data = response.json()
    except Exception:
        LOG.debug(
            "Could not discover local endpoint served context window", exc_info=True
        )
        return None

    if not isinstance(data, dict):
        return None
    models = data.get("data")
    if not isinstance(models, list):
        return None
    for item in models:
        if not isinstance(item, dict) or item.get("id") != endpoint.served_model_id:
            continue
        max_model_len = item.get("max_model_len")
        if isinstance(max_model_len, int) and not isinstance(max_model_len, bool):
            return max_model_len
        return None
    return None


def probe_local_endpoint(
    endpoint: LocalEndpoint,
    timeout_s: float = 1.0,
) -> tuple[bool, str | None]:
    """Run a shallow reachability probe against the configured endpoint root."""

    from solstone.think.services.spp_transport import confidential_probe_status

    confidential_status = confidential_probe_status()
    if confidential_status is not None:
        return confidential_status

    import httpx

    try:
        httpx.get(endpoint.base_url, timeout=timeout_s)
        return True, None
    except (httpx.TimeoutException, httpx.NetworkError, httpx.RequestError) as exc:
        return False, str(exc)


def _status_code(exc: BaseException) -> int | None:
    status = getattr(exc, "status_code", None)
    if isinstance(status, int):
        return status
    response = getattr(exc, "response", None)
    response_status = getattr(response, "status_code", None)
    return response_status if isinstance(response_status, int) else None


def _exception_chain(exc: BaseException, limit: int = 6) -> list[BaseException]:
    chain: list[BaseException] = []
    seen: set[int] = set()
    current: BaseException | None = exc
    while current is not None and len(chain) < limit and id(current) not in seen:
        chain.append(current)
        seen.add(id(current))
        current = current.__cause__ or current.__context__
    return chain


_BYO_NETWORK_EXC_NAMES = frozenset(
    {
        "ConnectError",
        "APIConnectionError",
        "ConnectTimeout",
        "ReadTimeout",
        "PoolTimeout",
        "TimeoutException",
        "NetworkError",
        "RequestError",
    }
)


_BYO_CAPACITY_EXC_NAMES = frozenset(
    {
        "ReadTimeout",
        "PoolTimeout",
        "TimeoutException",
        "WriteTimeout",
    }
)


def is_byo_network_error(exc: BaseException) -> bool:
    """True if the cause chain names a connection/timeout/network failure."""

    names = {type(item).__name__ for item in _exception_chain(exc)}
    return bool(_BYO_NETWORK_EXC_NAMES & names)


def is_byo_capacity_error(exc: BaseException) -> bool:
    """True if the cause chain names a serving-capacity timeout."""

    names = {type(item).__name__ for item in _exception_chain(exc)}
    return bool(_BYO_CAPACITY_EXC_NAMES & names)


def _payload_text(payload: Any, credential: str | None) -> str | None:
    if payload is None:
        return None
    redacted = redact_event_payload(payload, credential)
    if isinstance(redacted, str):
        text = redacted
    elif isinstance(redacted, dict | list | tuple):
        try:
            text = json.dumps(redacted, default=str)
        except Exception:
            text = str(redacted)
    else:
        text = str(redacted)
    if not text:
        return None
    return text[:PROVIDER_ERROR_TEXT_CAP_CHARS]


def _candidate_exception_texts(
    item: BaseException,
    credential: str | None,
) -> Iterator[str]:
    body = _payload_text(getattr(item, "body", None), credential)
    if body:
        yield body

    response = getattr(item, "response", None)
    response_text = _payload_text(getattr(response, "text", None), credential)
    if response_text:
        yield response_text
    response_json = getattr(response, "json", None)
    if callable(response_json):
        try:
            json_text = _payload_text(response_json(), credential)
        except Exception:
            json_text = None
        if json_text:
            yield json_text

    message = _payload_text(getattr(item, "message", None), credential)
    if message:
        yield message

    item_text = _payload_text(str(item), credential)
    if item_text:
        yield item_text


def byo_exception_matches_context_window(
    exc: BaseException,
    *,
    credential: str | None = None,
) -> bool:
    """True when a BYO exception chain carries a context-window-shaped body."""

    for item in _exception_chain(exc):
        for text in _candidate_exception_texts(item, credential):
            if _contains_any(text.lower(), _CONTEXT_WINDOW_PATTERNS):
                return True
    return False


def classify_byo_cogitate_error(exc: BaseException) -> str | None:
    """Return a BYO local-endpoint reason code for known native-client errors."""

    chain = _exception_chain(exc)
    names = {type(item).__name__ for item in chain}
    statuses = {_status_code(item) for item in chain}

    if byo_exception_matches_context_window(exc):
        return "context_window_exceeded"
    if 400 in statuses or "BadRequestError" in names:
        return "local_endpoint_contract_failed"
    if is_byo_network_error(exc) or 500 in statuses or "InternalServerError" in names:
        return "local_endpoint_unreachable"
    return None


def local_endpoint_reason_copy(reason_code: str | None) -> str | None:
    """Return fixed owner-facing copy for a BYO local-endpoint reason code."""

    return _REASON_COPY_BY_CODE.get(reason_code or "")


def redact_local_endpoint_credential(text: str, endpoint: LocalEndpoint) -> str:
    """Redact the BYO bearer credential from provider-controlled text."""

    if endpoint.credential:
        return text.replace(endpoint.credential, "***")
    return text


def _exception_graph(exc: BaseException) -> Iterator[BaseException]:
    """Yield both exception-chain branches once, including cyclic graphs."""

    pending = [exc]
    seen: set[int] = set()
    while pending:
        item = pending.pop()
        if id(item) in seen:
            continue
        seen.add(id(item))
        yield item
        if item.__context__ is not None:
            pending.append(item.__context__)
        if item.__cause__ is not None:
            pending.append(item.__cause__)


def redact_exception_credential(
    exc: BaseException,
    credential: str | None,
) -> BaseException:
    """Redact ``credential`` from the complete serialized exception graph."""

    if not credential:
        return exc
    for item in _exception_graph(exc):
        if item.args:
            item.args = tuple(
                redact_event_payload(value, credential) for value in item.args
            )
        notes = getattr(item, "__notes__", None)
        if isinstance(notes, list):
            item.__notes__ = [redact_event_payload(note, credential) for note in notes]
        attributes = vars(item)
        for name, value in list(attributes.items()):
            if isinstance(value, (str, dict, list, tuple)):
                attributes[name] = redact_event_payload(value, credential)
    return exc


def redact_event_payload(payload: Any, credential: str | None) -> Any:
    """Recursively replace credential in every string value of an event payload."""

    if not credential:
        return payload
    if isinstance(payload, str):
        return payload.replace(credential, "***") if credential in payload else payload
    if isinstance(payload, dict):
        redacted = {}
        changed = False
        for key, value in payload.items():
            redacted_value = redact_event_payload(value, credential)
            if redacted_value is not value:
                changed = True
            redacted[key] = redacted_value
        return redacted if changed else payload
    if isinstance(payload, list):
        items = []
        changed = False
        for item in payload:
            redacted_item = redact_event_payload(item, credential)
            if redacted_item is not item:
                changed = True
            items.append(redacted_item)
        return items if changed else payload
    if isinstance(payload, tuple):
        items = []
        changed = False
        for item in payload:
            redacted_item = redact_event_payload(item, credential)
            if redacted_item is not item:
                changed = True
            items.append(redacted_item)
        if not changed:
            return payload
        if hasattr(payload, "_fields"):
            return type(payload)._make(items)
        if type(payload) is tuple:
            return tuple(items)
        return payload
    return payload


def wrap_on_event_redacting(
    on_event: Callable[[dict], None] | None,
    credential: str | None,
) -> Callable[[dict], None] | None:
    """Return an on_event wrapper that redacts credential before forwarding."""

    if on_event is None or not credential:
        return on_event

    def redacting_on_event(event: dict) -> None:
        on_event(redact_event_payload(event, credential))

    return redacting_on_event


__all__ = [
    "ENDPOINT_MODELS_TIMEOUT_S",
    "ENDPOINT_SERVED_CONTEXT_WINDOW_CONFIG_KEY",
    "ENDPOINT_SERVED_CONTEXT_WINDOW_MIN_TOKENS",
    "ENDPOINT_SERVED_WINDOW_CACHE_TTL_S",
    "LOCAL_ENDPOINT_CONTRACT_COPY",
    "LOCAL_ENDPOINT_UNREACHABLE_COPY",
    "LocalEndpoint",
    "byo_exception_matches_context_window",
    "classify_byo_cogitate_error",
    "confidential_fingerprint_provenance_block",
    "confidential_provenance_block",
    "is_byo_capacity_error",
    "is_byo_network_error",
    "local_endpoint_reason_copy",
    "normalize_local_endpoint_url",
    "probe_local_endpoint",
    "redact_exception_credential",
    "redact_event_payload",
    "redact_local_endpoint_credential",
    "reset_endpoint_served_window_cache",
    "resolve_endpoint_served_window",
    "resolve_local_endpoint",
    "resolve_local_endpoint_from_config",
    "wrap_on_event_redacting",
]
