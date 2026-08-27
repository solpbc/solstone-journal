# Push Device Registry

## Summary

`solstone-core-push` owns the local push-device registry and the native
`/api/push/*` routes. The registry records where a future delivery system may
reach a linked device; this repository does not implement hosted relay or
notification delivery.

## Domain ownership

`core/crates/solstone-core-push/` is the sole writer for
`journal/config/push-registry.json`. The retired
`journal/config/push_devices.json` path is not read, written, or migrated.

The registry is one JSON document keyed by the linked-device CID supplied by
the authenticated connection:

```json
{
  "devices": {
    "sha256:...": {
      "device_token": "...",
      "bundle_id": "org.solpbc.solstone-swift",
      "environment": "development",
      "platform": "ios",
      "registered_at": "2026-08-27T12:00:00Z"
    }
  }
}
```

Registration replaces an existing row for the same CID. A token can belong to
only one CID, so registering an already-held token removes its previous row.
The store holds one sidecar lock across every read-modify-atomic-write
mutation. Existing malformed registry data is unavailable, not silently
treated as an empty registry.

## Routes

All routes are mounted by Convey's shell and receive its normal session and
door controls.

### `POST /api/push/register`

The handler first requires an `AccessBasis::LinkedDevice`; localhost, pairing
peers, and missing identities receive `403 linked_device_required` before the
request body is examined. The identity comes from the connection CID, never
from JSON.

The JSON object requires `device_token`, `bundle_id`, `environment`, and
`platform`. Token and bundle values must contain a non-whitespace character;
their submitted values are retained exactly. `environment` is `development` or
`production`, and `platform` is `ios`. Invalid input returns
`400 push_request_invalid`.

Success response:

```json
{"registered": true}
```

### `DELETE /api/push/register`

This route has the same linked-device requirement and removes only that CID's
row. Repeating it is successful and reports no removal:

```json
{"removed": false}
```

### `GET /api/push/status`

Status reports the local registry, newest registration first. It exposes no
CID or full token; `device_token` is the exact stored token's final four
characters prefixed with `...`.

```json
{
  "count": 1,
  "devices": [
    {
      "bundle_id": "org.solpbc.solstone-swift",
      "environment": "development",
      "platform": "ios",
      "registered_at": "2026-08-27T12:00:00Z",
      "device_token": "...abcd"
    }
  ]
}
```

### `POST /api/push/test`

This is a local registry round-trip and device-count gate only. It does not
contact a relay, enqueue a notification, or prove delivery. With no registered
devices it returns `503 feature_unavailable` and `no devices to reach`; with
one or more devices it returns:

```json
{"device_count": 1}
```

## Failures and scope

Registry read, parse, lock, or write failures return
`503 push_registry_unavailable`. They must never be reported as an empty
registry.

Out of scope: hosted relay enrollment, APNS delivery, push dispatch request
identifiers, delivery claims, per-device payload encryption, and registry
migration from retired storage.
