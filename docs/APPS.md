# solstone App Development Guide

**Complete guide for building apps in the `solstone/apps/` directory.**

Apps are the primary way to extend solstone's web interface (Convey). Each app is a self-contained module discovered automatically using **convention over configuration**—no base classes or manual registration required.

> **How to use this document:** This guide serves as a catalog of patterns and references. Each section points to authoritative source files—read those files alongside this guide for complete details. When in doubt, the source code is the definitive reference.

---

## Quick Start

Create a minimal app in two steps:

```bash
# 1. Create app directory (use underscores, not hyphens!)
mkdir solstone/apps/my_app

# 2. Create workspace template
touch solstone/apps/my_app/workspace.html
```

**Minimal `workspace.html`:**
```html
<h1>Hello from My App!</h1>
```

**That's it!** Restart Convey and your app is automatically available at `/app/my_app`.

All apps are served via a shared route handler at `/app/{app_name}`. You only need `routes.py` if your app requires custom routes beyond the index page (e.g., API endpoints, form handlers, or navigation routes).

---

## Directory Structure

```
apps/my_app/
├── workspace.html     # Required: Main content template
├── routes.py          # Optional: Flask blueprint (only if custom routes needed)
├── tools.py           # Optional: App tool functions for agent workflows
├── call.py            # Optional: CLI commands via Typer (auto-discovered)
├── events.py          # Optional: Server-side event handlers (auto-discovered)
├── app.json           # Optional: Metadata (icon, label, facet support)
├── app_bar.html       # Optional: Bottom bar controls (forms, buttons)
├── background.html    # Optional: Background JavaScript service
├── talent/              # Optional: Custom agents, generators, and skills (auto-discovered)
│   └── my-skill/      #   Optional: Agent Skill directories (SKILL.md + resources)
├── maint/             # Optional: One-time maintenance tasks (auto-discovered)
└── tests/             # Optional: App-specific tests (run via make test-apps)
```

### File Purposes

| File | Required | Purpose |
|------|----------|---------|
| `workspace.html` | **Yes** | Main app content (rendered in container) |
| `routes.py` | No | Flask blueprint for custom routes (API endpoints, forms, etc.) |
| `tools.py` | No | Callable tool functions for AI agent workflows |
| `call.py` | No | CLI commands via Typer, accessed as `sol call <app>` (auto-discovered) |
| `events.py` | No | Server-side Callosum event handlers (auto-discovered) |
| `app.json` | No | Icon, label, facet support overrides |
| `app_bar.html` | No | Bottom fixed bar for app controls |
| `background.html` | No | Background service (WebSocket listeners) |
| `solstone/talent/` | No | Custom agents, generators, and skills (`.md` files + skill subdirectories) |
| `maint/` | No | One-time maintenance tasks (run on Convey startup) |
| `tests/` | No | App-specific tests with self-contained fixtures |

---

## Naming Conventions

**Critical for auto-discovery:**

1. **App directory**: Use `snake_case` (e.g., `my_app`, **not** `my-app`)
2. **Blueprint variable** (if using routes.py): Must be `{app_name}_bp` (e.g., `my_app_bp`)
3. **Blueprint name** (if using routes.py): Must be `app:{app_name}` (e.g., `"app:my_app"`)
4. **URL prefix**: Convention is `/app/{app_name}` (e.g., `/app/my_app`)

**Index route**: All apps are automatically served at `/app/{app_name}` via a shared handler. You don't need to define an index route in `routes.py`.

See `solstone/apps/__init__.py` for discovery logic and route injection.

---

## Required Files

### 1. `workspace.html` - Main Content

The workspace template is included inside the app container (`app.html`).

**Available Template Context:**
- `app` - Current app name (auto-injected from URL)
- `day` - Current day as YYYYMMDD string (auto-injected from URL for apps with `date_nav: true`)
- `facets` - List of active facet dicts: `[{name, title, color, emoji}, ...]`
- `selected_facet` - Currently selected facet name (string or None)
- `app_registry` - Registry with all apps (usually not needed directly)
- `state.journal_root` - Path to journal directory
- Any variables passed from route handler via `render_template(...)`

**Note:** The server-side `selected_facet` is also available client-side as `window.selectedFacet` (see JavaScript APIs below).

**Vendor Libraries:**
- Use `&#123;&#123; vendor_lib('marked') &#125;&#125;` for markdown rendering
- See [VENDOR.md](VENDOR.md) for available libraries

**Reference implementations:**
- Minimal: `solstone/apps/home/workspace.html` (simple content)
- Styled: `solstone/apps/support/workspace.html` (custom CSS, forms, interactive JS)
- Data-driven: `solstone/apps/todos/workspace.html` (facet sections, dynamic rendering)

---

## Optional Files

### 2. `routes.py` - Flask Blueprint

Define custom routes for your app (API endpoints, form handlers, navigation routes).

**Key Points:**
- **Not needed for simple apps** - the shared handler at `/app/{app_name}` serves your workspace automatically
- Only create `routes.py` if you need custom routes beyond the index page
- Blueprint variable must be named `{app_name}_bp`
- Blueprint name must be `"app:{app_name}"`
- URL prefix convention: `/app/{app_name}`
- Access journal root via `state.journal_root` (always available)
- Import utilities from `convey.utils` (see [Flask Utilities](#flask-utilities))

**Reference implementations:**
- API endpoints: `solstone/apps/search/routes.py` (search APIs, no index route)
- Form handlers: `solstone/apps/todos/routes.py` (POST handlers, validation, flash messages)
- Navigation: `solstone/apps/activities/routes.py` (date-based routes with custom context)
- Redirects: `solstone/apps/todos/routes.py` index route (redirects `/` to today's date)



### 3. `app.json` - Metadata

Override default icon, label, and other app settings.

**Authoritative source:** See the `App` dataclass in `solstone/apps/__init__.py` for all supported fields, types, and defaults.

**Common fields:**
- `icon` - Emoji icon for menu bar (default: "📦")
- `label` - Display label in menu (default: title-cased app name)
- `facets` - Enable facet integration (default: true)
- `date_nav` - Show date navigation bar (default: false)
- `allow_future_dates` - Allow clicking future dates in month picker (default: false)

**When to disable facets:** Set `"facets": false` for apps that don't use facet-based organization (e.g., system settings, dev tools).

**Examples:** Browse `solstone/apps/*/app.json` for reference configurations.

### 4. `app_bar.html` - Bottom Bar Controls

Fixed bottom bar for forms, buttons, date pickers, search boxes.

**Key Points:**
- App bar is fixed to bottom when present
- Page body gets `has-app-bar` class (adjusts content margin)
- Only rendered when app provides this template
- Great for persistent input controls across views

**Date Navigation:**

Enable via `"date_nav": true` in `app.json` (not via includes). This renders a `← Date →` control with month picker. Requires `/app/{app_name}/api/stats/{month}` endpoint returning `{YYYYMMDD: count}` or `{YYYYMMDD: {facet: count}}`.

Keyboard shortcuts: `←`/`→` for day navigation, `t` for today.

### 5. `background.html` - Background Service

JavaScript service that runs globally, even when app is not active.

**AppServices API:**

**Core Methods:**
- `AppServices.register(appName, service)` - Register background service
- `AppServices.escapeHtml(value)` - DOM-based HTML escaping helper; null/undefined become `''`
- `AppServices.renderMarkdown(raw)` - `marked` + `DOMPurify` markdown rendering with `{ breaks: true, gfm: true }`; requires both libraries loaded

**Badge Methods:**

App icon badges (menu bar):
- `AppServices.badges.app.set(appName, count)` - Set app icon badge count
- `AppServices.badges.app.clear(appName)` - Remove app icon badge
- `AppServices.badges.app.get(appName)` - Get current badge count

Facet pill badges (facet bar):
- `AppServices.badges.facet.set(facetName, count)` - Set facet badge count
- `AppServices.badges.facet.clear(facetName)` - Remove facet badge
- `AppServices.badges.facet.get(facetName)` - Get current badge count

Both badge types appear as red notification counts.

**Notification Methods:**
- `AppServices.notifications.show(options)` - Show persistent notification card
- `AppServices.notifications.dismiss(id)` - Dismiss specific notification
- `AppServices.notifications.dismissApp(appName)` - Dismiss all for app
- `AppServices.notifications.dismissAll()` - Dismiss all notifications
- `AppServices.notifications.count()` - Get active notification count
- `AppServices.notifications.update(id, options)` - Update existing notification

**Notification Options:**
```javascript
{
  app: 'my_app',          // App name (required)
  icon: '📬',             // Emoji icon (optional)
  title: 'New Message',   // Title (required)
  message: 'You have...', // Message body (optional)
  action: '/app/todos',   // Click action URL (optional)
  facet: 'work',          // Auto-select facet on click (optional)
  badge: 5,               // Badge count (optional)
  dismissible: true,      // Show X button (default: true)
  autoDismiss: 10000      // Auto-dismiss ms (optional)
}
```

**WebSocket Events (`window.appEvents`):**
- `listen(tract, callback)` - Listen to specific tract ('cortex', 'indexer', 'observe', etc.)
- `listen('*', callback)` - Listen to all events
- Messages have structure: `{tract: 'cortex', event: 'agent_complete', ...data}`
- See [CALLOSUM.md](CALLOSUM.md) for event protocol details

**Reference implementations:**
- `solstone/apps/todos/background.html` - App icon badge with API fetch

**Implementation source:** `solstone/convey/static/app.js` - AppServices framework, `solstone/convey/static/websocket.js` - WebSocket API

---

### 6. `tools.py` - App Tool Functions

Define plain callable tool functions for your app in `tools.py`.

**Key Points:**
- Only create `tools.py` if your app needs reusable tool functions for agent workflows
- Keep functions simple: typed inputs, dict-style outputs, clear docstrings
- Put shared logic in your app/module layer and call it from these functions

**Reference implementations:**
- `solstone/apps/todos/tools.py`
- `solstone/apps/entities/tools.py`

---

### 7. `call.py` - CLI Commands

Define CLI commands for your app that are automatically discovered and available via `sol call <app> <command>`.

**Key Points:**
- Only create `call.py` if your app needs human-friendly CLI access to its operations
- Export an `app = typer.Typer()` instance with commands defined via `@app.command()`
- Automatically discovered and mounted at startup
- Errors in one app's CLI don't prevent other apps from loading
- CLI commands call the same data layer as `tools.py` but print formatted console output

**Required export:**
```python
import typer

app = typer.Typer(help="Description of your app commands.")
```

**Command pattern:** Define commands using Typer's `@app.command()` decorator with `typer.Argument` for positional args and `typer.Option` for flags. Call the underlying data layer directly (not tool helper wrappers) and print output via `typer.echo()`.

**CLI vs tool functions:** CLI commands parallel tool functions but are optimized for interactive terminal use. Key differences:
- Tool functions may accept a `Context` parameter for caller metadata; CLI has no context object
- Print formatted text instead of returning dicts
- Use `typer.Exit(1)` for errors instead of returning error dicts

**Discovery behavior:** The `sol call` dispatcher scans `solstone/apps/*/call.py` at startup, imports modules, and mounts any `app` variable that is a `typer.Typer` instance as a sub-command. Private apps (directories starting with `_`) are skipped.

**Reference implementations:**
- Discovery logic: `solstone/think/call.py` - `_discover_app_calls()` function
- App CLI example: `solstone/apps/todos/call.py` - Todo list command

**Skills app reference:** `solstone/apps/skills/call.py` is the current owner-wide pattern for a data-backed app CLI. It exposes `sol call skills list|show|observe|seed|promote|refresh|mark-dormant|retire|edit-request|rename` and routes all writes through `solstone/think/skills.py`, which owns `journal/skills/patterns.jsonl`, `journal/skills/edit_requests.jsonl`, and `journal/skills/{slug}.md`. The shipped daily talents for this app live in `solstone/apps/skills/talent/skill_observer.md` (daily cogitate, priority 41) and `solstone/apps/skills/talent/skill_editor.md` + `skill_editor.py` (daily generate, priority 60). The observer marks patterns for creation/refresh, and the editor consumes those flags or pending `edit-request` rows to write/update exactly one owner-wide profile per run.

---

### 8. `solstone/talent/` - App Generators

Define custom generator prompts that integrate with solstone's output generation system.

**Key Points:**
- Create `solstone/talent/` directory with `.md` files containing JSON frontmatter
- App generators are automatically discovered alongside system generators
- Keys are namespaced as `{app}:{agent}` (e.g., `my_app:weekly_summary`)
- Outputs go to `JOURNAL/YYYYMMDD/talents/_<app>_<agent>.md` (or `.json` if `output: "json"`)

**Metadata format:** Same schema as system generators in `solstone/talent/*.md` - JSON frontmatter includes `title`, `description`, `color`, `schedule` (required), `priority` (required for scheduled prompts), `hook`, `output`, `max_output_tokens`, and `thinking_budget` fields. The `schedule` field must be `"segment"` or `"daily"`. The `priority` field is required for all scheduled prompts - prompts without explicit priority will fail validation. Set `output: "json"` for structured JSON output instead of markdown. Optional `max_output_tokens` sets the maximum response length; `thinking_budget` sets the model's thinking token budget (provider-specific defaults apply if omitted). Generators reject a `cwd` field entirely; working-directory control is only available for `type: "cogitate"` prompts.

**Priority bands:** Prompts run in priority order (lowest first). Recommended bands:
- 10-30: Generators (content-producing prompts)
- 40-60: Analysis agents
- 90+: Late-stage agents
- 99: Fun/optional prompts

**Schedule extraction via hooks:** The live built-in extraction hook is `schedule`:

- `"hook": {"post": "schedule"}` - Writes future scheduled items to `facets/{facet}/activities/{target_day}.jsonl` as anticipated activity records

Example:

```json
{
  "title": "Schedule Extractor",
  "schedule": "daily",
  "hook": {"post": "schedule"}
}
```

**App-data outputs:** For outputs from app-specific data (not transcripts), store in `JOURNAL/apps/{app}/talents/*.md` - these are automatically indexed.

**Template variables:** Generator prompts can use template variables like `$name`, `$preferred`, `$daily_preamble`, and context variables like `$day` and `$day_YYYYMMDD`. See [PROMPT_TEMPLATES.md](PROMPT_TEMPLATES.md) for the complete template system documentation.

**Custom hooks:** Both generators and tool-using agents support custom `.py` hooks for transforming inputs and outputs programmatically. Hooks support both pre-processing (before LLM call) and post-processing (after LLM call):

**Hook configuration:**
- Use `"hook": {"pre": "my_hook"}` for pre-processing hooks
- Use `"hook": {"post": "my_hook"}` for post-processing hooks
- Use both together: `"hook": {"pre": "prep", "post": "process"}`
- Use `"hook": {"flush": true}` to opt into segment flush (see below)
- Resolution: `"name"` → `solstone/talent/{name}.py`, `"app:name"` → `solstone/apps/{app}/talent/{name}.py`, or explicit path

**Pre-hooks** (`pre_process`): Modify inputs before the LLM call
- `context` is the full config dict with: `name`, `use_id`, `provider`, `model`, `prompt`, `system_instruction` (if set), `user_instruction`, `output`, `meta`, and for generators: `day`, `segment`, `span`, `span_mode`, `transcript`, `output_path`
- Return a dict of modified fields to merge back (e.g., `{"prompt": "modified"}`)
- Return `None` for no changes

**Post-hooks** (`post_process`): Transform output after the LLM call
- `result` is the LLM output (markdown or JSON string)
- `context` is the full config dict with: `name`, `use_id`, `provider`, `model`, `prompt`, `output`, `meta`, and for generators: `day`, `segment`, `span`, `span_mode`, `transcript`, `output_path`
- Return modified string, or `None` to use original result

**Flush hooks:** Segment agents can declare `"hook": {"flush": true}` to participate in segment flush. When no new segments arrive for an extended period, the supervisor triggers `journal think --flush --segment <last>`, which runs only flush-enabled agents with `context["flush"] = True` and `context["refresh"] = True`. This lets agents close out dangling state (e.g., end active activities that would otherwise wait indefinitely for the next segment). The timeout is managed by the supervisor — agents should trust the flush signal without their own timeout logic.

Hook errors are logged but don't crash the pipeline (graceful degradation).

```python
# talent/my_hook.py
def pre_process(context: dict) -> dict | None:
    # Modify inputs before LLM call
    return {"prompt": context["prompt"] + "\n\nBe concise."}

def post_process(result: str, context: dict) -> str | None:
    # Transform output after LLM call
    return result + "\n\n## Generated by hook"
```

**Hook idempotency:** Post-hooks that write to shared journal state must be safe to run more than once on the same inputs. `journal think --refresh` bypasses the "output already exists" early-return in `solstone/think/talents.py` and re-executes the talent, which re-fires `post_process` against a fresh LLM result — so any side-effect the hook performs (writing events, appending to a log, updating an index file) will happen again. Pick one of these two patterns:

- **Natural-key dedup.** Read the existing output, compute a natural key per row (e.g., `(facet, event_day, title, start, end)` for facet events), skip rows already present, and append only the new ones. Use this when the output is append-only history and you want to preserve prior writes from other agents.
- **Atomic replace.** Recompute the full output, write it to a temp file, and rename into place. `atomic_write()` in `solstone/think/entities/core.py` is the established helper for text outputs; for JSONL, write the full set of lines to a tempfile and `os.replace()`. Use this when the hook owns the file end-to-end.

(Retired 2026-04-18 Sprint 4.) An earlier `write_events_jsonl` hook in `solstone/think/hooks.py` opened facet-event logs in `"a"` mode with no dedup and doubled row counts on every `journal think --refresh` — see the 2026-04-17 layer-violations audit (V6) tracked in sol pbc's internal engineering notes for the full write-up.

See `docs/coding-standards.md` L8/L9 for the broader principles.

**Reference implementations:**
- System generator templates: `solstone/talent/*.md` (files with `schedule` field but no `tools` field)
- Schedule hook: `solstone/talent/schedule.py`
- Discovery logic: `solstone/think/talent.py` - `get_talent_configs(has_tools=False)`, `get_output_name()`
- Hook loading: `solstone/think/talent.py` - `load_pre_hook()`, `load_post_hook()`

---

### 9. `solstone/talent/` - App Agents and Generators

Define custom agents and generator templates that integrate with solstone's Cortex agent system.

**Key Points:**
- Create `solstone/talent/` directory with `.md` files containing JSON frontmatter
- Both agents and generators live in the same directory - distinguished by frontmatter fields
- Agents have a `tools` field, generators have `schedule` but no `tools`
- App agents/generators are automatically discovered alongside system ones
- Keys are namespaced as `{app}:{name}` (e.g., `my_app:helper`)
- Agents inherit all system agent capabilities (tools, scheduling, multi-facet)

**Metadata format:** Same schema as system agents in `solstone/talent/*.md` - JSON frontmatter includes `title`, `provider`, `model`, `tools`, `schedule`, `priority`, `multi_facet`, `max_output_tokens`, and `thinking_budget` fields. The `priority` field is **required** for all scheduled prompts - prompts without explicit priority will fail validation. See the priority bands documentation in [THINK.md](THINK.md#unified-priority-execution). Optional `max_output_tokens` sets the maximum response length; `thinking_budget` sets the model's thinking token budget (provider-specific defaults apply if omitted; OpenAI uses fixed reasoning and ignores this field). Cogitate agents may also declare `cwd: "journal"` or `cwd: "repo"`; when omitted they default to `journal`, and repo-oriented prompts like `coder` should opt into `repo`. See [CORTEX.md](CORTEX.md) for agent configuration details.

**Template variables:** Agent prompts can use template variables like `$name`, `$preferred`, and pronoun variables. See [PROMPT_TEMPLATES.md](PROMPT_TEMPLATES.md) for the complete template system documentation.

**Reference implementations:**
- System agent examples: `solstone/talent/*.md` (files with `tools` field)
- Discovery logic: `solstone/think/talent.py` - `get_talent_configs(has_tools=True)`, `get_talent()`

#### Prompt Context Configuration

Both generators and agents support an optional `load` key for configuring source data dependencies:

```json
{
  "load": {"transcripts": true, "percepts": false, "talents": {"screen": true}}
}
```

- `load` controls which source types are clustered before generator execution. Values can be:
  - `false` - don't load this source type
  - `true` - load if available
  - `"required"` - load, and skip generation if no content found (useful for generators that only make sense with specific input types, e.g., `"audio": "required"` for speaker detection)
  - For `agents` only: a dict for selective filtering, e.g., `{"entities": true, "meetings": "required", "flow": false}`. Keys are agent names (system) or `"app:agent"` (app-namespaced). An empty dict `{}` means no agents.

Context is provided inline in the `.md` body via template variables:

- `$facets` - focused facet context or all available facets
- `$activity_context` - activity metadata, segment state, and analysis focus sections

**Authoritative source:** `solstone/think/talent.py` - `_DEFAULT_LOAD`, `source_is_enabled()`, `source_is_required()`, `get_talent_filter()`

---

### 10. `solstone/talent/` - Agent Skills

Define [Agent Skills](https://agentskills.io/specification) as subdirectories within `solstone/talent/`. Skills package procedural knowledge, workflows, and resources that AI coding agents (Claude Code, GitHub Copilot, Gemini CLI, etc.) can discover and use on demand.

**Key Points:**
- Create a subdirectory in `solstone/talent/` with a `SKILL.md` file (YAML frontmatter + markdown body)
- The directory name must match the `name` field in the YAML frontmatter
- Skill names must be unique across system `solstone/talent/` and all `solstone/apps/*/talent/` directories
- `make skills` discovers all skills and symlinks them into `journal/.agents/skills/` and `journal/.claude/skills/`
- Skills are standalone — they don't interact with the talent agent/generator system
- The talent loader ignores subdirectories, so skills won't interfere with agent discovery

**Directory structure:**
```
talent/my-skill/
├── SKILL.md           # Required: YAML frontmatter + instructions
├── scripts/           # Optional: Executable code (Python, Bash, etc.)
├── references/        # Optional: Additional documentation loaded on demand
└── assets/            # Optional: Static resources (templates, data files)
```

**SKILL.md format:**
```yaml
---
name: my-skill
description: Short description of what this skill does and when to use it.
---

# Instructions

Step-by-step procedures, examples, and domain knowledge for the agent.
```

**Required frontmatter fields:**
- `name` — Max 64 chars, lowercase letters + numbers + hyphens, must match directory name
- `description` — Max 1024 chars, describes what the skill does *and when to use it*

**Optional frontmatter fields:**
- `license` — License name (e.g., `Apache-2.0`)
- `compatibility` — Max 500 chars, environment requirements
- `metadata` — Arbitrary key-value string map
- `allowed-tools` — Space-delimited list of pre-approved tools (experimental)

**App skills** work the same way — place a skill directory inside `solstone/apps/my_app/talent/`:
```
apps/my_app/talent/my-skill/
├── SKILL.md
└── references/
```

**Running `make skills`:** Discovers all `SKILL.md` files under `solstone/talent/*/` and `solstone/apps/*/talent/*/`, then creates symlinks in `journal/.agents/skills/` and `journal/.claude/skills/` so that all supported coding agents see the same skills. Errors if two skills share the same directory name.

---

### 11. `maint/` - Maintenance Tasks

Define one-time maintenance scripts that run automatically when supervisor starts.

**Key Points:**
- Create `maint/` directory with standalone Python scripts (each with a `main()` function)
- Scripts are discovered and run in sorted order by filename (use `000_`, `001_` prefixes for ordering)
- Completed tasks tracked in `<journal>/maint/{app}/{task}.jsonl` - runs once per journal
- Exit code 0 = success, non-zero = failure (failed tasks can be re-run with `--force`)
- Use `setup_cli()` for consistent argument parsing and logging

**CLI:** `journal maint` (run pending), `journal maint --list` (show status), `journal maint --force` (re-run all)

**Reference implementations:**
- Example task: `solstone/apps/entities/maint/001_migrate_to_journal_entities.py` - real migration task demonstrating maint patterns
- Discovery logic: `solstone/think/maint.py` - `discover_tasks()`, `run_task()`

---

### 12. `tests/` - App Tests

Apps can include their own tests that are discovered and run separately from core tests.

**Key Points:**
- Create `tests/` directory with `conftest.py` and `test_*.py` files
- App fixtures should be self-contained (only use pytest builtins like `tmp_path`, `monkeypatch`)
- Tests run via `make test-apps` (all apps) or `make test-app APP=my_app`
- Integration tests can use `@pytest.mark.integration` but live in the same flat structure

**Directory structure:**
```
apps/my_app/tests/
├── __init__.py
├── conftest.py      # Self-contained fixtures
└── test_*.py        # Test files
```

**Reference implementations:**
- Fixture patterns: `solstone/apps/todos/tests/conftest.py`
- Tool testing: `solstone/apps/todos/tests/test_tools.py`

---

### 13. `events.py` - Server-Side Event Handlers

Define server-side handlers that react to Callosum events. Handlers run in Convey's thread pool, enabling reactive backend logic without creating new services.

**Key Points:**
- Create `events.py` with functions decorated with `@on_event(tract, event)`
- Handlers receive an `EventContext` with `msg`, `app`, `tract`, `event` fields
- Discovered at Convey startup; events processed serially with 30s timeout per handler
- Errors are logged but don't affect other handlers or the web server
- Wildcards supported: `@on_event("*", "*")` matches all events

**Available imports** (same as route handlers):
- `from convey import state` - Access `state.journal_root`
- `from convey import emit` - Emit events back to Callosum
- `from solstone.apps.utils import get_app_storage_path, log_app_action` - App storage
- `from solstone.convey.utils import load_json, save_json, spawn_agent` - Utilities

**Not available** (no Flask request context):
- `request`, `session`, `current_app`
- `error_response()`, `success_response()`, `parse_pagination_params()`

**Reference implementations:**
- Framework: `solstone/apps/events.py` - `EventContext` dataclass, decorator, discovery
- Example: `solstone/apps/entities/events.py` - Entity activity tracking via event handlers

---

## Flask Utilities

Available in `solstone/convey/utils.py`:

### Route Helpers
- `error_response(message, code=400)` - Standard JSON error response
- `success_response(data=None, code=200)` - Standard JSON success response
- `parse_pagination_params(default_limit, max_limit, min_limit)` - Extract and validate limit/offset from request.args

### Date Formatting
- `format_date(date_str)` - Format YYYYMMDD as "Wednesday January 14th"

### Agent Spawning
- `spawn_agent(prompt, name, provider, config)` - Spawn Cortex agent, returns use_id

### JSON Utilities
- `load_json(path)` - Load JSON file with error handling (returns None on error)
- `save_json(path, data, indent, add_newline)` - Save JSON with formatting (returns bool)

**See source:** `solstone/convey/utils.py` for full signatures and documentation

### App Storage

Apps can persist journal-specific configuration and data in `<journal>/apps/<app_name>/`:

```python
from solstone.apps.utils import get_app_storage_path, load_app_config, save_app_config
```

- `get_app_storage_path(app_name, *sub_dirs, ensure_exists)` - Get Path to app storage directory
- `load_app_config(app_name, default)` - Load app config from `config.json`
- `save_app_config(app_name, config)` - Save app config to `config.json`

**See source:** `solstone/apps/utils.py` for implementation details

### Action Logging

Apps that modify owner data should log actions for audit trail purposes:

```python
from solstone.apps.utils import log_app_action
```

- `log_app_action(app, facet, action, params, day=None)` - Log owner-initiated action

**Parameters:**
- `app` - App name where action originated
- `facet` - Facet where action occurred, or `None` for journal-level actions
- `action` - Action type using `{domain}_{verb}` naming (e.g., `entity_add`, `todo_complete`)
- `params` - Action-specific parameters dict
- `day` - Optional day in YYYYMMDD format (defaults to today)

**Facet-scoped vs journal-level:**
- Pass a facet name for facet-specific actions (todos, entities, etc.)
- Pass `facet=None` for journal-level actions (settings, observers, etc.)

Log after successful mutations, not attempts.

---

## Think Module Integration

Available functions from the `think` module:

### Facets
`solstone/think/facets.py`: `get_facets()` - Returns dict of facet configurations

### Todos
`solstone/apps/todos/todo.py`:
- `get_todos(day, facet)` - Get todo list for day and facet
- `TodoChecklist` class - Load and manipulate todo markdown files

### Entities
`solstone/think/entities/`: `load_entities(facet)` - Load entities for a facet

See [talent/journal/SKILL.md](../talent/journal/SKILL.md), [CORTEX.md](CORTEX.md), [CALLOSUM.md](CALLOSUM.md) for subsystem details.

---

## JavaScript APIs

### Global Variables

Defined in `solstone/convey/templates/app.html`:
- `window.facetsData` - Array of facet objects `[{name, title, color, emoji}, ...]`
- `window.selectedFacet` - Current facet name or null (see Facet Selection below)
- `window.appFacetCounts` - Badge counts for current app `{"work": 5, "personal": 3}` (set via route's `facet_counts`)

### Facet Selection

Apps can access and control facet selection through a uniform API:
- `window.selectedFacet` - Current facet name or null (initialized by server, updated on change)
- `window.selectFacet(name)` - Change selection programmatically
- `facet.switch` CustomEvent - Dispatched when selection changes
  - Event detail: `{facet: 'work' or null, facetData: {name, title, color, emoji} or null}`

**Facet Modes:**
- **all-facet mode**: `window.selectedFacet === null`, show content from all facets
- **specific-facet mode**: `window.selectedFacet === "work"`, show only that facet's content
- Selection persisted via cookie, synchronized across facet pills

**UX Tip:** Apps should provide visual indication when in all-facet mode vs showing a specific facet. For example, group items by facet, show facet badges/colors on items, or display a subtle "All facets" label. This helps owners understand the scope of what they're viewing.

**See implementation:** `solstone/convey/static/app.js` - Facet switching logic and event dispatch

**Disabled mode:** On apps with `facets.disabled: true`, the facet bar is visible but inert — pills render without interactivity or tab stops. The container is marked `aria-hidden="true"` so screen readers skip it. The bar remains visually present as always-visible chrome.

### WebSocket Events (Client-Side)

`window.appEvents` API defined in `solstone/convey/static/websocket.js`:
- `listen(tract, callback)` - Subscribe to specific tract or '*' for all events
- Messages structure: `{tract: 'cortex', event: 'agent_complete', ...data}`

**Common tracts:** `cortex`, `indexer`, `observe`, `task`

See [CALLOSUM.md](CALLOSUM.md) for complete event protocol.

### Server-Side Events

Emit Callosum events from route handlers using `convey.emit()`:

```python
from convey import emit

@my_bp.route("/action", methods=["POST"])
def handle_action():
    # ... process request ...

    # Emit event (non-blocking, drops if disconnected)
    emit("my_app", "action_complete", item_id=123, status="success")

    return jsonify({"status": "ok"})
```

**Behavior:**
- Non-blocking: queues message for background thread
- If Callosum disconnected, message is dropped (with debug logging)
- Returns `True` if queued, `False` if bridge not started or queue full

**Reference implementations:** `solstone/apps/import/routes.py`, `solstone/apps/observer/routes.py`

---

## CSS Styling

### Workspace Containers

**Always wrap your workspace content** in one of these standardized containers for consistent spacing and layout:

**For readable content** (forms, lists, messages, text):
```html
<div class="workspace-content">
  <!-- Your app content here -->
</div>
```

**For data-heavy content** (tables, grids, calendars):
```html
<div class="workspace-content-wide">
  <!-- Your app content here -->
</div>
```

**Key differences:**
- `.workspace-content` - Centered with 1200px max-width, ideal for readability
- `.workspace-content-wide` - Full viewport width, ideal for data tables and grids
- Both include consistent padding and mobile responsiveness

**See:** `solstone/convey/static/app.css` for implementation details

**Examples:**
- Standard: `solstone/apps/home/workspace.html`, `solstone/apps/todos/workspace.html`, `solstone/apps/entities/workspace.html`
- Wide: `solstone/apps/search/workspace.html`, `solstone/apps/activities/_day.html`, `solstone/apps/import/workspace.html`

### CSS Variables

Dynamic variables based on selected facet (update automatically on facet change):

```css
:root {
  --facet-color: #3b82f6;      /* Selected facet color */
  --facet-bg: #3b82f61a;       /* 10% opacity background */
  --facet-border: #3b82f6;     /* Border color */
}
```

Use these in your app-specific styles to respond to facet theme.

### App-Specific Styles

**Best practice:** Scope styles with unique class prefix to avoid conflicts.

**Example:** `solstone/apps/stats/workspace.html` shows scoped `.stats-*` classes for all custom styles in its `<style>` block.

### Global Styles

Main stylesheet `solstone/convey/static/app.css` provides base components. Review for available classes and patterns.

---

## Common Patterns

### Date-Based Navigation
See `solstone/apps/todos/routes.py:todos_day()` - Shows date validation and `format_date()` usage. Day navigation is handled automatically by the date_nav component.

### AJAX Endpoints
See `solstone/apps/todos/routes.py:move_todo()` - Shows JSON parsing, validation, `error_response()`, `success_response()`.

### Form Handling with Flash Messages
See `solstone/apps/todos/routes.py:todos_day()` POST handler - Shows form processing, validation, flash messages, redirects.

### Facet-Aware Queries
See `solstone/apps/todos/routes.py:todos_day()` - Loads data per-facet when selected, or all facets when null.

### Facet Pill Badges
Pass `facet_counts` dict to `render_template()` to show initial badge counts on facet pills:
```python
facet_counts = {"work": 5, "personal": 3}
return render_template("app.html", facet_counts=facet_counts)
```
For client-side updates (e.g., after completing a todo), use `AppServices.badges.facet.set(facetName, count)`.

See `solstone/apps/todos/routes.py:todos_day()` - Computes pending counts from already-loaded data.

---

## Debugging Tips

### Check Discovery

```bash
# Start Convey with debug logging
FLASK_DEBUG=1 convey

# Look for log lines:
# "Discovered app: my_app"
# "Registered blueprint: app:my_app"
```

### Common Issues

| Issue | Cause | Fix |
|-------|-------|-----|
| App not discovered | Missing `workspace.html` | Ensure workspace.html exists |
| Blueprint not found (with routes.py) | Wrong variable name | Use `{app_name}_bp` exactly |
| Import error (with routes.py) | Blueprint name mismatch | Use `"app:{app_name}"` exactly |
| Hyphens in name | Directory uses hyphens | Rename to use underscores |
| Custom routes don't work | URL prefix mismatch | Check `url_prefix` matches pattern |

### Logging

Use `current_app.logger` from Flask for debugging. See `solstone/apps/todos/routes.py` for examples.

---

## Best Practices

1. **Use underscores** in directory names (`my_app`, not `my-app`)
2. **Wrap workspace content** in `.workspace-content` or `.workspace-content-wide`
3. **Scope CSS** with unique class names to avoid conflicts
4. **Validate input** on all POST endpoints (use `error_response`)
5. **Check facet selection** when loading facet-specific data
6. **Use state.journal_root** for journal path (always available)
7. **Pass facet_counts** from routes if app has per-facet counts
8. **Handle errors gracefully** with flash messages or JSON errors
9. **Test facet switching** to ensure content updates correctly
10. **Use background services** for WebSocket event handling
11. **Follow Flask patterns** for blueprints, url_for, etc.

---

## Example Apps

Browse `solstone/apps/*/` directories for reference implementations. Apps range in complexity:

- **Minimal** - Just `workspace.html` (e.g., `solstone/apps/home/`, `solstone/apps/health/`)
- **Styled** - Custom CSS, background services (e.g., `solstone/apps/support/`)
- **Full-featured** - Routes, forms, AJAX, badges, tools (e.g., `solstone/apps/todos/`, `solstone/apps/entities/`)

---

## Additional Resources

- **`solstone/apps/__init__.py`** - App discovery and registry implementation
- **`solstone/convey/apps.py`** - Context processors and vendor library helper
- **`solstone/convey/templates/app.html`** - Main app container template
- **`solstone/convey/static/app.js`** - AppServices framework
- **`solstone/convey/static/websocket.js`** - WebSocket event system
- [../AGENTS.md](../AGENTS.md) - Project development guidelines and standards
- [storage.md](../talent/journal/references/storage.md) - Journal directory structure and data organization
- [CORTEX.md](CORTEX.md) - Agent system architecture and spawning agents
- [CALLOSUM.md](CALLOSUM.md) - Message bus protocol and WebSocket events

For Flask documentation, see [https://flask.palletsprojects.com/](https://flask.palletsprojects.com/)
