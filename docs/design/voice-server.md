# Voice server design

## 1. Summary

This design ships a root-level voice API for the existing Convey server: `POST /api/voice/session`, `POST /api/voice/connect`, `POST /api/voice/refresh-brain`, and `GET /api/voice/status`, all mounted from a new `solstone/convey/voice.py` blueprint at `/api/voice/*`. The implementation reuses existing journal, ledger, entity, briefing, and anticipated-activity read surfaces, keeps all voice-owned writes inside `journal/health/voice-brain-session*`, and treats the bridge contract in the scope as canonical when it conflicts with older prose (`solstone/convey/__init__.py:126-155`, `solstone/convey/system.py:18`, `solstone/apps/home/routes.py:149-198`, `solstone/think/surfaces/ledger.py:441-529`, `solstone/think/indexer/journal.py:1865-1948`).

## 2. Module layout

New files:

- `solstone/convey/voice.py` — root-level Flask blueprint for `/api/voice/*`; validates requests, reads app voice state, and bridges HTTP to the background runtime.
- `solstone/think/voice/__init__.py` — small public surface for runtime start/stop helpers and brain refresh helpers.
- `solstone/think/voice/brain.py` — persistent Claude CLI session manager; start, refresh, ask, readiness state, and `journal/health/voice-brain-session*` persistence.
- `solstone/think/voice/runtime.py` — singleton daemon-thread asyncio loop, app attachment, task tracking, and shutdown helper.
- `solstone/think/voice/sideband.py` — OpenAI Realtime sideband join loop, event filter, tool-call output emission, and task cleanup.
- `solstone/think/voice/tools.py` — the 9 tool schemas, argument validation, handler dispatch, and model-facing JSON shaping.
- `solstone/think/voice/config.py` — config readers for OpenAI key, voice model, and brain model.
- `tests/test_voice_config.py` — config-reader defaults, env fallback, and missing-key cases.
- `tests/test_voice_brain.py` — brain prompt content, session persistence, start/refresh/ask flow, and stale-refresh behavior with mocked Claude CLI.
- `tests/test_voice_tools.py` — unit coverage for all 9 tool handlers, including one happy path and one failure path per tool.
- `tests/test_voice_sideband.py` — sideband event routing, tool error wrapping, and output emission.
- `tests/test_voice_runtime.py` — loop/thread lifecycle, idempotent startup, future registration, and explicit shutdown.
- `tests/test_voice_routes.py` — endpoint validation and error-shape coverage with Flask test client.
- `tests/test_voice_integration.py` — end-to-end session mint, sideband task spawn, and one tool dispatch with fake OpenAI clients.

Existing files updated during implementation:

- `solstone/convey/__init__.py` — register `voice_bp` directly beside `system.bp`, then call `start_voice_runtime(app)` before returning the app (`solstone/convey/__init__.py:134-155`).
- `core/fixtures/journal_default.json` — add default `voice` config block so new journals inherit the documented keys.

Deliberate non-change:

- No `solstone/apps/voice/` package. `AppRegistry` would tolerate a custom `url_prefix`, but the app shell assumes `/app/<name>` and `workspace.html`; this feature is a root API, not a Convey app surface (`solstone/apps/__init__.py:124-127`, `solstone/apps/__init__.py:267-271`, `solstone/apps/__init__.py:322-337`, `solstone/convey/apps.py:241-251`).

## 3. Flow diagrams

### Startup sequence

1. `convey.create_app()` registers the existing root/config/triage/system blueprints.
2. `convey.create_app()` registers `voice_bp` at `url_prefix="/api/voice"`.
3. `convey.create_app()` calls `start_voice_runtime(app)`.
4. `start_voice_runtime(app)` creates or reuses the module-level event loop, starts a daemon thread running `loop.run_forever()`, attaches `app.voice_tasks = set()`, and stores voice state on the app.
5. The server begins serving immediately; the voice brain is started lazily on the first `POST /api/voice/session` request or an explicit `POST /api/voice/refresh-brain`.
6. On successful brain startup, `app.voice_brain_session`, `app.voice_brain_instruction`, and `app.voice_brain_refreshed_at` are populated, and the session file is atomically written under `journal/health/`.

### Session mint flow (`POST /api/voice/session`)

1. Validate that the request body is empty or a JSON object.
2. Resolve the OpenAI key through `think.voice.config.get_openai_api_key()`.
3. If no key exists, return HTTP 503 `{"error": "voice unavailable — openai key not configured"}`.
4. If `app.voice_brain_instruction` is empty, ensure a brain start is in flight and wait up to 10 seconds for readiness.
5. If the brain is still not ready after 10 seconds, return HTTP 503 `{"error": "voice unavailable — brain not ready"}`.
6. If the current instruction exists but is older than `BRAIN_REFRESH_MAX_AGE_SECONDS`, queue a non-blocking refresh and continue with the current instruction.
7. Build the 9-tool manifest from `think.voice.tools`.
8. Call `AsyncOpenAI(api_key=...).realtime.client_secrets.create(session=...)` with `model`, `instructions`, `tool_choice="auto"`, `tools`, and the Realtime modalities block.
9. Return `{"ephemeral_key": "<value>"}`.

### Connect + sideband loop (`POST /api/voice/connect`)

1. Validate JSON object body and required non-empty `call_id`.
2. Resolve OpenAI key; if missing, return HTTP 503 key-not-configured.
3. Schedule `_run_sideband(call_id, app)` onto the background loop with `asyncio.run_coroutine_threadsafe(...)`.
4. Insert the returned `Future` into `app.voice_tasks`.
5. Register a done-callback that removes the future from `app.voice_tasks`.
6. Return `{"status": "connected"}` immediately.
7. `_run_sideband(...)` opens `AsyncOpenAI(...).realtime.connect(call_id=call_id, model=get_voice_model())`.
8. `_sideband_loop(...)` consumes events until the call ends or the task is cancelled.

### Tool dispatch

1. `_sideband_loop(...)` filters for `event.type == "response.function_call_arguments.done"`.
2. `think.voice.tools.dispatch_tool_call(name, arguments, call_id, app)` JSON-decodes the arguments and validates the input schema for that tool.
3. The tool handler reads the existing surface and shapes the model-facing response.
4. `_sideband_loop(...)` posts `function_call_output` with the JSON string through `conn.conversation.item.create(...)`.
5. `_sideband_loop(...)` calls `conn.response.create()` so the model can continue the turn.
6. Any tool exception becomes `{"error": "<generic message>"}` and does not end the sideband task.

### Shutdown

1. Process exit or explicit test cleanup calls `stop_voice_runtime(app)`.
2. `stop_voice_runtime(app)` cancels each non-done future in `app.voice_tasks`.
3. `stop_voice_runtime(app)` requests `loop.stop()` with `loop.call_soon_threadsafe(...)`.
4. The daemon thread is joined with a bounded timeout.
5. Module-level runtime state is cleared so the next app instance can start cleanly.

## 4. Endpoint specs

### `POST /api/voice/session`

Request body:

- Empty body is allowed.
- If a body is present it must decode to a JSON object; any other JSON type returns HTTP 400.
- No request fields are currently used.

Success response:

- HTTP 200
- Body: `{"ephemeral_key": "<string>"}`

Failure responses:

- HTTP 400 `{"error": "request body must be a JSON object"}`
- HTTP 503 `{"error": "voice unavailable — openai key not configured"}`
- HTTP 503 `{"error": "voice unavailable — brain not ready"}`
- HTTP 500 `{"error": "voice session unavailable"}` for OpenAI hard failures after logging detail server-side

Side effects:

- Starts brain init if needed.
- Waits up to 10 seconds for first-time brain readiness.
- Queues a non-blocking brain refresh if the current instruction is older than 6 hours.

### `POST /api/voice/connect`

Request body:

- Required JSON object.
- Required `call_id: string`, trimmed, non-empty.

Success response:

- HTTP 200
- Body: `{"status": "connected"}`

Failure responses:

- HTTP 400 `{"error": "request body must be valid JSON"}`
- HTTP 400 `{"error": "call_id is required"}`
- HTTP 503 `{"error": "voice unavailable — openai key not configured"}`
- HTTP 500 `{"error": "voice runtime unavailable"}` if the background loop was not attached

Side effects:

- Schedules a background sideband task.
- Adds the future to `app.voice_tasks`.

### `POST /api/voice/refresh-brain`

Request body:

- Empty body is allowed.
- If a body is present it must decode to a JSON object; contents are ignored by this design.

Success response:

- HTTP 200 `{"status": "refreshed", "instruction_preview": "<first 240 chars>", "brain_ready": true, "brain_age_seconds": 0}`
- HTTP 202 `{"status": "refreshing"}` if the queued refresh has not finished within 30 seconds

Failure responses:

- HTTP 400 `{"error": "request body must be a JSON object"}`
- HTTP 500 `{"error": "brain refresh failed"}` if the queued future raises before returning a fresh instruction

Side effects:

- Forces a brain refresh even if the current instruction is fresh.
- Starts the brain first if no session has been established yet.
- Updates `app.voice_brain_*` state on success.

### `GET /api/voice/status`

Request body:

- None.

Success response:

- HTTP 200
- Body always contains:
  - `brain_ready: bool`
  - `brain_age_seconds: int | null`
  - `openai_configured: bool`
  - `active_sessions: int`

Failure responses:

- No endpoint-specific failure response; if internal state lookup fails, log and return the default falsey payload with HTTP 200.

Side effects:

- None.

## 5. Tool-handler table (canonical)

Rules that apply to every tool:

- Handlers return model-facing JSON objects only.
- Tool exceptions become `{"error": "<generic message>"}` and are returned inline through the sideband, never as HTTP errors.

| Tool | Input JSON | Output JSON | Reused surface | Failure shape |
|---|---|---|---|---|
| `journal.get_day` | `{"day": "YYYY-MM-DD"}` | `{"day": "YYYY-MM-DD", "segments": [{"id": "HHMMSS_LEN", "time_of_day": "HH:MM", "duration_s": 300, "summary": "<string>", "agent_type": "<stream>"}], "summary": "<string>"}` | `think.cluster.scan_day`, `think.cluster.cluster_segments`, and read-only day-path inspection for summary text. Use `scan_day()` for day existence and segment inventory, `cluster_segments()` for normalized segment rows, and synthesize `summary` from per-segment `*_summary.md` files under the day directory without calling `think.utils.segment_path()` because it creates missing directories (`solstone/think/cluster.py:413-505`, `solstone/think/utils.py:155-182`, `solstone/think/utils.py:247-260`) | `{"error": "invalid day"}` or `{"error": "day not found"}` |
| `journal.search` | `{"query": "<string>", "facet": "<string>|null", "days": 30|null, "limit": 10|null}` | `{"results": [{"id": "<id>", "day": "YYYY-MM-DD", "source": "<agent-or-path>", "snippet": "<text>", "entity_slug": "<slug>"?}], "count": N}` | `think.indexer.journal.search_journal(query, limit=limit, facet=facet, day_from=..., day_to=None)` with shaping inspired by `solstone/apps/search/routes.py::_format_result` (`solstone/think/indexer/journal.py:1865-1948`, `solstone/apps/search/routes.py:89-126`, `solstone/apps/search/routes.py:127-240`) | `{"error": "query is required"}` |
| `entities.get` | `{"entity_slug": "<slug>"}` | `{"slug": "<slug>", "name": "<name>", "type": "<type>", "profile": "<markdown>", "tags": ["<facet-or-aka>"], "recent_context": [{"date": "YYYY-MM-DD", "summary": "<string>"}]}` | Primary source is `think.surfaces.profile.full(slug)`. If it resolves, build `profile`, `tags`, and `recent_context` from the returned `Profile`; if facet relationship details are needed, mirror `solstone/apps/entities/routes.py::_build_facet_relationships(...)` and `think.entities.journal.load_journal_entity(slug)` (`solstone/think/surfaces/profile.py:207-245`, `solstone/apps/entities/routes.py:685-730`, `solstone/think/entities/journal.py:43-69`) | `{"error": "not found"}` |
| `entities.recent_with` | `{"entity_slug": "<slug>", "days": 7, "facet": "<string>|null"}` | `{"slug": "<slug>", "interactions": [{"date": "YYYY-MM-DD", "activity": "<title>", "context": "<story-or-description>", "note": "<details>"}], "count": N}` | Resolve the entity with `think.surfaces.profile.full(slug)`, then scan `think.activities.load_activity_records(facet, day)` across the requested day window. Match `participation[].entity_id` first and fall back to casefolded name / aka matching for older rows without `entity_id` (`solstone/think/surfaces/profile.py:207-245`, `solstone/think/activities.py:877-890`, `solstone/apps/activities/call.py:133-197`) | `{"error": "not found"}` or `{"error": "invalid days"}` |
| `commitments.list` | `{"state": "open"|"closed"|"dropped"|null, "facet": "<string>|null", "limit": 20|null}` | `{"commitments": [{"id": "<id>", "owner": "<owner>", "action": "<action>", "counterparty": "<counterparty>", "state": "<state>", "context": "<context>", "day_opened": "YYYY-MM-DD", "day_closed": "YYYY-MM-DD"?, "resolution": "<resolution>"?}]}` | `think.surfaces.ledger.list(state=..., facets=[facet] if facet else None, top=limit or 20)`. Convert each `LedgerItem` dataclass to a dict, drop `sources`, and derive `day_*` strings from the millisecond timestamps. `resolution` is best-effort only: set it to `"dropped"` when `item.state == "dropped"`, otherwise omit because the ledger surface does not expose the close-note resolution (`solstone/think/surfaces/ledger.py:441-487`, `solstone/think/surfaces/types.py:16-32`) | `{"error": "invalid state"}` |
| `commitments.complete` | `{"commitment_id": "lg_...", "resolution": "done"|"sent"|"signed"|"dropped"|"deferred"}` | `{"ok": true, "commitment": {"id": "...", "owner": "...", "action": "...", "counterparty": "...", "state": "...", "context": "...", "day_opened": "YYYY-MM-DD", "day_closed": "YYYY-MM-DD"?, "resolution": "<input-resolution>"}}` | Validate `resolution`. Map `dropped -> as_state="dropped", note="resolution: dropped"`. Map `done|sent|signed|deferred -> as_state="closed", note="resolution: <value>"`. Call `think.surfaces.ledger.close(...)`, catch `KeyError`, and shape the returned `LedgerItem` as above (`solstone/think/surfaces/ledger.py:497-529`, `solstone/think/activities.py:1156-1207`) | `{"error": "invalid resolution"}` or `{"error": "not found"}` |
| `calendar.today` | `{}` | `{"date": "YYYY-MM-DD", "events": [{"time": "HH:MM", "title": "<title>", "attendees": ["<name>"], "location": "<string>", "prep_notes": "<string>"}]}` | `think.activities.load_activity_records(facet, day)` across all enabled facets, filtered to `source == "anticipated"` using the same participation parsing pattern Home uses today (`solstone/apps/home/routes.py:305-337`, `solstone/think/activities.py:877-890`) | `{"error": "today unavailable"}` only on unexpected failures; normal empty day is `{"date": "...", "events": []}` |
| `briefing.get` | `{}` | `{"date": "YYYY-MM-DD", "facet": "identity", "text": "<spoken-English body>", "highlights": ["...", "..."]}` or `{"error": "no briefing today yet"}` | Reuse `solstone.think.briefing.load_briefing(today)` and `render_briefing_sections(...)` exactly. `None` returns the error object. `text` is a plain-text join of the loaded sections; `highlights` comes from `needs_attention` items first, then falls back to the first three bullets across the other sections | `{"error": "no briefing today yet"}` |
| `observer.start_listening` | `{"mode": "meeting"|"voice_memo"}` | `{"status": "ack", "mode": "<mode>", "note": "observer integration not yet wired"}` | No data dependency in this design. Log the requested mode at INFO and return the stub acknowledgement. | `{"error": "invalid mode"}` |

Implementation notes by tool:

- `journal.get_day` normalizes input day strings from `YYYY-MM-DD` to internal `YYYYMMDD`, then returns the external hyphenated form.
- `journal.get_day.summary` is synthesized from per-segment summary files if present; otherwise it is the empty string.
- `journal.search` derives `day_from` as `today - days` when `days` is provided; omit the filter when `days` is null.
- `journal.search.entity_slug` is best-effort and should only be included when it can be inferred from `metadata.path`, such as `entities/<slug>/...` or `entity:<slug>`.
- `entities.get.profile` is a markdown summary synthesized from the `Profile` object: description, cadence sentence, open commitments count, closed commitments count, and the most relevant facets.
- `entities.recent_with` sorts interactions descending by activity timestamp and truncates to a small spoken-friendly limit, default 10.
- `commitments.list` and `commitments.complete` must strip `sources` before returning anything model-facing.
- `calendar.today.location` and `calendar.today.prep_notes` default to `""` because current anticipated activity rows do not guarantee either field.
- `briefing.get.facet` is the literal string `"identity"` as a fixed spoken-context label, not a facet-scoped talent output; the briefing itself is read from `chronicle/<day>/talents/morning_briefing.json`.

## 6. Brain init prompt (full text)

Runtime template values:

- `{agent_name}` comes from `get_config().get("agent", {}).get("name") or "sol"` (`solstone/think/utils.py:557-588`, `tests/fixtures/journal/config/journal.json:70-74`).
- `<today>` is literal prompt text, not a `.format(...)` placeholder; the init
  template only interpolates `{agent_name}`.

Prompt text:

```text
You are preparing the current voice-session instruction for {agent_name}.

Your task right now is to read the current journal state and produce exactly one fresh instruction for an OpenAI Realtime voice session. The instruction must sound like spoken English. Keep it concise, natural, and useful in conversation. No markdown. No bullets. No XML outside the required wrapper tags.

Voice style rules:
- Write for speech, not reading.
- Keep the voice model oriented toward short spoken turns, usually 2 to 4 sentences unless the user clearly asks for more.
- Prefer concrete wording over abstract wording.
- If context is missing, the instruction should say to answer honestly and briefly rather than guessing.

Before you write the instruction, ingest the current context:
- Read the identity material under journal/identity/ and treat {agent_name} as the canonical spoken name.
- Read today's journal summary and today's segment-level summaries if they exist.
- Read the active entities that matter right now.
- Read the open commitments.
- Read today's calendar and anticipated activities.
- Read today's morning briefing at chronicle/<today>/talents/morning_briefing.json if it exists.

Then write one system instruction that does all of the following:
- Establish who {agent_name} is and how the voice should speak.
- Anchor the voice in today's real context.
- Name the most important people, commitments, and upcoming events if they are present.
- Tell the voice to stay concise, spoken, and honest about missing information.
Output only this wrapper and the instruction inside it:
<voice_instruction>
...
</voice_instruction>
```

Notes:

- The prompt intentionally has no `ask_sol` clause. This design ships the 9-tool manifest only.
- `think.voice.brain.ask_brain(...)` still exists as a parity helper for future expansion, but it is not referenced in the prompt or tool manifest.

## 7. Background runtime + shutdown

Exact runtime shape:

- `think.voice.runtime` owns a module-level `RuntimeState` singleton with:
  - `loop: asyncio.AbstractEventLoop | None`
  - `thread: threading.Thread | None`
  - `started: bool`
  - `lock: threading.Lock`
  - `atexit_registered: bool`
- `start_voice_runtime(app)` is idempotent.
- The thread target creates the loop, sets it as current for that thread, and runs `loop.run_forever()`.
- `app.voice_tasks` is attached directly to the Flask app as `set[concurrent.futures.Future]`.
- `app.voice_brain_session`, `app.voice_brain_instruction`, and `app.voice_brain_refreshed_at` are attached directly to the Flask app as mutable voice state.
- `app.voice_runtime_started = True` is attached so routes can fail loudly if startup wiring is missing.

Startup behavior:

1. `convey.create_app()` calls `start_voice_runtime(app)` exactly once per app instance.
2. `start_voice_runtime(app)` initializes the app attributes above.
3. `brain.wait_until_ready(app, timeout)` or `brain.schedule_refresh(app, force=True)` is the first caller that schedules `brain.schedule_start(app)` onto the runtime loop.
4. `brain.schedule_start(app)` either resumes an existing session from `journal/health/voice-brain-session` or starts a new Claude session if none exists.

Sideband scheduling:

- `POST /api/voice/connect` calls `asyncio.run_coroutine_threadsafe(_run_sideband(call_id, app), runtime.loop)`.
- The returned `Future` is inserted into `app.voice_tasks`.
- `future.add_done_callback(app.voice_tasks.discard)` prunes completed tasks.

Shutdown choice:

- Use `atexit.register(stop_voice_runtime)` as the process-level fallback.
- Also export `stop_voice_runtime(app)` and require tests to call it explicitly in teardown.
- Do not use `teardown_appcontext`; it fires per request and is the wrong lifecycle for long-lived sideband tasks.

Signal boundary:

- The Flask dev server and reloader are not treated as a reliable global-shutdown environment for this feature.
- Clean shutdown is expected when the stack runs under the supervisor-managed process model, not `flask run`.
- Tests must explicitly call `stop_voice_runtime(app)` so the daemon thread does not leak across cases.

Brain refresh lifecycle:

- `BRAIN_REFRESH_MAX_AGE_SECONDS = 6 * 3600`.
- Startup only prepares the runtime loop and app state; the first voice session or explicit refresh starts the brain.
- `POST /api/voice/session` queues a refresh when the current instruction is older than the threshold, but does not wait for it.
- `POST /api/voice/refresh-brain` forces a refresh and waits up to 30 seconds because it exists for explicit operator/debug use.

## 8. Config surface

Journal config additions:

```json
{
  "voice": {
    "openai_api_key": null,
    "model": "gpt-realtime",
    "brain_model": "haiku"
  }
}
```

Reader functions:

- `think.voice.config.get_openai_api_key()`:
  - Read `get_config().get("voice", {}).get("openai_api_key")`.
  - If that value is blank, fall back to `OPENAI_API_KEY`.
  - If still blank, return `None`.
- `think.voice.config.get_voice_model()`:
  - Read `config.voice.model`.
  - Default to `"gpt-realtime"`.
- `think.voice.config.get_brain_model()`:
  - Read `config.voice.brain_model`.
  - Default to `"haiku"`.

Error behavior:

- Missing OpenAI key is not a startup crash.
- Missing OpenAI key produces HTTP 503 only when a voice endpoint needs it, with `{"error": "voice unavailable — openai key not configured"}`.

Rationale:

- `get_config()` already defines the repo’s journal-config source of truth (`solstone/think/utils.py:557-588`).
- There is no existing `journal/config/openai.json` consumer in this repo.
- The fixture journal already carries `agent.name = "sol"` but no `voice` block, so the readers must provide defaults (`tests/fixtures/journal/config/journal.json:70-74`).

## 9. Status endpoint fields

- `brain_ready: bool` — `True` when `app.voice_brain_instruction` is a non-empty string.
- `brain_age_seconds: int | null` — `int(time.time() - app.voice_brain_refreshed_at)` when refreshed, otherwise `null`.
- `openai_configured: bool` — `True` if `get_openai_api_key()` resolves a non-empty key at request time.
- `active_sessions: int` — count of futures in `app.voice_tasks` where `future.done()` is false.

All four fields are always present.

## 10. Error shapes (universal)

| Scenario | HTTP code | JSON body |
|---|---|---|
| Request body missing or invalid JSON for an endpoint that expects JSON | 400 | `{"error": "request body must be valid JSON"}` |
| Request body decodes but is not a JSON object | 400 | `{"error": "request body must be a JSON object"}` |
| Missing `call_id` on `/api/voice/connect` | 400 | `{"error": "call_id is required"}` |
| OpenAI key not configured | 503 | `{"error": "voice unavailable — openai key not configured"}` |
| Brain not ready after the `/api/voice/session` 10-second wait | 503 | `{"error": "voice unavailable — brain not ready"}` |
| OpenAI session mint hard failure | 500 | `{"error": "voice session unavailable"}` |
| Background runtime missing | 500 | `{"error": "voice runtime unavailable"}` |
| Explicit brain refresh future raises | 500 | `{"error": "brain refresh failed"}` |
| Tool handler exception in sideband | n/a, inline tool output | `{"error": "<message>"}` |

Logging rule:

- HTTP and tool errors return generic client-safe strings.
- Diagnostic detail stays in logs only.

## 11. Test strategy

Uniform fixture-date strategy:

- Add one narrow helper in `think.voice.tools`, for example `_today() -> datetime.date` plus a formatter helper in the same module.
- All date-sensitive voice tools (`journal.get_day`, `journal.search` day-window math, `calendar.today`, `briefing.get`) use that helper.
- Tests monkeypatch the helper to the fixture briefing date or another explicit date instead of rewriting shared fixture files.
- This keeps the shared fixture journal stable and avoids clock-driven flakes from `tests/fixtures/journal/chronicle/20260327/talents/morning_briefing.json` being dated `20260327`.

Per-file plan:

| Test file | Coverage | Fixtures used | Mocking |
|---|---|---|---|
| `tests/test_voice_config.py` | `get_openai_api_key`, `get_voice_model`, `get_brain_model`, config defaults, env fallback | `tests/conftest.py` default fixture journal | `monkeypatch.setenv` only |
| `tests/test_voice_brain.py` | prompt rendering, session-file load/save/touch, start/refresh/ask control flow, 6-hour stale threshold, readiness state | fixture journal or `journal_copy` for isolated `health/` state | mock Claude CLI subprocess entry points such as `asyncio.create_subprocess_exec` or a small `_run_claude(...)` seam |
| `tests/test_voice_tools.py` | all 9 tool handlers, one happy path and one failure path each, `sources` stripping | real fixture journal via `tests/conftest.py`; `journal_copy` for close/edit cases | monkeypatch `_today()` and any narrow parser seams; do not mock journal contents |
| `tests/test_voice_sideband.py` | event filter, argument decode errors, tool dispatch, `function_call_output` emission, `conn.response.create()` cadence | no special journal beyond the fixture default | fake `conn` object and patched dispatcher |
| `tests/test_voice_runtime.py` | singleton startup, duplicate start no-op, future registration and pruning, explicit shutdown, atexit-registration guard | none beyond a minimal Flask app | patch thread join timing if needed |
| `tests/test_voice_routes.py` | endpoint validation: bad JSON, missing key, missing `call_id`, status payload defaults | Flask app from `convey.create_app()` with fixture journal | patch `think.voice.config.get_openai_api_key`, `brain.wait_until_ready`, and `AsyncOpenAI` as needed |
| `tests/test_voice_integration.py` | full flow: session mint, connect, one tool event through sideband, active session count transition | real fixture journal plus `journal_copy` when a tool mutates ledger state | patch the `openai` module with fake `AsyncOpenAI`; this follows the existing module-patching precedent in `tests/test_validate_key.py:56-75` |

Specific integration test shape:

1. Build the Flask app with the fixture journal override already supplied by `tests/conftest.py`.
2. Patch `openai.AsyncOpenAI` or the voice-module import site with a fake client that:
   - returns a fixed ephemeral key from `realtime.client_secrets.create(...)`
   - yields a scripted tool-call event stream from `realtime.connect(...)`
   - tracks `conversation.item.create(...)` and `response.create(...)` calls
3. Seed `app.voice_brain_instruction` to avoid the first-session wait in the happy-path integration case.
4. `POST /api/voice/session` and assert the key plus tool manifest wiring.
5. `POST /api/voice/connect` with a fake `call_id`.
6. Let the fake sideband drive one tool call.
7. Assert the OpenAI output JSON does not contain `_nav_target`.

Journal-data rule:

- Unit and integration tests both read the real fixture journal.
- No fake journal abstraction is introduced.
- Mutation tests use `journal_copy` so shared fixture files remain unchanged (`tests/conftest.py:61-68`).

## 12. Open questions / deviations for operator approval (gate list)

- Brain-not-ready behavior: this design treats the bridge contract and acceptance list as canonical and returns HTTP 503 from `/api/voice/session` after a 10-second wait, instead of using the older static fallback instruction path from the scope prose.
- Routing location: this design uses a root-level `solstone/convey/voice.py` blueprint, not `solstone/apps/voice/`, because the feature is a root API and the app shell assumes `/app/<name>` plus `workspace.html`.
- Briefing source path: `solstone.think.briefing.load_briefing(...)` reads the canonical `chronicle/<day>/talents/morning_briefing.json` talent output. (Updated 2026-07-02: an earlier revision read the phantom identity-dir briefing file; retired in the H1 cleanup.)
- Commitments resolution mapping: this design maps `done|sent|signed|deferred -> as_state="closed"` and `dropped -> as_state="dropped"` because `think.surfaces.ledger.close(...)` only accepts `closed|dropped`.
- OpenAI key sourcing: this design uses `config.voice.openai_api_key` in `journal/config/journal.json` first, then `OPENAI_API_KEY`, and does not add `journal/config/openai.json`.
- `ask_sol` clause: this design removes it from the brain init prompt and does not add a 10th tool to the manifest.
- Decision-record location: this design keeps the voice decisions in `docs/design/voice-server.md` .
- Config keys: this design adds a `voice` block to journal config and defaults, which is a scope-visible contract change.

## 13. Risks / sharp edges

- Async runtime on a daemon thread plus Flask test client: if tests do not call `stop_voice_runtime(app)`, the loop thread can leak across test cases.
- OpenAI Realtime API stability: the repo pins `openai>=1.2.0`, but the local environment on 2026-04-19 reports `openai 2.17.0`; implementation should lock tests to the currently observed surfaces `client.realtime.client_secrets.create(session=...)` and `client.realtime.connect(call_id=..., model=...)`.
- Brain subprocess prompt safety: the prompt itself is fixed and repo-controlled, but the Claude CLI invocation must avoid shell interpolation and must pass arguments as a list to `create_subprocess_exec`.
- Fixture briefing date drift: without the `_today()` seam, `briefing.get` and date-window tools would fail when run against the real system clock.
- `commitments.list.resolution` diminishment is accepted as an accepted known limit. The ledger surface stores resolution nuance in close-note edits rather than a typed field, so `commitments.list` surfaces `"dropped"` only for dropped items and omits the field otherwise. Full resolution carry-through remains available for a follow-up if post-ship live validation reveals a real need.
- `sources` leakage: `LedgerItem.sources` is provenance, not model-facing data. The tool layer must strip it before returning commitments to OpenAI.
- `segment_path()` still creates directories by default for write paths; read-only callers (for example segment-summary readers) must pass `create=False`.
