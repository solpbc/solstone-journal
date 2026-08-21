# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import hashlib
import json
import os
import shutil
import threading
import unittest
from dataclasses import replace
from email.parser import BytesParser
from email.policy import default
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from tempfile import TemporaryDirectory
from typing import Any, Callable
from urllib.parse import parse_qs, urlsplit
from unittest.mock import patch

from tools.journal_device_sim.http_client import BridgeHttpClient
from tools.journal_device_sim.manifest import (
    FixtureProfile,
    ManifestError,
    ProcessingExpectation,
    load_manifest,
)
from tools.journal_device_sim.process import LinkProcessError
from tools.journal_device_sim.runner import (
    RunOutcome,
    SimulationFailure,
    Simulator,
    SimulatorConfig,
    _atomic_json,
)

TEST_CLIENT_CID = "sha256:" + "a" * 64


class FakeIngestState:
    def __init__(self) -> None:
        self.lock = threading.Lock()
        self.items: list[dict[str, Any]] = []
        self.posts = 0
        self.fail_first_after_store = False
        self.instance_id = "fake-journal-instance"
        self.status_instance_id: str | None = None
        self.posture = "direct"
        self.status_extra: dict[str, Any] = {}
        self.observed: object = False
        self.listing_total_delta = 0
        self.listing_http_status = 200
        self.listing_file_status = "present"
        self.post_http_status = 200
        self.identity_raw_body: bytes | None = None
        self.post_raw_body: bytes | None = None
        self.day_manifest_missing = False
        self.root_manifest_missing = False
        self.local_manager_alive = True
        self.response_mutator: Callable[[dict[str, Any]], None] | None = None


def handler_for(state: FakeIngestState) -> type[BaseHTTPRequestHandler]:
    class Handler(BaseHTTPRequestHandler):
        def log_message(self, _format: str, *_args: object) -> None:
            return

        def _json(self, status: int, value: dict[str, Any]) -> None:
            payload = json.dumps(value).encode("utf-8")
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)

        def _raw(self, status: int, payload: bytes) -> None:
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)

        def do_GET(self) -> None:
            parsed = urlsplit(self.path)
            if parsed.path == "/app/link/api/identity":
                if state.identity_raw_body is not None:
                    self._raw(200, state.identity_raw_body)
                    return
                self._json(
                    200,
                    {
                        "committed": True,
                        "instance_id": state.instance_id,
                        "mark": None,
                    },
                )
                return
            if parsed.path == "/app/link/api/status":
                self._json(
                    200,
                    {
                        "instance_id": state.status_instance_id or state.instance_id,
                        "posture": state.posture,
                        **state.status_extra,
                    },
                )
                return
            if parsed.path == "/_solstone/link/status":
                self._json(200, {"manager_alive": state.local_manager_alive})
                return
            source = parse_qs(parsed.query).get("source", [""])[0]
            if parsed.path.startswith("/app/devices/ingest/segments/"):
                day = parsed.path.rsplit("/", 1)[-1]
                with state.lock:
                    items = [
                        item["listing"]
                        for item in state.items
                        if item["day"] == day and item["source"] == source
                    ]
                self._json(
                    state.listing_http_status,
                    {
                        "protocol_version": 3,
                        "total": len(items) + state.listing_total_delta,
                        "items": items,
                    },
                )
                return
            if parsed.path.startswith("/app/devices/ingest/manifest/"):
                if state.day_manifest_missing:
                    self._json(404, {"reason_code": "not_found"})
                    return
                day = parsed.path.rsplit("/", 1)[-1]
                with state.lock:
                    segments = {
                        item["listing"]["key"]: item["listing"]
                        for item in state.items
                        if item["day"] == day and item["source"] == source
                    }
                self._json(200, {"version": 1, "day": day, "segments": segments})
                return
            if parsed.path == "/app/devices/ingest/manifest":
                if state.root_manifest_missing:
                    self._json(404, {"reason_code": "not_found"})
                    return
                with state.lock:
                    days = sorted(
                        {
                            item["day"]
                            for item in state.items
                            if item["source"] == source
                        }
                    )
                    summaries = {
                        day: {
                            "segments": sum(
                                1
                                for item in state.items
                                if item["day"] == day and item["source"] == source
                            )
                        }
                        for day in days
                    }
                self._json(200, {"days": summaries})
                return
            else:
                self._json(404, {"reason_code": "not_found"})
                return

        def do_POST(self) -> None:
            if self.path != "/app/devices/ingest":
                self._json(404, {"reason_code": "not_found"})
                return
            if self.headers.get("X-Solstone-Protocol-Version") != "3":
                self._json(426, {"reason_code": "protocol_version_legacy"})
                return
            length = int(self.headers["Content-Length"])
            body = self.rfile.read(length)
            message = BytesParser(policy=default).parsebytes(
                (
                    f"Content-Type: {self.headers['Content-Type']}\r\n"
                    "MIME-Version: 1.0\r\n\r\n"
                ).encode("ascii")
                + body
            )
            envelope: dict[str, Any] | None = None
            files: list[dict[str, Any]] = []
            for part in message.iter_parts():
                name = part.get_param("name", header="content-disposition")
                payload = part.get_payload(decode=True)
                if name == "envelope":
                    envelope = json.loads(payload)
                elif name == "files":
                    submitted = part.get_filename()
                    files.append(
                        {
                            "name": submitted,
                            "submitted_name": submitted,
                            "size": len(payload),
                            "sha256": hashlib.sha256(payload).hexdigest(),
                            "status": state.listing_file_status,
                        }
                    )
            assert envelope is not None
            requested = envelope["segment"]
            day = envelope["day"]
            source = envelope.get("source", "")
            file_meta = {
                item["submitted"]: item for item in envelope.get("files", [])
            }

            def response_descriptors(disposition: str) -> list[dict[str, Any]]:
                return [
                    {
                        **file_meta[item["submitted_name"]],
                        "written": item["submitted_name"],
                        "size": item["size"],
                        "sha256": item["sha256"],
                        "disposition": disposition,
                    }
                    for item in files
                ]

            with state.lock:
                state.posts += 1
                exact = next(
                    (
                        item
                        for item in state.items
                        if item["day"] == day
                        and item["source"] == source
                        and item["listing"]["files"] == files
                    ),
                    None,
                )
                if exact is not None:
                    response = {
                        "status": "duplicate",
                        "existing_segment": exact["listing"]["key"],
                        "message": "segment content already held",
                        "meta": envelope["meta"],
                        "file_descriptors": response_descriptors("already_held"),
                    }
                else:
                    same_key = any(
                        item["day"] == day
                        and item["source"] == source
                        and item["listing"]["key"] == requested
                        for item in state.items
                    )
                    landed = requested if not same_key else "080000_31"
                    listing = {
                        "key": landed,
                        "observed": state.observed,
                        "files": files,
                    }
                    if same_key:
                        listing["original_key"] = requested
                    state.items.append(
                        {"day": day, "source": source, "listing": listing}
                    )
                    response = {
                        "status": "collision" if same_key else "ok",
                        "segment": landed,
                        "bytes": sum(item["size"] for item in files),
                        "files": [item["submitted_name"] for item in files],
                        "meta": envelope["meta"],
                        "file_descriptors": response_descriptors("written"),
                    }
                    if same_key:
                        response["segment_original"] = requested
                if state.response_mutator is not None:
                    state.response_mutator(response)
                should_fail = state.fail_first_after_store and state.posts == 1
            if should_fail:
                self._json(500, {"status": "failed", "reason_code": "notify_failed"})
            elif state.post_raw_body is not None:
                self._raw(200, state.post_raw_body)
            else:
                self._json(state.post_http_status, response)

    return Handler


class FakeServer:
    def __init__(self, state: FakeIngestState) -> None:
        self.server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(state))
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)

    def __enter__(self) -> str:
        self.thread.start()
        return f"http://127.0.0.1:{self.server.server_port}"

    def __exit__(self, _type: object, _value: object, _traceback: object) -> None:
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=5)


class RunnerTests(unittest.TestCase):
    @staticmethod
    def bundled_manifest() -> Path:
        return (
            Path(__file__).resolve().parents[1] / "fixtures" / "smoke" / "manifest.json"
        )

    def _config(
        self, temporary: str, bridge_url: str, manifest_path: Path | None = None
    ) -> SimulatorConfig:
        root = Path(temporary)
        return SimulatorConfig(
            manifest=load_manifest(manifest_path or self.bundled_manifest()),
            profile="smoke",
            carrier="direct",
            state_dir=root / "state",
            evidence_path=root / "evidence.json",
            bridge_url=bridge_url,
            date_mode="preserve",
            request_timeout=5,
            max_attempts=2,
            expected_cid=TEST_CLIENT_CID,
        )

    @staticmethod
    def _write_journal_identity(root: Path, instance_id: str) -> None:
        state_path = root / "link" / "ca" / "state.json"
        state_path.parent.mkdir(parents=True, exist_ok=True)
        state_path.write_text(
            json.dumps({"instance_id": instance_id}) + "\n", encoding="utf-8"
        )

    @staticmethod
    def _write_journal_segment(
        simulator: Simulator,
        segment_id: str,
        *,
        processing_rows: list[dict[str, Any]] | None = None,
    ) -> Path:
        assert simulator.journal_root is not None
        segment = simulator.config.manifest.segments[segment_id]
        day = simulator.day_map[segment.day]
        stream = segment.source or "device"
        target = simulator.journal_root / "chronicle" / day / stream / segment.segment
        target.mkdir(parents=True, exist_ok=True)
        (target / "ingest.json").write_text(
            json.dumps(
                {
                    "schema_version": 1,
                    "requested_segment": segment.segment,
                    "files": {
                        item.submitted: {
                            "sha256": item.sha256,
                            "size": item.size,
                        }
                        for item in segment.files
                    },
                }
            )
            + "\n",
            encoding="utf-8",
        )
        (target / "stream.json").write_text(
            json.dumps(
                {
                    "stream": stream,
                    "seq": 1,
                    "prev_day": None,
                    "prev_segment": None,
                }
            )
            + "\n",
            encoding="utf-8",
        )
        event = {
            "record_type": "device_ingest",
            "record_version": 1,
            "protocol_version": 3,
            "outcome": "accepted",
            "cid": TEST_CLIENT_CID,
            "source": segment.source,
            "stream": stream,
            "day": day,
            "segment": segment.segment,
            "meta": simulator._envelope(segment, day)["meta"],
            "files": [
                {
                    **item.metadata,
                    "submitted": item.submitted,
                    "written": item.submitted,
                    "size": item.size,
                    "sha256": item.sha256,
                    "disposition": "written",
                }
                for item in segment.files
            ],
        }
        (target / "events.jsonl").write_text(
            json.dumps(event) + "\n", encoding="utf-8"
        )
        for item in segment.files:
            (target / item.submitted).write_bytes(item.path.read_bytes())
        if processing_rows is not None:
            expectation = segment.expectation.processing[0]
            (target / expectation.output).write_text(
                "".join(json.dumps(row) + "\n" for row in processing_rows),
                encoding="utf-8",
            )
        return target

    def _processing_simulator(
        self,
        temporary: str,
        bridge_url: str,
        state: FakeIngestState,
        *,
        handler: str = "transcribe",
    ) -> tuple[SimulatorConfig, Simulator, bytes]:
        root = Path(temporary)
        fixture_root = root / "processing-fixture"
        fixture_root.mkdir()
        if handler == "transcribe":
            submitted = "sample.wav"
            output = "sample.jsonl"
            payload = b"RIFF-test-audio"
            source = "audio-wav"
        else:
            submitted = "sample.mp4"
            output = "sample.jsonl"
            payload = b"screen-test-video"
            source = "screen-mp4"
        fixture = fixture_root / submitted
        fixture.write_bytes(payload)
        manifest_path = fixture_root / "manifest.json"
        manifest_path.write_text(
            json.dumps(
                {
                    "schema": "solstone.journal-device-sim.fixtures.v1",
                    "profiles": {
                        "smoke": {
                            "segments": ["media"],
                            "verify_duplicate": False,
                            "verification": "processing",
                        }
                    },
                    "segments": [
                        {
                            "id": "media",
                            "day": "20260201",
                            "segment": "080000_15",
                            "source": source,
                            "files": [
                                {
                                    "path": submitted,
                                    "submitted": submitted,
                                    "size": len(payload),
                                    "sha256": hashlib.sha256(payload).hexdigest(),
                                }
                            ],
                            "expect": {
                                "processing": [
                                    {
                                        "input": submitted,
                                        "output": output,
                                        "handler": handler,
                                    }
                                ]
                            },
                        }
                    ],
                }
            )
            + "\n",
            encoding="utf-8",
        )
        journal_root = root / "processing-journal"
        self._write_journal_identity(journal_root, state.instance_id)
        config = replace(
            self._config(temporary, bridge_url, manifest_path),
            journal_root=journal_root,
        )
        return config, Simulator(config), payload

    def test_smoke_proves_ok_collision_duplicate_and_resume(self) -> None:
        state = FakeIngestState()
        with TemporaryDirectory() as temporary, FakeServer(state) as bridge_url:
            config = self._config(temporary, bridge_url)
            self.assertEqual(Simulator(config).run(), RunOutcome.PASS)
            self.assertEqual(state.posts, 4)
            evidence = json.loads(config.evidence_path.read_text(encoding="utf-8"))
            self.assertEqual(evidence["result"], "PASS")
            self.assertEqual(
                [item["response"]["status"] for item in evidence["segments"]],
                ["ok", "collision"],
            )
            self.assertTrue(
                all("duplicate_response" in item for item in evidence["segments"])
            )
            self.assertEqual(evidence["request_count"], 4)
            self.assertTrue(
                all(item["upload_attempts"] == 1 for item in evidence["segments"])
            )
            self.assertTrue(
                all(item["duplicate_attempts"] == 1 for item in evidence["segments"])
            )
            self.assertTrue(
                all(item["request_count"] == 2 for item in evidence["segments"])
            )
            self.assertTrue(
                all(
                    item["lifetime_request_count"] == 2
                    for item in evidence["segments"]
                )
            )
            self.assertEqual(Simulator(config).run(), RunOutcome.PASS)
            self.assertEqual(state.posts, 4)
            resumed_evidence = json.loads(
                config.evidence_path.read_text(encoding="utf-8")
            )
            self.assertEqual(resumed_evidence["request_count"], 0)
            self.assertTrue(
                all(
                    item["request_count"] == 0
                    for item in resumed_evidence["segments"]
                )
            )
            self.assertTrue(
                all(
                    item["lifetime_request_count"] == 2
                    for item in resumed_evidence["segments"]
                )
            )
            self.assertTrue(resumed_evidence["contract_reads"])
            self.assertFalse(
                resumed_evidence["contract_reads"][0]["segments"]["body"][
                    "items"
                ][0]["observed"]
            )

    def test_receiver_status_identity_and_posture_gate_uploads(self) -> None:
        cases = (
            ("identity", "another-instance", "direct", "different journals"),
            ("posture", None, "spl", "posture is not direct"),
        )
        for name, status_instance, posture, expected in cases:
            with self.subTest(name=name), TemporaryDirectory() as temporary:
                state = FakeIngestState()
                state.status_instance_id = status_instance
                state.posture = posture
                with FakeServer(state) as bridge_url:
                    config = self._config(temporary, bridge_url)
                    self.assertEqual(Simulator(config).run(), RunOutcome.FAIL)
                evidence = json.loads(
                    config.evidence_path.read_text(encoding="utf-8")
                )
                self.assertIn(expected, evidence["error"])
                self.assertEqual(state.posts, 0)

    def test_receiver_status_evidence_projects_private_endpoints(self) -> None:
        state = FakeIngestState()
        state.status_extra = {
            "home_address": "private-home.example:5015",
            "home_candidates": [{"address": "192.0.2.10:5015"}],
            "relay_url": "wss://relay-token@example.invalid/private",
            "vpn": {"candidates": [{"ip": "198.51.100.8"}]},
        }
        with TemporaryDirectory() as temporary, FakeServer(state) as bridge_url:
            config = self._config(temporary, bridge_url)
            self.assertEqual(Simulator(config).run(), RunOutcome.PASS)
            evidence = json.loads(config.evidence_path.read_text(encoding="utf-8"))
            self.assertEqual(
                evidence["receiver"]["status"],
                {
                    "http_status": 200,
                    "body": {
                        "instance_id": state.instance_id,
                        "posture": "direct",
                    },
                },
            )
            status_receipt = next(
                receipt
                for receipt in evidence["http_receipts"]
                if receipt["purpose"] == "receiver status"
            )
            self.assertEqual(
                status_receipt["body"],
                {"instance_id": state.instance_id, "posture": "direct"},
            )
            serialized = json.dumps(evidence, sort_keys=True)
            for private_value in (
                "private-home.example",
                "192.0.2.10",
                "relay-token",
                "198.51.100.8",
            ):
                self.assertNotIn(private_value, serialized)

    def test_nonfinite_timeouts_are_rejected(self) -> None:
        with TemporaryDirectory() as temporary:
            base = self._config(temporary, "http://127.0.0.1:9")
            cases = (
                ("request", {"request_timeout": float("nan")}, "request_timeout"),
                (
                    "processing",
                    {"processing_timeout": float("inf")},
                    "processing_timeout",
                ),
                ("poll", {"poll_interval": float("inf")}, "poll_interval"),
            )
            for name, changes, expected in cases:
                with self.subTest(name=name), self.assertRaisesRegex(
                    ManifestError, expected
                ):
                    Simulator(replace(base, **changes))

    def test_owned_carrier_assurance_waits_for_native_startup(self) -> None:
        with TemporaryDirectory() as temporary:
            pair_secret = "splink-secret-owned-run-value"
            external = Simulator(
                self._config(temporary, "http://127.0.0.1:9")
            )
            self.assertEqual(
                external.evidence["bridge"]["carrier_assurance"],
                "caller-asserted",
            )
            base = self._config(temporary, "http://127.0.0.1:9")
            direct = Simulator(
                replace(
                    base,
                    bridge_url=None,
                    pair_code=pair_secret,
                    state_dir=Path(temporary) / "direct-state",
                    evidence_path=Path(temporary) / "direct-evidence.json",
                )
            )
            relay = Simulator(
                replace(
                    base,
                    bridge_url=None,
                    pair_code=pair_secret,
                    carrier="relay",
                    state_dir=Path(temporary) / "relay-state",
                    evidence_path=Path(temporary) / "relay-evidence.json",
                )
            )
            self.assertEqual(
                direct.evidence["bridge"]["carrier_assurance"],
                None,
            )
            self.assertEqual(
                relay.evidence["bridge"]["carrier_assurance"],
                None,
            )
            for value in (direct.state, direct.evidence, relay.state, relay.evidence):
                self.assertNotIn(pair_secret, json.dumps(value, sort_keys=True))

    def test_listing_contract_rejects_bad_total_observed_and_status(self) -> None:
        cases = (
            ("total", {"listing_total_delta": 1}, "total does not match"),
            ("observed", {"observed": "false"}, "observed must be boolean"),
            ("status", {"listing_file_status": "unknown"}, "status is invalid"),
        )
        for name, changes, expected in cases:
            with self.subTest(name=name), TemporaryDirectory() as temporary:
                state = FakeIngestState()
                for attribute, value in changes.items():
                    setattr(state, attribute, value)
                with FakeServer(state) as bridge_url:
                    config = self._config(temporary, bridge_url)
                    self.assertEqual(Simulator(config).run(), RunOutcome.FAIL)
                evidence = json.loads(
                    config.evidence_path.read_text(encoding="utf-8")
                )
                self.assertIn(expected, evidence["error"])

    def test_listing_contract_rejects_path_bearing_file_names(self) -> None:
        state = FakeIngestState()
        with TemporaryDirectory() as temporary, FakeServer(state) as bridge_url:
            config = self._config(temporary, bridge_url)
            simulator = Simulator(config)
            segment = simulator.segments[0]
            expected = segment.files[0]
            listing_file = {
                "name": expected.submitted,
                "submitted_name": expected.submitted,
                "size": expected.size,
                "sha256": expected.sha256,
                "status": "present",
            }
            state.items = [
                {
                    "day": segment.day,
                    "source": segment.source,
                    "listing": {
                        "key": segment.segment,
                        "observed": False,
                        "files": [listing_file],
                    },
                }
            ]
            client = BridgeHttpClient(bridge_url, timeout=5)
            for field, value, expected_error in (
                ("name", "/other/segment/file", "file name is invalid"),
                ("submitted_name", "../file", "submitted_name is invalid"),
            ):
                with self.subTest(field=field):
                    original = listing_file[field]
                    listing_file[field] = value
                    with self.assertRaisesRegex(SimulationFailure, expected_error):
                        simulator._listing(client, segment, segment.day)
                    listing_file[field] = original

    def test_listing_and_upload_require_exact_http_200(self) -> None:
        with TemporaryDirectory() as temporary:
            state = FakeIngestState()
            state.listing_http_status = 201
            with FakeServer(state) as bridge_url:
                config = self._config(temporary, bridge_url)
                self.assertEqual(Simulator(config).run(), RunOutcome.FAIL)
            evidence = json.loads(config.evidence_path.read_text(encoding="utf-8"))
            self.assertIn("listing", evidence["error"])
            self.assertTrue(
                any(
                    receipt.get("http_status") == 201
                    and receipt.get("purpose", "").startswith("segment listing")
                    for receipt in evidence["http_receipts"]
                )
            )

        for status in (201, 202):
            with self.subTest(upload_status=status), TemporaryDirectory() as temporary:
                state = FakeIngestState()
                state.post_http_status = status
                with FakeServer(state) as bridge_url:
                    config = self._config(temporary, bridge_url)
                    self.assertEqual(Simulator(config).run(), RunOutcome.FAIL)
                    first_posts = state.posts
                    state.post_http_status = 200
                    self.assertEqual(Simulator(config).run(), RunOutcome.FAIL)
                self.assertEqual(first_posts, 1)
                self.assertEqual(state.posts, 1)
                saved = json.loads(
                    (config.state_dir / "state.json").read_text(encoding="utf-8")
                )
                first_id = config.manifest.profiles["smoke"].segment_ids[0]
                self.assertEqual(
                    saved["segments"][first_id]["phase"], "contract_failed"
                )
                self.assertEqual(
                    saved["segments"][first_id]["contract_failure"]["http_status"],
                    status,
                )

    def test_definitive_upload_contract_failure_cannot_pass_on_resume(self) -> None:
        def wrong_meta(response: dict[str, Any]) -> None:
            response["meta"] = {"wrong": True}

        state = FakeIngestState()
        state.response_mutator = wrong_meta
        with TemporaryDirectory() as temporary, FakeServer(state) as bridge_url:
            config = self._config(temporary, bridge_url)
            self.assertEqual(Simulator(config).run(), RunOutcome.FAIL)
            self.assertEqual(state.posts, 1)
            state.response_mutator = None
            self.assertEqual(Simulator(config).run(), RunOutcome.FAIL)
            self.assertEqual(state.posts, 1)
            evidence = json.loads(config.evidence_path.read_text(encoding="utf-8"))
            self.assertIn(
                "persisted response-contract failure", evidence["error"]
            )
            saved = json.loads(
                (config.state_dir / "state.json").read_text(encoding="utf-8")
            )
            first_id = config.manifest.profiles["smoke"].segment_ids[0]
            self.assertEqual(
                saved["segments"][first_id]["phase"], "contract_failed"
            )
            self.assertIn(
                "did not echo exact metadata",
                saved["segments"][first_id]["contract_failure"]["detail"],
            )

    def test_received_malformed_responses_are_failures_with_safe_receipts(self) -> None:
        cases = ("identity", "post")
        for endpoint in cases:
            with self.subTest(endpoint=endpoint), TemporaryDirectory() as temporary:
                state = FakeIngestState()
                if endpoint == "identity":
                    state.identity_raw_body = b"not-json-identity"
                else:
                    state.post_raw_body = b"not-json-upload"
                with FakeServer(state) as bridge_url:
                    config = self._config(temporary, bridge_url)
                    self.assertEqual(Simulator(config).run(), RunOutcome.FAIL)
                evidence = json.loads(
                    config.evidence_path.read_text(encoding="utf-8")
                )
                receipts = (
                    evidence["http_receipts"]
                    if endpoint == "identity"
                    else evidence["upload_receipts"]
                )
                serialized = json.dumps(receipts, sort_keys=True)
                self.assertIn("non_json_body", serialized)
                self.assertIn("raw_sha256", serialized)
                self.assertNotIn("not-json", serialized)
                if endpoint == "identity":
                    self.assertEqual(state.posts, 0)
                else:
                    self.assertEqual(state.posts, 1)
                    saved = json.loads(
                        (config.state_dir / "state.json").read_text(encoding="utf-8")
                    )
                    first_id = config.manifest.profiles["smoke"].segment_ids[0]
                    self.assertEqual(
                        saved["segments"][first_id]["phase"], "contract_failed"
                    )

    def test_upload_response_rejects_meta_descriptor_and_collision_drift(self) -> None:
        def wrong_meta(response: dict[str, Any]) -> None:
            if response.get("status") == "ok":
                response["meta"] = {"wrong": True}

        def wrong_descriptor(response: dict[str, Any]) -> None:
            if response.get("status") == "ok":
                response["file_descriptors"][0]["size"] += 1

        def lost_collision_lineage(response: dict[str, Any]) -> None:
            if response.get("status") == "collision":
                response.pop("segment_original", None)

        def wrong_byte_count(response: dict[str, Any]) -> None:
            if response.get("status") == "ok":
                response["bytes"] += 1

        def missing_duplicate_message(response: dict[str, Any]) -> None:
            if response.get("status") == "duplicate":
                response.pop("message", None)

        def wrong_duplicate_disposition(response: dict[str, Any]) -> None:
            if response.get("status") == "duplicate":
                response["file_descriptors"][0]["disposition"] = "written"

        cases = (
            ("meta", wrong_meta, "did not echo exact metadata"),
            ("descriptor", wrong_descriptor, "descriptor does not match"),
            ("collision", lost_collision_lineage, "lost requested-key lineage"),
            ("bytes", wrong_byte_count, "byte count is invalid"),
            ("duplicate message", missing_duplicate_message, "omitted its message"),
            (
                "duplicate disposition",
                wrong_duplicate_disposition,
                "duplicate response was not already held",
            ),
        )
        for name, mutator, expected in cases:
            with self.subTest(name=name), TemporaryDirectory() as temporary:
                state = FakeIngestState()
                state.response_mutator = mutator
                with FakeServer(state) as bridge_url:
                    config = self._config(temporary, bridge_url)
                    self.assertEqual(Simulator(config).run(), RunOutcome.FAIL)
                evidence = json.loads(
                    config.evidence_path.read_text(encoding="utf-8")
                )
                self.assertIn(expected, evidence["error"])

    def test_simulator_owned_bridge_requires_live_local_manager(self) -> None:
        class OwnedBridge:
            def __init__(self, base_url: str) -> None:
                self.base_url = base_url
                self.stopped = False
                self.provenance = {
                    "credentials": {"client_cid": TEST_CLIENT_CID}
                }

            def start(self) -> str:
                return self.base_url

            def finish(self, *, remove_credentials: bool) -> None:
                raise AssertionError(
                    f"failed run must not finish credentials: {remove_credentials}"
                )

            def stop(self) -> None:
                self.stopped = True

        state = FakeIngestState()
        state.local_manager_alive = False
        with TemporaryDirectory() as temporary, FakeServer(state) as bridge_url:
            base = self._config(temporary, bridge_url)
            config = replace(base, bridge_url=None, pair_code="pair-code")
            bridge = OwnedBridge(bridge_url)
            with patch(
                "tools.journal_device_sim.runner.LinkBridge", return_value=bridge
            ):
                self.assertEqual(Simulator(config).run(), RunOutcome.FAIL)
            evidence = json.loads(config.evidence_path.read_text(encoding="utf-8"))
            self.assertIn("did not report a live manager", evidence["error"])
            self.assertEqual(
                evidence["bridge"]["carrier_assurance"],
                "native-direct-only",
            )
            self.assertTrue(bridge.stopped)

    def test_final_contract_reads_require_day_and_root_manifests(self) -> None:
        cases = (
            ("day", "day_manifest_missing", "day manifest"),
            ("root", "root_manifest_missing", "ingest manifest"),
        )
        for name, attribute, expected in cases:
            with self.subTest(name=name), TemporaryDirectory() as temporary:
                state = FakeIngestState()
                setattr(state, attribute, True)
                with FakeServer(state) as bridge_url:
                    config = self._config(temporary, bridge_url)
                    self.assertEqual(Simulator(config).run(), RunOutcome.FAIL)
                evidence = json.loads(
                    config.evidence_path.read_text(encoding="utf-8")
                )
                self.assertIn(expected, evidence["error"])
                self.assertEqual(len(evidence["contract_reads"]), 1)
                receipt = evidence["contract_reads"][0]
                self.assertEqual(receipt["segments"]["http_status"], 200)
                if name == "day":
                    self.assertEqual(
                        receipt["manifest_day"]["http_status"], 404
                    )
                    self.assertIsNone(receipt["manifest"])
                else:
                    self.assertEqual(
                        receipt["manifest_day"]["http_status"], 200
                    )
                    self.assertEqual(receipt["manifest"]["http_status"], 404)

    def test_resumed_state_is_fully_validated(self) -> None:
        cases: tuple[tuple[str, Callable[[dict[str, Any], str], None], str], ...] = (
            (
                "run_id",
                lambda value, _fixture: value.update(run_id="not-a-run-id"),
                "run_id is malformed",
            ),
            (
                "unknown fixture",
                lambda value, _fixture: value["segments"].update(unknown={}),
                "unselected fixture",
            ),
            (
                "phase",
                lambda value, fixture: value["segments"].update(
                    {fixture: {"phase": "finished"}}
                ),
                "invalid phase",
            ),
            (
                "attempt bool",
                lambda value, fixture: value["segments"].update(
                    {fixture: {"upload_attempts": True}}
                ),
                "invalid upload_attempts",
            ),
            (
                "landed",
                lambda value, fixture: value["segments"].update(
                    {fixture: {"landed_segment": "../escape"}}
                ),
                "invalid landed_segment",
            ),
        )
        for name, mutate, expected in cases:
            with self.subTest(name=name), TemporaryDirectory() as temporary:
                config = self._config(temporary, "http://127.0.0.1:9")
                simulator = Simulator(config)
                value = json.loads(
                    simulator.state_path.read_text(encoding="utf-8")
                )
                fixture_id = simulator.segments[0].fixture_id
                mutate(value, fixture_id)
                simulator.state_path.write_text(
                    json.dumps(value) + "\n", encoding="utf-8"
                )
                with self.assertRaisesRegex(SimulationFailure, expected):
                    Simulator(config)

    def test_journal_root_binds_native_identity_and_legacy_takes_precedence(
        self,
    ) -> None:
        state = FakeIngestState()
        with TemporaryDirectory() as temporary, FakeServer(state) as bridge_url:
            root = Path(temporary)
            journal_root = root / "journal"
            self._write_journal_identity(journal_root, state.instance_id)
            config = replace(
                self._config(temporary, bridge_url), journal_root=journal_root
            )
            simulator = Simulator(config)
            simulator._bind_receiver(BridgeHttpClient(bridge_url, timeout=5))
            self.assertEqual(
                simulator.evidence["receiver"]["journal_root"]["state_path"],
                "link/ca/state.json",
            )

            legacy = journal_root / "link" / "state.json"
            legacy.write_text(
                json.dumps({"instance_id": "legacy-other"}) + "\n",
                encoding="utf-8",
            )
            changed = replace(
                config,
                state_dir=root / "legacy-state",
                evidence_path=root / "legacy-evidence.json",
            )
            with self.assertRaisesRegex(SimulationFailure, "different journal"):
                Simulator(changed)._bind_receiver(
                    BridgeHttpClient(bridge_url, timeout=5)
                )
            self.assertEqual(state.posts, 0)

    def test_journal_root_cannot_overlap_state_or_evidence(self) -> None:
        with TemporaryDirectory() as temporary:
            root = Path(temporary)
            journal_root = root / "journal"
            journal_root.mkdir()
            base = self._config(temporary, "http://127.0.0.1:9")
            cases = (
                {"state_dir": journal_root / "state"},
                {"evidence_path": journal_root / "evidence.json"},
            )
            for changes in cases:
                with self.subTest(changes=changes), self.assertRaisesRegex(
                    ManifestError, "cannot overlap the receiving journal root"
                ):
                    Simulator(replace(base, journal_root=journal_root, **changes))

    def test_processing_profile_records_structured_custody_and_processing_oracles(
        self,
    ) -> None:
        state = FakeIngestState()
        with TemporaryDirectory() as temporary, FakeServer(state) as bridge_url:
            root = Path(temporary)
            fixture_root = root / "fixture"
            fixture_root.mkdir()
            audio = fixture_root / "sample.wav"
            audio.write_bytes(b"RIFF-test-audio")
            raw = audio.read_bytes()
            manifest_path = fixture_root / "manifest.json"
            manifest_path.write_text(
                json.dumps(
                    {
                        "schema": "solstone.journal-device-sim.fixtures.v1",
                        "profiles": {
                            "smoke": {
                                "segments": ["audio"],
                                "verify_duplicate": False,
                                "verification": "processing",
                            }
                        },
                        "segments": [
                            {
                                "id": "audio",
                                "day": "20260201",
                                "segment": "080000_15",
                                "source": "audio-wav",
                                "files": [
                                    {
                                        "path": audio.name,
                                        "submitted": audio.name,
                                        "size": len(raw),
                                        "sha256": hashlib.sha256(raw).hexdigest(),
                                    }
                                ],
                                "expect": {
                                    "processing": [
                                        {
                                            "input": audio.name,
                                            "output": "sample.jsonl",
                                            "handler": "transcribe",
                                        }
                                    ]
                                },
                            }
                        ],
                    }
                )
                + "\n",
                encoding="utf-8",
            )
            journal_root = root / "journal"
            self._write_journal_identity(journal_root, state.instance_id)
            config = replace(
                self._config(temporary, bridge_url, manifest_path),
                journal_root=journal_root,
            )
            simulator = Simulator(config)
            processing_rows = [
                {
                    "raw": "sample.wav",
                    "_solstone_processing": {
                        "schema": "solstone.processing.v1",
                        "state": "analyzed",
                        "reason_code": "ok",
                        "handler": "transcribe",
                        "input_size": len(raw),
                        "attempted_at": "2026-08-21T00:00:00Z",
                    },
                },
                {
                    "start": "00:00:00",
                    "text": "test audio",
                    "sentence_id": 1,
                },
            ]
            self._write_journal_segment(
                simulator,
                "audio",
                processing_rows=processing_rows,
            )
            self.assertEqual(Simulator(config).run(), RunOutcome.PASS)
            evidence = json.loads(config.evidence_path.read_text(encoding="utf-8"))
            oracles = evidence["segments"][0]["journal_oracles"]
            self.assertEqual(oracles["custody"]["ingest"]["files_verified"], 1)
            self.assertEqual(
                oracles["processing"],
                [
                    {
                        "attempted_at": "2026-08-21T00:00:00Z",
                        "handler": "transcribe",
                        "input": "sample.wav",
                        "input_size": len(raw),
                        "output": "sample.jsonl",
                        "output_sha256": hashlib.sha256(
                            "".join(
                                json.dumps(row) + "\n" for row in processing_rows
                            ).encode("utf-8")
                        ).hexdigest(),
                        "reason_code": "ok",
                        "state": "analyzed",
                        "total_rows": 1,
                        "valid_semantic_rows": 1,
                    }
                ],
            )

    def test_terminal_empty_processing_proves_custody_but_not_semantics(self) -> None:
        state = FakeIngestState()
        with TemporaryDirectory() as temporary, FakeServer(state) as bridge_url:
            _config, simulator, raw = self._processing_simulator(
                temporary, bridge_url, state
            )
            segment = simulator.segments[0]
            expectation = segment.expectation.processing[0]
            target = self._write_journal_segment(
                simulator,
                "media",
                processing_rows=[
                    {
                        "raw": expectation.input,
                        "_solstone_processing": {
                            "schema": "solstone.processing.v1",
                            "state": "empty",
                            "reason_code": "no_speech",
                            "handler": expectation.handler,
                            "input_size": len(raw),
                            "attempted_at": "2026-08-21T00:00:00Z",
                        },
                    }
                ],
            )
            ready, proof = simulator._terminal_processing_oracle(
                segment, target, expectation
            )
            self.assertTrue(ready)
            self.assertEqual(proof["state"], "empty")
            with self.assertRaisesRegex(
                SimulationFailure, "successful exact-input proof"
            ):
                simulator._processing_oracle(segment, target, expectation)

    def test_processing_oracle_rejects_schema_and_semantic_drift(self) -> None:
        state = FakeIngestState()
        with TemporaryDirectory() as temporary, FakeServer(state) as bridge_url:
            _config, simulator, raw = self._processing_simulator(
                temporary, bridge_url, state
            )
            segment = simulator.segments[0]
            expectation = segment.expectation.processing[0]
            target = self._write_journal_segment(simulator, "media")
            output = target / expectation.output

            base_record: dict[str, Any] = {
                "schema": "solstone.processing.v1",
                "state": "analyzed",
                "reason_code": "ok",
                "handler": expectation.handler,
                "input_size": len(raw),
                "attempted_at": "2026-08-21T00:00:00Z",
            }
            cases = (
                (
                    "numeric audio start",
                    base_record,
                    {"start": 0.0, "text": "speech", "sentence_id": 1},
                    "no valid semantic artifact",
                ),
                (
                    "boolean input size",
                    {**base_record, "input_size": True},
                    {"start": "00:00:00", "text": "speech", "sentence_id": 1},
                    "successful exact-input proof",
                ),
                (
                    "naive timestamp",
                    {**base_record, "attempted_at": "2026-08-21T00:00:00"},
                    {"start": "00:00:00", "text": "speech", "sentence_id": 1},
                    "successful exact-input proof",
                ),
                (
                    "missing sentence identity",
                    base_record,
                    {"start": "00:00:00", "text": "speech"},
                    "no valid semantic artifact",
                ),
                (
                    "invalid audio clock",
                    base_record,
                    {"start": "99:99:99", "text": "speech", "sentence_id": 1},
                    "no valid semantic artifact",
                ),
            )
            for name, record, semantic, expected in cases:
                with self.subTest(name=name):
                    output.write_text(
                        json.dumps(
                            {
                                "raw": expectation.input,
                                "_solstone_processing": record,
                            }
                        )
                        + "\n"
                        + json.dumps(semantic)
                        + "\n",
                        encoding="utf-8",
                    )
                    with self.assertRaisesRegex(SimulationFailure, expected):
                        simulator._processing_oracle(segment, target, expectation)

            output.write_text(
                json.dumps(
                    {
                        "raw": expectation.input,
                        "_solstone_processing": base_record,
                    }
                )
                + '\n{"start":"00:00:00","text":"speech","sentence_id":1,"score":NaN}\n',
                encoding="utf-8",
            )
            with self.assertRaisesRegex(SimulationFailure, "valid bounded JSON"):
                simulator._processing_oracle(segment, target, expectation)

            valid_audio = {
                "start": "00:00:00",
                "text": "speech",
                "sentence_id": 1,
            }
            invalid_artifacts = (
                [valid_audio, {"start": "00:00:01", "text": "junk"}],
                [valid_audio, {**valid_audio, "start": "00:00:01"}],
                [{**valid_audio, "text": "   "}],
            )
            for semantic_rows in invalid_artifacts:
                output.write_text(
                    json.dumps(
                        {
                            "raw": expectation.input,
                            "_solstone_processing": base_record,
                        }
                    )
                    + "\n"
                    + "".join(json.dumps(row) + "\n" for row in semantic_rows),
                    encoding="utf-8",
                )
                with self.assertRaisesRegex(
                    SimulationFailure, "no valid semantic artifact"
                ):
                    simulator._processing_oracle(segment, target, expectation)

            retryable = {
                **base_record,
                "state": "failed",
                "reason_code": "temporary_model_error",
                "attempts": 1,
            }
            output.write_text(
                json.dumps(
                    {
                        "raw": expectation.input,
                        "_solstone_processing": retryable,
                    }
                )
                + "\n",
                encoding="utf-8",
            )
            with self.assertRaisesRegex(
                SimulationFailure, "successful exact-input proof"
            ):
                simulator._processing_oracle(segment, target, expectation)

    def test_screen_processing_oracle_accepts_finite_semantic_row(self) -> None:
        state = FakeIngestState()
        with TemporaryDirectory() as temporary, FakeServer(state) as bridge_url:
            _config, simulator, raw = self._processing_simulator(
                temporary, bridge_url, state, handler="describe"
            )
            segment = simulator.segments[0]
            expectation = segment.expectation.processing[0]
            rows = [
                {
                    "raw": expectation.input,
                    "_solstone_processing": {
                        "schema": "solstone.processing.v1",
                        "state": "analyzed",
                        "reason_code": "ok",
                        "handler": "describe",
                        "input_size": len(raw),
                        "attempted_at": "2026-08-21T00:00:00+00:00",
                    },
                },
                {
                    "frame_id": 1,
                    "timestamp": 0.5,
                    "analysis": {
                        "visual_description": "a screen",
                        "primary": "code",
                        "secondary": "none",
                        "overlap": True,
                    },
                    "requests": [
                        {"type": "describe", "model": "fixture", "duration": 0.1}
                    ],
                    "enhanced": False,
                },
            ]
            target = self._write_journal_segment(
                simulator, "media", processing_rows=rows
            )
            retryable = {
                **rows[0],
                "_solstone_processing": {
                    **rows[0]["_solstone_processing"],
                    "state": "failed",
                    "reason_code": "analysis_failed",
                    "attempts": 2,
                },
            }
            (target / expectation.output).write_text(
                json.dumps(retryable) + "\n", encoding="utf-8"
            )
            self.assertEqual(
                simulator._processing_oracle(segment, target, expectation),
                (False, None),
            )
            invalid_rows = (
                {"timestamp": -999, "analysis": {}},
                {
                    **rows[1],
                    "timestamp": 10**400,
                },
                {
                    **rows[1],
                    "requests": [
                        {
                            "type": "describe",
                            "model": "fixture",
                            "duration": 10**400,
                        }
                    ],
                },
                {
                    "frame_id": 1,
                    "timestamp": 0.5,
                    "analysis": {
                        "visual_description": "a screen",
                        "primary": "unknown",
                        "secondary": "none",
                        "overlap": True,
                    },
                    "requests": [
                        {"type": "describe", "model": "fixture", "duration": 0.1}
                    ],
                    "enhanced": False,
                },
            )
            for invalid in invalid_rows:
                (target / expectation.output).write_text(
                    json.dumps(rows[0]) + "\n" + json.dumps(invalid) + "\n",
                    encoding="utf-8",
                )
                with self.assertRaisesRegex(
                    SimulationFailure, "no valid semantic artifact"
                ):
                    simulator._processing_oracle(segment, target, expectation)
            (target / expectation.output).write_text(
                "".join(json.dumps(row) + "\n" for row in rows),
                encoding="utf-8",
            )
            ready, proof = simulator._processing_oracle(
                segment, target, expectation
            )
            self.assertTrue(ready)
            self.assertEqual(proof["valid_semantic_rows"], 1)
            self.assertRegex(proof["output_sha256"], r"^[0-9a-f]{64}$")

    def test_custody_binds_listing_name_to_event_and_segment_directory(self) -> None:
        state = FakeIngestState()
        with TemporaryDirectory() as temporary, FakeServer(state) as bridge_url:
            _config, simulator, raw = self._processing_simulator(
                temporary, bridge_url, state
            )
            segment = simulator.segments[0]
            expected = segment.files[0]
            target = self._write_journal_segment(simulator, "media")
            listing = {
                "key": segment.segment,
                "observed": False,
                "files": [
                    {
                        "name": str(target.parent / "outside.wav"),
                        "submitted_name": expected.submitted,
                        "size": expected.size,
                        "sha256": expected.sha256,
                        "status": "present",
                    }
                ],
            }
            (target.parent / "outside.wav").write_bytes(raw)
            with self.assertRaisesRegex(
                SimulationFailure, "not bound to its device event"
            ):
                simulator._custody_oracle(
                    segment, segment.day, segment.segment, listing
                )

            listing["files"][0]["name"] = "other.wav"
            (target / "other.wav").write_bytes(raw)
            with self.assertRaisesRegex(
                SimulationFailure, "not bound to its device event"
            ):
                simulator._custody_oracle(
                    segment, segment.day, segment.segment, listing
                )

            listing["files"][0]["name"] = expected.submitted
            ready, selected, proof = simulator._custody_oracle(
                segment, segment.day, segment.segment, listing
            )
            self.assertTrue(ready)
            self.assertEqual(selected, target)
            self.assertEqual(proof["physical_files"][0]["sha256_verified"], True)

    def test_receiver_custody_hash_is_streamed_without_bounded_byte_buffer(self) -> None:
        state = FakeIngestState()
        with TemporaryDirectory() as temporary, FakeServer(state) as bridge_url:
            _config, simulator, _raw = self._processing_simulator(
                temporary, bridge_url, state
            )
            segment = simulator.segments[0]
            expected = segment.files[0]
            target = self._write_journal_segment(simulator, "media")
            with patch(
                "tools.journal_device_sim.runner._confined_bytes",
                side_effect=AssertionError("custody hash buffered receiver bytes"),
            ):
                simulator._hash_receiver_file(
                    target, expected.submitted, expected
                )

    @unittest.skipUnless(os.name == "posix", "symlink fixture requires POSIX")
    def test_fixture_identity_is_pinned_through_the_upload_open(self) -> None:
        state = FakeIngestState()
        with TemporaryDirectory() as temporary, FakeServer(state) as bridge_url:
            config = self._config(temporary, bridge_url)
            fixture_id = config.manifest.profiles["smoke"].segment_ids[0]
            segment = config.manifest.segments[fixture_id]
            original = segment.files[0]
            tracked = Path(temporary) / "tracked.jsonl"
            tracked.write_bytes(original.path.read_bytes())
            tracked_metadata = tracked.stat()
            pinned = replace(
                original,
                path=tracked,
                device=tracked_metadata.st_dev,
                inode=tracked_metadata.st_ino,
            )
            config.manifest.segments[fixture_id] = replace(
                segment, files=(pinned, *segment.files[1:])
            )
            simulator = Simulator(config)
            untracked = Path(temporary) / "untracked-secret.jsonl"
            untracked.write_bytes(tracked.read_bytes())
            tracked.unlink()
            tracked.symlink_to(untracked.name)

            self.assertEqual(simulator.run(), RunOutcome.FAIL)
            self.assertEqual(state.posts, 0)
            evidence = json.loads(config.evidence_path.read_text(encoding="utf-8"))
            self.assertIn("cannot be opened safely", evidence["error"])
            self.assertEqual(evidence["request_count"], 0)
            saved_state = json.loads(
                (config.state_dir / "state.json").read_text(encoding="utf-8")
            )
            self.assertNotIn(fixture_id, saved_state["segments"])

    def test_fixture_digest_is_checked_before_transport_or_attempt_state(self) -> None:
        state = FakeIngestState()
        with TemporaryDirectory() as temporary, FakeServer(state) as bridge_url:
            config = self._config(temporary, bridge_url)
            fixture_id = config.manifest.profiles["smoke"].segment_ids[0]
            segment = config.manifest.segments[fixture_id]
            original = segment.files[0]
            tracked = Path(temporary) / "tracked.jsonl"
            tracked.write_bytes(original.path.read_bytes())
            tracked_metadata = tracked.stat()
            pinned = replace(
                original,
                path=tracked,
                device=tracked_metadata.st_dev,
                inode=tracked_metadata.st_ino,
            )
            config.manifest.segments[fixture_id] = replace(
                segment, files=(pinned, *segment.files[1:])
            )
            simulator = Simulator(config)
            changed = bytearray(tracked.read_bytes())
            changed[0] ^= 0xFF
            tracked.write_bytes(changed)
            after = tracked.stat()
            self.assertEqual(after.st_size, pinned.size)
            self.assertEqual(
                (after.st_dev, after.st_ino),
                (pinned.device, pinned.inode),
            )

            self.assertEqual(simulator.run(), RunOutcome.FAIL)
            self.assertEqual(state.posts, 0)
            evidence = json.loads(config.evidence_path.read_text(encoding="utf-8"))
            self.assertIn("changed digest", evidence["error"])
            self.assertEqual(evidence["request_count"], 0)
            saved_state = json.loads(
                (config.state_dir / "state.json").read_text(encoding="utf-8")
            )
            self.assertNotIn(fixture_id, saved_state["segments"])

    def test_evidence_destination_is_writable_before_transport(self) -> None:
        with TemporaryDirectory() as temporary:
            config = self._config(temporary, "http://127.0.0.1:9")
            config.evidence_path.mkdir()
            simulator = Simulator(config)
            with patch.object(simulator, "_run_with_client") as run_with_client:
                with self.assertRaisesRegex(
                    SimulationFailure, "initial evidence write failed"
                ):
                    simulator.run()
            run_with_client.assert_not_called()

    def test_terminal_evidence_failure_never_leaves_stale_pass(self) -> None:
        state = FakeIngestState()
        with TemporaryDirectory() as temporary, FakeServer(state) as bridge_url:
            config = self._config(temporary, bridge_url)

            def fail_terminal_evidence(
                path: Path, value: dict[str, Any], mode: int = 0o600
            ) -> None:
                if path == config.evidence_path and value.get("result") is not None:
                    raise PermissionError("injected terminal failure")
                _atomic_json(path, value, mode)

            with patch(
                "tools.journal_device_sim.runner._atomic_json",
                side_effect=fail_terminal_evidence,
            ):
                with self.assertRaisesRegex(
                    SimulationFailure, "terminal PASS evidence write failed"
                ):
                    Simulator(config).run()
            self.assertEqual(state.posts, 4)
            evidence = json.loads(config.evidence_path.read_text(encoding="utf-8"))
            self.assertIsNone(evidence["result"])

    @unittest.skipUnless(os.name == "posix", "symlink fixture requires POSIX")
    def test_white_box_outputs_reject_invalid_keys_and_linked_directories(
        self,
    ) -> None:
        with TemporaryDirectory() as temporary:
            root = Path(temporary)
            journal_root = root / "journal"
            journal_root.mkdir()
            base = self._config(temporary, "http://127.0.0.1:9")
            config = replace(base, journal_root=journal_root)
            simulator = Simulator(config)
            segment = simulator.segments[0]
            listing_item = {
                "key": segment.segment,
                "observed": False,
                "files": [
                    {
                        "name": item.submitted,
                        "submitted_name": item.submitted,
                        "size": item.size,
                        "sha256": item.sha256,
                        "status": "present",
                    }
                    for item in segment.files
                ],
            }
            with self.assertRaisesRegex(SimulationFailure, "invalid landed segment"):
                simulator._white_box_oracles(
                    segment, segment.day, "../../outside", listing_item
                )

            outside = root / "outside-segment"
            outside.mkdir()
            (outside / "stream.json").write_text("{}\n", encoding="utf-8")
            (outside / "events.jsonl").write_text("{}\n", encoding="utf-8")
            (outside / "ingest.json").write_text(
                segment.files[0].sha256, encoding="utf-8"
            )
            linked = (
                journal_root
                / "chronicle"
                / segment.day
                / "tmux"
                / segment.segment
            )
            linked.parent.mkdir(parents=True)
            linked.symlink_to(outside, target_is_directory=True)
            self.assertEqual(
                simulator._white_box_oracles(
                    segment, segment.day, segment.segment, listing_item
                ),
                (False, None),
            )

    def test_event_recovery_selects_only_exact_authenticated_cid(self) -> None:
        state = FakeIngestState()
        with TemporaryDirectory() as temporary, FakeServer(state) as bridge_url:
            root = Path(temporary)
            journal_root = root / "journal"
            self._write_journal_identity(journal_root, state.instance_id)
            config = replace(
                self._config(temporary, bridge_url), journal_root=journal_root
            )
            simulator = Simulator(config)
            segment = simulator.segments[0]
            exact = self._write_journal_segment(
                simulator, segment.fixture_id
            )
            event_path = exact / "events.jsonl"
            exact_event = json.loads(
                event_path.read_text(encoding="utf-8").splitlines()[0]
            )
            wrong_cid = {**exact_event, "cid": "sha256:" + "b" * 64}
            event_path.write_text(
                "{torn-json\n"
                + json.dumps({"tract": "cortex", "event": "request"})
                + "\n"
                + json.dumps({"unknown": True})
                + "\n"
                + json.dumps(wrong_cid)
                + "\n"
                + json.dumps(exact_event)
                + "\n",
                encoding="utf-8",
            )

            imposter = exact.parent.parent / "imposter" / exact.name
            shutil.copytree(exact, imposter)
            marker = json.loads(
                (imposter / "stream.json").read_text(encoding="utf-8")
            )
            marker["stream"] = "imposter"
            (imposter / "stream.json").write_text(
                json.dumps(marker) + "\n", encoding="utf-8"
            )
            imposter_event = {**wrong_cid, "stream": "imposter"}
            (imposter / "events.jsonl").write_text(
                json.dumps(imposter_event) + "\n", encoding="utf-8"
            )

            candidate = simulator._journal_candidate(
                segment, segment.day, segment.segment
            )
            self.assertIsNotNone(candidate)
            assert candidate is not None
            path, _ingest, _events, recovery, matching = candidate
            self.assertEqual(path, exact)
            self.assertEqual(len(matching), 1)
            self.assertEqual(matching[0]["cid"], TEST_CLIENT_CID)
            self.assertEqual(
                recovery,
                {
                    "total_rows": 5,
                    "device_ingest_rows": 2,
                    "wrong_family": 1,
                    "unparseable": 1,
                    "unrecognized": 1,
                },
            )

    def test_external_white_box_requires_explicit_authenticated_cid(self) -> None:
        with TemporaryDirectory() as temporary:
            root = Path(temporary)
            journal_root = root / "journal"
            journal_root.mkdir()
            config = replace(
                self._config(temporary, "http://127.0.0.1:9"),
                journal_root=journal_root,
                expected_cid=None,
            )
            with self.assertRaisesRegex(ManifestError, "requires expected_cid"):
                Simulator(config)

    @unittest.skipUnless(os.name == "posix", "symlink fixture requires POSIX")
    def test_state_and_receiver_reads_reject_links_and_oversize(self) -> None:
        with TemporaryDirectory() as temporary:
            root = Path(temporary)
            config = self._config(temporary, "http://127.0.0.1:9")
            simulator = Simulator(config)
            state_path = simulator.state_path
            state_path.unlink()
            target = root / "outside-state.json"
            target.write_text("{}\n", encoding="utf-8")
            state_path.symlink_to(target)
            with self.assertRaisesRegex(SimulationFailure, "cannot be opened safely"):
                Simulator(config)

        with TemporaryDirectory() as temporary:
            config = self._config(temporary, "http://127.0.0.1:9")
            simulator = Simulator(config)
            simulator.state_path.write_bytes(b" " * (4 * 1024 * 1024 + 1))
            with self.assertRaisesRegex(SimulationFailure, "exceeds its"):
                Simulator(config)

        state = FakeIngestState()
        with TemporaryDirectory() as temporary, FakeServer(state) as bridge_url:
            root = Path(temporary)
            journal_root = root / "journal"
            self._write_journal_identity(journal_root, state.instance_id)
            config = replace(
                self._config(temporary, bridge_url), journal_root=journal_root
            )
            simulator = Simulator(config)
            segment = simulator.segments[0]
            target = self._write_journal_segment(
                simulator, segment.fixture_id
            )
            held = target / segment.files[0].submitted
            outside = root / "outside-receiver-file"
            outside.write_bytes(held.read_bytes())
            held.unlink()
            held.symlink_to(outside)
            listing_item = {
                "key": segment.segment,
                "observed": False,
                "files": [
                    {
                        "name": item.submitted,
                        "submitted_name": item.submitted,
                        "size": item.size,
                        "sha256": item.sha256,
                        "status": "present",
                    }
                    for item in segment.files
                ],
            }
            with self.assertRaisesRegex(
                SimulationFailure, "cannot be opened safely"
            ):
                simulator._custody_oracle(
                    segment, segment.day, segment.segment, listing_item
                )

    def test_server_500_after_store_reconciles_without_retry(self) -> None:
        state = FakeIngestState()
        state.fail_first_after_store = True
        with TemporaryDirectory() as temporary, FakeServer(state) as bridge_url:
            fixture_root = Path(temporary) / "fixture"
            fixture_root.mkdir()
            fixture = fixture_root / "one.jsonl"
            fixture.write_text('{"t":"one","ts":0}\n', encoding="utf-8")
            raw = fixture.read_bytes()
            manifest_path = fixture_root / "manifest.json"
            manifest_path.write_text(
                json.dumps(
                    {
                        "schema": "solstone.journal-device-sim.fixtures.v1",
                        "profiles": {
                            "smoke": {
                                "segments": ["one"],
                                "verify_duplicate": False,
                                "verification": "contract",
                            }
                        },
                        "segments": [
                            {
                                "id": "one",
                                "day": "20260201",
                                "segment": "080000_30",
                                "source": "tmux",
                                "files": [
                                    {
                                        "path": fixture.name,
                                        "submitted": "tmux.jsonl",
                                        "size": len(raw),
                                        "sha256": hashlib.sha256(raw).hexdigest(),
                                    }
                                ],
                            }
                        ],
                    }
                ),
                encoding="utf-8",
            )
            config = self._config(temporary, bridge_url, manifest_path)
            self.assertEqual(Simulator(config).run(), RunOutcome.PASS)
            self.assertEqual(state.posts, 1)
            evidence = json.loads(config.evidence_path.read_text(encoding="utf-8"))
            self.assertEqual(evidence["segments"][0]["response_http_status"], 500)
            self.assertEqual(evidence["segments"][0]["listing"]["key"], "080000_30")

    def test_reconciliation_ignores_unrelated_joined_files(self) -> None:
        with TemporaryDirectory() as temporary:
            config = self._config(temporary, "http://127.0.0.1:9")
            simulator = Simulator(config)
            segment = simulator.segments[0]
            expected = segment.files[0]
            item = {
                "files": [
                    {
                        "name": expected.submitted,
                        "size": expected.size,
                        "sha256": expected.sha256,
                        "status": "present",
                    },
                    {
                        "name": "joined.jsonl",
                        "size": 9,
                        "sha256": "f" * 64,
                        "status": "missing",
                    },
                ]
            }
            self.assertEqual(
                simulator._matched_files(item, segment),
                [item["files"][0]],
            )

    def test_state_cannot_resume_against_another_journal(self) -> None:
        first = FakeIngestState()
        second = FakeIngestState()
        second.instance_id = "another-journal-instance"
        with TemporaryDirectory() as temporary:
            with FakeServer(first) as bridge_url:
                config = self._config(temporary, bridge_url)
                self.assertEqual(Simulator(config).run(), RunOutcome.PASS)
            with FakeServer(second) as bridge_url:
                changed = self._config(temporary, bridge_url)
                self.assertEqual(Simulator(changed).run(), RunOutcome.FAIL)
            evidence = json.loads(changed.evidence_path.read_text(encoding="utf-8"))
            self.assertIn("different receiving journal", evidence["error"])
            self.assertEqual(second.posts, 0)

    def test_cleanup_failure_cannot_leave_pass_evidence(self) -> None:
        class CleanupFailBridge:
            def __init__(self, base_url: str) -> None:
                self.base_url = base_url
                self.finish_calls = 0
                self.stop_calls = 0
                self.provenance = {
                    "credentials": {"client_cid": TEST_CLIENT_CID}
                }

            def start(self) -> str:
                return self.base_url

            def finish(self, *, remove_credentials: bool) -> None:
                self.finish_calls += 1
                self.assert_remove = remove_credentials
                raise LinkProcessError("credential cleanup failed: PermissionError")

            def stop(self) -> None:
                self.stop_calls += 1

        state = FakeIngestState()
        with TemporaryDirectory() as temporary, FakeServer(state) as bridge_url:
            base = self._config(temporary, bridge_url)
            config = replace(base, bridge_url=None, pair_code="pair-code")
            bridge = CleanupFailBridge(bridge_url)
            with patch(
                "tools.journal_device_sim.runner.LinkBridge", return_value=bridge
            ):
                self.assertEqual(Simulator(config).run(), RunOutcome.BLOCKED)
            evidence = json.loads(config.evidence_path.read_text(encoding="utf-8"))
            self.assertEqual(evidence["result"], "BLOCKED")
            self.assertIn("credential cleanup failed", evidence["error"])
            self.assertEqual(bridge.finish_calls, 1)
            self.assertTrue(bridge.assert_remove)

    def test_nonpassing_run_becomes_blocked_when_child_cannot_stop(self) -> None:
        class StopFailBridge:
            def __init__(self, base_url: str) -> None:
                self.base_url = base_url
                self.provenance = {
                    "credentials": {"client_cid": TEST_CLIENT_CID}
                }

            def start(self) -> str:
                return self.base_url

            def stop(self) -> None:
                raise LinkProcessError("child remains live")

        state = FakeIngestState()
        with TemporaryDirectory() as temporary, FakeServer(state) as bridge_url:
            base = self._config(temporary, bridge_url)
            config = replace(base, bridge_url=None, pair_code="pair-code")
            bridge = StopFailBridge(bridge_url)
            with patch(
                "tools.journal_device_sim.runner.LinkBridge", return_value=bridge
            ), patch.object(
                Simulator,
                "_run_with_client",
                side_effect=SimulationFailure("fixture refused"),
            ):
                self.assertEqual(Simulator(config).run(), RunOutcome.BLOCKED)
            evidence = json.loads(config.evidence_path.read_text(encoding="utf-8"))
            self.assertEqual(evidence["result"], "BLOCKED")
            self.assertIn("child remains live", evidence["error"])
            self.assertIn("prior outcome: fixture refused", evidence["error"])

    def test_crash_state_reconciles_before_reupload(self) -> None:
        state = FakeIngestState()
        with TemporaryDirectory() as temporary, FakeServer(state) as bridge_url:
            base_config = self._config(temporary, bridge_url)
            first_id = base_config.manifest.profiles["smoke"].segment_ids[0]
            base_config.manifest.profiles["contract"] = FixtureProfile(
                name="contract",
                segment_ids=(first_id,),
                verify_duplicate=False,
                verification="contract",
            )
            config = replace(base_config, profile="contract")
            simulator = Simulator(config)
            segment = simulator.segments[0]
            fixture = segment.files[0]
            state.items.append(
                {
                    "day": segment.day,
                    "source": segment.source,
                    "listing": {
                        "key": segment.segment,
                        "observed": True,
                        "files": [
                            {
                                "name": fixture.submitted,
                                "submitted_name": fixture.submitted,
                                "size": fixture.size,
                                "sha256": fixture.sha256,
                                "status": "present",
                            }
                        ],
                    },
                }
            )
            simulator.state["segments"][segment.fixture_id] = {
                "phase": "sending",
                "upload_attempts": 1,
            }
            simulator._save_state()
            self.assertEqual(Simulator(config).run(), RunOutcome.PASS)
            self.assertEqual(state.posts, 0)

    def test_attempt_limit_is_per_invocation_not_per_state_lifetime(self) -> None:
        state = FakeIngestState()
        with TemporaryDirectory() as temporary, FakeServer(state) as bridge_url:
            config = self._config(temporary, bridge_url)
            simulator = Simulator(config)
            first = simulator.segments[0]
            simulator.state["segments"][first.fixture_id] = {
                "phase": "uncertain",
                "upload_attempts": config.max_attempts,
            }
            simulator._save_state()
            self.assertEqual(Simulator(config).run(), RunOutcome.PASS)
            evidence = json.loads(config.evidence_path.read_text(encoding="utf-8"))
            self.assertEqual(evidence["segments"][0]["upload_attempts"], 3)

    def test_contract_is_black_box_and_custody_or_processing_require_root(self) -> None:
        with TemporaryDirectory() as temporary:
            config = self._config(temporary, "http://127.0.0.1:9")
            segment_id = config.manifest.profiles["smoke"].segment_ids[0]
            segment = config.manifest.segments[segment_id]
            config.manifest.segments[segment_id] = replace(
                segment,
                expectation=replace(
                    segment.expectation,
                    processing=(
                        ProcessingExpectation(
                            input=segment.files[0].submitted,
                            output="derived.jsonl",
                            handler="transcribe",
                        ),
                    ),
                ),
            )
            config.manifest.profiles["contract"] = FixtureProfile(
                name="contract",
                segment_ids=(segment_id,),
                verify_duplicate=False,
                verification="contract",
            )
            contract = Simulator(replace(config, profile="contract"))
            self.assertEqual(
                contract._white_box_oracles(
                    contract.segments[0],
                    contract.segments[0].day,
                    "080000_30",
                    {
                        "key": "080000_30",
                        "observed": False,
                        "files": [],
                    },
                ),
                (True, {"custody": None, "processing": []}),
            )
            config.manifest.profiles["custody"] = FixtureProfile(
                name="custody",
                segment_ids=(segment_id,),
                verify_duplicate=False,
                verification="custody",
            )
            with self.assertRaisesRegex(
                ManifestError, "requires an explicit receiving journal root"
            ):
                Simulator(
                    replace(
                        config,
                        profile="custody",
                        state_dir=Path(temporary) / "custody-state",
                        evidence_path=Path(temporary) / "custody-evidence.json",
                    )
                )
            config.manifest.profiles["processing"] = FixtureProfile(
                name="processing",
                segment_ids=(segment_id,),
                verify_duplicate=False,
                verification="processing",
            )
            with self.assertRaisesRegex(
                ManifestError, "requires an explicit receiving journal root"
            ):
                Simulator(
                    replace(
                        config,
                        profile="processing",
                        state_dir=Path(temporary) / "processing-state",
                        evidence_path=Path(temporary) / "processing-evidence.json",
                    )
                )


if __name__ == "__main__":
    unittest.main()
