# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import re
from pathlib import Path

from solstone.think.sol_cli import COMMANDS


def test_makefile_uses_journal_for_service_tagged_commands() -> None:
    text = Path("Makefile").read_text(encoding="utf-8")
    service_commands = sorted(
        name for name, command in COMMANDS.items() if command.surface == "service"
    )
    pattern = re.compile(
        r"\$\(VENV_BIN\)/sol\s+(" + "|".join(map(re.escape, service_commands)) + r")\b"
    )

    assert pattern.findall(text) == []
    assert "$(VENV_BIN)/journal supervisor" in text
    assert "$(VENV_BIN)/journal install-models" in text
    assert "$(VENV_BIN)/journal service logs -f" in text
    assert "journal service uninstall" in text
    assert "sol skills uninstall" in text
