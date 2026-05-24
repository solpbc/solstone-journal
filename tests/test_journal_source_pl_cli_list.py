# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import argparse
import json
import logging
from importlib import import_module

import pytest

import solstone.convey.state as convey_state
import solstone.think.utils as think_utils
from solstone.think.link.paths import authorized_clients_path

journal_sources = import_module("solstone.apps.import.journal_sources")
journal_source_cli = import_module("solstone.think.importers.journal_source_cli")

generate_key = journal_sources.generate_key
save_journal_source = journal_sources.save_journal_source

FINGERPRINT = "sha256:" + "e" * 64
OTHER_FINGERPRINT = "sha256:" + "f" * 64
LOGGER_NAME = "solstone.think.importers.journal_source_cli"
PAIRED_AT = "2026-05-20T00:00:00Z"
LAST_SEEN_AT = "2026-04-19T18:03:12Z"


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
        "paired_at": PAIRED_AT,
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


def _save_source(source: dict) -> dict:
    assert save_journal_source(source) is True
    return source


def _save_dl_and_pl(pl_source: dict | None = None) -> tuple[dict, dict]:
    dl_source = _save_source(_dl_source())
    pl_source = _save_source(pl_source or _pl_source())
    return dl_source, pl_source


def _cmd_list_json(capsys, mode: str | None = None) -> list[dict]:
    rc = journal_source_cli.cmd_list(argparse.Namespace(json_output=True, mode=mode))
    assert rc == 0
    return json.loads(capsys.readouterr().out)


def _cmd_list_human(capsys, mode: str | None = None) -> str:
    rc = journal_source_cli.cmd_list(argparse.Namespace(json_output=False, mode=mode))
    assert rc == 0
    return capsys.readouterr().out.strip()


def _row_by_mode(rows: list[dict], mode: str) -> dict:
    matches = [row for row in rows if row["mode"] == mode]
    assert len(matches) == 1
    return matches[0]


def _auth_entry(
    fingerprint: str = FINGERPRINT, last_seen_at: str | None = LAST_SEEN_AT
) -> dict:
    return {
        "fingerprint": fingerprint,
        "device_label": "peer laptop",
        "paired_at": "2026-04-19T17:42:13Z",
        "instance_id": "home-instance",
        "role": "peer",
        "last_seen_at": last_seen_at,
    }


def _write_authorized_clients(entries: list[dict]) -> None:
    path = authorized_clients_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(entries), encoding="utf-8")


def test_list_shows_both_dl_and_pl_records_json(journal_env, capsys) -> None:
    _save_dl_and_pl()

    rows = _cmd_list_json(capsys)

    assert {row["mode"] for row in rows} == {"dl", "pl"}
    assert _row_by_mode(rows, "pl")["fingerprint"] == FINGERPRINT
    assert _row_by_mode(rows, "dl")["name"] == "alpha"


def test_list_human_table_shows_both_modes(journal_env, capsys) -> None:
    _save_dl_and_pl()

    out = _cmd_list_human(capsys)

    assert "dl" in out
    assert "pl" in out
    assert "alpha" in out
    assert "peer laptop" in out


def test_mode_dl_filters_to_dl_only(journal_env, capsys) -> None:
    _save_dl_and_pl()

    rows = _cmd_list_json(capsys, mode="dl")

    assert [row["mode"] for row in rows] == ["dl"]
    assert rows[0]["name"] == "alpha"


def test_mode_pl_filters_to_pl_only(journal_env, capsys) -> None:
    _save_dl_and_pl()

    rows = _cmd_list_json(capsys, mode="pl")

    assert [row["mode"] for row in rows] == ["pl"]
    assert rows[0]["fingerprint"] == FINGERPRINT


def test_pl_row_omits_peer_instance_id_when_absent(journal_env, capsys) -> None:
    _save_source(_pl_source())

    row = _cmd_list_json(capsys)[0]

    assert "peer_instance_id" not in row


def test_pl_row_includes_peer_instance_id_when_present(journal_env, capsys) -> None:
    source = _pl_source()
    source["peer_instance_id"] = "inst-abc"
    _save_source(source)

    row = _cmd_list_json(capsys)[0]
    out = _cmd_list_human(capsys)

    assert row["peer_instance_id"] == "inst-abc"
    assert "inst-abc" in out


def test_pl_last_seen_present_with_timestamp(journal_env, capsys) -> None:
    _save_source(_pl_source())
    _write_authorized_clients([_auth_entry(last_seen_at=LAST_SEEN_AT)])

    row = _cmd_list_json(capsys)[0]
    out = _cmd_list_human(capsys)

    assert row["last_seen_at"] == LAST_SEEN_AT
    assert row["auth_status"] == "present"
    assert LAST_SEEN_AT in out


def test_pl_last_seen_present_but_null(journal_env, capsys) -> None:
    source = _pl_source()
    source["peer_instance_id"] = "inst-abc"
    _save_source(source)
    _write_authorized_clients([_auth_entry(last_seen_at=None)])

    row = _cmd_list_json(capsys)[0]
    out = _cmd_list_human(capsys)

    assert row["last_seen_at"] is None
    assert row["auth_status"] == "present"
    assert f"{PAIRED_AT} —" in out


def test_pl_last_seen_missing_auth_entry(journal_env, capsys) -> None:
    _save_source(_pl_source())
    _write_authorized_clients([_auth_entry(fingerprint=OTHER_FINGERPRINT)])

    row = _cmd_list_json(capsys)[0]
    out = _cmd_list_human(capsys)

    assert row["last_seen_at"] is None
    assert row["auth_status"] == "missing"
    assert "(no auth)" in out


def test_revoked_pl_record_renders_as_revoked(journal_env, capsys) -> None:
    source = _pl_source()
    source["revoked"] = True
    _save_source(source)

    row = _cmd_list_json(capsys)[0]
    out = _cmd_list_human(capsys)

    assert row["status"] == "revoked"
    assert "revoked" in out


def test_dl_row_json_shape_has_exactly_five_keys(journal_env, capsys) -> None:
    _save_source(_dl_source())

    row = _cmd_list_json(capsys)[0]

    assert set(row.keys()) == {"mode", "prefix", "name", "status", "created_at"}
    assert not {
        "fingerprint",
        "device_label",
        "paired_at",
        "last_seen_at",
        "auth_status",
        "peer_instance_id",
    } & set(row.keys())


def test_missing_authorized_clients_file_no_warning(
    journal_env, capsys, caplog
) -> None:
    _save_source(_pl_source())

    with caplog.at_level(logging.WARNING, logger=LOGGER_NAME):
        out = _cmd_list_human(capsys)

    assert "(no auth)" in out
    assert caplog.records == []


def test_malformed_authorized_clients_file_emits_warning_and_renders_no_auth(
    journal_env, capsys, caplog
) -> None:
    _save_source(_pl_source())
    path = authorized_clients_path()
    path.write_text("{not valid json", encoding="utf-8")

    with caplog.at_level(logging.WARNING, logger=LOGGER_NAME):
        out = _cmd_list_human(capsys)

    assert "(no auth)" in out
    assert len(caplog.records) == 1
    assert str(path) in caplog.records[0].getMessage()

    caplog.clear()
    with caplog.at_level(logging.WARNING, logger=LOGGER_NAME):
        row = _cmd_list_json(capsys)[0]

    assert len(caplog.records) == 1
    assert row["last_seen_at"] is None
    assert row["auth_status"] == "missing"


def test_empty_registry_human_and_json(journal_env, capsys) -> None:
    assert _cmd_list_human(capsys) == "No journal sources registered."
    assert _cmd_list_json(capsys) == []
    assert _cmd_list_human(capsys, mode="pl") == "No journal sources match --mode pl."
    assert _cmd_list_json(capsys, mode="pl") == []


def test_mode_pl_filter_with_dl_only_registry_gives_filtered_empty_message(
    journal_env, capsys
) -> None:
    _save_source(_dl_source())

    out = _cmd_list_human(capsys, mode="pl")

    assert out == "No journal sources match --mode pl."
