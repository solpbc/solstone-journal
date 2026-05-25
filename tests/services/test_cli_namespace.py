# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import io
import json
import ssl
import urllib.error
from typing import Any

import pytest

from solstone.think import sol_cli
from solstone.think.journal_config import write_journal_config
from solstone.think.services import cli


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


def _payload(suffix: str = "one") -> dict[str, str]:
    return {
        "google_api_key": f"google-{suffix}",
        "dispatch_token": f"dispatch-{suffix}",
        "account_id": f"acct-{suffix}",
        "created_at": "2026-05-24T00:00:00Z",
    }


def _payload_body(suffix: str = "one") -> bytes:
    return json.dumps(_payload(suffix)).encode("utf-8")


def _http_error(code: int) -> urllib.error.HTTPError:
    return urllib.error.HTTPError(
        "https://services.solstone.app/handoff/scout",
        code,
        "error",
        hdrs=None,
        fp=io.BytesIO(b""),
    )


def _install_urlopen(monkeypatch, items: list[Any]):
    calls = []
    queue = list(items)

    def fake_urlopen(request, timeout):
        calls.append((request, timeout))
        item = queue.pop(0)
        if isinstance(item, BaseException):
            raise item
        return item

    monkeypatch.setattr(cli.urllib.request, "urlopen", fake_urlopen)
    return calls


@pytest.fixture
def browser_ready(monkeypatch):
    opened = []
    monkeypatch.setattr(cli, "_is_headless", lambda: False)
    monkeypatch.setattr(cli, "_open_browser", lambda url: opened.append(url) or True)
    monkeypatch.delenv("SERVICES_PORTAL_URL", raising=False)
    return opened


def test_services_help_lists_enable(capsys) -> None:
    with pytest.raises(SystemExit) as exc:
        cli.main(["--help"])

    assert exc.value.code == 0
    assert "enable" in capsys.readouterr().out


def test_sol_cli_registers_services_command() -> None:
    assert sol_cli.COMMANDS["services"] == "solstone.think.services"
    assert sol_cli.GROUPS["Services"] == ["services"]


def test_enable_scout_help_lists_flags(capsys) -> None:
    with pytest.raises(SystemExit) as exc:
        cli.main(["enable", "scout", "--help"])

    assert exc.value.code == 0
    out = capsys.readouterr().out
    assert "--force" in out
    assert "--wait" in out


def test_unknown_service_exits_2(capsys) -> None:
    with pytest.raises(SystemExit) as exc:
        cli.main(["enable", "bad"])

    assert exc.value.code == 2
    assert capsys.readouterr().err.startswith("unknown_service: ")


def test_happy_path_writes_handoff(journal_copy, browser_ready, monkeypatch, capsys):
    calls = _install_urlopen(monkeypatch, [FakeResponse(200, _payload_body())])

    assert cli.main(["enable", "scout"]) == 0

    captured = capsys.readouterr()
    assert captured.err == ""
    assert cli.STDOUT_OPENING in captured.out
    assert cli.STDOUT_WAITING in captured.out
    assert cli.STDOUT_SUCCESS in captured.out
    assert browser_ready[0].startswith("https://services.solstone.app/enable/scout?")
    request, timeout = calls[0]
    assert request.full_url.startswith("https://services.solstone.app/handoff/scout?")
    assert request.headers["User-agent"].startswith("solstone-cli/")
    assert request.headers["Connection"] == "close"
    assert timeout == cli.POLL_TIMEOUT_SECONDS
    config = json.loads((journal_copy / "config" / "journal.json").read_text())
    assert config["env"]["GOOGLE_API_KEY"] == "google-one"
    assert config["services"]["scout"]["account_id"] == "acct-one"


@pytest.mark.parametrize(
    ("item", "token"),
    [
        (_http_error(410), "consent_link_expired"),
        (_http_error(400), "nonce_invalid"),
        (urllib.error.URLError("down"), "portal_unreachable"),
        (urllib.error.URLError(ssl.SSLError("bad cert")), "tls_verification_failed"),
        (FakeResponse(500), "unexpected_payload"),
        (FakeResponse(200, b"{"), "unexpected_payload"),
        (
            FakeResponse(200, json.dumps({"google_api_key": "only"}).encode()),
            "unexpected_payload",
        ),
    ],
)
def test_error_paths_emit_canonical_tokens(
    journal_copy, browser_ready, monkeypatch, capsys, item, token
) -> None:
    _install_urlopen(monkeypatch, [item])

    assert cli.main(["enable", "scout"]) == 1

    assert capsys.readouterr().err.startswith(f"{token}: ")


def test_204_sequence_then_200_succeeds(
    journal_copy, browser_ready, monkeypatch, capsys
):
    _install_urlopen(
        monkeypatch,
        [FakeResponse(204), FakeResponse(200, _payload_body("two"))],
    )

    assert cli.main(["enable", "scout"]) == 0

    assert cli.STDOUT_SUCCESS in capsys.readouterr().out
    config = json.loads((journal_copy / "config" / "journal.json").read_text())
    assert config["services"]["scout"]["account_id"] == "acct-two"


def test_consent_timeout_after_204(journal_copy, browser_ready, monkeypatch, capsys):
    monkeypatch.setattr(cli, "_wait_seconds", lambda _value: 0)
    _install_urlopen(monkeypatch, [FakeResponse(204)])

    assert cli.main(["enable", "scout", "--wait", "1"]) == 1

    assert capsys.readouterr().err.startswith("consent_timeout: ")


def test_write_failure_after_200_maps_to_write_failed(
    journal_copy, browser_ready, monkeypatch, capsys
):
    _install_urlopen(monkeypatch, [FakeResponse(200, _payload_body())])
    monkeypatch.setattr(
        cli,
        "provision_scout_handoff",
        lambda _payload: (_ for _ in ()).throw(OSError("disk full")),
    )

    assert cli.main(["enable", "scout"]) == 1

    assert capsys.readouterr().err.startswith("write_failed: ")


def test_already_enabled_is_idempotent_success(
    journal_copy, monkeypatch, capsys
) -> None:
    config = json.loads((journal_copy / "config" / "journal.json").read_text())
    config.setdefault("env", {})["GOOGLE_API_KEY"] = "existing"
    config.setdefault("services", {})["scout"] = {"account_id": "acct"}
    write_journal_config(config)
    opened = []
    monkeypatch.setattr(cli, "_open_browser", lambda url: opened.append(url) or True)

    assert cli.main(["enable", "scout"]) == 0

    assert opened == []
    assert capsys.readouterr().err.startswith("already_enabled: ")


def test_manual_key_present_is_idempotent_success(
    journal_copy, monkeypatch, capsys
) -> None:
    config = json.loads((journal_copy / "config" / "journal.json").read_text())
    config.setdefault("env", {})["GOOGLE_API_KEY"] = "manual"
    config.pop("services", None)
    write_journal_config(config)
    monkeypatch.setattr(cli, "_open_browser", lambda _url: pytest.fail("opened"))

    assert cli.main(["enable", "scout"]) == 0

    err = capsys.readouterr().err
    assert err.startswith("manual_key_present: ")
    assert "A manual Gemini key is already present in journal config." in err


def test_force_bypasses_manual_key_detection(
    journal_copy, browser_ready, monkeypatch, capsys
) -> None:
    config = json.loads((journal_copy / "config" / "journal.json").read_text())
    config.setdefault("env", {})["GOOGLE_API_KEY"] = "manual"
    config.pop("services", None)
    write_journal_config(config)
    _install_urlopen(monkeypatch, [FakeResponse(200, _payload_body("force"))])

    assert cli.main(["enable", "scout", "--force"]) == 0

    assert cli.STDOUT_SUCCESS in capsys.readouterr().out
    saved = json.loads((journal_copy / "config" / "journal.json").read_text())
    assert saved["env"]["GOOGLE_API_KEY"] == "google-force"


def test_headless_prints_url_and_exits_2(journal_copy, monkeypatch, capsys) -> None:
    monkeypatch.setattr(cli, "_is_headless", lambda: True)
    monkeypatch.setattr(cli, "_mint_nonce", lambda: "A" * 52)

    assert cli.main(["enable", "scout"]) == 2

    captured = capsys.readouterr()
    assert captured.out.strip().endswith("/enable/scout?nonce=" + "A" * 52)
    assert captured.err.startswith("headless_no_browser: ")


def test_open_browser_false_maps_to_headless(journal_copy, monkeypatch, capsys) -> None:
    monkeypatch.setattr(cli, "_is_headless", lambda: False)
    monkeypatch.setattr(cli, "_open_browser", lambda _url: False)
    monkeypatch.setattr(cli, "_mint_nonce", lambda: "B" * 52)

    assert cli.main(["enable", "scout"]) == 2

    captured = capsys.readouterr()
    assert "/enable/scout?nonce=" + "B" * 52 in captured.out
    assert captured.err.startswith("headless_no_browser: ")


def test_services_portal_url_override(journal_copy, browser_ready, monkeypatch):
    monkeypatch.setenv("SERVICES_PORTAL_URL", "https://example.test/base/")
    calls = _install_urlopen(monkeypatch, [FakeResponse(200, _payload_body())])

    assert cli.main(["enable", "scout"]) == 0

    assert browser_ready[0].startswith("https://example.test/base/enable/scout?")
    assert calls[0][0].full_url.startswith("https://example.test/base/handoff/scout?")


def test_journal_not_initialized_exits_1(tmp_path, monkeypatch, capsys) -> None:
    journal = tmp_path / "journal"
    (journal / "config").mkdir(parents=True)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))

    assert cli.main(["enable", "scout"]) == 1

    assert capsys.readouterr().err.startswith("journal_not_initialized: ")
