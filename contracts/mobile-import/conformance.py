#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
"""Validate the mobile-import resumable upload contract and live wire invariants."""

import argparse
import hashlib
import http.client
import json
from pathlib import Path
import socket
import sys
from urllib.parse import urlsplit
import uuid


CONTRACT_DIR = Path(__file__).resolve().parent
SCHEMA_PATH = CONTRACT_DIR / "v1.schema.json"
VECTORS_PATH = CONTRACT_DIR / "v1.vectors.json"


def fail(message: str) -> None:
    print(f"FAIL: {message}", file=sys.stderr)
    raise SystemExit(1)


def check(condition: bool, message: str) -> None:
    if not condition:
        fail(message)


def validate_contract_files() -> None:
    schema = json.loads(SCHEMA_PATH.read_text(encoding="utf-8"))
    vectors_doc = json.loads(VECTORS_PATH.read_text(encoding="utf-8"))
    vectors = vectors_doc["vectors"]
    policy = schema["x-solstone-resumable-policy"]

    check(policy["capability"] == "resumable_import_uploads", "capability name mismatch")
    check(policy["threshold_bytes"] == 16 * 1024 * 1024, "threshold_bytes must be 16 MiB")
    check(policy["chunk_bytes"] == 4 * 1024 * 1024, "chunk_bytes must be 4 MiB")
    check(policy["max_file_bytes"] == 4 * 1024 * 1024 * 1024, "max_file_bytes must be 4 GiB")
    check(policy["bridge_drain_ceiling_bytes"] == 8 * 1024 * 1024, "drain ceiling must be 8 MiB")
    check(policy["min_free_disk_bytes"] == 256 * 1024 * 1024, "free-space floor must be 256 MiB")
    check(policy["max_active_uploads_bytes"] == 8 * 1024 * 1024 * 1024, "active cap must be 8 GiB")
    check(policy["retry_backoff_seconds"] == [1, 2, 5, 10, 30, 60], "retry ladder mismatch")
    check(
        policy["fallback_on_404_or_405_when_capability_absent"] is True,
        "404/405 fallback policy missing",
    )

    framing = policy["chunk_wire_framing"]
    check(framing["content_length_required"] is True, "Content-Length requirement missing")
    check(framing["transfer_encoding_chunked_forbidden"] is True, "chunked prohibition missing")
    check(framing["expect_100_continue_forbidden"] is True, "100-continue prohibition missing")
    check(framing["chunk_sha256_header"] == "X-Chunk-Sha256", "chunk checksum header mismatch")

    routes = schema["x-solstone-routes"]
    for key in (
        "uploads_create",
        "uploads_list",
        "uploads_status",
        "uploads_chunk",
        "uploads_commit",
        "uploads_abort",
    ):
        check(key in routes, f"missing route {key}")

    check(
        vectors["upload_committed_200"]["body"] == vectors["save_staged_200"]["body"],
        "upload_committed_200 must match save_staged_200 handoff shape",
    )
    print("PASS: contract schema and vectors verified")


def request_json(
    host: str,
    port: int,
    method: str,
    path: str,
    body: bytes | None = None,
    headers: dict[str, str] | None = None,
) -> tuple[int, dict]:
    conn = http.client.HTTPConnection(host, port, timeout=30)
    try:
        req_headers = dict(headers or {})
        if body is not None and "Content-Length" not in req_headers:
            req_headers["Content-Length"] = str(len(body))
        conn.request(method, path, body=body, headers=req_headers)
        resp = conn.getresponse()
        raw = resp.read()
        parsed = json.loads(raw.decode("utf-8")) if raw else {}
        return resp.status, parsed
    finally:
        conn.close()


def send_raw_http(host: str, port: int, raw_request: bytes) -> tuple[int, dict]:
    with socket.create_connection((host, port), timeout=30) as sock:
        sock.sendall(raw_request)
        chunks: list[bytes] = []
        while True:
            data = sock.recv(65536)
            if not data:
                break
            chunks.append(data)
    response_bytes = b"".join(chunks)
    header_blob, _, body_blob = response_bytes.partition(b"\r\n\r\n")
    status_line = header_blob.split(b"\r\n", 1)[0].decode("latin1")
    status_code = int(status_line.split()[1])
    parsed = json.loads(body_blob.decode("utf-8")) if body_blob.strip() else {}
    return status_code, parsed


def send_paced_put_and_assert_drained(
    host: str,
    port: int,
    path: str,
    payload: bytes,
    extra_headers: dict[str, str] | None = None,
) -> tuple[int, dict]:
    """Stream body in multiple TCP writes to verify server drains before returning 4xx."""
    with socket.create_connection((host, port), timeout=30) as sock:
        lines = [
            f"PUT {path} HTTP/1.1",
            f"Host: {host}:{port}",
            "Connection: close",
            "Content-Type: application/octet-stream",
            f"Content-Length: {len(payload)}",
        ]
        for k, v in (extra_headers or {}).items():
            lines.append(f"{k}: {v}")
        header_bytes = ("\r\n".join(lines) + "\r\n\r\n").encode("ascii")
        sock.sendall(header_bytes)

        step = max(1, len(payload) // 8)
        for offset in range(0, len(payload), step):
            sock.sendall(payload[offset : offset + step])

        chunks: list[bytes] = []
        while True:
            data = sock.recv(65536)
            if not data:
                break
            chunks.append(data)

    raw = b"".join(chunks)
    header_blob, _, body_blob = raw.partition(b"\r\n\r\n")
    status_line = header_blob.split(b"\r\n", 1)[0].decode("latin1")
    status_code = int(status_line.split()[1])
    parsed = json.loads(body_blob.decode("utf-8"))
    return status_code, parsed


def run_live_conformance(origin: str) -> None:
    parsed_url = urlsplit(origin)
    check(parsed_url.scheme == "http", "conformance --origin expects an http:// loopback test origin")
    host = parsed_url.hostname or "127.0.0.1"
    port = parsed_url.port or 80

    # 1. Capability gate & sources limits
    status, sys_body = request_json(host, port, "GET", "/api/system/status")
    check(status == 200, f"GET /api/system/status returned {status}")
    check(
        "resumable_import_uploads" in sys_body.get("capabilities", []),
        "GET /api/system/status missing resumable_import_uploads capability",
    )

    status, sources_body = request_json(host, port, "GET", "/app/import/api/sources")
    check(status == 200, f"GET /app/import/api/sources returned {status}")
    resumable = sources_body.get("resumable", {})
    check(resumable.get("threshold_bytes") == 16 * 1024 * 1024, "sources threshold_bytes != 16 MiB")
    check(resumable.get("chunk_bytes") == 4 * 1024 * 1024, "sources chunk_bytes != 4 MiB")
    check(resumable.get("max_file_bytes") == 4 * 1024 * 1024 * 1024, "sources max_file_bytes != 4 GiB")
    print("PASS [1/11]: capability advertisement on /api/system/status and /app/import/api/sources")

    # Prepare synthetic 2-chunk payload
    run_tag = uuid.uuid4().hex[:12]
    chunk_0 = (f"conformance-{run_tag}-chunk-0-".encode("utf-8") * 4096)[:65536]
    chunk_1 = (f"conformance-{run_tag}-chunk-1-".encode("utf-8") * 2048)[:31111]
    full_bytes = chunk_0 + chunk_1
    full_sha = hashlib.sha256(full_bytes).hexdigest()
    chunk_0_sha = hashlib.sha256(chunk_0).hexdigest()
    chunk_1_sha = hashlib.sha256(chunk_1).hexdigest()
    client_item_id = f"conformance-item-{run_tag}"

    # 2. Create session
    status, created = request_json(
        host,
        port,
        "POST",
        "/app/import/api/uploads",
        body=json.dumps(
            {
                "client_item_id": client_item_id,
                "filename": f"conformance-{run_tag}.md",
                "mime_type": "text/markdown",
                "total_bytes": len(full_bytes),
                "sha256": full_sha,
                "imported_via": "desktop",
                "device_noun": "mac",
            }
        ).encode("utf-8"),
        headers={"Content-Type": "application/json"},
    )
    check(status == 201, f"POST /uploads expected 201, got {status}: {created}")
    upload_id = created["upload_id"]
    check(created["next_offset"] == 0, f"expected next_offset 0, got {created}")
    print("PASS [2/11]: POST /app/import/api/uploads created open session")

    # 3. Transfer-Encoding: chunked without Content-Length -> 400
    chunked_req = (
        f"PUT /app/import/api/uploads/{upload_id}?offset=0 HTTP/1.1\r\n"
        f"Host: {host}:{port}\r\n"
        "Connection: close\r\n"
        "Content-Type: application/octet-stream\r\n"
        "Transfer-Encoding: chunked\r\n\r\n"
        "4\r\nabcd\r\n0\r\n\r\n"
    ).encode("ascii")
    status, te_err = send_raw_http(host, port, chunked_req)
    check(
        status == 400 and te_err.get("reason_code") == "content_length_required",
        f"expected 400 content_length_required on chunked PUT, got {status}: {te_err}",
    )
    print("PASS [3/11]: Transfer-Encoding: chunked rejected with 400 content_length_required")

    # 4. Expect: 100-continue -> 417
    expect_req = (
        f"PUT /app/import/api/uploads/{upload_id}?offset=0 HTTP/1.1\r\n"
        f"Host: {host}:{port}\r\n"
        "Connection: close\r\n"
        "Content-Type: application/octet-stream\r\n"
        "Content-Length: 4\r\n"
        "Expect: 100-continue\r\n\r\n"
    ).encode("ascii")
    status, exp_err = send_raw_http(host, port, expect_req)
    check(
        status == 417 and exp_err.get("reason_code") == "invalid_chunk_framing",
        f"expected 417 invalid_chunk_framing on Expect: 100-continue, got {status}: {exp_err}",
    )
    print("PASS [4/11]: Expect: 100-continue rejected with 417 invalid_chunk_framing")

    # 5. Drain-before-422 on bad X-Chunk-Sha256
    status, bad_chunk = send_paced_put_and_assert_drained(
        host,
        port,
        f"/app/import/api/uploads/{upload_id}?offset=0",
        chunk_0,
        {"X-Chunk-Sha256": "0" * 64},
    )
    check(
        status == 422
        and bad_chunk.get("reason_code") == "chunk_checksum_mismatch"
        and bad_chunk.get("next_offset") == 0,
        f"expected 422 chunk_checksum_mismatch with next_offset 0, got {status}: {bad_chunk}",
    )
    print("PASS [5/11]: X-Chunk-Sha256 mismatch drained full body and returned 422 JSON")

    # 6. Upload chunk 0 and verify active_uploads visibility
    status, put_0 = request_json(
        host,
        port,
        "PUT",
        f"/app/import/api/uploads/{upload_id}?offset=0",
        body=chunk_0,
        headers={
            "Content-Type": "application/octet-stream",
            "X-Chunk-Sha256": chunk_0_sha,
        },
    )
    check(
        status == 200 and put_0.get("next_offset") == len(chunk_0) and put_0.get("complete") is False,
        f"chunk 0 PUT failed: {status} {put_0}",
    )
    status, list_uploads = request_json(host, port, "GET", "/app/import/api/uploads")
    check(
        status == 200 and any(u["upload_id"] == upload_id for u in list_uploads.get("items", [])),
        "active upload missing from GET /app/import/api/uploads",
    )
    status, import_list = request_json(host, port, "GET", "/app/import/api/list")
    check(
        status == 200
        and any(u["upload_id"] == upload_id for u in import_list.get("active_uploads", [])),
        "active upload missing from GET /app/import/api/list active_uploads",
    )
    print("PASS [6/11]: chunk 0 persisted and visible in /uploads and /list active_uploads")

    # 7. Drain-before-409 on stale offset
    status, mismatch_409 = send_paced_put_and_assert_drained(
        host,
        port,
        f"/app/import/api/uploads/{upload_id}?offset=0",
        chunk_0,
        {"X-Chunk-Sha256": chunk_0_sha},
    )
    check(
        status == 409
        and mismatch_409.get("reason_code") == "offset_mismatch"
        and mismatch_409.get("next_offset") == len(chunk_0),
        f"expected 409 offset_mismatch with next_offset {len(chunk_0)}, got {status}: {mismatch_409}",
    )
    print("PASS [7/11]: stale offset PUT drained full body and returned 409 next_offset")

    # 8. Resync via GET /uploads/{id} + idempotent POST /uploads, then send ONLY chunk 1
    status, status_doc = request_json(host, port, "GET", f"/app/import/api/uploads/{upload_id}")
    check(
        status == 200 and status_doc.get("next_offset") == len(chunk_0),
        f"GET /uploads/{{id}} resync failed: {status} {status_doc}",
    )
    status, resumed = request_json(
        host,
        port,
        "POST",
        "/app/import/api/uploads",
        body=json.dumps(
            {
                "client_item_id": client_item_id,
                "filename": f"conformance-{run_tag}.md",
                "total_bytes": len(full_bytes),
                "sha256": full_sha,
            }
        ).encode("utf-8"),
        headers={"Content-Type": "application/json"},
    )
    check(
        status == 200
        and resumed.get("upload_id") == upload_id
        and resumed.get("next_offset") == len(chunk_0),
        f"idempotent POST /uploads resume failed: {status} {resumed}",
    )
    status, put_1 = request_json(
        host,
        port,
        "PUT",
        f"/app/import/api/uploads/{upload_id}?offset={len(chunk_0)}",
        body=chunk_1,
        headers={
            "Content-Type": "application/octet-stream",
            "X-Chunk-Sha256": chunk_1_sha,
        },
    )
    check(
        status == 200 and put_1.get("next_offset") == len(full_bytes) and put_1.get("complete") is True,
        f"chunk 1 PUT failed: {status} {put_1}",
    )
    print("PASS [8/11]: resynced and resumed from chunk 1 without re-sending chunk 0")

    # 9. Commit from persisted midstate & verify pre-upload already_staged deduplication
    status, committed = request_json(
        host,
        port,
        "POST",
        f"/app/import/api/uploads/{upload_id}/commit",
        body=b"{}",
        headers={"Content-Type": "application/json"},
    )
    check(
        status == 200
        and committed.get("status") == "staged"
        and committed.get("recommended_action") == "start"
        and committed.get("client_item_id") == client_item_id,
        f"commit failed: {status} {committed}",
    )
    staged_file = Path(committed["path"])
    check(staged_file.is_file() and staged_file.read_bytes() == full_bytes, "staged file bytes mismatch")

    status, dedup = request_json(
        host,
        port,
        "POST",
        "/app/import/api/uploads",
        body=json.dumps(
            {
                "client_item_id": client_item_id,
                "filename": f"conformance-{run_tag}.md",
                "total_bytes": len(full_bytes),
                "sha256": full_sha,
            }
        ).encode("utf-8"),
        headers={"Content-Type": "application/json"},
    )
    check(
        status == 200 and dedup.get("status") == "already_staged" and dedup.get("replay") is True,
        f"expected 200 already_staged on repeat POST /uploads, got {status}: {dedup}",
    )
    print("PASS [9/11]: midstate SHA-256 commit staged exact bytes and deduplicated repeat create")

    # 10. Whole-file checksum mismatch -> 422 and removes partial dir
    status, bad_open = request_json(
        host,
        port,
        "POST",
        "/app/import/api/uploads",
        body=json.dumps(
            {
                "client_item_id": f"bad-whole-{run_tag}",
                "filename": "corrupt.txt",
                "total_bytes": 16,
            }
        ).encode("utf-8"),
        headers={"Content-Type": "application/json"},
    )
    check(status == 201, f"bad-whole create failed: {status} {bad_open}")
    bad_id = bad_open["upload_id"]
    status, _ = request_json(
        host,
        port,
        "PUT",
        f"/app/import/api/uploads/{bad_id}?offset=0",
        body=b"0123456789abcdef",
        headers={"Content-Type": "application/octet-stream"},
    )
    check(status == 200, "bad-whole chunk PUT failed")
    status, whole_err = request_json(
        host,
        port,
        "POST",
        f"/app/import/api/uploads/{bad_id}/commit",
        body=json.dumps({"sha256": "f" * 64}).encode("utf-8"),
        headers={"Content-Type": "application/json"},
    )
    check(
        status == 422 and whole_err.get("reason_code") == "whole_file_checksum_mismatch",
        f"expected 422 whole_file_checksum_mismatch, got {status}: {whole_err}",
    )
    status, after_bad = request_json(host, port, "GET", f"/app/import/api/uploads/{bad_id}")
    check(status == 404, f"corrupt partial upload should be deleted after 422, got {status}")
    print("PASS [10/11]: whole-file SHA-256 mismatch rejected with 422 and purged partial copy")

    # 11. Explicit DELETE cancel
    status, cancel_open = request_json(
        host,
        port,
        "POST",
        "/app/import/api/uploads",
        body=json.dumps(
            {
                "client_item_id": f"cancel-{run_tag}",
                "filename": "cancel.bin",
                "total_bytes": 1024,
            }
        ).encode("utf-8"),
        headers={"Content-Type": "application/json"},
    )
    check(status == 201, f"cancel create failed: {status}")
    cancel_id = cancel_open["upload_id"]
    status, del_body = request_json(host, port, "DELETE", f"/app/import/api/uploads/{cancel_id}")
    check(status == 200 and del_body.get("deleted") is True, f"DELETE failed: {status} {del_body}")
    status, _ = request_json(host, port, "GET", f"/app/import/api/uploads/{cancel_id}")
    check(status == 404, "cancelled upload still returned 200")
    print("PASS [11/11]: DELETE /app/import/api/uploads/{id} cancelled and cleaned up")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--validate-contract",
        action="store_true",
        help="verify v1.schema.json and v1.vectors.json invariants offline",
    )
    parser.add_argument(
        "--origin",
        help="disposable loopback journal origin (e.g. http://127.0.0.1:19400) for live wire checks",
    )
    args = parser.parse_args()
    if not args.validate_contract and not args.origin:
        parser.error("pass --validate-contract and/or --origin http://127.0.0.1:<port>")

    if args.validate_contract:
        validate_contract_files()
    if args.origin:
        run_live_conformance(args.origin)


if __name__ == "__main__":
    main()
