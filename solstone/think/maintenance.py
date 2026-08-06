# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""App-owned scheduled maintenance routine registry."""

from __future__ import annotations

import hashlib
import importlib.util
import logging
import re
from collections.abc import Callable, Mapping
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from solstone.think.schedule_config import (
    read_schedules,
    remove_schedule_entry,
    set_schedule_entries,
)
from solstone.think.scheduler import INTERVALS
from solstone.think.utils import parse_duration_seconds

logger = logging.getLogger(__name__)

APPS_DIR = Path(__file__).parent.parent / "apps"
MAINTENANCE_PREFIX = "maintenance:"
SLUG_RE = re.compile(r"^[a-z][a-z0-9_-]*$")


@dataclass
class MaintenanceRoutine:
    """Descriptor exported by app maintenance modules."""

    name: str
    description: str
    every: str
    run: Callable[[list[str]], int]
    max_runtime: str | None = None


class MaintenanceDescriptorError(ValueError):
    """Raised when an app exports a malformed maintenance descriptor."""


def discover_routines() -> dict[str, MaintenanceRoutine]:
    """Discover app-owned maintenance routines from ``apps/*/maintenance.py``."""
    routines: dict[str, MaintenanceRoutine] = {}
    if not APPS_DIR.exists():
        return routines

    for app_dir in sorted(APPS_DIR.iterdir()):
        if not app_dir.is_dir() or app_dir.name.startswith("_"):
            continue

        maintenance_file = app_dir / "maintenance.py"
        if not maintenance_file.exists():
            continue

        app_name = app_dir.name
        try:
            module = _load_maintenance_module(app_name, maintenance_file)
        except Exception as exc:
            logger.error(
                "Failed to load maintenance routines from app '%s': %s",
                app_name,
                exc,
                exc_info=True,
            )
            continue

        exported = getattr(module, "ROUTINES", None)
        if exported is None:
            logger.warning(
                "apps/%s/maintenance.py has no ROUTINES list, skipping",
                app_name,
            )
            continue
        if not isinstance(exported, list):
            logger.warning(
                "apps/%s/maintenance.py ROUTINES is not a list, skipping",
                app_name,
            )
            continue

        for descriptor in exported:
            routine = _validate_descriptor(app_name, descriptor)
            routine_id = f"{app_name}:{routine.name}"
            if routine_id in routines:
                raise _descriptor_error(
                    app_name,
                    routine.name,
                    "name",
                    "duplicate routine name",
                )
            routines[routine_id] = routine

    return dict(sorted(routines.items()))


def is_maintenance_schedule_name(name: str) -> bool:
    """Return whether ``name`` is owned by the maintenance scheduler registry."""
    return name.startswith(MAINTENANCE_PREFIX)


def maintenance_schedule_name(routine_id: str) -> str:
    """Return the schedule entry name for a discovered routine id."""
    return f"{MAINTENANCE_PREFIX}{routine_id}"


def expected_schedule_entry(
    routine_id: str, routine: MaintenanceRoutine
) -> dict[str, Any]:
    """Return the raw config/schedules.json entry for one routine."""
    entry: dict[str, Any] = {
        "cmd": ["journal", "maintenance", "run", routine_id],
        "every": routine.every,
        "enabled": True,
    }
    if routine.max_runtime is not None:
        entry["max_runtime"] = routine.max_runtime
    return entry


def get_routine_statuses(
    routines: Mapping[str, MaintenanceRoutine] | None = None,
    raw_schedules: Mapping[str, Any] | None = None,
) -> dict[str, str]:
    """Return divergence status per discovered routine id."""
    discovered = discover_routines() if routines is None else routines
    schedules = read_schedules() if raw_schedules is None else raw_schedules
    statuses = {
        routine_id: _routine_status(routine_id, routine, schedules)
        for routine_id, routine in discovered.items()
    }
    return dict(sorted(statuses.items()))


def register_maintenance_schedules() -> dict[str, list[str]]:
    """Add missing app-owned maintenance schedules.

    ``sync`` is safe by construction: it is additive only. It writes missing
    generated entries, but never overwrites, deletes, or re-enables existing
    owned entries, except that it idempotently removes the retired
    ``maintenance:health:release-raw`` entry and touches no other entry.
    Divergent and disabled entries are reported so the operator can make an
    intentional follow-up change. This mirrors ``scheduler.register_defaults()``
    and does not need a dry-run/commit gate.
    """
    routines = discover_routines()
    remove_schedule_entry("maintenance:health:release-raw")
    raw = read_schedules()
    statuses = get_routine_statuses(routines, raw)

    summary = {
        "added": sorted(
            routine_id for routine_id, status in statuses.items() if status == "missing"
        ),
        "synced": sorted(
            routine_id for routine_id, status in statuses.items() if status == "synced"
        ),
        "divergent": sorted(
            routine_id
            for routine_id, status in statuses.items()
            if status == "divergent"
        ),
        "disabled": sorted(
            routine_id
            for routine_id, status in statuses.items()
            if status == "disabled"
        ),
    }

    additions = {
        maintenance_schedule_name(routine_id): expected_schedule_entry(
            routine_id, routines[routine_id]
        )
        for routine_id in summary["added"]
    }
    if additions:
        set_schedule_entries(additions)
        logger.info(
            "Registered maintenance schedules: added=%d synced=%d divergent=%d disabled=%d",
            len(summary["added"]),
            len(summary["synced"]),
            len(summary["divergent"]),
            len(summary["disabled"]),
        )

    if summary["divergent"] or summary["disabled"]:
        logger.warning(
            "Maintenance schedules need attention: divergent=%s disabled=%s",
            ",".join(summary["divergent"]) or "-",
            ",".join(summary["disabled"]) or "-",
        )

    return summary


def _load_maintenance_module(app_name: str, path: Path) -> Any:
    digest = hashlib.sha1(str(path.resolve()).encode("utf-8")).hexdigest()[:12]
    module_name = f"_solstone_maintenance_{app_name}_{digest}"
    spec = importlib.util.spec_from_file_location(module_name, path)
    if spec is None or spec.loader is None:
        raise ImportError(f"could not create import spec for {path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _validate_descriptor(app_name: str, descriptor: Any) -> MaintenanceRoutine:
    if not isinstance(descriptor, MaintenanceRoutine):
        raise _descriptor_error(
            app_name,
            "<invalid>",
            "routine",
            "expected MaintenanceRoutine",
        )

    name = descriptor.name
    if not isinstance(name, str):
        raise _descriptor_error(
            app_name,
            "<invalid>",
            "name",
            "expected slug string",
        )
    if not name:
        raise _descriptor_error(app_name, "<missing>", "name", "must not be empty")
    if ":" in name:
        raise _descriptor_error(app_name, name, "name", "must not contain ':'")
    if SLUG_RE.fullmatch(name) is None:
        raise _descriptor_error(
            app_name,
            name,
            "name",
            "must match ^[a-z][a-z0-9_-]*$",
        )

    if descriptor.every not in INTERVALS:
        expected = ", ".join(sorted(INTERVALS))
        raise _descriptor_error(
            app_name,
            name,
            "every",
            f"expected one of {expected}",
        )

    if not callable(descriptor.run):
        raise _descriptor_error(app_name, name, "run", "expected callable")

    max_runtime = descriptor.max_runtime
    if max_runtime is not None:
        if not isinstance(max_runtime, str):
            raise _descriptor_error(
                app_name,
                name,
                "max_runtime",
                "expected duration string",
            )
        try:
            parse_duration_seconds(max_runtime)
        except ValueError as exc:
            raise _descriptor_error(
                app_name,
                name,
                "max_runtime",
                "expected duration string",
            ) from exc

    return descriptor


def _descriptor_error(
    app_name: str, routine_name: str, field: str, reason: str
) -> MaintenanceDescriptorError:
    return MaintenanceDescriptorError(
        f"maintenance routine {app_name}:{routine_name}: invalid {field}: {reason}"
    )


def _routine_status(
    routine_id: str,
    routine: MaintenanceRoutine,
    raw_schedules: Mapping[str, Any],
) -> str:
    schedule_name = maintenance_schedule_name(routine_id)
    if schedule_name not in raw_schedules:
        return "missing"

    entry = raw_schedules[schedule_name]
    if isinstance(entry, Mapping) and entry.get("enabled", True) is False:
        return "disabled"

    if not isinstance(entry, Mapping):
        return "divergent"

    expected = expected_schedule_entry(routine_id, routine)
    if _entry_matches_expected(entry, expected):
        return "synced"
    return "divergent"


def _entry_matches_expected(
    entry: Mapping[str, Any], expected: Mapping[str, Any]
) -> bool:
    return (
        entry.get("cmd") == expected.get("cmd")
        and entry.get("every") == expected.get("every")
        and _max_runtime_value(entry) == _max_runtime_value(expected)
    )


def _max_runtime_value(entry: Mapping[str, Any]) -> Any:
    return entry.get("max_runtime") if "max_runtime" in entry else None
