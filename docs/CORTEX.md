# Cortex API and Eventing

The Cortex system manages AI talent execution through the Callosum message bus with file-based persistence. It acts as a process manager for talent instances, receiving requests via Callosum and writing execution events to both JSONL files (for persistence) and the message bus (for real-time distribution).

For details on the Callosum protocol and message format, see [CALLOSUM.md](CALLOSUM.md).

## Architecture

### Event Flow
1. **Request Creation**: A client (Convey, `solstone journal talent`, or `solstone-core-cortex-client`) broadcasts to Callosum (`tract="cortex"`, `event="request"`)
2. **Request Reception**: Cortex receives message via Callosum callback and creates `<name>/<timestamp>_active.jsonl`
3. **Talent Spawning**: Cortex resolves the sibling `solstone-core` binary and spawns `solstone-core __talent-worker` with the raw request
4. **Event Emission**: Talents write JSON events to stdout (captured by Cortex)
5. **Event Distribution**: Cortex appends events to JSONL file AND broadcasts to Callosum
6. **Agent Completion**: Cortex renames file to `<name>/<timestamp>.jsonl` when agent finishes

### Key Components
- **Message Bus Integration**: Cortex connects to Callosum to receive requests and broadcast events
- **Process Management**: Spawns Generate talent workers
- **Execution-Fact Resolution**: Resolves the worker timeout before spawning; Cortex resolves no interpreter
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
  "prompt": "Analyze this code for security issues",  // Optional: additional task input
  "name": "work",                 // Required: talent name from talent/*.md
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
(`providers.active`). Requests cannot override the provider or model; supplied
overrides are rejected. There is no tier-based fallback or backup-provider
routing. See [PROVIDERS.md](PROVIDERS.md). A talent's budgets are its own as
well: a request carrying `max_output_tokens`, `thinking_budget`,
`context_window` or `temperature` is refused.

## Generator Request Format

Generate talents produce analysis output (Markdown or JSON) from the context their preparation supplies. An `output` field selects publication format; it does not select a separate execution engine.

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
  "refresh": false                  // Optional: regenerate even if output exists
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

## Talent Event Format

All subsequent lines are JSON objects with `event` and millisecond `ts` fields. The `ts` field is automatically added by Cortex if not provided by the provider. Additionally, Cortex automatically adds an `use_id` field (matching the timestamp component in the filename) to all events for tracking purposes.

### request
The initial spawn request (first line of file, written by client).
```json
{
  "event": "request",
  "ts": 1234567890123,
  "use_id": "1234567890123",
  "prompt": "User's task or question",
  "name": "work",
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
  "name": "work",
  "model": "example-model",
  "provider": "openai"
}
```

### tool_start
Historical Cogitate event marking the start of a tool execution.
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
Historical Cogitate event marking completion of a tool execution.
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
Historical event carrying model reasoning content.
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
Historical event marking a handoff between agents.
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

## Historical Tool Events

Stored Cogitate runs can contain `tool_start` and `tool_end` records paired by
`call_id`. Ordinary run readers preserve their inputs, outputs, timing and
terminal outcomes. New Generate talent runs do not execute a tool loop.

## Talent Output

When a talent completes successfully, Rust publishes its result through the talent's output contract.

- Include an `output` field in the talent's frontmatter with the format ("md" or "json")
- Output path is derived from talent name + format + schedule:
  - Daily talents: `chronicle/YYYYMMDD/talents/{name}.{ext}`
  - Segment talents: `chronicle/YYYYMMDD/{segment}/{name}.{ext}`
- Writing occurs before completion
- A publication failure produces a terminal failure even if the completion succeeded
- Commonly used for scheduled talents that generate daily reports

## Talent Configuration

Talents use configurations stored in the `core/payload/solstone/talent/` directory. Each talent is a `.md` file containing:
- JSON frontmatter with metadata and configuration
- The talent-specific prompt and instructions in the content

When spawning a talent:
1. Cortex resolves the worker timeout, then passes the raw request to the sibling `solstone-core __talent-worker` via stdin (NDJSON format).
2. The native talent worker discovers and composes the talent configuration, including request parameters, defaults, and provider/model context.
3. Rust prepares the talent's instruction and source context, including its applicable pre-hook. The Generate request carries that prepared content and any declared structured-output schema.
4. Rust validates and publishes the completion through the talent's output and domain-write rules, then emits the final outcome.

Talents define specialized behaviors and facet expertise. Available talents can be discovered with `solstone journal talent list` (its `--json` output carries each talent's frontmatter, including `type`) or by listing files in the `core/payload/solstone/talent/` directory.

### Talent Configuration Options

The JSON frontmatter for a talent can include:
- `max_output_tokens`: **Required.** The talent's own reply ceiling, measured on
  the bundled model: 1.5 times its largest real output, rounded up to a multiple
  of 256. A talent without one fails validation, and nothing raises it (see
  [PROVIDERS.md § Output and Context Budgets](PROVIDERS.md#output-and-context-budgets))
- `schedule`: Scheduling configuration for automated execution
  - `"daily"`: Run automatically at the configured daily scheduler time
    (`00:15` for a newly initialized schedule configuration)
- `priority`: Execution order for scheduled prompts (integer, **required** for scheduled prompts)
  - Lower numbers run first (e.g., priority 10 runs before priority 40)
  - See [Execution Order](#execution-order) for priority bands
- `multi_facet`: Boolean flag for facet-aware agents (default: false)
  - When true, the agent is spawned once for each **active** facet (see Multi-Facet Agents section)
  - Each instance receives a facet-specific prompt with the facet name
  - Useful for creating per-facet reports, newsletters, or analyses
- `always`: Override active facet detection for multi-facet agents (default: false)
  - When true, agent runs for all non-muted facets regardless of activity
- `env`: not applied. A talent's own `env` never reaches a process environment; only a request's `env` does (see [Request Format](#request-format))
  - Cortex adds each request `env` entry to the worker's environment, with non-string values written as their JSON text
  - Cortex also sets `SOL_FACET` and `SOL_DAY` from the request's `facet` and `day`; a request `env` entry of the same name wins
  - Note: `SOLSTONE_JOURNAL` is inherited by Cortex from the managed wrapper / test fixture / sandbox env, and the worker Cortex spawns inherits Cortex's environment, with the request's `env` entries added on top

### Model Resolution

Generate uses the single explicit `providers.active` provider/model
selected in the Thinking app. If it is missing or invalid, the request fails
closed. Key presence, tiers, backup maps, and talent frontmatter never select a
different provider or model. Talent `disabled` metadata lives in
the top-level `talent_overrides` map, keyed `talent.system.<name>` for a journal
talent and `talent.<app>.<name>` for an app talent. A talent's reply ceiling is
not owner metadata; how it meets the served context window is in
[PROVIDERS.md § Output and Context Budgets](PROVIDERS.md#output-and-context-budgets).

## Talent Providers

The system supports multiple provider identities. `resolve_lane` in
`core/crates/solstone-core-generate-wire/src/lane.rs` maps the configured
provider to a dispatch lane:

- **OpenAI, Google AI Studio, and Anthropic** (`openai.rs`, `google.rs` and `anthropic.rs` in `core/crates/solstone-core-generate-wire/src/`): native Generate transport
- **Local** (`core/crates/solstone-core-local/`, with the `bundled.rs`, `endpoint.rs` and `confidential.rs` lanes in `core/crates/solstone-core-generate-wire/src/`): bundled llama-server, BYO OpenAI-compatible endpoint, or the attested confidential-processing endpoint sol pbc operates

Effective providers:
- Run inside a `solstone-core generate --one-shot` child that the talent worker spawns; Cortex spawns only the worker
- Write their output to that child's stdout, which the talent worker reads; Cortex never reads a provider's output directly
- Use consistent event structures across providers

The talent worker writes the run's events to its own stdout, one JSON object per line, and Cortex captures them there.

## Scheduled Agents and Generators

Both agents and generators support scheduling via `solstone journal think`. Agents have `"schedule": "daily"` and generators have `"schedule": "segment"` or `"schedule": "daily"`.

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
3. The prompt arrives scoped to that facet: the runtime fills `$facets` with that facet's context, and a talent's pre hook can add facet-specific material
4. This enables per-facet reports, newsletters, and analyses

#### Daily Multi-Facet Agents

**Active Facet Detection**: By default, daily multi-facet agents only run for facets that had activity the previous day. `active_facets_checked` in `core/crates/solstone-core-system/src/activity_state.rs` determines activity by scanning segment-level `facets.json` files from the previous day, not facet event files. This prevents unnecessary agent runs for inactive facets.

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

The `solstone journal supervisor` command provides process management for the Cortex ecosystem:
- Starts and monitors the Cortex file watcher service
- Handles process restarts on failure
- Monitors system health indicators
- Runs configured daily entries, such as `solstone journal think`, at their scheduler
  time (`00:15` for a newly initialized schedule configuration)

This is distinct from agent lifecycle management, which Cortex handles internally through file state transitions.
