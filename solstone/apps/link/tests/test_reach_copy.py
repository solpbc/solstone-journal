# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Regression tests for locked link reach-shell copy constants."""

from __future__ import annotations

import re

from solstone.apps.link import copy

U2_COPY_VALUES = [
    copy.HEADER_TRUST_LINE,
    copy.POSTURE_MODAL_FOOTER,
    copy.REACH_CARD_TITLE,
    copy.REACH_DIRECT_LABEL,
    copy.REACH_DIRECT_DETAIL,
    copy.REACH_HOME_ADDRESS_LABEL,
    copy.REACH_VPN_CANDIDATE_LABEL,
    copy.REACH_VPN_USE_THIS,
    copy.REACH_CHANGE_LABEL,
    copy.REACH_UPGRADE_TITLE,
    copy.REACH_UPGRADE_BODY,
    copy.REACH_UPGRADE_LINK_LABEL,
    copy.POSTURE_MODAL_TITLE,
    copy.POSTURE_DIRECT_DESC,
    copy.POSTURE_SPL_TITLE,
    copy.POSTURE_SPL_DESC,
    copy.POSTURE_SPL_SETUP_LABEL,
    copy.POSTURE_SPL_MANAGE_LABEL,
    *copy.STATUS_SENTENCES.values(),
]


def test_reach_shell_spec_fixed_copy_is_locked() -> None:
    assert copy.HEADER_TRUST_LINE == (
        "sol pbc carries the connection — but can never see inside it"
    )
    assert copy.POSTURE_MODAL_FOOTER == (
        "switching is gentle — devices you've paired keep working either way, no re-pairing."
    )
    assert copy.STATUS_SENTENCES == {
        "direct_online": "your solstone is reachable on your network.",
        "direct_online_vpn": "your solstone is reachable on your network and over your VPN.",
        "reconnecting": "reconnecting to your solstone...",
        "offline": "can't reach your solstone right now.",
        "lan_unreachable": "your solstone is running, but devices can't reach it to pair yet.",
        "spl_online": "your solstone is reachable from anywhere.",
        "spl_finishing_setup": "finishing setup with sol private link...",
        "checking": "checking your solstone...",
    }


def test_reach_shell_corrected_copy_is_locked() -> None:
    assert copy.REACH_CARD_TITLE == "how your devices reach home"
    assert copy.POSTURE_MODAL_TITLE == "how should your devices reach home?"
    assert copy.REACH_DIRECT_LABEL == "on your network or your own VPN (free)"
    assert copy.REACH_DIRECT_DETAIL == (
        "your devices connect to this solstone directly, with no one in the middle."
    )


def test_reach_shell_copy_stays_in_bounds() -> None:
    banned_terms = (
        "account",
        "price",
        "$",
        "billing",
        "subscription",
        "invoice",
        "plan",
        "phone",
    )
    acronym_re = re.compile(r"\b(dl|pl|spl)\b")

    for value in U2_COPY_VALUES:
        lowered = value.lower()
        for term in banned_terms:
            assert term not in lowered, value
        assert not acronym_re.search(lowered), value
