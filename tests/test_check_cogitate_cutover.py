# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import sys
from pathlib import Path

from scripts import check_cogitate_cutover as checker
from scripts import report_cogitate_cutover_coverage as coverage_report


def test_rebased_root_reports_exactly_the_fixture_findings(
    tmp_path: Path, monkeypatch
) -> None:
    (tmp_path / "pyproject.toml").write_text(
        "[project]\ndependencies = []\n", encoding="utf-8"
    )
    target = tmp_path / "runtime.py"
    target.write_text(
        "import litellm\n\ndef assemble_prompt():\n    return ''\n",
        encoding="utf-8",
    )
    monkeypatch.setattr(sys, "argv", ["check_cogitate_cutover.py"])

    assert checker.main(root=tmp_path, paths=[target]) == 1
    assert checker.scan([target], root=tmp_path) == [
        {
            "file": "runtime.py",
            "kind": "agent_sdk_import",
            "detail": "litellm",
            "why": (
                "litellm exists in this repo for the tool-using runtime alone; "
                "the native runtime owns that now"
            ),
        },
        {
            "file": "runtime.py",
            "kind": "runtime_reimplementation",
            "detail": "assemble_prompt",
            "why": (
                "defines system-prompt assembly, which is a native crate's "
                "contract -- a second implementation, whatever it is called"
            ),
        },
    ]


def test_coverage_reports_each_detector_skip_reason(
    tmp_path: Path, monkeypatch, capsys
) -> None:
    paths = [
        tmp_path / "runtime.py",
        tmp_path / "tests" / "runtime.py",
        tmp_path / "scripts" / "runtime.py",
        tmp_path / "build" / "runtime.py",
        tmp_path / "test_runtime.py",
        tmp_path / "conftest.py",
        tmp_path / "broken.py",
    ]
    for path in paths:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("pass\n", encoding="utf-8")
    paths[-1].write_text("def broken(:\n", encoding="utf-8")
    monkeypatch.setattr(sys, "argv", ["report_cogitate_cutover_coverage.py"])

    assert coverage_report.main(root=tmp_path, paths=paths) == 0
    output = capsys.readouterr().out
    assert "selected: 7" in output
    assert "parsed: 1" in output
    assert "skipped: 6" in output
    assert "reference artifact: tests path component: 1" in output
    assert "reference artifact: scripts path component: 1" in output
    assert "reference artifact: build path component: 1" in output
    assert "reference artifact: test_* filename: 1" in output
    assert "reference artifact: conftest.py filename: 1" in output
    assert "skipped: parse error: 1" in output
    assert "detector findings: 0" in output
