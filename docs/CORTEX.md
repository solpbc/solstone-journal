# Cortex API and Eventing

The Cortex system manages AI talent execution through the Callosum message bus with file-based persistence. It acts as a process manager for talent instances, receiving requests via Callosum and writing execution events to both JSONL files (for persistence) and the message bus (for real-time distribution).

For details on the Callosum protocol and message format, see [CALLOSUM.md](CALLOSUM.md).

## Architecture

### Event Flow
1. **Request Creation**: Client calls `cortex_request()` which broadcasts to Callosum (`tract="cortex"`, `event="request"`)
2. **Request Reception**: Cortex receives message via Callosum callback and creates `<name>/<timestamp>_active.jsonl`
3. **Talent Spawning**: Cortex spawns a talent process via `python -m solstone.think.talents` with merged configuration
4. **Event Emission**: Talents write JSON events to stdout (captured by Cortex)
5. **Event Distribution**: Cortex appends events to JSONL file AND broadcasts to Callosum
6. **Agent Completion**: Cortex renames file to `<name>/<timestamp>.jsonl` when agent finishes

### Key Components
- **Message Bus Integration**: Cortex connects to Callosum to receive requests and broadcast events
- **Process Management**: Spawns talent subprocesses (both tool talents and generators)
- **Configuration Delegation**: Passes raw requests to `python -m solstone.think.talents`, which handles all config loading, validation, and hydration
- **Event Capture**: Monitors agent stdout/stderr and appends to JSONL files
- **Dual Event Distribution**: Events go to both persistent files and real-time message bus
- **NDJSON Input Mode**: Agent processes accept newline-delimited JSON via stdin containing the full merged configuration

### File States
- `<name>/<timestamp>_active.jsonl`: Talent currently executing (Cortex is appending events)
- `<name>/<timestamp>.jsonl`: Talent completed (contains full event history)

**Note**: Files provide persistence and historical record, while Callosum provides real-time event distribution to all interested services.

## Request Format

Requests are created via `cortex_request()` from `think.cortex_client`, which broadcasts to Callosum. The request message follows this format:

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
  "chat_id": "1234567890122",        // Optional: chat ID for reverse lookup
  "facet": "my-project",          // Optional: project context
  "output": "md",                     // Optional: output format ("md" or "json"), writes to talents/
  "day": "20250109",                  // Optional: YYYYMMDD format, defaults to current day
  "env": {                           // Optional: environment variables for subprocess
    "API_KEY": "secret",
    "DEBUG": "true"
  }
}
```

The model is automatically resolved based on the talent context (`talent.{source}.{name}`)
and the configured tier in `journal.json`. Provider can optionally be overridden at
request time, which will resolve the appropriate model for that provider at the same tier.

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
finish event; cloud providers and the bundled local provider continue through
OpenHands.

Chats are locked to their original provider — continuations must use the same provider
that started the conversation. The `chat_id` field enables reverse lookup from an
talent back to its parent chat.

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

### start
Emitted when a talent run begins.
```json
{
  "event": "start",
  "ts": 1234567890123,
  "use_id": "1234567890123",
  "name": "default",
  "model": "gpt-4o",
  "session_id": "sess-abc",
  "chat_id": "1234567890122"
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
  "result": "Final response text to the owner"
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
  "trace": "Full stack trace..."
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

Talents use configurations stored in the `solstone/talent/` directory. Each talent is a `.md` file containing:
- JSON frontmatter with metadata and configuration
- The talent-specific prompt and instructions in the content

When spawning a talent:
1. Cortex passes the raw request to `python -m solstone.think.talents` via stdin (NDJSON format)
2. The talent process (`solstone/think/talents.py`) handles all config loading via `prepare_config()`:
   - Loads talent configuration using `get_talent()` from `solstone/think/talent.py`
   - Merges request parameters with talent defaults
   - Resolves provider and model based on context
3. The agent validates the config via `validate_config()` before execution
4. Instructions are built with three components:
   - `system_instruction`: `journal.md` (shared base prompt, cacheable)
   - `extra_context`: Runtime context (facets, generators list, datetime)
   - `user_instruction`: The agent's `.md` file content

Agents define specialized behaviors and facet expertise. Available agents can be discovered using `get_talent_configs(type="cogitate")` or by listing files in the `solstone/talent/` directory.

### Agent Configuration Options

The JSON frontmatter for an agent can include:
- `max_tokens`: Maximum response token limit
- `schedule`: Scheduling configuration for automated execution
  - `"daily"`: Run automatically at midnight each day
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

Models are resolved automatically based on context and tier:
1. Each talent config has a context pattern: `talent.{source}.{name}` (e.g., `talent.system.default`)
2. The context determines the tier (pro/flash/lite) from `journal.json` or system defaults
3. The tier + provider determines the actual model to use

This allows controlling model selection via tier configuration rather than hardcoding models:
```json
{
  "providers": {
    "contexts": {
      "talent.system.default": {"tier": 1},
      "talent.*": {"tier": 2}
    }
  }
}
```

## Agent Providers

The system supports multiple AI providers, each implementing the same event interface:

- **OpenAI** (`solstone/think/providers/openai.py`): GPT models with OpenAI Agents SDK
- **Google** (`solstone/think/providers/google.py`): Gemini models with Google AI SDK
- **Anthropic** (`solstone/think/providers/anthropic.py`): Claude models with Anthropic SDK

All providers:
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
