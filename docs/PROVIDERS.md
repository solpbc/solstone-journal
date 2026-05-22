# Provider Implementation Guide

Guide for implementing new AI providers in the think module.

For a high-level overview of the think module, see [THINK.md](THINK.md).

## Required Exports

Each provider module in `solstone/think/providers/` must export three functions:

| Function | Purpose |
|----------|---------|
| `run_generate()` | Synchronous text generation, returns `GenerateResult` |
| `run_agenerate()` | Asynchronous text generation, returns `GenerateResult` |
| `run_cogitate()` | Tool-calling execution |

See `solstone/think/providers/__init__.py` for the canonical export list and `solstone/think/providers/google.py` as a reference implementation.

Each provider module must also define `__all__` exporting these three functions.

## API Key Handling

API keys are configured in the ``env`` section of ``journal/config/journal.json``. At process startup, ``setup_cli()`` loads these into ``os.environ``. Providers read keys from ``os.environ`` — no ``.env`` files or ``dotenv`` are involved.

**Naming convention:** `{PROVIDER}_API_KEY` (e.g., `GOOGLE_API_KEY`, `OPENAI_API_KEY`)

**Implementation pattern:**
```python
api_key = os.getenv("MYPROVIDER_API_KEY")
if not api_key:
    raise ValueError("MYPROVIDER_API_KEY not found in environment")
```

**Client caching:** Providers typically cache client instances as module-level singletons to enable connection reuse:
```python
_client = None

def _get_client():
    global _client
    if _client is None:
        api_key = os.getenv("MYPROVIDER_API_KEY")
        if not api_key:
            raise ValueError("MYPROVIDER_API_KEY not found in environment")
        _client = MyProviderClient(api_key=api_key)
    return _client
```

**Settings app integration:** Add your provider to `PROVIDER_METADATA` in `solstone/think/providers/__init__.py` with `label` and `env_key` fields. The settings UI dynamically builds provider dropdowns from the registry. Add corresponding API key UI fields in `solstone/apps/settings/workspace.html` for owner configuration.

## run_generate() / run_agenerate()

These functions handle direct LLM text generation. The unified API in `solstone/think/models.py` routes requests to provider-specific implementations and handles token logging and JSON validation centrally.

**Function signature:**
```python
from solstone.think.providers.shared import GenerateResult

def run_generate(
    contents: Union[str, List[Any]],
    model: str,
    temperature: float = 0.3,
    max_output_tokens: int = 8192 * 2,
    system_instruction: Optional[str] = None,
    json_output: bool = False,
    thinking_budget: Optional[int] = None,
    timeout_s: Optional[float] = None,
    **kwargs: Any,
) -> GenerateResult:
```

The `run_agenerate()` function has the same signature but is `async`.

**Return type - GenerateResult:**
```python
class GenerateResult(TypedDict, total=False):
    text: Required[str]           # Response text
    usage: Optional[dict]         # Normalized usage dict
    finish_reason: Optional[str]  # Normalized: "stop", "max_tokens", etc.
    thinking: Optional[list]      # List of thinking block dicts
```

**Parameter details:**

| Parameter | Notes |
|-----------|-------|
| `contents` | String, list of strings, or list with mixed content. For vision-capable providers (currently Google only), can include PIL Image objects. Other providers stringify non-text content. |
| `model` | Already resolved by routing - providers don't need to handle model selection. |
| `max_output_tokens` | Response token limit. Note: Google internally adds `thinking_budget` to this for total budget calculation. |
| `system_instruction` | System prompt. Providers handle this per their API (separate field, prepended message, etc.). |
| `json_output` | Request JSON response. Google uses `response_mime_type`, Anthropic/OpenAI use response format or system instruction. |
| `thinking_budget` | Token budget for reasoning/thinking. Must be `> 0` to enable; `None` or `0` means no thinking. Google and Anthropic use this directly. OpenAI ignores `thinking_budget` — instead, reasoning effort is controlled via model name suffixes (e.g., `"gpt-5.2-high"`). Valid suffixes: `-none`, `-low`, `-medium`, `-high`, `-xhigh`. Without a suffix, `reasoning_effort` is omitted and OpenAI uses the model default. Note: `run_cogitate()` always enables thinking regardless of this parameter. |
| `timeout_s` | Request timeout in seconds. Convert to provider's expected format (e.g., Google uses milliseconds internally). |
| `**kwargs` | Absorb unknown kwargs for forward compatibility. Provider-specific options (e.g., `cached_content` for Google) pass through here. |

**Key responsibilities:**
- Accept the common parameter set shown above
- Return `GenerateResult` with text, usage, finish_reason, and thinking
- Normalize `finish_reason` to standard values: `"stop"`, `"max_tokens"`, `"safety"`, etc.
- Handle provider-specific response parsing

**Note:** Token logging and JSON validation are handled by the wrapper in `solstone/think/models.py`, not by providers.

**Important:** Providers should gracefully ignore unsupported parameters rather than raising errors.

## run_cogitate()

Handles tool-calling execution.

```python
async def run_cogitate(
    config: Dict[str, Any],
    on_event: Optional[Callable[[dict], None]] = None,
) -> str:
```

**Config dict fields** (see `solstone/think/agents.py` `main_async()` for routing logic):
- `prompt`: User's input (required)
- `model`: Model identifier
- `max_tokens`: Output token limit
- `system_instruction`: System instruction (journal.md for agents)
- `extra_context`: Runtime context (facets, insights list, datetime) as first user message
- `user_instruction`: Agent-specific prompt as second user message
- `tools`: Optional list of allowed tool names
- `use_id`, `name`: Identity for logging and tool calls
- `session_id`: solstone-owned session ID for conversation continuation; Google cogitate history is stored under `journal/.cache/cogitate-history/`
- `chat_id`: Chat ID for reverse lookup from agent to chat

**Event emission:**

Providers must emit events via the `on_event` callback. See `solstone/think/providers/shared.py` for TypedDict definitions:

| Event | When |
|-------|------|
| `StartEvent` | Agent run begins |
| `ToolStartEvent` | Tool invocation starts |
| `ToolEndEvent` | Tool invocation completes |
| `ThinkingEvent` | Reasoning/thinking content available |
| `FinishEvent` | Agent run completes successfully |
| `ErrorEvent` | Error occurs |

Use `JSONEventCallback` from `solstone/think/providers/shared.py` to wrap the callback and auto-add timestamps.

**Finish event format:**

The `finish` event must include the result text and should include usage for token tracking:
```python
callback.emit({
    "event": "finish",
    "result": final_text,
    "usage": usage_dict,  # Same format as token logging
    "ts": int(time.time() * 1000),
})
```

**Error handling pattern:**

All providers must follow this pattern to prevent duplicate error reporting:
```python
try:
    # ... agent logic ...
except Exception as exc:
    callback.emit({
        "event": "error",
        "error": str(exc),
        "trace": traceback.format_exc(),
    })
    setattr(exc, "_evented", True)  # Prevents duplicate reporting
    raise
```

**Tool integration:**

Invoke tools via `sol call <module> <command> [args...]` commands.
Providers should route tool calls through the configured command path and
honor `config["tools"]` allowlists when present.


**Conversation continuation:**

When `session_id` is provided, use the provider's native continuation mechanism
or a solstone-owned history file where the provider has no durable session
handle. The `session_id` is reused for all subsequent continuations within the
same chat.

## Token Logging

Token logging is handled centrally by the wrapper in `solstone/think/models.py`. Providers return usage data in their `GenerateResult`, and the wrapper calls `log_token_usage()`.

**Usage dict format:**

Providers normalize usage into the unified schema defined by `USAGE_KEYS` in `solstone/think/providers/shared.py`. Each provider's `_extract_usage()` is responsible for mapping API-specific field names to these canonical keys. `log_token_usage()` passes through known keys — it does **not** re-normalize.

```python
usage_dict = {
    "input_tokens": 1500,            # Required
    "output_tokens": 500,            # Required
    "total_tokens": 2000,            # Required (computed if missing)
    "cached_tokens": 800,            # Optional: cache hits
    "reasoning_tokens": 200,         # Optional: thinking/reasoning tokens
    "cache_creation_tokens": 100,    # Optional: cache creation cost
    "requests": 1,                   # Optional: request count
}
```

**Key points:**
- Return usage in `GenerateResult["usage"]` - wrapper handles logging
- For `run_cogitate()`, include usage in the `finish` event

## Context & Routing

Context strings determine provider and model selection. Providers receive already-resolved models, but understanding the system helps:

**Context naming convention:**
- Talent configs (agents/generators): `talent.{source}.{name}` where source is `system` or app name
  - System: `talent.system.meetings`, `talent.system.default`
  - App: `talent.entities.observer`, `talent.chat.helper`
- Other contexts: `{module}.{feature}[.{operation}]`
  - Examples: `observe.describe.frame`, `app.chat.title`

**Dynamic discovery:** All context metadata (tier/label/group) is defined in prompt .md files via YAML frontmatter:
- Prompt files: Listed in `PROMPT_PATHS` in `solstone/think/models.py` - add `context`, `tier`, `label`, `group` fields
- Categories: `solstone/observe/categories/*.md` - add `tier`, `label`, `group` fields
- System talent: `solstone/talent/*.md` - add `tier`, `label`, `group` fields in frontmatter
- App talent: `solstone/apps/*/talent/*.md` - add `tier`, `label`, `group` fields in frontmatter

All contexts are discovered at runtime. Use `get_context_registry()` to get the complete context map.

**Resolution** (handled by `solstone/think/models.py` `resolve_provider(context, agent_type)`):
1. Exact match in journal.json `providers.contexts`
2. Glob pattern match (fnmatch) with specificity ranking
3. Dynamic context registry (discovered prompts, categories, talent configs)
4. Type-specific default (from `providers.generate` or `providers.cogitate`)
5. System defaults from `TYPE_DEFAULTS`

Providers don't implement routing - they receive the resolved model.

## Configuration

Provider configuration lives in `journal.json` under the `providers` key.

**Structure:**
```
providers:
  generate:
    provider: <provider-name>
    tier: <1|2|3>
    backup: <provider-name>
  cogitate:
    provider: <provider-name>
    tier: <1|2|3>
    backup: <provider-name>
  contexts:
    <context-pattern>:
      provider: <provider-name>
      model: <explicit-model>  # OR
      tier: <1|2|3>            # tier-based resolution
  models:
    <provider-name>:
      "<tier>": "<model-override>"
```

The `generate` section controls text generation (analysis, extraction, transcription).
The `cogitate` section controls tool-calling agents (interactive chat, daily briefings).
Each section has its own provider, tier, and backup provider.

**Tier system:**
- 1 = PRO (most capable)
- 2 = FLASH (balanced)
- 3 = LITE (fast/cheap)

See `tests/fixtures/journal/config/journal.json` for a complete example and `solstone/think/models.py` `PROVIDER_DEFAULTS` for tier-to-model mappings.

## Testing

**Required test coverage:**

1. **Unit tests** in `tests/test_<provider>.py`:
   - Mock API responses
   - Test parameter handling
   - Test error cases

2. **Integration tests** in `tests/integration/test_<provider>_backend.py`:
   - Live API calls (require API keys)
   - End-to-end generation
   - Token usage verification

See existing test files for patterns:
- `tests/test_google.py`, `tests/test_openai.py`, `tests/test_anthropic.py`
- `tests/integration/test_google_backend.py`, etc.

Run integration tests with: `make test-integration`

## Batch Processing

The `Batch` class in `solstone/think/batch.py` automatically works with all providers via the unified `agenerate()` API in `solstone/think/models.py`. No provider-specific batch implementation is needed - just ensure your `run_agenerate()` works correctly.

## OpenAI-Compatible Providers

For providers with OpenAI-compatible APIs (e.g., DigitalOcean, Azure OpenAI, local LLMs), you can leverage the OpenAI SDK with a custom base URL:

```python
from openai import OpenAI

client = OpenAI(
    api_key=os.getenv("MYPROVIDER_API_KEY"),
    base_url="https://api.myprovider.com/v1",
)
```

This allows reusing much of the OpenAI provider's patterns for request/response handling.

The Ollama provider (`solstone/think/providers/ollama.py`) takes a different approach —
it uses Ollama's native ``/api/chat`` endpoint directly via ``httpx`` for
reliable thinking control. See the Ollama section below.

## Ollama (Local) Provider

The ``ollama`` provider connects to a local Ollama instance via the native
``/api/chat`` endpoint (not the OpenAI-compatible endpoint, which silently
ignores the ``think`` parameter on models like Qwen3.5). Key differences
from cloud providers:

- **No API key required.** ``validate_key()`` checks Ollama reachability
  instead of key validity.
- **Model prefix convention:** Models use the ``ollama-local/`` prefix
  (e.g., ``ollama-local/qwen3.5:9b``). The prefix is stripped before
  sending requests to the Ollama API.
- **Thinking support:** Controlled via Ollama's ``think`` parameter,
  mapped from ``thinking_budget``. Budget > 0 enables thinking;
  None or 0 disables it.
- **Cogitate via OpenCode CLI.** ``run_cogitate()`` uses the OpenCode CLI
  (``opencode run --format json``) as a subprocess, following the same
  CLIRunner pattern as the other providers. Requires OpenCode CLI installed
  and configured with a user-level ``.opencode/opencode.json`` that registers
  the local Ollama instance as a provider. Do not place this config in the
  project root — it belongs in the user's config directory.
- **Base URL:** Reads ``OLLAMA_BASE_URL`` env var, defaults to
  ``http://localhost:11434``.

## MLX (Local, Apple Silicon) Provider

The ``mlx`` provider (`solstone/think/providers/mlx.py`) runs vision/generate
on-device on Apple Silicon via the MLX framework — used for the screen-analysis
path with nothing sent to a cloud provider. It surfaces in Settings → Providers
as **"MLX (Local, Apple Silicon)"**.

- **Generate-only, no cogitate.** ``run_generate()`` / ``run_agenerate()`` are
  implemented; ``run_cogitate()`` raises — MLX is vision/generate-only in v1.
  Configure a cloud provider for cogitate (tool-using) agents.
- **No API key.** ``env_key`` is empty and ``validate_key()`` always returns
  ``{"valid": True}`` — availability is gated on platform + RAM, not a secret.
- **Availability gating.** ``is_mlx_available()`` requires Apple Silicon plus the
  ``mlx``/``mlx-vlm`` packages; ``is_mlx_available_for_model(spec)`` additionally
  enforces each model's ``min_ram_bytes`` floor.
- **Model registry (`_MLX_MODEL_REGISTRY`).** Pinned by repo + revision:
  ``qwen3.5:9b`` (`mlx-community/Qwen3.5-9B-MLX-8bit`, ≥16 GB) and
  ``gemma-4-26b-a4b-it-mlx-4bit`` (`mlx-community/gemma-4-26b-a4b-it-4bit`, ≥24 GB,
  with a ``post_load`` hook that constrains the Gemma 4 vision tower to the
  screenshot-faithful patch budget).
- **On-demand snapshot.** The pinned snapshot downloads in the background on first
  enable; a missing snapshot raises ``ModelSnapshotMissingError`` rather than
  silently degrading. Loaded models are cached at module level.

## Checklist for New Providers

**Core implementation:**
1. Create `solstone/think/providers/<name>.py` with `__all__ = ["run_generate", "run_agenerate", "run_cogitate"]`
2. Implement `run_generate()`, `run_agenerate()`, `run_cogitate()` following signatures above
3. Import `GenerateResult` from `think.providers.shared` and return it from generate functions

**Model constants** in `solstone/think/models.py`:
4. Add model constants using the pattern `{PROVIDER}_{TIER}` (e.g., `DO_LLAMA_70B`, `DO_MISTRAL_NEMO`)
   - Existing examples: `GEMINI_FLASH`, `GPT_5`, `CLAUDE_SONNET_4`
5. Add provider tier mappings to `PROVIDER_DEFAULTS` dict
6. Update `get_model_provider()` to detect your models by prefix (critical for cost tracking)

**Registry:**
7. Add provider to `PROVIDER_REGISTRY` in `solstone/think/providers/__init__.py`
8. Add routing case in `solstone/think/agents.py` `main_async()` (around line 331)

**Settings UI:**
9. Add provider to `PROVIDER_METADATA` in `solstone/think/providers/__init__.py` with `label` and `env_key`
10. Add API key UI field in `solstone/apps/settings/workspace.html`

**Testing:**
11. Create unit tests in `tests/test_<name>.py`
12. Create integration tests in `tests/integration/test_<name>_backend.py`
13. Add test contexts to `tests/fixtures/journal/config/journal.json`

**Documentation:**
14. Update `solstone/think/providers/__init__.py` docstring
15. Update `docs/THINK.md` providers table
16. Update `docs/CORTEX.md` valid provider values
