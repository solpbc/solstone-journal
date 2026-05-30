# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Regression tests for locked link reach-shell copy constants."""

from __future__ import annotations

import re

from solstone.apps.link import copy

U2_COPY_VALUES = [
    copy.POSTURE_MODAL_FOOTER,
    copy.REACH_CARD_TITLE,
    copy.REACH_DIRECT_LABEL,
    copy.REACH_DIRECT_DETAIL,
    copy.REACH_HOME_ADDRESS_LABEL,
    copy.REACH_HOST_ADDRESS_DISCLOSURE,
    copy.REACH_HOST_ADDRESS_PLACEHOLDER,
    copy.REACH_HOST_ADDRESS_APPLY_LABEL,
    copy.REACH_HOST_ADDRESS_CLEAR_LABEL,
    copy.REACH_VPN_CANDIDATE_LABEL,
    copy.REACH_VPN_USE_THIS,
    copy.REACH_CHANGE_LABEL,
    copy.REACH_UPGRADE_TITLE,
    copy.REACH_UPGRADE_BODY,
    copy.REACH_UPGRADE_LINK_LABEL,
    copy.REACH_SPL_ACTIVE_BODY,
    copy.REACH_SPL_TRUST_LINE,
    copy.REACH_SPL_MANAGE_LABEL,
    copy.REACH_SPL_CONNECTING_NOTE,
    copy.CHECK_AGAIN_LABEL,
    copy.POSTURE_MODAL_TITLE,
    copy.POSTURE_DIRECT_DESC,
    copy.POSTURE_SPL_TITLE,
    copy.POSTURE_SPL_DESC,
    copy.POSTURE_SPL_SETUP_LABEL,
    copy.POSTURE_SPL_MANAGE_LABEL,
    *copy.STATUS_SENTENCES.values(),
]

U5_COPY_VALUES = [
    copy.LAN_BANNER_TITLE,
    copy.LAN_BANNER_BODY,
    copy.LAN_BANNER_ENABLE_CTA,
    copy.LAN_BANNER_PASSWORD_INTRO,
    copy.LAN_BANNER_PASSWORD_LABEL,
    copy.LAN_BANNER_CONFIRM_LABEL,
    copy.LAN_BANNER_PASSWORD_TOO_SHORT,
    copy.LAN_BANNER_PASSWORD_MISMATCH,
    copy.LAN_BANNER_RESTARTING,
    copy.LAN_BANNER_SLOW,
    copy.LAN_BANNER_STILL_UNREACHABLE,
    copy.LAN_BANNER_RETRY,
    copy.LAN_BANNER_DIY_LABEL,
    copy.LAN_BANNER_DIY_BODY,
]


def test_reach_shell_spec_fixed_copy_is_locked() -> None:
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
        "spl_offline": (
            "your solstone isn't reaching the network right now — devices can't "
            "connect from away. on your home wifi they still work."
        ),
        "checking": "checking your solstone...",
    }


def test_reach_shell_corrected_copy_is_locked() -> None:
    assert copy.REACH_CARD_TITLE == "how your devices reach home"
    assert copy.POSTURE_MODAL_TITLE == "how should your devices reach home?"
    assert copy.REACH_DIRECT_LABEL == "on your network or your own VPN (free)"
    assert copy.REACH_DIRECT_DETAIL == (
        "your devices connect to this solstone directly, with no one in the middle."
    )
    assert copy.REACH_HOST_ADDRESS_DISCLOSURE == "▸ use a different address"
    assert copy.REACH_HOST_ADDRESS_PLACEHOLDER == "192.168.1.44:5015"
    assert copy.REACH_HOST_ADDRESS_APPLY_LABEL == "apply"
    assert copy.REACH_HOST_ADDRESS_CLEAR_LABEL == "clear"
    assert (
        copy.REACH_SPL_ACTIVE_BODY
        == "your devices reach home over the internet, wherever you are."
    )
    assert copy.REACH_SPL_TRUST_LINE == (
        "the connection is end-to-end encrypted — sol pbc and cloudflare can see "
        "that your device and home met, and nothing inside."
    )
    assert (
        copy.REACH_SPL_MANAGE_LABEL
        == "manage sol private link at services.solstone.app →"
    )
    assert (
        copy.REACH_SPL_CONNECTING_NOTE
        == "your home is connecting. this is usually quick."
    )
    assert copy.CHECK_AGAIN_LABEL == "check again"


def test_lan_banner_copy_is_locked() -> None:
    assert copy.LAN_BANNER_TITLE == "let devices reach this solstone"
    assert copy.LAN_BANNER_BODY == (
        "pairing needs this web interface to accept connections from your network. "
        "you can turn that on here."
    )
    assert copy.LAN_BANNER_ENABLE_CTA == "turn on network access"
    assert copy.LAN_BANNER_PASSWORD_INTRO == (
        "set a web password first. other devices will need it before opening your journal."
    )
    assert copy.LAN_BANNER_PASSWORD_LABEL == "web password"
    assert copy.LAN_BANNER_CONFIRM_LABEL == "confirm web password"
    assert (
        copy.LAN_BANNER_PASSWORD_TOO_SHORT == "password must be at least 8 characters."
    )
    assert copy.LAN_BANNER_PASSWORD_MISMATCH == "passwords do not match."
    assert copy.LAN_BANNER_RESTARTING == "turning on network access..."
    assert copy.LAN_BANNER_SLOW == (
        "saved. this is taking longer than usual. reload in a moment to check."
    )
    assert copy.LAN_BANNER_STILL_UNREACHABLE == (
        "network access is on, but this page still cannot see a network address. "
        "try the steps below."
    )
    assert copy.LAN_BANNER_RETRY == "couldn't turn on network access. try again."
    assert copy.LAN_BANNER_DIY_LABEL == "do it yourself ▸"
    assert copy.LAN_BANNER_DIY_BODY == (
        "if you're running from source, start convey on your network with "
        "make dev PORT=0.0.0.0:5015, or set convey.host in your journal config "
        "to a non-loopback interface, then reload this page."
    )


def test_pair_web_password_settings_link_is_locked() -> None:
    assert (
        copy.PAIR_WEB_PASSWORD_SETTINGS_LINK
        == "set a web password for this page in settings →"
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

    for value in [
        *U2_COPY_VALUES,
        *U5_COPY_VALUES,
        copy.PAIR_WEB_PASSWORD_SETTINGS_LINK,
    ]:
        lowered = value.lower()
        for term in banned_terms:
            assert term not in lowered, value
        assert not acronym_re.search(lowered), value
