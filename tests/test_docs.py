# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import subprocess
from pathlib import Path


def test_install_md_has_no_ollama_references() -> None:
    # Negative invariant: the ollama -> "local provider" sweep stays done.
    # Asserting on retired terminology is a rule that must always hold, not a
    # prose snapshot. We deliberately do NOT pin the verbatim wording of the
    # install copy (that's a checklist concern, not a test, and breaks on every
    # legitimate copy edit) -- we assert only the load-bearing concepts survive.
    text = Path("INSTALL.md").read_text(encoding="utf-8").lower()

    assert "ollama" not in text
    assert "cogitate" in text
    assert "no extra install step" in text
    assert "local provider" in text


def test_ollama_grep_returns_zero_lines() -> None:
    result = subprocess.run(
        [
            "git",
            "grep",
            "-i",
            "ollama",
            "--",
            ":!tests/",
            ":!docs/design/",
            ":!solstone/apps/settings/maint/_migrate_ollama_to_local.py",
            ":!solstone/apps/settings/call.py",
            ":!CHANGELOG.md",
        ],
        check=False,
        capture_output=True,
        text=True,
    )

    assert result.returncode == 1
    assert result.stdout == ""
