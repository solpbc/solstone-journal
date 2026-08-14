# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Regression tests for locked link reach-shell copy constants."""

from __future__ import annotations

from solstone.apps.network import copy


def test_reach_shell_spec_fixed_copy_is_locked() -> None:
    assert copy.STATUS_SENTENCES == {
        "direct_online": "your journal is reachable on your network.",
        "direct_online_vpn": "your journal is reachable on your network and over your VPN.",
        "reconnecting": "reconnecting to your journal…",
        "offline": "can't reach your journal right now.",
        "lan_unreachable": "your journal is running, but devices can't reach it to pair yet.",
        "spl_online": "your journal is reachable from anywhere.",
        "spl_not_enrolled": "your private network isn't set up yet — devices can't connect from away.",
        "spl_finishing_setup": "finishing setup with your private network…",
        "spl_offline": (
            "your journal isn't reaching the network right now — devices can't "
            "connect from away. on your home wifi they still work."
        ),
        "checking": "checking your journal…",
    }


def test_reach_shell_corrected_copy_is_locked() -> None:
    assert copy.BRANDLOCK_LINE == "your journal is always private, only yours."
    assert copy.REACH_SELECTOR_TITLE == "how your devices reach your journal"
    assert copy.REACH_SELECTOR_HINT == (
        "your choice — switch anytime. either way, what syncs is end-to-end "
        "encrypted and only your devices can read it."
    )
    assert copy.MODE_BYO_NAME == "your own"
    assert copy.MODE_BYO_DESC == (
        "your devices reach your journal over your own network — same wifi, or "
        "your own VPN. the default."
    )
    assert copy.MODE_BYO_DISCLOSURE == "sol pbc is never in the path"
    assert copy.MODE_HOSTED_NAME == "private network"
    assert copy.MODE_HOSTED_DESC == (
        "reach your journal from anywhere, through a relay sol pbc runs for you."
    )
    assert copy.MODE_HOSTED_DISCLOSURE == "operated by sol pbc"
    assert copy.MODE_BYO_BODY_NOTE == (
        "your journal stays on this device. your other devices connect straight "
        "to it — nothing routes through sol pbc."
    )
    assert copy.MODE_HOSTED_SETUP_NOTE == (
        "your journal stays on this device; the relay only passes along "
        "encrypted traffic it can't read."
    )
    assert copy.MODE_HOSTED_SETUP_CTA == "set up the relay →"
    assert copy.APP_ONOFF_LABEL == "network"
    assert copy.APP_ONOFF_SUB_BYO == "on — reachable over your own network"
    assert copy.APP_ONOFF_SUB_HOSTED == "on — reachable from anywhere"
    assert copy.REACH_HOST_ADDRESS_DISCLOSURE == "▸ use a different address"
    assert copy.REACH_HOST_ADDRESS_PLACEHOLDER == "192.168.1.44:7657"
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
        == "manage your private network at services.solstone.app →"
    )
    assert (
        copy.REACH_SPL_CONNECTING_NOTE
        == "your home is connecting. this is usually quick."
    )
    assert copy.CHECK_AGAIN_LABEL == "check again"
    assert copy.PRIVATE_LINK_DISABLE_CTA == "turn off your private network"
    assert copy.PRIVATE_LINK_SETTING_UP == "setting up your private network…"
    assert copy.PRIVATE_LINK_PORTAL_CTA == "continue to approve →"
    assert (
        copy.PRIVATE_LINK_SETUP_SUCCESS
        == "your private network is on. your devices can reach home from anywhere."
    )
    assert (
        copy.PRIVATE_LINK_SETUP_FAILED
        == "couldn't finish setting up your private network."
    )
    assert (
        copy.PRIVATE_LINK_DISABLE_SUCCESS
        == "your private network is off. devices connect directly again."
    )
    assert (
        copy.PRIVATE_LINK_DISABLE_FAILED
        == "couldn't turn off your private network — it's still on. try again."
    )
    assert (
        copy.PRIVATE_LINK_NEEDS_REPAIR == "your private network needs setting up again."
    )
    assert copy.PRIVATE_LINK_RETRY_CTA == "try again"
