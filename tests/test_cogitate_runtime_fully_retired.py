# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""The deleted Python cogitate runtime must not be importable by accident."""

from __future__ import annotations

import importlib

import pytest


@pytest.mark.parametrize(
    "module_name",
    [
        "solstone.think.cogitate_contract",
        "solstone.think.cogitate_policy",
        "solstone.think.providers.cli",
        "solstone.think.providers.openhands",
        "solstone.think.providers.emit_final_tool",
        "solstone.think.providers.read_tools",
        "solstone.think.cogitate_read_tools",
    ],
)
def test_retired_cogitate_runtime_module_cannot_be_imported(module_name: str) -> None:
    with pytest.raises(ModuleNotFoundError):
        importlib.import_module(module_name)
