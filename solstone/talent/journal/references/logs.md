# Logs

## Action Logs

Action logs record an audit trail of owner-initiated actions and agent tool calls. There are two types:

- **Journal-level logs** (`config/actions/`) – actions not tied to a specific facet (settings changes, observer management)
- **Facet-scoped logs** (`facets/{facet}/logs/`) – actions within a specific facet (activities, entities)

### Journal Action Logs

The `config/actions/` directory records journal-level actions. Logs are organized by day as `config/actions/YYYYMMDD.jsonl`.

```json
{
  "timestamp": "2025-12-16T07:33:05.135587+00:00",
  "source": "app",
  "actor": "settings",
  "action": "identity_update",
  "params": {
    "changed_fields": {"name": {"old": "John", "new": "John Doe"}}
  }
}
```

### Facet Action Logs

The `logs/` directory within each facet records facet-scoped actions. Logs are organized by day as `facets/{facet}/logs/YYYYMMDD.jsonl`.

```json
{
  "timestamp": "2025-12-16T07:33:05.135587+00:00",
  "source": "tool",
  "actor": "entities:entities_review",
  "action": "entity_attach",
  "params": {
    "type": "Person",
    "name": "Alice Johnson",
    "description": "Lead engineer on the API project"
  },
  "facet": "work",
  "use_id": "1765870373972"
}
```

### Log Entry Fields

Both log types share the same structure:

- `timestamp` – ISO 8601 timestamp of the action
- `source` – Origin type: "app" for web UI, "tool" for agent tools
- `actor` – App or tool name that performed the action
- `action` – Action name (e.g., "entity_attach", "identity_update")
- `params` – Action-specific parameters
- `facet` – Facet name (only present in facet-scoped logs)
- `use_id` – Agent ID (only present for agent tool actions)

These logs enable auditing, debugging, and potential rollback of automated actions.

## Token Usage

The `tokens/` directory tracks token usage from all AI model calls across the system. Usage data is organized by day as `tokens/YYYYMMDD.jsonl` where each file contains JSON Lines entries for that day's API calls.

### Token log format

Each line in a token log file is a JSON object with the following structure:

```json
{
  "timestamp": 1736812345000,
  "model": "gemini-2.5-flash",
  "context": "agent.default.20250113_143022",
  "segment": "143022_300",
  "usage": {
    "input_tokens": 1500,
    "output_tokens": 500,
    "total_tokens": 2000,
    "cached_tokens": 800,
    "reasoning_tokens": 200
  }
}
```

Required fields:
- `timestamp` – Unix timestamp in milliseconds (13 digits)
- `model` – Model identifier (e.g., "gemini-2.5-flash", "gpt-5", "claude-sonnet-4-5")
- `context` – Calling context (e.g., "agent.name.use_id" or "module.function:line")
- `usage` – Token counts dictionary with normalized field names

Optional fields:
- `segment` – Recording segment key (e.g., "143022_300") when token usage is attributable to a specific observation window

Usage fields (all optional depending on model capabilities):
- `input_tokens` – Tokens in the prompt/input
- `output_tokens` – Tokens in the response/output
- `total_tokens` – Total tokens consumed
- `cached_tokens` – Tokens served from cache (reduces cost)
- `reasoning_tokens` – Tokens used for extended thinking/reasoning
- `requests` – Number of API requests made (for batch operations)

The logging system normalizes provider-specific formats (OpenAI, Gemini, Anthropic) into this unified schema for consistent cost tracking and analysis across all models.

## Agent Event Logs

The `talents/` directory stores event logs for all AI talent sessions managed by Cortex. Each talent session produces a JSONL file containing the complete event history.

**Directory layout:**
- `<name>/` – per-agent subdirectory (e.g., `default/`, `entities--observer/`)
- `<name>/<use_id>_active.jsonl` – currently running agent (renamed when complete)
- `<name>/<use_id>.jsonl` – completed agent session
- `<name>.log` – symlink to the latest completed run for each agent name
- `<day>.jsonl` – day index with one summary line per agent that completed on that day

The `use_id` is a Unix timestamp in milliseconds that uniquely identifies the session.

**Event format (JSONL):**

Each line is a JSON object with an `event` field indicating the event type:

```jsonl
{"event": "start", "ts": 1755450767962, "name": "helper", "prompt": "Help me with...", "facet": "work"}
{"event": "text", "ts": 1755450768000, "content": "I'll help you with that."}
{"event": "tool_call", "ts": 1755450769000, "tool": "search", "params": {"query": "example"}}
{"event": "tool_result", "ts": 1755450770000, "tool": "search", "result": "..."}
{"event": "finish", "ts": 1755450771000, "result": "Here's what I found..."}
```

**Common event types:**
- `start` – agent session started, includes name, prompt, and facet
- `text` – streaming text output from the agent
- `tool_call` – agent invoked a tool
- `tool_result` – result returned from tool execution
- `error` – error occurred during execution
- `finish` – agent session completed, includes final result

See [CORTEX.md](../../../docs/CORTEX.md) for agent architecture and spawning details.

## Service Health

The `health/` directory contains log files for long-running services.

**Files:**
- `health/<service>.log` – log output for each service (e.g., `observe.log`, `cortex.log`, `convey.log`)
- `health/retention.log` – retired legacy JSONL retention-purge summaries; existing journals may contain historical entries, but no live writer appends new ones
- `health/pruning-runs/YYYYMMDD.jsonl` – per-segment raw-media offload audit records, one JSONL line per real archive-confirmed offload deletion with `kind: "raw_media_offload"`, day, stream, segment, files, bytes freed, and `processed_at`; offload dry-runs do not write pruning-run records

These logs are useful for debugging service issues. See [DOCTOR.md](../../../docs/DOCTOR.md) for diagnostics and troubleshooting guidance.
