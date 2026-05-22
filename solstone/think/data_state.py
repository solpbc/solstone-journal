# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Shared per-modality data-state vocabulary."""

from enum import StrEnum


class DataState(StrEnum):
    """Read-only visibility state for modality data."""

    ANALYZED = "analyzed"
    PENDING = "pending"
    FAILED = "failed"
    PURGED = "purged"
    ABSENT = "absent"
