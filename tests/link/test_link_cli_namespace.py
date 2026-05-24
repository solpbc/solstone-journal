# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import pytest

from solstone.think.link import service


async def _fake_run_service() -> None:
    return None


def test_bare_link_routes_to_serve(monkeypatch: pytest.MonkeyPatch) -> None:
    calls = []
    monkeypatch.setattr(service, "run_service", _fake_run_service)
    monkeypatch.setattr(
        "solstone.think.link.service.require_solstone", lambda: calls.append("serve")
    )

    assert service.main([]) == 0

    assert calls == ["serve"]


def test_link_dash_v_routes_to_serve(monkeypatch: pytest.MonkeyPatch) -> None:
    calls = []
    monkeypatch.setattr(service, "run_service", _fake_run_service)
    monkeypatch.setattr(
        "solstone.think.link.service.require_solstone", lambda: calls.append("serve")
    )

    assert service.main(["-v"]) == 0

    assert calls == ["serve"]


def test_link_serve_routes_to_serve(monkeypatch: pytest.MonkeyPatch) -> None:
    calls = []
    monkeypatch.setattr(service, "run_service", _fake_run_service)
    monkeypatch.setattr(
        "solstone.think.link.service.require_solstone", lambda: calls.append("serve")
    )

    assert service.main(["serve"]) == 0

    assert calls == ["serve"]


def test_link_serve_dash_v_routes_to_serve(monkeypatch: pytest.MonkeyPatch) -> None:
    calls = []
    seen_verbose = []
    monkeypatch.setattr(service, "run_service", _fake_run_service)
    monkeypatch.setattr(
        "solstone.think.link.service.require_solstone", lambda: calls.append("serve")
    )

    original = service._run_service_command

    def capture(args):
        seen_verbose.append(args.verbose)
        return original(args)

    monkeypatch.setattr(service, "_run_service_command", capture)

    assert service.main(["serve", "-v"]) == 0

    assert calls == ["serve"]
    assert seen_verbose == [True]


def test_link_join_dispatches_to_join_cli(monkeypatch: pytest.MonkeyPatch) -> None:
    calls = []

    def fake_join(args) -> int:
        calls.append((args.home, args.code, args.as_role, args.label))
        return 0

    monkeypatch.setattr("solstone.think.link.join_cli.main", fake_join)

    assert (
        service.main(
            [
                "join",
                "--home",
                "http://receiver",
                "--code",
                "ABCD-EFGH",
                "--as",
                "observer",
                "--label",
                "laptop",
            ]
        )
        == 0
    )

    assert calls == [("http://receiver", "ABCD-EFGH", "observer", "laptop")]


def test_link_help_lists_serve_and_join(capsys: pytest.CaptureFixture[str]) -> None:
    with pytest.raises(SystemExit) as exc:
        service.main(["--help"])

    assert exc.value.code == 0
    out = capsys.readouterr().out
    assert "serve" in out
    assert "join" in out
