# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Cohesion labels for speaker voiceprint consistency."""

from __future__ import annotations

from solstone.apps.speakers.copy import SPK_OVERVIEW_COHESION_LABELS


def cohesion_bin(intra_p25: float | None) -> int:
    """Return the 0-5 cohesion dot bin for a p25 intra-speaker cosine."""
    if intra_p25 is None:
        return 0
    if intra_p25 >= 0.50:
        return 5
    if intra_p25 >= 0.40:
        return 4
    if intra_p25 >= 0.30:
        return 3
    if intra_p25 >= 0.20:
        return 2
    if intra_p25 >= 0.10:
        return 1
    return 0


def cohesion_label(intra_p25: float | None) -> str:
    """Return the copy label for a p25 intra-speaker cosine."""
    return SPK_OVERVIEW_COHESION_LABELS[cohesion_bin(intra_p25)]
