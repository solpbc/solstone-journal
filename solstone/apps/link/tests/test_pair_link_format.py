# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

from solstone.apps.link.routes import _build_pair_link


def test_pair_link_v2_matches_reference_vector() -> None:
    url = _build_pair_link(
        "192.0.2.42",
        7070,
        "a1b2c3d4e5f60718",
        "deadbeefcafebabe0123456789abcdef",
    )

    assert (
        url
        == "https://link.solpbc.org/p#080W000258DSX8DJRFAEBXG733FAVFQFSBZBNFG14D2PF2DBSQQG"
    )
