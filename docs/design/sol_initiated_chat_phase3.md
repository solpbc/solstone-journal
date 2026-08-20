# Sol-Initiated Chat Phase 3 Design

> Retired 2026-08-20. Sol-initiated chat was designed here and later removed entirely. This document is the snapshot of that design. It does not describe a current capability.
> Partial. `sol_voice` settings API exists; APNs/portal_dispatch is not shipped. Ignore Python paths.


## Goals

This change makes sol-initiated chat visible, configurable, and deliverable:

- expose `sol_voice` controls in settings through a dedicated API and UI
- make self-mute clear markers per category
- show where sol-initiated chat replies came from in chat history
- preserve that origin tag for live SSE appends
- deliver solstone chat requests to iOS as APNs alerts
- send silent APNs lifecycle updates when the owner opens or dismisses a request

This change builds on the phase 1 event contract. The existing chat-stream events
remain the source of truth. No new chat-stream event kinds are introduced here.

## Scope

In scope:

- `sol_voice` settings schema changes and a dedicated settings API/UI surface
- per-category clear-marker behavior for category self-mute
- server-rendered and live-rendered origin tags for `sol_message` bubbles that
  answer a `sol_chat_request`
- APNs payload builders and transport support for alert and background pushes
- push-runtime handlers for chat-tract `sol_chat_request`, `owner_chat_open`,
  and `owner_chat_dismissed` messages
- focused tests for settings, chat UI rendering, APNs payloads, and trigger
  wiring

Out of scope:

- producer intelligence that decides when to call `solstone call chat start`
- new chat-stream event kinds
- compatibility shims for the old scalar clear-marker field
- migration of existing owner data beyond updating defaults and tests
- mobile-client behavior beyond the APNs contract emitted by this repo

## Constants Discipline

All phase-3 literals that are part of the sol-initiated UI contract live in
`solstone/convey/sol_initiated/copy.py`. This includes strings rendered by
Jinja templates, strings shared by settings UI code, and the APNs category
literal.

Add constants for:

- chat origin label with talent: `sol noticed (from {trigger_talent}) at {time}`
- chat origin label without talent: `sol noticed at {time}`
- supersede suffix: ` (superseded by {time})`
- provenance toggle collapsed label: `details`
- provenance toggle expanded label: `hide details`
- provenance field labels: `trigger_talent`, `dedupe`, `since_ts`
- settings section title for sol voice controls
- settings labels for daily cap, category caps, rate floor, mute window, category
  self-mute hours, per-category clear markers, system notifications, and debug
  throttled log
- throttled-log empty and error states
- APNs category `SOLSTONE_SOL_CHAT_REQUEST`

## D1: Per-Category Clear-Marker Schema

Chosen option: clean rename.

Rename `category_self_mute_clear_marker_ts: int = 0` to
`category_self_mute_clear_markers: dict[str, int] = field(default_factory=dict)`.
The default seed in `core/fixtures/journal_default.json` becomes
`"category_self_mute_clear_markers": {}`.

Loader behavior:

- missing or empty value means `{}`
- non-dict value logs one WARN and falls back to `{}`
- each marker value must be an int and non-negative
- invalid values log one WARN per rejected category and fall back to omission
- unknown category keys may be preserved only if the loader intentionally treats
  them as inert config; policy reads only known categories

Policy behavior:

- `check_category_self_mute` looks up `settings.category_self_mute_clear_markers.get(category, 0)`
- unrelated categories do not clear each other's self-mute history

Second-order consequences:

- existing journals with the scalar field drop that clear-marker on first load
- this is acceptable because phase 1 just shipped and no users rely on the scalar
- no compatibility alias or fallback reader should be added, per the clean-break
  invariant
- fixtures in `tests/test_sol_initiated_policy.py` and
  `tests/test_chat_stream_sol_initiated.py` need the new field name
- add a per-category isolation test that clears one category while another
  remains self-muted

## D2: Settings Write Path

Chosen option: dedicated `GET/PUT /api/sol_voice`.

`solstone/apps/settings/routes.py:update_config()` only supports flat section
writes plus the transcribe-specific nested backend exception. Phase 3 needs
dynamic category keys, nested mute-window settings, nested notification settings,
and a debug toggle. A dedicated endpoint matches existing complex settings
surfaces such as providers, generators, vision, observe, sync, and storage.

New routes:

- `GET /api/sol_voice`
- `PUT /api/sol_voice`
- `GET /api/sol_voice/throttled?limit=50`

`GET /api/sol_voice` returns `SolVoiceSettings.to_dict()`.

`PUT /api/sol_voice` accepts a partial dict and calls
`solstone.convey.sol_initiated.settings.save_settings(updates)`. That helper:

- reads the current top-level `sol_voice` block through `get_config()`
- deep-merges updates, replacing leaves rather than whole sections
- validates through the same dataclass parser used by `load_settings`
- writes the merged validated block back to `journal/config/journal.json`
- uses `set_config("sol_voice", merged_dict)` if available during
  implementation; otherwise adds a small atomic writer in `settings.py`

`GET /api/sol_voice/throttled` reads `journal/push/nudge_log.jsonl`, takes the
last N rows, filters to `kind == "sol_chat_request"` and `outcome != "written"`,
and returns `ts`, `category`, `dedupe_key`, and `outcome`.

Second-order consequences:

- settings writes stay out of generic `update_config()` special cases
- validation stays centralized with the sol-initiated settings loader
- the debug panel can show throttled/deduped starts without exposing
  `default_dedupe_window` in UI

## D3: Origin-Tag Computation Site

Chosen option: routes-side precomputation.

`solstone/apps/chat/routes.py:day()` should compute origin metadata after
`read_chat_events(day)` and before rendering. It passes
`sol_message_origins: dict[int, dict]` into the template, keyed by event index.

Origin metadata contains:

- `trigger_talent`
- `dedupe`
- `since_ts`
- `ts`
- `request_id`
- `superseded_by_id`
- `superseded_at`

Computation walks the events once:

- on `sol_chat_request`, remember the latest unresponded request
- on `sol_message`, attach the pending request metadata to that event index and
  clear the pointer
- on `sol_chat_request_superseded`, if it matches the pending request or an
  already-rendered request, annotate the corresponding origin metadata with
  supersede information

`solstone/apps/chat/_chat_event.html` renders the origin tag above the
`sol_message` bubble when `origins.get(loop.index0)` exists. The visible tag uses
copy constants:

- with `trigger_talent`: `sol noticed (from {trigger_talent}) at {time}`
- without `trigger_talent`: `sol noticed at {time}`
- with supersede: append ` (superseded by {time})`

The provenance row is initially hidden and contains three labeled fields:
`trigger_talent`, `dedupe`, and `since_ts`. A small `data-origin-toggle` control
expands or collapses it.

Second-order consequences:

- SSR history is authoritative and does not depend on client-side event replay
- route tests can assert deterministic HTML for older days
- initial rendering does not need to maintain a client-side event list

## D4: Live-Append Origin Tracking

Chosen option: a single `pendingSolChatRequest` JavaScript variable.

`solstone/apps/chat/workspace.html` currently stores rendered chat state in the
DOM only. Add one closure-scoped variable near `timeFormatter`:
`pendingSolChatRequest`, containing request id, summary, trigger talent, dedupe,
since timestamp, category, and timestamp.

Live SSE behavior:

- `sol_chat_request` stores the message in `pendingSolChatRequest`
- `sol_message` renders an origin tag when the pointer is present, then clears it
- `sol_chat_request_superseded` drops the pointer if it supersedes the pending
  request; if a matching rendered bubble exists, annotate it
- `owner_chat_open` and `owner_chat_dismissed` are accepted by the SSE handler
  but do not render bubbles

The live allowlist in `renderEventItem` must include the four sol-initiated kinds
so schema-valid chat messages are not dropped. The render body for owner
lifecycle events returns `null` or otherwise avoids appending visible UI.

`renderOriginTag(meta)` in JS should mirror the SSR structure and use the
existing `timeFormatter` for time labels.

Second-order consequences:

- live rendering remains light and does not introduce a full event reducer
- a page loaded before a pending request arrives can still tag the next live sol
  reply
- if the page loads after the request but before the reply, SSR remains
  authoritative for existing events; implementation may optionally initialize
  the pointer by walking DOM, but this is not required for phase 3

## D5: APNs Silent-Push Transport

Chosen option: extend the existing transport path with a `push_type` parameter.

`solstone/think/push/dispatch.py:_headers` currently hard-codes
`apns-push-type: alert`. Add `push_type: str = "alert"` to:

- `_headers`
- `_send_with_client`
- `_send_async`
- `send`
- `_send_many_async`
- `send_many`

When `push_type == "background"`, set `apns-push-type` to `background`.
Otherwise keep `alert`. Existing callers keep alert behavior by default.

Add payload and collapse-id builders:

- `build_sol_chat_request_payload(request_id, summary, category)` returns an
  alert payload with title `sol`, body equal to summary, category
  `SOLSTONE_SOL_CHAT_REQUEST`, default sound, mutable content, and data action
  `open_chat_request`
- `build_sol_chat_request_collapse_id(request_id)` returns
  `sol_chat_request:{request_id}`
- `build_silent_chat_lifecycle_payload(request_id, action)` returns a silent
  payload with `mutable-content: 1`, `content-available: 1`, and data action plus
  request id
- `build_silent_chat_lifecycle_collapse_id(request_id, action)` returns
  `sol_chat_lifecycle:{request_id}:{action}`

Second-order consequences:

- existing push callers and tests remain alert-only unless they opt in
- background pushes can use APNs priority 5 through the existing priority
  argument
- silent payloads carry no alert, sound, or category

## D6: Triggers And Runtime Wiring

Add two handlers in `solstone/think/push/triggers.py`.

`handle_sol_chat_request(message)`:

- returns unless `tract == "chat"` and `event == "sol_chat_request"`
- extracts `request_id`, `summary`, and `category` directly from the callosum
  message
- returns when push is unconfigured or no eligible devices exist
- sends the alert payload with `send_many(..., priority=10)`
- appends a nudge-log row with `kind="sol_chat_request_push"`,
  `dedupe_key=request_id`, `category=category`, and `outcome="dispatched"`
- on APNs failure, logs a warning, appends `outcome="error"`, and does not raise

`handle_chat_lifecycle(message)`:

- returns unless `tract == "chat"` and `event` is `owner_chat_open` or
  `owner_chat_dismissed`
- extracts `request_id` and uses the event value as the lifecycle action
- returns when push is unconfigured or no eligible devices exist
- sends the silent payload with `send_many(..., priority=5, push_type="background")`
- appends a nudge-log row with `kind="sol_chat_lifecycle_push"`,
  `dedupe_key=request_id`, `category=action`, and `outcome="dispatched"`
- logs and records `outcome="error"` on APNs failure without raising

Wire both in `solstone/think/push/runtime.py:_on_callosum_message` alongside
existing briefing and weekly-reflection handlers.

Second-order consequences:

- no disk reload is needed because `_broadcast_chat_event` already sends the full
  event payload on the chat tract
- push-runtime remains the single background subscriber for push behavior
- lifecycle pushes are best-effort side effects and must not disrupt chat stream
  writes

## File-By-File Change List

Group 1 - constants + design doc:

- NEW `docs/design/sol_initiated_chat_phase3.md` (this doc)
- EDIT `solstone/convey/sol_initiated/copy.py`: add phase-3 constants for every Jinja-rendered string in the new chat origin-tag UI, settings section, and provenance toggle. Enumerate them in the design doc.

Group 2 - settings schema rename:

- EDIT `solstone/convey/sol_initiated/settings.py`: rename field, add `save_settings()` helper, extend schema with `system_notifications: {macos: bool, linux: bool}` and `debug_show_throttled: bool`, add WARN-on-rejected for new dict fields.
- EDIT `solstone/convey/sol_initiated/policy.py:88`: read per-category marker.
- EDIT `core/fixtures/journal_default.json`: rename + new fields.
- EDIT `tests/test_sol_initiated_policy.py`, `tests/test_chat_stream_sol_initiated.py`: update fixtures + add per-category isolation test.

Group 3 - settings API + UI:

- EDIT `solstone/apps/settings/routes.py`: add `GET/PUT /api/sol_voice` and `GET /api/sol_voice/throttled` endpoints.
- EDIT `solstone/apps/settings/workspace.html`: new section with all controls; bind to new endpoints; debug log panel reads `/api/sol_voice/throttled`.
- NEW or EDIT `solstone/apps/settings/tests/`: tests for round-trip read/write, debug log filter, system_notifications opt-ins persist.

Group 4 - chat UI origin-tag SSR:

- EDIT `solstone/apps/chat/routes.py`: precompute `sol_message_origins` map.
- EDIT `solstone/apps/chat/_chat_event.html`: render origin tag + provenance row above sol_message bubble.
- NEW `solstone/apps/chat/tests/test_origin_tag_ssr.py`: fixture day with sol_chat_request -> sol_message chain; assert rendered HTML contains origin tag with both branches (with/without trigger_talent) and supersede marker.

Group 5 - chat UI origin-tag live-append:

- EDIT `solstone/apps/chat/workspace.html`: add `pendingSolChatRequest` state + SSE handlers + `renderOriginTag` helper.
- NEW `solstone/apps/chat/tests/test_origin_tag_live.py` (or extend existing JS-light fixture test): exercise the SSE path.

Group 6 - APNs category + builders:

- EDIT `solstone/think/push/dispatch.py`: add `SOLSTONE_SOL_CHAT_REQUEST` to `CATEGORIES`, add three new builders, thread `push_type` param through `_headers`/`_send_with_client`/`send`/`send_many`.
- NEW or EDIT `tests/test_push_dispatch.py`: payload shape tests for both alert and silent payloads, `push_type` propagation to headers.

Group 7 - chat-tract triggers + runtime wiring:

- EDIT `solstone/think/push/triggers.py`: two new handlers.
- EDIT `solstone/think/push/runtime.py:_on_callosum_message`: wire both.
- NEW or EDIT `tests/test_push_triggers.py`: trigger filter behavior, no-op when unconfigured, no-op when zero devices, nudge-log row written on success, distinct `kind` tags.

## Implementation Sequence

1. Add constants and update the locked-literals allowlist.
2. Rename the settings field, extend the settings dataclasses, and add
   `save_settings`.
3. Update self-mute policy and default config.
4. Add settings API routes and the settings UI controls.
5. Add SSR origin-tag precomputation and template rendering.
6. Add live SSE origin tracking and lifecycle-event ignore behavior.
7. Add APNs category, payload builders, and `push_type` transport plumbing.
8. Add push trigger handlers and runtime wiring.
9. Add focused tests in the same dependency order.
10. Run targeted tests first, then the full validation plan.

## Test Plan

Settings:

- loader accepts missing `category_self_mute_clear_markers` as `{}`
- loader warns on non-dict clear markers
- loader warns on negative or non-int marker values
- per-category clear marker isolates one category from another
- `save_settings` round-trips nested mute-window, category caps, notification
  settings, and debug toggle
- throttled endpoint filters only solstone chat request rows whose outcome is not
  `written`

Chat UI:

- SSR renders no origin tag for ordinary sol messages
- SSR renders origin tag with `trigger_talent`
- SSR renders origin tag without `trigger_talent`
- SSR renders supersede suffix when present
- provenance row contains `trigger_talent`, `dedupe`, and `since_ts`
- live append stores `sol_chat_request`, tags the next `sol_message`, then clears
  the pending pointer
- live append ignores owner lifecycle events visibly

Push dispatch:

- existing payload builders still produce alert headers by default
- solstone chat request alert payload has the expected APNs key set
- silent lifecycle payload has no alert, sound, or category
- `push_type="background"` reaches `apns-push-type: background`
- alert callers retain `apns-push-type: alert`

Push triggers:

- chat request handler ignores non-chat and non-request events
- lifecycle handler ignores non-chat and non-lifecycle events
- both handlers no-op when unconfigured
- both handlers no-op when there are no eligible devices
- success rows use distinct `kind` values
- APNs failures are logged and recorded without raising

## Validation Plan

Run, in order:

- `make test-only TEST=tests/test_sol_initiated_policy.py`
- `make test-only TEST=tests/test_chat_stream_sol_initiated.py`
- `make test-only TEST=tests/test_push_dispatch.py`
- `make test-only TEST=tests/test_push_triggers.py`
- `make test-app APP=settings`
- `make test-app APP=chat`
- `make ci`
- `make verify-api`

`make verify-api` is required because this change adds new settings endpoints.

## Risks And Open Questions

- Confirm during implementation whether a suitable `set_config` helper exists.
  If not, keep the `sol_voice` writer thin, local to `settings.py`, and atomic.
- The file-by-file list says "add three new builders" in dispatch, but the
  detailed transport decision names four helper functions: two payload builders
  and two collapse-id helpers. Implementation should treat this as two payload
  builders plus their collapse-id helpers.
- Live append can rely on SSR for already-rendered history. If manual testing
  shows a request arriving before page load and the reply arriving after page
  load, initialize `pendingSolChatRequest` by walking DOM or by passing a
  server-computed pending request value.
- APNs background pushes are best-effort. Tests should assert headers and
  payload shape, not actual Apple delivery behavior.
