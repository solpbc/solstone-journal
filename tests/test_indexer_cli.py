# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for the journal indexer CLI."""

from __future__ import annotations

import json
import runpy
import subprocess
import sys
from collections import Counter
from pathlib import Path
from typing import Any

import pytest

import solstone.think.journal_config as journal_config
import solstone.think.utils as think_utils
from solstone.think import core_handshake
from solstone.think.indexer import cli as indexer_cli
from solstone.think.indexer import native as indexer_native
from solstone.think.indexer.journal import ScanReport


def test_module_entrypoint_propagates_main_return(monkeypatch):
    monkeypatch.setattr(indexer_cli, "main", lambda: 75)

    with pytest.raises(SystemExit) as exc_info:
        runpy.run_module("solstone.think.indexer.__main__", run_name="__main__")

    assert exc_info.value.code == 75


def _setup_cli_for(args: list[str]):
    def setup_cli(parser):
        parsed = parser.parse_args(args)
        if not hasattr(parsed, "verbose"):
            parsed.verbose = False
        if not hasattr(parsed, "debug"):
            parsed.debug = False
        return parsed

    return setup_cli


def _run_indexer_cli(
    monkeypatch: pytest.MonkeyPatch,
    journal: Path,
    args: list[str],
) -> int | None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    think_utils._journal_path_cache = None
    monkeypatch.setattr(indexer_cli, "get_journal", lambda: str(journal))
    monkeypatch.setattr(indexer_cli, "require_solstone", lambda: None)
    monkeypatch.setattr(indexer_cli, "setup_cli", _setup_cli_for(args))
    return indexer_cli.main()


def _install_python_write_recorders(monkeypatch: pytest.MonkeyPatch) -> list[str]:
    calls: list[str] = []

    def scan_journal(
        _journal: str,
        *,
        verbose: bool = False,
        full: bool = False,
    ) -> ScanReport:
        calls.append(f"scan_journal:{verbose}:{full}")
        return ScanReport(changed=True, edge_rows_inserted=1)

    def index_file(_journal: str, _path: str, *, verbose: bool = False) -> bool:
        calls.append(f"index_file:{verbose}")
        return True

    def rebuild_edges(_journal: str) -> dict[str, int]:
        calls.append("rebuild_edges")
        return {"files": 1, "rows": 1, "drops": 0, "failed": 0}

    monkeypatch.setattr(indexer_cli, "scan_journal", scan_journal, raising=False)
    monkeypatch.setattr(indexer_cli, "index_file", index_file, raising=False)
    monkeypatch.setattr(indexer_cli, "rebuild_edges", rebuild_edges, raising=False)
    return calls


def _native_argv_from_args(args: Any, journal: str) -> list[str]:
    flags: list[str] = []
    if args.reset:
        flags.append("--reset")
    if args.rebuild_edges:
        flags.append("--rebuild-edges")
    if args.rescan_file:
        flags.extend(["--rescan-file", args.rescan_file])
    elif args.rescan_full:
        flags.append("--rescan-full")
    elif args.rescan:
        flags.append("--rescan")
    return ["/tmp/bin/solstone-core", "indexer", "--journal", journal, *flags]


def _install_native_stub(
    monkeypatch: pytest.MonkeyPatch,
    native_argvs: list[list[str]],
    *,
    returncode: int = 0,
) -> None:
    def run_native_indexer(args: Any, journal: str) -> int:
        native_argvs.append(_native_argv_from_args(args, journal))
        return returncode

    monkeypatch.setattr(indexer_cli, "run_native_indexer", run_native_indexer)


def _install_query_stubs(monkeypatch: pytest.MonkeyPatch) -> list[str]:
    calls: list[str] = []

    def search_counts(query: str, **_kwargs: Any) -> dict[str, Any]:
        calls.append(f"counts:{query}")
        return {
            "total": 1,
            "facets": Counter({"work": 1}),
            "agents": Counter({"flow": 1}),
            "days": Counter({"20260101": 1}),
        }

    def search_journal(
        query: str,
        limit: int,
        offset: int,
        **_kwargs: Any,
    ) -> tuple[int, list[dict[str, Any]]]:
        calls.append(f"search:{query}:{limit}:{offset}")
        return (
            1,
            [
                {
                    "metadata": {
                        "day": "20260101",
                        "agent": "flow",
                        "facet": "work",
                    },
                    "text": f"found {query}",
                }
            ],
        )

    monkeypatch.setattr(indexer_cli, "search_counts", search_counts)
    monkeypatch.setattr(indexer_cli, "search_journal", search_journal)
    return calls


def _install_no_config_handshake_or_subprocess(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    def fail(name: str):
        def raise_unexpected(*_args: Any, **_kwargs: Any):
            raise AssertionError(f"{name} should not be called")

        return raise_unexpected

    monkeypatch.setattr(
        journal_config,
        "read_journal_config",
        fail("read_journal_config"),
    )
    monkeypatch.setattr(
        core_handshake,
        "check_solstone_core_handshake",
        fail("check_solstone_core_handshake"),
    )
    monkeypatch.setattr(subprocess, "run", fail("subprocess.run"))
    monkeypatch.setattr(indexer_cli, "run_native_indexer", fail("run_native_indexer"))


def test_help_no_work_uses_python_help_without_config_handshake_or_subprocess(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    journal = tmp_path / "journal"
    journal.mkdir()
    _install_no_config_handshake_or_subprocess(monkeypatch)

    result = _run_indexer_cli(monkeypatch, journal, [])

    captured = capsys.readouterr()
    assert result is None
    assert "usage:" in captured.out
    assert "Index journal content" in captured.out
    assert captured.err == ""


def test_query_only_uses_python_search_without_config_handshake_or_subprocess(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    journal = tmp_path / "journal"
    journal.mkdir()
    _install_no_config_handshake_or_subprocess(monkeypatch)
    query_calls = _install_query_stubs(monkeypatch)

    result = _run_indexer_cli(monkeypatch, journal, ["-q", "needle"])

    captured = capsys.readouterr()
    assert result is None
    assert query_calls == ["counts:needle", "search:needle:10:0"]
    assert "Total: 1 chunks" in captured.out
    assert "found needle" in captured.out
    assert captured.err == ""


def test_interactive_query_uses_python_search_without_config_handshake_or_subprocess(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    journal = tmp_path / "journal"
    journal.mkdir()
    _install_no_config_handshake_or_subprocess(monkeypatch)
    query_calls = _install_query_stubs(monkeypatch)
    inputs = iter(["needle", ""])
    monkeypatch.setattr("builtins.input", lambda _prompt: next(inputs))

    result = _run_indexer_cli(monkeypatch, journal, ["-q"])

    captured = capsys.readouterr()
    assert result is None
    assert query_calls == ["counts:needle", "search:needle:10:0"]
    assert "Total: 1 chunks" in captured.out
    assert "found needle" in captured.out
    assert captured.err == ""


def test_write_only_routes_to_native_without_python_writes(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    journal = tmp_path / "journal"
    journal.mkdir()
    native_argvs: list[list[str]] = []
    python_write_calls = _install_python_write_recorders(monkeypatch)
    _install_native_stub(monkeypatch, native_argvs)

    result = _run_indexer_cli(monkeypatch, journal, ["--rescan"])

    assert result == 0
    assert native_argvs == [
        ["/tmp/bin/solstone-core", "indexer", "--journal", str(journal), "--rescan"]
    ]
    assert python_write_calls == []


def test_mixed_write_query_runs_query_after_native_success(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    journal = tmp_path / "journal"
    journal.mkdir()
    native_argvs: list[list[str]] = []
    python_write_calls = _install_python_write_recorders(monkeypatch)
    _install_native_stub(monkeypatch, native_argvs)
    query_calls = _install_query_stubs(monkeypatch)

    result = _run_indexer_cli(
        monkeypatch,
        journal,
        ["--rescan", "--query", "needle"],
    )

    captured = capsys.readouterr()
    assert result is None
    assert native_argvs == [
        ["/tmp/bin/solstone-core", "indexer", "--journal", str(journal), "--rescan"]
    ]
    assert python_write_calls == []
    assert query_calls == ["counts:needle", "search:needle:10:0"]
    assert "Total: 1 chunks" in captured.out
    assert "found needle" in captured.out


def test_mixed_empty_query_enters_interactive_after_native_success(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    journal = tmp_path / "journal"
    journal.mkdir()
    native_argvs: list[list[str]] = []
    python_write_calls = _install_python_write_recorders(monkeypatch)
    _install_native_stub(monkeypatch, native_argvs)
    query_calls = _install_query_stubs(monkeypatch)
    inputs = iter(["needle", ""])
    monkeypatch.setattr("builtins.input", lambda _prompt: next(inputs))

    result = _run_indexer_cli(monkeypatch, journal, ["--rescan", "-q"])

    captured = capsys.readouterr()
    assert result is None
    assert native_argvs == [
        ["/tmp/bin/solstone-core", "indexer", "--journal", str(journal), "--rescan"]
    ]
    assert python_write_calls == []
    assert query_calls == ["counts:needle", "search:needle:10:0"]
    assert "found needle" in captured.out


def test_mixed_write_query_native_failure_skips_query(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    journal = tmp_path / "journal"
    journal.mkdir()
    native_argvs: list[list[str]] = []
    python_write_calls = _install_python_write_recorders(monkeypatch)
    _install_native_stub(monkeypatch, native_argvs, returncode=75)
    query_calls = _install_query_stubs(monkeypatch)

    result = _run_indexer_cli(
        monkeypatch,
        journal,
        ["--rescan", "--query", "needle"],
    )

    captured = capsys.readouterr()
    assert result == 75
    assert native_argvs == [
        ["/tmp/bin/solstone-core", "indexer", "--journal", str(journal), "--rescan"]
    ]
    assert python_write_calls == []
    assert query_calls == []
    assert captured.out == ""


def test_rescan_file_with_scan_exits_64_before_side_effects(
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    python_write_calls = _install_python_write_recorders(monkeypatch)
    query_calls = _install_query_stubs(monkeypatch)
    monkeypatch.setattr(
        indexer_cli,
        "setup_cli",
        _setup_cli_for(
            ["--rescan", "--rescan-file", "chronicle/today.md", "--query", "needle"]
        ),
    )
    monkeypatch.setattr(
        indexer_cli,
        "require_solstone",
        lambda: pytest.fail("require_solstone should not run"),
    )
    monkeypatch.setattr(
        indexer_cli,
        "get_journal",
        lambda: pytest.fail("get_journal should not run"),
    )
    monkeypatch.setattr(
        indexer_cli,
        "run_native_indexer",
        lambda *_args, **_kwargs: pytest.fail("native should not run"),
    )

    result = indexer_cli.main()

    captured = capsys.readouterr()
    assert result == 64
    assert python_write_calls == []
    assert query_calls == []
    assert captured.err.strip() == (
        "journal indexer usage error: --rescan-file cannot be combined with "
        "--rescan or --rescan-full."
    )


@pytest.mark.parametrize(
    "args",
    [
        ["--rescan"],
        ["--rescan", "--query", "needle"],
    ],
)
def test_stale_core_indexer_keys_are_inert(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    args: list[str],
) -> None:
    journal = tmp_path / "journal"
    journal.mkdir()
    native_argvs: list[list[str]] = []
    _install_python_write_recorders(monkeypatch)
    _install_native_stub(monkeypatch, native_argvs)
    _install_query_stubs(monkeypatch)

    def fail_read_journal_config(*_args: Any, **_kwargs: Any) -> dict[str, Any]:
        raise AssertionError("read_journal_config should not be touched")

    monkeypatch.setattr(
        journal_config,
        "read_journal_config",
        fail_read_journal_config,
    )

    first_result = _run_indexer_cli(monkeypatch, journal, args)
    first_output = capsys.readouterr()

    config_path = journal / "config" / "journal.json"
    config_path.parent.mkdir(parents=True)
    config_path.write_text(
        json.dumps(
            {
                "core": {
                    "indexer": "python",
                    "indexer_on_decline": "fallback",
                }
            }
        )
        + "\n",
        encoding="utf-8",
    )

    second_result = _run_indexer_cli(monkeypatch, journal, args)
    second_output = capsys.readouterr()

    assert first_result == second_result
    assert native_argvs[0] == native_argvs[1]
    assert first_output.out == second_output.out
    assert first_output.err == second_output.err


@pytest.mark.parametrize(
    "args",
    [
        ["--rescan"],
        ["--rescan-full"],
        ["--rebuild-edges"],
        ["--rescan-file", "chronicle/today.md"],
    ],
)
def test_rescan_leaves_existing_root_task_log_unchanged(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    args: list[str],
) -> None:
    journal = tmp_path / "journal"
    journal.mkdir()
    root_log = journal / "task_log.txt"
    original = b"123\tkeep\n"
    root_log.write_bytes(original)
    native_argvs: list[list[str]] = []
    python_write_calls = _install_python_write_recorders(monkeypatch)
    _install_native_stub(monkeypatch, native_argvs)

    result = _run_indexer_cli(monkeypatch, journal, args)

    assert result == 0
    assert native_argvs == [
        ["/tmp/bin/solstone-core", "indexer", "--journal", str(journal), *args]
    ]
    assert python_write_calls == []
    assert root_log.read_bytes() == original


def _dispatcher_native_impl(
    *, native_returncode: int | None = None, launch_error=False
):
    def run_native_indexer(args: Any, journal: str) -> int:
        def native_runner(argv: list[str], *, check: bool = False):
            assert check is False
            if launch_error:
                raise OSError("missing helper")
            return subprocess.CompletedProcess(argv, native_returncode)

        return indexer_native.run_native_indexer(
            args,
            journal,
            handshake_checker=lambda: core_handshake.CoreHandshakeResult("ok"),
            helper_locator=lambda: Path("/tmp/bin/solstone-core"),
            native_runner=native_runner,
            platform_reader=lambda: ("linux", "x86_64"),
            platform_tag_reader=lambda: {"manylinux2014_x86_64"},
        )

    return run_native_indexer


def _run_dispatcher(
    monkeypatch: pytest.MonkeyPatch,
    journal: Path,
    args: list[str],
    run_native_indexer,
) -> int:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    think_utils._journal_path_cache = None
    monkeypatch.setattr(indexer_cli, "get_journal", lambda: str(journal))
    monkeypatch.setattr(indexer_cli, "require_solstone", lambda: None)
    monkeypatch.setattr(indexer_cli, "setup_cli", _setup_cli_for(args))
    monkeypatch.setattr(indexer_cli, "run_native_indexer", run_native_indexer)
    monkeypatch.setattr(sys, "argv", ["journal", "indexer", *args])
    result = indexer_cli.main()
    return 0 if result is None else int(result)


@pytest.mark.parametrize(
    ("case", "run_native_indexer", "expected"),
    [
        ("success", _dispatcher_native_impl(native_returncode=0), 0),
        ("native_69", _dispatcher_native_impl(native_returncode=69), 69),
        ("native_tempfail", _dispatcher_native_impl(native_returncode=75), 75),
        ("native_signal", _dispatcher_native_impl(native_returncode=-9), 75),
        ("native_launch", _dispatcher_native_impl(launch_error=True), 75),
        ("native_other", _dispatcher_native_impl(native_returncode=12), 12),
    ],
)
def test_dispatcher_exit_contract_for_native_results(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    case: str,
    run_native_indexer,
    expected: int,
) -> None:
    journal = tmp_path / case
    journal.mkdir()

    result = _run_dispatcher(
        monkeypatch,
        journal,
        ["--rescan"],
        run_native_indexer,
    )

    assert result == expected


def test_dispatcher_exit_contract_for_usage_64(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    journal = tmp_path / "journal"
    journal.mkdir()

    result = _run_dispatcher(
        monkeypatch,
        journal,
        ["--rescan", "--rescan-file", "chronicle/today.md"],
        lambda *_args, **_kwargs: pytest.fail("native should not run"),
    )

    assert result == 64
