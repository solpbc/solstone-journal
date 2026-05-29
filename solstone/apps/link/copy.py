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
MODAL_TITLE = "pair a device"
STEP_1 = "open the camera on the device you're adding"
STEP_2 = "point it at this code"
STEP_3 = "tap the link to open solstone"
MANUAL_CODE_LABEL = "can't scan? type this on the device:"
PAIR_NETWORK_LINE = (
    "this device needs to be on your network (or your VPN) to pair. expires in 5:00."
)
DETAILS_DISCLOSURE = "verify this is really your home"
CA_FP_LABEL = "fingerprint"
CA_FP_NOTE = (
    "the device checks this when it scans, so no one on your wifi can impersonate home."
)
DEVICE_LABEL_FIELD_LABEL = "name this device"
DEVICE_LABEL_PLACEHOLDER = "e.g. my iPhone"
DEVICE_LABEL_DEFAULT_FORMAT = "device — added {month} {day}"
EXPIRED_BUTTON = "this code expired — show a new one"
SUCCESS_HEADING = '"{label}" is now paired with your solstone'
SUCCESS_SUBHEAD = "{short_fp} · paired just now"
SUCCESS_DONE = "done"
PAIR_ERROR_BODY = (
    "can't start pairing — your solstone isn't reachable on a network address yet."
)
SUCCESS_VERIFY_NOTE = (
    "check the device you just paired — this fingerprint should match what it "
    "shows. didn't do this?"
)
SUCCESS_REMOVE_LABEL = "that wasn't me — remove"
PAIR_WEB_PASSWORD_SETTINGS_LINK = "set a web password for this page in settings →"

# --- U4 first-run hero ---
HERO_TITLE = "let's connect a device"
HERO_BODY = (
    "your journal lives here, on this machine. to read it from your phone or "
    "laptop, that device needs a way to reach it. right now it can be reached "
    "on your home network."
)
HERO_HOW_REACH_LABEL = "how reach works ▸"

# --- U5 LAN banner ---
LAN_BANNER_TITLE = "let devices reach this solstone"
LAN_BANNER_BODY = (
    "pairing needs this web interface to accept connections from your network. "
    "you can turn that on here."
)
LAN_BANNER_ENABLE_CTA = "turn on network access"
LAN_BANNER_PASSWORD_INTRO = (
    "set a web password first. other devices will need it before opening your journal."
)
LAN_BANNER_PASSWORD_LABEL = "web password"
LAN_BANNER_CONFIRM_LABEL = "confirm web password"
LAN_BANNER_PASSWORD_TOO_SHORT = "password must be at least 8 characters."
LAN_BANNER_PASSWORD_MISMATCH = "passwords do not match."
LAN_BANNER_RESTARTING = "turning on network access..."
LAN_BANNER_SLOW = (
    "saved. this is taking longer than usual. reload in a moment to check."
)
LAN_BANNER_STILL_UNREACHABLE = (
    "network access is on, but this page still cannot see a network address. "
    "try the steps below."
)
LAN_BANNER_RETRY = "couldn't turn on network access. try again."
LAN_BANNER_DIY_LABEL = "do it yourself ▸"
LAN_BANNER_DIY_BODY = (
    "if you're running from source, start convey on your network with "
    "make dev PORT=0.0.0.0:5015, or set convey.host in your journal config "
    "to a non-loopback interface, then reload this page."
)

# --- U2 reach-shell copy ---
# No unconditional header trust line: relay-trust messaging is posture-specific
# (direct card = "no one in the middle"; spl card = POSTURE_SPL_DESC). An always-on
# "sol pbc carries the connection" claim is false in direct posture (spec § problem).
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

# --- U3 device-section copy ---
DEVICE_SECTION_TITLE = "your devices"
DEVICE_PAIR_CTA = "pair a device"
DEVICE_EMPTY_TITLE = "no devices connected yet"
DEVICE_EMPTY_BODY = "pair a device to read your journal on the go."
RECENT_SECTION_TITLE = "recently paired"
RECENT_NETWORK_LABEL = "on your network"
REFRESH_FAIL_NOTICE = "showing the last state we saw"
UNPAIR_TITLE_FORMAT = "unpair '{label}'?"
UNPAIR_BODY = (
    "this device loses access to your solstone immediately and can't reconnect until "
    "you pair it again. anything stored on the device stays on the device."
)
DEVICE_STATUS_LABELS = {
    "online": "online",
    "recent": "recently seen",
    "offline": "offline",
}
DEVICE_GROUP_LABELS = {
    "observers": "observers",
    "peers": "peers",
}
DEVICE_ACTION_LABELS = {
    "rename": "rename",
    "copy_fingerprint": "copy fingerprint",
    "unpair": "unpair",
}
RECENT_SEE_ALL_LABEL = "see all ▸"
RECENT_SHOW_LESS_LABEL = "show less ▾"
FINGERPRINT_COPY_SUCCESS_TOAST = "fingerprint copied"
FINGERPRINT_COPY_FAIL_TOAST = "couldn't copy fingerprint"
RENAME_FAIL_TOAST = "couldn't rename device"
UNPAIR_SUCCESS_TOAST = "unpaired"
UNPAIR_FAIL_TOAST = "couldn't unpair this device"
