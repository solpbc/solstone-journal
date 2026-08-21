# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import hashlib
import io
import json
import os
import tempfile
import unittest
from pathlib import Path

from tools.journal_device_sim.http_client import (
    BridgeHttpClient,
    HttpRequestError,
    HttpResponseError,
    MAX_RESPONSE_BYTES,
    MultipartPart,
    _send_part_data,
    multipart_length,
)


class RecordingConnection:
    def __init__(self) -> None:
        self.sent = bytearray()

    def send(self, data: bytes) -> None:
        self.sent.extend(data)


class StubResponse:
    def __init__(
        self,
        *,
        status: int,
        body: bytes,
        content_length: str | None = None,
    ) -> None:
        self.status = status
        self.body = body
        self.content_length = content_length
        self.read_calls = 0

    def getheader(self, name: str) -> str | None:
        if name == "Content-Length":
            return self.content_length
        return None

    def read(self, amount: int) -> bytes:
        self.read_calls += 1
        return self.body[:amount]


class SocketFailureConnection:
    def request(self, *_args: object, **_kwargs: object) -> None:
        raise ConnectionRefusedError("test refusal")

    def close(self) -> None:
        pass


class StubConnection:
    def __init__(self, response: StubResponse) -> None:
        self.response = response

    def request(self, *_args: object, **_kwargs: object) -> None:
        pass

    def getresponse(self) -> StubResponse:
        return self.response

    def close(self) -> None:
        pass


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
        for malformed in ("http://127.0.0.1:99999", "http://[::1"):
            with self.subTest(malformed=malformed), self.assertRaisesRegex(
                HttpRequestError, "malformed"
            ):
                BridgeHttpClient(malformed)

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

    def test_malformed_content_length_has_received_response_receipt(self) -> None:
        response = StubResponse(status=502, body=b"{}", content_length="12x")

        with self.assertRaises(HttpResponseError) as caught:
            BridgeHttpClient("http://127.0.0.1")._read_response(  # type: ignore[arg-type]
                response
            )

        self.assertEqual(
            caught.exception.receipt,
            {"status": 502, "reason_category": "malformed_content_length"},
        )
        self.assertEqual(response.read_calls, 0)
        json.dumps(caught.exception.receipt)

    def test_declared_oversize_is_rejected_without_reading_body(self) -> None:
        declared = MAX_RESPONSE_BYTES + 1
        response = StubResponse(
            status=413,
            body=b"unread secret body",
            content_length=str(declared),
        )

        with self.assertRaises(HttpResponseError) as caught:
            BridgeHttpClient("http://127.0.0.1")._read_response(  # type: ignore[arg-type]
                response
            )

        self.assertEqual(
            caught.exception.receipt,
            {
                "status": 413,
                "reason_category": "declared_response_too_large",
                "declared_length": declared,
            },
        )
        self.assertEqual(response.read_calls, 0)

    def test_actual_oversize_receipt_hashes_only_bounded_read(self) -> None:
        raw = b"x" * (MAX_RESPONSE_BYTES + 1)
        response = StubResponse(status=502, body=raw)

        with self.assertRaises(HttpResponseError) as caught:
            BridgeHttpClient("http://127.0.0.1")._read_response(  # type: ignore[arg-type]
                response
            )

        self.assertEqual(
            caught.exception.receipt,
            {
                "status": 502,
                "reason_category": "actual_response_too_large",
                "actual_length": MAX_RESPONSE_BYTES + 1,
                "raw_sha256": hashlib.sha256(raw).hexdigest(),
            },
        )

    def test_non_json_receipt_has_status_length_and_digest_not_body(self) -> None:
        raw = b"gateway exploded: internal detail"
        response = StubResponse(
            status=502,
            body=raw,
            content_length=str(len(raw)),
        )

        with self.assertRaises(HttpResponseError) as caught:
            BridgeHttpClient("http://127.0.0.1")._read_response(  # type: ignore[arg-type]
                response
            )

        self.assertEqual(
            caught.exception.receipt,
            {
                "status": 502,
                "reason_category": "non_json_body",
                "declared_length": len(raw),
                "actual_length": len(raw),
                "raw_sha256": hashlib.sha256(raw).hexdigest(),
            },
        )

    def test_nonfinite_json_is_rejected_as_a_received_contract_error(self) -> None:
        raw = b'{"value":NaN}'
        response = StubResponse(
            status=200,
            body=raw,
            content_length=str(len(raw)),
        )
        with self.assertRaises(HttpResponseError) as caught:
            BridgeHttpClient("http://127.0.0.1")._read_response(  # type: ignore[arg-type]
                response
            )
        self.assertEqual(caught.exception.reason_category, "non_json_body")
        self.assertEqual(caught.exception.raw_sha256, hashlib.sha256(raw).hexdigest())
        self.assertNotIn(raw.decode("ascii"), str(caught.exception))
        self.assertNotIn(raw.decode("ascii"), json.dumps(caught.exception.receipt))
        self.assertIsNone(caught.exception.__cause__)

    def test_nonobject_json_receipt_retains_http_metadata(self) -> None:
        raw = b'["not", "an", "object"]'
        response = StubResponse(status=418, body=raw)

        with self.assertRaises(HttpResponseError) as caught:
            BridgeHttpClient("http://127.0.0.1")._read_response(  # type: ignore[arg-type]
                response
            )

        self.assertEqual(caught.exception.receipt["status"], 418)
        self.assertEqual(
            caught.exception.receipt["reason_category"], "non_object_json"
        )
        self.assertEqual(caught.exception.receipt["actual_length"], len(raw))
        self.assertEqual(
            caught.exception.receipt["raw_sha256"], hashlib.sha256(raw).hexdigest()
        )

    def test_valid_json_object_response_is_unchanged(self) -> None:
        response = StubResponse(status=207, body=b'{"ok":true}')

        result = BridgeHttpClient("http://127.0.0.1")._read_response(  # type: ignore[arg-type]
            response
        )

        self.assertEqual(result.status, 207)
        self.assertEqual(result.body, {"ok": True})

    def test_received_invalid_response_stays_typed_at_public_boundary(self) -> None:
        client = BridgeHttpClient("http://127.0.0.1")
        response = StubResponse(status=502, body=b"not JSON")
        client._connection = lambda: StubConnection(response)  # type: ignore[method-assign,return-value]

        with self.assertRaises(HttpResponseError) as caught:
            client.get_json("/app/link/api/status")

        self.assertEqual(caught.exception.status, 502)
        self.assertEqual(caught.exception.reason_category, "non_json_body")

    def test_socket_transport_failure_stays_base_request_error(self) -> None:
        client = BridgeHttpClient("http://127.0.0.1")
        client._connection = lambda: SocketFailureConnection()  # type: ignore[method-assign,return-value]

        with self.assertRaises(HttpRequestError) as caught:
            client.get_json("/app/link/api/status")

        self.assertIs(type(caught.exception), HttpRequestError)


if __name__ == "__main__":
    unittest.main()
