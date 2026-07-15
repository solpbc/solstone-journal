# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for health app routes."""

import json
import os
import time
from datetime import date

from solstone.apps.health import routes as health_routes
from solstone.convey import backlog_copy
from solstone.convey.reasons import REPROCESS_ALREADY_COMPLETE
from solstone.think.talent_runs import AgentFailure, AgentFailureScan

DAY = "20250115"
SEGMENT = "120000_300"


def test_errors_today_label_pluralizes_count():
    assert health_routes._errors_today_label(1) == "error today"
    assert health_routes._errors_today_label(0) == "errors today"
    assert health_routes._errors_today_label(2) == "errors today"
    assert health_routes._errors_today_label(None) == "errors today"


def _readiness_snapshot(severity: str = "neutral") -> dict:
    return {
        "summary": {
            "status": "unknown" if severity == "neutral" else "blocked",
            "severity": severity,
            "active_groups": 1 if severity in {"blocker", "attention"} else 0,
            "blocked_count": 1 if severity == "blocker" else 0,
        },
        "interfaces": {},
        "groups": [
            {
                "semantic_key": "provider_key_missing:anthropic:",
                "work_key": None,
                "status": "blocked",
                "severity": severity,
                "reason_code": "provider_key_missing",
                "provider": "anthropic",
                "model": None,
                "context": None,
                "interface": "generate",
                "summary": "Anthropic needs credentials before it can read your screen descriptions",
                "detail": "Open provider setup.",
                "recovery_action": {
                    "label": "Open Thinking",
                    "href": "/app/thinking/#main",
                },
                "operator_detail": "reason_code=provider_key_missing provider=anthropic",
            }
        ]
        if severity in {"blocker", "attention"}
        else [],
    }


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
    def test_build_agent_error_seed_shape(self):
        from solstone.apps.health.routes import _build_agent_error_seed

        scan = AgentFailureScan(
            [
                AgentFailure(
                    use_id="agent-1",
                    name="flow",
                    ts=1770000000000,
                    reason_code="provider_key_missing",
                    provider="anthropic",
                    model="claude-test",
                )
            ],
            ok=True,
        )

        assert _build_agent_error_seed(scan) == [
            {
                "type": "agent",
                "id": "agent-1",
                "name": "flow",
                "ts": 1770000000000,
                "service": "cortex",
                "error": "talent error",
                "reason_code": "provider_key_missing",
                "provider": "anthropic",
                "model": "claude-test",
            }
        ]

    def test_returns_hostname_and_readiness(self, health_env, monkeypatch):
        snapshot = _readiness_snapshot("blocker")
        monkeypatch.setattr(
            "solstone.apps.health.routes.build_readiness_snapshot",
            lambda: snapshot,
        )
        env = health_env()
        response = env.client.get("/app/health/api/info")
        assert response.status_code == 200
        data = response.get_json()
        assert "hostname" in data
        assert isinstance(data["hostname"], str)
        assert len(data["hostname"]) > 0
        assert data["readiness"] == snapshot

    def test_info_readiness_degrades_when_snapshot_raises(
        self, health_env, monkeypatch
    ):
        monkeypatch.setattr(
            "solstone.apps.health.routes.build_readiness_snapshot",
            lambda: (_ for _ in ()).throw(RuntimeError("boom")),
        )
        env = health_env()

        response = env.client.get("/app/health/api/info")

        assert response.status_code == 200
        readiness = response.get_json()["readiness"]
        assert readiness["unavailable"] is True
        assert readiness["summary"]["severity"] == "neutral"

    def test_index_serves_shell_and_info_returns_readiness(
        self, health_env, monkeypatch
    ):
        snapshot = _readiness_snapshot("blocker")
        monkeypatch.setattr(
            "solstone.apps.health.routes.build_readiness_snapshot",
            lambda: snapshot,
        )
        env = health_env()

        index_response = env.client.get("/app/health/")
        info_response = env.client.get("/app/health/api/info")

        assert index_response.status_code == 200
        assert b'data-solstone-shell="spa"' in index_response.data
        assert info_response.status_code == 200
        assert info_response.get_json()["readiness"] == snapshot

    def test_state_returns_agent_error_seed(self, health_env):
        env = health_env()
        today = date.today().strftime("%Y%m%d")
        now_ms = int(time.time() * 1000)
        talents = env.journal / "talents"
        talents.mkdir()
        (talents / f"{today}.jsonl").write_text(
            json.dumps(
                {
                    "use_id": "agent-1",
                    "name": "flow",
                    "day": today,
                    "ts": now_ms,
                    "status": "error",
                    "reason_code": "provider_key_missing",
                    "provider": "anthropic",
                    "model": "claude-test",
                }
            )
            + "\n",
            encoding="utf-8",
        )

        response = env.client.get("/app/health/api/state")

        assert response.status_code == 200
        data = response.get_json()
        assert set(data) == {"backlog", "agent_errors"}
        assert "readiness" not in data
        assert data["agent_errors"] == {
            "items": [
                {
                    "type": "agent",
                    "id": "agent-1",
                    "name": "flow",
                    "ts": now_ms,
                    "service": "cortex",
                    "error": "talent error",
                    "reason_code": "provider_key_missing",
                    "provider": "anthropic",
                    "model": "claude-test",
                }
            ],
            "ok": True,
            "count": 1,
            "label": "error today",
        }
        assert data["backlog"]["copy"] == {
            "bucket_heading": backlog_copy.BACKLOG_BUCKET_HEADING,
            "bucket_description": backlog_copy.BACKLOG_BUCKET_DESCRIPTION,
            "day_badge": backlog_copy.BACKLOG_DAY_BADGE,
            "action_process_now": backlog_copy.BACKLOG_ACTION_PROCESS_NOW,
            "action_redo_scratch": backlog_copy.BACKLOG_ACTION_REDO_SCRATCH,
            "confirm_redo_scratch": backlog_copy.BACKLOG_CONFIRM_REDO_SCRATCH,
            "queued_feedback": backlog_copy.BACKLOG_QUEUED_FEEDBACK,
        }

    def test_state_agent_error_scan_degraded(self, health_env, monkeypatch):
        monkeypatch.setattr(
            "solstone.apps.health.routes.read_unresolved_agent_failures",
            lambda: AgentFailureScan([], ok=False),
        )
        env = health_env()

        response = env.client.get("/app/health/api/state")

        assert response.status_code == 200
        data = response.get_json()["agent_errors"]
        assert data == {
            "items": [],
            "ok": False,
            "count": 0,
            "label": "errors today",
        }

    def test_index_stays_shell_when_readiness_snapshot_raises(
        self, health_env, monkeypatch
    ):
        monkeypatch.setattr(
            "solstone.apps.health.routes.build_readiness_snapshot",
            lambda: (_ for _ in ()).throw(RuntimeError("boom")),
        )
        env = health_env()

        index_response = env.client.get("/app/health/")
        info_response = env.client.get("/app/health/api/info")

        assert index_response.status_code == 200
        assert b'data-solstone-shell="spa"' in index_response.data
        readiness = info_response.get_json()["readiness"]
        assert readiness["unavailable"] is True
        assert readiness["summary"]["severity"] == "neutral"


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
