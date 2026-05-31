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
**Source:** `solstone/think/cortex.py`
**Events:** `request`, `start`, `thinking`, `tool_start`, `tool_end`, `finish`, `error`, `talent_updated`, `info`, `status`
**Details:** See [CORTEX.md](CORTEX.md) for agent lifecycle, configuration, and event schemas

### `supervisor` - Process lifecycle management
**Source:** `solstone/think/supervisor.py`
**Events:** `started`, `stopped`, `restarting`, `status`, `queue`
**Listens for:** `request` (task spawn), `restart` (service restart)
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
**Source:** `solstone/think/runner.py`
**Events:** `exec`, `line`, `exit`
**Key fields:** `ref` (correlates with supervisor), `name`, `stream` (stdout/stderr), `line`
**Purpose:** Real-time stdout/stderr streaming and process exit events

### `observe` - Multimodal capture and processing
**Sources:**
- Capture: standalone observer services (solstone-linux, solstone-tmux, solstone-macos) upload vian observer ingest
- Processing: `solstone/observe/sense.py`, `solstone/observe/describe.py`, `solstone/observe/transcribe/`

**Events:**
| Event | Emitter | Purpose |
|-------|---------|---------|
| `status` | sense | Periodic state (every 5s) - see `emit_status()` in source |
| `observing` | ingest | Recording window boundary crossed, files saved |
| `detected` | sense | File detected, handler spawned |
| `described` | describe | Vision analysis complete |
| `transcribed` | transcribe | Audio transcription complete (includes VAD metadata) |
| `observed` | sense | All files for segment fully processed (may include errors) |

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
**Source:** `solstone/think/importers/cli.py`
**Events:** `started`, `status`, `completed`, `error`
**Key fields:** `import_id` (correlates all events), `stage`, `segments` (created segment keys), `stream` (stream name, e.g., `"import.apple"`)
**Stages:** `initialization`, `segmenting`, `transcribing`, `summarizing`
**Purpose:** Track media file import from upload through transcription to segment creation

### `think` - Generator and agent processing
**Source:** `solstone/think/thinking.py`
**Events:** `started`, `status`, `group_started`, `group_completed`, `talent_started`, `talent_completed`, `completed`, `segments_started`, `segments_completed`
**Key fields:** `mode` ("daily"/"segment"/"activity"/"flush"), `day`, `segment` (when mode="segment" or "flush"), `activity` and `facet` (when mode="activity")
**Purpose:** Track think processing from generators through scheduled agents
**`status`** - Periodic progress (every ~5s). Fields: `mode`, `day`, `segment`, `stream`, `agents_completed`, `agents_total`, `current_group_priority`, `current_agents` (list of running agent names). In `--segments` batch mode, also includes `segments_completed`, `segments_total`. In activity mode, includes `activity`, `facet`.

### `activity` - Activity lifecycle events
**Sources:** `solstone/talent/activity_state.py` (post-hook), `solstone/talent/activities.py` (post-hook)
**Events:** `live`, `recorded`
**Event Log:** Logged to `<day>/<segment>/events.jsonl` by supervisor

**`live`** - Emitted per active activity per segment (new or continuing). Provides real-time activity tracking.
**Key fields:** `facet`, `day`, `segment`, `id`, `activity` (type), `since`, `description`, `level`, `active_entities`

**`recorded`** - Emitted when a completed activity record is written to journal. Supervisor queues a per-activity think task on receipt.
**Key fields:** `facet`, `day`, `segment`, `id`, `activity` (type), `segments` (full span), `level_avg`, `description`, `active_entities`

### `sync` - Observer segment synchronization
**Source:** `solstone/observe/sync.py`
**Events:** `status`
**Key fields:** `queue_size`, `segment`, `state`, `host`, `platform`
**Purpose:** Track observer sync service status for segment uploads to central server

### `notification` - In-app notification display
**Source:** `solstone/convey/static/websocket.js` (client-side listener; any service can emit)
**Events:** any (event name is not interpreted)
**Key fields:** `title` (string), `message` (string), `icon` (string, emoji), `action` (string, URL path), `facet` (string), `autoDismiss` (number, ms), `app` (string, app name)
**Defaults:** `app` → "system", `icon` → "📬", `title` → "Notification" (applied by `AppServices.notifications.show()`)
**Purpose:** Forward Callosum events directly to the browser notification UI — any service can trigger an in-app notification card by emitting to this tract

**Example:**
```python
callosum_send("notification", "show", title="Import Complete", message="3 segments imported", icon="📥", autoDismiss=5000)
```

### `navigate` - Browser navigation control
**Source:** `solstone/think/tools/navigate.py` (`journal navigate`)
**Events:** `request`
**Key fields:** `path` (string, URL path), `facet` (string, facet name) — at least one required
**Consumer:** `solstone/convey/static/websocket.js` (built-in listener)
**Purpose:** Navigate the browser to a URL path and/or switch to a facet — facet-only triggers `selectFacet()` without page reload, path triggers full page load, path+facet sets facet cookie before navigating

---

## Key Concepts

**Correlation ID (`ref`):** Universal identifier for process instances, used across tracts to correlate events. Auto-generated as epoch milliseconds if not provided.

**Field Semantics:**
- `service` - Human-readable name (e.g., "cortex", "sol import")
- `ref` - Unique instance ID (changes on each restart)
- `pid` - Operating system process ID

---

## Implementation

**Source:** `solstone/think/callosum.py`

### Client APIs

**`CallosumConnection`** - Long-lived bidirectional connection with background thread
```python
from solstone.think.callosum import CallosumConnection

conn = CallosumConnection()
conn.start(callback=handle_message)  # Start with optional message handler
conn.emit("tract", "event", field1="value")  # Queue message for send
conn.stop()  # Clean shutdown
```

**`callosum_send()`** - One-shot fire-and-forget for simple cases
```python
from solstone.think.callosum import callosum_send

callosum_send("observe", "described", day="20251102", segment="143045_300")
```

**`CallosumServer`** - Broadcast server (started in-process by supervisor)

### Convey Integration

- `convey.emit()` - Non-blocking emission from route handlers (uses shared bridge connection)
- `apps.events` - Server-side event handlers via `@on_event` decorator

See [APPS.md](APPS.md) for app event handler patterns.

### CLI Tools

**`sol callosum`** / **`sol callosum listen`** - Listen to events on the message bus
```bash
sol callosum                              # Stream all events as JSONL
sol callosum listen --tract cortex        # Filter to cortex tract
sol callosum listen --event finish -p     # Pretty-print finish events
```

**`sol callosum send`** - Send a message to the bus
```bash
sol callosum send observe described day=20250101 segment=143045_300
sol callosum send '{"tract":"test","event":"ping","data":42}'
echo '{"tract":"test","event":"ping"}' | sol callosum send
```

---

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
    ↓ solstone/apps/entities/events.py updates entity activity
activity.recorded (activity span completed)
    ↓ supervisor queues per-activity think
think --activity (runs schedule="activity" agents)

[If no new segments for FLUSH_TIMEOUT (1h):]
    ↓ supervisor queues flush
think --flush (runs hook.flush agents to close dangling state)
```

See `solstone/think/supervisor.py:_handle_segment_observed()` for the observe→think trigger and `_handle_activity_recorded()` for activity→think.

**Activity-scheduled agents** declare `schedule: "activity"` with a required `activities` list (activity types to match, or `["*"]` for all). They receive the activity's segment span as transcript source and `$activity_*` template variables in their prompts.

### Status Event Pattern

Long-running services emit `status` events every 5 seconds for health monitoring:
- Supervisor checks event freshness to detect stale processes
- UI displays live state from status events
- See status emission methods in observer, sense, cortex for examples

### Request/Response via Callosum

For async task dispatch, use supervisor's request handling:
```python
from convey import emit
emit("supervisor", "request", ref=task_id, cmd=["sol", "import", path])
```

For agent requests, use the cortex client:
```python
from solstone.think.cortex_client import cortex_request
use_id = cortex_request(prompt="...", name="default")
```

See `solstone/think/cortex_client.py` for the full API.
