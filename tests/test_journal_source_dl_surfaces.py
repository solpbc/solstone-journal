# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import argparse
from importlib import import_module

import pytest
from flask import Flask

import solstone.convey.state as convey_state
import solstone.think.utils as think_utils

journal_sources = import_module("solstone.apps.import.journal_sources")
import_routes = import_module("solstone.apps.import.routes")
journal_source_cli = import_module("solstone.think.importers.journal_source_cli")

generate_key = journal_sources.generate_key
journal_source_state_prefix = journal_sources.journal_source_state_prefix
load_journal_source_by_fingerprint = journal_sources.load_journal_source_by_fingerprint
save_journal_source = journal_sources.save_journal_source

FINGERPRINT = "sha256:" + "e" * 64


@pytest.fixture
def journal_env(tmp_path, monkeypatch):
    monkeypatch.setattr(convey_state, "journal_root", str(tmp_path), raising=False)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    think_utils._journal_path_cache = None
    (tmp_path / "apps" / "import" / "journal_sources").mkdir(
        parents=True, exist_ok=True
    )
    return tmp_path


def _dl_source() -> dict:
    return {
        "key": generate_key(),
        "name": "alpha",
        "created_at": 1000,
        "enabled": True,
        "revoked": False,
        "revoked_at": None,
        "stats": {
            "segments_received": 0,
            "entities_received": 0,
            "facets_received": 0,
            "imports_received": 0,
            "config_received": 0,
        },
    }


def _pl_source() -> dict:
    return {
        "pair_mode": "pl",
        "fingerprint": FINGERPRINT,
        "device_label": "peer laptop",
        "paired_at": "2026-05-20T00:00:00Z",
        "created_at": 2000,
        "enabled": True,
        "revoked": False,
        "revoked_at": None,
        "stats": {
            "segments_received": 0,
            "entities_received": 0,
            "facets_received": 0,
            "imports_received": 0,
            "config_received": 0,
        },
    }


def _save_dl_and_pl() -> dict:
    dl_source = _dl_source()
    assert save_journal_source(dl_source) is True
    assert save_journal_source(_pl_source()) is True
    return dl_source


def test_api_journal_source_list_excludes_pl_records(journal_env) -> None:
    dl_source = _save_dl_and_pl()
    app = Flask(__name__)
    app.register_blueprint(import_routes.import_bp)

    response = app.test_client().get("/app/import/api/journal-sources/list")

    assert response.status_code == 200
    assert response.get_json() == [
        {
            "name": "alpha",
            "prefix": journal_source_state_prefix(dl_source),
            "status": "active",
            "created_at": 1000,
        }
    ]


def test_cli_status_and_revoke_cannot_target_pl_fingerprint_by_name(
    journal_env, capsys
) -> None:
    _save_dl_and_pl()

    status_rc = journal_source_cli.cmd_status(
        argparse.Namespace(name=FINGERPRINT, json_output=True)
    )
    status = capsys.readouterr()

    revoke_rc = journal_source_cli.cmd_revoke(
        argparse.Namespace(name=FINGERPRINT, json_output=True)
    )
    revoke = capsys.readouterr()

    assert status_rc == 1
    assert f"journal source '{FINGERPRINT}' not found" in status.err
    assert revoke_rc == 1
    assert f"journal source '{FINGERPRINT}' not found" in revoke.err
    pl_record = load_journal_source_by_fingerprint(FINGERPRINT)
    assert pl_record is not None
    assert pl_record["revoked"] is False
