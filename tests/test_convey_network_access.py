# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import copy
import json
from pathlib import Path
from unittest.mock import patch

import pytest
from werkzeug.security import check_password_hash

from solstone.convey.network_access import (
    NetworkAccessPasswordRequired,
    NetworkAccessPasswordTooShort,
    set_network_access,
)


def _read_config(journal_dir: Path) -> dict:
    return json.loads((journal_dir / "config" / "journal.json").read_text("utf-8"))


def _write_config(journal_dir: Path, payload: dict) -> None:
    (journal_dir / "config" / "journal.json").write_text(
        json.dumps(payload, indent=2) + "\n",
        encoding="utf-8",
    )


def _clear_password(journal_dir: Path) -> None:
    config = _read_config(journal_dir)
    config["convey"].pop("password_hash", None)
    config["convey"].pop("password", None)
    config["convey"]["allow_network_access"] = False
    _write_config(journal_dir, config)


def test_enable_requires_password_without_persisting_or_restart(journal_copy):
    _clear_password(journal_copy)
    before = copy.deepcopy(_read_config(journal_copy))

    with patch("solstone.convey.restart.wait_for_convey_restart") as restart:
        with pytest.raises(NetworkAccessPasswordRequired):
            set_network_access(enable=True)

    restart.assert_not_called()
    assert _read_config(journal_copy) == before


def test_enable_with_password_hashes_persists_and_restarts(journal_copy):
    _clear_password(journal_copy)

    with (
        patch(
            "solstone.convey.restart.wait_for_convey_restart", return_value=(True, [])
        ),
        patch(
            "solstone.think.pairing.config.get_host_url",
            return_value="http://192.168.1.44:5015",
        ),
    ):
        result = set_network_access(enable=True, password="atomicpw8")

    assert result == {
        "ok": True,
        "restart_timeout": False,
        "effective_host_url": "http://192.168.1.44:5015",
    }
    config = _read_config(journal_copy)
    assert config["convey"]["allow_network_access"] is True
    assert "password" not in config["convey"]
    assert check_password_hash(config["convey"]["password_hash"], "atomicpw8")


def test_enable_with_short_password_preserves_config_and_skips_restart(journal_copy):
    _clear_password(journal_copy)
    before = copy.deepcopy(_read_config(journal_copy))

    with patch("solstone.convey.restart.wait_for_convey_restart") as restart:
        with pytest.raises(NetworkAccessPasswordTooShort):
            set_network_access(enable=True, password="short")

    restart.assert_not_called()
    assert _read_config(journal_copy) == before


def test_disable_persists_and_reports_restart_timeout(journal_copy):
    config = _read_config(journal_copy)
    config["convey"]["allow_network_access"] = True
    _write_config(journal_copy, config)

    with (
        patch(
            "solstone.convey.restart.wait_for_convey_restart", return_value=(False, [])
        ),
        patch(
            "solstone.think.pairing.config.get_host_url",
            return_value="http://localhost:5015",
        ),
    ):
        result = set_network_access(enable=False)

    assert result == {
        "ok": True,
        "restart_timeout": True,
        "effective_host_url": "http://localhost:5015",
    }
    assert _read_config(journal_copy)["convey"]["allow_network_access"] is False
