# plates — the boundaries strands connect to

**A plate is a boundary of the journal where strands connect from other plates.** ⛔ It has no "sides" — the contract lives at one *end of a strand*, and by convention that is the second plate named in `S:a:b`.

Definitions and vocabulary: [`README.md`](README.md). Siblings: [`strands.md`](strands.md) · [`cables.md`](cables.md).

⛔ **Reserved words:** `health` = journal system health **only**; owner physiological data is **`body`**. `activities` = the internal facet model **only**; owner movement is **body motion / fitness / kinetics**.

---

## `P-journal`

The on-disk journal. 🔴 **Deliberately coarse.** Talk about `P-journal` at a high level; when doing work, talk about the sub-plate.

| Sub-plate | Holds |
|---|---|
| `P-journal-segment` | the segment directory — chronicle day/stream/segment |
| `P-journal-stream` | stream declarations and state |
| `P-journal-thinking` | talent outputs and run logs |
| `P-journal-health` | durable **system**-health records and logs |
| `P-journal-body` | owner **body** records — never "health" |

⚠ **6 formats carry a real `x-journal-contract`; roughly 20 more on-disk shapes are defined only by a Python writer**, 8 of them undocumented anywhere. The path spine itself — including the default-stream transposition that `layout.json` never states — is Python-only.

## `P-device-link`

Link identity, security, mTLS.

⚠ **The machine-checked part of this boundary is the QR bootstrap only.** The `spl` conformance corpus covers pair-link forms `0x04`/`0x05`/`0x06`, address admission and ca_fp tags. The pairing ceremony, session lifecycle, framing and token JWTs are prose.

🔴 **`convey/secure_listener/` is this plate's authorization boundary** — TLS termination, the in-handshake fingerprint check, admission, framing, mux, and inline WSGI dispatch. It is Python.

**Identity:** the journal's own identity is a `jid` derived from its CA's P-256 SPKI. A **device's** identity is a **`did`** — the device certificate fingerprint, `sha256:<hex>` over **certificate DER** (`cert_fingerprint()`, `think/link/ca.py:269`), which is the value already stored per client.

⛔ **Two kinds of fingerprint exist here on purpose and must never be crossed.** `cert_fingerprint()` hashes the **certificate DER** — that is the `did`. `spki_fingerprint_sha256()` (`ca.py:66`) hashes the **DER SPKI** and is the `0x06` pair-link CA pin; its own docstring says *"not cert DER."* Reversing them silently breaks off-LAN pairing.

✅ The `did` is stable for the device's life — client certificates are 10-year and there is no renewal path in `think/link/`; revocation is via `authorized_clients.json`. ⚠ If a renewal path is ever added, a device's `did` changes and prior segments carry the old one.

**Carry forward, non-negotiable:** the certificate fingerprint is the identity and the label is only a label · revocation is the ledger, not the certificate · **an unreadable `authorized_clients.json` authorizes nobody** · loading never rewrites the ledger.

🔴 **The mark derivation is the most dangerous thing in this plate to reimplement.** `think/link/mark.py` calls itself the frozen journal identity contract. Argon2id v0x13, `t=3`, `m=65536`, `p=1`, `hash_len=32`, salt `solstone-journal-mark-v1`, secret = the jid's 16 raw bytes; seven big-endian u32 words drive `pick`/`pick_distinct` over 60 icons, 16 colours, 7,776 words, plus two rotation bits rendered `45 if bit else 0`. ⛔ **`_ICON_NAMES = tuple(_GLYPHS)` makes the icon index the JSON insertion order of `mark_assets/glyphs.json`.** That order is *currently identical to sorted order*, so an implementation that sorts passes every derivation vector and diverges the first time a glyph is appended out of alphabetical order. ⚠ The colour list is spectrum-ordered — that is the tell that file order is the rule. **The mark is what an owner looks at to confirm they are pairing with their own journal; a subtly-wrong reimplementation presents as evidence of compromise.**

## `P-device-ingest`

The ingest **API**. Deliberately separate from `P-segment-media`. ⚠ 9 published operations, 7 named client consumers.

## `P-segment-media`

An ingested, not-yet-processed segment.

## `P-segment-processing`

🆕 **Added 2026-08-05 by operator ruling.** The per-file **outcome ledger** — the record of
what processing decided about one media file: `state` (`analyzed` / `empty` / `failed`),
`reason_code`, `handler`, `attempted_at`, `input_size`, and `attempts` on failing records.
Written into the analysis header as `_solstone_processing`, plus the predicates every reader
decides against — failure exhaustion, re-entry eligibility, and terminal proof.

🔴 **It is its own boundary because four plates read it for four different purposes, and two
of those decide irreversible deletion.** `P-segment-sense` writes it and re-enters on it ·
`P-journal-retention` purges raw media on it · `P-device-ingest` proves to a device that its
upload was consumed so the device may drop its local copy · `P-web` and `P-index` derive
displayed and clustered state from it. Leaving it as a field of a storage format made it
nobody's contract while being everybody's decision input.

⚠ **9 production read sites in 8 modules across two languages** — see
[`strands.md`](strands.md) § `S:segment-sense:segment-processing` for the enumeration.
✅ **BUILT 2026-08-05 — `core/crates/solstone-core-processing-record`.** The record, its closed
vocabularies, the terminal-proof predicate and a published schema now live in one crate, and
`solstone-core-ingest-resolve` consumes the predicate instead of restating it.

- ✅ **One version string in the Rust tree.** `vocab::SCHEMA` is the only declaration, and a
  committed test `include_str!`s `terminal_proof.rs` to assert neither the literal nor the old
  constant name can come back. ⚠ **Cross-language equality with `processing_record.py:23` is
  still confirmed by inspection only** — nothing mechanically enforces it, and that is an
  accepted residual until Python is deleted.
- ✅ **The predicate is field-wise and missing-tolerant**, reading `record.get(...)` over a map
  rather than deserializing into a struct. ⛔ **Do not "improve" this into a typed struct with
  required fields** — the reference tolerates a record carrying only `schema`, `state`,
  `handler` and `input_size`, and a typed reader would withdraw terminal proof from partial
  records already on owner disks.
- ✅ A vectors fixture carries per-row `provenance_tag` and a `citation` to the reference
  `file:line` that determines each verdict, and its header records that **tests never execute
  Python**.

⚠ **A published schema now exists** (`schema/processing-record.v1.schema.json`) but it is
published **for readers**: the record still lives inside two other formats' headers, both of
which carry `additionalProperties: true`, so a malformed record still cannot fail validation
against a real journal file. ⛔ Enforcement is a later wave, not a solved problem.

## `P-segment-sense`

Media processing: an ingested segment's raw media becoming analysed output on disk.

⚠ **"Emits two strands" is stale** — it predates the wire/durable split. This plate emits **five** Tier-1 strands with five different contracts (`:journal-segment` fixture · `:system` schema · `:journal-segment-events` fixture · `:thinking` schema · `:segment-processing` fixture) plus the Tier-2 `S:segment-sense:speaker-id`. ⛔ The wire and durable halves of the event contract are deliberately separate; do not re-merge them.

**Shape, measured 2026-08-05.** One long-running dispatcher plus three handlers, all separate processes:

| | Module | Reads | Writes | Notes |
|---|---|---|---|---|
| dispatcher | `observe/sense.py` (1,508) | the `observe.observing` bus event, or a `--day` scan | nothing in the segment | spawns handlers by **file extension**, one `ThreadPoolExecutor` per handler, memory-gated, per-job wall-clock caps (`describe` 1800s · `transcribe` 2700s · `depict` 600s) |
| audio | `observe/transcribe/` (~3,100) | `.flac .opus .ogg .m4a .mp3 .wav` | `<stem>.jsonl`, `<stem>.npz` | VAD → silence reduction → backend registry → STT → native speaker analysis |
| screen | `observe/describe.py` (1,660) | `.webm .mp4 .mov` | `<stem>.jsonl` | dHash winnow → ArUco mask → categorize → select → extract |
| image | `observe/depict.py` (104) | `.png .jpg .jpeg .heic .heif .gif .webp .tiff` | `<stem>.jsonl` | one VLM call |

🔴 **The dispatcher is the plate.** Its behaviour is not incidental: skip and defer gates, re-entry rules, the memory gate, the watchdog, `exit 69` hold-raw, and segment completion all live there, and none of it is in a handler. ⚠ `observe/{hear,screen,see,grab,pdf_worker}.py` (2,269 lines) carry `observe/` names but are **read-side or other plates entirely** — sense reaches none of them.

⚠ **Handler exit codes are a contract of their own** and `observe/exit_codes.py` declares only part of it: `EXIT_PROVIDER_BLOCKED = 69`, plus `WATCHDOG_TIMEOUT`, which despite the module name is **a log string compared against nothing**. Also live and undeclared: **1** (transcribe hard failure and speaker-analysis failure), and **78** — ⚠ which is *not* a handler code at all but the dispatcher's own startup exit (`sense.py:1423`), before any handler runs.

🔴 **CORRECTED 2026-08-05 — "a deferral records neither success nor failure" is FALSE, and the comment that says so is in the code.** `sense.py:549-560` states the intent and does not implement it. The `69` branch and the `exit 0` branch **both** call `_check_segment_observed(file_path)`; the *only* difference is that success additionally calls `_record_successful_contact()`, a health-beacon counter — and that same counter is also ticked by the idle status loop every 5 seconds (`:885-887`), so it distinguishes nothing durable either.

⛔ **Consequence: a deferred segment is emitted as `observe.observed` with no error field, indistinguishable from a cleanly processed one**, and `stream.updated` is touched on the live path. Every downstream consumer — `think/top.py:274`, `think/supervisor.py:5919`, `think/importers/cli.py:164`, `apps/events.py:13` — proceeds as though the media was processed when it was not. ⚠ The hold-raw half is real and works: no output is written and the input is left in place, so the next scan re-picks the file. **It is the announcement that lies, not the retention.**

📌 **What a rebuild must anchor on instead:** no output written · the dispatcher does not unlink · **no `observed` emission that a consumer can mistake for a completed segment** · the file is re-selected on the next scan. ⛔ Do not port the code comment; port the corrected behaviour, and note that the same wording appears as a carry-forward on `S:segment-media:journal-segment` in [`strands.md`](strands.md).

⚠ **Two more undeclared behaviours in the same file.** Segment identity in every tracking structure is the **bare `HHMMSS_LEN` key**, not `(day, stream, segment)` — so two streams whose segments start in the same second collide, merging pending sets and landing errors on the wrong segment. And **shutdown records in-flight work as terminal failure** (`_run_handler:573-584`): a SIGTERM'd handler's non-zero exit becomes a segment error and emits `observed` with `error: True` — ⚠ and the daily repair phase runs the batch dispatcher under a wall-clock budget (`think/thinking.py:4611`), so a phase that runs over systematically writes **false failures**.

🔴 **Two silent-success paths.** `describe.py:964-967` and `depict.py:64-69` **return exit 0 having written nothing** when no thinking engine is configured, and the dispatcher reads that as success — it records a successful contact and marks the file done (`sense.py:562-571`). The live path is protected by a gate (`sense.py:817-825`); ⚠ **the `--day` batch path is not**, and the daily sense-repair pre-phase (`think/thinking.py:4592-4632`) is exactly that path. Re-entry eventually recovers because no output exists, but the success signal is false while it does.

⚠ **The retry budget is describe-only in practice.** `should_reenter_analysis_output` (`observe/processing_record.py:118-152`) returns `True` **only** for `handler == "describe"`, and transcribe writes its `corrupt_input` output through `_write_failed_processing_jsonl`, which then blocks re-entry at three separate guards. `FAILED_ATTEMPT_BOUND` never applies to audio.

**What is already Rust** — the speaker math, behind a one-record argv+stdio contract: `solstone-core-speakers` (3,749), `-speakers-analyze` (2,049), `-speakers-onnx` (662), reached through the 765-line Python adapter. 🆕 `solstone-core-depict` (`core/crates/solstone-core-depict/src/lib.rs`) is a standalone generate-wire consumer added 2026-08-05, but it is not dispatcher-wired: `journal depict` still resolves to `observe/depict.py`. ⛔ No dispatcher, describe, transcribe driver, or `_solstone_processing` *writer* is Rust; the one Rust touch on that header is a reader (`solstone-core-ingest-resolve/src/terminal_proof.rs`).

## `P-index`

🔴 **`day` semantics — one meaning, not three.** `day` is **the day the content originated from**: the source segment's day, or for an activity its **start** time. ⛔ It is not the recording day, not the last-seen day, and not the ingest day. For content that is genuinely not day-based, the **only** permitted fallback is the day it was last updated, and a fallback must be named as one rather than silently occupying the same field. ⚠ Before this, `day` conflated recording, source and last-seen meanings.

The SQLite index. **Ephemeral by design and always rebuildable — that property is required, not incidental.** ⚠ The index schema needs architecture work.

🔴 **Half of it IS already Rust, and the half that is has the larger share of the code.** ⛔ Do not read `think/indexer/native.py:6-11` as "this plate is Python" — it is accurate about what went native and silent about how much. `core/crates/solstone-core-indexer` (11,825 lines) + `solstone-core-indexer-store` (4,411) = **16,236 lines of Rust owning the entire CLI write path** — `--reset`, `--rescan`, `--rescan-full`, `--rescan-file`, `--rebuild-edges`. That is 5.5× the Python it fronts (`indexer/journal.py` 1,693 + `edges.py` 1,263). **A full rebuild is already native.** What remains Python is **the whole read/query path** plus the in-process writers.

🔴 **The schema DDL exists in two hand-maintained copies** — `think/indexer/journal.py:SCHEMA` and `core/crates/solstone-core-indexer-store/src/db.rs` (`CREATE_FILES` · `CREATE_CHUNKS` · `CREATE_EDGE_FILES` · `CREATE_EDGES` + the three edge indices). `db.rs:27` names the Python side as source of truth **for the edges half only**; the `chunks` DDL carries no such note. This is the two-places-one-contract class inside the plate whose schema is the thing being redesigned.

⚠ **Rust's `ensure_schema` has no equivalent of Python's `time_bucket` rebuild check** and its own comment says it relies on `--reset` instead. A pre-`time_bucket` index reached by the native path first gets `CREATE VIRTUAL TABLE IF NOT EXISTS` as a no-op, then an 8-column insert against a 7-column table.

**Shape of the live schema:** one FTS5 virtual table (`content` + **seven `UNINDEXED` columns** — `path`, `day`, `facet`, `agent`, `stream`, `idx`, `time_bucket`), a `files(path, mtime)` staleness watermark, and the derived `edges` / `edge_files` pair. 🔴 **Every metadata filter is therefore a post-filter over the whole match set, and a filter with no search term is a full table scan** — `_build_where_clause` emits `1=1` for an empty query. The `edges` half, which does have real indices, is the existing proof the same file can serve indexed lookups.

**Carry forward — measured on a large populated journal (2.83M chunk rows, 1.64 GB, 439 days):**

- 🔴 **FTS5 `optimize` is never run anywhere in either implementation, and the scheduler does not run it.** On a corpus with ~98k write transactions this left **34% of the file** as unmerged-segment fragmentation: the inverted index measured 695.7 MB where a single-pass rebuild of the identical rows measured 208.1 MB, and `optimize` + `VACUUM` recovered it in 7.6 s. Whatever the new schema is, **index maintenance has to be part of it** — this is not a schema flaw, it is a missing operation.
- 🔴 **The segment aggregate double-indexes its own children.** `_index_segment_chunks` re-concatenates a segment's `talents/*.md` under `agent='segment'` while those files are also indexed individually. Measured: **48.2% of rows and 41.1% of indexed text**, with **100% of aggregate paths also having their children indexed separately.** ⚠ It is not pointless — it buys phrase/`NEAR` matching *across* talents within one segment, which per-file chunks cannot serve. ⛔ But the read path then spends two `SELECT DISTINCT path` scans per query undoing it, and materializes the result as **one bind parameter per aggregate path** against SQLite's 32,766-variable ceiling — measured at 24,127 on that corpus, growing one per segment recorded. A redesign must decide whether that recall capability survives, and if it does, carry it as **written segment identity on the chunk row**, never as a query-time `IN` list.
- 🔴 **Filter-only retrieval has no defined order, and the reference silently returns an arbitrary sample.** The result fetch always orders by `bm25()`, but with no `MATCH` term every row scores identically, so the order degenerates to insertion order. Verified against a large journal: a caller asking for 12 chunks across a 7-day range received 12 consecutive rows from **day one only**, out of 832 available across all seven days — six days invisible, with no error and no signal. ⛔ **A rebuild must give filter-only retrieval an explicit, documented order**; recency is the obvious one for a journal. ⚠ Ordering by an identity that encodes the day gives *day* ordering, **not event-time ordering** — backfill, file replacement and a differently-ordered rebuild all diverge from event time within a day. If event time is required it is a separate written field, not more bits in the identity.
- ⚠ **`day` is the dominant query axis** (three of eight filter parameters are date bounds, and it is present on two thirds of recorded queries) and is stored as unindexed `TEXT` compared with `>=`/`<=`.
- ⚠ **The index cannot search non-ASCII text.** The query path strips every character outside `[a-zA-Z0-9\s"'*]` before the term reaches FTS5, so `José` becomes `Jos `. The corpus is indexed correctly — 98.6% of chunks contain non-ASCII and the terms are reachable when queried directly. ⛔ The sanitizer's job is FTS5 **syntax** safety, never charset restriction; a rebuild must escape and quote rather than delete.
- ⚠ **Aggregation is part of every read**, not a separate feature — results are always paired with counts by facet/agent/day/stream, and today that is done by pulling every matching row into the application.

## `P-format`

Consistent formatting of **structured journal data** for its consumers — the indexer and the convey apps.

🔴 **No import graph shows this plate's fan-out.** `FORMATTERS` (`think/formatters.py:139-265`) reaches 12 modules by **string key** via `import_module` + `getattr` (`:283-286`), with zero static import edges. It is the de facto read-side inventory of every on-disk shape, and it lives only in Python.

🔴 **LIVE DEFECT, recorded 2026-08-05.** Dispatch is by **`fnmatch` on the journal path**, and **9 of 36 patterns embed a stream name**. Projected stream names match none of them, so `get_formatter()` returns `None` and **the read path silently loses its formatter** — no error, no fallback, just no formatting. Projected names are now being written. ⚠ This also forced the segment `kind` to carry a source dimension rather than being a flat enum. Enumeration: `vpe/workspace/ingest-cable-260804/tools/stream-name-dependents.md`.

## `P-thinking`

🔴 **A grouping plate.** Holds **two contracts: `generate` and `cogitate`**. Everything connects to it. `P-local`, `P-BYO` and `P-SPP` sit behind it. `resolve_provider()` accepts exactly those two interface names and no others (`models.py:512`).

**`generate` is defined in [`../GENERATE.md`](../GENERATE.md).** Tier **schema + fixture** — an interface format whose closed vocabularies and conformance vectors are pinned as data in `core/fixtures/generate_contract.json`.

🔴 **The plate's import count is not the contract's fan-out, and the difference is tenfold.** 46 production modules import `think.models`; **11 of them import a `generate` entry point** (`generate`, `generate_with_result`, `agenerate`, `agenerate_with_result`), and one of those 11 is the wire itself. The other 35 import model constants, the error classes, `resolve_provider`, or cost helpers — `think.models` is a grab-bag module and its import count is a property of the module, not of this boundary. ⛔ Do not size `generate` work from the module's importers.

⚠ **Ten of the eleven are one-shot; one is a fan-out.** `think/batch.py` is the only caller that needs many completions in flight, and it has three consumers of its own (`observe/describe.py`, `apps/timeline/rollup.py`, `apps/timeline/maintenance.py`). That single asymmetry is why `generate` is one vocabulary in **two framings** rather than one shape or two contracts.

🔴 **The retry and hold-raw decisions are keyed on a reason code, and the classification belongs here.** `RUNTIME_REASON_CODES` (`providers/shared.py:253`) is a closed 17-member set; `is_non_retryable_generate_reason` (same file) and `is_blocking_reason` (`convey/provider_readiness.py:420`) map it to *retry or not* and *hold the owner's raw media or not*. Those two predicates live in two modules outside this plate. ⛔ A caller that re-derives them owns a copy of this plate's contract — the boundary publishes the decisions.

⚠ **Four near-identical entry points, three error semantics.** `generate` / `generate_with_result` / `agenerate` / `agenerate_with_result` each repeat the same nine-step policy sequence; the two `_with_result` forms make schema validation advisory while the two plain forms raise on it. Only `generate_with_result` accepts `num_retries`, `inference_retry_index`, `local_exclusive_admission` and `enforce_responsiveness`. One boundary, four doors, differing on what a schema failure means.

⚠ **The runtime preamble is `cogitate`'s, not `generate`'s.** `COGITATE_RUNTIME_PREAMBLE` is prepended by `providers/cli.assemble_prompt`, reached only from `run_cogitate` (`providers/openhands.py:1744`); `run_generate` and `run_agenerate` never touch it. It exists as a **sha256 only** in `core/fixtures/cogitate_contract.json` — 1,989 bytes, not reconstructible. ⚠ **And "cross-language" is a location, not yet a fact: zero Rust files read that fixture**, so today the digest detects only Python-source-versus-fixture drift. It would catch real drift the moment a native `cogitate` exists — and would then be unable to tell it what text to send.

⚠ **Only two provider modules implement `run_generate`** — `providers/local.py` (1,444 lines) and `providers/openhands.py` (2,248). `providers/` totals 21,029 lines; the remainder is install, health and attestation machinery belonging to `P-local` and `P-SPP`, not to this call path.

## `P-local`

Local model runtime, inside the security boundary. ~9k lines of live install/runtime machinery with zero dead modules.

⚠ The boundary is loopback HTTP **plus a durable record** — `health/brain.json`, written and validated by `providers/brain_state.py` (2,692 lines, HMAC-fingerprinted, refresh lease, three lanes). Calling it a types boundary drops the durable half.

⚠ **Do not carry** the in-loop USD ceiling — it fabricates a cloud price for local runs and force-stops them. Keep the **context**-fraction half of the same function.

⚠ **Do not carry the loopback "guard" as written.** The `cmd` list is built with `"--host", "127.0.0.1"` hardcoded and the next statement is `if "0.0.0.0" in cmd: raise` — a membership test against a literal set three lines above. It cannot fire at runtime and would miss `--host=0.0.0.0`, `::`, or a hostname. ✅ **The honest statement of the same invariant is `services/spp_transport.py:190-197`** — it binds `("127.0.0.1", 0)` and writes down why same-UID reachability is acceptable and which constraints are actually load-bearing. Carry that one.

## `P-BYO`

An owner's own token to a known provider, or their own OpenAI-compatible URL and token. ⛔ **Egress.** Will split per provider.

## `P-SPP`

Confidential hosted processing. ⛔ **Egress.** Attested, non-retained. ⚠ **Fails closed by the absence of a fallback branch** — a refactor that tidies it can open a downgrade path with no test going red. Make "no downgrade path" an explicitly tested invariant.

## `P-speaker-id`

Per-statement embeddings → speaker fingerprints. ✅ A native kernel already sits behind an argv+stdio wire with an algorithm-identity handshake.

⚠ `.npz` sidecar and `speaker_labels.json` are Python-only. ⚠ The `sentence_id` join key is recomputed at read time and stored nowhere. ⚠ The voiceprint corpus is real-person biometric data. 🔴 **Years of voiceprints must survive with no re-teach** — that is a shipped promise.

## `P-entity`

The entity store. ⚠ **47 distinct production modules** import `think.entities`, fanning in from at least four plates.

🔴 **This store fails by BRICKING, not degrading** — the history tree and `ambiguities.jsonl` are self-validating, so a subtly-wrong implementation destroys the store rather than producing worse results. All three algorithm-identity hazards — `entity_slug()`, `casefold`, `rapidfuzz` at threshold 90 — live behind this boundary.

**Holds:** `entities/{id}/entity.json` · `entities/{id}/history/{events,prepared,private}/` · `entities/{id}/voiceprints.npz` · `entities/ambiguities.jsonl`. ⚠ The ambiguity file is **journal-level and belongs here even though its rows carry a facet scope** — scope is a *field*, not a location. Putting it in `P-facet` would give two plates one contract.

🔴 **The precise failure, both formats: reads keep working and every mutation is refused, permanently.** ⛔ It is not corruption, and ⛔ it is not degradation.

| | Trigger | Blast radius |
|---|---|---|
| history tree | the on-disk identity equals **neither** a leftover prepared event's `identity_before` nor its `identity_after` → `EntityHistoryRepairRequired` (`history.py:244-280`) | **one entity**, forever |
| `ambiguities.jsonl` | any row fails `_validate_row` — mutation re-reads `strict=True` under the lock before writing (`ambiguities.py:341-374`) | **the whole file** |

⚠ **`EntityHistoryRepairRequired` is raised in one file and caught in none.** No handler, no repair verb, no `doctor` check — it surfaces as an unhandled exception into whatever called it.

🔴 **Reconciliation compares the whole entity dict with no field allowlist** — `_identity_snapshot()` is `copy.deepcopy(dict(entity))` (`history.py:552-555`). **One extra field bricks the entity.** ⚠ And the comparison is *Python* equality: `1785889922582 == 1785889922582.0` is true, so an int→float drift recovers. `serde_json::Value`'s `PartialEq` says those differ — **an implementation using it bricks where the old one recovered.** The equality predicate is part of the contract.

✅ **Carry forward — fail closed on mutation, stay open on read.** This is the same posture as `CorruptConfigError` in `P-journal-config`, and it is the house style rather than a local quirk. ⛔ Do not relax it into leniency.

⚠ **Two serialization conventions live one directory apart, and both are load-bearing.** `entity.json` is `indent=2, ensure_ascii=False` in **insertion order** (`history.py:573`); history events add **`sort_keys=True`** (`:681`); `ambiguities.jsonl` is one compact object per line (`:330`). All `0600`. Event filenames are `{seq:020d}-{version_id}.json` (`:661`), so lexical order **is** chronological order.

⚠ **Timestamps are not one format.** `created_at` is epoch **milliseconds**; `ambiguities` `ts` is ISO-8601 `Z` at **seconds** (`ambiguities.py:377-381`); history `ts` is ISO-8601 `Z` at **variable width** — `history._now_iso()` (`:685-686`) drops the fractional part entirely when `microsecond == 0`, so roughly one event in 10⁶ reads `…:02Z` and the rest read `…:02.582506Z`. **A reader that requires six fractional digits fails on those records years after they were written.**

## `P-facet`

Facets and their per-facet contents, including facet-scoped entity and speaker material. ⛔ Distinct from `P-entity`: the entity store is the identity-bearing thing, the facet is the organizing structure over it.

**Holds:** `facets/{facet}/facet.json` · `facets/{facet}/entities/{slug}/` (observations, relationships) · `facets/{facet}/activities/{day}.jsonl` · `facets/{facet}/news/` · `facets/{facet}/logs/{day}.jsonl`.

🔴 **The two levels key the same concept differently, and that is the seam between these plates.** Journal level is `journal_entity_memory_path(entity_id)`; facet level is `entity_memory_path(facet, name)` → `entity_slug(name)`, re-derived on every access. See [`README.md`](README.md) § writing and reading.

⚠ **`rename_facet()` (`facets.py:1246-1320`) re-derives the facet's identity and repairs almost nothing.** It `os.rename`s the directory, updates **only** `config/convey.json`, and *prints* an instruction for a human to rebuild the index. Ambiguity rows carry `scope: {kind: "facet", facet: …}` and are not touched — **so renaming a facet orphans every resolution choice the owner made inside it**, and they are asked again.

⛔ **`activities` here is the internal facet model only** — never the owner's physical movement, which is body motion / fitness / kinetics.

## `P-journal-config`

`journal/config/journal.json`, read by **30 production modules**. Durable, `0o600`, mutated under `hold_lock` + `atomic_replace` with an explicit transaction type.

✅ **Carry forward — this is the house style, not a local quirk.** `CorruptConfigError` (`think/utils.py:53-68`): a **missing** config returns deep-copied defaults; a config that **exists and will not parse raises**, in owner voice — *"I couldn't read your settings file… Your settings were NOT changed."* Two deliberately different postures on two failure modes, never silently substituting on the dangerous one.

⚠ **The config file being the source of truth is an external commitment made in writing.** A contract-breaking pass here can violate it by accident.

## `P-journal-retention`

The logic that decides what raw media is retained, and what logs are retained for how long. `think/retention.py` (708 lines) **irreversibly deletes owner raw media**; `log_retention.py` (1,006) prunes logs.

🆕 🔴 **Widened 2026-08-05 by operator ruling: this plate EXECUTES every removal of owner media, and it is the only plate that does.** Other plates **request**; retention removes. Three consequences that are not local to retention:

1. ⛔ **The segment is the unit of deletion.** A segment is removed whole — every file, leaving a `tombstone.json` — or it is not removed. **There is no partial-segment delete.** The *mixed* classification and the reserved-name set that fed it existed only to serve a capability the product no longer offers.
2. ⛔ **`transcribe` stops unlinking VAD-empty raw audio.** It writes the terminal-empty marker exactly as it does today and hands the raw to retention. One subsystem, one policy, one place to look when owner media went.
3. 🔴 **Retention notifies `P-index` of the paths it actually removed, after removing them.** ⛔ Ordering is the contract: the index is told about removals that have happened, never about removals that are intended. An index prune is not a removal — the index is rebuildable by design, so pruning it is a cache invalidation and a rebuild undoes it. **Anything an owner is told was removed must be removed from the chronicle first.**

⚠ **Open, and not settled by that ruling: legacy segments holding one source's data beside another's.** An owner asking to delete one source from such a segment either loses the segment whole, including material they did not ask to delete, or keeps the data they asked to remove. Rule 4's unacceptable outcome is older journal data left *unseen*; this is the sibling — older journal data left **undeletable**.

🔴 **Carry forward:** *read one extraction file strictly enough for irreversible deletion* (`retention.py:110`) — reads at most two lines and treats any `OSError` / `JSONDecodeError` / non-dict as **`"malformed"`, never as "empty, safe to purge"**, with an explicit guard at `:136-139` against a stray marker key making a header-only file look chunk-bearing. Plus `resolve_segment_gate`: `.npz` without `talents/speaker_labels.json` ⇒ incomplete.

⚠ Retention imports `apps/backup/copy` and `think/offload.py` imports back out of retention, so deletion is coupled to backup.

## `P-web`

The journal web service — human interface, web apps, **and the API**.

⛔ **Access model: 100% of `P-web` is either `localhost:5015` (human web, or a same-device CLI) or an authorized linked device. There is no third way in — the boundary *is* the authorization.** ⛔ A third access path is not a decision to make inside this repo.

⚠ 77 of 135 routes uncontracted including 20 state-changing; `/api/shell` — the most load-bearing endpoint in the owner UI — is not in the contract. ⛔ Zero of 177 contracted operations declare authentication, so a port generated from the contract inherits an unauthenticated surface.

## `P-CLI`

Splits almost immediately.

| | Reach | May do |
|---|---|---|
| `P-CLI-sol` | any device | **API only**, over a link |
| `P-CLI-journal` | the same journal device only | **modify the journal directly**, or the API over localhost |

## `P-system`

Operations for managing asynchronous activity — starting things, running things.

**Carry forward:** the task-request refusal **classifies rather than guesses** — it distinguishes `"wedged"` (runtime > 2× cap) from `"still_running"` and emits a skip event with both refs and the reason. A refusal that says *which* refusal it is.

## `P-system-health`

Health of running things — current status, in-memory. ⛔ **System** health, never owner body data.

🔴 The per-day health JSONL grammar is **entirely Python string literals**. The callosum envelope likewise — two constants and a docstring carrying the whole control plane.

## `P-body-source`

Owner **body** data arriving from outside — Oura, Apple Health. ⚠ **Ingress, not egress** — nothing of the owner's journal leaves.

⚠ The normalized shard format is defined only by its **reader** (`apps/body/routes.py:311-334`), and `imports/health-dedupe.sqlite` uses raw `sqlite3` entirely outside `journal_io` discipline. 🔴 **Excluded from every backup with no rebuild path** — a restore silently empties the owner's body history.

---

## ⛔ Egress — where the covenant applies

Journal and devices are **one secure environment**. No per-plate privacy tracking inside it; transport security is already covered.

**Actual egress — three:** `P-BYO` (the owner's own key, owner-directed) · `P-SPP` (attested, non-retained) · **support requests** — ⚠ the `_SECRET_*` redaction in `apps/support/diagnostics.py:29-50` is the **last thing** between a journal config and an external service.

**Blind by construction, therefore not egress:** relay transit · push notifications · encrypted backups.

🔴 **Push is only reimplemented in an end-to-end encrypted form** — the journal encrypting and the receiving device decrypting with the link cryptographic identities. ⛔ The current plaintext path, which carries journal-derived chat content to a push service and which unpairing does not revoke, does **not** come across.
