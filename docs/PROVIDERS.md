# Thinking Provider Architecture

Solstone is local-first software for personal use. It supports one active
provider/model profile, configured in Thinking, and never silently switches
providers. The implementation deliberately has no special Vertex AI, Azure
OpenAI, Bedrock, or other enterprise-cloud integration.

For the broader pipeline, see `docs/THINK.md`.

## One Active Brain

`config/journal.json` stores the selected profile at:

```json
{
  "providers": {
    "active": {
      "provider": "local",
      "model": "local/qwen3.5-4b"
    }
  }
}
```

`solstone/think/models.py::resolve_provider()` is the only runtime resolver.
The `generate` and `cogitate` arguments identify the interface being invoked,
but both resolve the same `providers.active` profile. A missing profile is an
explicit no-brain state. Key presence and local readiness never choose a
provider implicitly.

Provider and model overrides are rejected in talent frontmatter, cortex
requests, batch requests, and direct generate calls. Thinking is the sole
configuration surface for the active brain. Talent `disabled` and `extract`
controls are separate metadata under `talent_overrides`; they do not route
models.

## Supported Owner Choices

The Thinking app exposes five setup choices:

- Bundled local, using Solstone's installed llama-server or mlx-vlm runtime.
- An owner-supplied OpenAI-compatible URL, model id, and optional bearer key.
- OpenAI with an owner-supplied API key and model id.
- Anthropic with an owner-supplied API key and model id.
- Google AI Studio with an owner-supplied API key and Gemini model id.

The direct cloud options are convenience presets. The arbitrary endpoint is a
plain compatibility contract: Solstone sends OpenAI-compatible requests, but
does not add vendor-specific support for whatever sits behind that URL.

Managed personal cloud keys remain journal-local:

- `env.OPENAI_API_KEY`
- `env.ANTHROPIC_API_KEY`
- `env.GOOGLE_API_KEY`

## Dispatch

`solstone/think/providers/__init__.py` has a deliberately small registry:

- `google`, `openai`, and `anthropic` all map to
  `solstone/think/providers/openhands.py`.
- `local` maps to `solstone/think/providers/local.py`.

The effective modules implement:

- `run_generate()` for synchronous single-shot generation.
- `run_agenerate()` for asynchronous single-shot generation.
- `run_cogitate()` for tool-using OpenHands conversations.

### Personal cloud

`openhands.py` is the single cloud transport. It builds an OpenHands `LLM` and
lets LiteLLM translate the request to OpenAI, Anthropic, or Google AI Studio.
It also normalizes text, usage, finish reasons, and thinking blocks back into
Solstone's `GenerateResult`.

Generate calls explicitly neutralize OpenHands' agent-oriented defaults and
then apply only Solstone's requested behavior. The transport preserves:

- sync and async calls;
- multimodal message content;
- JSON object and JSON Schema response formats;
- OpenAI reasoning-effort suffixes;
- Anthropic and Gemini thinking budgets;
- normalized usage, resolved model, and finish reason.

Direct OpenAI generation uses the Responses API. Anthropic and Google use chat
completion through LiteLLM's provider translation. Key/model validation sends
a tiny request through this same runtime path, so validation can incur a small
provider charge.

OpenHands/LiteLLM may internally contain code for many providers. That does not
make them Solstone-supported providers: Solstone exposes no registry entry,
config, UI, credential flow, or validation path for enterprise integrations.

### Local and arbitrary endpoints

`local.py` remains a thin product-policy wrapper rather than a second general
cloud adapter. It owns guarantees that OpenHands alone does not provide:

- bundled runtime installation and manifest-backed readiness;
- context-budget fitting and local schema preparation;
- Qwen sampling and chat-template controls;
- cross-process local admission and bounded retry;
- content-free local inference telemetry;
- confidential egress/attestation gates;
- stable local error classification.

Bundled local posts to the supervisor-owned loopback server. Its install status
lives under `health/providers/`, while artifact truth lives in provider manifests
and the affirmative proof cache. A configured endpoint uses:

- `providers.local.endpoint_url`
- `providers.local.served_model_id`
- `providers.local.credential` (optional)
- `providers.local.parallel_slots` (optional)

Both generate and cogitate use the endpoint's OpenAI-compatible contract. The
configured logical provider remains `local`, so the same readiness and safety
boundary applies without maintaining vendor-specific adapters.

## Local Admission

Bundled local and non-confidential arbitrary endpoints share the governed local
admission boundary. Cloud and confidential processing bypass it. Capacity is
kept intentionally small: one slot on the Linux floor and Apple mlx-vlm, two on
the capable Linux tier, or the explicit `parallel_slots` value for an arbitrary
endpoint.

Admission uses per-slot `flock` files under
`health/local-inference-admission/`, coordinating independent journal
processes. Queue time consumes the caller's existing timeout. Cogitate yields
its permit while a nested `sol` command runs and reacquires it before the next
model turn.

Bundled attempts append content-free telemetry to
`health/local-inference/YYYYMMDD.jsonl`. Records include timing, capacity,
token counts, retry index, finish reason, and safe failure codes—never prompts,
responses, schemas, images, URLs, or credentials.

## Failure Semantics

Provider failure is not a routing signal. Solstone surfaces the failure and
recovery action for the active profile.

- Quota failures are recorded through `record_brain_runtime_failure` into `health/brain.json`.
- Endpoint reachability and contract errors are classified by the local
  endpoint wrapper.
- Local generate retries once only for narrow capacity/truncation cases, using
  the same provider.
- Missing local runtime, model files, RAM, endpoint readiness, or confidential
  attestation fails closed rather than falling back to cloud.

Owner-facing brain health and Thinking readiness read canonical evidence from
`health/brain.json`. Confidential SPP egress remains authorized only by the
current process-local attestation state in `spp_transport`.

## Migration Boundary

The Thinking maintenance task collapses legacy `providers.generate` and
`providers.cogitate` into `providers.active`. If they differ, cogitate wins
because its model already satisfies the tool-capable interface. A key-only
legacy install is materialized once in Google, Anthropic, OpenAI order. The task
selects bundled local when no prior profile or personal cloud key exists. It
also:

- removes tier, backup, model-map, Google-backend, and Vertex fields;
- deletes the canonical legacy Vertex credential file;
- moves `providers.contexts` enable/extract controls to `talent_overrides`;
- moves Rev.ai/Plaud validation state to `service_key_validation`.

The next Thinking maintenance task moves legacy provider install truth out of
`providers.bundled`. It promotes only artifacts that can be proven against the
current pins, writes provider-owned status and manifests, and then removes the
retired operational fields. Missing or mismatched proof exits successfully
without promotion and is repaired by the provider installer under the provider
lease. Unreadable proof exits successfully without promotion and is preserved
until the owner fixes the underlying access or I/O problem.

`solstone-core assets` emits an additive declarative registry of downloadable
artifacts. The installer pin tables remain the operational source in this wave,
and no fetch path reads the registry yet. Its rows resolve upstream, so no URL
an owner's machine fetches from changes.

There are no runtime compatibility shims for the retired shapes.
