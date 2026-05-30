# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Caller-side observer SPL bundle path helpers."""

from __future__ import annotations

import os
from pathlib import Path


def observer_spl_root() -> Path:
    config_home = os.environ.get("XDG_CONFIG_HOME")
    base = Path(config_home) if config_home else Path.home() / ".config"
    return base / "solstone-observer" / "spl"


def observer_bundle_dir(label: str) -> Path:
    return observer_spl_root() / label
