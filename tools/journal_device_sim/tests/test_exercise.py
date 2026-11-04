# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
import unittest
from unittest.mock import patch

from tools.journal_device_sim.exercise import DeviceExercise, ExerciseFailure
from tools.journal_device_sim.http_client import BridgeHttpClient, HttpRequestError, HttpResponse


class EventResponse:
    status = 200

    def __init__(self, chunks):
        self.chunks = iter(chunks)

    def getheader(self, name):
        return "text/event-stream" if name == "Content-Type" else None

    def read1(self, _size):
        return next(self.chunks, b"")


class EventConnection:
    def __init__(self, chunks):
        self.response = EventResponse(chunks)
        self.sock = self
        self.closed = False

    def connect(self):
        pass

    def settimeout(self, value):
        assert value > 0

    def request(self, *_args, **_kwargs):
        pass

    def getresponse(self):
        return self.response

    def close(self):
        self.closed = True


class ExerciseTests(unittest.TestCase):
    def test_sse_requires_connected_data_across_wire_chunks(self):
        client = BridgeHttpClient("http://127.0.0.1:6200", timeout=1)
        wire = b': heartbeat\n\nevent: continuity\ndata: {"state":"connecting"}\n\nevent: continuity\ndata: {"state":"connected","generation":1}\n\n'
        connection = EventConnection([wire[:63], wire[63:102], wire[102:]])
        with patch.object(client, "_connection", return_value=connection):
            result = client.check_events()
        self.assertEqual(result["state"], "connected")
        self.assertEqual(result["bytes"], len(wire))
        self.assertNotIn("data", result)
        self.assertTrue(connection.closed)

    def test_heartbeat_unavailable_or_truncated_event_never_passes(self):
        for wire in (b": heartbeat\n\n", b'event: continuity\ndata: {"state":"unavailable"}\n\n',
                     b'event: continuity\ndata: {"state":"stopped"}\n\n',
                     b'event: continuity\ndata: {"state":"connected"}\n'):
            with self.subTest(wire=wire):
                client = BridgeHttpClient("http://127.0.0.1:6200", timeout=1)
                connection = EventConnection([wire])
                with patch.object(client, "_connection", return_value=connection):
                    with self.assertRaises(HttpRequestError):
                        client.check_events()
                self.assertTrue(connection.closed)

    def exercise(self, reason="tls-access-denied", state="unpaired"):
        class Bridge:
            provenance = {"credentials": {"client_cid": "sha256:test"}}

            def run_cli(self, _args):
                return 1, b"", b"lost self-unpair response", None

        class Client:
            revoked = False

            def get_json(self, path):
                if path.endswith("/identity"):
                    return HttpResponse(502, {}) if self.revoked else HttpResponse(200, {"committed": True, "instance_id": "test-journal"})
                if path.endswith("/clients"):
                    return HttpResponse(200, {"clients": [{"cid": "sha256:test"}]})
                return HttpResponse(200, {"state": state, "manager_alive": True,
                                          "instance_id": "test-journal", "terminal_reason": reason,
                                          "last_failure": None})

        bridge, client = Bridge(), Client()
        original = bridge.run_cli
        def revoke(args):
            client.revoked = True
            return original(args)
        bridge.run_cli = revoke
        return DeviceExercise(bridge, client, instance_id="test-journal", cid="sha256:test",
                              timeout=0, poll_interval=0, receipts=[], progress=lambda *_a, **_kw: None)

    def test_lost_unpair_response_is_reconciled_by_exact_native_denial(self):
        exercise = self.exercise()
        result = exercise.revoke()
        self.assertEqual(result["terminal_reason"], "tls-access-denied")
        self.assertEqual(result["cli_exit_code"], 1)

    def test_generic_refusals_and_exhausted_retries_cannot_prove_revocation(self):
        for reason, state in (("tls-certificate-unknown", "unpaired"),
                              ("refusals-exhausted", "unpaired"),
                              ("tls-access-denied", "connected"), (None, "unpaired")):
            with self.subTest(reason=reason, state=state):
                with self.assertRaises(HttpRequestError):
                    self.exercise(reason, state).revoke()

    def test_reconnected_upstream_identity_overrides_saved_provenance(self):
        exercise = self.exercise()
        exercise.bridge.stop = lambda: None
        exercise.bridge.start = lambda: "http://127.0.0.1:6300"
        wrong = type("WrongJournal", (), {"get_json": lambda *_: HttpResponse(200, {"committed": True, "instance_id": "other"})})()
        with patch("tools.journal_device_sim.exercise.BridgeHttpClient", return_value=wrong):
            with self.assertRaises(ExerciseFailure):
                exercise.reconnect()

    def test_failed_cli_preserves_safe_receipt_and_incomplete_step(self):
        exercise = self.exercise()
        with self.assertRaises(ExerciseFailure):
            exercise.step("native_cli", lambda: exercise.cli(["call", "link", "status"]))
        result = exercise.receipts[0]
        self.assertEqual(result["result"], "INCOMPLETE")
        self.assertEqual(result["exit_code"], 1)
        self.assertNotIn("lost self-unpair response", json.dumps(result))


if __name__ == "__main__":
    unittest.main()
