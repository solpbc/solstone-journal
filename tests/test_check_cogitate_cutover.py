# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import sys
from pathlib import Path

from scripts import check_cogitate_cutover as checker


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
