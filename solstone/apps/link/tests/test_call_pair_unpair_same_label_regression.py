# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

from typer.testing import CliRunner

from solstone.apps.link import call
from solstone.think.link.auth import AuthorizedClients
from solstone.think.link.paths import authorized_clients_path


def test_unpair_same_label_removes_first_inserted_match(tmp_path, monkeypatch) -> None:
    journal = tmp_path / "journal"
    journal.mkdir()
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    monkeypatch.setattr(call, "require_solstone", lambda: None)
    store = AuthorizedClients(authorized_clients_path())
    store.add("sha256:phone", "laptop", "inst-1")
    store.add("sha256:observer", "laptop", "inst-1", role="observer")

    result = CliRunner().invoke(call.app, ["unpair", "laptop"])

    assert result.exit_code == 0
    remaining = AuthorizedClients(authorized_clients_path()).snapshot()
    assert len(remaining) == 1
    # find_by_label returns the first insertion-order match, so the observer survives.
    assert remaining[0].fingerprint == "sha256:observer"
    assert remaining[0].role == "observer"
