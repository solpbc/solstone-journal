# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Unit tests for the bootstrap response classifier and leftover MLX detection."""

import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from tools.local_thinking_install.fixtures import write_prior_mlx_status
from tools.local_thinking_install.harness import (
    RUNTIME_TERMINAL_PHASES,
    MODEL_TOTAL_BYTES,
    PrerequisiteError,
    classify_bootstrap_response,
    find_or_build_helper,
    is_leftover_mlx,
    run_post_admit_checks,
    validate_direct_run_dir,
    wait_until_mlx_replaced,
)
from tools.local_thinking_install.portal import HttpResponse


class TestBootstrapClassifier(unittest.TestCase):
    def test_direct_run_dir_rejects_overlong_socket_path(self) -> None:
        with self.assertRaises(PrerequisiteError):
            validate_direct_run_dir(Path("/var/tmp") / ("x" * 100))

    def test_service_install_only_reuses_terminal_install_checks(self) -> None:
        responses = [
            HttpResponse(200, {}, "", {
                "install_state": "downloading",
                "progress_bytes_received": 1,
                "progress_bytes_total": MODEL_TOTAL_BYTES,
            }, None),
            HttpResponse(200, {}, "", {
                "install_state": "downloading",
                "progress_bytes_received": MODEL_TOTAL_BYTES,
                "progress_bytes_total": MODEL_TOTAL_BYTES,
            }, None),
            HttpResponse(200, {}, "", {"install_state": "installed"}, None),
            HttpResponse(200, {}, "", {"install_state": "installed"}, None),
        ]
        with tempfile.TemporaryDirectory() as tmp_dir, patch(
            "tools.local_thinking_install.harness.send_http_request",
            side_effect=responses,
        ), patch(
            "tools.local_thinking_install.harness.time.sleep",
            return_value=None,
        ):
            root = Path(tmp_dir)
            outcome, answer, evidence = run_post_admit_checks(
                port=5015,
                staged_core_bin=root / "solstone-core",
                journal_dir=root / "journal",
                case_name="fresh",
                case_dir=root,
                helper_bin=None,
                install_timeout_seconds=1.0,
                install_only=True,
            )
            self.assertEqual(outcome, "passed_install_only")
            self.assertIsNone(answer)
            self.assertEqual(evidence, "service_context_install_only")
            self.assertTrue((root / "final-install-status.json").is_file())

    def test_terminal_runtime_phases_are_explicit(self) -> None:
        self.assertIn("artifact-not-ready", RUNTIME_TERMINAL_PHASES)
        self.assertIn("host-blocked", RUNTIME_TERMINAL_PHASES)
        self.assertIn("failed", RUNTIME_TERMINAL_PHASES)
        self.assertNotIn("observing", RUNTIME_TERMINAL_PHASES)
        self.assertNotIn("stopped", RUNTIME_TERMINAL_PHASES)
        self.assertNotIn("warming", RUNTIME_TERMINAL_PHASES)

    def test_500_spawn_unavailable_refusal(self) -> None:
        detail_msg = (
            "local install can't be started from this build yet - use `journal` "
            "on this machine, or check back after an update"
        )
        resp = HttpResponse(
            status=500,
            headers={"content-type": "application/json"},
            body=f'{{"reason_code":"settings_operation_failed","error":"those settings couldn\'t be saved.","detail":"{detail_msg}"}}',
            json_data={
                "reason_code": "settings_operation_failed",
                "error": "those settings couldn't be saved.",
                "detail": detail_msg,
            },
            error_detail=detail_msg,
        )
        classification, note = classify_bootstrap_response(resp, "fresh")
        self.assertEqual(classification, "bootstrap_refused")
        self.assertIn("baseline refusal", note)

    def test_302_redirect_is_harness_infra(self) -> None:
        resp = HttpResponse(
            status=302,
            headers={"location": "/init"},
            body="",
            json_data=None,
            error_detail=None,
        )
        classification, note = classify_bootstrap_response(resp, "fresh")
        self.assertEqual(classification, "harness_infra")
        self.assertIn("session gate not established", note)

    def test_200_resolving_is_admitted(self) -> None:
        resp = HttpResponse(
            status=200,
            headers={"content-type": "application/json"},
            body='{"install_state":"resolving","attempt_id":"018e4f1a2b3c"}',
            json_data={"install_state": "resolving", "attempt_id": "018e4f1a2b3c"},
            error_detail=None,
        )
        classification, note = classify_bootstrap_response(resp, "fresh")
        self.assertEqual(classification, "admitted")
        self.assertIn("resolving", note)

    def test_200_downloading_is_admitted_at_http_layer(self) -> None:
        resp = HttpResponse(
            status=200,
            headers={"content-type": "application/json"},
            body='{"install_state":"downloading","attempt_id":"018e4f1a2b3c"}',
            json_data={"install_state": "downloading", "attempt_id": "018e4f1a2b3c"},
            error_detail=None,
        )
        classification, note = classify_bootstrap_response(resp, "prior_mlx")
        self.assertEqual(classification, "admitted")
        self.assertIn("downloading", note)

    def test_leftover_mlx_status_detection(self) -> None:
        helper_bin = find_or_build_helper()
        with tempfile.TemporaryDirectory() as tmp_dir:
            journal_dir = Path(tmp_dir)
            write_prior_mlx_status(journal_dir)
            self.assertTrue(is_leftover_mlx(helper_bin, journal_dir))
            # Test that wait_until_mlx_replaced returns False when MLX status is never replaced
            self.assertFalse(wait_until_mlx_replaced(helper_bin, journal_dir, timeout_seconds=0.1, poll_interval=0.02))

            # Simulate overwrite with native target
            status_file = journal_dir / "health" / "providers" / "local.json"
            status_file.write_text(
                '{"schema_version":1,"provider":"local","revision":2,"install_state":"installed","attempt_id":"018e4f1a2b3c4d5e0000000000000002","target_fingerprint_sha256":"native-metal-sha","target_fingerprint_json":"{\\"provider\\":\\"local\\",\\"runtime\\":\\"metal\\"}","started_at":null,"last_transition_at":null,"last_progress_at":null,"completed_at":null,"progress_bytes_received":null,"progress_bytes_total":null,"install_error":null,"error_code":null,"owner":null}',
                encoding="utf-8",
            )
            self.assertFalse(is_leftover_mlx(helper_bin, journal_dir))

    def test_missing_status_is_leftover_mlx(self) -> None:
        helper_bin = find_or_build_helper()
        with tempfile.TemporaryDirectory() as tmp_dir:
            journal_dir = Path(tmp_dir)
            # Empty journal directory without health/providers/local.json
            self.assertTrue(is_leftover_mlx(helper_bin, journal_dir))

    def test_500_generic_error_is_unexpected(self) -> None:
        resp = HttpResponse(
            status=500,
            headers={"content-type": "application/json"},
            body='{"error":"internal database failure"}',
            json_data={"error": "internal database failure"},
            error_detail="internal database failure",
        )
        classification, note = classify_bootstrap_response(resp, "fresh")
        self.assertEqual(classification, "unexpected")

    def test_connection_error_is_harness_infra(self) -> None:
        resp = HttpResponse(
            status=0,
            headers={},
            body="<urlopen error [Errno 111] Connection refused>",
            json_data=None,
            error_detail="[Errno 111] Connection refused",
        )
        classification, note = classify_bootstrap_response(resp, "fresh")
        self.assertEqual(classification, "harness_infra")


if __name__ == "__main__":
    unittest.main()
