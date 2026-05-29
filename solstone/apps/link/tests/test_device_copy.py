# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

from solstone.apps.link import copy


def test_device_section_copy_is_locked() -> None:
    assert copy.DEVICE_SECTION_TITLE == "your devices"
    assert copy.DEVICE_PAIR_CTA == "pair a device"
    assert copy.DEVICE_EMPTY_TITLE == "no devices connected yet"
    assert copy.DEVICE_EMPTY_BODY == "pair a device to read your journal on the go."
    assert copy.RECENT_SECTION_TITLE == "recently paired"
    assert copy.RECENT_NETWORK_LABEL == "on your network"
    assert copy.REFRESH_FAIL_NOTICE == "showing the last state we saw"
    assert copy.UNPAIR_TITLE_FORMAT == "unpair '{label}'?"
    assert copy.UNPAIR_BODY == (
        "this device loses access to your solstone immediately and can't reconnect until "
        "you pair it again. anything stored on the device stays on the device."
    )
    assert copy.DEVICE_STATUS_LABELS == {
        "online": "online",
        "recent": "recently seen",
        "offline": "offline",
    }
    assert copy.DEVICE_GROUP_LABELS == {
        "observers": "observers",
        "peers": "peers",
    }
    assert copy.DEVICE_ACTION_LABELS == {
        "rename": "rename",
        "copy_fingerprint": "copy fingerprint",
        "unpair": "unpair",
    }
    assert copy.RECENT_SEE_ALL_LABEL == "see all ▸"
    assert copy.RECENT_SHOW_LESS_LABEL == "show less ▾"
    assert copy.FINGERPRINT_COPY_SUCCESS_TOAST == "fingerprint copied"
    assert copy.FINGERPRINT_COPY_FAIL_TOAST == "couldn't copy fingerprint"
    assert copy.RENAME_FAIL_TOAST == "couldn't rename device"
    assert copy.UNPAIR_SUCCESS_TOAST == "unpaired"
    assert copy.UNPAIR_FAIL_TOAST == "couldn't unpair this device"
