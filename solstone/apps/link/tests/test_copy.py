# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Regression tests for locked link pairing copy constants."""

from __future__ import annotations

from solstone.apps.link import copy
from solstone.think.link.nonces import NONCE_TTL_SECONDS


def test_copy_constants_are_locked() -> None:
    assert copy.PAIR_LINK_HOST == "link.solpbc.org"
    assert copy.PAIR_LINK_PATH == "/p"
    assert copy.MANUAL_CODE_ALPHABET == "0123456789ABCDEFGHJKMNPQRSTVWXYZ"
    assert len(copy.MANUAL_CODE_ALPHABET) == 32
    assert not {"I", "L", "O", "U"} & set(copy.MANUAL_CODE_ALPHABET)
    assert copy.MANUAL_CODE_LEN == 8
    assert copy.MANUAL_CODE_GROUP == 4
    assert copy.PAIR_CODE_TTL_SECONDS == NONCE_TTL_SECONDS
    assert copy.PAIR_CODE_TTL_SECONDS == 300
    assert copy.CLI_MANUAL_CODE_LABEL == "manual code"
    assert copy.MODAL_TITLE == "pair a phone"
    assert copy.STEP_1 == "open the camera on your phone"
    assert copy.STEP_2 == "point at this code"
    assert copy.STEP_3 == "tap to open solstone"
    assert copy.MANUAL_CODE_LABEL == "can't scan? type this on your phone:"
    assert copy.TRUST_COPY == "only scan with your own phone. expires in 5 minutes."
    assert copy.LAN_URL_LABEL == "server:"
    assert copy.DETAILS_DISCLOSURE == "details"
    assert copy.CA_FP_LABEL == "server fingerprint:"
    assert copy.CA_FP_NOTE == (
        "the phone verifies this fingerprint when it scans, "
        "so a wifi attacker can't impersonate this server."
    )
    assert copy.DEVICE_LABEL_PLACEHOLDER == "e.g. my iPhone"
    assert copy.DEVICE_LABEL_DEFAULT_FORMAT == "Phone — added {month} {day}"
    assert copy.AUTO_REFRESH_HINT == "code refreshes automatically"
    assert copy.EXPIRED_BUTTON == "code expired — generate new code"
    assert copy.SUCCESS_HEADING == '"{label}" is now paired with your solstone'
    assert copy.SUCCESS_SUBHEAD == "{short_fp} · paired just now"
    assert copy.SUCCESS_DONE == "done"
