# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

from pathlib import Path

from solstone.think import core_handshake, spl_native


def test_native_service_execs_launcher_sibling_with_preserved_flags() -> None:
    calls: list[tuple[str, list[str]]] = []

    def execv(path: str, argv: list[str]) -> None:
        calls.append((path, argv))

    result = spl_native.exec_native_service(
        ["-v", "-d"],
        handshake_checker=lambda: core_handshake.CoreHandshakeResult("ok"),
        helper_locator=lambda: Path("/tmp/bin/solstone-core"),
        execv=execv,
    )

    assert result == 0
    assert calls == [
        (
            "/tmp/bin/solstone-core",
            ["/tmp/bin/solstone-core", "spl", "service", "-v", "-d"],
        )
    ]


def test_native_service_refuses_a_non_ok_handshake(capsys) -> None:
    result = spl_native.exec_native_service(
        ["-v"],
        handshake_checker=lambda: core_handshake.CoreHandshakeResult(
            "fail", "version skew"
        ),
        helper_locator=lambda: (_ for _ in ()).throw(AssertionError("must not locate")),
    )

    assert result == core_handshake.EX_CONFIG
    assert capsys.readouterr().err == "version skew\n"


def test_native_service_reports_exec_failure() -> None:
    def fail_execv(_path: str, _argv: list[str]) -> None:
        raise OSError("not executable")

    result = spl_native.exec_native_service(
        [],
        handshake_checker=lambda: core_handshake.CoreHandshakeResult("ok"),
        helper_locator=lambda: Path("/tmp/bin/solstone-core"),
        execv=fail_execv,
    )

    assert result == 75
