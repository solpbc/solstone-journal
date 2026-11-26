# solstone import upload contract

`v1.schema.json` and `v1.vectors.json` define the generated client contract for bringing an owner-chosen file into a paired or local journal. Both files are generated from `core/crates/solstone-core-import-web/src/mobile_import_contract.rs` and verified in CI by `solstone-core-repository-contracts`.

Regenerate after editing `mobile_import_contract.rs`:

```sh
cargo test --manifest-path core/Cargo.toml -p solstone-core-repository-contracts --lib mobile_import_contract::regenerate_mobile_import_contract -- --ignored
```

## Capability gate and version skew

1. Check `GET /api/system/status` for `"resumable_import_uploads"` in `capabilities` (or `resumable` on `GET /app/import/api/sources`: `{ "threshold_bytes": 16777216, "chunk_bytes": 4194304, "max_file_bytes": 4294967296 }`).
2. For files below `threshold_bytes` (16 MiB), use multipart `POST /app/import/api/save` by default. When the bridge rejects a browser file with HTTP 413 and the journal advertises resumable uploads, the browser retries once through the upload routes with the same item ID.
3. Files at or above `16 MiB` (up to `4 GiB`) use `/app/import/api/uploads*` when `"resumable_import_uploads"` is advertised.
4. Fallback rule:
   - If `"resumable_import_uploads"` is **not** advertised (older journal), fall back to `POST /app/import/api/save` when the platform transport supports streaming that size, or show a clear update prompt when the platform cannot single-POST the file. Note that an older journal answers `405 Method Not Allowed` (not `404`) to `POST /app/import/api/uploads` because that path matches the `GET`-only `/app/import/api/{timestamp}` route, so treat `404` and `405` identically on any unprobed fallback check.
   - Once `"resumable_import_uploads"` **is** advertised, a `404 upload_not_found` on `PUT`, `GET`, `POST .../commit`, or `DELETE` means that `upload_id` expired or was cleaned up. Re-run `POST /app/import/api/uploads` to start a new resumable session rather than falling back to single-POST `/save`.

## Routes and state machine

1. **`POST /app/import/api/uploads`** (`application/json`)
   - Request: `upload_create_request` (`client_item_id`, `filename`, `total_bytes`, optional `sha256`, `mime_type`, `source_hint`, `imported_via`, allowlisted `device_noun`, `area`, `setting`, `caption`, `captured_at`, `observer_handle`, `context`, `client`).
   - Native apps compute whole-file `sha256` from disk before creating the session so pre-upload deduplication can return `200 already_staged` or `200 already_imported` without sending file bytes. Browser clients may omit `sha256` on create and supply it on `POST .../commit`.
   - Responses:
     - `201 Created` (`upload_open_response`): new session (`next_offset: 0`, `chunk_bytes: 4194304`).
     - `200 OK` (`upload_open_response`): resuming an existing open session for `client_item_id` from `next_offset`.
     - `200 OK` (`upload_already_staged_response` / `upload_already_imported_response`): deduplicated before byte transfer.
     - `400` / `413` / `422` / `507`: validation refusal or insufficient journal space (`total_bytes + 256 MiB` free-space floor, `8 GiB` aggregate partial-upload cap).

2. **`PUT /app/import/api/uploads/{upload_id}?offset={offset}`** (`application/octet-stream`)
   - Body: raw bytes starting at the confirmed offset. Send between one byte and the smaller of `chunk_bytes` and the remaining file length. A nonfinal PUT can be shorter than `chunk_bytes`.
   - Wire framing invariants (required for both direct HTTP and `spl-rust` mTLS bridge pass-through):
     - Exact `Content-Length` header is mandatory.
     - Never send `Transfer-Encoding: chunked` (`400 content_length_required`).
     - Never send `Expect: 100-continue` (`417 invalid_chunk_framing`).
     - Include `X-Chunk-Sha256: <64-hex>` so per-chunk corruption is rejected immediately (`422 chunk_checksum_mismatch`) without advancing `next_offset`.
     - The journal always drains bounded chunk bodies (`<= 8 MiB`) to EOF before returning any `400`, `404`, `409`, `413`, `422`, or `507` response so relay bridges never drop JSON error bodies.
   - Responses:
     - `200 OK` (`upload_chunk_response`): durable `next_offset` and updated persisted SHA-256 midstate.
     - `409 offset_mismatch`: read `next_offset` from the response JSON and seek directly there.
     - `422 chunk_checksum_mismatch`: re-read and re-send the same chunk.
     - `404 upload_not_found`: session expired or aborted; call `POST /app/import/api/uploads`.
     - `507 insufficient_journal_space`: surface low-space attention state; keep local queued item intact for retry.

3. **`GET /app/import/api/uploads/{upload_id}`** and **`GET /app/import/api/uploads`**
   - Resyncs `next_offset`, `total_bytes`, `phase` (`"sending" | "paused" | "checking"`), and `expires_at` (`7-day` rolling inactivity TTL) after a lost `PUT` response or reconnect.
   - `GET /app/import/api/list` also includes `active_uploads` with the same `upload_status_response` objects.

4. **`POST /app/import/api/uploads/{upload_id}/commit`** (`application/json` or empty body)
   - Body: `{ "sha256": "<hex>" }` (or `{}` / empty when `sha256` was already declared on `POST /uploads`).
   - Finalizes the persisted 32-byte FIPS 180-4 SHA-256 midstate without re-reading the file from disk, verifies it against `sha256` (`422 whole_file_checksum_mismatch` deletes `.uploads/{upload_id}` on mismatch), atomically moves `payload.part` into `imports/<timestamp>/<filename>`, writes `import.json`, and returns `200 OK` with the exact `save_response` payload used by `POST /app/import/api/start`.

5. **`DELETE /app/import/api/uploads/{upload_id}`**
   - Cancels an open upload, removes `.uploads/{upload_id}` immediately, and emits `upload_aborted`.

## Retry ladder, resync, and pairing-switch rule

- **Retry ladder:** On transient transport errors, timeouts, or `502`/`503`/`504`, retry chunk uploads automatically using `1s -> 2s -> 5s -> 10s -> 30s -> 60s` (`+-20%` jitter) without advancing the queue item to a terminal failure count while the device is offline or switching networks.
- **Resync before resume:** After any ambiguous `PUT` disconnect or backoff wait, call `GET /app/import/api/uploads/{upload_id}` and resume from the journal's `next_offset` so completed chunks are never re-sent.
- **Pairing-switch rule:** Persist `upload_instance_id` alongside `upload_id` and `remote_offset` on the local queue record. If the owner switches paired journals (`instance_id != upload_instance_id`), clear `upload_id` and `remote_offset` locally and start from offset `0` on the new journal while keeping the staged local file.

## Host bridge and SSE progress fields

- **Desktop / mobile WebView `import.status` (`solstone-journal-host-status`):** Hosts emit `bytes_sent`, `bytes_total`, `resumable`, `phase` (`"sending" | "paused" | "checking"`), and allowlisted `device_noun` (`iphone`, `ipad`, `phone`, `mac`, `PC`, `computer`, `browser`) alongside `item` (`== client_item_id`), `name`, and `state`.
- **Callosum / SSE (`/sse/events`, tract `"importer"`):** The journal emits `upload_progress` (throttled to `>= 500 ms` plus first and final chunk), `upload_committed`, and `upload_aborted`, keyed by `client_item_id` so client or web consumers can correlate SSE progress with host `import.status` events.

## Running the conformance check

Validate the committed schema and vectors offline:

```sh
python3 contracts/mobile-import/conformance.py --validate-contract
```

Exercise all live wire invariants against a disposable test journal origin:

```sh
python3 contracts/mobile-import/conformance.py --origin http://127.0.0.1:<port>
```
