# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for health app routes."""

import os
from datetime import date

from solstone.convey.reasons import REPROCESS_ALREADY_COMPLETE

DAY = "20250115"
SEGMENT = "120000_300"


def _seed_reprocess_segment(journal, day=DAY):
    segment_dir = journal / "chronicle" / day / "default" / SEGMENT
    segment_dir.mkdir(parents=True)
    return segment_dir


def _touch_reprocess_marker(journal, day, name, ns):
    marker = journal / "chronicle" / day / "health" / name
    marker.parent.mkdir(parents=True, exist_ok=True)
    marker.touch()
    os.utime(marker, ns=(ns, ns))
    return marker


class TestLogRoute:
    """Tests for GET /app/health/api/log."""

    def test_valid_log_path(self, health_env):
        env = health_env()
        resp = env.client.get(
            "/app/health/api/log?path=20260322/health/1774196508583_transcribe.log"
        )
        assert resp.status_code == 200
        data = resp.get_json()
        assert data["path"] == "20260322/health/1774196508583_transcribe.log"
        assert "test log content" in data["content"]

    def test_path_traversal_rejected(self, health_env):
        env = health_env()
        resp = env.client.get("/app/health/api/log?path=../../../etc/passwd")
        assert resp.status_code == 400

    def test_non_log_extension_rejected(self, health_env):
        env = health_env()
        resp = env.client.get("/app/health/api/log?path=20260322/health/foo.txt")
        assert resp.status_code == 400

    def test_path_outside_health_dir_rejected(self, health_env):
        env = health_env()
        resp = env.client.get("/app/health/api/log?path=20260322/talents/something.log")
        assert resp.status_code == 400

    def test_missing_file_returns_404(self, health_env):
        env = health_env()
        resp = env.client.get(
            "/app/health/api/log?path=20260322/health/nonexistent.log"
        )
        assert resp.status_code == 404

    def test_missing_path_param_returns_400(self, health_env):
        env = health_env()
        resp = env.client.get("/app/health/api/log")
        assert resp.status_code == 400

    def test_encoded_traversal_rejected(self, health_env):
        env = health_env()
        resp = env.client.get(
            "/app/health/api/log?path=20260322/health/..%2F..%2Fetc%2Fpasswd.log"
        )
        assert resp.status_code == 400

    def test_null_byte_rejected(self, health_env):
        env = health_env()
        resp = env.client.get("/app/health/api/log?path=20260322/health/foo%00.log")
        assert resp.status_code == 400


class TestInfoRoute:
    def test_returns_hostname(self, health_env):
        env = health_env()
        response = env.client.get("/app/health/api/info")
        assert response.status_code == 200
        data = response.get_json()
        assert "hostname" in data
        assert isinstance(data["hostname"], str)
        assert len(data["hostname"]) > 0


class TestRestartObserverRoute:
    def test_restart_observer_emits_supervisor_restart(self, health_env, monkeypatch):
        env = health_env()
        calls = []

        def fake_send(tract, event, **fields):
            calls.append((tract, event, fields))
            return True

        monkeypatch.setattr("solstone.apps.health.routes.callosum_send", fake_send)

        response = env.client.post(
            "/app/health/api/restart-observer",
            json={"service": "sense"},
        )

        assert response.status_code == 200
        assert response.get_json() == {
            "status": "restart_requested",
            "service": "sense",
        }
        assert calls == [("supervisor", "restart", {"service": "sense"})]

    def test_restart_observer_missing_service_returns_400(self, health_env):
        env = health_env()

        response = env.client.post("/app/health/api/restart-observer", json={})

        assert response.status_code == 400
        assert response.get_json()["reason_code"] == "missing_required_field"

    def test_restart_observer_unknown_service_returns_400(self, health_env):
        env = health_env()

        response = env.client.post(
            "/app/health/api/restart-observer",
            json={"service": "convey"},
        )

        assert response.status_code == 400
        assert response.get_json()["reason_code"] == "invalid_request_value"

    def test_restart_observer_emit_failure_returns_503(self, health_env, monkeypatch):
        env = health_env()
        monkeypatch.setattr(
            "solstone.apps.health.routes.callosum_send",
            lambda *args, **kwargs: False,
        )

        response = env.client.post(
            "/app/health/api/restart-observer",
            json={"service": "sense"},
        )

        assert response.status_code == 503
        assert response.get_json()["reason_code"] == "observer_restart_failed"


class TestReprocessRoute:
    def test_reprocess_route_reuses_extracted_action_symbol(self):
        from solstone.apps.health import routes
        from solstone.think import reprocess

        assert routes.reprocess_day is reprocess.reprocess_day

    def test_reprocess_missing_day_returns_400(self, health_env):
        env = health_env()

        response = env.client.post(
            "/app/health/api/reprocess",
            json={"flavor": "process-now"},
        )

        assert response.status_code == 400
        assert response.get_json()["reason_code"] == "missing_required_field"

    def test_reprocess_bad_flavor_returns_400(self, health_env):
        env = health_env()

        response = env.client.post(
            "/app/health/api/reprocess",
            json={"day": DAY, "flavor": "redo"},
        )

        assert response.status_code == 400
        assert response.get_json()["reason_code"] == "invalid_request_value"

    def test_reprocess_process_now_queues_drain(self, health_env, monkeypatch):
        env = health_env()
        _seed_reprocess_segment(env.journal)
        _touch_reprocess_marker(env.journal, DAY, "stream.updated", 2_000_000_000)
        calls = []

        def fake_send(tract, event, **fields):
            calls.append((tract, event, fields))
            return True

        monkeypatch.setattr("solstone.think.reprocess.callosum_send", fake_send)

        response = env.client.post(
            "/app/health/api/reprocess",
            json={"day": DAY, "flavor": "process-now"},
        )

        assert response.status_code == 200
        assert response.get_json() == {"status": "queued", "day": DAY}
        assert calls == [("supervisor", "drain", {"day": DAY})]

    def test_reprocess_from_scratch_queues_request(self, health_env, monkeypatch):
        env = health_env()
        _seed_reprocess_segment(env.journal)
        _touch_reprocess_marker(env.journal, DAY, "stream.updated", 2_000_000_000)
        calls = []

        def fake_send(tract, event, **fields):
            calls.append((tract, event, fields))
            return True

        monkeypatch.setattr("solstone.think.reprocess.callosum_send", fake_send)

        response = env.client.post(
            "/app/health/api/reprocess",
            json={"day": DAY, "flavor": "from-scratch"},
        )

        assert response.status_code == 200
        assert response.get_json() == {"status": "queued", "day": DAY}
        assert calls == [
            (
                "supervisor",
                "request",
                {
                    "cmd": [
                        "journal",
                        "think",
                        "-v",
                        "--day",
                        DAY,
                        "--from-scratch",
                    ],
                    "day": DAY,
                },
            )
        ]

    def test_reprocess_already_complete_returns_success_payload(
        self, health_env, monkeypatch
    ):
        env = health_env()
        _seed_reprocess_segment(env.journal)
        _touch_reprocess_marker(env.journal, DAY, "stream.updated", 1_000_000_000)
        _touch_reprocess_marker(env.journal, DAY, "daily.updated", 2_000_000_000)
        calls = []
        monkeypatch.setattr(
            "solstone.think.reprocess.callosum_send",
            lambda tract, event, **fields: calls.append((tract, event, fields)) or True,
        )

        response = env.client.post(
            "/app/health/api/reprocess",
            json={"day": DAY, "flavor": "process-now"},
        )

        assert response.status_code == 200
        assert response.get_json() == {
            "status": "already_complete",
            "day": DAY,
            "message": REPROCESS_ALREADY_COMPLETE.message,
            "reason_code": REPROCESS_ALREADY_COMPLETE.code,
        }
        assert calls == []

    def test_reprocess_today_returns_past_only(self, health_env, monkeypatch):
        env = health_env()
        today = date.today().strftime("%Y%m%d")
        calls = []
        monkeypatch.setattr(
            "solstone.think.reprocess.callosum_send",
            lambda tract, event, **fields: calls.append((tract, event, fields)) or True,
        )

        response = env.client.post(
            "/app/health/api/reprocess",
            json={"day": today, "flavor": "process-now"},
        )

        assert response.status_code == 400
        assert response.get_json()["reason_code"] == "reprocess_past_only"
        assert calls == []

    def test_reprocess_unreachable_returns_503(self, health_env, monkeypatch):
        env = health_env()
        _seed_reprocess_segment(env.journal)
        _touch_reprocess_marker(env.journal, DAY, "stream.updated", 2_000_000_000)
        monkeypatch.setattr(
            "solstone.think.reprocess.callosum_send",
            lambda *args, **kwargs: False,
        )

        response = env.client.post(
            "/app/health/api/reprocess",
            json={"day": DAY, "flavor": "process-now"},
        )

        assert response.status_code == 503
        assert response.get_json()["reason_code"] == "reprocess_unreachable"

    def test_reprocess_malformed_day_returns_invalid_day(self, health_env, monkeypatch):
        env = health_env()
        calls = []
        monkeypatch.setattr(
            "solstone.think.reprocess.callosum_send",
            lambda tract, event, **fields: calls.append((tract, event, fields)) or True,
        )

        response = env.client.post(
            "/app/health/api/reprocess",
            json={"day": "20250230", "flavor": "process-now"},
        )

        assert response.status_code == 400
        assert response.get_json()["reason_code"] == "invalid_day"
        assert calls == []

    def test_reprocess_no_data_returns_invalid_day(self, health_env, monkeypatch):
        env = health_env()
        calls = []
        monkeypatch.setattr(
            "solstone.think.reprocess.callosum_send",
            lambda tract, event, **fields: calls.append((tract, event, fields)) or True,
        )

        response = env.client.post(
            "/app/health/api/reprocess",
            json={"day": DAY, "flavor": "process-now"},
        )

        assert response.status_code == 400
        assert response.get_json()["reason_code"] == "invalid_day"
        assert calls == []


class TestRetryImportRoute:
    def test_retry_import_missing_import_id_returns_400(self, health_env):
        env = health_env()

        response = env.client.post("/app/health/api/retry-import", json={})

        assert response.status_code == 400
        assert response.get_json()["reason_code"] == "missing_required_field"

    def test_retry_import_accepts_optional_stage_stub(self, health_env):
        env = health_env()

        response = env.client.post(
            "/app/health/api/retry-import",
            json={"import_id": "import-1", "stage": "transcribe"},
        )

        assert response.status_code == 501
        data = response.get_json()
        assert data["status"] == "not_implemented"
        assert "transcribe" in data["message"]
