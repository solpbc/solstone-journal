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

The native thinking / talent runtime resolves Generate requests from
`providers.active`. A missing profile is an
explicit no-brain state. Key presence and local readiness never choose a
provider implicitly.

Provider and model overrides are rejected in talent frontmatter, cortex
requests, batch requests, and direct generate calls. Thinking is the sole
configuration surface for the active brain. Talent `disabled`
controls are separate metadata under `talent_overrides`; they do not route
models. See [Output and Context Budgets](#output-and-context-budgets).

## Supported Owner Choices

The Thinking app exposes five setup choices:

- Bundled local, using Solstone's installed llama-server runtime.
- An owner-supplied OpenAI-compatible URL, model id, and optional bearer key.
- OpenAI with an owner-supplied API key and model id.
- Anthropic with an owner-supplied API key and model id.
- Google AI Studio with an owner-supplied API key and Gemini model id.

The direct cloud options are convenience presets. The arbitrary endpoint is a
plain compatibility contract: Solstone sends OpenAI-compatible requests, but
does not add vendor-specific support for whatever sits behind that URL.

The OpenAI preset always sends to `api.openai.com`. Any other service that
speaks the OpenAI API, including a cloud vendor's OpenAI-compatible endpoint,
belongs on the owner-supplied URL choice, which is also where the endpoint's
context window is handled.

Managed personal cloud keys remain journal-local:

- `env.OPENAI_API_KEY`
- `env.ANTHROPIC_API_KEY`
- `env.GOOGLE_API_KEY`

## Dispatch

Cloud (`google`, `openai`, `anthropic`) and `local` are the four dispatch
lanes. Single-shot generation is `solstone-core generate --one-shot`. There is no Python
provider registry.

### Personal cloud

Rust prepares the talent's prompt and source context, sends a bounded Generate
request, validates the completion and publishes through the talent's output
contract. The runtime emits usage and terminal events.

Key/model validation sends a bounded native generate probe through
`generate_client.generate_with_result`, so validation can incur a small
provider charge. Solstone still exposes only the three registry cloud choices;
there is no enterprise-provider configuration or credential path.

### Local and arbitrary endpoints

The local lane is a product-policy wrapper around the native runtime, not a
second general cloud adapter. It owns guarantees cloud providers do not
require:

- bundled runtime installation and manifest-backed readiness;
- context-budget fitting and local schema preparation;
- Qwen sampling and chat-template controls;
- cross-process local admission and bounded retry;
- content-free inference metadata in Generate responses;
- confidential egress/attestation gates;
- stable local error classification.

Bundled local posts to the supervisor-owned loopback server. Its install status
lives under `health/providers/`, while artifact truth lives in provider manifests
and the affirmative proof cache. A configured endpoint uses:

- `providers.local.endpoint_url`
- `providers.local.served_model_id`
- `providers.local.credential` (optional)
- `providers.local.parallel_slots` (optional)
- `providers.local.served_context_window` (optional; see
  [Output and Context Budgets](#output-and-context-budgets))

The configured logical provider remains `local`, so the same readiness and
safety boundary applies without maintaining vendor-specific adapters. Native
Generate owns endpoint requests and holds governed local admission during the
request.

Bundled and configured local endpoints can use different JSON grammar engines.
Shipped talent schemas therefore stay inside the measured regex subset shared
by the pinned llama-server and the supported endpoint engine. The ordinary test
suite checks the known incompatible shapes and schema semantics. Schema
preparation also omits `minLength` or `maxLength` at 2,000 and above because the
pinned llama-server rejects those repetition bounds; canonical response
validation still enforces the original bound. These checks do not compile either
provider grammar. For schema and preparation changes, run the
ignored `live_schema_compatibility` test once against each engine. That probe
first verifies the endpoint rejects a deliberately invalid pattern, then asks it
to admit every shipped prepared schema without recording response bodies.

## Output and Context Budgets

Every request reserves room for the model's reply. Each talent declares that
ceiling as `max_output_tokens`, and so does every describe category and every
other built-in caller: its largest output measured on the bundled model, times
1.5, rounded up to a multiple of 256. The key is required, so a talent without
one fails validation, and there is no default.

Nothing raises it. There is no owner setting and no request field for it, and
an old `talent_overrides[…].max_output_tokens` in `config/journal.json` is
ignored. A lane only ever lowers the ceiling to fit a window it knows, and adds
room for thinking on top of it (below).

### Thinking

Thinking is off unless the owner turns it on for their own model, and it only
ever adds room on top of a talent's ceiling; it never takes any away. The
Thinking app stores the choice as `providers.byo_thinking_budget`: `8192`,
`16384` or `32768` turns it on at that budget, and absent, `0` or any other
value is off. It applies to Generate calls on the OpenAI,
Anthropic and Google presets and on a configured endpoint. Bundled local and
confidential processing always run with thinking off (`enable_thinking: false`)
and never read it.

Off still sends each cloud provider its lowest setting, because a model that
reasons by default can spend a small talent's whole ceiling before it writes
anything. Which values a model accepts differs from model to model, so each
preset sends the first value below and moves to the next only when the provider
refuses that field:

| | off | on, at the chosen budget |
|---|---|---|
| OpenAI | `reasoning.effort` `none`, then `low`, then none; ceiling + 1,024 | `medium`, `high` or `xhigh`, stepping down one level if refused; ceiling + budget |
| Anthropic | `output_config.effort` `low`, then none; ceiling + 1,024 | adaptive thinking at `medium`, `high` or `xhigh`, or a fixed `budget_tokens` on a model without effort; ceiling + budget |
| Google | `thinkingBudget` `0`, then `128`, then `512`; ceiling + 1,024 | `thinkingBudget` equal to the budget (`24576` where a model caps it lower); ceiling + budget, capped at 65,535 in total |
| configured endpoint | nothing sent | nothing sent; ceiling + budget as room for a model that thinks on its own |

The 1,024 covers the reasoning a current cloud model still does at its lowest
setting, since that reasoning shares the ceiling.

A reply with no visible text is never a result. When it reaches its ceiling
with reasoning done, it fails as `thinking_consumed_budget`. A configured
endpoint's leading `<think>…</think>` block or separate `reasoning_content` is
never talent output: it is removed and counted as `reasoning_tokens`.

### Context window

A configured endpoint also has to fit `input + reply` inside the context window
it serves. The window comes from, in order:

1. `providers.local.served_context_window`, when it is at least 2,048;
2. the `max_model_len` that `GET /v1/models` reports for `served_model_id`;
3. otherwise 32,768.

The input is fitted to the window less the reply ceiling, holding back at most
a quarter of the window for the reply, and the ceiling is clamped to what
remains after the input. When too little room is left for a reply, the
request fails as `context_budget_exceeded` instead of being sent. An endpoint
smaller than the window assumed for it answers with a context refusal; a
generate call then refits its input against half the window, up to four times.

Bundled local fits against the window its own server was launched with, and
confidential processing against the service's own 262,144. Neither reads
`served_context_window`, which describes the owner's endpoint only.

## Local Admission

Bundled local and non-confidential arbitrary endpoints share the governed local
[admission boundary](../core/crates/solstone-core-local/src/admission.rs). Cloud
and confidential processing do not use this local slot pool. The
[tier contract](../core/crates/solstone-core-local/src/tier.rs) keeps capacity
intentionally small: one slot on the floor tier and two on the capable tier.
An arbitrary endpoint may instead set `parallel_slots` explicitly.

Admission uses per-slot `flock` files under
`health/local-inference-admission/`, coordinating independent journal
processes. Queue time consumes the caller's existing timeout. The local lane
holds admission during a Generate request and releases it on success or failure.

Generate responses carry inference timing, capacity, retry index, finish reason
and safe failure codes. Historical `health/local-inference/YYYYMMDD.jsonl`
records remain subject to operational-log retention; there is no current writer
for those files.

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
`health/brain.json`. When confidential processing is not the active thinking lane
but confidential transcription is on, the Thinking page's confidential status and
the Home health line also read `health/confidential-transcription.json`, which
transcription writes when it cannot verify the service and removes once it can.
Confidential SPP egress goes only over an RA-TLS channel
that passed attestation for that call (`confidential_generate` in
`core/crates/solstone-core-generate-wire/src/confidential.rs`).
The process-local result is kept in `AttestationStateStore`
(`core/crates/solstone-core-spp-ratls/src/state.rs`).

## Configuration and Install State

The active profile is explicit. The runtime does not infer it from retired
per-engine profiles or choose a provider because a key is present. Provider
install status and artifact manifests have separate owners under
`health/providers/`; selecting a profile does not establish installation or
readiness.

`solstone-core assets` emits an additive declarative registry of downloadable
artifacts. The installer pin tables remain the operational source for manifest
identity and cache layout; the four native Rust fetch call sites (llama-server,
parakeet-server, parakeet-model, and local-model) now resolve their download
URLs through this registry. The remaining catalog rows stay declarative until
their owners adopt the registry.

There are no runtime compatibility shims for the retired shapes.
