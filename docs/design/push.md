# Push Registry And Relay

> Paused. Device registry, relay, and transport survive; the chat-trigger path is retired. No crate route. Treat surviving sections as an unbuilt spec; ignore Python paths.


## Summary

The journal's push role is deliberately narrow:

- Keep a device registry keyed by each paired device's link fingerprint.
- Self-provision a reach relay token from the hosted services portal.
- Provide a test-push endpoint that goes through the hosted relay.

Push is paused, not gone. **Retired (historical):** relaying sol-initiated chat
push events to the hosted service through `portal_dispatch`. Future payload
direction: notify owners of journal states and if their devices stopped checking
in.

The journal does not contact Apple. Device delivery and platform-specific
delivery details live outside this repository.

## Module Layout

| Path | Role |
|---|---|
| `solstone/think/push/devices.py` | Sole writer for `journal/config/push_devices.json`. Stores one row per link fingerprint. Re-registering a fingerprint replaces that row, and registering a token under another fingerprint drops the older holder so one token maps to exactly one device. |
| `solstone/convey/push.py` | Root Flask blueprint for `/api/push/register`, `/api/push/status`, and `/api/push/test`. Registration and deregistration source the fingerprint from `g.identity.fingerprint`, never from request JSON. |
| `solstone/think/push/triggers.py` | **Retired (historical).** Relay-only callosum handlers for direct chat requests and chat lifecycle events, plus the nudge-log writer used by sol-initiated chat accounting. No native replacement is asserted here. |
| `solstone/think/push/runtime.py` | **Retired (historical).** Runtime singleton that started a callosum listener and routed each message through the two push trigger handlers. |
| `solstone/think/push/portal_dispatch.py` | HTTP relay client for the hosted `/push/dispatch` and `/push/dedup` endpoints. |
| `solstone/think/push/reach.py` | Reach relay-token client. Enrolls with the hosted reach endpoint, stores the opaque token under journal config, and refreshes before dispatch. |

`push_devices.json` stores:

```json
{
  "devices": [
    {
      "fingerprint": "sha256:...",
      "token": "...",
      "bundle_id": "org.solpbc.solstone-swift",
      "environment": "development",
      "platform": "ios",
      "registered_at": 1770000000
    }
  ]
}
```

No device public key is stored today. Per-device body encryption is a future
arc and will require an explicit `device_pubkey` field or equivalent schema.

## Endpoint Shapes

All `/api/push/*` routes inherit the normal Convey auth gate.

### `POST /api/push/register`

Request body:

```json
{
  "device_token": "...",
  "bundle_id": "org.solpbc.solstone-swift",
  "environment": "development",
  "platform": "ios"
}
```

The handler requires `g.identity.fingerprint`. Requests with no fingerprint are
rejected with `push_request_invalid` and detail
`push registration requires a paired device`.

On success, `devices.register_device(...)` upserts by fingerprint and returns:

```json
{"registered": true, "device_count": 1}
```

### `DELETE /api/push/register`

No request body is needed. The handler requires `g.identity.fingerprint` and
removes that fingerprint's row:

```json
{"removed": true, "device_count": 0}
```

### `GET /api/push/status`

Response:

```json
{
  "device_count": 1,
  "relay_available": true,
  "devices": [
    {
      "token_suffix": "...abcd",
      "bundle_id": "org.solpbc.solstone-swift",
      "environment": "development",
      "platform": "ios",
      "registered_at": "2026-05-20T00:00:00Z"
    }
  ]
}
```

`relay_available` is true when a reach relay token is present in journal config.
Fingerprints and full tokens are not exposed.

### `POST /api/push/test`

Optional request body:

```json
{"body": "This is a test notification."}
```

The handler requires at least one registered device. It creates a
`push-test-<hex>` request id and calls `dispatch_via_portal(...)` with the test
summary. **Retired (historical):** the test dispatch used a `sol-chat-request`
category. Dispatch self-provisions or refreshes the
reach relay token as needed. If no devices are registered, the route returns
`503 feature_unavailable` with detail `no devices to reach`. If token enrollment
or the relay call fails, it returns `503 feature_unavailable` with detail
`push relay dispatch failed`.

Success response:

```json
{"dispatched": true, "request_id": "push-test-abc123def456"}
```

## Relay Triggers — retired (historical)

The chat-trigger path is retired. The listening behavior below is a snapshot of
what `triggers.py` used to do; it does not describe a current capability.

`runtime._on_callosum_message(...)` called:

1. The direct chat request trigger handler.
2. `triggers.handle_chat_lifecycle(message)`

The direct chat request handler listened for:

- `tract == "chat"`
- `event == KIND_SOL_CHAT_REQUEST`
- non-empty `request_id`

With registered devices, it called `dispatch_via_portal(request_id, summary,
category)`. Dispatch provisions or refreshes the reach relay token internally.

`handle_chat_lifecycle` listened for:

- `tract == "chat"`
- `event in {KIND_OWNER_CHAT_OPEN, KIND_OWNER_CHAT_DISMISSED}`
- non-empty `request_id`

With registered devices, it called
`dispatch_dedup_via_portal(request_id, action=event)`. Dispatch provisions or
refreshes the reach relay token internally.

## Nudge Log

The append-only JSONL row shape at `journal/push/nudge_log.jsonl` survives.
**Retired (historical):** `triggers.py` was the writer, used by sol-initiated
chat accounting. Chat-specific `kind` values in the examples below are
historical.

Successful relay row:

```json
{
  "ts": 1770000000,
  "kind": "<chat-request push kind>",
  "dedupe_key": "req-1",
  "category": "notice",
  "outcome": "dispatched",
  "via": "portal"
}
```

No devices row:

```json
{
  "ts": 1770000000,
  "kind": "<chat-request push kind>",
  "dedupe_key": "req-1",
  "category": "notice",
  "outcome": "skipped",
  "reason": "no_devices"
}
```

Relay unavailable row, including reach-token enrollment or refresh failure:

```json
{
  "ts": 1770000000,
  "kind": "<chat-request push kind>",
  "dedupe_key": "req-1",
  "category": "notice",
  "outcome": "skipped",
  "reason": "portal_unavailable"
}
```

Lifecycle rows used `kind == "sol_chat_lifecycle_push"` and stored the lifecycle
event name in `category`. **Retired (historical):** silent chat-lifecycle
payloads.

Chat-fold rows may also skip with `reason == "owner_viewing_chat"` when the
owner already has a recent open chat view. **Retired (historical).** There is no token-specific skipped
reason; token acquisition failures collapse into `portal_unavailable`.

## Reach Token Enrollment

The reach relay token is opaque to the journal. The journal stores and forwards
it as a Bearer token and never decodes it. State lives under
`services.push.reach_token` in `journal/config/journal.json`:

```json
{
  "token": "<opaque>",
  "instance_id": "<uuid>",
  "expires_at": "2026-06-20T12:00:00Z",
  "expires_epoch": 1781956800
}
```

`expires_at` is the display/source string returned by the service.
`expires_epoch` is derived from `expires_at` for integer refresh checks. The
legacy relay-token string path is not read.

Enrollment is best-effort on successful device registration and refreshed on
dispatch when the stored token is expired, malformed, for another instance, or
inside the one-hour refresh margin. The journal POSTs to:

```text
POST {portal_base_url()}/reach/push/relay-token
```

Request body:

```json
{
  "instance_id": "<uuid>",
  "ca_pubkey": "<PEM SPKI>",
  "assertion": "<compact ES256 JWT>"
}
```

Assertion header:

```json
{"alg":"ES256","typ":"home-reach"}
```

Assertion claims:

```json
{
  "iss": "home:<instance_id>",
  "aud": "solstone-reach",
  "scope": "push.relay.enroll",
  "instance_id": "<instance_id>",
  "iat": 1770000000,
  "exp": 1770000240,
  "jti": "<uuid>"
}
```

Response body:

```json
{
  "token": "<opaque>",
  "token_type": "Bearer",
  "expires_at": "2026-06-20T12:00:00Z",
  "expires_in": 86400,
  "instance_id": "<instance_id>"
}
```

## Domain Ownership

Per AGENTS.md L2, `solstone/think/push/devices.py` is the sole writer for
`journal/config/push_devices.json`. **Retired (historical):** `solstone/think/push/triggers.py` was the
sole writer for `journal/push/nudge_log.jsonl`. No native replacement is asserted here.

`solstone/convey/push.py` validates HTTP input and delegates mutations to the
think layer: device registry changes go through `devices.py`, and reach token
state changes go through `reach.py` via `journal_config`. It must not write
journal files directly.

## Out Of Scope

- Per-device body encryption.
- A `device_pubkey` column or migration.
- Delivery-provider behavior owned by the hosted relay.
