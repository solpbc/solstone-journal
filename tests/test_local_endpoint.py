# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
import traceback

import pytest

from solstone.think.providers import local_endpoint


def _config(payload: dict) -> dict:
    return {"providers": {"local": payload}}


@pytest.mark.parametrize(
    ("config", "is_bundled"),
    [
        ({}, True),
        ({"providers": {"local": {}}}, True),
        (_config({"endpoint_url": "http://h:8080"}), True),
        (_config({"served_model_id": "model"}), True),
        ({"providers": {"local": "not-a-dict"}}, True),
        (
            _config(
                {"endpoint_url": " http://h:8080/v1/ ", "served_model_id": " model "}
            ),
            False,
        ),
    ],
)
def test_resolve_local_endpoint_active_requires_url_and_model(
    monkeypatch,
    config,
    is_bundled,
):
    monkeypatch.setattr(local_endpoint, "read_journal_config", lambda: config)

    endpoint = local_endpoint.resolve_local_endpoint()

    assert endpoint.is_bundled is is_bundled
    if not is_bundled:
        assert endpoint.base_url == "http://h:8080"
        assert endpoint.served_model_id == "model"
        assert endpoint.credential is None


def test_resolve_local_endpoint_carries_placeholder_credential(monkeypatch):
    monkeypatch.setattr(
        local_endpoint,
        "read_journal_config",
        lambda: _config(
            {
                "endpoint_url": "http://h:8080",
                "served_model_id": "model",
                "credential": "test-token-PLACEHOLDER",
            }
        ),
    )

    endpoint = local_endpoint.resolve_local_endpoint()

    assert endpoint.is_bundled is False
    assert endpoint.credential == "test-token-PLACEHOLDER"
    assert endpoint.parallel_slots == 2


@pytest.mark.parametrize(
    ("raw", "expected"),
    [
        (1, 1),
        (2, 2),
        (8, 8),
    ],
)
def test_resolve_local_endpoint_parallel_slots_from_config(monkeypatch, raw, expected):
    monkeypatch.setattr(
        local_endpoint,
        "read_journal_config",
        lambda: _config(
            {
                "endpoint_url": "http://h:8080",
                "served_model_id": "model",
                "parallel_slots": raw,
            }
        ),
    )

    endpoint = local_endpoint.resolve_local_endpoint()

    assert endpoint.is_bundled is False
    assert endpoint.parallel_slots == expected


@pytest.mark.parametrize("raw", [0, -1, True, False, 1.5, "2", [], {}])
def test_resolve_local_endpoint_invalid_parallel_slots_defaults_and_warns(
    monkeypatch,
    caplog,
    raw,
):
    monkeypatch.setattr(
        local_endpoint,
        "read_journal_config",
        lambda: _config(
            {
                "endpoint_url": "http://h:8080",
                "served_model_id": "model",
                "parallel_slots": raw,
            }
        ),
    )

    endpoint = local_endpoint.resolve_local_endpoint()

    assert endpoint.parallel_slots == 2
    assert (
        f"Invalid providers.local.parallel_slots in journal config: {raw!r} - "
        "defaulting to 2"
    ) in caplog.text


def test_resolve_local_endpoint_bundled_ignores_stray_parallel_slots(
    monkeypatch,
    caplog,
):
    monkeypatch.setattr(
        local_endpoint,
        "read_journal_config",
        lambda: _config({"parallel_slots": 0}),
    )

    endpoint = local_endpoint.resolve_local_endpoint()

    assert endpoint.is_bundled is True
    assert endpoint.parallel_slots is None
    assert "providers.local.parallel_slots" not in caplog.text


def test_resolve_local_endpoint_confidential_ignores_stray_parallel_slots(
    monkeypatch,
    caplog,
):
    monkeypatch.setattr(
        local_endpoint,
        "read_journal_config",
        lambda: {
            "providers": {
                "local": {
                    "endpoint_url": "https://spp.example.test",
                    "served_model_id": "confidential-model",
                    "parallel_slots": 0,
                }
            },
            "services": {"confidential": {"account_id": "acct"}},
        },
    )

    endpoint = local_endpoint.resolve_local_endpoint()

    assert endpoint.is_bundled is False
    assert endpoint.parallel_slots is None
    assert "providers.local.parallel_slots" not in caplog.text


def test_confidential_provenance_block_returns_dict_only():
    block = {"account_id": "acct"}

    assert (
        local_endpoint.confidential_provenance_block(
            {"services": {"confidential": block}}
        )
        is block
    )
    assert local_endpoint.confidential_provenance_block({}) is None
    assert (
        local_endpoint.confidential_provenance_block(
            {"services": {"confidential": "bad"}}
        )
        is None
    )


@pytest.mark.parametrize(
    ("raw", "expected"),
    [
        ("http://h:8080", "http://h:8080"),
        ("http://h:8080/v1", "http://h:8080"),
        ("http://h:8080/v1/", "http://h:8080"),
        (" http://h:8080/openai/v1/ ", "http://h:8080/openai"),
    ],
)
def test_normalize_local_endpoint_url(raw, expected):
    assert local_endpoint.normalize_local_endpoint_url(raw) == expected


def test_probe_local_endpoint_treats_any_response_as_reachable(monkeypatch):
    calls = []

    def fake_get(url, timeout):
        calls.append((url, timeout))
        return object()

    import httpx

    monkeypatch.setattr(httpx, "get", fake_get)
    endpoint = local_endpoint.LocalEndpoint(
        base_url="http://h:8080",
        served_model_id="model",
        credential=None,
        is_bundled=False,
    )

    assert local_endpoint.probe_local_endpoint(endpoint, timeout_s=0.2) == (True, None)
    assert calls == [("http://h:8080", 0.2)]


def test_probe_local_endpoint_skips_get_for_confidential_status(monkeypatch):
    import httpx

    def fake_get(url, timeout):
        raise AssertionError("endpoint probe attempted")

    monkeypatch.setattr(httpx, "get", fake_get)
    monkeypatch.setattr(
        "solstone.think.services.spp_transport.confidential_probe_status",
        lambda: (False, "attestation_not_yet_verified"),
    )
    endpoint = local_endpoint.LocalEndpoint(
        base_url="https://spp.example.test",
        served_model_id="model",
        credential=None,
        is_bundled=False,
    )

    assert local_endpoint.probe_local_endpoint(endpoint) == (
        False,
        "attestation_not_yet_verified",
    )


@pytest.mark.parametrize("exc", ["connect", "timeout"])
def test_probe_local_endpoint_reports_transport_failures(monkeypatch, exc):
    import httpx

    error = (
        httpx.ConnectError("connection refused")
        if exc == "connect"
        else httpx.ReadTimeout("too slow")
    )

    def fake_get(url, timeout):
        raise error

    monkeypatch.setattr(httpx, "get", fake_get)
    endpoint = local_endpoint.LocalEndpoint(
        base_url="http://h:8080",
        served_model_id="model",
        credential=None,
        is_bundled=False,
    )

    reachable, detail = local_endpoint.probe_local_endpoint(endpoint)

    assert reachable is False
    assert detail == str(error)


class BadRequestError(Exception):
    status_code = 400


class InternalServerError(Exception):
    status_code = 500


class APIConnectionError(Exception):
    pass


class ConnectError(Exception):
    pass


class ConnectTimeout(Exception):
    pass


class ReadTimeout(Exception):
    pass


class PoolTimeout(Exception):
    pass


class TimeoutException(Exception):
    pass


class NetworkError(Exception):
    pass


class RequestError(Exception):
    pass


def test_classify_byo_cogitate_error_contract_by_status_or_name():
    assert (
        local_endpoint.classify_byo_cogitate_error(BadRequestError("bad request"))
        == "local_endpoint_contract_failed"
    )


@pytest.mark.parametrize(
    "inner",
    [
        ConnectError("down"),
        APIConnectionError("api down"),
        ConnectTimeout("connect timeout"),
        ReadTimeout("read timeout"),
        PoolTimeout("pool timeout"),
        TimeoutException("timeout"),
        NetworkError("network"),
        RequestError("request"),
    ],
)
def test_classify_byo_cogitate_error_unreachable_by_cause_chain(inner):
    exc = RuntimeError("outer")
    exc.__cause__ = inner

    assert (
        local_endpoint.classify_byo_cogitate_error(exc) == "local_endpoint_unreachable"
    )


@pytest.mark.parametrize(
    "inner",
    [
        ConnectError("down"),
        APIConnectionError("api down"),
        ConnectTimeout("connect timeout"),
        ReadTimeout("read timeout"),
        PoolTimeout("pool timeout"),
        TimeoutException("timeout"),
        NetworkError("network"),
        RequestError("request"),
    ],
)
def test_is_byo_network_error_matches_names_over_cause_chain(inner):
    exc = RuntimeError("outer")
    exc.__cause__ = inner

    assert local_endpoint.is_byo_network_error(exc) is True


def test_is_byo_network_error_ignores_non_network_names():
    assert local_endpoint.is_byo_network_error(InternalServerError("server")) is False


def test_classify_byo_cogitate_error_unreachable_by_internal_server():
    assert (
        local_endpoint.classify_byo_cogitate_error(InternalServerError("connection"))
        == "local_endpoint_unreachable"
    )


def test_classify_byo_cogitate_error_returns_none_for_unknown():
    assert local_endpoint.classify_byo_cogitate_error(RuntimeError("unknown")) is None


def test_redact_event_payload_recurses_through_values():
    sentinel = "SENTINEL-BYO-CRED-9f3a2b"
    payload = {
        "error": f"bad {sentinel}",
        "raw": {
            "api_key": sentinel,
            "headers": {"Authorization": f"Bearer {sentinel}"},
            "nested": [f"x-{sentinel}", {"trace": sentinel}],
            "tuple": (sentinel, 7),
        },
        "status": 503,
        "ok": False,
        "none": None,
    }

    assert sentinel in json.dumps(payload)

    redacted = local_endpoint.redact_event_payload(payload, sentinel)

    assert sentinel not in json.dumps(redacted)
    assert redacted["raw"]["api_key"] == "***"
    assert redacted["raw"]["headers"]["Authorization"] == "Bearer ***"
    assert redacted["raw"]["tuple"] == ("***", 7)
    assert redacted["status"] == 503
    assert redacted["ok"] is False
    assert redacted["none"] is None


def test_redact_exception_credential_scrubs_chain():
    sentinel = "SENTINEL-BYO-CRED-9f3a2b"
    try:
        try:
            raise RuntimeError(f"inner {sentinel}")
        except RuntimeError as cause:
            raise ValueError(f"outer {sentinel}", 7) from cause
    except ValueError as exc:
        raised = exc

    assert sentinel in str(raised)
    assert raised.__cause__ is not None
    assert sentinel in str(raised.__cause__)
    assert sentinel in "".join(
        traceback.format_exception(type(raised), raised, raised.__traceback__)
    )

    redacted = local_endpoint.redact_exception_credential(raised, sentinel)

    assert redacted is raised
    assert sentinel not in str(raised)
    assert sentinel not in str(raised.__cause__)
    serialized = "".join(
        traceback.format_exception(type(raised), raised, raised.__traceback__)
    )
    assert sentinel not in serialized
    assert "***" in serialized

    untouched = RuntimeError(f"still {sentinel}")
    assert local_endpoint.redact_exception_credential(untouched, None) is untouched
    assert sentinel in str(untouched)
    assert local_endpoint.redact_exception_credential(untouched, "") is untouched
    assert sentinel in str(untouched)


def test_wrap_on_event_redacting_passthrough_without_sink_or_credential():
    events = []

    def on_event(event):
        events.append(event)

    assert local_endpoint.wrap_on_event_redacting(None, "token") is None
    assert local_endpoint.wrap_on_event_redacting(on_event, None) is on_event
    assert local_endpoint.wrap_on_event_redacting(on_event, "") is on_event


def test_wrap_on_event_redacting_covers_event_surfaces():
    sentinel = "SENTINEL-BYO-CRED-9f3a2b"
    events = [
        {
            "event": "error",
            "error": f"terminal {sentinel}",
            "trace": f"Trace {sentinel}",
        },
        {
            "event": "tool_start",
            "raw": {
                "api_key": sentinel,
                "headers": {"Authorization": f"Bearer {sentinel}"},
            },
        },
        {"event": "thinking", "raw": {"message": f"thinking {sentinel}"}},
        {"event": "tool_end", "raw": [{"result": sentinel}]},
        {
            "event": "error",
            "error": f"agent {sentinel}",
            "raw": {"detail": sentinel},
        },
    ]
    forwarded = []
    wrapped = local_endpoint.wrap_on_event_redacting(forwarded.append, sentinel)

    assert sentinel in json.dumps(events)
    for event in events:
        wrapped(event)

    assert sentinel not in json.dumps(forwarded)
