# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for the journal password CLI."""

from __future__ import annotations

import json

import pytest
from werkzeug.security import check_password_hash

from solstone.think.password_cli import main


def _read_config(journal_dir):
    return json.loads((journal_dir / "config" / "journal.json").read_text())


def _mock_getpass(monkeypatch, *responses):
    """Mock getpass.getpass to return successive responses."""
    it = iter(responses)
    monkeypatch.setattr(
        "solstone.think.password_cli.getpass.getpass", lambda prompt="": next(it)
    )


class TestSetPassword:
    def test_set_writes_hash(self, journal_copy, monkeypatch, capsys):
        monkeypatch.setattr("sys.argv", ["sol password", "set"])
        _mock_getpass(monkeypatch, "mypassword", "mypassword")

        main()

        config = _read_config(journal_copy)
        assert config["convey"]["password_hash"].startswith("scrypt:")
        assert check_password_hash(config["convey"]["password_hash"], "mypassword")
        assert "Password set successfully." in capsys.readouterr().out

    def test_mismatch_rejected(self, journal_copy, monkeypatch, capsys):
        monkeypatch.setattr("sys.argv", ["sol password", "set"])
        _mock_getpass(monkeypatch, "password1", "different")

        with pytest.raises(SystemExit) as exc_info:
            main()

        assert exc_info.value.code == 1
        assert "Passwords do not match." in capsys.readouterr().err

    def test_plaintext_cleanup(self, journal_copy, monkeypatch):
        # Seed a plaintext password
        config_path = journal_copy / "config" / "journal.json"
        config = json.loads(config_path.read_text())
        config["convey"]["password"] = "old-plaintext"
        config_path.write_text(json.dumps(config, indent=2))

        monkeypatch.setattr("sys.argv", ["sol password", "set"])
        _mock_getpass(monkeypatch, "newpass", "newpass")

        main()

        config = _read_config(journal_copy)
        assert "password" not in config["convey"]
        assert "password_hash" in config["convey"]

    def test_no_config_file(self, tmp_path, monkeypatch, capsys):
        """Works when no journal.json exists yet."""
        monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
        monkeypatch.setattr("sys.argv", ["sol password", "set"])
        _mock_getpass(monkeypatch, "freshpass", "freshpass")

        main()

        config_path = tmp_path / "config" / "journal.json"
        assert config_path.exists()
        config = json.loads(config_path.read_text())
        assert check_password_hash(config["convey"]["password_hash"], "freshpass")
        assert config_path.stat().st_mode & 0o777 == 0o600

    def test_file_permissions(self, journal_copy, monkeypatch):
        monkeypatch.setattr("sys.argv", ["sol password", "set"])
        _mock_getpass(monkeypatch, "securepass", "securepass")

        main()

        config_path = journal_copy / "config" / "journal.json"
        assert config_path.stat().st_mode & 0o777 == 0o600


class TestResetAlias:
    def test_reset_writes_hash(self, journal_copy, monkeypatch, capsys):
        monkeypatch.setattr("sys.argv", ["sol password", "reset"])
        _mock_getpass(monkeypatch, "resetpass", "resetpass")

        main()

        config = _read_config(journal_copy)
        assert check_password_hash(config["convey"]["password_hash"], "resetpass")
        assert "Password set successfully." in capsys.readouterr().out
