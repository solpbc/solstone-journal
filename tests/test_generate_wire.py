# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Contract tests for the one-record Python generate wire."""

from __future__ import annotations

import io
import json
import re
import sys
from pathlib import Path

import pytest

from solstone.think import generate_wire
from solstone.think.models import (
    AttestationFailedError,
    AttestationNotVerifiedError,
    AttestationStaleError,
    IncompleteJSONError,
    IncompleteTextError,
    NoBrainConfiguredError,
    ProviderResponseInvalidError,
    SchemaValidationError,
)
from solstone.think.responsiveness import NonResponsiveOutputError


def _request(**overrides):
    request = {
        "schema": generate_wire.REQUEST_SCHEMA,
        "contents": [{"type": "text", "text": "describe"}],
        "context": "observe.depict",
    }
    request.update(overrides)
    return request


def test_schema_constants_are_pinned_to_native_source():
    source = (Path(__file__).parents[1] / "core/crates/solstone-core-depict/src/lib.rs").read_text()
    for name, expected in {
        "REQUEST_SCHEMA": generate_wire.REQUEST_SCHEMA,
        "RESPONSE_SCHEMA": generate_wire.RESPONSE_SCHEMA,
        "ERROR_SCHEMA": generate_wire.ERROR_SCHEMA,
    }.items():
        found = re.search(rf'pub const {name}: &str = "([^"]+)";', source)
        assert found, name
        assert found.group(1) == expected


@pytest.mark.parametrize(
    "wire_request",
    [
        {},
        _request(extra=True),
        _request(contents="wrong"),
        _request(context=3),
        _request(contents=[{"type": "text", "text": 3}]),
    ],
)
def test_handle_request_rejects_malformed_requests(wire_request):
    with pytest.raises(generate_wire.WireError) as exc:
        generate_wire.handle_request(wire_request)
    assert exc.value.reason == "malformed-request"
    assert exc.value.exit_code == 64


def test_main_rejects_bad_json(monkeypatch, capsys):
    monkeypatch.setattr(sys, "stdin", io.StringIO("{"))
    with pytest.raises(SystemExit) as exc:
        generate_wire.main()
    assert exc.value.code == 64
    assert capsys.readouterr().err == json.dumps(
        {"schema": generate_wire.ERROR_SCHEMA, "reason": "malformed-request", "detail": "stdin is not valid JSON"}
    ) + "\n"


@pytest.mark.parametrize(
    ("exception", "reason", "exit_code"),
    [
        (NoBrainConfiguredError(), "no-engine-configured", 69),
        (AttestationNotVerifiedError(), "attestation-not-verified", 75),
        (AttestationFailedError("bad"), "attestation-failed", 75),
        (AttestationStaleError("old"), "attestation-stale", 75),
        (IncompleteJSONError("length", "{"), "incomplete-json", 75),
        (IncompleteTextError("length", "text"), "incomplete-text", 75),
        (ProviderResponseInvalidError("malformed"), "provider-response-invalid", 75),
        (SchemaValidationError([], "{}"), "schema-validation-failed", 75),
        (NonResponsiveOutputError(), "non-responsive-output", 75),
    ],
)
def test_typed_generate_errors_have_stable_wire_reasons(monkeypatch, exception, reason, exit_code):
    monkeypatch.setattr(generate_wire, "generate_with_result", lambda **_: (_ for _ in ()).throw(exception))
    with pytest.raises(generate_wire.WireError) as exc:
        generate_wire.handle_request(_request())
    assert exc.value.reason == reason
    assert exc.value.exit_code == exit_code


def test_success_preserves_full_generate_result_envelope(monkeypatch):
    result = {
        "text": "description",
        "model": "test-model",
        "usage": {"input_tokens": 4},
        "finish_reason": "stop",
        "thinking": "brief",
        "schema_validation": {"valid": True},
        "input_budget": {"tokens": 1},
        "request_budget": {"tokens": 2},
        "inference": {"provider": "test"},
    }
    captured = {}
    monkeypatch.setattr(generate_wire, "generate_with_result", lambda **kwargs: captured.update(kwargs) or result)
    response = generate_wire.handle_request(_request())
    assert response == {"schema": generate_wire.RESPONSE_SCHEMA, "result": result}
    assert captured["temperature"] == 0.3
    assert captured["max_output_tokens"] == 16384
    assert captured["enforce_responsiveness"] is True


def test_request_contract_has_no_provider_or_model_field():
    assert "provider" not in generate_wire._REQUEST_FIELDS
    assert "model" not in generate_wire._REQUEST_FIELDS
