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

🆕 🔴 **`solstone-core-depict` is not merely unwired — it is UNPACKAGED, and so is `solstone-core-retention-cli`.** Measured 2026-08-06: only two Rust wheels exist (`packages/solstone-core`, `packages/solstone-core-speakers-analyze`), neither builds either binary, and `solstone-retention` is absent from `PATH` and from the dev environment on the reference host. A crate that compiles, tests and gates reaches no journal at all until something packages it, and nothing in the gate set detects that. ⛔ Read "is already Rust" in this section as a statement about the source tree, never about what a host runs.

🆕 ⚠ **`describe` DOES write a processing record** — it stamps one at every terminal promote, including `attempts` on failures, and `should_reenter_analysis_output` is keyed on it. **`depict` writes none**, in Python or in `solstone-core-depict`, so an ingested still image can never be proven consumed and the sending device never releases its copy.

🆕 🔴 **Truncation is invisible to this plate.** Measured against a reference-observed corpus (`core/fixtures/describe_frames.json`): a WebM cut short by a crashed recorder decodes **cleanly** to a shorter frame set with no decode-failure flag, so the handler records `analyzed` / `ok` over a partial description and nothing anywhere says frames were lost. Corruption early in the stream does set the flag, and yields nothing. ⚠ The reference's branch that returns already-collected frames *alongside* a decode failure was unreachable across a sweep of 46 corruption offsets at two widths — it is unpinned by any corpus.

🆕 ⚠ **The frame loop's order is not the obvious one, and a rebuild that gets it wrong stays green.** Per decoded frame: the `raw` counter increments **before** the presentation-timestamp check; the fiducial mask runs **before** the perceptual hash, so the hash is computed on the *masked* image; and a frame the mask rejects consumes its frame index without advancing the winnow's last-kept reference. ⛔ A corpus carrying no fiducials cannot detect the mask being applied after the winnow instead of before the hash.

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

🆕 **Measured 2026-08-06 against a populated journal, and three of these correct the notes above.**

- 🔴 **Coverage degrades by recency, and the newest content is the least indexed.** Not an average
  miss rate — a cliff. On a real journal the four newest days held **1,400 talent files across 967
  segments with zero index rows**, neither individually nor through the aggregate, while days a week
  older were 37% and 61% covered. The mechanism: the daily pipeline produces a day's talent output
  *after* that day closes, and a light scan excluded any day directory sorting before wall-clock
  today — so the scan that exists to index the day's output could not index it. ✅ **Closed:** scan
  scope now follows discovery, and `scan_journal` no longer takes a clock at all. ⚠ A light scan may
  still retain rows for a day discovery produced *no* files for, and warns when it does; only a full
  rescan removes those.
- 🔴 **The read path, not the invocation path, is where the time goes.** An earlier reading attributed
  ~500 ms of a search to process startup. Measured: interpreter and import cost **66 ms**, the native
  binary spawns in **under 1 ms**, and of a 761 ms search **~695 ms is the read path doing work**. The
  filter-only browse shape costs **1,644 ms** against **0 ms** of full-text work — 483 ms of it a full
  scan to validate an agent name against a 31-element set, 330 ms materialising counts nobody asked
  for, 326 ms counting a match set with no `MATCH` term. ⛔ Do not size a redesign as though the
  invocation path dominates; it is ~9%.
- 🔴 **A wholly non-Latin query returned the entire corpus, presented as a match.** The query path
  deleted every character outside `[a-zA-Z0-9\s"'*]`, so a query in Han, Arabic or Greek compiled to
  the empty string; an empty expression means no `MATCH` term, which means every row qualifies. On a
  real journal that reported over 1.4 million results with confident facet and agent breakdowns, while
  a genuinely-unmatched term correctly returned zero. ⛔ "Non-ASCII is unsearchable" understates it:
  it was **mis-answered**, which no caller can distinguish from a real result.
- ⚠ **The honest invariant is that no query text FTS5 could act on is destroyed before it gets
  there** — ⛔ **not** "any token in any script is searchable." With `unicode61` a run of Han indexes
  as **one** token, so a query for part of it cannot match; emoji are separators and are not indexed
  at all. Both were measured. Making those findable is a **tokenizer** decision, not a query-path one.
- 🔴 **Schema work is gated on there being one writer, and there are two.** Only the CLI write
  operations are native; the in-process writers — day-accumulator appends, chat stream appends,
  importers, backup restore, observer prune, share delete, entity-merge — still write the index
  directly from the reference implementation, against its own copy of the DDL. Day-ordered
  identities, a typed `day`, a content-type dimension and `secure_delete` on every writer connection
  all need both writers moved together. ⛔ Do not scope a schema change as though the write path were
  already single.

## `P-format`

Consistent formatting of **structured journal data** for its consumers — the indexer and the convey apps.

🔴 **No import graph shows the reference implementation's fan-out.** `FORMATTERS` (`think/formatters.py:139-265`) reaches 12 modules — **18 entry-point functions** — by **string key** via `import_module` + `getattr` (`:283-286`), with zero static import edges. It is the de facto read-side inventory of every on-disk shape.

✅ **The indexer half is built.** `core/crates/solstone-core-indexer/src/content/` carries **30 of the reference's 36 patterns** across 15 families, and is the shipped index write path — `think/indexer/native.py` routes every index write operation to `solstone-core indexer` with no fallback. The 30 are exactly the reference's `indexed=True` subset; every family agrees.

⚠ **The six missing are exactly the six the reference marks `indexed=False`:** `entities/*/entity.json`, `*/*/*/audio.jsonl`, `*/*/*/*_audio.jsonl`, `*/*/*/*_transcript.jsonl`, `*/*/*/screen.jsonl`, `*/*/*/*_screen.jsonl`. ⛔ **Name trap:** `content/screen.rs` is the `talents/screen.json` record formatter, **not** the raw `screen.jsonl` one — the file list overstates coverage.

⚠ **The rendered-value half is complete; storage and serving remain open.** `produce_chunks` now carries the full formatter contract: document `header`, chunk `occurrence_time_ms`, and originating `source` record. The index/SQLite layer still stores only content, and the convey read path still cannot serve the added fields for speaker attribution, audio seek, or frame overlays. `S:web:format` has no implementation here at all. ⚠ Rule 1 says the one-to-many end cannot negotiate per-consumer, and an output shape chosen for the indexer is exactly that.

⚠ **Corrections to the 2026-08-05 defect note, measured rather than inherited.** **10 of 36** patterns pin a stream name, not 9 — `*/chat/*/chat.jsonl` was missed because the enumeration scanned the `import.*` family; it is projection-stable, which is why nothing caught it. ⛔ **"Projected names are now being written" was not true** — the projection landed after the last release and no projected stream name has reached a journal. Against the largest journal available, a cutover changes **18 of 538,647** formatted files, all `*_transcript.md` under two import streams, and swaps none. ⛔ **And the failure mode is not a silent `None`:** six of the nine import patterns fall through to a *different* formatter — an AI-chat transcript lands on the audio formatter at `indexed=False`, so it stays formatted and silently stops being searchable. A `None` at least raises.

📌 **The same shape is reachable with no projection involved:** `browser_*_screen.jsonl` matches `*/*/*/*_screen.jsonl` (`indexed=False`) before `*/*/*/browser_*.jsonl` (`indexed=True`), so discovery finds it as one shape and dispatch renders it as another. Latent today.

⚠ **Three matchers, two semantics.** Reference dispatch uses `fnmatch`, where `*` crosses `/`; reference discovery uses `Path.glob`, and this crate uses `glob` with `require_literal_separator`, where it does not. Dispatch is the outlier — which is why discovery and dispatch can disagree about the same file.

✅ **Every family is pinned to a reference-generated corpus** — `core/fixtures/content_families.json`, 40 cases from `scripts/content_family_corpus.py`, resolving each case through the registry by journal-relative path so it pins dispatch as well as render. ⚠ It is a **frozen record**: regenerating it needs a runnable reference tree. A `DIVERGENCES` ledger in `content/mod.rs` makes every difference a written decision; an unrecorded one fails the gate. ✅ **Chat speaker labels are resolved at scan time** from journal config with the reference precedence and a fallback diagnostic, so a rescan preserves the owner's configured label; the ledger has no `Defect` entries.

🔴 **Shape resolution is deliberately two-path, and the path-derived half is PERMANENT — operator-approved 2026-08-06.** ⛔ Do not read the write-new-read-old rule as requiring its removal: this plate is a sanctioned instance, not an unconverted one. The written value wins wherever present; path classification serves content written before the identity existed, and that content is never migrated. ⚠ **Precedence is the load-bearing half** — a written value that does not win is decoration, which is the failure this document already records for `entity_slug()`. 📌 Rendering already takes a **shape** rather than a path (`produce_chunks_by_shape`), so adding the written source is additive and touches no renderer. ⚠ Where that value physically lives is **not** settled and is `P-journal`'s call, not this plate's.

## `P-thinking`

🔴 **A grouping plate.** Holds **two contracts: `generate` and `cogitate`**. Everything connects to it. `P-local`, `P-BYO` and `P-SPP` sit behind it. `resolve_provider()` accepts exactly those two interface names and no others (`models.py:512`).

**`generate` is defined in [`../GENERATE.md`](../GENERATE.md).** Tier **schema + fixture** — an interface format whose closed vocabularies and conformance vectors are pinned as data in `core/fixtures/generate_contract.json`.

🔴 **The plate's import count is not the contract's fan-out, and the difference is tenfold.** 46 production modules import `think.models`; **11 of them import a `generate` entry point** (`generate`, `generate_with_result`, `agenerate`, `agenerate_with_result`), and one of those 11 is the wire itself. The other 35 import model constants, the error classes, `resolve_provider`, or cost helpers — `think.models` is a grab-bag module and its import count is a property of the module, not of this boundary. ⛔ Do not size `generate` work from the module's importers.

⚠ **Ten of the eleven are one-shot; one is a fan-out.** `think/batch.py` is the only caller that needs many completions in flight, and it has three consumers of its own (`observe/describe.py`, `apps/timeline/rollup.py`, `apps/timeline/maintenance.py`). That single asymmetry is why `generate` is one vocabulary in **two framings** rather than one shape or two contracts.

🔴 **The retry and hold-raw decisions are keyed on a reason code, and the classification belongs here.** `is_non_retryable_generate_reason` (`providers/shared.py:275`) and `is_blocking_reason` (`convey/provider_readiness.py:420`) map a reason code to *retry or not* and *hold the owner's raw media or not*. ⛔ A caller that re-derives them owns a copy of this plate's contract — the boundary publishes the decisions.

🔴 **FIVE reason-code vocabularies exist, three of them share the name `RUNTIME_REASON_CODES`, and two spell the same concept differently.** Measured 2026-08-05:

| set | size | case | what it serves |
|---|---|---|---|
| `providers/shared.RUNTIME_REASON_CODES` | 16 | snake | generate-path error classification |
| `providers/brain_state.RUNTIME_REASON_CODES` (`get_args(ReasonCode)`) | 42 | **kebab** | local-runtime health records |
| `brain_cli.RUNTIME_REASON_CODES` — an **alias import** of `REASON_CODES` | 41 | | CLI presentation |
| `brain_health.LOCAL_RUNTIME_REASON_CODES` | 8 | snake | local health grouping |
| `convey/provider_readiness._ENTRIES` | 43 | snake | owner-facing presentation **and the blocking decision** |

⚠ `gpu_probe_failed` and `gpu-probe-failed` are the same concept in two of these. ✅ The 16 are a proper subset of the 43. 🔴 **The vocabulary a `generate` consumer needs is the 43** — `blocking` is decided over it (24 of 43 are blocking), and the sole non-retryable code, `non_responsive`, is in the 43 and **not** in the 16. ⛔ Wiring the 16 into a caller loses both decisions.

🔴 **And the 43 do not cover this plate's own egress failures.** `attestation_not_yet_verified`, `attestation_failed` and `attestation_stale` are the `reason_code` class attributes on the three attestation exceptions (`models.py:275-303`) and are **absent from the taxonomy entirely**, so the blocking predicate answers `false` for all three — while a missing provider key answers `true`. **Operator ruling 2026-08-05: an unverifiable confidential environment holds the owner's material.** The `generate` contract therefore classifies that family `blocking: true` explicitly, and an unknown or absent code resolves to `retryable: false, blocking: true` — the preserving direction. ⛔ The live Python predicate is deliberately **not** changed; the classification lives in the contract as a rebuild invariant. ⚠ It becomes a behaviour change the first time a converted consumer reads `blocking` off the wire, and whoever lands that inherits a media handler that aborts-and-holds on an attestation outage instead of burning its re-entry bound.

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

The logic that decides what raw media is retained, and what logs are retained for how long. `think/retention.py` (709 lines) **irreversibly deletes owner raw media**; `log_retention.py` (1,006) prunes logs.

🆕 🔴 **Widened 2026-08-05 by operator ruling: this plate EXECUTES every removal of owner media, and it is the only plate that does.** Other plates **request**; retention removes. Three consequences that are not local to retention:

1. ⛔ **The segment is the unit of deletion.** A segment is removed whole — every file, leaving a `tombstone.json` — or it is not removed. **There is no partial-segment delete.** The *mixed* classification and the reserved-name set that fed it existed only to serve a capability the product no longer offers.
2. ⛔ **`transcribe` stops unlinking VAD-empty raw audio.** It writes the terminal-empty marker exactly as it does today and hands the raw to retention. One subsystem, one policy, one place to look when owner media went.
3. 🔴 **Retention notifies `P-index` of the paths it actually removed, after removing them.** ⛔ Ordering is the contract: the index is told about removals that have happened, never about removals that are intended. An index prune is not a removal — the index is rebuildable by design, so pruning it is a cache invalidation and a rebuild undoes it. **Anything an owner is told was removed must be removed from the chronicle first.**

🆕 ⛔ **CLOSED 2026-08-05 by operator ruling, by removing the question rather than answering it. Do not re-derive it.** This entry used to record an open call: legacy segments holding one source's data beside another's, where an owner deleting one source either loses the segment whole or keeps the data they asked to remove.

🔴 **There is no source-delete affordance.** The owner deletes **a segment, or a set of segments** — that is the only owner-facing removal there is, and ⛔ **there is no affordance for a partial owner-directed delete of any kind.** The legacy-mixed problem existed only as a *resolution* step, turning "delete my ⟨source⟩ data" into a set of segments; with the owner naming segments directly there is nothing to resolve and no disposition to choose.

✅ **The surface already exists** — the per-segment delete route under the transcripts app, with containment via `commonpath` and a **10-second undo window**. ⛔ Retention never resolves anything; it receives owner-chosen targets. Selection lives with the surface.

🔴 **The whole-segment verb therefore takes a SET.** One receipt covers it, and per-target failures are receipt rows. ⛔ It is not all-or-nothing: an owner deleting forty segments must not lose the thirty-nine that succeeded because the fortieth was unreadable.

⛔ **Retired with it:** the owner-facing source-delete route · the source-delete implementation and both of its branches · the deletable-source-stream allowlist · the mixed / location-only classification and its discovery helper. ⚠ **The reserved-name set divergence loses its last load-bearing consumer** — it fed the mixed classifier, and there is no classifier.

### Two units of removal, and the plate serves both

🔴 **Ruled 2026-08-05: § 1 binds owner-directed deletion ONLY, and the plate keeps two units.** Reading § 1 as binding every removal makes § 2 above contradictory — handing retention a VAD-empty raw would destroy the terminal-empty marker `transcribe` had just written, along with the segment's transcript and every derived output. The distinguishing property is **what the owner asked for**, not what is on disk.

| unit | the owner asked for | what goes | what survives |
|---|---|---|---|
| **the segment**, or a set of them | *"delete these segments"* | every file in each | `tombstone.json` only |
| **the proven originals** | nothing — this is the retention lifecycle | raw media whose processing is proven terminal | every derived output |

⛔ **§ 1 binds the first unit.** What it forbids is a *deletion* that leaves part of its target behind — the failure it was ruled against was a segment keeping derived output on disk, undisclosed, after the owner asked for that data to go. The second unit is the plate's standing scope (`retention.py:12-14`: *"Scope: raw media ONLY. Chronicle JSONL, derived outputs, `talents/` directories … persist indefinitely"*), and derived output surviving is the point of it.

⚠ **The second unit needs a guardrail or it becomes a back door to the first.** A releasing caller must not be able to name a path: it names *proven* originals only, and the proof is the predicate below. A sidecar, a derived output, a `talents/` entry or a reserved name must be **unnameable** in a release request, not merely refused by a check.

### The removal-request contract

🆕 ✅ **Minted 2026-08-05 as `S:*:journal-retention`, and retention owns it.** Retention is the one-to-many end: it serves all comers and cannot negotiate per-caller, so rule 1 puts the contract here. The other four strands are ones where retention is the *consumer* and the far plate owns the contract; this is the one where it is the provider. ⚠ **Four requesters** — the owner's segment delete, the terminal-empty hand-off, the offload pass, and the configured policy — and until this strand existed that contract had four callers and no name. See [`strands.md`](strands.md) § Tier 1.

⛔ **The request names its unit**: whole segments (one, or a set) leaving a `tombstone.json`, or the proven raw originals leaving every derived output. ⛔ There is no third unit, and ⛔ no partial owner-directed delete.

🔴 **A removal request must carry its own precondition, because consolidating the removers must not weaken the strongest one.** `think/offload.py:308-402` archives to backup, **confirms the snapshot holds every byte at the recorded size**, appends its ledger, and only then mints an approval-required removal mark — where retention's own path hashes the bytes and unlinks with no archive. When that removal becomes a request, the confirmed-snapshot precondition travels **with the request**; a request type that can be constructed without it has moved the guard out of the executor and into the caller.

### 🔴 The predicate — measured, because the two irreversible readers disagree

**Retention does not decide on the processing record's `state`.** `derive_modality_state` (`think/data_state.py:121-158`) consults `state` for exactly two values — `failed` and `empty` — and derives *analyzed* from something else: the presence of a **second JSONL line carrying the modality's marker key** (`start` for audio, `timestamp` for screen), via `_classify_marker(has_chunks=True) → "chunks_win"`. So there are two doors, and only one looks at a record at all.

Measured against hand-built segments, `eligible` meaning the owner's raw audio is unlinked:

| on-disk shape | retention | terminal proof |
|---|---|---|
| full valid record, **no** analysis row | `incomplete` | ✅ holds |
| full valid record + analysis row | **`eligible`** | ✅ holds |
| **no record at all** + analysis row | **`eligible`** | ✗ refuses |
| `{"state": "empty"}` and nothing else | **`eligible`** | ✗ refuses |
| `state=empty` with a **wrong schema**, a **wrong handler**, or a **mismatched `input_size`** | **`eligible`** | ✗ refuses |
| `{"state": "analyzed"}` and nothing else | `incomplete` | ✗ refuses |
| an unrecognized `state` + analysis row | **`eligible`** | ✗ refuses |
| `{}` as the record + analysis row | **`eligible`** | ✗ refuses |
| an analysis row carrying **only** the marker key | **`eligible`** | ✗ refuses |
| `state=failed` | `failed`, blocks | ✗ refuses |

🔴 **The divergence runs in both directions.** Retention releases raw media on eight shapes terminal proof refuses, **and holds it forever on one shape proof accepts.** A rebuild that only tightens retention toward proof fixes eight rows and leaves one; making them the same call fixes all nine and makes re-divergence unrepresentable rather than merely detectable.

⚠ **The `has_chunks` door consults no processing record whatsoever** — `{}` releases, an unrecognized `state` releases. This is not *"no schema check"*; it is *no record check*, and it is the door most pre-record data arrives through. Rule 4 (read old) says that evidence is real and must keep being honoured — analysis rows **are** evidence the media was consumed — but a rebuild that honours it silently cannot tell an owner which files were released on the weaker evidence. **Tag it and disclose it.**

⚠ **An analysis row carrying only the marker key satisfies the row test.** A real transcript row carries `start`, `end` and `text`. The row-key test is `P-segment-processing`'s evidence vocabulary, not retention's — but retention is the reader that acts irreversibly on it.

### 🔴 Two facts about the current boundary that a rebuild must not reproduce

**An image-only segment is releasable with no evidence, and with no sidecar at all.** `resolve_segment_gate` globs `audio.jsonl`, `*_audio.jsonl`, `screen.jsonl`, `*_screen.jsonl` (`retention.py:207-212`) while `is_raw_media` accepts all eight still-image formats (`:57`). The still-image handler writes its sidecar as `<stem>.jsonl` (`observe/depict.py:49`), which matches neither glob, so `has_audio_raw` and `has_video_raw` are both false, both incompleteness checks are skipped, and the verdict is `eligible` with `processed_at = None`. ⚠ The comment at `:216-218` claims `monitor_*_diff.png` diffs *"ride the whole-segment gate"* — true only when audio or video is **also** present. ✅ A predicate that resolves an expected handler **before** reading a record closes this by construction: no handler in the closed set means no obtainable proof, which must never release.

✅ **Resolved in W3a — historical finding:** **The raw-media policy has no runner.** `purge()` had no schedule entry, no maintenance routine and no timer; its only two callers were a CLI command and one HTTP route. So `retention.raw_media: "days"` and `"processed"` were owner-settable, rendered in the owner UI, and **never executed**. ⚠ The two doors also carried **opposite destructive defaults** — `--dry-run` defaulted *false* on the CLI, *true* on the route — and the route required `older_than_days >= 1`, so it could not express the configured policy at all. `purge()` is now removed; both doors call `mark()` against the configured policy and expose neither override nor dry-run mode.

### Retention marks; the owner approves

🔴 A retention proposal is not a removal. Policy and offload passes record exact raw
basenames, byte total, reason, and stable target identity in a durable register;
the owner approves only explicit marked identities. Before a marked release acts,
retention re-reads the segment, re-evaluates current policy and terminal proof,
and releases only files still named by that proposal. A changed, missing, or
no-longer-proven file remains in place and is reported. A staged whole-segment
failure is structured recovery state, not a string convention: the mark records
the staged path only when the receipt carries one, and recovery resolves it after
the set-aside directory is finished.

### Carry forward

🔴 *Read one extraction file strictly enough for irreversible deletion* (`retention.py:110`) — reads at most two lines and treats any `OSError` / `JSONDecodeError` / non-dict as **`"malformed"`, never as "empty, safe to purge"**, with an explicit guard at `:136-139` against a stray marker key making a header-only file look chunk-bearing. Plus `resolve_segment_gate`: `.npz` without `talents/speaker_labels.json` ⇒ incomplete.

✅ **The decision unit is the segment even when the removal unit is the originals.** One blocked file holds the whole segment. `monitor_*_diff.png` files have no extraction record of their own and depend on this (`:216-218`); checked, still relevant.

✅ **Append the removal intent before removing, not after.** The current whole-segment path appends its ledger row before any unlink, so a crash leaves evidence of what was in flight. ⚠ The append primitive can leave a **partial line** on a short write, so a torn intent row must read as *"unknown, go look"* and never as *"nothing happened."*

⚠ **`.npz` gating is defeated by the speaker wipe.** `apps/speakers/wipe.py:76` deletes `chronicle/*/*/*/*.npz` **and** both `speaker_labels.json` paths, so after a wipe the `.npz`-without-labels check cannot fire and previously-held segments become releasable.

⚠ Retention imports `apps/backup/copy` and `think/offload.py` imports back out of retention, so deletion is coupled to backup. ✅ **No cycle** — `apps/backup/copy.py` is a leaf with two imports.

⚠ **Historical at ruling; partly superseded in W3a:** **Two logs this plate writes and never prunes**: `health/retention.log` and `health/pruning-runs/{day}.jsonl`. The log-retention class list covers `chronicle/{day}/health/`, not the journal root's `health/`, so the subsystem that prunes logs does not prune its own. `health/retention.log` no longer has a writer; `health/pruning-runs/{day}.jsonl` still grows from `raw_media_offload` audit records written by offload.

⚠ **Historical at ruling; partly superseded in W3a:** **Two of this plate's writes bypass the journal write discipline** and pass the access lint by not importing the primitives it inspects: `_write_retention_log` has been deleted; `write_prune_audit` still uses a bare `open(path, "a")` (`think/pruning_audit.py:55-56`).

⚠ **Two config keys are dead.** `retention.storage_warning_disk_percent` and `retention.storage_warning_raw_media_gb` are read (`retention.py:413`, `:439`) and written nowhere — no route, CLI, migration or UI. The owner cannot change either.

⚠ **`retention.raw_media` accepts an arbitrary string.** The init-finalize route validates the day count and never checks the mode for membership, and `RetentionPolicy.is_eligible` (`:273-281`) reads an unknown mode as *keep* by falling off the end — it fails closed **by accident**, not by construction.

## `P-web` — 🔴 SPLITS into `P-web-[app]`

**Founder-ruled 2026-08-06: this plate breaks into a set of per-app plates**, `P-web-[app]`, worked through individually. ⏸ **The list of apps is a founder approval and has not been made yet** — ⛔ no `P-web-*` plate exists until it is.

🆕 **The retention approval surface lives in the home app.** When retention has marked something for removal it populates there, with a minimal interface for the owner to choose. ⛔ That is the only owner-facing affordance for approving a removal.

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
