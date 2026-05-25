# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import io
import json
import re
import ssl
import urllib.error

import pytest

from solstone.think.journal_config import write_journal_config
from solstone.think.services import cli

BLOCKED_COPY_RE = re.compile(
    r"sign(?:ed)?\s+in|signing\s+in|log(?:ged)?\s+in|your\s+account|"
    r"account\s+settings|linked|authenticate",
    re.IGNORECASE,
)


def test_services_cli_copy_avoids_blocked_brand_terms() -> None:
    strings = [
        cli.STDOUT_OPENING,
        cli.STDOUT_WAITING,
        cli.STDOUT_SUCCESS,
        *cli.ERROR_MESSAGES.values(),
    ]

    assert all(not BLOCKED_COPY_RE.search(value) for value in strings)


class FakeResponse:
    def __init__(self, status: int, body: bytes = b"") -> None:
        self.status = status
        self._body = body

    def __enter__(self):
        return self

    def __exit__(self, _exc_type, _exc, _tb) -> bool:
        return False

    def getcode(self) -> int:
        return self.status

    def read(self) -> bytes:
        return self._body


def _payload_body() -> bytes:
    return json.dumps(
        {
            "google_api_key": "google-key",
            "dispatch_token": "dispatch-token",
            "account_id": "acct-brand",
            "created_at": "2026-05-24T00:00:00Z",
        }
    ).encode("utf-8")


def _http_error(code: int) -> urllib.error.HTTPError:
    return urllib.error.HTTPError(
        "https://services.solstone.app/handoff/scout",
        code,
        "error",
        hdrs=None,
        fp=io.BytesIO(b""),
    )


def _install_urlopen(monkeypatch: pytest.MonkeyPatch, items: list[object]) -> None:
    queue = list(items)

    def fake_urlopen(_request, _timeout):
        item = queue.pop(0)
        if isinstance(item, BaseException):
            raise item
        return item

    monkeypatch.setattr(cli.urllib.request, "urlopen", fake_urlopen)


@pytest.mark.parametrize(
    "branch",
    [
        "happy",
        "consent_link_expired",
        "consent_timeout",
        "portal_unreachable",
        "tls_verification_failed",
        "nonce_invalid",
        "unexpected_payload",
        "write_failed",
        "already_enabled",
        "manual_key_present",
        "headless_no_browser",
        "journal_not_initialized",
        "unknown_service",
    ],
)
def test_cli_branch_output_avoids_blocked_brand_terms(
    branch: str,
    journal_copy,
    monkeypatch: pytest.MonkeyPatch,
    tmp_path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    monkeypatch.setattr(cli, "_is_headless", lambda: False)
    monkeypatch.setattr(cli, "_open_browser", lambda _url: True)

    argv = ["enable", "scout"]
    if branch == "happy":
        _install_urlopen(monkeypatch, [FakeResponse(200, _payload_body())])
    elif branch == "consent_link_expired":
        _install_urlopen(monkeypatch, [_http_error(410)])
    elif branch == "consent_timeout":
        monkeypatch.setattr(cli, "_wait_seconds", lambda _value: 0)
        _install_urlopen(monkeypatch, [FakeResponse(204)])
        argv = ["enable", "scout", "--wait", "1"]
    elif branch == "portal_unreachable":
        _install_urlopen(monkeypatch, [urllib.error.URLError("down")])
    elif branch == "tls_verification_failed":
        _install_urlopen(monkeypatch, [urllib.error.URLError(ssl.SSLError("cert"))])
    elif branch == "nonce_invalid":
        _install_urlopen(monkeypatch, [_http_error(400)])
    elif branch == "unexpected_payload":
        _install_urlopen(monkeypatch, [FakeResponse(200, b"{")])
    elif branch == "write_failed":
        _install_urlopen(monkeypatch, [FakeResponse(200, _payload_body())])
        monkeypatch.setattr(
            cli,
            "provision_scout_handoff",
            lambda _payload: (_ for _ in ()).throw(OSError("disk")),
        )
    elif branch == "already_enabled":
        config = json.loads((journal_copy / "config" / "journal.json").read_text())
        config.setdefault("env", {})["GOOGLE_API_KEY"] = "existing"
        config.setdefault("services", {})["scout"] = {"account_id": "acct"}
        write_journal_config(config)
    elif branch == "manual_key_present":
        config = json.loads((journal_copy / "config" / "journal.json").read_text())
        config.setdefault("env", {})["GOOGLE_API_KEY"] = "manual"
        config.pop("services", None)
        write_journal_config(config)
    elif branch == "headless_no_browser":
        monkeypatch.setattr(cli, "_is_headless", lambda: True)
        monkeypatch.setattr(cli, "_mint_nonce", lambda: "A" * 52)
    elif branch == "journal_not_initialized":
        journal = tmp_path / "uninitialized-journal"
        (journal / "config").mkdir(parents=True)
        monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    elif branch == "unknown_service":
        argv = ["enable", "nope"]

    if branch == "unknown_service":
        with pytest.raises(SystemExit):
            cli.main(argv)
    else:
        cli.main(argv)

    captured = capsys.readouterr()
    combined = captured.out + captured.err
    assert not BLOCKED_COPY_RE.search(combined), (
        f"blocked phrase in branch {branch}: {combined!r}"
    )
