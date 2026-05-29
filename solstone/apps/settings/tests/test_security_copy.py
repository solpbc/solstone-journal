# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import re

from solstone.convey import copy as convey_copy


def test_security_reach_hint_is_locked():
    assert convey_copy.SETTINGS_SECURITY_REACH_HINT == (
        "how your devices reach home — managing your connection, paired devices, "
        "and reach-from-anywhere now lives in link →"
    )


def test_security_reach_hint_stays_in_bounds():
    lowered = convey_copy.SETTINGS_SECURITY_REACH_HINT.lower()

    assert "account" not in lowered
    assert not re.search(r"\b(dl|pl|spl)\b", lowered)
    assert "phone" not in lowered
    assert "iphone" not in lowered


def test_orphaned_network_mode_constants_removed():
    for name in (
        "SETTINGS_NETWORK_MODE_LABEL",
        "SETTINGS_NETWORK_MODE_OFF",
        "SETTINGS_NETWORK_MODE_ON",
        "SETTINGS_NETWORK_DESC_OFF",
        "SETTINGS_NETWORK_DESC_ON",
        "SETTINGS_NETWORK_BUTTON_ENABLE",
        "SETTINGS_NETWORK_BUTTON_DISABLE",
        "SETTINGS_NETWORK_DISCLOSURE_TITLE",
        "SETTINGS_NETWORK_DISCLOSURE_BODY",
        "SETTINGS_NETWORK_DISCLOSURE_PASSWORD_LABEL",
        "SETTINGS_NETWORK_DISCLOSURE_CONFIRM_LABEL",
        "SETTINGS_NETWORK_DISCLOSURE_SUBMIT",
        "SETTINGS_NETWORK_DISCLOSURE_MISMATCH",
        "SETTINGS_NETWORK_DISCLOSURE_TOO_SHORT",
        "SETTINGS_NETWORK_RESTARTING",
    ):
        assert not hasattr(convey_copy, name), name
        assert name not in convey_copy.__all__, name
