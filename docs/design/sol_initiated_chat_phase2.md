# Sol-Initiated Chat Phase 2 Design

## Overview

This change adds the consumer-side surface for sol-initiated chat requests. Phase 1
already added the stream contract and producer primitive in
`solstone/convey/sol_initiated/` plus the four chat-stream kinds in
`solstone/convey/chat_stream.py`.

Phase 2 consumes those stream facts in three places:

- the universal chat bar in `solstone/convey/templates/app.html`
- the `/app/chat` page in `solstone/apps/chat/routes.py` and
  `solstone/apps/chat/workspace.html`
- observer delivery surfaces in `solstone/convey/bridge.py`,
  `solstone/apps/observer/routes.py`, and `solstone/observe/`

The relevant existing anchors are:

- `solstone/convey/chat_stream.py:47-59`: `_VALID_KINDS` already includes all
  four phase-1 kinds.
- `solstone/convey/chat_stream.py:61-66`: `_TRIGGER_KINDS` includes only
  `sol_chat_request`, matching the phase-1 design and not the broader wording in
  this change's scope.
- `solstone/convey/chat_stream.py:173-197`: `read_chat_events()` returns events
  in ascending timestamp order by sorting `(ts, segment, line)`.
- `solstone/convey/templates/app.html:137-745`: the chat-bar IIFE owns chat-bar
  state and behavior.
- `solstone/apps/chat/workspace.html:97-107`: existing hash-scroll behavior for
  `#event-<idx>` anchors.
- `solstone/convey/bridge.py:87-112`: SSE broadcast fans every Callosum event to
  every active observer subscriber; it does not key-prefix filter messages.

## Goals

- Show the latest unread `sol_chat_request` in the universal chat bar.
- Let owners open or dismiss a sol-initiated chat request from the chat bar.
- Record `owner_chat_open` when the owner opens today's `/app/chat` page while a
  request is still unread.
- Clear the sol-ping state across all open browser tabs by consuming chat-stream
  broadcasts.
- Track per-observer `last_chat_request_at` delivery state in the convey bridge
  and expose it in `/app/observer/api/list`.
- Add an observer-side helper that normalizes the four sol-initiated chat kinds
  from either Callosum frames or chronicle stream events.
- Keep phase-1 producer policy, stream kind validation, and trigger semantics
  unchanged.

## Non-goals

- No changes to `chat_stream._VALID_KINDS`.
- No changes to `chat_stream._TRIGGER_KINDS`.
- No changes to phase-1 policy checks, dedupe rules, rate limits, settings, or
  `sol call chat start`.
- No producer intelligence that decides when to start a chat.
- No phase-3 surfaces beyond the universal chat bar, `/app/chat`, `/app/observer`,
  and observer-side filtering helper.
- No persisted observer metadata writes for `last_chat_request_at`.
- No migration of existing chat streams or observer records.

## Design Decisions

### D1. `window.openConversation` semantics

Add `window.openConversation({prompt: null, openOn: "chat-request"})` inside the
chat-bar IIFE in `solstone/convey/templates/app.html:137-745`.

Semantics:

- If the active request has an `event_index`, build `#event-<event_index>`.
- If `window.location.pathname` is `/app/chat/<request_day>`, scroll that anchor
  into view and focus `#chatBarInput`.
- Otherwise navigate to `/app/chat/<request_day>#event-<event_index>`.
- If `event_index` is missing because of a race, navigate to `/app/chat/<request_day>`
  without a hash.

The existing `/app/chat` hash behavior at `solstone/apps/chat/workspace.html:97-107`
handles the target page after navigation. The chat-bar needs `request_id`,
`event_index`, and `day` from backend context. The requested context fields were
`request_id`, `summary`, `ts`, and `event_index`; this design also carries `day`
so the browser does not compute "today" with a different timezone or surface
route assumption than the backend.

`openConversation` is deliberately small. It does not submit chat, mark dismissals,
or infer request state. Those are handled by explicit lifecycle events.

### D2. Per-observer `last_chat_request_at` tracking

Use in-memory bridge state in `solstone/convey/bridge.py`.

Changes:

- Add `_SSE_LAST_CHAT_REQUEST_AT_BY_KEY: dict[str, int] = {}` near the existing
  `_SSE_SUBSCRIBERS_BY_KEY` at `solstone/convey/bridge.py:48`.
- Import `KIND_SOL_CHAT_REQUEST` from `solstone.convey.sol_initiated.copy`.
- In `_broadcast_to_sse_clients()` at `solstone/convey/bridge.py:87-112`, when
  `message["tract"] == "chat"` and `message["event"] == KIND_SOL_CHAT_REQUEST`,
  update the dict after each successful `subscriber.queue.put_nowait(serialized)`.
- Key the dict by `subscriber.key_prefix`.
- Add public reader `last_chat_request_at(key_prefix: str) -> int | None`.
- In `_serialize_observer()` at `solstone/apps/observer/routes.py:156-177`, add
  `last_chat_request_at` to the serialized observer object.

This is a delivery/visibility signal local to the bridge process, not observer
domain state. `CLAUDE.md` L2 says observer-domain writes must stay in the owning
module. This design preserves L2 by not modifying `observers/*.json`.

Rejected alternative: persist this through `save_observer()` or a new sibling
helper in `solstone/apps/observer/utils.py`. That would be L2-compliant only if
the write helper lived in the observer domain, but it is worse for this use case:
it would turn a transient fan-out signal into disk churn on every sol-ping,
increase bridge/domain coupling, and require conflict handling for concurrent
observer metadata updates. Restart resets are acceptable because active observers
reconnect quickly and the next `sol_chat_request` repopulates the value.

### D3. `/app/chat` page-load `record_owner_chat_open`

> **Superseded:** the `/app/chat` page-load open is now recorded by a front-end POST-on-load to `POST /api/chat/sol_chat_request/open`; the GET handler is read-only and no longer writes `owner_chat_open`.

Add server-side open recording in `solstone/apps/chat/routes.py::day(day)`.

Current route shape:

- `solstone/apps/chat/routes.py:29-45`: validates the day, computes `today_day`,
  and passes `events=read_chat_events(day)` to `render_template`.

New route flow:

1. Read `events = read_chat_events(day)`.
2. Only when `day == today_day`, find the latest unresolved `sol_chat_request`.
3. A request is unresolved when its `request_id` has no later
   `owner_chat_open`, no later `owner_chat_dismissed`, and has not appeared as
   `request_id` in a later `sol_chat_request_superseded`.
4. If found, call `record_owner_chat_open(request_id, surface="convey")` before
   rendering.
5. Do not append the freshly written open event to the local `events` list passed
   to the template.

The freshly appended stream fact is intentionally not rendered in the initial
HTML. `solstone/apps/chat/workspace.html:207-215` drops all four sol-initiated
kinds in its live allowlist today, and the server partial
`solstone/apps/chat/_chat_event.html:1-45` has no branch for them.

Idempotency trade-off: phase-1 dedupe releases on `owner_chat_open` by request id,
and repeated opens are idempotent because dropping an already-absent pending
request is a no-op. Repeated reloads therefore append repeated
`owner_chat_open` events, which is acceptable — each page load is an engagement
signal. No suppression logic in phase 2.

### D4. Backend context surfaced to chat-bar

Split structured attention from structured sol-ping state in
`solstone/convey/apps.py`.

Current context processor:

- `solstone/convey/apps.py:236-317`: `inject_app_context()`
- `solstone/convey/apps.py:297-304`: computes `chat_bar_placeholder`
- `solstone/convey/templates/chat_bar.html:11`: places the string into the
  textarea placeholder

New context keys from `inject_app_context()`:

- `chat_bar_attention`: dict or null. Initial shape is `{placeholder_text: str}`.
  It mirrors the visible output from `_resolve_attention()` without losing the
  structured state.
- `chat_bar_sol_request`: dict or null. Shape:
  - `request_id`
  - `summary`
  - `ts`
  - `event_index`
  - `day`

`chat_bar_placeholder` becomes only the fallback default. Existing behavior stays
the same when there is no unread `sol_chat_request` and no attention. When
attention exists and no unread request exists, JS uses
`chat_bar_attention.placeholder_text`. When an unread `sol_chat_request` exists,
JS renders the sol-ping state regardless of attention.

Reason for splitting attention out of the placeholder string: the chat bar must
respond to live `sol_chat_request`, `sol_chat_request_superseded`,
`owner_chat_open`, and `owner_chat_dismissed` broadcasts without a page reload.
After a sol-ping clears, JS must know whether to revert to attention copy or the
fallback placeholder. A flat precomputed placeholder string loses that state.

Event index lookup:

- Reuse `read_chat_events(today)` once.
- While finding the latest unresolved request, record its zero-based index in
  the sorted event list.
- `read_chat_events()` ordering is stable per `solstone/convey/chat_stream.py:173-197`.
- If no stable index is available in a race, JS falls back to `/app/chat/<day>`.

To avoid duplicate reverse-scan implementations, add a small read-only helper in
`solstone/convey/sol_initiated/state.py`. It should accept a chronological
`list[dict]` and return either null or a request summary including `event_index`.
Both `solstone/convey/apps.py` and `solstone/apps/chat/routes.py` should use it.

### D5. Multi-tab clear semantics

Use existing WebSocket fan-out.

Existing chat-bar listener:

- `solstone/convey/templates/app.html:733-735` registers
  `window.appEvents.listen('chat', handleChatEvent)`.
- `solstone/convey/templates/app.html:591-638` dispatches chat kinds.

Extend `handleChatEvent` so every open tab clears its local sol-ping when it
receives `owner_chat_open` or `owner_chat_dismissed` for the active `request_id`.
It also clears or replaces local sol-ping state when it receives
`sol_chat_request_superseded`.

No per-tab polling is needed. No new read endpoint is needed for cross-tab sync.
All open tabs already receive each chat broadcast through `websocket.js`.

### D6. Filter helper file and signature

Add new module `solstone/observe/sol_chat_filter.py`.

Public function:

- `filter_sol_chat_event(frame: dict) -> dict | None`

The helper imports the four kind constants from
`solstone.convey.sol_initiated.copy`.

Inputs:

- Callosum/SSE frame shape, guaranteed by
  `solstone/apps/observer/routes.py:229-235`: `tract`, `event`, `ts`, plus
  event-specific fields.
- Chronicle stream event shape, where `kind` is present and `tract`/`event` may
  not be.

Matching:

- Return `None` unless `frame.get("tract") == "chat"` for callosum-style frames,
  or unless the frame is chronicle-style with a matching `kind`.
- Use `frame.get("event") or frame.get("kind")` as the normalized kind.
- Match only the four sol-initiated chat kinds.

Return:

- `sol_chat_request`: normalized dict with `kind`, `ts`, `request_id`, `summary`,
  `message`, `category`, `dedupe`, `dedupe_window`, `since_ts`, and
  `trigger_talent`.
- `sol_chat_request_superseded`: normalized dict with `kind`, `ts`,
  `request_id`, and `replaced_by`.
- `owner_chat_open`: normalized dict with `kind`, `ts`, `request_id`, and
  `surface`.
- `owner_chat_dismissed`: normalized dict with `kind`, `ts`, `request_id`,
  `surface`, and `reason`.

Boundary validation should be light: this helper is a filter/normalizer, not a
stream validator. It should return `None` for nonmatching frames and normalize
missing optional text to empty strings or `None` according to the phase-1 stream
contract.

### D7. Shared JS constants module

Add `core/crates/solstone-core-convey-shell/assets/static/sol_initiated_constants.js`.

The module defines `window.SOL_INITIATED` with:

- `KIND_SOL_CHAT_REQUEST`
- `KIND_SOL_CHAT_REQUEST_SUPERSEDED`
- `KIND_OWNER_CHAT_OPEN`
- `KIND_OWNER_CHAT_DISMISSED`
- `SURFACE_CONVEY`
- `SOL_PINGED_OFFLINE_TOOLTIP`

Load it in `solstone/convey/templates/app.html` after
`relative-time.js` at line 11 and before `websocket.js` at line 21. The chat-bar
IIFE consumes it. Observer cards may also consume it later.

Constants discipline:

- `core/crates/solstone-core-convey-shell/assets/static/sol_initiated_constants.js` is the centralized
  browser contract.
- Keep its literal values aligned with
  `solstone/convey/sol_initiated/copy.py:6-9`, with the tooltip literal in a
  single home.

### D8. CSS pulse animation

Add CSS in `core/crates/solstone-core-convey-shell/assets/static/app.css`.

Behavior:

- Add `@keyframes sol-ping-pulse` with a 3s opacity cycle: `0.4 -> 1.0 -> 0.4`.
- Add a chat-bar dot element/class for the sol-ping leading indicator.
- JS adds the pulsing class when rendering an unread sol-ping.
- JS removes the pulsing class after 30 seconds with one timeout tied to the
  active request.
- Disconnect state is separate from pulse state. If the websocket has been
  disconnected for at least 30 seconds while the request is unread, JS adds an
  offline class that greys the dot and uses `SOL_PINGED_OFFLINE_TOOLTIP`.

The timeout must be cleared when the request opens, dismisses, or is superseded.

### D9. Offline detection

Prep found no state-change subscription API in `core/crates/solstone-core-convey-shell/assets/static/websocket.js`.
The existing public signal is `window.appEvents.getMetrics()` at
`core/crates/solstone-core-convey-shell/assets/static/websocket.js:346-356`, which returns `connected`, `state`,
`uptimeMs`, `lastMessageMs`, `lastMessageAt`, and `connectedAt`.

Design:

- Add a tiny browser event hook in `websocket.js` from `updateStatusIcon(state)`
  at `core/crates/solstone-core-convey-shell/assets/static/websocket.js:168-195`.
- The hook announces connection state changes only; it does not change websocket
  reconnect behavior.
- The chat-bar listens for that event only while a sol-ping is unread.
- The chat-bar tracks `disconnectedAt` locally. If disconnected for at least
  30 seconds, set the offline class. Reconnection clears it.
- As a fallback, if the event hook is not available, a 5s interval gated on
  unread sol-ping state may call `window.appEvents.getMetrics()`.

Testability:

- The chat-bar time source should allow a tiny optional
  `window.__solChatTestClock` function. Tests can monkey-patch that function
  without waiting 30 real seconds.
- Keep the hook optional and local to the chat-bar code path. Production code
  falls back to `Date.now()`.

### D10. POST endpoints on `chat_bp`

Add two POST endpoints to `solstone/convey/chat.py`, near the existing
`POST /api/chat` route at `solstone/convey/chat.py:64-108`.

Endpoints:

- `POST /api/chat/sol_chat_request/open`
- `POST /api/chat/sol_chat_request/dismissed`

Behavior:

- Both parse JSON with the existing route style.
- Missing or empty `request_id` returns HTTP 400 with an error object.
- The open endpoint calls `record_owner_chat_open(request_id, surface="convey")`.
- The dismiss endpoint calls
  `record_owner_chat_dismissed(request_id, surface="convey", reason=reason)`.
- JS does not send `surface`; this avoids duplicating the `"convey"` literal in
  client code.
- Success returns `{ok: true}`.

These routes are write routes and use explicit POST verbs. They do not affect
read-only CLI or template paths.

## Module Layout

New files:

- `docs/design/sol_initiated_chat_phase2.md`: this design.
- `solstone/convey/sol_initiated/state.py`: read-only latest-unread request
  helper shared by `apps.py` and `/app/chat`.
- `core/crates/solstone-core-convey-shell/assets/static/sol_initiated_constants.js`: browser constants for the
  four kinds, surface label, and offline tooltip.
- `solstone/observe/sol_chat_filter.py`: observer-side filter/normalizer.

Modified files:

- `solstone/convey/apps.py`: add structured `chat_bar_attention` and
  `chat_bar_sol_request` context.
- `solstone/convey/templates/app.html`: load JS constants, seed chat-bar context,
  add `openConversation`, render sol-ping state, consume live lifecycle events.
- `solstone/convey/templates/chat_bar.html`: add only minimal data/DOM hooks if
  the IIFE cannot use inline JSON cleanly.
- `core/crates/solstone-core-convey-shell/assets/static/app.css`: pulse/offline classes.
- `core/crates/solstone-core-convey-shell/assets/static/websocket.js`: add a tiny connection-state browser
  event hook from `updateStatusIcon`.
- `solstone/convey/chat.py`: add open/dismiss POST endpoints.
- `solstone/apps/chat/routes.py`: record page-load `owner_chat_open` for today's
  latest unresolved request.
- `solstone/apps/chat/workspace.html`: include sol-initiated request bodies in
  live rendering only if needed for anchor visibility. The initial server list
  already creates anchors for every event.
- `solstone/apps/chat/_chat_event.html`: optional, only if the transcript should
  visibly render `sol_chat_request`; not required for D1 because the list item
  anchor exists even when the body partial emits no content.
- `solstone/convey/bridge.py`: per-observer in-memory
  `last_chat_request_at` tracking and reader.
- `solstone/apps/observer/routes.py`: include `last_chat_request_at` in
  `_serialize_observer()`.
- `solstone/apps/observer/workspace.html`: display `last_chat_request_at` in
  observer cards if present.

## Implementation Sequence

1. Add `solstone/convey/sol_initiated/state.py` and focused unit tests for the
   latest-unread request helper.
2. Add backend context in `solstone/convey/apps.py` using the helper.
3. Add JS constants and load them in `app.html`.
4. Update chat-bar IIFE state/rendering, `openConversation`, and live clear
   handling.
5. Add `chat_bp` open/dismiss POST endpoints.
6. Add `/app/chat` page-load open recording using the shared helper.
7. Add bridge in-memory tracking and observer serialization/display.
8. Add observer-side `sol_chat_filter.py`.
9. Add CSS pulse/offline states and websocket connection-state hook or fallback.
10. Add and update tests, then run targeted suites before broader verification.

## Tests

Add or extend:

- `tests/test_sol_initiated_state.py`: latest-unread helper; open/dismiss clear;
  supersede clears old request; returns stable `event_index`; ignores unrelated
  chat events.
- `tests/test_convey_apps.py`: `inject_app_context()` surfaces
  `chat_bar_attention` and `chat_bar_sol_request`; existing placeholder behavior
  remains when neither is present.
- `tests/test_chat_stream_sol_initiated.py`: keep existing stream contract tests;
  add no `_TRIGGER_KINDS` expansion.
- `tests/test_convey_chat_sol_initiated.py`: POST open/dismiss endpoints append
  lifecycle events, validate missing request ids, hardcode `surface="convey"`.
- `solstone/apps/chat/tests/test_routes.py`: `/app/chat/<today>` records
  `owner_chat_open`; past days do not; repeated reloads append repeated opens;
  rendered `events` list excludes the newly appended open.
- `solstone/apps/observer/tests/test_callosum_sse.py`: bridge updates
  `last_chat_request_at` after successful enqueue per subscriber key; slow
  subscriber drop path does not update after failed enqueue.
- `solstone/apps/observer/tests/test_routes.py`: `/api/list` includes
  `last_chat_request_at` as int or null.
- `solstone/apps/observer/tests/test_observer_client_sse.py`: existing SSE frame
  parsing remains compatible.
- `solstone/apps/observer/tests/test_sol_chat_filter.py` or
  `solstone/apps/observer/tests/test_observer_client_sse.py`: filter helper
  returns normalized payloads for four kinds and `None` for non-chat frames.
- Manual smoke: sol-ping renders, pulse clears after the
  test clock advances, open navigates to `/app/chat/<day>#event-<idx>`, dismiss
  clears all tabs.

## Acceptance Criteria Mapping

1. Latest unread `sol_chat_request` appears in chat bar:
   `solstone/convey/apps.py`, `solstone/convey/templates/app.html`,
   `solstone/convey/sol_initiated/state.py`.
2. Chat-bar open action navigates or scrolls to the request anchor:
   `solstone/convey/templates/app.html`,
   `solstone/apps/chat/workspace.html:97-107`.
3. Chat-bar open records `owner_chat_open`:
   `solstone/convey/chat.py`,
   `solstone/convey/sol_initiated/events.py:15-21`.
4. Chat-bar dismiss records `owner_chat_dismissed`:
   `solstone/convey/chat.py`,
   `solstone/convey/sol_initiated/events.py:24-35`.
5. `/app/chat/<today>` page load records `owner_chat_open`:
   `solstone/apps/chat/routes.py:29-45`.
6. Multi-tab clear works from broadcasts:
   `solstone/convey/templates/app.html:591-638`,
   `core/crates/solstone-core-convey-shell/assets/static/websocket.js:267-271`.
7. Per-observer `last_chat_request_at` updates on SSE delivery:
   `solstone/convey/bridge.py:87-112`.
8. `/app/observer/api/list` exposes `last_chat_request_at`:
   `solstone/apps/observer/routes.py:156-177`.
9. Observer cards can display last chat request delivery:
   `solstone/apps/observer/workspace.html:604-616` and `696-720`.
10. Observer-side filter helper normalizes four kinds:
    `solstone/observe/sol_chat_filter.py`.
11. JS constants are centralized:
    `core/crates/solstone-core-convey-shell/assets/static/sol_initiated_constants.js`.
12. Offline pulse behavior has deterministic test path:
    `core/crates/solstone-core-convey-shell/assets/static/app.css`,
    `solstone/convey/templates/app.html`,
    optional `window.__solChatTestClock`.

## Invariants Checklist

- L1 layer boundaries: bridge in-memory state does not write observer domain
  files. Observer persistence remains owned by `solstone/apps/observer/utils.py`.
- L2 domain write ownership: no direct writes to `observers/*.json`; rejected
  persisted alternative would require a helper in the observer owner module.
- L3 naming contract: new read helper uses read-style naming and does not mutate;
  POST routes use write semantics.
- L4 CLI read verbs: no CLI read command changes.
- L5 write-verb defaults: no new CLI write command.
- L6 indexers: no indexer source-data mutation.
- L7 importers: no importer changes.
- L8 hooks: no hook output changes.
- L9 event handler idempotence: page-load open is intentionally not deduped; the
  design documents that each load appends an engagement event. Bridge delivery
  tracking is idempotent enough for repeated events because the latest timestamp
  overwrites the prior value.
- SPDX headers: new source files `sol_initiated_constants.js`,
  `sol_chat_filter.py`, and `state.py` need the standard header. This markdown
  design file does not.
- Trust `get_journal()`: no application code sets `SOLSTONE_JOURNAL`.
- No backwards-compatibility shims: new helpers are direct consumers, not aliases
  or legacy paths.

## Open Trade-offs

- The scope wording says `_TRIGGER_KINDS` lists all four kinds. Prep verified
  `solstone/convey/chat_stream.py:61-66` includes only `sol_chat_request` among
  the new kinds. This design does not modify `_TRIGGER_KINDS`.
- `owner_chat_open` releases dedupe by request id; repeated opens remain
  idempotent no-ops after the first release.
- Repeated `/app/chat/<today>` reloads append repeated `owner_chat_open` events.
  This is accepted as an engagement signal.
- The server-side initial transcript creates anchors for all events, but
  `_chat_event.html` emits no body for sol-initiated kinds. D1 only needs the
  anchor, not visible transcript content.
- `last_chat_request_at` resets on convey restart. Persisting it is intentionally
  rejected for phase 2.
- `_broadcast_to_sse_clients()` sends every event to every subscriber. Updating
  per `subscriber.key_prefix` is correct because every subscriber saw the
  sol-ping frame.
- Browser offline detection has no existing connection-state event hook. Add the
  smallest hook in `websocket.js`; keep a gated polling fallback if needed.
- Pinchtab has no fake-clock primitive. The chat-bar should use a tiny optional
  `window.__solChatTestClock` hook if implementation wants deterministic browser
  verification without a 30 second wait.

## Out-of-scope Reminders

- Do not change phase-1 stream field order.
- Do not add new chat-stream kinds.
- Do not make `owner_chat_open` a trigger kind.
- Do not change `start_chat()` policy order or dedupe semantics.
- Do not persist bridge delivery state into observer records.
- Do not add observer client network behavior beyond the filter helper.
- Do not add phase-3 surfaces or mobile/push delivery.
- Do not migrate existing chat history.
