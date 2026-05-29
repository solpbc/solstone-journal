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

# --- U2 reach-shell copy ---
HEADER_TRUST_LINE = "sol pbc carries the connection — but can never see inside it"
POSTURE_MODAL_FOOTER = "switching is gentle — devices you've paired keep working either way, no re-pairing."
STATUS_SENTENCES = {
    "direct_online": "your solstone is reachable on your network.",
    "direct_online_vpn": "your solstone is reachable on your network and over your VPN.",
    "reconnecting": "reconnecting to your solstone...",
    "offline": "can't reach your solstone right now.",
    "lan_unreachable": "your solstone is running, but devices can't reach it to pair yet.",
    "spl_online": "your solstone is reachable from anywhere.",
    "spl_finishing_setup": "finishing setup with sol private link...",
    "checking": "checking your solstone...",
}
REACH_CARD_TITLE = "how your devices reach home"
REACH_DIRECT_LABEL = "on your network or your own VPN (free)"
REACH_DIRECT_DETAIL = (
    "your devices connect to this solstone directly, with no one in the middle."
)
REACH_HOME_ADDRESS_LABEL = "home address"
REACH_VPN_CANDIDATE_LABEL = "VPN address"
REACH_VPN_USE_THIS = "use this"
REACH_CHANGE_LABEL = "change"
REACH_UPGRADE_TITLE = "reach from anywhere"
REACH_UPGRADE_BODY = (
    "when you're away, sol private link can carry the connection for paired devices."
)
REACH_UPGRADE_LINK_LABEL = "set up sol private link at services.solstone.app"
POSTURE_MODAL_TITLE = "how should your devices reach home?"
POSTURE_DIRECT_DESC = "devices connect locally or through your own VPN."
POSTURE_SPL_TITLE = "from anywhere · sol private link"
POSTURE_SPL_DESC = "sol pbc carries the connection and cannot see inside it."
POSTURE_SPL_SETUP_LABEL = "set up sol private link at services.solstone.app →"
POSTURE_SPL_MANAGE_LABEL = "manage at services.solstone.app →"
