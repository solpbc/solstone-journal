#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Verify that Describe transports its explicit Journal to the real Generate child."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import signal
import stat
import subprocess
import sys
import tempfile
import threading
import unittest
import urllib.request
from dataclasses import dataclass
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any, Sequence

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_FIXTURE = ROOT / "core/fixtures/describe_corpus/single_frame_vp8_screen.webm"
MODEL_MARKER = "describe-journal-child-harness"


class VerificationError(RuntimeError):
    """The process-boundary invariant was not observed."""


@dataclass(frozen=True)
class ProcessResult:
    returncode: int
    stdout: str
    stderr: str


class EndpointState:
    def __init__(self) -> None:
        self._lock = threading.Lock()
        self.paths: list[str] = []
        self.chat_requests: list[dict[str, Any]] = []

    def record(self, path: str, body: bytes) -> None:
        with self._lock:
            self.paths.append(path)
            if path == "/v1/chat/completions":
                self.chat_requests.append(json.loads(body))

    def chat_request_snapshot(self) -> tuple[dict[str, Any], ...]:
        with self._lock:
            return tuple(self.chat_requests)


class _EndpointHandler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_GET(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler API
        self._serve(b"")

    def do_POST(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler API
        length = int(self.headers.get("Content-Length", "0"))
        self._serve(self.rfile.read(length))

    def _serve(self, request_body: bytes) -> None:
        state: EndpointState = self.server.state  # type: ignore[attr-defined]
        state.record(self.path, request_body)
        if self.path == "/health":
            body = {"loaded_model": MODEL_MARKER}
        elif self.path == "/props":
            body = {"n_ctx": 16_384, "total_slots": 1}
        elif self.path == "/tokenize":
            body = {"tokens": [1]}
        elif self.path == "/v1/chat/completions":
            content = json.dumps(
                {
                    "visual_description": "deterministic harness frame",
                    "primary": "code",
                    "secondary": "none",
                    "overlap": True,
                },
                separators=(",", ":"),
            )
            body = {
                "choices": [{"message": {"content": content}, "finish_reason": "stop"}],
                "usage": {
                    "prompt_tokens": 1,
                    "completion_tokens": 1,
                    "total_tokens": 2,
                },
            }
        else:
            self.send_error(404)
            return
        encoded = json.dumps(body, separators=(",", ":")).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(encoded)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(encoded)

    def log_message(self, _format: str, *_args: object) -> None:
        pass


class LocalEndpoint:
    def __init__(self) -> None:
        self.state = EndpointState()
        self.server = ThreadingHTTPServer(("127.0.0.1", 0), _EndpointHandler)
        self.server.daemon_threads = True
        self.server.state = self.state  # type: ignore[attr-defined]
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)

    @property
    def url(self) -> str:
        host, port = self.server.server_address
        return f"http://{host}:{port}"

    def __enter__(self) -> LocalEndpoint:
        self.thread.start()
        return self

    def __exit__(self, *_exc: object) -> None:
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=2)
        if self.thread.is_alive():
            raise VerificationError("loopback endpoint did not stop")


def _require_executable(label: str, value: Path) -> Path:
    path = value.expanduser().resolve()
    if not path.is_file():
        raise VerificationError(f"{label} is not a file: {path}")
    if not os.access(path, os.X_OK):
        raise VerificationError(f"{label} is not executable: {path}")
    return path


def _snapshot_tree(root: Path) -> tuple[tuple[object, ...], ...]:
    records: list[tuple[object, ...]] = []
    paths = [root, *sorted(root.rglob("*"), key=lambda item: item.as_posix())]
    for path in paths:
        metadata = path.lstat()
        relative = "." if path == root else path.relative_to(root).as_posix()
        mode = stat.S_IMODE(metadata.st_mode)
        common = (
            relative,
            mode,
            metadata.st_uid,
            metadata.st_gid,
            metadata.st_mtime_ns,
        )
        if path.is_symlink():
            records.append((*common, "symlink", os.readlink(path)))
        elif path.is_dir():
            records.append((*common, "directory"))
        elif path.is_file():
            records.append(
                (
                    *common,
                    "file",
                    metadata.st_size,
                    hashlib.sha256(path.read_bytes()).hexdigest(),
                )
            )
        else:
            records.append((*common, "other"))
    return tuple(records)


def _run_bounded(
    command: Sequence[str], env: dict[str, str], timeout_seconds: float
) -> ProcessResult:
    try:
        process = subprocess.Popen(
            command,
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            start_new_session=True,
        )
    except OSError as error:
        raise VerificationError(
            f"could not launch {' '.join(command)}: {error}"
        ) from error
    try:
        stdout, stderr = process.communicate(timeout=timeout_seconds)
    except subprocess.TimeoutExpired as error:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        stdout, stderr = process.communicate()
        raise VerificationError(
            f"process timed out after {timeout_seconds:g}s: {' '.join(command)}\n"
            f"stdout:\n{stdout}\nstderr:\n{stderr}"
        ) from error
    return ProcessResult(process.returncode, stdout, stderr)


def _write_journals(explicit: Path, inherited: Path, endpoint_url: str) -> None:
    for root in (explicit, inherited):
        (root / "config").mkdir(parents=True)
    explicit_config = {
        "providers": {
            "active": {"provider": "local"},
            "local": {
                "endpoint_url": endpoint_url,
                "served_context_window": 16_384,
                "served_model_id": MODEL_MARKER,
            },
        }
    }
    inherited_config = {"providers": {"active": {"provider": "none"}}}
    (explicit / "config/journal.json").write_text(
        json.dumps(explicit_config, separators=(",", ":")), encoding="utf-8"
    )
    (inherited / "config/journal.json").write_text(
        json.dumps(inherited_config, separators=(",", ":")), encoding="utf-8"
    )


def _artifact_rows(path: Path) -> list[dict[str, Any]]:
    if not path.is_file():
        raise VerificationError(f"Describe did not create artifact: {path}")
    try:
        rows = [json.loads(line) for line in path.read_text().splitlines() if line]
    except (OSError, json.JSONDecodeError) as error:
        raise VerificationError(f"invalid Describe artifact {path}: {error}") from error
    if not rows:
        raise VerificationError(f"Describe artifact is empty: {path}")
    return rows


def _token_rows(root: Path) -> list[dict[str, Any]]:
    token_paths = sorted((root / "tokens").glob("*.jsonl"))
    rows: list[dict[str, Any]] = []
    try:
        for path in token_paths:
            rows.extend(
                json.loads(line) for line in path.read_text().splitlines() if line
            )
    except (OSError, json.JSONDecodeError) as error:
        raise VerificationError(f"invalid token usage under {root}: {error}") from error
    return rows


def verify(
    describe_bin: Path,
    generate_bin: Path,
    fixture: Path,
    timeout_seconds: float,
) -> None:
    describe = _require_executable("--describe-bin", describe_bin)
    generate = _require_executable("--generate-bin", generate_bin)
    fixture = fixture.expanduser().resolve()
    if not fixture.is_file():
        raise VerificationError(f"--fixture is not a file: {fixture}")
    if timeout_seconds <= 0:
        raise VerificationError("--timeout-seconds must be positive")

    with tempfile.TemporaryDirectory(prefix="solstone-describe-journal-child-") as bed:
        temporary = Path(bed)
        explicit = temporary / "journal-a"
        inherited = temporary / "journal-b"
        video = temporary / fixture.name
        shutil.copyfile(fixture, video)

        with LocalEndpoint() as endpoint:
            _write_journals(explicit, inherited, endpoint.url)
            inherited_before = _snapshot_tree(inherited)
            env = os.environ.copy()
            env["SOLSTONE_JOURNAL"] = str(inherited)
            env["SOLSTONE_DESCRIBE_GENERATE_WIRE"] = str(generate)
            command = [
                str(describe),
                "--describe",
                str(video),
                "--journal",
                str(explicit),
                "--jobs",
                "1",
            ]
            result = _run_bounded(command, env, timeout_seconds)

            diagnostic = (
                f"describe-bin={describe}\ngenerate-bin={generate}\n"
                f"exit={result.returncode}\nstdout:\n{result.stdout}\n"
                f"stderr:\n{result.stderr}"
            )
            if result.returncode != 0:
                raise VerificationError(f"Describe failed\n{diagnostic}")
            if result.stdout or result.stderr:
                raise VerificationError(f"Describe emitted output\n{diagnostic}")

            rows = _artifact_rows(video.with_suffix(".jsonl"))
            header = rows[0]
            if header.get("_solstone_processing", {}).get("state") != "analyzed":
                raise VerificationError(f"artifact is not analyzed: {header}")
            if header.get("_solstone_thinking", {}).get("model") != MODEL_MARKER:
                raise VerificationError(
                    f"artifact did not record endpoint model {MODEL_MARKER}: {header}"
                )

            chat_requests = endpoint.state.chat_request_snapshot()
            if not chat_requests:
                raise VerificationError("explicit Journal endpoint served no inference")
            for request in chat_requests:
                max_tokens = request.get("max_tokens")
                if not isinstance(max_tokens, int) or not 0 < max_tokens <= 16_384:
                    raise VerificationError(
                        f"endpoint request has no admitted token budget: {request}"
                    )

            token_rows = _token_rows(explicit)
            if not token_rows:
                raise VerificationError("explicit Journal recorded no token usage")
            if not all(row.get("model") == MODEL_MARKER for row in token_rows):
                raise VerificationError(
                    f"explicit Journal token marker mismatch: {token_rows}"
                )
            if not all(
                isinstance(row.get("usage"), dict)
                and row["usage"].get("total_tokens", 0) > 0
                for row in token_rows
            ):
                raise VerificationError(
                    f"explicit Journal token usage is empty: {token_rows}"
                )

            inherited_after = _snapshot_tree(inherited)
            if inherited_after != inherited_before:
                raise VerificationError(
                    "inherited Journal changed\n"
                    f"before={inherited_before!r}\nafter={inherited_after!r}"
                )


class HelperTests(unittest.TestCase):
    def test_require_executable_names_missing_input(self) -> None:
        with tempfile.TemporaryDirectory() as bed:
            missing = Path(bed) / "missing"
            with self.assertRaisesRegex(VerificationError, "--generate-bin.*missing"):
                _require_executable("--generate-bin", missing)

    def test_snapshot_detects_content_and_mode_changes(self) -> None:
        with tempfile.TemporaryDirectory() as bed:
            root = Path(bed)
            path = root / "config.json"
            path.write_text("before")
            before = _snapshot_tree(root)
            path.write_text("after")
            self.assertNotEqual(_snapshot_tree(root), before)
            content = _snapshot_tree(root)
            path.chmod(0o600)
            self.assertNotEqual(_snapshot_tree(root), content)
            nested = _snapshot_tree(root)
            root.chmod(0o700 if stat.S_IMODE(root.stat().st_mode) != 0o700 else 0o750)
            self.assertNotEqual(_snapshot_tree(root), nested)

    def test_loopback_records_admitted_completion(self) -> None:
        with LocalEndpoint() as endpoint:
            request = urllib.request.Request(
                endpoint.url + "/v1/chat/completions",
                data=json.dumps({"max_tokens": 7}).encode(),
                headers={"Content-Type": "application/json"},
            )
            with urllib.request.urlopen(request, timeout=2) as response:
                self.assertEqual(response.status, 200)
            self.assertEqual(
                endpoint.state.chat_request_snapshot(), ({"max_tokens": 7},)
            )

    def test_launch_error_names_the_incompatible_binary(self) -> None:
        with tempfile.TemporaryDirectory() as bed:
            incompatible = Path(bed) / "incompatible"
            incompatible.write_text("not an executable format")
            incompatible.chmod(0o700)
            with self.assertRaisesRegex(VerificationError, str(incompatible)):
                _run_bounded([str(incompatible)], os.environ.copy(), 1)

    def test_bounded_process_kills_timed_out_group(self) -> None:
        with self.assertRaisesRegex(VerificationError, "timed out"):
            _run_bounded(["/bin/sh", "-c", "sleep 30"], os.environ.copy(), 0.05)


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--describe-bin", type=Path)
    parser.add_argument("--generate-bin", type=Path)
    parser.add_argument("--fixture", type=Path, default=DEFAULT_FIXTURE)
    parser.add_argument("--timeout-seconds", type=float, default=30.0)
    parser.add_argument(
        "--self-test", action="store_true", help="run deterministic helper tests"
    )
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = _parser().parse_args(argv)
    if args.self_test:
        suite = unittest.defaultTestLoader.loadTestsFromTestCase(HelperTests)
        result = unittest.TextTestRunner(verbosity=2).run(suite)
        return 0 if result.wasSuccessful() else 1
    if args.describe_bin is None or args.generate_bin is None:
        raise VerificationError("--describe-bin and --generate-bin are required")
    verify(args.describe_bin, args.generate_bin, args.fixture, args.timeout_seconds)
    print("Describe real-child Journal transport verified")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except VerificationError as error:
        print(f"verify-describe-journal-child: {error}", file=sys.stderr)
        raise SystemExit(1) from error
