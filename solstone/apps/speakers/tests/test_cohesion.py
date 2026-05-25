# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for speaker cohesion label bins."""

from __future__ import annotations

import pytest

from solstone.apps.speakers.copy import SPK_OVERVIEW_COHESION_LABELS


@pytest.mark.parametrize(
    ("value", "index"),
    [
        (None, 0),
        (0.099, 0),
        (0.10, 1),
        (0.20, 2),
        (0.30, 3),
        (0.40, 4),
        (0.50, 5),
        (0.99, 5),
    ],
)
def test_cohesion_label_boundaries(value, index):
    from solstone.apps.speakers.cohesion import cohesion_label

    assert cohesion_label(value) == SPK_OVERVIEW_COHESION_LABELS[index]
