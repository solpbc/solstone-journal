# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import logging

from solstone.log_policy import apply_http_logging_policy, snapshot_root_logging
from tests._logging_isolation import preserve_global_logging


def test_apply_sets_httpx_warning_baseline_less():
    logging.getLogger("httpx").setLevel(logging.INFO)

    apply_http_logging_policy()

    assert logging.getLogger("httpx").level == logging.WARNING


def test_apply_reconciles_root_against_snapshot():
    root = logging.getLogger()
    baseline_level, baseline_handlers = snapshot_root_logging()
    added = logging.NullHandler()

    try:
        root.setLevel(logging.INFO)
        root.addHandler(added)

        apply_http_logging_policy((baseline_level, baseline_handlers))

        assert root.level == baseline_level
        assert all(handler is not added for handler in root.handlers)
        for baseline_handler in baseline_handlers:
            assert any(handler is baseline_handler for handler in root.handlers)
    finally:
        if any(handler is added for handler in root.handlers):
            root.removeHandler(added)
        root.setLevel(baseline_level)
        for baseline_handler in baseline_handlers:
            if all(handler is not baseline_handler for handler in root.handlers):
                root.addHandler(baseline_handler)
