# Watched Folder Source Contract (`source = "folder"`)

This directory documents the linked-device protocol-v3 contract for the watched
folder source.

The authoritative machine-readable OpenAPI specification and generated golden
envelopes live in:

- `core/crates/solstone-core-repository-contracts/src/contracts/client_ingest_authority.json` (`x-sources.folder`)
- `docs/openapi/client-ingest-contract/projection.openapi.json`
- `docs/openapi/client-ingest-contract/fixtures/folder-golden.jsonl`

## Capability Gate

Clients send watched-folder requests only to a journal whose
`GET /api/system/status` lists `"folder"` in `capabilities`.

## Endpoints (Linked Device mTLS + `X-Solstone-Protocol-Version: 3`)

- `POST /app/devices/ingest` (`source = "folder"`): Upload one 5-minute window
  (`HHMMSS_300` on the journal's 300-second grid) for one folder. Multipart
  parts are one `envelope` JSON part, one `folder_<sha256>.jsonl` activity file,
  and 0–63 companion content parts referenced by `part` fields on lines in the
  activity file.
- `GET /app/devices/ingest/segments/{day}?source=folder`: List uploaded segments
  for all watched folders bound to the calling device on `day`.
- `GET /app/devices/ingest/folder/supported`: Published supported file
  extensions, limits, triage ruleset version, document revision cooldown, and
  per-kind ingestion modes.
- `POST /app/devices/ingest/folder/estimate`: Backfill preview estimate in
  `items`, `segments`, `days`, and approximate model passes (`thoughts`),
  excluding items already recorded in the folder's received ledger, plus
  `backfill_paused`.
- `GET /app/devices/ingest/folder/facets`: List enabled (non-muted) facets on
  the journal for the optional owner-chosen folder facet.

## Semantics

- **One segment per folder per 5-minute window**: Each folder binds its own
  internal stream keyed by `meta.folder.id` (`folder_<12 hex>`), so concurrent
  folders never collide.
- **Activity record (`talents/screen_text.md`)**: Rendered deterministically
  from the activity JSONL with credential redaction and column-0 timed headings.
  Activity-only folder segments are indexed immediately under `screen_text` and
  schedule zero talents and zero model passes.
- **Content routing (`kind_hint` + triage)**: When a companion content part
  carries a known `kind_hint` (`notes`, `documents`, `transcripts`,
  `screenshots`, `photos`, `screen_text`), the journal honors it; absent or
  unrecognized hints fall back to deterministic triage (`ruleset_version = 8`).
- **Per-file checking**: Broken companion files are refused by name in
  `refused_files` while valid activity events and files in the same window land
  (`200 OK`). If every item in the upload is refused, the journal returns `400`
  with `reason_code: "folder_file_invalid"` and `refused_files`.
- **Erase-surviving received ledger**: Stored per `meta.folder.id` under
  `config/folders/<hash>/received/` and read fail-closed (`503 retryable` when
  unreadable). Re-sending an already-received event, screenshot, photo, or
  document revision — including after segment erase or re-pairing — returns
  `200 OK` with `status: "duplicate"`.
- **Owner-chosen facet**: Optional `meta.folder.facet` is applied to every
  segment of that folder. Content processing may add other facets and never
  removes the owner's. Without an owner-chosen facet, activity-only segments
  are visible only to whole-journal connections.
- **Historical backfill**: Folder days older than today (`day < today`) schedule
  in Band 4 (`WorkBand::Import`) alongside other syncing sources.
