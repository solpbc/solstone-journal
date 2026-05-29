# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import urllib.parse
from pathlib import Path
from typing import Any

import pytest
from flask import Flask
from werkzeug.routing import BuildError, Rule

from solstone.convey.secure_listener.wsgi import certless_target_allowed
from solstone.think.link.nonces import NonceStore
from solstone.think.link.paths import nonces_path
from tests.link.certless_helpers import (
    certless_identity,
    dispatch_request,
    make_convey_app,
)


def test_route_enumeration_allows_only_pair_post(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    app, _journal = make_convey_app(tmp_path, monkeypatch, link={"posture": "spl"})

    seen_pair_post = False
    for rule in app.url_map.iter_rules():
        for method in _route_methods(rule):
            path = _path_for_rule(app, rule, method)
            allowed = certless_target_allowed(app, path, method)
            expected = rule.endpoint == "app:link.pair" and method == "POST"
            assert allowed is expected, (rule.endpoint, method, path)
            seen_pair_post = seen_pair_post or expected

    assert seen_pair_post
    assert certless_target_allowed(app, "/app/link/pair", "POST") is True
    assert certless_target_allowed(app, "/app/link/pair", "GET") is False


def test_named_non_pair_endpoints_are_refused(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    app, _journal = make_convey_app(tmp_path, monkeypatch, link={"posture": "spl"})
    endpoints = {
        "app:link.pair_start": "POST",
        "app:link.by_code": "POST",
        "app:link.api_status": "GET",
        "app:observer.ingest_upload": "POST",
        "app:observer.ingest_event": "POST",
        "app:observer.ingest_segments": "GET",
        "app:observer.ingest_transfer": "POST",
        "app:observer.ingest_manifest": "GET",
        "app:observer.ingest_manifest_day": "GET",
        "app:import.journal_source_manifest": "GET",
        "app:import.ingest_segments": "POST",
        "app:import.ingest_entities": "POST",
        "app:import.ingest_facets": "POST",
        "app:import.ingest_imports": "POST",
        "app:import.ingest_config": "POST",
        "root.callosum_sse": "GET",
        "static": "GET",
    }

    for endpoint, method in endpoints.items():
        path = _path_for_endpoint(app, endpoint, method)
        assert certless_target_allowed(app, path, method) is False, (
            endpoint,
            method,
            path,
        )

    for path, method in (
        ("/", "GET"),
        ("/sse/events", "GET"),
        ("/app/link/api/devices", "GET"),
        ("/api/config", "GET"),
    ):
        assert certless_target_allowed(app, path, method) is False


@pytest.mark.asyncio
async def test_evasion_paths_are_refused(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    app, _journal = make_convey_app(tmp_path, monkeypatch, link={"posture": "spl"})
    NonceStore(nonces_path()).add("live", "phone")

    for raw_path in (
        "/app/link/%2e%2e/pair",
        "/app/link/../pair-start",
        "/app/link/./pair",
        "/app/link/pair.",
        "/app/link/pair/.",
    ):
        path_info = urllib.parse.unquote(raw_path)
        assert certless_target_allowed(app, path_info, "POST") is False, raw_path

    response = await dispatch_request(
        app,
        certless_identity(),
        "POST",
        "/app/link%2Fpair",
        body=b"{}",
        headers={"content-type": "application/json"},
    )

    assert response.status == 403
    assert b"pairing tunnel may only use /app/link/pair" in response.body


@pytest.mark.asyncio
async def test_window_recheck_refuses_before_handler(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    app, _journal = make_convey_app(tmp_path, monkeypatch, link={"posture": "spl"})
    called = False

    def pair_spy() -> str:
        nonlocal called
        called = True
        return "reached"

    app.view_functions["app:link.pair"] = pair_spy

    response = await dispatch_request(
        app,
        certless_identity(),
        "POST",
        "/app/link/pair",
        body=b"{}",
        headers={"content-type": "application/json"},
    )

    assert response.status == 403
    assert b"pairing window closed" in response.body
    assert called is False


def _route_methods(rule: Rule) -> list[str]:
    return sorted((rule.methods or set()) - {"HEAD", "OPTIONS"})


def _path_for_endpoint(app: Flask, endpoint: str, method: str) -> str:
    rules = [
        rule
        for rule in app.url_map.iter_rules(endpoint)
        if method in (rule.methods or set())
    ]
    assert rules, f"missing route {endpoint} {method}"
    return _path_for_rule(app, rules[0], method)


def _path_for_rule(app: Flask, rule: Rule, method: str) -> str:
    adapter = app.url_map.bind("solstone.local", url_scheme="https")
    try:
        built = adapter.build(
            rule.endpoint,
            _values_for_rule(rule),
            method=method,
            force_external=False,
        )
    except BuildError as exc:
        raise AssertionError(f"could not build {rule.endpoint} {method}") from exc
    return urllib.parse.urlsplit(built).path


def _values_for_rule(rule: Rule) -> dict[str, Any]:
    return {argument: _dummy_value(rule, argument) for argument in rule.arguments}


def _dummy_value(rule: Rule, argument: str) -> object:
    converter = rule._converters.get(argument)
    converter_name = converter.__class__.__name__ if converter is not None else ""
    if converter_name in {"IntegerConverter", "FloatConverter"}:
        return 1
    if argument in {"day", "date"}:
        return "20260413"
    if argument == "month":
        return "202604"
    if argument == "filename":
        return "app.css"
    if argument in {"key", "key_prefix"}:
        return "deadbeef"
    if converter_name == "PathConverter":
        return "path/value"
    return "value"
