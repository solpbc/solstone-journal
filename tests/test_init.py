# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import json
import re
from pathlib import Path
from types import SimpleNamespace

import pytest

from solstone.apps.observer.routes import ACTIVE_THRESHOLD_MS, STALE_THRESHOLD_MS
from solstone.apps.observer.utils import save_observer
from solstone.apps.thinking.copy import CONFIDENTIAL_LANE_DETAIL, LANES
from solstone.convey import create_app
from solstone.think.utils import get_journal, now_ms
from tests.helpers.journal_config import seed_journal_config


def _read_config(journal_dir):
    return json.loads((journal_dir / "config" / "journal.json").read_text())


def _read_init_state(client):
    response = client.get("/init/api/state")
    assert response.status_code == 200
    return response.get_json()


def _make_empty_client(tmp_path, monkeypatch):
    journal = tmp_path / "journal"
    journal.mkdir()
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    app = create_app(str(journal))
    app.config["TESTING"] = True
    return app.test_client(), journal


def _commit_journal_identity() -> None:
    from solstone.think.link.ca import load_or_generate_ca
    from solstone.think.link.paths import ca_dir

    load_or_generate_ca(ca_dir())


def _clear_setup(journal_dir):
    config = _read_config(journal_dir)
    config.pop("setup", None)
    seed_journal_config(config, journal_dir)


def _save_test_observer(
    key_prefix: str,
    name: str,
    *,
    created_at: int,
    last_seen: int | None,
    revoked: bool = False,
):
    key = key_prefix + ("f" * 56)
    assert save_observer(
        {
            "key": key,
            "name": name,
            "created_at": created_at,
            "last_seen": last_seen,
            "last_segment": None,
            "enabled": True,
            "revoked": revoked,
            "revoked_at": created_at + 1 if revoked else None,
            "stats": {},
        }
    )
    return key


@pytest.fixture
def fresh_client(journal_copy):
    _clear_setup(journal_copy)
    app = create_app(str(journal_copy))
    app.config["TESTING"] = True
    return app.test_client()


@pytest.fixture
def configured_client(journal_copy):
    app = create_app(str(journal_copy))
    app.config["TESTING"] = True
    return app.test_client()


class TestInitDetection:
    def test_redirects_to_init_when_setup_incomplete(self, fresh_client):
        resp = fresh_client.get("/")
        assert resp.status_code == 302
        assert "/init" in resp.headers["Location"]

    def test_setup_complete_serves_root(self, configured_client):
        resp = configured_client.get("/")
        assert resp.status_code == 302
        assert resp.headers["Location"].endswith("/app/home/")

    def test_init_page_renders(self, fresh_client):
        resp = fresh_client.get("/init")
        assert resp.status_code == 200
        assert b"create your journal" in resp.data
        state = _read_init_state(fresh_client)
        assert state["identity_name"] == "Test User"
        assert state["identity_preferred"] == "Tester"
        assert b'id="section-password"' not in resp.data
        assert b'id="password"' not in resp.data

    def test_init_title_is_welcome_setup(self, fresh_client):
        resp = fresh_client.get("/init")
        assert "<title>create your journal — solstone</title>".encode() in resp.data

    def test_init_renders_version(self, fresh_client):
        try:
            from importlib.metadata import version as _v

            expected = _v("solstone")
        except Exception:
            expected = "dev"

        assert _read_init_state(fresh_client)["version"] in {expected, "dev"}

    def test_init_state_carries_thinking_lanes(self, fresh_client):
        state = _read_init_state(fresh_client)

        assert state["lanes"] == [dict(lane) for lane in LANES]
        assert state["confidential"]["lane_detail"] == dict(CONFIDENTIAL_LANE_DETAIL)
        assert set(state["confidential"]["lane_detail"]) == {
            "heading",
            "sub",
            "mechanism",
            "egress",
            "claims",
            "attestation",
            "early_access",
        }

    def test_init_renders_journal_path_in_welcome(self, fresh_client):
        journal_path = str(Path(get_journal()))

        resp = fresh_client.get("/init")

        assert _read_init_state(fresh_client)["journal_path"] == journal_path
        assert b"solstone is three things working together" not in resp.data
        assert b"solstone is two things working together" not in resp.data
        assert b"your journal lives on this computer." in resp.data
        assert (
            b"sol experiences your day with you and keeps it all here, in your journal"
            in resp.data
        )

    def test_init_sol_agent_section_renders(self, fresh_client):
        resp = fresh_client.get("/init")
        assert b">how should sol think?<" in resp.data
        assert b'id="provider-lanes"' in resp.data
        assert b'id="provider-skip"' in resp.data

    def test_init_sol_agent_paragraphs(self, fresh_client):
        resp = fresh_client.get("/init")
        assert (
            b"sol reasons about your journal and answers you. it thinks the way "
            b"you choose here \xe2\x80\x94 and you can change it anytime in "
            b"<b>thinking</b>."
        ) in resp.data
        assert (
            b"when you finish, <b>thinking</b> opens to the lane you picked. "
            b"the fork lives in thinking from here on \xe2\x80\x94 switch anytime."
        ) in resp.data

    def test_init_no_legacy_trust_note(self, fresh_client):
        resp = fresh_client.get("/init")
        assert b"your key is stored locally" not in resp.data

    def test_init_lane_cards_are_dynamic(self, fresh_client):
        resp = fresh_client.get("/init")
        assert b'id="provider-lane-local"' not in resp.data
        assert b'id="provider-lane-confidential"' not in resp.data
        assert b'id="provider-lane-byo"' not in resp.data
        assert b"LANE_GLYPHS" in resp.data

    def test_machine_card_present_and_verbatim(self, fresh_client):
        resp = fresh_client.get("/init")
        assert b"your journal lives on this computer." in resp.data
        assert (
            b"sol experiences your day with you and keeps it all here, "
            b"in your journal. nothing else is required to start."
        ) in resp.data
        assert b"sol pbc offers a few optional services" not in resp.data
        assert (
            b"turn them on if they help. turn them off whenever you want. or never."
            not in resp.data
        )
        assert (
            b"observers \xe2\x80\x94 experience your day along with you"
            not in resp.data
        )
        assert b"where memories are stored and curated by sol" not in resp.data

    def test_machine_card_section_id(self, fresh_client):
        resp = fresh_client.get("/init")
        assert b'id="section-machine-card"' in resp.data
        assert b'id="section-trinity"' not in resp.data

    def test_identity_hint_refresh(self, fresh_client):
        resp = fresh_client.get("/init")
        assert (
            b"optional \xe2\x80\x94 just helps sol know what to call you." in resp.data
        )
        assert (
            b"optional \xe2\x80\x94 helps sol address you correctly." not in resp.data
        )

    def test_footer_note_refresh(self, fresh_client):
        resp = fresh_client.get("/init")
        assert (
            b"your journal stays on this computer "
            b"\xe2\x80\x94 nothing leaves unless you send it."
        ) in resp.data
        assert b"your data stays on your machine" not in resp.data

    def test_portal_unreachable_stub_inert_on_default_path(self, fresh_client):
        resp = fresh_client.get("/init")
        assert b"portal-unreachable" not in resp.data
        assert b"can't reach sol pbc right now." not in resp.data
        assert b"L11-stub: portal-unreachable" not in resp.data

    def test_init_retention_radios_present(self, fresh_client):
        resp = fresh_client.get("/init")
        assert resp.data.count(b'<input type="radio" name="retention_mode"') == 3
        assert _read_init_state(fresh_client)["retention_mode"] == "keep"
        assert b'name="retention_mode" value="keep"' in resp.data
        assert b'name="retention_mode" value="days"' in resp.data
        assert b'name="retention_mode" value="processed"' in resp.data

    def test_init_retention_reflects_persisted_state(self, journal_copy):
        config = _read_config(journal_copy)
        config.pop("setup", None)
        config["retention"] = {"raw_media": "days", "raw_media_days": 14}
        seed_journal_config(config, journal_copy)
        app = create_app(str(journal_copy))
        app.config["TESTING"] = True

        resp = app.test_client().get("/init")

        assert b'name="retention_mode" value="days"' in resp.data
        state = app.test_client().get("/init/api/state").get_json()
        assert state["retention_mode"] == "days"
        assert state["retention_days"] == 14

    def test_init_observed_media_copy_updated(self, fresh_client):
        resp = fresh_client.get("/init")
        assert b"so you can access it again later" in resp.data
        assert b"re-derive insights" not in resp.data
        assert b"we recommend leaving this on" not in resp.data

    def test_init_observers_section_removed(self, fresh_client):
        resp = fresh_client.get("/init")
        assert b'id="section-observers"' not in resp.data

    def test_init_get_started_section_removed(self, fresh_client):
        resp = fresh_client.get("/init")
        assert b'id="section-finalize"' not in resp.data

    def test_init_finalize_button_text(self, fresh_client):
        resp = fresh_client.get("/init")
        assert b"finish welcome setup" in resp.data
        assert b'type="submit"' in resp.data
        body = resp.data.decode()
        form_start = body.index("<form ")
        button = body.index("finish welcome setup")
        form_end = body.index("</form>")
        assert form_start < button < form_end

    def test_init_redirects_when_configured(self, configured_client):
        resp = configured_client.get("/init")
        assert resp.status_code == 302
        assert resp.headers["Location"] == "/"

    def test_init_empty_journal_materializes_config(self, tmp_path, monkeypatch):
        client, journal = _make_empty_client(tmp_path, monkeypatch)

        resp = client.get("/init")

        assert resp.status_code == 200
        config = _read_config(journal)
        for field in ("name", "preferred", "timezone"):
            assert isinstance(config["identity"][field], str)
        assert "convey" not in config
        state = _read_init_state(client)
        assert state["identity_name"] == config["identity"]["name"]
        assert state["identity_preferred"] == config["identity"]["preferred"]

    def test_init_escapes_identity_values(self, journal_copy):
        config = _read_config(journal_copy)
        config.pop("setup", None)
        config["identity"]["name"] = "<script>alert(1)</script>"
        seed_journal_config(config, journal_copy)
        app = create_app(str(journal_copy))
        app.config["TESTING"] = True

        resp = app.test_client().get("/init")

        assert b"<script>alert(1)</script>" not in resp.data
        state = app.test_client().get("/init/api/state").get_json()
        assert state["identity_name"] == "<script>alert(1)</script>"

    def test_init_does_not_overwrite_existing_identity(self, journal_copy):
        config = _read_config(journal_copy)
        config.pop("setup", None)
        config["identity"]["name"] = "Existing User"
        config["identity"]["preferred"] = "Existing"
        config["identity"]["timezone"] = "UTC"
        seed_journal_config(config, journal_copy)
        before = _read_config(journal_copy)
        app = create_app(str(journal_copy))
        app.config["TESTING"] = True

        resp = app.test_client().get("/init")
        after = _read_config(journal_copy)

        assert resp.status_code == 200
        assert after == before

    def test_init_get_does_not_overwrite_corrupt_config(self, journal_copy):
        app = create_app(str(journal_copy))
        app.config["TESTING"] = True
        client = app.test_client()
        config_path = journal_copy / "config" / "journal.json"
        config_path.write_bytes(b"{ invalid json }")
        before = config_path.read_bytes()

        resp = client.get("/init")

        assert resp.status_code == 500
        assert resp.get_json()["reason_code"] == "corrupt_config"
        assert config_path.read_bytes() == before


class TestInitMark:
    def test_mark_section_scaffold(self, fresh_client):
        resp = fresh_client.get("/init")
        assert resp.status_code == 200
        assert b'id="section-journal-mark"' in resp.data
        assert b"this is your journal" in resp.data
        assert b'id="journal-mark-display"' in resp.data

    def test_mark_routes_referenced(self, fresh_client):
        resp = fresh_client.get("/init")
        assert b"/init/mark/regenerate" in resp.data
        assert b"/init/mark/lock" in resp.data
        assert b"/init/mark" in resp.data

    def test_mark_svg_wrapper_present(self, fresh_client):
        resp = fresh_client.get("/init")
        assert b'viewBox="0 0 24 24"' in resp.data
        assert b'stroke-width="2"' in resp.data

    def test_mark_error_branches_present(self, fresh_client):
        resp = fresh_client.get("/init")
        assert b'id="journal-mark-error"' in resp.data
        assert b'id="journal-mark-retry"' in resp.data
        assert resp.data.count(b"showMarkError(") >= 4

    def test_mark_copy_verbatim(self, fresh_client):
        resp = fresh_client.get("/init")
        for text in (
            "this is your journal",
            "every journal has its own mark — two symbols and two words, unique to yours. you'll recognize it whenever you connect a device.",
            "try another",
            "lock it in",
            "lock it in and this mark is your journal's, for good — it can't be changed later. try as many as you like first.",
            "locked in · this is your journal's mark, for good.",
        ):
            assert text.encode() in resp.data

    def test_no_jid_token(self, fresh_client):
        resp = fresh_client.get("/init")
        assert re.search(r"\bjid\b", resp.data.decode()) is None

    def test_finalize_blocked_when_unlocked(self, fresh_client):
        resp = fresh_client.post(
            "/init/finalize",
            json={"name": "X"},
            content_type="application/json",
        )
        assert resp.status_code == 400
        assert resp.get_json()["reason_code"] == "identity_not_locked"

    def test_finalize_catch_routes_server_error(self, fresh_client):
        resp = fresh_client.get("/init")
        assert b"data.warnings" in resp.data
        assert b"err.serverMessage" in resp.data
        assert b"showFinalizeError" in resp.data

    def test_mark_locked_on_commit(self, fresh_client):
        _commit_journal_identity()
        resp = fresh_client.get("/init/mark")
        assert resp.get_json()["locked"] is True

    def test_regenerate_blocked_when_locked(self, fresh_client):
        _commit_journal_identity()
        resp = fresh_client.post("/init/mark/regenerate")
        assert resp.status_code == 400
        assert resp.get_json()["reason_code"] == "invalid_operation_for_state"

    def test_linked_assets_load(self, fresh_client):
        resp = fresh_client.get("/init")
        assert resp.status_code == 200
        text = resp.data.decode()
        urls = re.findall(r'<script src="([^"]+)"', text)
        urls.extend(re.findall(r'<link [^>]*href="([^"]+)"', text))
        assert urls
        for url in urls:
            asset_resp = fresh_client.get(url)
            assert asset_resp.status_code == 200, url


class TestInitLocalCapability:
    """Tests for the local capability endpoint."""

    def test_validate_provider_route_removed(self, fresh_client):
        resp = fresh_client.post(
            "/init/validate-provider",
            json={"key": "test-api-key-123"},
            content_type="application/json",
        )

        assert resp.status_code == 404

    def test_local_capability_ok_payload(self, fresh_client, monkeypatch):
        report = SimpleNamespace(
            report=SimpleNamespace(
                overall="ok",
                checks=(
                    SimpleNamespace(
                        name="platform",
                        severity="ok",
                        detail="Linux (x86_64)",
                        required_bytes=1,
                        available_bytes=2,
                    ),
                    SimpleNamespace(
                        name="gpu",
                        severity="ok",
                        detail="NVIDIA GPU with 6 GB",
                        required_bytes=3,
                        available_bytes=4,
                    ),
                ),
            )
        )
        monkeypatch.setattr("solstone.think.check.build_check_report", lambda: report)

        resp = fresh_client.get("/init/api/local-capability")

        assert resp.status_code == 200
        assert resp.get_json() == {
            "overall": "ok",
            "checks": [
                {
                    "name": "platform",
                    "severity": "ok",
                    "detail": "Linux (x86_64)",
                },
                {
                    "name": "gpu",
                    "severity": "ok",
                    "detail": "NVIDIA GPU with 6 GB",
                },
            ],
        }

    def test_local_capability_blocked_payload(self, fresh_client, monkeypatch):
        report = SimpleNamespace(
            report=SimpleNamespace(
                overall="blocked",
                checks=(
                    SimpleNamespace(
                        name="platform",
                        severity="ok",
                        detail="Linux (x86_64)",
                    ),
                    SimpleNamespace(
                        name="gpu",
                        severity="blocked",
                        detail="no supported NVIDIA GPU found",
                    ),
                ),
            )
        )
        monkeypatch.setattr("solstone.think.check.build_check_report", lambda: report)

        resp = fresh_client.get("/init/api/local-capability")

        assert resp.status_code == 200
        data = resp.get_json()
        assert data["overall"] == "blocked"
        assert data["checks"][1] == {
            "name": "gpu",
            "severity": "blocked",
            "detail": "no supported NVIDIA GPU found",
        }

    def test_local_capability_probe_failure_is_unknown(self, fresh_client, monkeypatch):
        def fail_probe():
            raise RuntimeError("probe failed")

        monkeypatch.setattr("solstone.think.check.build_check_report", fail_probe)

        resp = fresh_client.get("/init/api/local-capability")

        assert resp.status_code == 200
        assert resp.get_json() == {"overall": "unknown", "checks": []}


class TestInitObservers:
    """Tests for the observer list endpoint during onboarding."""

    def test_init_observers_returns_thresholds_and_observers_dict(
        self, fresh_client, monkeypatch
    ):
        monkeypatch.setattr(
            "solstone.apps.observer.utils.list_observers",
            lambda: [],
        )
        resp = fresh_client.get("/init/observers")
        assert resp.status_code == 200
        data = resp.get_json()
        assert data == {
            "thresholds": {
                "active_ms": ACTIVE_THRESHOLD_MS,
                "stale_ms": STALE_THRESHOLD_MS,
            },
            "observers": [],
        }
        assert isinstance(data["thresholds"]["active_ms"], int)
        assert isinstance(data["thresholds"]["stale_ms"], int)

    def test_observers_available_before_setup(self, fresh_client, monkeypatch):
        """Observers endpoint works before setup completes."""
        monkeypatch.setattr(
            "solstone.apps.observer.utils.list_observers",
            lambda: [],
        )
        resp = fresh_client.get("/init/observers")
        assert resp.status_code == 200

    def test_observers_returns_list(self, fresh_client, monkeypatch):
        monkeypatch.setattr(
            "solstone.apps.observer.utils.list_observers",
            lambda: [
                {
                    "key": "abcd1234xxxx",
                    "name": "my-phone",
                    "created_at": 100,
                    "last_seen": None,
                    "last_segment": None,
                    "enabled": True,
                    "revoked": False,
                    "revoked_at": None,
                    "stats": {},
                },
                {
                    "key": "revoked1xxxx",
                    "name": "old-device",
                    "created_at": 50,
                    "last_seen": None,
                    "last_segment": None,
                    "enabled": False,
                    "revoked": True,
                    "revoked_at": 90,
                    "stats": {},
                },
            ],
        )
        resp = fresh_client.get("/init/observers")
        assert resp.status_code == 200
        data = resp.get_json()
        observers = data["observers"]
        assert len(observers) == 1
        assert observers[0]["name"] == "my-phone"
        assert observers[0]["prefix"] == "abcd1234"
        assert observers[0]["state"] == "disconnected"
        assert observers[0]["group"] == "inactive"
        assert observers[0]["label"] == "offline"
        assert observers[0]["elapsed_ms"] is None
        assert observers[0]["clock_skew"] is False

    def test_init_observers_endpoint_parity(self, fresh_client, journal_copy):
        current_now = now_ms()
        _save_test_observer(
            "aaaa0000",
            "active-observer",
            created_at=10,
            last_seen=current_now - 5_000,
        )
        _save_test_observer(
            "bbbb0000",
            "stale-observer",
            created_at=20,
            last_seen=current_now - 60_000,
        )
        _save_test_observer(
            "cccc0000",
            "disconnected-observer",
            created_at=30,
            last_seen=current_now - 600_000,
        )
        config = _read_config(journal_copy)
        config["setup"] = {"completed_at": current_now}
        seed_journal_config(config, journal_copy)

        api_resp = fresh_client.get("/app/observer/api/list")
        init_resp = fresh_client.get("/init/observers")
        assert api_resp.status_code == 200
        assert init_resp.status_code == 200

        api_by_key = {
            observer["prefix"]: observer
            for observer in api_resp.get_json()["observers"]
            if not observer["revoked"]
        }
        init_by_key = {
            observer["prefix"]: observer
            for observer in init_resp.get_json()["observers"]
        }

        assert set(init_by_key) == set(api_by_key)
        for key_prefix, init_observer in init_by_key.items():
            api_observer = api_by_key[key_prefix]
            assert init_observer["state"] == api_observer["state"]
            assert init_observer["group"] == api_observer["group"]
            assert init_observer["label"] == api_observer["label"]
            assert init_observer["clock_skew"] == api_observer["clock_skew"]
            assert abs(init_observer["elapsed_ms"] - api_observer["elapsed_ms"]) < 200


class TestInitFinalize:
    """Tests for the atomic finalize endpoint."""

    def test_finalize_saves_all_config(self, fresh_client, journal_copy):
        _commit_journal_identity()
        resp = fresh_client.post(
            "/init/finalize",
            json={
                "name": "Jane Doe",
                "preferred": "Jane",
                "timezone": "America/Denver",
            },
            content_type="application/json",
        )
        assert resp.status_code == 200
        data = resp.get_json()
        assert data["success"] is True
        assert data["redirect"] == "/app/thinking/"

        config = _read_config(journal_copy)
        assert "allow_network_access" not in config["convey"]
        # Identity
        assert config["identity"]["name"] == "Jane Doe"
        assert config["identity"]["preferred"] == "Jane"
        assert config["identity"]["timezone"] == "America/Denver"
        # Setup
        assert "completed_at" in config["setup"]

    @pytest.mark.parametrize(
        ("payload", "expected_redirect"),
        [
            ({"lane": "local"}, "/app/thinking/#local-setup"),
            ({"lane": "confidential"}, "/app/thinking/#confidential-setup"),
            ({"lane": "byo"}, "/app/thinking/#byo-setup"),
            ({}, "/app/thinking/"),
            ({"lane": ""}, "/app/thinking/"),
        ],
    )
    def test_finalize_lane_redirects(self, fresh_client, payload, expected_redirect):
        _commit_journal_identity()

        resp = fresh_client.post(
            "/init/finalize",
            json=payload,
            content_type="application/json",
        )

        assert resp.status_code == 200
        assert resp.get_json()["redirect"] == expected_redirect

    @pytest.mark.parametrize("lane", ["retired", 123, ["local"]])
    def test_finalize_invalid_lane_returns_reason(self, fresh_client, lane):
        _commit_journal_identity()

        resp = fresh_client.post(
            "/init/finalize",
            json={"lane": lane},
            content_type="application/json",
        )

        assert resp.status_code == 400
        assert resp.get_json()["reason_code"] == "invalid_request_value"

    def test_finalize_succeeds(self, fresh_client, journal_copy):
        _commit_journal_identity()
        resp = fresh_client.post(
            "/init/finalize",
            json={"name": "Jane"},
            content_type="application/json",
        )
        assert resp.status_code == 200
        data = resp.get_json()
        assert data["success"] is True
        assert data["redirect"] == "/app/thinking/"
        config = _read_config(journal_copy)
        assert "completed_at" in config["setup"]
        assert "allow_network_access" not in config["convey"]

    def test_finalize_warns_when_secure_listener_fails(self, fresh_client, monkeypatch):
        _commit_journal_identity()

        def fail_start(_app):
            raise RuntimeError("listener boom")

        monkeypatch.setattr("solstone.convey.root.start_secure_listener", fail_start)

        resp = fresh_client.post(
            "/init/finalize",
            json={"name": "Jane"},
            content_type="application/json",
        )

        assert resp.status_code == 500
        data = resp.get_json()
        assert data["committed"] is True
        assert data["secondary_effect"] == "secure_listener"

    def test_finalize_happy_path_returns_empty_warnings(
        self, fresh_client, monkeypatch
    ):
        _commit_journal_identity()
        monkeypatch.setattr(
            "solstone.convey.root.start_secure_listener",
            lambda _app: None,
        )

        resp = fresh_client.post(
            "/init/finalize",
            json={"name": "Jane"},
            content_type="application/json",
        )

        assert resp.status_code == 200
        assert resp.get_json()["warnings"] == []

    def test_finalize_minimal(self, fresh_client, journal_copy):
        """Finalize with optional fields omitted."""
        _commit_journal_identity()
        resp = fresh_client.post(
            "/init/finalize",
            json={},
            content_type="application/json",
        )
        assert resp.status_code == 200
        config = _read_config(journal_copy)
        assert "completed_at" in config["setup"]
        # No provider key written.
        assert "GOOGLE_API_KEY" not in config.get("env", {})

    def test_finalize_preserves_env_and_providers(self, fresh_client, journal_copy):
        _commit_journal_identity()
        before = _read_config(journal_copy)

        resp = fresh_client.post(
            "/init/finalize",
            json={"lane": "byo"},
            content_type="application/json",
        )

        assert resp.status_code == 200
        after = _read_config(journal_copy)
        assert after.get("env") == before.get("env")
        assert after.get("providers") == before.get("providers")
        assert "lane" not in after

    def test_finalize_empty_journal_writes_no_provider_config(
        self, tmp_path, monkeypatch
    ):
        client, journal = _make_empty_client(tmp_path, monkeypatch)
        client.get("/init")
        _commit_journal_identity()

        resp = client.post(
            "/init/finalize",
            json={},
            content_type="application/json",
        )

        assert resp.status_code == 200
        config = _read_config(journal)
        assert "env" not in config
        assert "providers" not in config

    def test_finalize_ignores_legacy_gemini_key_field(self, tmp_path, monkeypatch):
        client, journal = _make_empty_client(tmp_path, monkeypatch)
        client.get("/init")
        _commit_journal_identity()

        resp = client.post(
            "/init/finalize",
            json={"gemini_key": "SHOULD_NOT_PERSIST", "lane": "byo"},
            content_type="application/json",
        )

        assert resp.status_code == 200
        assert resp.get_json()["redirect"] == "/app/thinking/#byo-setup"
        config = _read_config(journal)
        assert "env" not in config
        assert "providers" not in config

    def test_finalize_form_timezone_overrides_os_default(self, tmp_path, monkeypatch):
        client, journal = _make_empty_client(tmp_path, monkeypatch)
        client.get("/init")
        _commit_journal_identity()

        resp = client.post(
            "/init/finalize",
            json={
                "name": "Form User",
                "preferred": "Form",
                "timezone": "America/New_York",
            },
            content_type="application/json",
        )

        assert resp.status_code == 200
        config = _read_config(journal)
        assert config["identity"]["name"] == "Form User"
        assert config["identity"]["preferred"] == "Form"
        assert config["identity"]["timezone"] == "America/New_York"
        assert "completed_at" in config["setup"]

    def test_finalize_without_timezone_preserves_os_default(
        self, tmp_path, monkeypatch
    ):
        client, journal = _make_empty_client(tmp_path, monkeypatch)
        client.get("/init")
        initial_timezone = _read_config(journal)["identity"]["timezone"]
        _commit_journal_identity()

        resp = client.post(
            "/init/finalize",
            json={"name": "Form User", "preferred": "Form"},
            content_type="application/json",
        )

        assert resp.status_code == 200
        config = _read_config(journal)
        assert config["identity"]["name"] == "Form User"
        assert config["identity"]["preferred"] == "Form"
        assert config["identity"]["timezone"] == initial_timezone
        assert "completed_at" in config["setup"]

    def test_finalize_completes_setup_access(self, fresh_client, journal_copy):
        _commit_journal_identity()
        response = fresh_client.post(
            "/init/finalize",
            json={},
            content_type="application/json",
        )
        assert response.get_json()["redirect"] == "/app/thinking/"
        resp = fresh_client.get("/")
        assert resp.status_code == 302
        assert resp.headers["Location"].endswith("/app/home/")

    def test_finalize_no_early_config_write(self, fresh_client, journal_copy):
        """Before finalize, config should have no setup."""
        config = _read_config(journal_copy)
        assert "setup" not in config or "completed_at" not in config.get("setup", {})

    def test_post_init_redirect(self, fresh_client, journal_copy):
        """After finalize, /init redirects away."""
        _commit_journal_identity()
        fresh_client.post(
            "/init/finalize",
            json={},
            content_type="application/json",
        )
        resp = fresh_client.get("/init")
        assert resp.status_code == 302

    def test_finalize_with_retention_config(self, fresh_client, journal_copy):
        """Finalize with explicit retention config writes correct values."""
        _commit_journal_identity()
        resp = fresh_client.post(
            "/init/finalize",
            json={
                "retention_mode": "processed",
                "retention_days": 30,
            },
            content_type="application/json",
        )
        assert resp.status_code == 200
        config = _read_config(journal_copy)
        assert config["retention"]["raw_media"] == "processed"
        assert config["retention"]["raw_media_days"] is None

    def test_finalize_default_retention(self, fresh_client, journal_copy):
        """Finalize without retention fields writes default (keep/null)."""
        _commit_journal_identity()
        resp = fresh_client.post(
            "/init/finalize",
            json={},
            content_type="application/json",
        )
        assert resp.status_code == 200
        config = _read_config(journal_copy)
        assert config["retention"]["raw_media"] == "keep"
        assert config["retention"]["raw_media_days"] is None

    def test_finalize_corrupt_config_returns_reason_without_writing(
        self, fresh_client, journal_copy
    ):
        _commit_journal_identity()
        config_path = journal_copy / "config" / "journal.json"
        config_path.write_bytes(b"{ invalid json }")
        before = config_path.read_bytes()

        resp = fresh_client.post(
            "/init/finalize",
            json={"name": "Jane"},
            content_type="application/json",
        )

        assert resp.status_code == 500
        assert resp.get_json()["reason_code"] == "corrupt_config"
        assert config_path.read_bytes() == before
