# Callosum Protocol

Callosum is a JSON-per-line message bus for real-time event distribution across solstone services.

## Protocol

**Transport:** Unix domain socket at `journal/health/callosum.sock`

**Format:** Newline-delimited JSON. Broadcast to all connected clients.

**Message Structure:**
```json
{
  "tract": "source_subsystem",
  "event": "event_type",
  "ts": 1234567890123,
  // ... tract-specific fields
}
```

**Required Fields:**
- `tract` - Source subsystem identifier (string)
- `event` - Event type within tract (string)
- `ts` - Timestamp in milliseconds (auto-added by server if missing)

**Behavior:**
- All connections are bidirectional (can emit and receive)
- No routing, no filtering - all messages broadcast to all clients
- Clients should drain socket continuously to prevent backpressure

---

## Tract Registry

> **Note:** This registry is kept intentionally high-level. For detailed field schemas and current implementation, always refer to the source files listed - they are the authoritative reference.

### `cortex` - Agent execution events
**Source:** `solstone-core-cortex`
**Events:** `request`, `start`, `thinking`, `tool_start`, `tool_end`, `finish`, `error`, `talent_updated`, `info`, `status`, `cancel`, `dry_run`, `progress`, `text_delta`, `tool_budget_exhausted`, `warning`, `budget_escalation`
**Details:** See [CORTEX.md](CORTEX.md) for agent lifecycle, configuration, and event schemas

**`info` note:** `info` remains the correct vocabulary for cortex telemetry. The current non-JSON stdout fallback in `_monitor_stdout()` writes an `info` record to the durable use-log through `_append_use_event()`; it does not broadcast that fallback record to Callosum. This is a bus-wiring gap, not a naming mismatch.

### `work` - Talent-run, reflection, and support-draft events
**Source:** talent-run and support-draft producers. There is no native producer on this tract today; the vocabulary is closed so a future dispatcher cannot silently grow it.
**Events:** `talent_queued`, `talent_spawned`, `talent_finished`, `talent_errored`, `result`, `reflection_ready`, `support_draft`, `support_submit_claim`
**Purpose:** Live talent-run status, thinking `reflection_ready`, and support draft/submit-claim. Unknown event kinds are rejected. Closedness lives in `callosum.work.event` (`classification: closed`, `unknown_value_behavior: reject`), published in the OpenAPI `x-vocabularies` and the observer-client `manifest.json` `vocabularies[]`. The registry-level `callosum.tract_event` stays extensible. This tract used to be named `chat`; it was renamed rather than folded into `cortex` (1:1 with the cogitate wire contract), `think` (the daily-think pipeline, not live run status), or `support` (an open registry list). `work` is also an owner-facing facet id — tract and facet are different namespaces.

### `supervisor` - Process lifecycle management
**Source:** `solstone-core-system` (supervisor)
**Events:** `started`, `stopped`, `restarting`, `status`, `queue`, `request`, `restart`, `drain`, `skipped`, `sync_conflict`
**Listens for:** `request` (task spawn), `restart` (service restart), `drain` (catchup work)
**Key fields:** `ref` (instance ID), `service` (name), `pid`, `exit_code`
**Purpose:** Unified lifecycle events for all supervised processes (services and tasks)

**Per-command task queue:** Tasks are serialized by command name (e.g., "indexer"):
- If no task with that command is running → run immediately
- If command is already running → queue the request (FIFO)
- Deduped by exact `cmd` match (same command+args won't queue twice)
- When task completes → next queued request runs automatically

**Ref tracking:** Callers can provide a `ref` field in requests to track completion:
- If omitted, supervisor generates a timestamp-based ref
- `stopped` events include the ref, allowing callers to match their request
- When duplicate requests are deduped, their refs are coalesced - all refs receive `stopped` events when the single execution completes

**Queue event:** Emitted when queue state changes:
```json
{"tract": "supervisor", "event": "queue", "command": "indexer", "running": "ref123", "queued": 2, "queue": [{"refs": ["ref456"], "cmd": ["sol", "indexer", "--rescan"]}]}
```

### `logs` - Process output streaming
**Source:** `solstone-core-think-cli`
**Events:** `exec`, `line`, `exit`
**Key fields:** `ref` (correlates with supervisor), `name`, `stream` (stdout/stderr), `line`
**Purpose:** Real-time stdout/stderr streaming and process exit events

### `observe` - Multimodal capture and processing
**Sources:**
- Capture: standalone observer services (solstone-linux, solstone-tmux, solstone-macos) upload vian observer ingest
- Processing: native `journal sense`, `solstone-core-describe`, native `journal transcribe`

**Events:**
| Event | Emitter | Purpose |
|-------|---------|---------|
| `status` | sense | Periodic state (every 5s) - see `emit_status()` in source |
| `observing` | ingest | Recording window boundary crossed, files saved |
| `detected` | sense | File detected, handler spawned |
| `described` | describe | Vision analysis complete |
| `transcribed` | transcribe | Audio transcription complete (includes VAD metadata) |
| `observed` | sense | All files for segment fully processed (may include errors) |
| `memory_throttle_started` | sense | Handler waiting for memory headroom |
| `memory_throttle_completed` | sense | Handler admitted or stopped after memory throttle |

**Common fields:** `day`, `segment`, `observer` (for observer uploads), `stream` (stream name, e.g., `"archon"`, `"import.apple"`)
**`observing` event fields:**
- `meta` (dict, optional): Metadata dict from observer. Contains `host`, `platform`, and any client-provided fields (e.g., `facet`, `setting`). Passed to handlers via `SEGMENT_META` env var and unrolled into JSONL metadata headers.
- `stream` (str, optional): Stream name identifying the segment source. Set by observers, observer ingest, and importer.

**`observed` event fields:**
- `stream` (str, optional): Stream name, forwarded from the originating `observing` event.
- `error` (bool, optional): `true` if any handler failed during segment processing
- `errors` (list[str], optional): Error descriptions for failed handlers (e.g., `["transcribe exit 1"]`)

**Correlation:** `detected.ref` matches `logs.exec.ref`; `segment` groups files from same capture window
**Event Log:** Observe, think, and activity tract events with `day` + `segment` are logged to `<day>/<segment>/events.jsonl` by supervisor

### `importer` - Media import processing
**Source:** `solstone-core-import`
**Events:** `started`, `status`, `completed`, `error`, `file_imported`, `enrichment_ready`
**Key fields:** `import_id` (correlates all events), `stage`, `segments` (created segment keys), `stream` (stream name, e.g., `"import.apple"`)
**Stages:** `initialization`, `segmenting`, `transcribing`, `summarizing`
**Purpose:** Track media file import from upload through transcription to segment creation

### `link` - Secure listener and device-link events
**Source:** `solstone-core-convey-shell` (`network.rs`) and `solstone-core-sol-link`
**Events:** `pair_complete`, `last_seen`, `stream_reset`
**Purpose:** Report device pairing, secure-listener handshake activity, and stream-reset diagnostics. `pair_complete` is emitted directly by the network route. The secure-listener runtime supplies a `link`-tract callback that relays the `last_seen` and `stream_reset` events emitted by its accept loop.

### `think` - Generator and agent processing
**Source:** `solstone-core-thinking`
**Events:** `started`, `status`, `group_started`, `group_completed`, `talent_started`, `talent_completed`, `completed`, `segments_started`, `segments_completed`, `memory_throttle_started`, `memory_throttle_completed`, `daily_complete`
**Key fields:** `mode` ("daily"/"segment"/"activity"/"flush"), `day`, `segment` (when mode="segment" or "flush"), `activity` and `facet` (when mode="activity")
**Purpose:** Track think processing from generators through scheduled agents
**`status`** - Periodic progress (every ~5s). Fields: `mode`, `day`, `segment`, `stream`, `agents_completed`, `agents_total`, `current_group_priority`, `current_agents` (list of running agent names). In `--segments` batch mode, also includes `segments_completed`, `segments_total`. In activity mode, includes `activity`, `facet`.

### `activity` - Activity lifecycle events
**Source:** `solstone-core-talent-runtime` (activity hooks)
**Events:** `live`, `recorded`
**Event Log:** Logged to `<day>/<segment>/events.jsonl` by supervisor

**`live`** - Emitted per active activity per segment (new or continuing). Provides real-time activity tracking.
**Key fields:** `facet`, `day`, `segment`, `id`, `activity` (type), `since`, `description`, `level`, `active_entities`

**`recorded`** - Emitted when a completed activity record is written to journal. Supervisor queues a per-activity think task on receipt.
**Key fields:** `facet`, `day`, `segment`, `id`, `activity` (type), `segments` (full span), `level_avg`, `description`, `active_entities`

### `storage` - Storage health warnings
**Sources:** `solstone-core-system` (supervisor) and `solstone-core-thinking`
**Events:** `warning`
**Purpose:** Surface storage conditions that need attention while retaining a shared notification path for owner-facing alerts.

### `support` - Proactive support suggestions
**Source:** `solstone-core-support-web`
**Events:** `proactive_suggestion`
**Purpose:** Signal a support suggestion when the support event handler identifies a qualifying condition.

### `notification` - In-app notification display
**Source:** `core/crates/solstone-core-convey-shell/assets/static/websocket.js` (client-side listener; any service can emit)
**Events:** any (event name is not interpreted)
**Key fields:** `title` (string), `message` (string), `icon` (string, Lucide icon name), `action` (string, URL path), `autoDismiss` (number, ms), `app` (string, app name)
**Defaults:** `app` → "system", `icon` → "mailbox", `title` → "Notification" (applied by `AppServices.notifications.show()`)
**Purpose:** Forward Callosum events directly to the browser notification UI — any service can trigger an in-app notification card by emitting to this tract

### `navigate` - Browser navigation control
**Source:** `journal navigate` (`solstone-core-journal-cli`)
**Events:** `request`
**Key fields:** `path` (string, URL path)
**Consumer:** `core/crates/solstone-core-convey-shell/assets/static/websocket.js` (built-in listener)
**Purpose:** Navigate the browser to a URL path. Workspace-local facet selection stays in that workspace's URL/query contract.

---

## Key Concepts

**Correlation ID (`ref`):** Universal identifier for process instances, used across tracts to correlate events. Auto-generated as epoch milliseconds if not provided.

**Field Semantics:**
- `service` - Human-readable name (e.g., "cortex", "sol import")
- `ref` - Unique instance ID (changes on each restart)
- `pid` - Operating system process ID

---

## Implementation

The bus is `solstone-core-callosum`. The socket is
`journal/health/callosum.sock`. Messages are one JSON object per line with
`tract`, `event`, `ts`, plus event fields.

Convey emits through the shell bridge. Native crates use the callosum client
in `solstone-core-callosum`. There is no Python `CallosumConnection` and no
`@on_event` app handler.

## Common Patterns

### Event-Driven Processing Chain

The observe pipeline demonstrates event-driven handoffs:

```
observe.observing (files saved)
    ↓ sense (listening via Callosum)
observe.detected (handler spawned)
    ↓ logs.exec (process started)
observe.described / observe.transcribed (processing complete)
    ↓ sense tracks completion
observe.observed (segment fully processed)
    ↓ supervisor triggers think, tracks flush timer
think.completed
    ↓ solstone-core-entity / talent-runtime updates entity activity
activity.recorded (activity span completed)
    ↓ supervisor queues per-activity think
think --activity (runs schedule="activity" agents)

[If no new segments for FLUSH_TIMEOUT (1h):]
    ↓ supervisor queues flush
think --flush (runs hook.flush agents to close dangling state)
```

See `solstone-core-system` for the observe→think trigger and the activity→think queue.

**Activity-scheduled agents** declare `schedule: "activity"` with a required `activities` list (activity types to match, or `["*"]` for all). They receive the activity's segment span as transcript source and `$activity_*` template variables in their prompts.

### Status Event Pattern

Long-running services emit `status` events every 5 seconds for health monitoring:
- Supervisor checks event freshness to detect stale processes
- UI displays live state from status events
- See status emission methods in observer, sense, cortex for examples

### Request/Response via Callosum

For async task dispatch, emit `supervisor` / `request` on Callosum. For
talent requests, emit `cortex` / `request` (see [CORTEX.md](CORTEX.md)).
The native client is `solstone-core-cortex-client`.
