# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Syncable importer backend framework."""

import json
import logging
from pathlib import Path
from typing import Any, Protocol, runtime_checkable

from solstone.think.importers.shared import (
    PRIVATE_IMPORT_FILE_MODE,
    ensure_private_import_dir,
)
from solstone.think.journal_io import atomic_replace

logger = logging.getLogger(__name__)


@runtime_checkable
class SyncableBackend(Protocol):
    """Protocol for importer backends that support syncing."""

    name: str

    def sync(self, journal_root: Path, *, dry_run: bool = True) -> dict[str, Any]: ...


SYNCABLE_REGISTRY: dict[str, str] = {
    "plaud": "solstone.think.importers.plaud",
    "obsidian": "solstone.think.importers.obsidian",
    "audio": "solstone.think.importers.audio",
}


def load_sync_state(journal_root: Path, backend: str) -> dict[str, Any] | None:
    """Load sync state for a backend."""
    state_path = journal_root / "imports" / f"{backend}.json"
    if not state_path.exists():
        return None

    try:
        with open(state_path, "r", encoding="utf-8") as f:
            return json.load(f)
    except (json.JSONDecodeError, OSError) as exc:
        logger.warning("Failed to load sync state for %s: %s", backend, exc)
        return None


def save_sync_state(journal_root: Path, backend: str, state: dict[str, Any]) -> None:
    """Save sync state for a backend with an atomic write."""
    imports_dir = journal_root / "imports"
    ensure_private_import_dir(imports_dir)
    state_path = imports_dir / f"{backend}.json"

    atomic_replace(
        state_path,
        json.dumps(state, indent=2),
        mode=PRIVATE_IMPORT_FILE_MODE,
    )


def get_syncable_backends() -> list[SyncableBackend]:
    """Discover and instantiate all registered syncable backends."""
    import importlib

    backends: list[SyncableBackend] = []
    for name, module_path in SYNCABLE_REGISTRY.items():
        try:
            module = importlib.import_module(module_path)
            backend = getattr(module, "backend", None)
            if isinstance(backend, SyncableBackend):
                backends.append(backend)
            else:
                logger.warning(
                    "Backend %s from %s does not conform to SyncableBackend",
                    name,
                    module_path,
                )
        except Exception as exc:
            logger.warning("Failed to load syncable backend %s: %s", name, exc)
    return backends
