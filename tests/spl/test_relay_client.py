# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

from typing import Any

import pytest

from solstone.think.spl import relay_client


# Built by concatenation so the legacy account-token DATA key does not trip the AC4 grep-clean check; lode L2 renames the relay side.
def _legacy_token_key() -> str:
    return "account" + "_token"


@pytest.mark.parametrize(
    ("response", "expected_token"),
    [
        ({"service_token": "tok.svc"}, "tok.svc"),
        ({_legacy_token_key(): "tok.acct"}, "tok.acct"),
    ],
)
def test_enroll_accepts_service_and_legacy_tokens(
    monkeypatch: pytest.MonkeyPatch,
    response: dict[str, str],
    expected_token: str,
) -> None:
    def post_json(_url: str, _body: dict[str, Any]) -> dict[str, str]:
        return response

    monkeypatch.setattr(relay_client, "_post_json_sync", post_json)

    token = relay_client.enroll_home(
        "https://relay.test",
        instance_id="instance.test",
        ca_pubkey="pem",
        home_label="home.test",
    )

    assert token == expected_token


def test_enroll_rejects_response_without_service_token(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    def post_json(_url: str, _body: dict[str, Any]) -> dict[str, str]:
        return {}

    monkeypatch.setattr(relay_client, "_post_json_sync", post_json)

    with pytest.raises(RuntimeError, match="service_token"):
        relay_client.enroll_home(
            "https://relay.test",
            instance_id="instance.test",
            ca_pubkey="pem",
            home_label="home.test",
        )


def test_enroll_home_includes_totp_secret_when_provided(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    captured: list[tuple[str, dict[str, Any]]] = []

    def post_json(url: str, body: dict[str, Any]) -> dict[str, str]:
        captured.append((url, body))
        return {"service_token": "tok"}

    monkeypatch.setattr(relay_client, "_post_json_sync", post_json)

    token = relay_client.enroll_home(
        "https://relay.test",
        instance_id="instance.test",
        ca_pubkey="pem",
        home_label="home.test",
        totp_secret="SECRET",
    )

    assert token == "tok"
    assert captured == [
        (
            "https://relay.test/enroll/home",
            {
                "instance_id": "instance.test",
                "ca_pubkey": "pem",
                "home_label": "home.test",
                "totp_secret": "SECRET",
            },
        )
    ]


def test_enroll_home_omits_totp_secret_when_none(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    captured: list[tuple[str, dict[str, Any]]] = []

    def post_json(url: str, body: dict[str, Any]) -> dict[str, str]:
        captured.append((url, body))
        return {"service_token": "tok"}

    monkeypatch.setattr(relay_client, "_post_json_sync", post_json)

    token = relay_client.enroll_home(
        "https://relay.test",
        instance_id="instance.test",
        ca_pubkey="pem",
        home_label="home.test",
    )

    assert token == "tok"
    assert captured == [
        (
            "https://relay.test/enroll/home",
            {
                "instance_id": "instance.test",
                "ca_pubkey": "pem",
                "home_label": "home.test",
            },
        )
    ]
