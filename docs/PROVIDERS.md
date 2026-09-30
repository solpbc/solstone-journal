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

The native thinking / talent runtime is the only resolver.
The `generate` and `cogitate` arguments identify the interface being invoked,
but both resolve the same `providers.active` profile. A missing profile is an
explicit no-brain state. Key presence and local readiness never choose a
provider implicitly.

Provider and model overrides are rejected in talent frontmatter, cortex
requests, batch requests, and direct generate calls. Thinking is the sole
configuration surface for the active brain. Talent `disabled`, `extract` and
`max_output_tokens` controls are separate metadata under `talent_overrides`;
they do not route models. See [Output and Context Budgets](#output-and-context-budgets).

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
lanes. Cogitate runs as `solstone-core cogitate --one-shot`. Single-shot
generation is `solstone-core generate --one-shot`. There is no Python
provider registry.

### Personal cloud

The native cogitate runtime serializes the prepared talent configuration,
composes the system instruction, applies the talent contract and command
policy, performs the tool loop, and emits usage and terminal events.

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
- `providers.local.served_context_window` (optional; see
  [Output and Context Budgets](#output-and-context-budgets))

The configured logical provider remains `local`, so the same readiness and
safety boundary applies without maintaining vendor-specific adapters. Native generate owns endpoint requests; the local lane adds governed admission
around native cogitate execution.

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

Every request reserves room for the model's reply. A talent declares that
ceiling as `max_output_tokens` in its frontmatter, and a generate talent that
declares none asks for 49,152 tokens.

The right ceiling depends on what serves the model, so an owner can set it per
talent in `config/journal.json`:

```json
{
  "talent_overrides": {
    "talent.system.speaker_attribution": { "max_output_tokens": 8192 },
    "talent.entities.entity_describe": { "max_output_tokens": 4096 }
  }
}
```

- A journal talent's key is `talent.system.<name>` and an app talent's is
  `talent.<app>.<name>`, where `<name>` is the talent's file name without
  `.md`. `journal talent list` shows every talent.
- Only a positive integer applies. Zero, a negative number or a non-number is
  ignored, and the talent keeps its own ceiling.
- The override applies to generate and cogitate talents alike. On Linux with
  bundled local, `screen` splits an oversized input into batches and sizes
  each batch's reply itself.

The OpenAI, Anthropic and Google presets send the ceiling as given, except
that Anthropic and Google widen it to make room for a thinking budget and
Google caps the total at 65,535.

A configured endpoint also has to fit `input + reply` inside the context window
it serves. The window comes from, in order:

1. `providers.local.served_context_window`, when it is at least 2,048;
2. the `max_model_len` that `GET /v1/models` reports for `served_model_id`;
3. nowhere: the window is unknown, and the reply ceiling is capped at 8,192.

With a known window, the input is fitted to the window less the reply
ceiling, holding back at most a quarter of the window for the reply, and the
ceiling is clamped to what remains after the input. When too little
room is left for a reply, the request fails as `context_budget_exceeded`
instead of being sent. If an endpoint turns away the ceiling it is asked for,
lower that talent's `max_output_tokens` or set `served_context_window`.

`served_context_window` is one setting for the whole `local` lane: bundled
local's cogitate turns and confidential processing read it too, and it stays
set when the endpoint or model changes. Update or remove it when either
changes.

## Local Admission

Bundled local and non-confidential arbitrary endpoints share the governed local
[admission boundary](../core/crates/solstone-core-local/src/admission.rs). Cloud
and confidential processing do not use this local slot pool. The
[tier contract](../core/crates/solstone-core-local/src/tier.rs) keeps capacity
intentionally small: one slot on the floor tier and two on the capable tier.
An arbitrary endpoint may instead set `parallel_slots` explicitly.

Admission uses per-slot `flock` files under
`health/local-inference-admission/`, coordinating independent journal
processes. Queue time consumes the caller's existing timeout. The local wrapper
holds admission around the native cogitate subprocess.

Bundled cogitate attempts append content-free telemetry to
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
`health/brain.json`. When confidential processing is not the active thinking lane
but confidential transcription is on, the Thinking page's confidential status and
the Home health line also read `health/confidential-transcription.json`, which
transcription writes when it cannot verify the service and removes once it can.
Confidential SPP egress goes only over an RA-TLS channel
that passed attestation for that call (`confidential_generate` and
`confidential_converse` in `core/crates/solstone-core-generate-wire/src/confidential.rs`).
The process-local result is kept in `AttestationStateStore`
(`core/crates/solstone-core-spp-ratls/src/state.rs`).

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
artifacts. The installer pin tables remain the operational source for manifest
identity and cache layout; the four native Rust fetch call sites (llama-server,
parakeet-server, parakeet-model, and local-model) now resolve their download
URLs through this registry. The remaining catalog rows stay declarative until
their owners adopt the registry.

There are no runtime compatibility shims for the retired shapes.
