# solstone-think

Post-processing utilities for clustering and summarising captured data. The tools leverage the Gemini API to analyse transcriptions and screenshots. All commands work with a **journal** directory that holds daily folders in `YYYYMMDD` format.

## Installation

```bash
make install
```

All dependencies are listed in `pyproject.toml`.

## Usage

The package exposes several commands:

- `sol call transcripts read` groups audio and screen transcripts into report sections. Use `--start` and
  `--length` to limit the report to a specific time range. See `sol call transcripts --help` for additional commands.
- `journal think` runs generators and agents for a single day via Cortex.
- `python -m solstone.think.talents` is the unified execution module for tool talents and generators spawned by Cortex (NDJSON protocol).
- `journal supervisor` monitors journaling health and starts the local services that feed Convey, Cortex, and related background tasks. Use the `--no-*` flags to opt out of specific services when debugging.
- `journal cortex` starts a Callosum-based service for managing AI agent instances and generators.
- `journal talent` lists available agents and generators with their configuration. Use `journal talent show <name>` to see details, and `journal talent show <name> --prompt` to see the fully composed prompt that would be sent to the LLM.

```bash
sol call transcripts read YYYYMMDD [--start HHMMSS --length MINUTES]
journal think [--day YYYYMMDD] [--segment HHMMSS_LEN] [--stream NAME] [--refresh] [--flush]
journal supervisor [--no-daily] [--no-cortex] [--no-link] [--no-convey] [--no-schedule]
journal cortex [--host HOST] [--port PORT] [--path PATH]
journal talent list [--schedule daily|segment] [--json]
journal talent show <name> [--prompt] [--day YYYYMMDD] [--segment HHMMSS_LEN] [--full]
```

Use `--refresh` to overwrite existing files, and `-v` for verbose logs.

Set `GOOGLE_API_KEY` before running any command that contacts Gemini.
`GOOGLE_API_KEY` can also be provided in a `.env` file which
is loaded automatically by most commands.

Structured file importers are registered in `solstone/think/importers/file_importer.py` and
run through `sol import`'s dispatcher. Their `process()` contract now accepts
`dry_run: bool = False`, and journal-archive imports use the same dispatcher
surface while serializing journal mutation with the merge lock contract.

## Service Discovery

Agents invoke tools through `sol call` shell commands:
`sol call <module> <command> [args...]`.
Tool access is command-based via the `sol call` CLI framework.

## Automating daily processing

The `journal think` command can be triggered by a systemd timer. Below is a
minimal service and timer that process yesterday's folder every morning at
06:00:

```ini
[Unit]
Description=Process solstone journal

[Service]
Type=oneshot
ExecStart=/usr/local/bin/journal think

[Install]
WantedBy=multi-user.target
```

```ini
[Unit]
Description=Run journal think daily

[Timer]
OnCalendar=*-*-* 06:00:00
Persistent=true
Unit=sol-think.service

[Install]
WantedBy=timers.target
```

## Agent System

### Unified Priority Execution

All scheduled prompts (both generators and tool-using agents) share a unified priority system. The `journal think` command executes prompts ordered by priority, from lowest (runs first) to highest (runs last).

**Priority is required for all scheduled prompts.** Prompts without a `priority` field will fail validation. Suggested priority bands:

| Band | Range | Use Case |
|------|-------|----------|
| Generators | 10-30 | Content-producing prompts that create `.md` files |
| Analysis Agents | 40-60 | Agents that analyze generated content |
| Late-stage | 90+ | Agents that run after most others complete |
| Fun/Optional | 99 | Low-priority or experimental prompts |

After each generator completes and creates output, the indexer runs `--rescan-file` for incremental indexing. A full `--rescan` runs in the post phase.

### Cortex: Central Talent Manager

The Cortex service (`journal cortex`) is the central system for managing AI talent instances and generators. It monitors the journal's `talents/` directory for new requests and manages execution. All talent spawning should go through Cortex for proper event tracking and management.

Cortex routes requests based on configuration:
- Requests with `tools` field → tool-using talents (`python -m solstone.think.talents`)
- Requests with `output` field (no `tools`) → generators (`python -m solstone.think.talents`)

Both types are handled by the unified `python -m solstone.think.talents` execution module.

To spawn talents programmatically, use the cortex_client functions:

```python
from solstone.think.cortex_client import cortex_request
from solstone.think.callosum import CallosumConnection

# Create a request
use_id = cortex_request(
    prompt="Your task here",
    name="default",
    provider="openai"  # or "google", "anthropic", "claude"
)

# Watch for talent events via Callosum
def on_event(message):
    # Filter for cortex tract events
    if message.get('tract') != 'cortex':
        return

    print(f"Event: {message['event']}")
    if message.get('event') == 'finish':
        print(f"Result: {message.get('result')}")

watcher = CallosumConnection()
watcher.start(callback=on_event)
# ... later, when done:
watcher.stop()
```

### Spawning Generators via Cortex

Generators can also be spawned via `cortex_request` by including an `output` field:

```python
from solstone.think.cortex_client import cortex_request, wait_for_uses

# Spawn a generator
use_id = cortex_request(
    prompt="",  # Generators don't use prompts
    name="activity",
    config={
        "day": "20250109",
        "output": "md",
        "refresh": True,  # Regenerate even if output exists
    }
)

# Wait for completion
completed, timed_out = wait_for_uses([use_id], timeout=300)
```

### Direct CLI Usage (Testing Only)

The `sol providers check` command is an ad-hoc provider check CLI. Cortex does not use it as the talent spawn path. For testing purposes, it can be invoked directly:

```bash
sol providers check [TASK_FILE] [--provider PROVIDER] [--model MODEL] [--max-tokens N] [-o OUT_FILE]
```

The provider can be ``openai`` (default), ``google``, ``anthropic``, ``local``, or ``mlx``. Configure cloud API keys in the ``env`` section of ``journal/config/journal.json`` (for example, ``OPENAI_API_KEY``, ``GOOGLE_API_KEY``, or ``ANTHROPIC_API_KEY``). The ``local`` provider requires no API key — Settings installs a bundled loopback llama-server runtime and GGUF model on demand. Keys are loaded into ``os.environ`` by ``setup_cli()`` at process startup.

### Provider modules

Each provider lives in `solstone/think/providers/` and exposes a common interface:

- `run_generate()` - Sync text generation, returns `GenerateResult`
- `run_agenerate()` - Async text generation, returns `GenerateResult`
- `run_cogitate()` - Tool-calling execution via `sol call` commands and event streaming

For direct LLM calls, use `think.models.generate()` or `think.models.agenerate()`
which automatically routes to the configured provider based on context.

## Generator map keys

`think.talent.get_talent_configs(has_tools=False)` reads the `.md` prompt files under `solstone/talent/` and
returns a dictionary keyed by generator name. Each entry contains:

- `path` – the prompt file path
- `color` – UI color hex string
- `mtime` – modification time of the `.md` file
- Additional keys from JSON frontmatter such as `title`, `description`, `hook`, or `load`

The `hook` field enables output processing by invoking named hooks like `"schedule"`.
The `load` key controls transcript/percept/agent source filtering for generators.
See [APPS.md](APPS.md#prompt-context-configuration) for the full schema.

## Cortex API

Cortex is the central agent management system that all agent spawning should go through. See [CORTEX.md](CORTEX.md) for complete documentation of the Cortex API and agent event structures.

### Using cortex_client

The `think.cortex_client` module provides functions for interacting with Cortex:

```python
from solstone.think.cortex_client import cortex_request, cortex_uses

# Create an agent request
request_file = cortex_request(
    prompt="Your prompt",
    name="default",
    provider="openai"
)

# List running and completed agents
agents_info = cortex_uses(limit=10, use_type="live")
print(f"Found {agents_info['live_count']} running agents")
```
# Talent Module

AI agent system and tool-calling support for solstone.

## Commands

| Command | Purpose |
|---------|---------|
| `journal cortex` | Agent orchestration service |
| `sol providers check` | Ad-hoc provider check (testing only) |

## Architecture

```
Cortex (orchestrator)
   ├── Callosum connection (events)
   ├── Tool execution via `sol call`
   └── Agent subprocess management
          ↓
   Providers (openai, google, anthropic, local, mlx)
```

## Providers

| Provider | Module | Features |
|----------|--------|----------|
| OpenAI | `solstone/think/providers/openhands.py` | GPT models via OpenHands/LiteLLM |
| Google | `solstone/think/providers/openhands.py` | Gemini models via OpenHands/LiteLLM |
| Anthropic | `solstone/think/providers/openhands.py` | Claude via OpenHands/LiteLLM |
| Local | `solstone/think/providers/local.py` | On-device llama-server models |
| MLX | `solstone/think/providers/mlx.py` | On-device Apple Silicon vision/generate |

Providers implement `run_generate()`, `run_agenerate()`, and `run_cogitate()` functions. See [PROVIDERS.md](PROVIDERS.md) for implementation details.

## Key Components

- **cortex.py** - Central agent manager, file watcher, event distribution, spawns agents.py
- **cortex_client.py** - Client functions: `cortex_request()`, `cortex_uses()`, `wait_for_uses()`
- **agents.py** - Unified CLI entry point for both tool-using agents and generators (NDJSON protocol)
- **models.py** - Unified `generate()`/`agenerate()` API, provider routing, token logging
- **batch.py** - `Batch` class for concurrent LLM requests with dynamic queuing

## Agent Personas

System prompts in `solstone/talent/*.md` (markdown with JSON frontmatter). Apps can add custom agents in `solstone/apps/{app}/talent/`.

JSON metadata supports `title`, `provider`, `model`, `tools`, `schedule`, `priority`, `multi_facet`, and `load` keys. Cogitate prompts may also set `cwd: "journal"` or `cwd: "repo"`; when omitted they default to `journal`, while repo-root agents such as `coder` should set `repo`. Generators reject `cwd`.

**Important:** The `priority` field is **required** for all prompts with a `schedule`. Prompts without explicit priority will fail validation. See the [Unified Priority Execution](#unified-priority-execution) section for priority bands.

See [APPS.md](APPS.md#prompt-context-configuration) for the `load` schema and inline template variables that control source filtering and prompt context.

## Documentation

- [PROVIDERS.md](PROVIDERS.md) - Provider implementation guide
- [CORTEX.md](CORTEX.md) - Full API, event schemas, request format
- [CALLOSUM.md](CALLOSUM.md) - Message bus protocol
- [THINK.md](THINK.md) - Cortex usage examples
