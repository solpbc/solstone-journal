# The generate contract

`generate` is one of the two contracts held by the thinking boundary; the other is
[`COGITATE.md`](COGITATE.md). This document defines `generate`: the record vocabulary every caller
uses to ask for a single model completion, the two framings that vocabulary travels in, and the
invariants the boundary guarantees.

**Read this for what the contract is.** [`PORTING.md`](PORTING.md) is how to port;
[`conversion/`](conversion/README.md) is the map of boundaries. This is the definition of one of them.

---

## Why the boundary exists

Every part of the system that needs a model reaches it through this contract. Behind it sit three
provider lanes — the local runtime, an owner's own provider key, and confidential hosted processing.
**Two of those three are egress**: content leaves the machine.

That is the reason the boundary is a boundary and not a function call. The decision about where a
given request egresses can only be made once, enforced once and audited once if there is exactly one
place it happens. Callers ask for a completion; they do not choose a provider, and they cannot.

⛔ **The request carries no provider and no model field, and never will.** Provider resolution happens
behind the boundary. A request type that can name a provider has given the egress decision back to
its callers, which is the failure this contract exists to prevent.

---

## The record vocabulary

Three record types, three schema identifiers:

| | identifier |
|---|---|
| request | `solstone-generate-request-v2` |
| response | `solstone-generate-response-v2` |
| protocol error | `solstone-generate-error-v2` |

The **contract fixture** `core/fixtures/generate_contract.json` is the single source for the schema
identifiers, every closed vocabulary, the exit-code table and the conformance vectors. Both the Rust
crate and the Python shim read that file. ⛔ **Neither language holds its own copy of any vocabulary.**

### Request

```json
{
  "schema": "solstone-generate-request-v2",
  "id": "f3a1",
  "context": "observe.depict",
  "contents": [
    {"type": "text",  "text": "Describe this image in detail."},
    {"type": "image", "mime_type": "image/png", "data": "<base64>"}
  ],
  "system_instruction": null,
  "temperature": 0.3,
  "max_output_tokens": 16384,
  "thinking_budget": null,
  "timeout_s": null,
  "json_output": false,
  "json_schema": null,
  "enforce_responsiveness": true,
  "attempt_index": 0,
  "exclusive_admission": false,
  "transport_retries": null
}
```

- **`id`** — optional in one-shot framing, **required** in session framing. Correlates a response with
  its request. Opaque to the boundary.
- **`context`** — required. The telemetry and routing context string, e.g. `observe.depict`. It is
  what usage is recorded against. It is **not** a provider selector.
- **`contents`** — non-empty array of parts. A `text` part carries `text`; an `image` part carries
  `mime_type` and base64 `data`. Unknown part types are refused.
- **`attempt_index`, `exclusive_admission`, `transport_retries`** — retry and admission hints. Each is
  meaningful to one lane and ignored by the others. ⚠ These are deliberately named for what they
  *do* rather than for the lane that honours them; a lane name in a field name is the provider leak
  arriving by the side door.

⛔ **Unknown request fields are refused, not ignored.** A caller sending a field the boundary does not
know is a caller that believes something false about the contract.

### Response — a tagged union

🔴 **`outcome` is required and closed. A response is exactly one of two things.**

**Generated:**

```json
{
  "schema": "solstone-generate-response-v2",
  "id": "f3a1",
  "outcome": "generated",
  "text": "A desk with two monitors…",
  "model": "…",
  "usage": {"input_tokens": 700, "output_tokens": 818, "total_tokens": 1518},
  "finish_reason": "stop",
  "thinking": null,
  "schema_validation": null,
  "input_budget": null,
  "request_budget": null,
  "inference": null
}
```

**Refused:**

```json
{
  "schema": "solstone-generate-response-v2",
  "id": "f3a1",
  "outcome": "refused",
  "reason": "attestation-stale",
  "reason_code": null,
  "retryable": false,
  "blocking": true,
  "reset_at_ms": null,
  "provider": null,
  "detail": "provider attestation is stale"
}
```

🔴 **Why a union and not a struct with optional fields.** A struct with an optional `text` and an
optional `error` lets a consumer test the wrong one. Testing an error *string* for truthiness rather
than presence is a defect this codebase has already paid for: an exception whose message is empty
reads as success, the absent text is then parsed as if it were text, and the failure surfaces
somewhere else entirely as a type error. **Under this contract that state cannot be constructed.**
There is no response carrying both a text and a reason, and none carrying neither.

⛔ **A refusal is a successful answer to a well-formed question.** It travels on stdout as a response
record, and the process exits `0`. It is not an error. See § exit codes.

### The three classifications on a refusal

`reason` is the boundary's own closed vocabulary — what kind of thing went wrong, derived from the
failure class. `reason_code` is the operational classification, and may be `null` when the failure
carries no operational code.

🔴 **`reason_code` is drawn from the provider-readiness taxonomy, and the contract names it because
five overlapping reason-code vocabularies exist in this tree** — three of them under the same
identifier, and two spelling the same concept in different cases:

| set | size | serves |
|---|---|---|
| `convey/provider_readiness` entries | **43** | ✅ **this contract**, owner-facing presentation, and the blocking decision |
| `think/providers/shared.RUNTIME_REASON_CODES` | 16 | generate-path error classification — a proper subset of the 43 |
| `think/providers/brain_state.RUNTIME_REASON_CODES` | 42 | local-runtime health records, **kebab-cased** |
| `think/brain_cli.RUNTIME_REASON_CODES` (an alias import) | 41 | command-line presentation |
| `think/brain_health.LOCAL_RUNTIME_REASON_CODES` | 8 | local health grouping |

⛔ **Wiring the 16 into a caller loses both decisions this contract exists to deliver**: 24 of the 43
are blocking and only 4 of those are in the 16, and the sole non-retryable code — `non_responsive` —
is in the 43 and not in the 16. The fixture carries the set this contract uses, so no caller has to
choose.

🔴 **`retryable` and `blocking` are computed by the boundary and carried as answers, not left for each
consumer to derive.**

| field | means |
|---|---|
| `retryable` | retrying this same request could produce a different outcome |
| `blocking` | the provider is not merely failing, it is unusable — **the caller must preserve the owner's source material rather than record a failed attempt** |

⚠ **This is the field that decides whether an owner's raw media survives.** Leaving each consumer to
map reason codes to that decision puts one contract in as many places as there are consumers, and the
consequence of any one of them getting it wrong is deleted owner data. The boundary knows what the
code means; it publishes the decision. Consumers still receive `reason_code` for recording and
presentation.

### Protocol error

Reserved for the boundary being unable to parse the question at all.

```json
{"schema": "solstone-generate-error-v2", "id": null, "reason": "malformed-request", "detail": "…"}
```

Written to **stderr**, with a non-zero exit code. ⛔ Never to stdout — stdout is the protocol channel.

---

## The two framings

**The record vocabulary is one thing. It travels in two framings, and a consumer picks by need.**

### One-shot

One JSON request on stdin, one JSON response on stdout, process exits. `id` optional. This is the
right framing for a caller that makes a single completion — which is nearly all of them.

### Session

Newline-delimited JSON in both directions over a long-lived child process. `id` **required**. Requests
may be written while earlier responses are outstanding; responses may arrive in any order. Closing
stdin drains the outstanding requests and exits `0`.

Session framing exists for callers that make many completions for one unit of work — a screen
recording's qualified frames, a day's rollups. Under one-shot framing those are one operating-system
process per completion.

🔴 **One child per consumer process, never a shared daemon.** The child is launched by the consumer,
lives as long as the consumer, and dies with it. The pipeline's failure containment — one process per
input file, so a crash, a leak or a watchdog kill reaches exactly one file — is a property worth more
than the process savings a shared service would buy. ⛔ A long-lived shared generate service would put
every handler behind one fate and is out of scope for this contract.

### Framing hazard

⚠ **stdout is the protocol channel in both framings.** One stray print, traceback, unflushed write or
provider log line on stdout corrupts the stream — in one-shot framing that costs one call, in session
framing it costs every outstanding call and every later one. All diagnostics go to **stderr**. This is
the standard hazard of every newline-framed stdio protocol and it has exactly one mitigation.

---

## Exit codes

| code | meaning |
|---|---|
| `0` | a response record was written to stdout — **`generated` or `refused`**; in session framing, the stream ended cleanly |
| `64` | the request could not be parsed; an error record is on stderr |
| `70` | the boundary itself failed; an error record is on stderr |

🔴 **`69` is deliberately absent from this table, and the absence is load-bearing.** The media
pipeline reads exit `69` **from a handler** as *hold the owner's raw media and try again later*. A
boundary that also exits `69` puts two opposite meanings on one number in two processes one pipe
apart, and the consumer's disambiguation then depends on matching a schema string and a reason string
as well as the code. Making every ordinary outcome — including a refusal — exit `0` removes the
collision instead of documenting it.

⛔ **Nothing in the caller's exit-code namespace is decided here.** A caller translates a refusal into
whatever its own dispatcher expects; the `blocking` field is what that translation reads.

---

## The handshake

`solstone-generate-wire --contract` writes the contract fixture to stdout and exits `0`. A consumer
can discover the schema identifiers, the closed vocabularies and the exit-code table rather than
assume them.

⚠ **A consumer receiving an unrecognised `reason` or `reason_code` maps it to the declared unknown
member rather than failing.** Reader tolerance over reader strictness: an older consumer meeting a
newer boundary degrades to a less specific answer; it does not refuse a response it could have acted
on. Unrecognised **request** fields are still refused — tolerance applies to reading, never to
accepting.

---

## Versioning

- The **schema identifier's `-vN`** changes only on a break: a removed field, a narrowed type, a
  changed meaning.
- 📌 **`-v1` was a one-record predecessor with no adopters and is removed rather than redefined.** It
  had no `id`, no `outcome` tag, no reason code and no session framing, and it exited `69` for a
  no-engine refusal. Redefining a published identifier in place would have meant two shapes answering
  to one name across the tree's own history for no gain, since nothing had adopted it.
- The **fixture's `fixture_version`** changes on any additive change: a new refusal reason, a new
  runtime reason code, a new optional field.
- Because every closed vocabulary lives in the fixture rather than in either language's source,
  **adding a member is additive**. Under the previous arrangement it was a new constant in every
  implementation.

---

## Invariants

These are guarantees of the boundary, each backed by a test.

1. **No provider or model selection crosses in the request.** Asserted structurally.
2. 🔴 **No downgrade path.** When the resolved lane is `confidential` and its attestation is not
   verified, failed or stale, the boundary produces a **refusal** — and cannot produce a `generated`
   outcome. Asserted for every combination of lane and attestation state, not inferred from the
   absence of a fallback branch. ⚠ A guarantee that holds because a branch is missing is a guarantee
   that a tidy-up can remove with nothing going red.
3. **Every guard runs before egress.** The no-engine guard, the confidential-attestation guard, schema
   preparation, strict result validation and responsiveness classification all execute for every
   request, in both framings.
4. **The boundary writes no owner content.** It touches operational ledgers only — the token log, the
   provider health and cache records — and reads configuration. ⛔ The caller is the only writer of
   the owner's journal content, and the caller does not write the boundary's operational paths. An
   invariant enforced on one side is a coincidence.
5. **Usage reaches the token log for every completion.** ⛔ The forbidden shortcut is suppressing
   usage logging to simplify invariant 4; that silently empties the cost ledger for every future
   caller, and the logger swallows its own exceptions, so nothing would ever error.
6. **A refusal is never a partial success.** No response carries both a text and a reason.

---

## What this contract is not

- ⛔ **Not `cogitate`.** The tool-using, multi-turn talent runtime is a separate contract in the same
  boundary, with its own event vocabulary and its own runtime preamble. See [`COGITATE.md`](COGITATE.md).
- ⛔ **Not provider selection policy, budgets, or fallback behaviour.** Those live behind the boundary
  and are invisible to every caller by design.
- ⛔ **Not a streaming-token interface.** `generate` returns a complete completion. Incremental
  delivery to an owner-facing surface is a different contract.
