# Push Device Registry & Notification Sealing

## Summary

`solstone-core-push` owns the local push-device registry, the native
`/api/push/*` routes, and notification envelope sealing. The registry records
where a future delivery system may reach a linked device; this repository does
not implement hosted relay or notification delivery. This crate does not send
notifications.

## Domain ownership

`core/crates/solstone-core-push/` is the sole writer for
`journal/config/push-registry.json`. The retired
`journal/config/push_devices.json` path is not read, written, or migrated.

The registry is one JSON document containing an array of registered devices:

```json
{
  "version": 2,
  "devices": [
    {
      "platform": "ios",
      "cid": "sha256:...",
      "device_token": "0123456789abcdef",
      "bundle_id": "org.solpbc.solstone-swift",
      "environment": "development",
      "push_key": "KioqKioqKioqKioqKioqKioqKioqKioqKioqKioqKio",
      "registered_at": "2026-09-24T00:00:00Z"
    }
  ]
}
```

Rows accumulate as tokens rotate until a sender prunes them. A token can belong
to only one CID, so registering an already-held token removes its previous row
on another CID while retaining other tokens for the registering device.
The store holds one sidecar lock across every read-modify-atomic-write
mutation (`0o600` mode). Existing malformed registry data is unavailable (`503
push_registry_unavailable`), not silently treated as an empty registry.

### Migration from v1

A legacy v1 registry (keyed by CID without a version field or `push_key`) is
recognized during read and treated as empty without mutating the file or logging.
When a new registration triggers a write to a journal with a non-empty v1
registry, the discarded count is logged and the file is atomically rewritten as
v2. A journal build from before this format cannot read v2 and returns `503
push_registry_unavailable` on push routes. An app build that predates `push_key`
gets `400` on register and on DELETE. Nothing sends notifications before such an
app is updated.

## Routes

All routes are mounted by Convey's shell and receive its normal session and
door controls.

### `POST /api/push/register`

The handler first requires an `AccessBasis::LinkedDevice`; localhost, pairing
peers, and missing identities receive `403 linked_device_required` before the
request body is examined. The identity comes from the connection CID, never
from JSON.

The JSON object requires `platform` (`ios`), `device_token` (lowercase even hex
string of length 16..=200), `bundle_id` (non-blank after trim), `environment`
(`development` or `production`), and `push_key` (32 bytes unpadded URL-safe
base64). Invalid or extra fields return `400 push_request_invalid` naming the
failing field.

Success returns `201 Created` for a new `(cid, device_token)` pair or `200 OK`
when refreshing an existing pair:

```json
{
  "platform": "ios",
  "target": "...cdef",
  "environment": "development",
  "registered_at": "2026-09-24T00:00:00Z"
}
```

### `DELETE /api/push/register`

Requires `AccessBasis::LinkedDevice`. Body requires exactly `platform` and
`device_token`. Removes only that `(cid, device_token)` pair. Returns `204 No
Content` with an empty body, even when no row matched.

### `GET /api/push/status`

Status reports the local registry without authentication, newest
registration first:

```json
{
  "items": [
    {
      "platform": "ios",
      "target": "...cdef",
      "environment": "development",
      "registered_at": "2026-09-24T00:00:00Z"
    }
  ],
  "total": 1,
  "cursor": null
}
```

### `POST /api/push/test`

Unauthenticated round-trip device check. With 0 devices (or an unmigrated v1
file) it returns `503 feature_unavailable` with detail `no devices to reach`.
With one or more devices it returns:

```json
{
  "device_count": 1
}
```

## Notification Envelope

Envelopes are encrypted and padded for delivery privacy:

```
envelope = 0x01 || nonce(12) || AES-256-GCM(key, nonce, padded, aad = [0x01]) || tag(16)  // 1053 bytes
padded   = utf8(json) || 0x80 || 0x00... to exactly 1024 bytes
```

Plaintext notifications serialize canonical JSON:

```json
{"v":1,"at":"2026-09-24T00:00:00Z","kind":"test","title":"solstone","body":"...","open":"/app/..."}
```

`open` is optional and validated to start with `/app/` without relative navigation
segments (`.` or `..`). Sealed envelopes serialize as 1404-character URL-safe
unpadded base64 strings.
