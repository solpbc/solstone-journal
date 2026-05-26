# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import inspect
import logging

# Audit AC2: pin OpenHands SDK method shapes used by the provider.
# solstone/think/providers/openhands.py:810 calls send_message without await.
# solstone/think/providers/openhands.py:812 awaits arun.


def test_local_conversation_methods_match_provider_await_sites(monkeypatch):
    root_logger = logging.getLogger()
    root_level = root_logger.level
    logger_names = ("httpx", "solstone.observe.utils")
    logger_state = {
        name: (
            logging.getLogger(name).level,
            logging.getLogger(name).disabled,
            logging.getLogger(name).propagate,
        )
        for name in logger_names
    }

    monkeypatch.setenv("OPENHANDS_SUPPRESS_BANNER", "1")
    try:
        from openhands.sdk.conversation.impl.local_conversation import (
            LocalConversation,
        )

        assert inspect.iscoroutinefunction(LocalConversation.arun) is True
        assert inspect.iscoroutinefunction(LocalConversation.send_message) is False
    finally:
        root_logger.setLevel(root_level)
        for name, (level, disabled, propagate) in logger_state.items():
            logger = logging.getLogger(name)
            logger.setLevel(level)
            logger.disabled = disabled
            logger.propagate = propagate
