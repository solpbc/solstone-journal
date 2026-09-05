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
identifiers, every closed vocabulary, the exit-code table and the conformance vectors. The Rust crate
reads that file. ⛔ **No implementation holds its own copy of any vocabulary.**

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
  🔴 **Owner media travels inline, over the pipe, and is never written to a temp file.** The obvious
  alternative — hand the child a path — is what the speaker-analysis boundary does for bulk audio, and
  it puts an owner's frames at rest outside the journal, outside retention, with a cleanup step that a
  killed child never runs. ⛔ Inline is the covenant-correct choice and it is not negotiable for
  convenience: **content crossing this boundary is in flight or it is nowhere.**
  ⚠ It has a cost, and it lands on the caller: a screen recording's frames are megabytes each, so **a
  client must write requests and read responses concurrently.** A client that writes a large request
  while the child is blocked writing a response nobody is reading deadlocks both. See § the two
  framings.
- **`attempt_index`, `exclusive_admission`, `transport_retries`** — retry and admission **hints**. ⚠ They
  are deliberately named for what they *do* rather than for the lane that honours them; a lane name in
  a field name is the provider leak arriving by the side door.
  🔴 **A hint is the one category of field a lane may decline to apply, and the response says so.** Each
  is meaningful to one lane and meaningless to the others — and a caller **cannot see which lane it
  resolved to**, by design. So refusing a request because its lane cannot honour a hint would punish a
  caller for a fact the contract deliberately hides from it: the talent path sets an admission hint on
  every local retry, and it would start being refused whenever the journal happens to resolve to a
  cloud provider.
  ✅ **The resolution is neither refuse nor drop: a generated response reports `hints_applied`**, the
  subset that took effect. Nothing is silently ignored, because the answer carries what happened, and
  nothing is refused for asking. ⛔ This exemption is for **hints only** — every other declared field is
  honoured or the request is refused.
  ⚠ **Today one lane accepts both admission hints and discards them**: the non-bundled local path pops
  them and then takes a branch that never reads them. That is exactly the state `hints_applied` makes
  visible instead of invisible.

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
  "inference": null,
  "hints_applied": ["attempt_index"]
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

🔴 **`reason_code` is drawn from this contract's own taxonomy.** The fixture
`core/fixtures/generate_contract.json` is the only generate-path list. A caller
does not map codes from any other set.

Two other reason-code lists live in this tree. They are different domains. Do not
unify them with this contract:

| set | size | case | serves |
|---|---|---|---|
| this fixture (`reason_codes`) | **46** | snake | ✅ **this contract**: generate refusals, with `retryable` and `blocking` on every row |
| `KNOWN_REASON_CODES` (provider runtime) | 43 | kebab | local-provider process health |
| `DETERMINISTIC_FAILURE_REASON_CODES` (cogitate) | 10 | snake | talent failures that have reached a known terminal class — a named subset, not a second generate list |

Three names overlap after case-fold (`gpu_probe_failed`, `gpu_unavailable`,
`ram_insufficient`). That is coincidence of concept, not a shared vocabulary.

⚠ **Two declared reasons are reachable only through the raising entry points, not through this
boundary**, and a caller reads the evidence instead:

| reason | what a one-shot caller reads instead |
|---|---|
| `schema-validation-failed` | `schema_validation.valid` on the **generated** response |
| `incomplete-text` | `finish_reason` on the **generated** response |

🔴 **So a completion the provider cut off arrives as `generated`, not as a refusal**, and it is the
caller's job to notice. That is deliberate — the result-returning path exists so a caller decides for
itself — but it means **`finish_reason` is load-bearing, not decorative**. ⚠ And it is *normalised* on
the way through: an endpoint saying `length` reaches the caller as `max_tokens`, so a consumer matching
the provider's spelling sees nothing wrong.

⛔ **Do not map `reason_code` from the kebab process-health list.** That list does
not carry `retryable` or `blocking`. The fixture carries both answers, so no
caller has to choose.

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

### 🔴 `blocking` governs, and `retryable` is read only when `blocking` is false

**A refusal can carry both.** When it does, the caller stops and preserves the owner's source material.
It does not retry, whatever `retryable` says.

⚠ **This has to be stated because the two fields never disambiguate each other by content.** Of the
reason codes the fixture carries, **27 are blocking and every one of them is also retryable**; the sole
non-retryable code is not blocking. A consumer that reads `retryable` first and a consumer that reads
`blocking` first therefore both produce a defensible-looking answer on the same record, and nothing in
the pair of booleans tells either one it is wrong.

🔴 **What "stop" means here, precisely.** The caller ends *this* attempt and holds the owner's
material. ⛔ It does not discard it, and ⛔ it does not mean the work is abandoned — a later
run picks the held material up. That is the whole distinction `blocking` carries: *preserve rather than
record a failed attempt*. Reading it as "never try again" throws away the same data that reading it as
"retry now" burns the provider for.

⚠ **The failure mode is invisible on the codes most likely to be tested.** For a failure that
persists, both orderings reach the hold once the retry budget is spent, so an assertion on the final
outcome or exit code passes either way. Only the **transient** blocking codes separate them —
`attestation_stale`, `local_model_loading`, `provider_quota_exceeded`, `install_busy` — and there
the wrong order has a consumer calling repeatedly through a provider the boundary has just declared
unusable, which is the inverse of what `blocking` exists to instruct.

✅ **The unknown member already encodes this rule and is the one place the combination is visible.**
An unrecognised or absent `reason_code` resolves to `retryable: false, blocking: true` — blocking
wins, stated as data. The rule above generalises what that default already assumes.

⛔ **The resolution is a rule about reading two true facts, not a contradiction in the facts, so do
not "fix" it by marking blocking codes non-retryable.** `local_model_loading` genuinely *is* retryable
later; collapsing it would destroy the information a subsequent run needs, and the retry classification
is consumed elsewhere on its own terms.


🔴 **The readiness taxonomy alone does not deliver this, and the gap is on the egress lane.** The three
attestation failures — not-verified, failed, stale — carry reason codes that are **absent from that
taxonomy entirely**, so a lookup returns not-found and the blocking predicate answers `false`. That is
the one failure class meaning *the confidential processing environment could not be verified*, and the
taxonomy classifies it as an ordinary bad response, while a missing provider key classifies as
blocking.

⛔ **This contract classifies the attestation family as `blocking: true`.** An unverifiable
confidential environment is *the provider is unusable, preserve the owner's source material* — the same
shape as a missing key and strictly more serious. The fixture carries that classification explicitly
rather than deriving it from a lookup that would answer `false`.

🔴 **An unknown or absent `reason_code` resolves to `retryable: false, blocking: true`.** The safe
direction is the one that preserves the owner's material: holding is recoverable and consuming is not.
⚠ This is the opposite of what a not-found lookup returns today, and it is deliberate — a code the
boundary does not recognise is a failure it does not understand, and a failure it does not understand
must not license discarding an owner's source.

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

**Entering it, and bounding it.** ⛔ **The framing is not auto-detected** — one-shot reads stdin to
end-of-file, which is structurally incompatible with a session that keeps stdin open, so a child that
guessed wrong would hang rather than error. The caller declares the framing, concurrency bound, and
any optional framing values on the command line when it launches the child. The declared vocabulary
lives in the contract fixture alongside everything else. An explicit Journal root is one such value:
it travels in the child argv and binds that session without changing process environment.

**Two failure classes exist only in this framing, and both carry the same classifications a refusal
does.** A **desynchronised stream** — a non-record line on stdout, or a well-formed record bearing an
unknown or already-retired `id` — is terminal for the session: every outstanding request completes as
a failure and no further request is accepted. A **child that dies** fails every outstanding request.
🔴 **Both carry `retryable: false, blocking: true`**, for the same reason an unknown reason code does:
the caller has lost the ability to know what happened to work it submitted, and that must not license
discarding the owner's source material.

⚠ **The caller must drain the child's stderr continuously.** Diagnostics go to stderr by design, the
child outlives a single request, and an undrained pipe fills — at which point the child blocks writing
a log line and every outstanding request stalls behind it. In one-shot framing this cannot happen; in
session framing it is a hang wearing a performance costume.

⚠ **Request fields are honoured or refused, never accepted and dropped** — with the single, named
exception of the three **hints**, which a lane may decline to apply and which the response reports back
in `hints_applied`. See § Request. Silently ignoring a declared field is a contract that lies, which is
worse than one that says no; reporting what was applied is better than either.

### 🔴 Closing a session, and the two ways stdin ends

**A caller closes a session by writing a terminal record, then closing stdin.** On seeing it the child
drains its outstanding requests, answers them, and exits `0`.

⛔ **Bare end-of-file without that record means the caller is gone, and the child aborts.** It answers
nothing further, writes no further usage, and exits.

📌 **This exists because the two cases are otherwise indistinguishable.** When a caller is killed, the
kernel closes its write end exactly as a deliberate close does — so a child with only one shutdown
signal must guess between *finish the work* and *your owner is dead*, and both guesses are wrong half
the time. Draining for a dead owner means completions nobody receives, usage logged against work
nobody asked for, and a child outliving the process containment it was supposed to inherit.

⚠ **The obvious alternatives do not work here.** A process group does not die when its leader dies —
something must send the signal, and a killed caller cannot. A parent-death signal is Linux-only and
this tree builds for Apple targets. A supervising reaper is a third process with the same problem one
level up. **One JSON line settles it portably.**

✅ **The guarantee holds today.** `criterion_8_killing_session_owner_aborts_wire_without_usage` in
`core/crates/solstone-core/tests/generate_session.rs` is the test that holds it. The registered
`solstone-core::core_brain_contracts` integration target includes that module and runs by default in
`make ci-full`.

📌 **It did not always, and the reason is worth keeping — it is a constraint on any future
implementation of this lane, not a closed ticket.** The retired Python shim failed this guarantee, and
the failure was measured rather than suspected: on bare end-of-file it set its abort flag and cancelled
the in-flight request, but the cancellation could not reach the work. The bundled local lane ran the
provider call on a worker thread wrapping a blocking subprocess call that took no timeout of its own,
so interpreter shutdown joined that thread. The child exited only when the provider call returned by
itself — 30.1 s for a request carrying `timeout_s: 30`, and 120 s for a caller that sent none — and the
helper process it spawned lived exactly as long. Two processes held a provider slot for that window
after their caller was already gone.

⚠ **That was a regression introduced by a cutover, which is the part that generalises.** Before the
bundled lane moved to the native `local generate` verb the same request aborted **0.2 s** after
end-of-file; after it, 30.2 s. The cutover replaced an awaited in-process call with a worker thread
wrapping a blocking subprocess call, and an abort cannot reach work parked there. 🔴 **Whatever
implements this lane must keep the provider call cancellable, or exit outright** — which the contract
permits, because nothing further is to be answered anyway.

✅ **The usage half held even while the abort half did not.** A request whose provider answered after
the caller died wrote no token-log line, against a control that writes one for an ordinary completion.
Nothing is recorded against work nobody will receive.

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

`solstone-core generate --contract` writes the contract fixture to stdout and exits `0`. A consumer
can discover the schema identifiers, the closed vocabularies and the exit-code table rather than
assume them.

⚠ **A consumer receiving an unrecognised `reason` or `reason_code` maps it to the declared unknown
member rather than failing.** Reader tolerance over reader strictness: an older consumer meeting a
newer boundary degrades to a less specific answer; it does not refuse a response it could have acted
on. ⚠ **And it degrades to the safe classification** — an unrecognised member arrives with
`retryable: false, blocking: true`, so tolerance never costs the owner's source material.

⛔ **Tolerance covers vocabulary members. It does not cover the schema identifier.** A record whose
`schema` is not this contract's is refused as a protocol error, not read leniently — the identifier is
what tells a reader which shape it is holding, and reading it tolerantly is how a predecessor's records
survive a migration that was supposed to end them.

⛔ **And unrecognised request fields are refused.** Tolerance applies to reading, never to accepting.

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
   usage logging to simplify invariant 4; that silently empties the usage ledger for every future
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
