# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import tempfile
import hashlib
import io
import os
import unittest
from pathlib import Path

from tools.journal_device_sim.http_client import (
    BridgeHttpClient,
    HttpRequestError,
    MultipartPart,
    _send_part_data,
    multipart_length,
)


class RecordingConnection:
    def __init__(self) -> None:
        self.sent = bytearray()

    def send(self, data: bytes) -> None:
        self.sent.extend(data)


class HttpClientTests(unittest.TestCase):
    def test_bridge_origin_is_explicit_loopback(self) -> None:
        BridgeHttpClient("http://127.0.0.1:5015")
        BridgeHttpClient("http://[::1]:5015")
        with self.assertRaisesRegex(HttpRequestError, "loopback"):
            BridgeHttpClient("http://localhost:5015")
        with self.assertRaisesRegex(HttpRequestError, "loopback"):
            BridgeHttpClient("http://journal.example:5015")
        with self.assertRaisesRegex(HttpRequestError, "unauthenticated"):
            BridgeHttpClient("https://127.0.0.1:5015")

    def test_multipart_length_matches_materialized_body(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            payload = Path(temporary) / "payload.bin"
            payload.write_bytes(b"abc\x00def")
            boundary = "test-boundary"
            parts = [
                MultipartPart("envelope", None, b"{}", "application/json"),
                MultipartPart("files", "café.bin", payload, "application/octet-stream"),
            ]
            expected = 0
            for part in parts:
                disposition = f'Content-Disposition: form-data; name="{part.name}"'
                if part.filename:
                    disposition += f'; filename="{part.filename}"'
                prefix = (
                    f"--{boundary}\r\n{disposition}\r\n"
                    f"Content-Type: {part.content_type}\r\n\r\n"
                ).encode("utf-8")
                data = (
                    part.data
                    if isinstance(part.data, bytes)
                    else part.data.read_bytes()
                )
                expected += len(prefix) + len(data) + 2
            expected += len(f"--{boundary}--\r\n".encode("ascii"))
            self.assertEqual(multipart_length(boundary, parts), expected)

    def test_streaming_part_sends_exact_declared_bytes(self) -> None:
        payload = b"manifest-pinned\x00bytes"
        connection = RecordingConnection()
        _send_part_data(
            connection,  # type: ignore[arg-type]
            MultipartPart(
                "files",
                "capture.bin",
                io.BytesIO(payload),
                "application/octet-stream",
                size=len(payload),
                sha256=hashlib.sha256(payload).hexdigest(),
            ),
        )
        self.assertEqual(bytes(connection.sent), payload)

    def test_streaming_part_refuses_short_and_long_data(self) -> None:
        cases = ((b"a", 2, "shorter"), (b"abc", 2, "longer"))
        for payload, declared, message in cases:
            with self.subTest(message=message):
                part = MultipartPart(
                    "files",
                    "capture.bin",
                    io.BytesIO(payload),
                    "application/octet-stream",
                    size=declared,
                    sha256=hashlib.sha256(payload[:declared]).hexdigest(),
                )
                with self.assertRaisesRegex(HttpRequestError, message):
                    _send_part_data(RecordingConnection(), part)  # type: ignore[arg-type]

    def test_streaming_part_refuses_same_inode_content_mutation(self) -> None:
        original = b"manifest-bytes"
        changed = b"mutated-bytes!"
        self.assertEqual(len(original), len(changed))
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "capture.bin"
            path.write_bytes(original)
            with path.open("rb") as upload:
                before = os.fstat(upload.fileno())
                with path.open("r+b") as mutator:
                    mutator.write(changed)
                    mutator.flush()
                after = os.fstat(upload.fileno())
                self.assertEqual(
                    (before.st_dev, before.st_ino),
                    (after.st_dev, after.st_ino),
                )
                part = MultipartPart(
                    "files",
                    path.name,
                    upload,
                    "application/octet-stream",
                    size=len(original),
                    sha256=hashlib.sha256(original).hexdigest(),
                )
                with self.assertRaisesRegex(HttpRequestError, "digest changed"):
                    _send_part_data(
                        RecordingConnection(), part  # type: ignore[arg-type]
                    )


if __name__ == "__main__":
    unittest.main()
