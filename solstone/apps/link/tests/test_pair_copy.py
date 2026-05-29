# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Regression tests for locked U4 pair-flow copy constants."""

from __future__ import annotations

import html
import re

from solstone.apps.link import copy

U4_COPY_VALUES = [
    copy.MODAL_TITLE,
    copy.STEP_1,
    copy.STEP_2,
    copy.STEP_3,
    copy.MANUAL_CODE_LABEL,
    copy.PAIR_NETWORK_LINE,
    copy.DETAILS_DISCLOSURE,
    copy.CA_FP_LABEL,
    copy.CA_FP_NOTE,
    copy.DEVICE_LABEL_FIELD_LABEL,
    copy.DEVICE_LABEL_DEFAULT_FORMAT,
    copy.EXPIRED_BUTTON,
    copy.PAIR_ERROR_BODY,
    copy.SUCCESS_HEADING,
    copy.SUCCESS_SUBHEAD,
    copy.SUCCESS_DONE,
    copy.SUCCESS_VERIFY_NOTE,
    copy.SUCCESS_REMOVE_LABEL,
    copy.DEVICE_LABEL_PLACEHOLDER,
    copy.HERO_TITLE,
    copy.HERO_BODY,
    copy.HERO_HOW_REACH_LABEL,
]


def _normalized_body(body: str) -> str:
    return (
        html.unescape(body)
        .replace('\\"', '"')
        .replace("\\u0027", "'")
        .replace("\\u00b7", "·")
        .replace("\\u2014", "—")
        .replace("\\u2192", "→")
    )


def test_u4_copy_values_are_locked() -> None:
    assert copy.MODAL_TITLE == "pair a device"
    assert copy.STEP_1 == "open the camera on the device you're adding"
    assert copy.STEP_2 == "point it at this code"
    assert copy.STEP_3 == "tap the link to open solstone"
    assert copy.MANUAL_CODE_LABEL == "can't scan? type this on the device:"
    assert copy.PAIR_NETWORK_LINE == (
        "this device needs to be on your network (or your VPN) to pair. "
        "expires in 5:00."
    )
    assert copy.DETAILS_DISCLOSURE == "verify this is really your home"
    assert copy.CA_FP_LABEL == "fingerprint"
    assert copy.CA_FP_NOTE == (
        "the device checks this when it scans, so no one on your wifi can "
        "impersonate home."
    )
    assert copy.DEVICE_LABEL_FIELD_LABEL == "name this device"
    assert copy.DEVICE_LABEL_DEFAULT_FORMAT == "device — added {month} {day}"
    assert copy.EXPIRED_BUTTON == "this code expired — show a new one"
    assert copy.PAIR_ERROR_BODY == (
        "can't start pairing — your solstone isn't reachable on a network address yet."
    )
    assert copy.SUCCESS_HEADING == '"{label}" is now paired with your solstone'
    assert copy.SUCCESS_SUBHEAD == "{short_fp} · paired just now"
    assert copy.SUCCESS_DONE == "done"
    assert copy.SUCCESS_VERIFY_NOTE == (
        "check the device you just paired — this fingerprint should match what it "
        "shows. didn't do this?"
    )
    assert copy.SUCCESS_REMOVE_LABEL == "that wasn't me — remove"
    assert copy.DEVICE_LABEL_PLACEHOLDER == "e.g. my iPhone"
    assert copy.HERO_TITLE == "let's connect a device"
    assert copy.HERO_BODY == (
        "your journal lives here, on this machine. to read it from your phone or "
        "laptop, that device needs a way to reach it. right now it can be reached "
        "on your home network."
    )
    assert copy.HERO_HOW_REACH_LABEL == "how reach works ▸"


def test_u4_copy_stays_in_bounds() -> None:
    banned_terms = ("account",)
    acronym_re = re.compile(r"\b(dl|pl|spl)\b")
    device_noun_re = re.compile(r"\bphone\b")

    for value in U4_COPY_VALUES:
        lowered = value.lower()
        for term in banned_terms:
            assert term not in lowered, value
        assert not acronym_re.search(lowered), value

    for value in U4_COPY_VALUES:
        if value == copy.HERO_BODY:
            continue
        assert not device_noun_re.search(value.lower()), value

    assert "phone or laptop" in copy.HERO_BODY.lower()


def test_u4_copy_matches_rendered_body(link_env) -> None:
    env = link_env()
    response = env.client.get("/app/link/")

    assert response.status_code == 200
    body_text = _normalized_body(response.get_data(as_text=True))

    for value in U4_COPY_VALUES:
        assert value in body_text
