# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Owner-facing copy and locked constants for the link pairing flow."""

from __future__ import annotations

from solstone.think.link.nonces import NONCE_TTL_SECONDS

PAIR_LINK_HOST = "link.solpbc.org"
PAIR_LINK_PATH = "/p"
MANUAL_CODE_ALPHABET = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"  # Crockford base32.
MANUAL_CODE_LEN = 8
MANUAL_CODE_GROUP = 4
PAIR_CODE_TTL_SECONDS = NONCE_TTL_SECONDS
CLI_MANUAL_CODE_LABEL = "manual code"
MODAL_TITLE = "pair a phone"
STEP_1 = "open the camera on your phone"
STEP_2 = "point at this code"
STEP_3 = "tap to open solstone"
MANUAL_CODE_LABEL = "can't scan? type this on your phone:"
TRUST_COPY = "only scan with your own phone. expires in 5 minutes."
LAN_URL_LABEL = "server:"
DETAILS_DISCLOSURE = "details"
CA_FP_LABEL = "server fingerprint:"
CA_FP_NOTE = (
    "the phone verifies this fingerprint when it scans, "
    "so a wifi attacker can't impersonate this server."
)
DEVICE_LABEL_PLACEHOLDER = "e.g. my iPhone"
DEVICE_LABEL_DEFAULT_FORMAT = "Phone — added {month} {day}"
AUTO_REFRESH_HINT = "code refreshes automatically"
EXPIRED_BUTTON = "code expired — generate new code"
SUCCESS_HEADING = '"{label}" is now paired with your solstone'
SUCCESS_SUBHEAD = "{short_fp} · paired just now"
SUCCESS_DONE = "done"
