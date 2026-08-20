# Sol-Initiated Chat Phase 1 Design

> Retired 2026-08-20. Sol-initiated chat was designed here and later removed entirely. This document is the snapshot of that design. It does not describe a current capability.
> Partial. `sol` `chat.start` exists; there is no Rust `/api/chat/start` handler. Ignore Python paths.


## Scope
This change adds the producer-side primitive for sol-initiated chat starts:
`solstone call chat start`, four new chat-stream event kinds, prompt-context support
for the new trigger, and a six-step policy gauntlet. It does not build the UI
or any producer intelligence that decides when to call the primitive.

The LOCKED identifiers in the locked contract are source of truth. Field order
for new chat-stream kinds must be preserved exactly.

## Event Contract
Extend `solstone/convey/chat_stream.py` `_VALID_KINDS` with:

- `sol_chat_request`: `request_id`, `summary`, `message`, `category`, `dedupe`, `dedupe_window`, `since_ts`, `trigger_talent`
- `sol_chat_request_superseded`: `request_id`, `replaced_by`
- `owner_chat_open`: `request_id`, `surface`
- `owner_chat_dismissed`: `request_id`, `surface`, `reason`

Extend `_TRIGGER_KINDS` with only `sol_chat_request`. The other three new
kinds are stream facts, not chat-generate triggers.

`reason` on `owner_chat_dismissed` is required by key presence only. Callers may
pass `None` or an empty string.

## Module Layout
Create `solstone/convey/sol_initiated/` with the smallest split that keeps the
policy auditable:

- `__init__.py`: re-export `start_chat`, `StartChatResult`, `record_owner_chat_open`, `record_owner_chat_dismissed`, and `CATEGORIES`.
- `copy.py`: single home for spec literals: event kinds, throttle reasons, trigger label, categories, and `SOLSTONE_SOL_CHAT_REQUEST`.
- `settings.py`: typed settings loader with code-level defaults and one WARN per rejected field.
- `dedup.py`: `parse_dedupe_window`, `_is_live_for_dedup`, and `_is_unresolved_for_supersede`.
- `policy.py`: the six gauntlet checks, in order, as small pure-ish functions over loaded settings and today's stream events.
- `nudge_log.py`: sol-initiated nudge-log row writer that reuses `solstone.think.push.triggers._append_nudge_log`.
- `events.py`: `record_owner_chat_open` and `record_owner_chat_dismissed` wrappers around `append_chat_event`.
- `start.py`: `StartChatResult`, request-id minting, validation, policy orchestration, supersede, and the locked critical section.

`nudge_log.py` and `events.py` are small but worth keeping because they are
public integration points for later changes and isolate cross-package details.

## Constants Discipline
All new spec literals live in `solstone/convey/sol_initiated/copy.py`.

The locking-discipline grep test should exempt only:

- `solstone/convey/sol_initiated/copy.py`
- the `_VALID_KINDS` and `_TRIGGER_KINDS` blocks in `solstone/convey/chat_stream.py`
- the test file that enforces this discipline

`SOLSTONE_SOL_CHAT_REQUEST` is a guard literal only. No runtime environment
variable by that name should exist in this change, and no settings override should
read from the environment. The grep target exists to catch accidental leakage
or ad hoc env-driven bypasses.

## Categories

Use the LOCKED 6-tuple from the locked contract:

```python
CATEGORIES = ("briefing", "pattern", "commitment", "error", "arrival", "notice")
```

Order is contract; preserve as a tuple, not a set, so iteration order is
stable. Validator rejects any value outside this set with a specific error.

## Request IDs
Use `secrets.token_hex(16)` for `request_id`.

`ulid-py` is not present in `pyproject.toml` or `uv.lock`, and sortable IDs are
not needed because stream order and `ts` already provide ordering. A 32-char
random hex ID avoids a new dependency and is sufficient for cross-event joins.

## Settings
Store settings in the top-level `sol_voice` block of `journal/config/journal.json`,
with defaults shipped in `core/fixtures/journal_default.json`.

Fields:

- `daily_cap`: default `5`
- `category_caps`: `CATEGORY_CAP_DEFAULTS` — `briefing=3`, `pattern=2`, `commitment=2`, `error=2`, `arrival=3`, `notice=2`
- `rate_floor_minutes`: default `20`
- `mute_window.enabled`: default `false`
- `mute_window.start_hour_local`: default `22`
- `mute_window.end_hour_local`: default `7`
- `category_self_mute_hours`: default `24`
- `category_self_mute_clear_marker_ts`: default `0`
- `default_dedupe_window`: default `"24h"`

The loader must apply in-code defaults for missing fields because `get_config()`
does not merge defaults once `journal.json` exists. On a single-field validation
failure, emit one WARN with `key=<name>` and `value=<rejected>`, use that field's
default, and continue. Do not raise for malformed settings.

Mute-window hours are local owner time using the existing owner-timezone helper.
Caps are counted by UTC day as required by the contract.

## Start Chat Flow
`start_chat` validates boundary inputs first:

- `summary` required, trimmed, at most 80 chars
- `message` optional, trimmed when present, at most 500 chars
- `category` must be one of `CATEGORIES`
- `dedupe` required, non-empty
- `dedupe_window` parsed from the CLI value or settings default
- `since_ts` must be a positive int ms epoch and not later than current `now_ms()`
- `trigger_talent` required slug-like talent name

`StartChatResult` has `written: bool`, `deduped: bool`, `throttled: str | None`,
and `request_id: str | None`.

Policy order is fixed:

1. mute window
2. rate floor
3. self-mute
4. per-category cap
5. daily cap
6. dedup

For throttle hits, return `written=False`, `deduped=False`, `throttled=<reason>`,
`request_id=None`, and write `outcome="throttled:<reason>"` to nudge log.

For dedup hits, return `written=False`, `deduped=True`, `throttled=None`,
`request_id=None`, and write `outcome="deduped"`.

For success, mint `request_id`, append any supersede event first, append the
`sol_chat_request` second, return `written=True`, `deduped=False`,
`throttled=None`, and write `outcome="written"`.

## Locking And Append Mechanics

All six policy checks and both possible stream appends must run under one
`_CHAT_LOCK` acquisition.

The current `append_chat_event` cannot be called while holding `_CHAT_LOCK`
because it also acquires that non-reentrant lock. Add a small stream-layer batch
helper in `solstone/convey/chat_stream.py` that:

- validates all event kinds and required fields
- appends one or two stored events while the caller holds `_CHAT_LOCK`
- writes the affected `chat.jsonl` files with the existing atomic replace logic
- returns stored events and touched paths
- lets indexing and broadcast happen after disk writes have completed

Keep the existing single-event `append_chat_event` behavior by routing it through
the same helper. Broadcast remains after disk append. Indexing remains outside
the critical section where possible.

## Dedup And Supersede Readers

Keep these as distinct named functions in `dedup.py`.

`_is_live_for_dedup(dedupe_key, dedupe_window)` walks today's stream looking for
a `sol_chat_request` whose `dedupe` matches `dedupe_key`, whose window has not
expired, and whose `request_id` is still pending owner engagement.

Engagement releases dedupe: `owner_chat_open` releases the matching
`request_id`, and `owner_message` releases pending requests older than the owner
message timestamp (strict `<`). Dismissal is not a dedupe release event.

`_is_unresolved_for_supersede() -> request_id | None` walks today's stream and
returns the most recent `sol_chat_request` not yet followed by `sol_message` and
not already replaced by `sol_chat_request_superseded`.

Do not reuse the dedup reader for supersede. The two readers answer different
questions.

## Self-Mute

Self-mute scans recent `owner_chat_dismissed` events, resolves each dismissal's
`request_id` to the original `sol_chat_request.category`, ignores dismissals at
or before `category_self_mute_clear_marker_ts`, and throttles the category when a
matching dismissal is inside `category_self_mute_hours`.

Throttle reason is exactly `category-self-mute`.

## Nudge Log

Every `start_chat` call appends one row to `push/nudge_log.jsonl` via the existing
`_append_nudge_log` helper.

Row fields:

- `ts`: int seconds
- `kind`: `sol_chat_request`
- `dedupe_key`: the raw `dedupe` string
- `category`: category string
- `outcome`: `written`, `deduped`, or `throttled:<reason>`

Add a docstring at the new append site documenting that legacy rows written by
`_record_send` do not have `kind`, and no migration is performed.

## CLI

Add `solstone/apps/chat/call.py`. It auto-mounts as `solstone call chat` through the
existing app discovery path.

Command:

- `start`
- `--summary`
- optional `--message`
- `--category`
- `--dedupe`
- optional `--dedupe-window`, defaulting through settings when omitted
- `--since-ts`
- `--trigger-talent`

The command calls `solstone.convey.sol_initiated.start_chat` and prints the
`StartChatResult` as JSON. Validation errors print to stderr and exit `1`.

Use command name `start`, not a read verb, to avoid layer-hygiene false positives.

## Chat Context

`chat_context.py` keeps `_normalize_trigger(context) -> (kind, payload)`.

For stream kind `sol_chat_request`, `chat.py` passes trigger type
`sol_chat_request`; `_normalize_trigger` returns that kind and the payload.

Add template vars:

- `trigger_kind`
- `summary`
- `message`
- `category`
- `since_ts`
- `trigger_talent`

For `sol_chat_request`, set `trigger_kind` to the prompt-facing label
`sol_initiated`. For existing trigger types, set:

- `owner_message` -> `owner_message`
- `talent_finished` -> `talent_finished`
- `talent_errored` -> `talent_errored`
- `synthetic-max-active` -> `synthetic`

`_render_trigger_context` gets a `sol_chat_request` branch that renders summary,
message when present, category, since_ts, and trigger_talent. `summary` and
`message` stay bounded by `start_chat` validation.

## since_ts Propagation

Path:

1. CLI receives `--since-ts <int_ms>` as Typer `int`.
2. `start_chat` validates positive int and `<= now_ms()`.
3. Event payload stores `since_ts` as an int through `append_chat_event`.
4. JSONL round-trips it as a number, not a string.
5. `_trigger_from_stream_event` in `chat.py` copies it into the trigger payload.
6. `_normalize_trigger` extracts it and sets the template var as an int.
7. Prompt context renders it; downstream presentation is the prompt's responsibility.

## Chat Runtime Wiring

Update `_trigger_from_stream_event` in `solstone/convey/chat.py` with a
`sol_chat_request` branch returning the stream payload as trigger data:
`type`, `summary`, `message`, `category`, `since_ts`, `trigger_talent`, and
`request_id`.

`_recover_chat_if_needed` already activates from `find_unresponded_trigger`, so
once `_TRIGGER_KINDS` includes `sol_chat_request`, singleton-serial stays intact.
No second callosum consumer is needed.

## Chat Formatter

Update `solstone/think/chat_formatter.py` for new kinds.

Index `sol_chat_request` as `[sol] <summary>`, appending the message on a new
line when `message` is present. Skip indexing for `sol_chat_request_superseded`,
`owner_chat_open`, and `owner_chat_dismissed` by producing no chunk content for
those events.

`reflection_ready` is already a valid chat-stream kind but currently raises in
the formatter. That is an existing bug and out of scope here unless a
test in this change exposes chunk deletion from mixed files.

## Tests

Add or extend:

- `tests/test_chat_stream_sol_initiated.py`: kind validation, trigger walk-back, supersede atomicity, two-reader correctness, broadcast-after-disk-append for each new kind.
- `tests/test_sol_initiated_policy.py`: every gauntlet branch, malformed-config WARN, settings defaults, UTC cap boundaries, local mute-window boundaries.
- `tests/test_sol_initiated_dedup.py`: dedupe-window parsing, live-for-dedup logic, dismissal releases dedupe, expired windows.
- `tests/test_sol_initiated_nudge_log.py`: written/deduped/throttled row shapes and legacy-row tolerance.
- `solstone/apps/chat/tests/test_call.py`: Typer success, deduped, every throttle reason, and validation errors.
- `tests/test_chat_context_sol_initiated.py`: template vars and trigger context for sol-initiated; existing trigger labels unchanged.
- `tests/test_convey_chat_sol_initiated.py`: `_recover_chat_if_needed` wakes chat from `sol_chat_request`; singleton-serial preserved when owner and sol triggers coexist.
- Locking-discipline grep test: forbidden literals outside the constants file, chat-stream kind blocks, and the test itself.
- `tests/test_chat_stream_indexing.py` or a new formatter test: new request kind does not delete existing indexed chat chunks.

## Implementation Sequence

Constants/settings, stream kinds and locked append helper,
dedup/supersede readers, policy, nudge-log writer, `start_chat`, CLI, chat
runtime mapping, chat-context rendering, chat formatter, then focused and
broader tests.

## Non-Goals

No observer code, APNs delivery changes, UI templates, second callosum consumer,
producer intelligence, smart policy beyond the six ordered gates, or migration
of legacy `nudge_log.jsonl` rows.
