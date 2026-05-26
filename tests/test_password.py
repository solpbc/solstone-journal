# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for password hashing: login, migration, and settings API."""

from __future__ import annotations

import json
from pathlib import Path

import pytest
from werkzeug.security import check_password_hash

from solstone.convey import create_app
from tests.conftest import copytree_tracked


@pytest.fixture
def client(journal_copy):
    app = create_app(str(journal_copy))
    app.config["TESTING"] = True
    return app.test_client()


def _read_config(journal_dir):
    return json.loads((journal_dir / "config" / "journal.json").read_text())


class TestLogin:
    def test_correct_password(self, client):
        resp = client.post("/login", data={"password": "test123"})
        assert resp.status_code == 302

    def test_wrong_password(self, client):
        resp = client.post("/login", data={"password": "wrong"})
        assert resp.status_code == 200
        assert b"incorrect password. passwords are case-sensitive." in resp.data

    def test_no_password_configured(self, journal_copy):
        config = _read_config(journal_copy)
        config["convey"].pop("password_hash", None)
        config["convey"].pop("password", None)
        (journal_copy / "config" / "journal.json").write_text(
            json.dumps(config, indent=2)
        )
        app = create_app(str(journal_copy))
        app.config["TESTING"] = True
        client = app.test_client()
        resp = client.get("/login")
        assert b"journal password set" in resp.data


class TestMigration:
    def test_plaintext_migrated_to_hash(self, tmp_path, monkeypatch):
        """Plaintext password is hashed and old key removed on app creation."""
        src = Path(__file__).resolve().parent / "fixtures" / "journal"
        dst = tmp_path / "journal"
        copytree_tracked(src, dst)
        config_path = dst / "config" / "journal.json"
        config = json.loads(config_path.read_text())
        config["convey"].pop("password_hash", None)
        config["convey"]["password"] = "migrate-me"
        config_path.write_text(json.dumps(config, indent=2))
        monkeypatch.setenv("SOLSTONE_JOURNAL", str(dst))

        create_app(str(dst))

        config = json.loads(config_path.read_text())
        assert "password" not in config["convey"]
        assert "password_hash" in config["convey"]
        assert check_password_hash(config["convey"]["password_hash"], "migrate-me")

    def test_empty_password_removed(self, tmp_path, monkeypatch):
        """Empty plaintext password is removed, not hashed."""
        src = Path(__file__).resolve().parent / "fixtures" / "journal"
        dst = tmp_path / "journal"
        copytree_tracked(src, dst)
        config_path = dst / "config" / "journal.json"
        config = json.loads(config_path.read_text())
        config["convey"].pop("password_hash", None)
        config["convey"]["password"] = ""
        config_path.write_text(json.dumps(config, indent=2))
        monkeypatch.setenv("SOLSTONE_JOURNAL", str(dst))

        create_app(str(dst))

        config = json.loads(config_path.read_text())
        assert "password" not in config["convey"]
        assert "password_hash" not in config["convey"]

    def test_already_migrated_skipped(self, journal_copy):
        """If password_hash exists, migration is a no-op."""
        config_before = _read_config(journal_copy)
        hash_before = config_before["convey"]["password_hash"]

        create_app(str(journal_copy))

        config_after = _read_config(journal_copy)
        assert config_after["convey"]["password_hash"] == hash_before


class TestSettingsAPI:
    def test_get_config_strips_password(self, client):
        """GET /app/settings/api/config must not return password or password_hash."""
        resp = client.get("/app/settings/api/config")
        data = resp.get_json()
        convey = data.get("convey", {})
        assert "password" not in convey
        assert "password_hash" not in convey
        assert convey.get("has_password") is True

    def test_put_hashes_password(self, client, journal_copy):
        """PUT with convey.password hashes before writing to disk."""
        resp = client.put(
            "/app/settings/api/config",
            json={"section": "convey", "data": {"password": "new-secret"}},
            content_type="application/json",
        )
        assert resp.status_code == 200
        config = _read_config(journal_copy)
        assert "password" not in config["convey"]
        assert check_password_hash(config["convey"]["password_hash"], "new-secret")

    def test_put_empty_password_skipped(self, client, journal_copy):
        """PUT with empty password does not overwrite existing hash."""
        config_before = _read_config(journal_copy)
        hash_before = config_before["convey"]["password_hash"]

        resp = client.put(
            "/app/settings/api/config",
            json={"section": "convey", "data": {"password": ""}},
            content_type="application/json",
        )
        assert resp.status_code == 200
        config_after = _read_config(journal_copy)
        assert config_after["convey"]["password_hash"] == hash_before
