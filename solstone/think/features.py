# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import importlib.util
import sys
from dataclasses import dataclass
from typing import Literal

Platform = Literal["linux", "darwin"]


@dataclass(frozen=True)
class Feature:
    name: str
    summary: str
    pip_modules: tuple[str, ...]
    apt_packages: tuple[str, ...]
    brew_packages: tuple[str, ...]
    usage: str


FEATURES: dict[str, Feature] = {
    "pdf": Feature(
        name="pdf",
        summary="PDF rendering and ingestion",
        pip_modules=("weasyprint", "pypdf", "pdf2image"),
        apt_packages=("libpango-1.0-0", "libpangoft2-1.0-0", "poppler-utils"),
        brew_packages=("pango", "poppler"),
        usage="Render reflections to PDF and ingest PDF documents into the journal",
    ),
    "whisper": Feature(
        name="whisper",
        summary="Whisper transcription backend (optional)",
        pip_modules=("faster_whisper",),
        apt_packages=(),
        brew_packages=(),
        usage="Transcribe audio with the Whisper backend",
    ),
}


def is_available(name: str) -> bool:
    feature = FEATURES[name]
    return all(
        importlib.util.find_spec(module) is not None for module in feature.pip_modules
    )


def install_hint(name: str, platform: Platform) -> str:
    feature = FEATURES[name]
    hint = f"pip install 'solstone[{name}]'"
    if platform == "linux":
        packages = feature.apt_packages
        manager = "apt"
    else:
        packages = feature.brew_packages
        manager = "brew"
    if packages:
        hint = f"{hint} and {manager} install {' '.join(packages)}"
    return hint


class MissingExtraError(RuntimeError):
    def __init__(self, name: str, platform: Platform):
        hint = install_hint(name, platform)
        super().__init__(f"feature '{name}' requires the [{name}] extra: {hint}")
        self.name = name


def require_extra(name: str) -> None:
    platform: Platform = "darwin" if sys.platform == "darwin" else "linux"
    if not is_available(name):
        raise MissingExtraError(name, platform)
