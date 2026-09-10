# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
import copy
import os
import sys
import time
import unittest
from pathlib import Path

from tools.talent_fault_sim.runner import GOOD, run_bounded, verify, verify_publication


class OracleTests(unittest.TestCase):
    def test_replacement_identity_controls(self):
        self.assertEqual(verify_publication((1, 2), (1, 3), False), [])
        self.assertEqual(verify_publication((1, 2), (1, 2), True), [])
        self.assertEqual(
            verify_publication((1, 2), (1, 2), False), ["publication_not_replaced"]
        )
        self.assertEqual(
            verify_publication((1, 2), (1, 3), True),
            ["failed_publication_replaced_artifact"],
        )

    def setUp(self):
        self.journal = Path("/disposable/journal")
        self.events = [
            {
                "event": "generate_attempt",
                "ordinal": 0,
                "batch": None,
                "terminal": False,
                "status": "retry_eligible",
                "cause": "schema_validation_failed",
                "retry": True,
            },
            {
                "event": "generate_attempt",
                "ordinal": 1,
                "batch": None,
                "terminal": False,
                "status": "success",
                "cause": None,
                "retry": False,
            },
            {"event": "finish", "output": GOOD},
        ]
        self.calls = [{"contents": [{"type": "text", "text": "fixture phrase"}]}] * 2

    def check(self, events=None, calls=None, output=GOOD.encode(), reason=None):
        return verify(
            self.events if events is None else events,
            self.calls if calls is None else calls,
            "generate",
            2,
            reason,
            output,
            GOOD.encode(),
            "fixture phrase",
            self.journal,
        )

    def test_clean_and_independent_negative_controls(self):
        self.assertEqual(self.check(), [])
        self.assertIn("model_call_count", self.check(calls=self.calls[:1]))
        self.assertIn(
            "assembled_fixture_missing", self.check(calls=[{"contents": "wrong"}] * 2)
        )
        self.assertIn("terminal_count", self.check(events=self.events[:-1]))
        self.assertIn(
            "terminal_count", self.check(events=self.events + [self.events[-1]])
        )
        self.assertIn("artifact_bytes", self.check(output=b"truncated"))
        self.assertIn("artifact_bytes", self.check(output=GOOD.encode() + b"\r\n"))
        self.assertIn("attempt_evidence_count", self.check(events=self.events[1:]))
        events = copy.deepcopy(self.events)
        events[0]["cause"] = None
        self.assertIn("intermediate_failure_missing", self.check(events=events))
        events[-1] = {"event": "error", "terminal": True, "reason_code": "wrong"}
        self.assertIn(
            "terminal_cause",
            self.check(events=events, reason="schema_validation_failed"),
        )

    def test_attempt_identity_and_retry_negative_controls(self):
        for field, value, error in (
            ("ordinal", 7, "attempt_identity"),
            ("batch", 2, "attempt_identity"),
            ("terminal", True, "attempt_retry_decision"),
            ("retry", False, "attempt_retry_decision"),
            ("status", "success", "attempt_status"),
            ("cause", "incomplete_json_length", "intermediate_failure_missing"),
        ):
            with self.subTest(field=field):
                events = copy.deepcopy(self.events)
                events[0][field] = value
                self.assertIn(error, self.check(events=events))

    def test_cogitate_identity_and_premature_finish_controls(self):
        events = [
            {
                "event": "error",
                "terminal": True,
                "reason_code": "talent_stage_failed",
                "usage": {"input_tokens": 11, "output_tokens": 7},
            }
        ]
        calls = [{"journal_root": str(self.journal)}]
        args = ("cogitate", 1, "talent_stage_failed", b"old", b"old", "", self.journal)
        self.assertEqual(verify(events, calls, *args), [])
        missing_usage = copy.deepcopy(events)
        missing_usage[0].pop("usage")
        self.assertIn("terminal_usage", verify(missing_usage, calls, *args))
        self.assertIn(
            "wrong_child_journal", verify(events, [{"journal_root": "/wrong"}], *args)
        )
        self.assertIn(
            "terminal_count", verify([{"event": "finish"}] + events, calls, *args)
        )


@unittest.skipUnless(os.name == "posix", "Unix process groups required")
class ProcessBoundsTests(unittest.TestCase):
    def test_interruption_reaps_the_owned_worker(self):
        code = (
            "import os,signal,time; os.kill(os.getppid(),signal.SIGINT); time.sleep(30)"
        )
        start = time.monotonic()
        result = run_bounded([sys.executable, "-c", code], b"", os.environ.copy())
        self.assertEqual(result["failure"], "worker_interrupted")
        self.assertLess(time.monotonic() - start, 5)

    def test_output_limit_and_normal_completion(self):
        good = run_bounded(
            [sys.executable, "-c", "print('ok')"], b"", os.environ.copy()
        )
        self.assertEqual(good["stdout"], b"ok\n")
        self.assertIsNone(good["failure"])
        bad = run_bounded(
            [sys.executable, "-c", "import os; os.write(1,b'x'*8192)"],
            b"",
            os.environ.copy(),
            limits=(1024, 1024),
        )
        self.assertEqual(bad["failure"], "worker_output_limit")
        self.assertEqual(len(bad["stdout"]), 1024)

    def test_timeout_reaps_descendant_holding_pipe(self):
        code = (
            "import subprocess,time; subprocess.Popen(['sleep','30']); time.sleep(30)"
        )
        start = time.monotonic()
        result = run_bounded(
            [sys.executable, "-c", code], b"", os.environ.copy(), timeout=0.2
        )
        self.assertEqual(result["failure"], "worker_timeout")
        self.assertLess(time.monotonic() - start, 5)


if __name__ == "__main__":
    unittest.main()
