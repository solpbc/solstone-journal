# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import hashlib
import json
import os
import threading
import unittest
from dataclasses import replace
from email.parser import BytesParser
from email.policy import default
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from tempfile import TemporaryDirectory
from typing import Any
from urllib.parse import parse_qs, urlsplit
from unittest.mock import patch

from tools.journal_device_sim.manifest import FixtureProfile, load_manifest
from tools.journal_device_sim.process import LinkProcessError
from tools.journal_device_sim.runner import (
    RunOutcome,
    SimulationFailure,
    Simulator,
    SimulatorConfig,
    _atomic_json,
)


class FakeIngestState:
    def __init__(self) -> None:
        self.lock = threading.Lock()
        self.items: list[dict[str, Any]] = []
        self.posts = 0
        self.fail_first_after_store = False
        self.instance_id = "fake-journal-instance"


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

        def do_GET(self) -> None:
            parsed = urlsplit(self.path)
            if parsed.path == "/app/link/api/identity":
                self._json(
                    200,
                    {
                        "committed": True,
                        "instance_id": state.instance_id,
                        "mark": None,
                    },
                )
                return
            if not parsed.path.startswith("/app/devices/ingest/segments/"):
                self._json(404, {"reason_code": "not_found"})
                return
            day = parsed.path.rsplit("/", 1)[-1]
            source = parse_qs(parsed.query).get("source", [""])[0]
            with state.lock:
                items = [
                    item["listing"]
                    for item in state.items
                    if item["day"] == day and item["source"] == source
                ]
            self._json(
                200,
                {"protocol_version": 3, "total": len(items), "items": items},
            )

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
                            "status": "present",
                        }
                    )
            assert envelope is not None
            requested = envelope["segment"]
            day = envelope["day"]
            source = envelope.get("source", "")
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
                    }
                else:
                    same_key = any(
                        item["day"] == day
                        and item["source"] == source
                        and item["listing"]["key"] == requested
                        for item in state.items
                    )
                    landed = requested if not same_key else "080000_31"
                    listing = {"key": landed, "observed": True, "files": files}
                    if same_key:
                        listing["original_key"] = requested
                    state.items.append(
                        {"day": day, "source": source, "listing": listing}
                    )
                    response = {
                        "status": "collision" if same_key else "ok",
                        "segment": landed,
                        "file_descriptors": files,
                    }
                should_fail = state.fail_first_after_store and state.posts == 1
            if should_fail:
                self._json(500, {"status": "failed", "reason_code": "notify_failed"})
            else:
                self._json(200, response)

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
        )

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
            with self.assertRaisesRegex(SimulationFailure, "invalid landed segment"):
                simulator._required_outputs_present(
                    segment, segment.day, "../../outside"
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
                simulator._required_outputs_present(
                    segment, segment.day, segment.segment
                ),
                (False, None),
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
            base_config.manifest.profiles["custody"] = FixtureProfile(
                name="custody",
                segment_ids=(first_id,),
                verify_duplicate=False,
                verify_processing=True,
            )
            config = replace(base_config, profile="custody")
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

    def test_custody_profile_does_not_claim_processing_outputs(self) -> None:
        with TemporaryDirectory() as temporary:
            config = self._config(temporary, "http://127.0.0.1:9")
            segment_id = config.manifest.profiles["smoke"].segment_ids[0]
            segment = config.manifest.segments[segment_id]
            config.manifest.segments[segment_id] = replace(
                segment,
                expectation=replace(
                    segment.expectation,
                    required_outputs=("derived.jsonl",),
                ),
            )
            config.manifest.profiles["custody"] = FixtureProfile(
                name="custody",
                segment_ids=(segment_id,),
                verify_duplicate=False,
                verify_processing=False,
            )
            custody = Simulator(replace(config, profile="custody"))
            self.assertEqual(
                custody._required_outputs_present(
                    custody.segments[0], custody.segments[0].day, "080000_30"
                ),
                (True, None),
            )
            processing_config = replace(
                config,
                profile="smoke",
                state_dir=Path(temporary) / "processing-state",
                evidence_path=Path(temporary) / "processing-evidence.json",
            )
            processing = Simulator(processing_config)
            with self.assertRaisesRegex(SimulationFailure, "requires white-box"):
                processing._required_outputs_present(
                    processing.segments[0],
                    processing.segments[0].day,
                    "080000_30",
                )


if __name__ == "__main__":
    unittest.main()
