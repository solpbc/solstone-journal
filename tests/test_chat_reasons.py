# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import json
from pathlib import Path

from solstone.convey.provider_readiness import (
    DISPLAY_NAMES,
    chat_reason_projection,
    chat_view,
)

EXPECTED_CODES = {
    "provider_key_missing",
    "thinking_engine_not_chosen",
    "ram_insufficient",
    "gpu_unavailable",
    "gpu_probe_failed",
    "local_model_missing",
    "model_missing",
    "binary_missing",
    "local_model_installing",
    "local_model_loading",
    "local_model_not_ready",
    "local_server_unhealthy",
    "local_endpoint_unreachable",
    "local_endpoint_contract_failed",
    "unsupported_platform",
    "host_unfit",
    "unsupported_model",
    "sha256_mismatch",
    "archive_path_traversal",
    "provider_key_invalid",
    "provider_quota_exceeded",
    "network_unreachable",
    "provider_response_invalid",
    "provider_unavailable",
    "chat_pipeline_unavailable",
    "chat_timeout",
    "local_queue_timeout",
    "local_capacity_exhausted",
    "context_window_exceeded",
    "context_budget_exceeded",
    "incomplete_json_length",
    "incomplete_text_length",
    "max_turns_exhausted",
    "no_output",
    "token_budget_exceeded",
    "wall_clock_exceeded",
    "unknown",
}


def _extract_frozen_object(text: str, name: str) -> dict:
    marker = f"const {name} = Object.freeze("
    start = text.index(marker) + len(marker)
    depth = 0
    in_string = False
    escaped = False
    object_start = None

    for index in range(start, len(text)):
        char = text[index]
        if in_string:
            if escaped:
                escaped = False
            elif char == "\\":
                escaped = True
            elif char == '"':
                in_string = False
            continue
        if char == '"':
            in_string = True
            continue
        if char == "{":
            if object_start is None:
                object_start = index
            depth += 1
        elif char == "}":
            depth -= 1
            if depth == 0 and object_start is not None:
                return json.loads(text[object_start : index + 1])

    raise AssertionError(f"Could not extract {name}")


def _render_js_chat_reason(
    reasons: dict, display_names: dict, code: str, provider: str
) -> dict:
    reason = reasons.get(code)
    if reason is None:
        return {"code": code, "message": code, "action": None}

    provider_slug = str(provider or "")
    if code == "unknown":
        display_name = display_names.get(provider_slug)
        message = (
            f"something went wrong with {display_name}"
            if display_name
            else reason["template"]
        )
        return {"code": code, "message": message, "action": None}

    display_name = display_names.get(provider_slug, provider_slug)
    message = reason["template"].replace("{provider}", display_name)
    action = (
        {"label": reason["action"]["label"], "href": reason["action"]["href"]}
        if reason["action"]
        else None
    )
    return {"code": code, "message": message, "action": action}


def test_registry_shape():
    reasons = chat_reason_projection()
    assert set(reasons) == EXPECTED_CODES
    for reason in reasons.values():
        assert reason["template"]
        action = reason["action"]
        assert action is None or set(action) == {"label", "href"}


def test_render_known_codes():
    for code, reason in chat_reason_projection().items():
        rendered = chat_view(code, "google")
        assert rendered["code"] == code
        assert rendered["message"]
        if code == "unknown":
            assert rendered["message"] == "something went wrong with Gemini"
        elif "{provider}" in reason["template"]:
            assert "Gemini" in rendered["message"]
        if code == "provider_key_invalid":
            assert rendered["action"] == {
                "label": "Open Thinking",
                "href": "/app/thinking/#main",
            }
        else:
            assert rendered["action"] == reason["action"]


def test_render_display_names():
    for slug, display in DISPLAY_NAMES.items():
        rendered = chat_view("provider_key_invalid", slug)
        assert display in rendered["message"]


def test_render_unknown_code():
    assert chat_view("not_a_real_code", "") == {
        "code": "not_a_real_code",
        "message": "not_a_real_code",
        "action": None,
    }


def test_render_unknown_with_known_provider():
    for slug, display_name in DISPLAY_NAMES.items():
        assert chat_view("unknown", slug) == {
            "code": "unknown",
            "message": f"something went wrong with {display_name}",
            "action": None,
        }


def test_render_unknown_with_empty_or_unknown_provider():
    for provider in ("", "weirdslug"):
        assert chat_view("unknown", provider) == {
            "code": "unknown",
            "message": "chat had trouble",
            "action": None,
        }


def test_render_empty_provider():
    assert chat_view("network_unreachable", "") == {
        "code": "network_unreachable",
        "message": "I couldn't reach the network",
        "action": None,
    }


def test_render_local_runtime_codes():
    expected = {
        "local_queue_timeout": "the local model was busy and couldn't start in time",
        "local_capacity_exhausted": (
            "the local model was busy and could not finish this request"
        ),
        "context_budget_exceeded": "the request was too long for the local model",
    }
    for code, message in expected.items():
        rendered = chat_view(code, "local")
        assert rendered == {"code": code, "message": message, "action": None}
        assert rendered["message"] != code


def test_render_no_placeholder_artifacts_for_all_providers():
    providers = ["", "google", "openai", "anthropic", "local", "weirdslug"]
    for code in chat_reason_projection():
        for provider in providers:
            message = chat_view(code, provider)["message"]
            assert "{provider}" not in message
            assert "None" not in message


def test_js_parity():
    js_path = Path("solstone/convey/static/chat_reasons.js")
    text = js_path.read_text(encoding="utf-8")
    js_reasons = _extract_frozen_object(text, "CHAT_REASONS")
    js_display_names = _extract_frozen_object(text, "CHAT_REASON_DISPLAY_NAMES")

    py_reasons = chat_reason_projection()

    assert js_reasons == py_reasons
    assert js_display_names == DISPLAY_NAMES

    for code, reason in py_reasons.items():
        for provider, display in DISPLAY_NAMES.items():
            js_rendered = _render_js_chat_reason(
                js_reasons, js_display_names, code, provider
            )
            py_rendered = chat_view(code, provider)
            assert js_rendered == py_rendered
            if code == "unknown":
                continue
            expected = reason["template"].replace("{provider}", display)
            assert py_rendered["message"] == expected

    removed_constants = ("CHAT_" + "TROUBLE_REASON", "CHAT_" + "WATCHDOG_REASON")
    assert all(name not in text for name in removed_constants)


def test_no_hardcoded_chat_had_trouble_literal():
    roots = [Path("solstone/apps/chat"), Path("solstone/convey")]
    excluded_names = {"provider_readiness.py", "chat_reasons.js"}
    text_suffixes = {".css", ".html", ".js", ".json", ".md", ".py", ".txt"}

    offenders = []
    for root in roots:
        for path in root.rglob("*"):
            if not path.is_file() or path.suffix not in text_suffixes:
                continue
            if path.name in excluded_names or "tests" in path.parts:
                continue
            if "chat had trouble" in path.read_text(encoding="utf-8"):
                offenders.append(str(path))

    assert offenders == []
