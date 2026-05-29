# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import io
import json
import ssl
import stat
import urllib.error
from typing import Any

import pytest

from solstone.think import sol_cli
from solstone.think.journal_config import write_journal_config
from solstone.think.link import relay_client
from solstone.think.link.paths import save_totp_secret, totp_secret_path
from solstone.think.services import cli, portal_client


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

    monkeypatch.setattr(portal_client.urllib.request, "urlopen", fake_urlopen)
    return calls


def _install_spl_relay(
    monkeypatch: pytest.MonkeyPatch,
    captured: list[tuple[str, dict[str, Any]]],
) -> None:
    monkeypatch.setenv("SOL_LINK_RELAY_URL", "https://relay.test")

    def post_json(url: str, body: dict[str, Any]) -> dict[str, str]:
        captured.append((url, body))
        return {"service_token": "tok.spl"}

    monkeypatch.setattr(relay_client, "_post_json_sync", post_json)


def _set_posture(journal_copy, posture: str) -> None:
    config = json.loads((journal_copy / "config" / "journal.json").read_text())
    config.setdefault("link", {})["posture"] = posture
    write_journal_config(config)


def test_mint_device_code_posts_empty_body_and_returns_success(monkeypatch) -> None:
    calls = _install_urlopen(
        monkeypatch,
        [
            FakeResponse(
                200,
                json.dumps(
                    {
                        "nonce": "A" * 52,
                        "code": "SCOUT-2345-6789",
                        "expires_in": 900,
                    }
                ).encode("utf-8"),
            )
        ],
    )

    outcome = portal_client.mint_device_code("https://services.example")

    assert outcome == portal_client.DeviceCodeOutcome(
        kind="success",
        nonce="A" * 52,
        code="SCOUT-2345-6789",
        expires_in=900,
    )
    request, timeout = calls[0]
    assert request.full_url == "https://services.example/enable/scout/code"
    assert request.data == b""
    assert request.get_method() == "POST"
    assert request.headers["User-agent"].startswith("solstone-cli/")
    assert timeout == portal_client.POLL_TIMEOUT_SECONDS


def test_mint_device_code_429_maps_rate_limited(monkeypatch) -> None:
    _install_urlopen(monkeypatch, [_http_error(429)])

    outcome = portal_client.mint_device_code("https://services.example")

    assert outcome.kind == "failed"
    assert outcome.reason == "rate_limited"


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
    assert sol_cli.COMMANDS["services"].module == "solstone.think.services"
    assert "services" in sol_cli.service_help_group().commands


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


def test_enable_spl_happy_path_writes_posture_and_secret(
    journal_copy, monkeypatch, capsys
) -> None:
    captured_bodies: list[tuple[str, dict[str, Any]]] = []
    _install_spl_relay(monkeypatch, captured_bodies)

    assert cli.main(["enable", "spl"]) == 0

    captured = capsys.readouterr()
    assert cli.STDOUT_SPL_SUCCESS in captured.out
    assert captured.err == ""
    config_path = journal_copy / "config" / "journal.json"
    config = json.loads(config_path.read_text("utf-8"))
    secret = json.loads(totp_secret_path().read_text("utf-8"))["totp_secret"]
    assert config["link"]["posture"] == "spl"
    assert stat.S_IMODE(totp_secret_path().stat().st_mode) == 0o600
    assert captured_bodies[0][1]["totp_secret"] == secret
    combined_output = captured.out + captured.err
    assert secret not in combined_output
    assert secret not in config_path.read_text("utf-8")


def test_enable_spl_already_enabled_is_idempotent_success(
    journal_copy, monkeypatch, capsys
) -> None:
    captured_bodies: list[tuple[str, dict[str, Any]]] = []
    _set_posture(journal_copy, "spl")
    save_totp_secret("SECRET")
    _install_spl_relay(monkeypatch, captured_bodies)

    assert cli.main(["enable", "spl"]) == 0

    captured = capsys.readouterr()
    assert captured.out == ""
    assert captured.err.startswith("spl_already_enabled: ")
    assert captured_bodies == []


def test_enable_spl_relay_down_emits_relay_unreachable(
    journal_copy, monkeypatch, capsys
) -> None:
    monkeypatch.setenv("SOL_LINK_RELAY_URL", "https://relay.test")

    def post_json(_url: str, _body: dict[str, Any]) -> dict[str, str]:
        raise urllib.error.URLError("down")

    monkeypatch.setattr(relay_client, "_post_json_sync", post_json)

    assert cli.main(["enable", "spl"]) == 1

    assert capsys.readouterr().err.startswith("relay_unreachable: ")


def test_enable_spl_journal_not_initialized_exits_1(
    tmp_path, monkeypatch, capsys
) -> None:
    journal = tmp_path / "journal"
    (journal / "config").mkdir(parents=True)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))

    assert cli.main(["enable", "spl"]) == 1

    assert capsys.readouterr().err.startswith("journal_not_initialized: ")


def test_disable_spl_when_enabled_sets_direct_and_retains_secret(
    journal_copy, capsys
) -> None:
    _set_posture(journal_copy, "spl")
    save_totp_secret("SECRET")

    assert cli.main(["disable", "spl"]) == 0

    captured = capsys.readouterr()
    assert captured.out.strip() == cli.STDOUT_SPL_DISABLE_SUCCESS
    assert captured.err == ""
    config = json.loads((journal_copy / "config" / "journal.json").read_text())
    assert config["link"]["posture"] == "direct"
    assert totp_secret_path().exists()


def test_disable_spl_when_not_enabled_is_idempotent_success(
    journal_copy, capsys
) -> None:
    _set_posture(journal_copy, "direct")

    assert cli.main(["disable", "spl"]) == 0

    captured = capsys.readouterr()
    assert captured.out == ""
    assert captured.err.startswith("spl_already_disabled: ")


def test_enable_disable_help_lists_spl(capsys) -> None:
    with pytest.raises(SystemExit) as enable_exc:
        cli.main(["enable", "--help"])
    enable_out = capsys.readouterr().out

    with pytest.raises(SystemExit) as disable_exc:
        cli.main(["disable", "--help"])
    disable_out = capsys.readouterr().out

    assert enable_exc.value.code == 0
    assert disable_exc.value.code == 0
    assert "spl" in enable_out
    assert "spl" in disable_out


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
    assert timeout == portal_client.POLL_TIMEOUT_SECONDS
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
        cli.scout,
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


def test_headless_mints_device_code_and_polls(
    journal_copy, monkeypatch, capsys
) -> None:
    monkeypatch.setattr(cli, "_is_headless", lambda: True)
    monkeypatch.setattr(
        portal_client,
        "mint_device_code",
        lambda _base_url: portal_client.DeviceCodeOutcome(
            kind="success",
            nonce="A" * 52,
            code="SCOUT-2345-6789",
            expires_in=900,
        ),
    )
    monkeypatch.setattr(
        portal_client,
        "poll_handoff_once",
        lambda _base_url, _nonce, *, timeout: portal_client.PollOutcome(
            kind="success",
            payload=_payload(),
        ),
    )

    assert cli.main(["enable", "scout"]) == 0

    captured = capsys.readouterr()
    assert "https://services.solstone.app/enable/scout" in captured.out
    assert "?nonce=" not in captured.out
    assert "SCOUT-2345-6789" in captured.out
    assert cli.STDOUT_SUCCESS in captured.out
    assert captured.err == ""


def test_open_browser_false_falls_back_to_device_code(
    journal_copy, monkeypatch, capsys
) -> None:
    monkeypatch.setattr(cli, "_is_headless", lambda: False)
    monkeypatch.setattr(cli, "_open_browser", lambda _url: False)
    monkeypatch.setattr(
        portal_client,
        "mint_device_code",
        lambda _base_url: portal_client.DeviceCodeOutcome(
            kind="success",
            nonce="B" * 52,
            code="SCOUT-9876-ZYXW",
            expires_in=900,
        ),
    )
    monkeypatch.setattr(
        portal_client,
        "poll_handoff_once",
        lambda _base_url, _nonce, *, timeout: portal_client.PollOutcome(
            kind="success",
            payload=_payload("fallback"),
        ),
    )

    assert cli.main(["enable", "scout"]) == 0

    captured = capsys.readouterr()
    assert cli.STDOUT_OPENING in captured.out
    assert "https://services.solstone.app/enable/scout" in captured.out
    assert "?nonce=" not in captured.out
    assert "SCOUT-9876-ZYXW" in captured.out
    assert captured.err == ""


def test_mint_device_code_rate_limited(journal_copy, monkeypatch, capsys) -> None:
    monkeypatch.setattr(cli, "_is_headless", lambda: True)
    monkeypatch.setattr(
        portal_client,
        "mint_device_code",
        lambda _base_url: portal_client.DeviceCodeOutcome(
            kind="failed",
            reason="rate_limited",
        ),
    )

    assert cli.main(["enable", "scout"]) == 1

    captured = capsys.readouterr()
    assert captured.out == ""
    assert captured.err.startswith("rate_limited: ")


def test_disable_scout_when_enabled_clears_block_and_env_key(
    journal_copy, capsys
) -> None:
    cli.scout.provision_scout_handoff(_payload("disable"))

    assert cli.main(["disable", "scout"]) == 0

    captured = capsys.readouterr()
    assert captured.out.strip() == cli.STDOUT_DISABLE_SUCCESS
    assert captured.err == ""
    config = json.loads((journal_copy / "config" / "journal.json").read_text())
    assert "GOOGLE_API_KEY" not in config["env"]
    assert config["services"] == {}


def test_disable_scout_when_manually_keyed_preserves_env_key(
    journal_copy, capsys
) -> None:
    cli.scout.provision_scout_handoff(_payload("manual"))
    config = json.loads((journal_copy / "config" / "journal.json").read_text())
    config["env"]["GOOGLE_API_KEY"] = "manual-replacement"
    write_journal_config(config)

    assert cli.main(["disable", "scout"]) == 0

    captured = capsys.readouterr()
    assert "preserved" in captured.out
    assert captured.err == ""
    saved = json.loads((journal_copy / "config" / "journal.json").read_text())
    assert saved["env"]["GOOGLE_API_KEY"] == "manual-replacement"
    assert saved["services"] == {}


def test_disable_scout_when_not_enabled_emits_already_disabled(
    journal_copy, capsys
) -> None:
    assert cli.main(["disable", "scout"]) == 0

    captured = capsys.readouterr()
    assert captured.out == ""
    assert captured.err.startswith("already_disabled: ")


def test_disable_scout_when_journal_not_initialized_emits_token(
    tmp_path, monkeypatch, capsys
) -> None:
    journal = tmp_path / "journal"
    (journal / "config").mkdir(parents=True)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))

    assert cli.main(["disable", "scout"]) == 1

    assert capsys.readouterr().err.startswith("journal_not_initialized: ")


def test_disable_unknown_service_emits_unknown_service(capsys) -> None:
    with pytest.raises(SystemExit) as exc:
        cli.main(["disable", "bad"])

    assert exc.value.code == 2
    assert capsys.readouterr().err.startswith("unknown_service: ")


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
