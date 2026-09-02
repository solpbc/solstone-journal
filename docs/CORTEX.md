# Cortex API and Eventing

The Cortex system manages AI talent execution through the Callosum message bus with file-based persistence. It acts as a process manager for talent instances, receiving requests via Callosum and writing execution events to both JSONL files (for persistence) and the message bus (for real-time distribution).

For details on the Callosum protocol and message format, see [CALLOSUM.md](CALLOSUM.md).

## Architecture

### Event Flow
1. **Request Creation**: A client (Convey, `journal talent`, or `solstone-core-cortex-client`) broadcasts to Callosum (`tract="cortex"`, `event="request"`)
2. **Request Reception**: Cortex receives message via Callosum callback and creates `<name>/<timestamp>_active.jsonl`
3. **Talent Spawning**: Cortex resolves the sibling `solstone-core` binary and spawns `solstone-core __talent-worker` with the raw request
4. **Event Emission**: Talents write JSON events to stdout (captured by Cortex)
5. **Event Distribution**: Cortex appends events to JSONL file AND broadcasts to Callosum
6. **Agent Completion**: Cortex renames file to `<name>/<timestamp>.jsonl` when agent finishes

### Key Components
- **Message Bus Integration**: Cortex connects to Callosum to receive requests and broadcast events
- **Process Management**: Spawns talent subprocesses (both tool talents and generators)
- **Execution-Fact Resolution**: Resolves talent type, declared cwd, and timeout with `resolve_execution_facts` before spawning the native worker; Cortex resolves no interpreter
- **Configuration Delegation**: Passes raw requests to `solstone-core __talent-worker`, whose native talent runtime loads and prepares the talent configuration
- **Event Capture**: Monitors agent stdout/stderr and appends to JSONL files
- **Dual Event Distribution**: Events go to both persistent files and real-time message bus
- **NDJSON Input Mode**: The native worker accepts newline-delimited raw request JSON via stdin, then composes the talent configuration

### File States
- `<name>/<timestamp>_active.jsonl`: Talent currently executing (Cortex is appending events)
- `<name>/<timestamp>.jsonl`: Talent completed (contains full event history)

**Note**: Files provide persistence and historical record, while Callosum provides real-time event distribution to all interested services.

## Request Format

Requests are Callosum messages on the `cortex` tract. The request message follows this format:

```json
{
  "event": "request",
  "ts": 1234567890123,              // Required: millisecond timestamp (must match use_id in filename)
  "prompt": "Analyze this code for security issues",  // Required for talents (not generators)
  "name": "default",              // Optional: talent name from talent/*.md
  "provider": "openai",              // Optional: override provider (openai, google, anthropic)
  "max_output_tokens": 8192,        // Optional: maximum response tokens
  "thinking_budget": 10000,         // Optional: thinking token budget (ignored by OpenAI)
  "session_id": "sess-abc123",       // Optional: CLI session ID for continuation
  "facet": "my-project",          // Optional: project context
  "output": "md",                     // Optional: output format ("md" or "json"), writes to talents/
  "day": "20250109",                  // Optional: YYYYMMDD format, defaults to current day
  "env": {                           // Optional: environment variables for subprocess
    "API_KEY": "secret",
    "DEBUG": "true"
  }
}
```

The model is resolved from the active brain in `config/journal.json`
(`providers.active`). Request-time provider/model pins may be supplied, but
there is no tier-based fallback or backup-provider routing. See
[PROVIDERS.md](PROVIDERS.md).

## Generator Request Format

Generators are spawned via Cortex when a request has an `output` field but no `tools` field. They produce analysis output (markdown or JSON) from clustered transcripts.

```json
{
  "event": "request",
  "ts": 1234567890123,              // Required: millisecond timestamp
  "name": "activity",               // Required: generator name from talent/*.md
  "day": "20250109",                // Required: day in YYYYMMDD format
  "output": "md",                   // Required: output format ("md" or "json")
  "segment": "120000_300",          // Optional: single segment key (HHMMSS_duration)
  "span": ["120000_300", "120500_300"],  // Optional: list of sequential segment keys
  "output_path": "/path/to/file.md", // Optional: override output location
  "refresh": false,                 // Optional: regenerate even if output exists
  "provider": "google",             // Optional: AI provider override
  "model": "gemini-2.0-flash"       // Optional: model override
}
```

### Generator Events

Generators emit the same event types as talents:
- `start` - When generation begins
- `finish` - On completion, with `result` containing generated content
- `error` - On failure

The `finish` event may include a `skipped` field when generation is skipped:
- `"no_input"` - Insufficient transcript content to analyze
- `"disabled"` - Generator is marked as disabled in frontmatter

### Conversation Continuations

Conversation continuation is owned by the active provider runtime. Include a
`session_id` field in the request with the session ID from a previous talent's
finish event; the native provider runtime continues the conversation.

Continuations must use the same provider that started the conversation.
`session_id` is the continuation handle.

## Agent Event Format

All subsequent lines are JSON objects with `event` and millisecond `ts` fields. The `ts` field is automatically added by Cortex if not provided by the provider. Additionally, Cortex automatically adds an `use_id` field (matching the timestamp component in the filename) to all events for tracking purposes.

### request
The initial spawn request (first line of file, written by client).
```json
{
  "event": "request",
  "ts": 1234567890123,
  "use_id": "1234567890123",
  "prompt": "User's task or question",
  "provider": "openai",
  "name": "default",
  "output": "md",
  "day": "20250109"
}
```

### cancel
Inbound request to stop a running talent use. Cortex queues cancellation work off
the receive thread and terminalizes the use with an error carrying `reason_code`.
```json
{
  "event": "cancel",
  "use_id": "1234567890123",
  "reason_code": "talent_watchdog_cancelled"
}
```

### start
Emitted when a talent run begins.
```json
{
  "event": "start",
  "ts": 1234567890123,
  "use_id": "1234567890123",
  "name": "default",
  "model": "gpt-4o",
  "session_id": "sess-abc"
}
```

### tool_start
Emitted when a tool execution begins.
```json
{
  "event": "tool_start",
  "ts": 1234567890123,
  "use_id": "1234567890123",
  "tool": "search_journal",
  "args": {"query": "search terms", "limit": 10},
  "call_id": "search_journal-1"
}
```

### tool_end
Emitted when a tool execution completes.
```json
{
  "event": "tool_end",
  "ts": 1234567890123,
  "use_id": "1234567890123",
  "tool": "search_journal",
  "args": {"query": "search terms"},
  "result": ["result", "array", "or", "object"],
  "call_id": "search_journal-1"
}
```

### thinking
Emitted when the model produces reasoning/thinking content (model-dependent, primarily o1 models).
```json
{
  "event": "thinking",
  "ts": 1234567890123,
  "use_id": "1234567890123",
  "summary": "Model's internal reasoning about the task...",
  "model": "o1-mini"
}
```

### progress
Emitted by synchronous generator runs as a liveness heartbeat while provider work
is still in flight. It intentionally carries no `summary`.
```json
{
  "event": "progress",
  "ts": 1234567890123,
  "use_id": "1234567890123",
  "phase": "generate"
}
```

### talent_updated
Emitted when control is handed off to a different agent (multi-agent scenarios).
```json
{
  "event": "talent_updated",
  "ts": 1234567890123,
  "use_id": "1234567890123",
  "agent": "SpecializedAgent"
}
```

### finish
Emitted when the talent run completes successfully.
```json
{
  "event": "finish",
  "ts": 1234567890123,
  "use_id": "1234567890123",
  "result": "Final response text to the owner",
  "generate_progress_count": 3
}
```

### error
Emitted when an error occurs during execution.
```json
{
  "event": "error",
  "ts": 1234567890123,
  "use_id": "1234567890123",
  "error": "Error message",
  "trace": "Full stack trace...",
  "generate_progress_count": 3
}
```

### info
Emitted when non-JSON output is captured from agent stdout.
```json
{
  "event": "info",
  "ts": 1234567890123,
  "use_id": "1234567890123",
  "message": "Non-JSON output line from agent"
}
```

## Tool Call Tracking

Tool events use `call_id` to pair `tool_start` and `tool_end` events. This allows tracking:
- Which tools are currently running
- Tool execution duration
- Tool inputs and outputs
- Concurrent tool executions

The frontend uses this to show real-time status updates as tools execute, changing from "running..." to "✓" when complete.

## Agent Output

When an agent completes successfully, its result can be automatically written to a file. This uses the same output path logic as generators.

- Include an `output` field in the agent's frontmatter with the format ("md" or "json")
- Output path is derived from agent name + format + schedule:
  - Daily agents: `YYYYMMDD/talents/{name}.{ext}`
  - Segment agents: `YYYYMMDD/{segment}/{name}.{ext}`
- Writing occurs before completion
- Write failures are logged but don't interrupt the agent flow
- Commonly used for scheduled agents that generate daily reports

## Talent Configuration

Talents use configurations stored in the `core/payload/solstone/talent/` directory. Each talent is a `.md` file containing:
- JSON frontmatter with metadata and configuration
- The talent-specific prompt and instructions in the content

When spawning a talent:
1. Cortex resolves the talent type, declared cwd, and timeout via `resolve_execution_facts`, then passes the raw request to the sibling `solstone-core __talent-worker` via stdin (NDJSON format).
2. The native talent worker discovers and composes the talent configuration, including request parameters, defaults, and provider/model context.
3. Cortex sets the child cwd to the journal only when the talent type is `cogitate` and its declared `cwd` is `journal`; otherwise it leaves the worker's cwd unchanged.
4. Instructions are built with three components:
   - `system_instruction`: `journal.md` (shared base prompt, cacheable)
   - `extra_context`: Runtime context (facets, generators list, datetime)
   - `user_instruction`: The agent's `.md` file content

Agents define specialized behaviors and facet expertise. Available agents can be discovered using `get_talent_configs(type="cogitate")` or by listing files in the `core/payload/solstone/talent/` directory.

### Agent Configuration Options

The JSON frontmatter for an agent can include:
- `max_tokens`: Maximum response token limit
- `schedule`: Scheduling configuration for automated execution
  - `"daily"`: Run automatically at the configured daily scheduler time
    (`00:15` for a newly initialized schedule configuration)
- `priority`: Execution order for scheduled prompts (integer, **required** for scheduled prompts)
  - Lower numbers run first (e.g., priority 10 runs before priority 40)
  - See [THINK.md](THINK.md#unified-priority-execution) for priority bands
- `multi_facet`: Boolean flag for facet-aware agents (default: false)
  - When true, the agent is spawned once for each **active** facet (see Multi-Facet Agents section)
  - Each instance receives a facet-specific prompt with the facet name
  - Useful for creating per-facet reports, newsletters, or analyses
- `always`: Override active facet detection for multi-facet agents (default: false)
  - When true, agent runs for all non-muted facets regardless of activity
- `env`: Environment variables to set for the agent subprocess (object)
  - Keys are variable names, values are coerced to strings
  - Request-level `env` overrides agent defaults
  - Note: `SOLSTONE_JOURNAL` is inherited by Cortex from the managed wrapper / test fixture / sandbox env, and child processes inherit it through `os.environ`

### Model Resolution

Generate and cogitate use the single explicit `providers.active` provider/model
selected in the Thinking app. If it is missing or invalid, the request fails
closed. Key presence, tiers, backup maps, and talent frontmatter never select a
different provider or model. Talent `disabled` and `extract` metadata lives in
the top-level `talent_overrides` map.

## Agent Providers

The system supports multiple provider identities through
`solstone/think/providers/__init__.py`:

- **OpenAI, Google AI Studio, and Anthropic** (`solstone/think/cogitate_client.py`): native one-shot cogitate transport; native generate owns single-shot generation
- **Local** (`solstone/think/providers/local.py`): bundled llama-server, BYO OpenAI-compatible endpoint, or confidential local endpoint

Effective providers:
- Emit JSON events to stdout (one per line)
- Are spawned as subprocesses by Cortex
- Use consistent event structures across providers
- Process events are written to stdout for Cortex to capture

## Scheduled Agents and Generators

Both agents and generators support scheduling via `journal think`. Agents have `"schedule": "daily"` and generators have `"schedule": "segment"` or `"schedule": "daily"`.

### Execution Order
Scheduled items run in priority order (lower numbers first):
1. Items are sorted by their `priority` field (required for all scheduled prompts)
2. Items with the same priority run in parallel, then think waits for completion
3. After each generator completes, incremental indexing runs for its output

**Priority bands (recommended):**
- **10-30**: Generators (content-producing prompts)
- **40-60**: Analysis agents
- **90+**: Late-stage agents
- **99**: Fun/optional prompts

### Multi-Facet Agents
When an agent has `"multi_facet": true`:
1. The agent is spawned once for each **active** facet
2. Each instance receives a prompt including the facet name
3. The agent should call `get_facet(facet_name)` to load facet context
4. This enables per-facet reports, newsletters, and analyses

#### Daily Multi-Facet Agents

**Active Facet Detection**: By default, daily multi-facet agents only run for facets that had activity the previous day. `solstone/think/facets.py:get_active_facets()` determines activity by scanning segment-level `facets.json` files from the previous day, not facet event files. This prevents unnecessary agent runs for inactive facets.

To force an agent to run for all facets regardless of activity, set `"always": true`:

```json
{
  "title": "Facet Newsletter Generator",
  "schedule": "daily",
  "priority": 10,
  "multi_facet": true
}
```

```json
{
  "title": "Facet Auditor",
  "schedule": "daily",
  "multi_facet": true,
  "always": true
}
```

#### Segment Multi-Facet Agents

Segment agents can also be multi-facet. Active facets are determined from the `facets.json` output written by the facets generator (priority 90) during segment processing.

```json
{
  "title": "Facet Activity Tracker",
  "schedule": "segment",
  "multi_facet": true
}
```

The facets generator outputs an array of detected facets for each segment:
```json
[
  {"facet": "work", "activity": "Code review", "level": "high"},
  {"facet": "personal", "activity": "Email check", "level": "low"}
]
```

Multi-facet segment agents spawn once per non-muted facet in this array. Muted facets are filtered out, consistent with daily agent behavior. If no enabled facets are detected (empty array, missing file, or all facets muted), the agent simply doesn't spawn for that segment.

**Note**: The `"always"` flag is not supported for segment agents since facet detection is inherent to the segment content.

## Process Management

The `journal supervisor` command provides process management for the Cortex ecosystem:
- Starts and monitors the Cortex file watcher service
- Handles process restarts on failure
- Monitors system health indicators
- Triggers `journal think` at midnight for daily processing (generators + agents)

This is distinct from agent lifecycle management, which Cortex handles internally through file state transitions.
