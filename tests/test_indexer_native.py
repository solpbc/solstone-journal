# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import argparse
import re
import subprocess
from pathlib import Path
from typing import Any

import pytest

from solstone.think import core_handshake
from solstone.think.indexer import native


def _args(**overrides: Any) -> argparse.Namespace:
    values: dict[str, Any] = {
        "rescan": False,
        "rescan_full": False,
        "rescan_file": None,
        "rebuild_edges": False,
        "reset": False,
        "day": None,
        "day_from": None,
        "day_to": None,
        "facet": None,
        "agent": None,
        "stream": None,
        "query": None,
        "limit": 10,
        "offset": 0,
        "top": 5,
        "verbose": False,
        "debug": False,
    }
    values.update(overrides)
    return argparse.Namespace(**values)


def _ok() -> core_handshake.CoreHandshakeResult:
    return core_handshake.CoreHandshakeResult("ok")


def _raise_unexpected(name: str):
    def fail(*_args, **_kwargs):
        raise AssertionError(f"{name} should not be called")

    return fail


def _native_argv(*flags: str, journal: str = "/tmp/journal") -> list[str]:
    return ["/tmp/bin/solstone-core", "indexer", "--journal", journal, *flags]


def _route(
    args: argparse.Namespace,
    *,
    native_returncode: int = 0,
    native_runner=None,
    handshake_checker=None,
    helper_path: Path | None = None,
    journal: str = "/tmp/journal",
    platform_tuple: tuple[str, str] = ("linux", "x86_64"),
    platform_tags: set[str] | None = None,
) -> tuple[int, list[list[str]]]:
    native_argvs: list[list[str]] = []

    def default_native_runner(argv: list[str], *, check: bool = False):
        assert check is False
        native_argvs.append(argv)
        return subprocess.CompletedProcess(argv, native_returncode)

    result = native.run_native_indexer(
        args,
        journal,
        handshake_checker=handshake_checker or _ok,
        helper_locator=lambda: helper_path or Path("/tmp/bin/solstone-core"),
        native_runner=native_runner or default_native_runner,
        platform_reader=lambda: platform_tuple,
        platform_tag_reader=lambda: platform_tags or {"manylinux2014_x86_64"},
    )
    return result, native_argvs


@pytest.mark.parametrize(
    ("platform_tuple", "platform_tags", "expected"),
    [
        (("linux", "x86_64"), {"manylinux2014_x86_64"}, True),
        (("linux", "x86_64"), {"musllinux_1_2_x86_64"}, False),
        (("linux", "aarch64"), {"musllinux_1_2_aarch64"}, False),
        (("darwin", "x86_64"), {"macosx_14_0_x86_64"}, False),
        (("linux", "ppc64le"), {"manylinux_2_17_ppc64le"}, False),
    ],
)
def test_runtime_coverage_uses_platform_tuple_and_packaging_tags(
    platform_tuple: tuple[str, str],
    platform_tags: set[str],
    expected: bool,
) -> None:
    assert (
        native.runtime_has_solstone_core_wheel_coverage(
            platform_reader=lambda: platform_tuple,
            platform_tag_reader=lambda: platform_tags,
        )
        is expected
    )


def test_runtime_coverage_splits_compressed_linux_tag_set() -> None:
    assert native.runtime_has_solstone_core_wheel_coverage(
        platform_reader=lambda: ("linux", "x86_64"),
        platform_tag_reader=lambda: {"manylinux_2_17_x86_64"},
    )


def test_uncovered_runtime_returns_78_without_handshake(
    capsys: pytest.CaptureFixture[str],
) -> None:
    result, native_argvs = _route(
        _args(rescan=True),
        platform_tags={"musllinux_1_2_x86_64"},
        handshake_checker=_raise_unexpected("handshake_checker"),
    )

    assert result == core_handshake.EX_CONFIG
    assert native_argvs == []
    assert "This host has no compatible solstone-core wheel" in capsys.readouterr().err


def test_handshake_skip_reports_source_checkout_guidance(
    capsys: pytest.CaptureFixture[str],
) -> None:
    result, native_argvs = _route(
        _args(rescan=True),
        handshake_checker=lambda: core_handshake.CoreHandshakeResult("skip", "reason"),
    )

    assert result == core_handshake.EX_CONFIG
    assert native_argvs == []
    assert capsys.readouterr().err.strip() == (
        "journal indexer requires solstone-core for command writes, but "
        "solstone-core distribution metadata is missing in this source checkout. "
        "Run make install to restore the journal-host environment, then rerun "
        "journal indexer."
    )


def test_handshake_fail_passes_through_context(
    capsys: pytest.CaptureFixture[str],
) -> None:
    result, native_argvs = _route(
        _args(rescan=True),
        handshake_checker=lambda: core_handshake.CoreHandshakeResult("fail", "reason"),
    )

    assert result == core_handshake.EX_CONFIG
    assert native_argvs == []
    assert capsys.readouterr().err.strip() == (
        "journal indexer cannot run native command writes: reason"
    )


@pytest.mark.parametrize(
    ("overrides", "expected_flags"),
    [
        ({"reset": True}, ["--reset"]),
        ({"rebuild_edges": True}, ["--rebuild-edges"]),
        ({"rescan": True}, ["--rescan"]),
        ({"rescan_full": True}, ["--rescan-full"]),
        (
            {"rescan_file": "chronicle/today.md"},
            ["--rescan-file", str(Path("/tmp/journal/chronicle/today.md").resolve())],
        ),
    ],
)
def test_write_operations_invoke_native_with_expected_flags(
    overrides: dict[str, Any],
    expected_flags: list[str],
) -> None:
    result, native_argvs = _route(_args(**overrides))

    assert result == 0
    assert native_argvs == [_native_argv(*expected_flags)]


@pytest.mark.parametrize(
    ("overrides", "expected_flags"),
    [
        (
            {"reset": True, "rebuild_edges": True, "rescan_full": True},
            ["--reset", "--rebuild-edges", "--rescan-full"],
        ),
        (
            {"rebuild_edges": True, "rescan_full": True},
            ["--rebuild-edges", "--rescan-full"],
        ),
        ({"reset": True, "rescan_full": True}, ["--reset", "--rescan-full"]),
        ({"rescan": True, "rescan_full": True}, ["--rescan-full"]),
    ],
)
def test_composed_write_order(
    overrides: dict[str, Any],
    expected_flags: list[str],
) -> None:
    result, native_argvs = _route(_args(**overrides))

    assert result == 0
    assert native_argvs == [_native_argv(*expected_flags)]


def test_native_tail_drops_verbose_debug_and_query_filters() -> None:
    result, native_argvs = _route(
        _args(
            rescan=True,
            verbose=True,
            debug=True,
            day="20260101",
            facet="work",
            agent="flow",
            stream="archon",
            limit=99,
            offset=4,
            top=7,
            query="needle",
        )
    )

    assert result == 0
    assert native_argvs == [_native_argv("--rescan")]


def test_rescan_file_normalizes_chronicle_prefixed_relative_to_absolute(
    tmp_path: Path,
) -> None:
    journal = tmp_path / "journal"
    rel = "chronicle/20240101/talents/flow.md"
    (journal / "chronicle" / "20240101" / "talents").mkdir(parents=True)

    result, native_argvs = _route(
        _args(rescan_file=rel),
        journal=str(journal),
    )

    assert result == 0
    assert native_argvs == [
        [
            "/tmp/bin/solstone-core",
            "indexer",
            "--journal",
            str(journal),
            "--rescan-file",
            str((journal / rel).resolve()),
        ]
    ]


def test_empty_tail_raises_runtime_error(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(native, "_build_operation_flags", lambda _args, _journal: [])

    with pytest.raises(RuntimeError, match=re.escape(native.EMPTY_TAIL_MESSAGE)):
        _route(
            _args(rescan=True),
            native_runner=_raise_unexpected("native_runner"),
        )


@pytest.mark.parametrize(
    ("returncode", "expected_result", "expected_error"),
    [
        (
            64,
            64,
            "journal indexer usage error: native solstone-core indexer rejected "
            "the command arguments with exit 64. This is a command "
            "argument-construction bug; update solstone or report the full "
            "journal indexer command.",
        ),
        (
            69,
            69,
            "journal indexer native command exited 69 (unsupported input). The "
            "Python command-write path has been retired; remove or change the "
            "unsupported input and rerun journal indexer.",
        ),
        (
            75,
            75,
            "journal indexer native command exited 75 (temporary failure). Fix "
            "the reported cause and rerun journal indexer.",
        ),
        (
            -9,
            75,
            "journal indexer native command died from signal 9 (returncode -9); "
            "treating as temporary failure. Fix the cause and rerun journal "
            "indexer.",
        ),
        (
            12,
            12,
            "journal indexer native command exited 12. Fix the reported cause "
            "and rerun journal indexer.",
        ),
    ],
)
def test_native_exit_mappings(
    returncode: int,
    expected_result: int,
    expected_error: str,
    capsys: pytest.CaptureFixture[str],
) -> None:
    result, native_argvs = _route(
        _args(rescan=True),
        native_returncode=returncode,
    )

    assert result == expected_result
    assert native_argvs == [_native_argv("--rescan")]
    assert capsys.readouterr().err.strip() == expected_error


def test_native_launch_oserror_maps_to_tempfail_without_composed_warning(
    capsys: pytest.CaptureFixture[str],
) -> None:
    def native_runner(_argv: list[str], *, check: bool = False):
        assert check is False
        raise OSError("missing helper")

    result, native_argvs = _route(
        _args(reset=True, rebuild_edges=True, rescan=True),
        native_runner=native_runner,
    )

    assert result == 75
    assert native_argvs == []
    err = capsys.readouterr().err.strip()
    assert err == (
        "journal indexer failed to launch solstone-core indexer: missing helper. "
        "Run make install in a source checkout or reinstall solstone-journal, "
        "then rerun journal indexer."
    )
    assert native.COMPOSED_COMMAND_WARNING not in err


@pytest.mark.parametrize("returncode", [69, 75, -9, 12])
def test_composed_nonzero_after_process_start_warns(
    returncode: int,
    capsys: pytest.CaptureFixture[str],
) -> None:
    result, native_argvs = _route(
        _args(reset=True, rebuild_edges=True, rescan=True),
        native_returncode=returncode,
    )

    assert result == (75 if returncode < 0 else returncode)
    assert native_argvs == [_native_argv("--reset", "--rebuild-edges", "--rescan")]
    assert native.COMPOSED_COMMAND_WARNING in capsys.readouterr().err


def test_composed_native_usage_64_does_not_warn(
    capsys: pytest.CaptureFixture[str],
) -> None:
    result, native_argvs = _route(
        _args(reset=True, rebuild_edges=True, rescan=True),
        native_returncode=64,
    )

    assert result == 64
    assert native_argvs == [_native_argv("--reset", "--rebuild-edges", "--rescan")]
    assert native.COMPOSED_COMMAND_WARNING not in capsys.readouterr().err


def _read_kwargs(native_runner) -> dict[str, Any]:
    return {
        "handshake_checker": _ok,
        "helper_locator": lambda: Path("/tmp/bin/solstone-core"),
        "native_runner": native_runner,
        "platform_reader": lambda: ("linux", "x86_64"),
        "platform_tag_reader": lambda: {"manylinux2014_x86_64"},
    }


@pytest.mark.parametrize(
    "invoke",
    [
        lambda kwargs: native.run_native_indexer_search(
            "needle", "/tmp/journal", limit=10, offset=0, **kwargs
        ),
        lambda kwargs: native.run_native_indexer_search(
            "", "/tmp/journal", limit=0, offset=0, **kwargs
        ),
        lambda kwargs: native.run_native_indexer_agents("/tmp/journal", **kwargs),
        lambda kwargs: native.run_native_indexer_coverage("/tmp/journal", **kwargs),
    ],
)
def test_read_verbs_raise_named_error_when_native_launch_fails(invoke) -> None:
    def broken_runner(*_args, **_kwargs):
        raise OSError("missing helper")

    with pytest.raises(native.NativeIndexerReadError, match="missing helper"):
        invoke(_read_kwargs(broken_runner))


@pytest.mark.parametrize(
    ("returncode", "reason"),
    [
        (69, "index_absent"),
        (69, "index_unreadable"),
        (75, "index_locked"),
        (69, "empty_index"),
    ],
)
def test_read_access_errors_preserve_native_reason(returncode: int, reason: str) -> None:
    def runner(argv, *, capture_output: bool, text: bool, check: bool):
        assert capture_output is True
        assert text is True
        assert check is False
        return subprocess.CompletedProcess(
            argv,
            returncode,
            stdout=f'{{"error": {{"reason": "{reason}", "message": "native message"}}}}',
        )

    with pytest.raises(native.NativeIndexerReadError, match="native message") as exc_info:
        native.run_native_indexer_search(
            "needle", "/tmp/journal", limit=10, offset=0, **_read_kwargs(runner)
        )
    assert exc_info.value.reason == reason
