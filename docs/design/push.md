# Push Device Registry & Notification Sealing

## Summary

`solstone-core-push` owns the local push-device registry, the local VAPID key
pair, the native `/api/push/*` routes, notification envelope sealing, hosted
iOS push delivery through the push relay, and direct Android Web Push delivery
(RFC 8030 / RFC 8291 / RFC 8292). The registry records where push
notifications may reach a linked device. Convey shell routes unpair mutations
to purge push registrations for unpaired devices.

## Domain ownership

`core/crates/solstone-core-push/` is the sole writer for
`journal/config/push-registry.json` and `journal/config/push-vapid.json`. The
retired `journal/config/push_devices.json` path is not read, written, or
migrated.

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
    },
    {
      "platform": "android",
      "cid": "sha256:...",
      "endpoint": "https://push.example.com/v1/sub123",
      "p256dh": "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4",
      "auth": "BTBZMqHH6r4Tts7J_aSIgg",
      "push_key": "KioqKioqKioqKioqKioqKioqKioqKioqKioqKioqKio",
      "registered_at": "2026-09-24T00:00:00Z"
    }
  ]
}
```

Lifetime behavior:
- Re-register of the same `(cid, device_token)` or `(cid, endpoint)` replaces that row.
- Registering a token or endpoint another CID holds moves the row.
- Unpair deletes every row for that CID.
- A failed delete or a race can leave a row. That row is never sent to, because the CID is no longer authorized. Unpairing that CID again removes it.
- Sending never deletes a row. Status lists every row, including rows that are not recipients. A `revoked` outcome is not stored.

The store holds one sidecar lock across every read-modify-atomic-write
mutation (`0o600` mode). Existing malformed registry data is unavailable (`503
push_registry_unavailable`), not silently treated as an empty registry.

### VAPID Key Management

`journal/config/push-vapid.json` holds the NIST P-256 key pair used for RFC 8292
VAPID authentication on Android Web Push deliveries. The file is created on
demand when `GET /api/push/vapid-key` is first called, held under a sidecar lock
with owner-only mode (`0o600`), and contains:

```json
{
  "pkcs8": "...",
  "created_at": "2026-09-24T00:00:00Z"
}
```

Existing corrupt VAPID data is unavailable (`503 push_vapid_key_unavailable`).
The VAPID key never rotates. Deleting or replacing `config/push-vapid.json` strands
every Android registration until each device subscribes again. The only recovery
for a corrupt file is to remove it; the next GET creates a new key and strands
those registrations. The journal does not silently replace a corrupt key.

### Migration and Rollback

A legacy v1 registry (keyed by CID without a version field or `push_key`) is
recognized during read and treated as empty without mutating the file or logging.
When a new registration triggers a write to a journal with a non-empty v1
registry, the discarded count is logged and the file is atomically rewritten as
v2. A journal build from before this format cannot read v2 and returns `503
push_registry_unavailable` on push routes. An app build that predates `push_key`
gets `400` on register and on DELETE.

Rollback: a build from before Android registrations does not know the `android`
variant, fails the read, and returns 503 on every push route, iOS included, until
a newer build runs.

## Routes

All routes are mounted by Convey's shell and receive its normal session and
door controls.

### `GET /api/push/vapid-key`

Returns the uncompressed 65-byte VAPID public key encoded as an 87-character
URL-safe unpadded base64 string. Generates the VAPID key pair on first call if
absent.

```json
{
  "public_key": "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4"
}
```

### `POST /api/push/register`

The handler first requires an `AccessBasis::LinkedDevice`; localhost, pairing
peers, and missing identities receive `403 linked_device_required` before the
request body is examined. The identity comes from the connection CID, never
from JSON.

For `platform: "ios"`, the JSON object requires:
- `platform`: `"ios"`
- `device_token`: lowercase even hex string of length 16..=200
- `bundle_id`: non-blank after trim
- `environment`: `"development"` or `"production"`
- `push_key`: 32 bytes unpadded URL-safe base64

For `platform: "android"`, the JSON object requires:
- `platform`: `"android"`
- `endpoint`: valid HTTPS push service URL
- `p256dh`: 65-byte uncompressed P-256 public key (87 characters unpadded URL-safe base64, starts with `0x04`)
- `auth`: 16-byte authentication secret (22 characters unpadded URL-safe base64)
- `push_key`: 32 bytes unpadded URL-safe base64

`endpoint_target` is the only judge of an endpoint for registration, status, send,
and the VAPID `aud`. When it returns `None`, status is still 200 and `target` is
`"invalid"`. The stored reader does not call it. A stored endpoint the reader
accepts and `endpoint_target` rejects is `endpoint_invalid` on send and is not
POSTed.

Invalid or extra fields return `400 push_request_invalid` naming the failing field.

Success returns `201 Created` for a new device row or `200 OK` when refreshing:

```json
{
  "platform": "android",
  "target": "push.example.com",
  "registered_at": "2026-09-24T00:00:00Z"
}
```

### `DELETE /api/push/register`

Requires `AccessBasis::LinkedDevice`.
For iOS, body requires `platform: "ios"` and `device_token`.
For Android, body requires `platform: "android"` and `endpoint`.
Removes only that specific device row. Returns `204 No Content` with an empty
body, even when no row matched.

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
    },
    {
      "platform": "android",
      "target": "push.example.com",
      "registered_at": "2026-09-24T00:00:00Z"
    }
  ],
  "total": 2,
  "cursor": null
}
```

### `POST /api/push/test`

Unauthenticated round-trip device test.

- Registry read error is `503 push_registry_unavailable` before anything else, including a bad ledger.
- An empty registry, including a v1 document read as empty, is `503 feature_unavailable` (`no devices to reach`) even when the ledger is malformed. The ledger is not read.
- Ledger `Unreadable`, `Malformed`, or `DuplicateCid` is `503 push_ledger_unavailable`.
- A missing ledger file is no authorized CIDs. A non-empty registry with no recipient is `503 feature_unavailable`, not `push_ledger_unavailable`.
- A recipient is an authorized CID of any role, including `""`, whose `registered_at` is no more than 30 days before the send, or after it. Walk the stored `devices` array. Exactly 30 days is in. 31 days is out.
- Otherwise HTTP 200, one item per recipient: `platform`, `target` (masked or host), `outcome` (`sent`, `revoked`, or `failed`), optional `reason`.
- iOS delivery: Enrollment is `POST {SERVICES_PORTAL_URL}/reach/push/relay-token` with keys `assertion`, `ca_pubkey`, `instance_id`. Dispatch is `POST {SERVICES_PORTAL_URL}/push/dispatch` with `Authorization: Bearer` and `devices[]` keys `envelope`, `environment`, `token`. `development` maps to `sandbox`. `production` maps to `production`. Batches of 16. The production timeout is 60 seconds.
- Android delivery: The journal sends Android pushes directly to the device's chosen push service (no hosted Android service). RFC 8291 `aes128gcm` encrypted record sent directly to the subscription `endpoint`. Web Push headers are `Content-Encoding: aes128gcm`, `Content-Type: application/octet-stream`, `TTL: 86400`, `Urgency: high`, and `Authorization: vapid t=..., k=...`. VAPID claims are `aud` and `exp` (`now + 43200`). There is no `sub`. `aud` is `https://` plus the host, plus `:` and the port only when the port is not 443. The timeout is 30 seconds with no redirects.
- Outcome mapping: any 2xx is `sent` (the push service accepted the message); 404 and 410 are `revoked`; any other status is `failed` / `endpoint_rejected_<status>`; timeout is `endpoint_timeout`; connect or other transport failure is `endpoint_unreachable`. ntfy cannot report an unsubscribe, so `revoked` depends on the service returning 404 or 410. A distributor that uses a private CA fails as `endpoint_unreachable`.
- Isolation: An iOS relay failure does not skip Android sends. A VAPID failure does not skip iOS sends. An Android-only send does not contact the relay and does not load the link identity.
- Worst case is 60 seconds for the relay token, plus 60 seconds per relay batch, plus 30 seconds per Android device. There is no overall bound.
- Logging: At ureq `trace`, ureq logs full requests, including the VAPID JWT and the relay bearer. The journal default level is `warn`. This crate's own log lines do not include the endpoint URL, keys, or JWT.
- An app build that predates `push_key` gets `400` on register and DELETE, so it never becomes a recipient.

With eligible devices, it returns:

```json
{
  "items": [
    {
      "platform": "ios",
      "target": "...cdef",
      "outcome": "sent"
    },
    {
      "platform": "android",
      "target": "push.example.com",
      "outcome": "sent"
    }
  ],
  "total": 2,
  "cursor": null
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
