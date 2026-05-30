# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import pytest

from solstone.think.link import cli


def test_link_join_dispatches_to_join_cli(monkeypatch: pytest.MonkeyPatch) -> None:
    calls = []

    def fake_join(args) -> int:
        calls.append((args.home, args.code, args.as_role, args.label))
        return 0

    monkeypatch.setattr("solstone.think.link.join_cli.main", fake_join)

    assert (
        cli.main(
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


def test_link_list_dispatches_to_list_cli(monkeypatch: pytest.MonkeyPatch) -> None:
    calls = []

    def fake_list(args) -> int:
        calls.append((args.command, args.observers, args.json))
        return 0

    monkeypatch.setattr("solstone.think.link.list_cli.main", fake_list)

    assert cli.main(["list"]) == 0

    assert calls == [("list", False, False)]


def test_link_list_dispatches_flags_to_list_cli(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    calls = []

    def fake_list(args) -> int:
        calls.append((args.command, args.observers, args.json))
        return 0

    monkeypatch.setattr("solstone.think.link.list_cli.main", fake_list)

    assert cli.main(["list", "--observers", "--json"]) == 0

    assert calls == [("list", True, True)]


def test_link_no_subcommand_help_lists_commands(
    capsys: pytest.CaptureFixture[str],
) -> None:
    assert cli.main([]) == 0

    out = capsys.readouterr().out
    assert "{join,list,serve}" in out
    assert "join" in out
    assert "list" in out
    assert "serve" in out


def test_link_serve_dispatches_to_serve_cli(monkeypatch: pytest.MonkeyPatch) -> None:
    calls = []

    def fake_serve(args) -> int:
        calls.append((args.command, args.label, args.port, args.relay_url))
        return 0

    monkeypatch.setattr("solstone.think.link.serve_cli.main", fake_serve)

    assert cli.main(["serve", "--label", "x", "--port", "5099"]) == 0

    assert calls == [("serve", "x", 5099, None)]
