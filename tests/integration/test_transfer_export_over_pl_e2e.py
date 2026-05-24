# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import pytest

pytestmark = pytest.mark.integration


def test_transfer_export_over_pl_e2e() -> None:
    pytest.skip("requires Lode F receiver journal-source PL ingest routes")
