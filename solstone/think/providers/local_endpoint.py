# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""BYO OpenAI-compatible endpoint resolution for the local provider."""

from __future__ import annotations

import logging
from collections.abc import Callable
from dataclasses import dataclass
from typing import Any

from solstone.think.journal_config import read_journal_config

LOG = logging.getLogger(__name__)

# COPY REVIEW: founder-gated owner-facing copy; keep in sync with convey reason UI.
LOCAL_ENDPOINT_UNREACHABLE_COPY = (
    "The inference endpoint you configured could not be reached."
)
# COPY REVIEW: founder-gated owner-facing copy; keep in sync with convey reason UI.
LOCAL_ENDPOINT_CONTRACT_COPY = (
    "The configured endpoint did not respond in the expected format."
)

_REASON_COPY_BY_CODE = {
    "local_endpoint_unreachable": LOCAL_ENDPOINT_UNREACHABLE_COPY,
    "local_endpoint_contract_failed": LOCAL_ENDPOINT_CONTRACT_COPY,
}
_DEFAULT_BYO_PARALLEL_SLOTS = 2


@dataclass(frozen=True)
class LocalEndpoint:
    """Resolved local endpoint and optional client-side admission capacity.

    Bundled endpoints always carry ``parallel_slots=None``; bundled capacity is
    live server state and this field is inert. Confidential BYO endpoints also
    carry ``None`` and are ungoverned. Non-confidential BYO endpoints carry the
    resolved ``int >= 1`` client-side slot count.
    """

    base_url: str
    served_model_id: str
    credential: str | None
    is_bundled: bool
    parallel_slots: int | None = None


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


def resolve_local_endpoint() -> LocalEndpoint:
    """Resolve whether local provider traffic uses bundled runtime or BYO endpoint."""

    config = read_journal_config()
    providers_config = config.get("providers", {})
    local_config: Any = {}
    if isinstance(providers_config, dict):
        local_config = providers_config.get("local", {})
    if not isinstance(local_config, dict):
        local_config = {}

    endpoint_url = str(local_config.get("endpoint_url") or "").strip()
    served_model_id = str(local_config.get("served_model_id") or "").strip()
    if endpoint_url and served_model_id:
        credential = local_config.get("credential") or None
        return LocalEndpoint(
            base_url=normalize_local_endpoint_url(endpoint_url),
            served_model_id=served_model_id,
            credential=str(credential) if credential is not None else None,
            is_bundled=False,
            parallel_slots=(
                None
                if confidential_provenance_block(config) is not None
                else _configured_byo_parallel_slots(local_config)
            ),
        )
    return LocalEndpoint("", "", None, is_bundled=True)


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


def is_byo_network_error(exc: BaseException) -> bool:
    """True if the cause chain names a connection/timeout/network failure."""

    names = {type(item).__name__ for item in _exception_chain(exc)}
    return bool(_BYO_NETWORK_EXC_NAMES & names)


def classify_byo_cogitate_error(exc: BaseException) -> str | None:
    """Return a BYO local-endpoint reason code for known OpenHands/LiteLLM errors."""

    chain = _exception_chain(exc)
    names = {type(item).__name__ for item in chain}
    statuses = {_status_code(item) for item in chain}

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


def redact_event_payload(payload: Any, credential: str | None) -> Any:
    """Recursively replace credential in every string value of an event payload."""

    if not credential:
        return payload
    if isinstance(payload, dict):
        return {
            key: redact_event_payload(value, credential)
            for key, value in payload.items()
        }
    if isinstance(payload, list):
        return [redact_event_payload(item, credential) for item in payload]
    if isinstance(payload, tuple):
        return tuple(redact_event_payload(item, credential) for item in payload)
    if isinstance(payload, str):
        return payload.replace(credential, "***")
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
    "LOCAL_ENDPOINT_CONTRACT_COPY",
    "LOCAL_ENDPOINT_UNREACHABLE_COPY",
    "LocalEndpoint",
    "classify_byo_cogitate_error",
    "confidential_provenance_block",
    "is_byo_network_error",
    "local_endpoint_reason_copy",
    "normalize_local_endpoint_url",
    "probe_local_endpoint",
    "redact_event_payload",
    "redact_local_endpoint_credential",
    "resolve_local_endpoint",
    "wrap_on_event_redacting",
]
