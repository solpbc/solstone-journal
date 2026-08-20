# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

from tools.journal_device_sim.http_client import (
    BridgeHttpClient,
    HttpRequestError,
    MultipartPart,
    multipart_length,
)


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


if __name__ == "__main__":
    unittest.main()
