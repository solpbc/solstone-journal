# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from pathlib import Path

from scripts.check_brain_health_cutover import _brain_state_write_calls


def test_inline_brain_path_helper_write_is_rejected() -> None:
    root = Path("/synthetic-root")
    findings = _brain_state_write_calls(
        root,
        root / "writer.py",
        "from solstone.think.providers.brain_state import brain_state_path\n"
        'open(brain_state_path(), "w")\n',
    )

    assert [(finding.path, finding.rule, finding.detail) for finding in findings] == [
        ("writer.py:2", "python-brain-state-writer", "open")
    ]
